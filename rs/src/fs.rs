use std::path::Path;
use std::sync::Arc;

use crate::batch::Batch;
use crate::error::{Error, Result};
use crate::lock::with_repo_lock;
use crate::store::GitStoreInner;
use crate::tree;
use crate::types::{
    ChangeReport, CommitInfo, FileEntry, FileType, StatResult, WalkDirEntry, WalkEntry, WriteEntry,
    MODE_BLOB, MODE_LINK, MODE_TREE,
};

// ---------------------------------------------------------------------------
// TreeWrite — pub(crate) unit of work for tree rebuilding
// ---------------------------------------------------------------------------

/// A pending write within a tree rebuild.
#[derive(Debug, Clone)]
pub struct TreeWrite {
    pub data: Vec<u8>,
    pub oid: git2::Oid,
    pub mode: u32,
}

// ---------------------------------------------------------------------------
// Option structs
// ---------------------------------------------------------------------------

/// Options for [`Fs::write`], [`Fs::write_text`], [`Fs::write_from_file`],
/// and [`Fs::write_symlink`].
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Git filemode override (e.g. `MODE_BLOB`, `MODE_LINK`). Auto-detected if `None`.
    pub mode: Option<u32>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::apply`].
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Operation prefix for auto-generated commit messages (e.g. `"import"`).
    pub operation: Option<String>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::batch`].
#[derive(Debug, Clone, Default)]
pub struct BatchOptions {
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Operation prefix for auto-generated commit messages (e.g. `"mv"`).
    pub operation: Option<String>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::copy_in`].
#[derive(Debug, Clone)]
pub struct CopyInOptions {
    /// Glob patterns to include. `None` means include all.
    pub include: Option<Vec<String>>,
    /// Glob patterns to exclude. `None` means exclude nothing.
    pub exclude: Option<Vec<String>>,
    /// Gitignore-style exclude filter. When set, files matching the filter
    /// are skipped during disk enumeration. Applied in addition to `exclude`.
    pub exclude_filter: Option<crate::ExcludeFilter>,
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Preview only; when `true` the returned `Fs` is unchanged but the
    /// `ChangeReport` reflects what *would* happen.
    pub dry_run: bool,
    /// Compare by content hash to skip unchanged files (default `true`).
    pub checksum: bool,
    /// Follow symlinks instead of recording them as symlink entries.
    /// When `true`, symlinks are dereferenced and the target content is
    /// stored. When `false` (default), symlinks are preserved.
    pub follow_symlinks: bool,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

impl Default for CopyInOptions {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            exclude_filter: None,
            message: None,
            dry_run: false,
            checksum: true,
            follow_symlinks: false,
            parents: Vec::new(),
        }
    }
}

/// Options for [`Fs::copy_out`].
#[derive(Debug, Clone, Default)]
pub struct CopyOutOptions {
    /// Glob patterns to include. `None` means include all.
    pub include: Option<Vec<String>>,
    /// Glob patterns to exclude. `None` means exclude nothing.
    pub exclude: Option<Vec<String>>,
}

/// Options for [`Fs::sync_in`] and [`Fs::sync_out`].
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Glob patterns to include. `None` means include all.
    pub include: Option<Vec<String>>,
    /// Glob patterns to exclude. `None` means exclude nothing.
    pub exclude: Option<Vec<String>>,
    /// Gitignore-style exclude filter. When set, files matching the filter
    /// are skipped during disk enumeration. Applied in addition to `exclude`.
    pub exclude_filter: Option<crate::ExcludeFilter>,
    /// Commit message (only used by `sync_in`). Auto-generated if `None`.
    pub message: Option<String>,
    /// Preview only; when `true` the store is not modified.
    pub dry_run: bool,
    /// Compare by content hash to skip unchanged files (default `true`).
    pub checksum: bool,
    /// Skip files that fail and continue.
    pub ignore_errors: bool,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            exclude_filter: None,
            message: None,
            dry_run: false,
            ignore_errors: false,
            checksum: true,
            parents: Vec::new(),
        }
    }
}

/// Options for [`Fs::remove`].
#[derive(Debug, Clone, Default)]
pub struct RemoveOptions {
    /// Allow removing directories (and their contents).
    pub recursive: bool,
    /// Preview only; when `true` the store is not modified.
    pub dry_run: bool,
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::remove_from_disk`].
#[derive(Debug, Clone, Default)]
pub struct RemoveFromDiskOptions {
    /// Glob patterns to include. `None` means include all.
    pub include: Option<Vec<String>>,
    /// Glob patterns to exclude. `None` means exclude nothing.
    pub exclude: Option<Vec<String>>,
}

/// Options for [`Fs::move_paths`].
#[derive(Debug, Clone, Default)]
pub struct MoveOptions {
    /// Allow moving directories (and their contents).
    pub recursive: bool,
    /// Preview only; when `true` the store is not modified.
    pub dry_run: bool,
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::copy_from_ref`].
#[derive(Debug, Clone, Default)]
pub struct CopyFromRefOptions {
    /// Remove dest files under the target that are not in the source.
    /// Excluded files (via `ExcludeFilter`) are preserved (rsync behavior).
    pub delete: bool,
    /// Preview only; when `true` the store is not modified but the returned
    /// `Fs` has its `changes` field set.
    pub dry_run: bool,
    /// Commit message. Auto-generated if `None`.
    pub message: Option<String>,
    /// Advisory extra parent commits (e.g. merge parents). These are appended
    /// after the branch tip (first parent) without any tree merging.
    pub parents: Vec<Fs>,
}

/// Options for [`Fs::log`].
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// Maximum number of entries to return.
    pub limit: Option<usize>,
    /// Number of matching entries to skip before collecting results.
    pub skip: Option<usize>,
    /// Only include commits that changed this path.
    pub path: Option<String>,
    /// Only include commits whose message matches this glob pattern (`*`/`?` wildcards).
    pub match_pattern: Option<String>,
    /// Only include commits with timestamp <= this value (seconds since epoch).
    pub before: Option<u64>,
}

// ---------------------------------------------------------------------------
// Fs
// ---------------------------------------------------------------------------

/// An immutable snapshot of a committed tree.
///
/// Read-only when [`writable()`](Fs::writable) returns `false` (tag or
/// detached snapshot). Writable when `true` -- write methods auto-commit and
/// return a **new** `Fs`.
///
/// Cheap to clone (`Arc` internally). No lifetime parameter -- can be stored
/// in structs, returned from functions, sent across threads.
#[derive(Clone, Debug)]
pub struct Fs {
    pub(crate) inner: Arc<GitStoreInner>,
    pub(crate) commit_oid: Option<git2::Oid>,
    pub(crate) tree_oid: Option<git2::Oid>,
    pub(crate) ref_name: Option<String>,
    pub(crate) writable: bool,
    pub(crate) changes: Option<ChangeReport>,
}

impl Fs {
    /// Helper: lock the repo mutex and call `f` with the repository.
    pub(crate) fn with_repo<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&git2::Repository) -> Result<T>,
    {
        let repo = self
            .inner
            .repo
            .lock()
            .map_err(|e| Error::git_msg(e.to_string()))?;
        f(&repo)
    }

    /// The tree OID, or error if there is none.
    fn require_tree(&self) -> Result<git2::Oid> {
        self.tree_oid
            .ok_or_else(|| Error::not_found("no tree in snapshot"))
    }

    /// The 40-character hex SHA of this snapshot's commit, or `None` for an
    /// empty (no-commit) snapshot.
    pub fn commit_hash(&self) -> Option<String> {
        self.commit_oid.map(|oid| oid.to_string())
    }

    /// The 40-character hex SHA of the root tree, or `None` for an empty
    /// (no-tree) snapshot.
    pub fn tree_hash(&self) -> Option<String> {
        self.tree_oid.map(|oid| oid.to_string())
    }

    /// The branch or tag name, or `None` for detached snapshots.
    pub fn ref_name(&self) -> Option<&str> {
        self.ref_name.as_deref()
    }

    /// Whether this snapshot can be written to.
    ///
    /// Returns `true` for branch snapshots, `false` for tags and detached commits.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// Check that this Fs is writable and return the ref name.
    fn require_writable(&self, verb: &str) -> Result<&str> {
        if !self.writable {
            return Err(match &self.ref_name {
                Some(name) => Error::permission(format!("cannot {} read-only snapshot (ref {:?})", verb, name)),
                None => Error::permission(format!("cannot {} read-only snapshot", verb)),
            });
        }
        self.ref_name.as_deref()
            .ok_or_else(|| Error::permission(format!("cannot {} without a branch", verb)))
    }

    /// The commit message, with trailing newline stripped.
    ///
    /// # Errors
    /// Returns an error if there is no commit in this snapshot.
    pub fn message(&self) -> Result<String> {
        let commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;
        self.with_repo(|repo| {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            let msg = commit.message().unwrap_or("");
            Ok(msg.trim_end_matches('\n').to_string())
        })
    }

    /// The commit timestamp as seconds since the Unix epoch.
    ///
    /// # Errors
    /// Returns an error if there is no commit in this snapshot.
    pub fn time(&self) -> Result<u64> {
        let commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;
        self.with_repo(|repo| {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            Ok(commit.time().seconds() as u64)
        })
    }

    /// The commit author's name.
    ///
    /// # Errors
    /// Returns an error if there is no commit in this snapshot.
    pub fn author_name(&self) -> Result<String> {
        let commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;
        self.with_repo(|repo| {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            let name = commit.author().name().unwrap_or("").to_string();
            Ok(name)
        })
    }

    /// The commit author's email address.
    ///
    /// # Errors
    /// Returns an error if there is no commit in this snapshot.
    pub fn author_email(&self) -> Result<String> {
        let commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;
        self.with_repo(|repo| {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            let email = commit.author().email().unwrap_or("").to_string();
            Ok(email)
        })
    }

    /// The change report from the operation that produced this snapshot, if any.
    ///
    /// Set after write, copy, sync, remove, and move operations. `None` for
    /// snapshots obtained directly from a branch or tag.
    pub fn changes(&self) -> Option<&ChangeReport> {
        self.changes.as_ref()
    }

    // -- Read ---------------------------------------------------------------

    /// Read file contents as bytes.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| tree::read_blob_at_path(repo, tree_oid, path))
    }

    /// Read file contents as a UTF-8 string.
    pub fn read_text(&self, path: &str) -> Result<String> {
        let data = self.read(path)?;
        String::from_utf8(data).map_err(|e| Error::git_msg(format!("invalid UTF-8: {}", e)))
    }

    /// List entry names at `path` (or root if empty).
    pub fn ls(&self, path: &str) -> Result<Vec<String>> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            let entries = tree::list_tree_at_path(repo, tree_oid, path)?;
            Ok(entries.into_iter().map(|e| e.name).collect())
        })
    }

    /// List all file paths recursively under `path`.
    ///
    /// Returns a flat list of full relative paths. Directories are not included.
    pub fn ls_recursive(&self, path: &str) -> Result<Vec<String>> {
        let walk = self.walk(path)?;
        let mut result = Vec::new();
        for wde in walk {
            for fe in &wde.files {
                if wde.dirpath.is_empty() {
                    result.push(fe.name.clone());
                } else {
                    result.push(format!("{}/{}", wde.dirpath, fe.name));
                }
            }
        }
        Ok(result)
    }

    /// Recursively walk the tree under `path` (os.walk-style).
    pub fn walk(&self, path: &str) -> Result<Vec<WalkDirEntry>> {
        let tree_oid = self.require_tree()?;
        let path_norm = crate::paths::normalize_path(path)?;

        self.with_repo(|repo| {
            if path_norm.is_empty() {
                tree::walk_tree_dirs(repo, tree_oid)
            } else {
                let entry = tree::entry_at_path(repo, tree_oid, &path_norm)?
                    .ok_or_else(|| Error::not_found(&path_norm))?;
                if entry.mode != MODE_TREE {
                    return Err(Error::not_a_directory(&path_norm));
                }
                let mut entries = tree::walk_tree_dirs(repo, entry.oid)?;
                for e in &mut entries {
                    if e.dirpath.is_empty() {
                        e.dirpath = path_norm.clone();
                    } else {
                        e.dirpath = format!("{}/{}", path_norm, e.dirpath);
                    }
                }
                Ok(entries)
            }
        })
    }

    /// Return `true` if `path` exists (file, directory, or symlink).
    pub fn exists(&self, path: &str) -> Result<bool> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| tree::exists_at_path(repo, tree_oid, path))
    }

    /// Return `true` if `path` is a directory (tree) in the repo.
    pub fn is_dir(&self, path: &str) -> Result<bool> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            match tree::entry_at_path(repo, tree_oid, path)? {
                Some(entry) => Ok(entry.mode == MODE_TREE),
                None => Ok(false),
            }
        })
    }

    /// Return the [`FileType`] of `path`.
    pub fn file_type(&self, path: &str) -> Result<FileType> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            let entry = tree::entry_at_path(repo, tree_oid, path)?
                .ok_or_else(|| Error::not_found(path))?;
            FileType::from_mode(entry.mode)
                .ok_or_else(|| Error::git_msg(format!("unknown mode: {:#o}", entry.mode)))
        })
    }

    /// Return the size in bytes of the object at `path`.
    pub fn size(&self, path: &str) -> Result<u64> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            let entry = tree::entry_at_path(repo, tree_oid, path)?
                .ok_or_else(|| Error::not_found(path))?;
            if entry.mode == MODE_TREE {
                return Err(Error::is_a_directory(path));
            }
            let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
            Ok(blob.content().len() as u64)
        })
    }

    /// Return the 40-character hex SHA of the object at `path`.
    pub fn object_hash(&self, path: &str) -> Result<String> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            let entry = tree::entry_at_path(repo, tree_oid, path)?
                .ok_or_else(|| Error::not_found(path))?;
            Ok(entry.oid.to_string())
        })
    }

    /// Read the target of a symlink at `path`.
    pub fn readlink(&self, path: &str) -> Result<String> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| {
            let entry = tree::entry_at_path(repo, tree_oid, path)?
                .ok_or_else(|| Error::not_found(path))?;
            if entry.mode != MODE_LINK {
                return Err(Error::invalid_path(format!(
                    "{} is not a symlink",
                    path
                )));
            }
            let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
            String::from_utf8(blob.content().to_vec())
                .map_err(|e| Error::git_msg(format!("invalid UTF-8 in symlink: {}", e)))
        })
    }

    // -- FUSE-readiness API -------------------------------------------------

    /// Return a [`StatResult`] for `path` (pass `""` for the root).
    pub fn stat(&self, path: &str) -> Result<StatResult> {
        let tree_oid = self.require_tree()?;
        let mtime = self.time()?;

        self.with_repo(|repo| {
            let path_norm = crate::paths::normalize_path(path)?;

            if path_norm.is_empty() {
                let nlink = 2 + tree::count_subdirs(repo, tree_oid)?;
                return Ok(StatResult {
                    mode: MODE_TREE,
                    file_type: FileType::Tree,
                    size: 0,
                    hash: tree_oid.to_string(),
                    nlink,
                    mtime,
                });
            }

            let entry = tree::entry_at_path(repo, tree_oid, &path_norm)?
                .ok_or_else(|| Error::not_found(&path_norm))?;
            let ft = FileType::from_mode(entry.mode)
                .ok_or_else(|| Error::git_msg(format!("unknown mode: {:#o}", entry.mode)))?;

            if entry.mode == MODE_TREE {
                let nlink = 2 + tree::count_subdirs(repo, entry.oid)?;
                Ok(StatResult {
                    mode: entry.mode,
                    file_type: ft,
                    size: 0,
                    hash: entry.oid.to_string(),
                    nlink,
                    mtime,
                })
            } else {
                let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
                Ok(StatResult {
                    mode: entry.mode,
                    file_type: ft,
                    size: blob.content().len() as u64,
                    hash: entry.oid.to_string(),
                    nlink: 1,
                    mtime,
                })
            }
        })
    }

    /// List directory entries with name, OID, and mode.
    pub fn listdir(&self, path: &str) -> Result<Vec<WalkEntry>> {
        let tree_oid = self.require_tree()?;
        self.with_repo(|repo| tree::list_tree_at_path(repo, tree_oid, path))
    }

    /// Read file contents as bytes with optional offset and size.
    pub fn read_range(&self, path: &str, offset: usize, size: Option<usize>) -> Result<Vec<u8>> {
        let data = self.read(path)?;
        let start = offset.min(data.len());
        let end = match size {
            Some(s) => start.saturating_add(s).min(data.len()),
            None => data.len(),
        };
        Ok(data[start..end].to_vec())
    }

    /// Read raw blob data by its hex hash, bypassing tree lookup.
    pub fn read_by_hash(
        &self,
        hash: &str,
        offset: usize,
        size: Option<usize>,
    ) -> Result<Vec<u8>> {
        let oid = git2::Oid::from_str(hash)
            .map_err(|e| Error::git_msg(format!("invalid hash: {}", e)))?;
        self.with_repo(|repo| {
            let blob = repo.find_blob(oid).map_err(Error::git)?;
            let data = blob.content();
            let start = offset.min(data.len());
            let end = match size {
                Some(s) => start.saturating_add(s).min(data.len()),
                None => data.len(),
            };
            Ok(data[start..end].to_vec())
        })
    }

    // -- Glob ---------------------------------------------------------------

    /// Expand a glob pattern against the repo tree.
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let mut paths = self.iglob(pattern)?;
        paths.sort();
        Ok(paths)
    }

    /// Expand a glob pattern against the repo tree (unsorted).
    pub fn iglob(&self, pattern: &str) -> Result<Vec<String>> {
        let tree_oid = self.require_tree()?;
        let segments: Vec<&str> = pattern.split('/').collect();

        self.with_repo(|repo| {
            let mut results = Vec::new();
            iglob_recursive(repo, tree_oid, &segments, "", &mut results)?;
            Ok(results)
        })
    }

    // -- Write --------------------------------------------------------------

    /// Write `data` to `path` and commit, returning a new [`Fs`].
    pub fn write(
        &self,
        path: &str,
        data: &[u8],
        opts: WriteOptions,
    ) -> Result<Fs> {
        let path = crate::paths::normalize_path(path)?;
        let mode = opts.mode.unwrap_or(MODE_BLOB);
        let message = opts
            .message
            .unwrap_or_else(|| crate::paths::format_commit_message("write", Some(&path)));
        let extra: Vec<&Fs> = opts.parents.iter().collect();

        let tw = self.with_repo(|repo| {
            let blob_oid = repo.blob(data).map_err(Error::git)?;
            Ok(TreeWrite {
                data: data.to_vec(),
                oid: blob_oid,
                mode,
            })
        })?;

        let writes = vec![(path, Some(tw))];
        self.commit_changes_with_parents(&writes, &message, &extra)
    }

    /// Write `text` to `path` and commit, returning a new [`Fs`].
    pub fn write_text(
        &self,
        path: &str,
        text: &str,
        opts: WriteOptions,
    ) -> Result<Fs> {
        self.write(path, text.as_bytes(), opts)
    }

    /// Write a local file into the repo and commit, returning a new [`Fs`].
    pub fn write_from_file(
        &self,
        path: &str,
        src: &Path,
        opts: WriteOptions,
    ) -> Result<Fs> {
        let data = std::fs::read(src).map_err(|e| Error::io(src, e))?;
        let mode = opts
            .mode
            .unwrap_or_else(|| tree::mode_from_disk(src).unwrap_or(MODE_BLOB));
        let opts = WriteOptions {
            mode: Some(mode),
            ..opts
        };
        self.write(path, &data, opts)
    }

    /// Create a symbolic link entry and commit, returning a new [`Fs`].
    pub fn write_symlink(
        &self,
        path: &str,
        target: &str,
        opts: WriteOptions,
    ) -> Result<Fs> {
        let opts = WriteOptions {
            mode: Some(MODE_LINK),
            ..opts
        };
        self.write(path, target.as_bytes(), opts)
    }

    /// Apply multiple writes and removes in a single atomic commit.
    pub fn apply(
        &self,
        entries: &[(&str, WriteEntry)],
        removes: &[&str],
        opts: ApplyOptions,
    ) -> Result<Fs> {
        let mut writes = Vec::new();
        for (path, entry) in entries {
            entry.validate()?;
            let path = crate::paths::normalize_path(path)?;
            let (data, mode) = if entry.mode == MODE_LINK {
                (
                    entry.target.as_ref().unwrap().as_bytes().to_vec(),
                    MODE_LINK,
                )
            } else {
                (entry.data.as_ref().unwrap().clone(), entry.mode)
            };
            let tw = self.with_repo(|repo| {
                let blob_oid = repo.blob(&data).map_err(Error::git)?;
                Ok(TreeWrite {
                    data,
                    oid: blob_oid,
                    mode,
                })
            })?;
            writes.push((path, Some(tw)));
        }

        for path in removes {
            let path = crate::paths::normalize_path(path)?;
            writes.push((path, None));
        }

        let op = opts.operation.as_deref().unwrap_or("apply");
        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let message = opts
            .message
            .unwrap_or_else(|| crate::paths::format_commit_message(op, None));
        self.commit_changes_with_parents(&writes, &message, &extra)
    }

    /// Return a [`Batch`] for accumulating multiple writes in one commit.
    pub fn batch(&self, opts: BatchOptions) -> Batch {
        Batch {
            fs: self.clone(),
            writes: vec![],
            removes: vec![],
            message: opts.message,
            operation: opts.operation,
            parents: opts.parents,
            closed: false,
        }
    }

    /// Return a buffered [`FsWriter`](crate::fileobj::FsWriter) that commits on close.
    pub fn writer(&self, path: &str) -> Result<crate::fileobj::FsWriter> {
        self.require_writable("write to")?;
        let normalized = crate::paths::normalize_path(path)?;
        Ok(crate::fileobj::FsWriter::new(self.clone(), normalized))
    }

    // -- Copy / sync --------------------------------------------------------

    /// Copy local files from disk into the repo.
    pub fn copy_in(
        &self,
        sources: &[&str],
        dest: &str,
        opts: CopyInOptions,
    ) -> Result<(ChangeReport, Fs)> {
        let tree_oid = self.require_tree()?;
        let checksum = opts.checksum;
        let follow_symlinks = opts.follow_symlinks;
        let exclude_filter = opts.exclude_filter;
        let commit_time = if !checksum { self.time().ok() } else { None };
        let (writes, report) = self.with_repo(|repo| {
            let inc: Option<Vec<&str>> = opts.include.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            let exc: Option<Vec<&str>> = opts.exclude.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            crate::copy::copy_in_multi(repo, tree_oid, sources, dest, inc.as_deref(), exc.as_deref(), checksum, commit_time, follow_symlinks)
        })?;
        let writes: Vec<_> = if let Some(ref ef) = exclude_filter {
            if ef.active() {
                writes.into_iter().filter(|(p, _)| !ef.is_excluded(p, false)).collect()
            } else {
                writes
            }
        } else {
            writes
        };
        if opts.dry_run {
            return Ok((report, self.clone()));
        }
        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let new_fs = if !writes.is_empty() {
            let tw_writes: Vec<(String, Option<TreeWrite>)> = writes
                .into_iter()
                .map(|(p, tw)| (p, Some(tw)))
                .collect();
            let msg = opts.message.unwrap_or_else(|| crate::paths::format_commit_message("copy_in", None));
            self.commit_changes_with_parents(&tw_writes, &msg, &extra)?
        } else {
            self.clone()
        };
        Ok((report, new_fs))
    }

    /// Copy repo files to local disk.
    pub fn copy_out(
        &self,
        sources: &[&str],
        dest: &str,
        opts: CopyOutOptions,
    ) -> Result<ChangeReport> {
        let tree_oid = self.require_tree()?;
        let commit_time = self.time().ok();
        self.with_repo(|repo| {
            let inc: Option<Vec<&str>> = opts.include.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            let exc: Option<Vec<&str>> = opts.exclude.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            crate::copy::copy_out_multi(repo, tree_oid, sources, dest, inc.as_deref(), exc.as_deref(), commit_time)
        })
    }

    /// Make `dest` in the repo identical to the local `src` directory.
    pub fn sync_in(
        &self,
        src: &str,
        dest: &str,
        opts: SyncOptions,
    ) -> Result<(ChangeReport, Fs)> {
        let tree_oid = self.require_tree()?;
        let checksum = opts.checksum;
        let mut exclude_filter = opts.exclude_filter;
        let commit_time = if !checksum { self.time().ok() } else { None };
        let src_path = Path::new(src);
        let (writes, report) = self.with_repo(|repo| {
            let inc: Option<Vec<&str>> = opts.include.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            let exc: Option<Vec<&str>> = opts.exclude.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            crate::copy::sync_in(repo, tree_oid, src_path, dest, inc.as_deref(), exc.as_deref(), checksum, commit_time, &mut exclude_filter, opts.ignore_errors)
        })?;
        let writes: Vec<_> = if let Some(ref ef) = exclude_filter {
            if ef.active() {
                writes.into_iter().filter(|(p, tw)| tw.is_none() || !ef.is_excluded(p, false)).collect()
            } else {
                writes
            }
        } else {
            writes
        };
        if opts.dry_run {
            return Ok((report, self.clone()));
        }
        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let new_fs = if !writes.is_empty() {
            let msg = opts.message.unwrap_or_else(|| crate::paths::format_commit_message("sync_in", None));
            self.commit_changes_with_parents(&writes, &msg, &extra)?
        } else {
            self.clone()
        };
        Ok((report, new_fs))
    }

    /// Make the local `dest` directory identical to `src` in the repo.
    pub fn sync_out(
        &self,
        src: &str,
        dest: &str,
        opts: SyncOptions,
    ) -> Result<ChangeReport> {
        let tree_oid = self.require_tree()?;
        let checksum = opts.checksum;
        let commit_time = self.time().ok();
        let dest_path = Path::new(dest);
        self.with_repo(|repo| {
            let inc: Option<Vec<&str>> = opts.include.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            let exc: Option<Vec<&str>> = opts.exclude.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
            crate::copy::sync_out(repo, tree_oid, src, dest_path, inc.as_deref(), exc.as_deref(), checksum, commit_time)
        })
    }

    /// Remove files from local disk that match the include/exclude filters.
    pub fn remove_from_disk(
        &self,
        path: &Path,
        opts: RemoveFromDiskOptions,
    ) -> Result<ChangeReport> {
        let inc: Option<Vec<&str>> = opts.include.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
        let exc: Option<Vec<&str>> = opts.exclude.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
        crate::copy::remove_from_disk(path, inc.as_deref(), exc.as_deref())
    }

    /// Remove files from the repo and commit, returning a new [`Fs`].
    pub fn remove(
        &self,
        sources: &[&str],
        opts: RemoveOptions,
    ) -> Result<Fs> {
        let tree_oid = self.require_tree()?;
        let mut writes: Vec<(String, Option<TreeWrite>)> = Vec::new();
        let mut report = ChangeReport::new();

        self.with_repo(|repo| {
            for src in sources {
                let path = crate::paths::normalize_path(src)?;
                let entry = tree::entry_at_path(repo, tree_oid, &path)?
                    .ok_or_else(|| Error::not_found(&path))?;

                if entry.mode == MODE_TREE {
                    if !opts.recursive {
                        return Err(Error::is_a_directory(&path));
                    }
                    let sub_entries = tree::walk_tree(repo, entry.oid)?;
                    for (rel_path, we) in &sub_entries {
                        let full_path = format!("{}/{}", path, rel_path);
                        let ft = FileType::from_mode(we.mode).unwrap_or(FileType::Blob);
                        if !opts.dry_run {
                            writes.push((full_path.clone(), None));
                        }
                        report.delete.push(FileEntry::new(&full_path, ft));
                    }
                } else {
                    let ft = FileType::from_mode(entry.mode).unwrap_or(FileType::Blob);
                    if !opts.dry_run {
                        writes.push((path.clone(), None));
                    }
                    report.delete.push(FileEntry::new(&path, ft));
                }
            }
            Ok(())
        })?;

        if opts.dry_run || writes.is_empty() {
            let mut fs = self.clone();
            fs.changes = Some(report);
            return Ok(fs);
        }

        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let msg = opts.message.unwrap_or_else(|| {
            crate::paths::format_commit_message("remove", None)
        });
        let mut new_fs = self.commit_changes_with_parents(&writes, &msg, &extra)?;
        new_fs.changes = Some(report);
        Ok(new_fs)
    }

    /// Rename a single path within the repo and commit, returning a new [`Fs`].
    pub fn rename(
        &self,
        src: &str,
        dest: &str,
        opts: WriteOptions,
    ) -> Result<Fs> {
        let tree_oid = self.require_tree()?;
        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let writes = self.with_repo(|repo| crate::copy::rename(repo, tree_oid, src, dest))?;
        if !writes.is_empty() {
            let msg = opts.message.unwrap_or_else(|| {
                crate::paths::format_commit_message("rename", Some(&format!("{} -> {}", src, dest)))
            });
            self.commit_changes_with_parents(&writes, &msg, &extra)
        } else {
            Ok(self.clone())
        }
    }

    /// Move or rename files within the repo, following POSIX `mv` semantics.
    pub fn move_paths(
        &self,
        sources: &[&str],
        dest: &str,
        opts: MoveOptions,
    ) -> Result<Fs> {
        let tree_oid = self.require_tree()?;
        let dest_norm = crate::paths::normalize_path(dest)?;

        let dest_is_dir = self.with_repo(|repo| {
            match tree::entry_at_path(repo, tree_oid, &dest_norm)? {
                Some(entry) => Ok(entry.mode == MODE_TREE),
                None => Ok(false),
            }
        })?;

        if sources.len() > 1 && !dest_is_dir {
            return Err(Error::not_a_directory(&dest_norm));
        }

        let mut all_writes: Vec<(String, Option<TreeWrite>)> = Vec::new();

        self.with_repo(|repo| {
            for src in sources {
                let src_norm = crate::paths::normalize_path(src)?;
                let entry = tree::entry_at_path(repo, tree_oid, &src_norm)?
                    .ok_or_else(|| Error::not_found(&src_norm))?;

                let final_dest = if dest_is_dir {
                    let basename = src_norm.rsplit('/').next().unwrap_or(&src_norm);
                    format!("{}/{}", dest_norm, basename)
                } else {
                    dest_norm.clone()
                };

                if entry.mode == MODE_TREE {
                    if !opts.recursive {
                        return Err(Error::is_a_directory(&src_norm));
                    }
                    let sub_entries = tree::walk_tree(repo, entry.oid)?;
                    for (rel_path, we) in &sub_entries {
                        let old_path = format!("{}/{}", src_norm, rel_path);
                        let new_path = format!("{}/{}", final_dest, rel_path);
                        let blob = repo.find_blob(we.oid).map_err(Error::git)?;
                        all_writes.push((old_path, None));
                        all_writes.push((
                            new_path,
                            Some(TreeWrite {
                                data: blob.content().to_vec(),
                                oid: we.oid,
                                mode: we.mode,
                            }),
                        ));
                    }
                } else {
                    let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
                    all_writes.push((src_norm, None));
                    all_writes.push((
                        final_dest,
                        Some(TreeWrite {
                            data: blob.content().to_vec(),
                            oid: entry.oid,
                            mode: entry.mode,
                        }),
                    ));
                }
            }
            Ok(())
        })?;

        if opts.dry_run || all_writes.is_empty() {
            return Ok(self.clone());
        }

        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let msg = opts.message.unwrap_or_else(|| {
            crate::paths::format_commit_message("move", None)
        });
        self.commit_changes_with_parents(&all_writes, &msg, &extra)
    }

    /// Copy files from another branch, tag, or detached commit into this
    /// branch in a single atomic commit.
    pub fn copy_from_ref(
        &self,
        source: &Fs,
        sources: &[&str],
        dest: &str,
        opts: CopyFromRefOptions,
    ) -> Result<Fs> {
        self.require_writable("write to")?;

        let same = Arc::ptr_eq(&self.inner, &source.inner) || {
            let self_canon = std::fs::canonicalize(&self.inner.path).ok();
            let src_canon = std::fs::canonicalize(&source.inner.path).ok();
            self_canon.is_some() && self_canon == src_canon
        };
        if !same {
            return Err(Error::invalid_path(
                "source must belong to the same repo as self".to_string(),
            ));
        }

        let dest_norm = crate::paths::normalize_path(dest)?;
        let src_tree = source.require_tree()?;
        let dest_tree = self.require_tree()?;

        let mut src_mapped = std::collections::BTreeMap::<String, (git2::Oid, u32)>::new();
        let mut dest_prefixes = std::collections::BTreeSet::<String>::new();

        self.with_repo(|repo| {
            for &src in sources {
                let contents_mode = src.ends_with('/');
                let stripped = src.trim_end_matches('/');
                let normalized = if stripped.is_empty() {
                    String::new()
                } else {
                    crate::paths::normalize_path(stripped)?
                };

                enum SrcMode { File(git2::Oid, u32), Dir, Contents }

                let mode = if contents_mode {
                    if !normalized.is_empty() {
                        let entry = tree::entry_at_path(repo, src_tree, &normalized)?;
                        match entry {
                            Some(e) if e.mode == MODE_TREE => {},
                            Some(_) => return Err(Error::not_a_directory(
                                format!("Not a directory in repo: {}", normalized),
                            )),
                            None => return Err(Error::not_found(
                                format!("File not found in repo: {}", normalized),
                            )),
                        }
                    }
                    SrcMode::Contents
                } else if normalized.is_empty() {
                    SrcMode::Contents
                } else {
                    let entry = tree::entry_at_path(repo, src_tree, &normalized)?;
                    match entry {
                        Some(e) if e.mode == MODE_TREE => SrcMode::Dir,
                        Some(e) => SrcMode::File(e.oid, e.mode),
                        None => return Err(Error::not_found(
                            format!("File not found in repo: {}", normalized),
                        )),
                    }
                };

                match mode {
                    SrcMode::File(oid, fmode) => {
                        let name = normalized.rsplit('/').next().unwrap_or(&normalized);
                        let dest_file = if dest_norm.is_empty() {
                            name.to_string()
                        } else {
                            format!("{}/{}", dest_norm, name)
                        };
                        src_mapped.insert(dest_file, (oid, fmode));
                        dest_prefixes.insert(dest_norm.clone());
                    }
                    SrcMode::Dir => {
                        let dirname = normalized.rsplit('/').next().unwrap_or(&normalized);
                        let target = if dest_norm.is_empty() {
                            dirname.to_string()
                        } else {
                            format!("{}/{}", dest_norm, dirname)
                        };
                        let entries = walk_subtree(repo, src_tree, &normalized)?;
                        for (rel, (oid, fmode)) in entries {
                            let dest_file = format!("{}/{}", target, rel);
                            src_mapped.insert(dest_file, (oid, fmode));
                        }
                        dest_prefixes.insert(target);
                    }
                    SrcMode::Contents => {
                        let entries = walk_subtree(repo, src_tree, &normalized)?;
                        for (rel, (oid, fmode)) in entries {
                            let dest_file = if dest_norm.is_empty() {
                                rel
                            } else {
                                format!("{}/{}", dest_norm, rel)
                            };
                            src_mapped.insert(dest_file, (oid, fmode));
                        }
                        dest_prefixes.insert(dest_norm.clone());
                    }
                }
            }
            Ok(())
        })?;

        let dest_files = self.with_repo(|repo| {
            let mut dest_files = std::collections::BTreeMap::<String, (git2::Oid, u32)>::new();
            for dp in &dest_prefixes {
                let walked = walk_subtree(repo, dest_tree, dp)?;
                for (rel, entry) in walked {
                    let full = if dp.is_empty() {
                        rel
                    } else {
                        format!("{}/{}", dp, rel)
                    };
                    dest_files.insert(full, entry);
                }
            }
            Ok(dest_files)
        })?;

        let mut writes: Vec<(String, Option<TreeWrite>)> = Vec::new();
        let mut report = ChangeReport::new();

        for (dest_path, (src_oid, src_mode)) in &src_mapped {
            let dest_entry = dest_files.get(dest_path);
            match dest_entry {
                None => {
                    let ft = FileType::from_mode(*src_mode).unwrap_or(FileType::Blob);
                    report.add.push(FileEntry::new(dest_path, ft));
                    writes.push((
                        dest_path.clone(),
                        Some(TreeWrite {
                            data: vec![],
                            oid: *src_oid,
                            mode: *src_mode,
                        }),
                    ));
                }
                Some((d_oid, d_mode)) if d_oid != src_oid || d_mode != src_mode => {
                    let ft = FileType::from_mode(*src_mode).unwrap_or(FileType::Blob);
                    report.update.push(FileEntry::new(dest_path, ft));
                    writes.push((
                        dest_path.clone(),
                        Some(TreeWrite {
                            data: vec![],
                            oid: *src_oid,
                            mode: *src_mode,
                        }),
                    ));
                }
                _ => {}
            }
        }

        if opts.delete {
            for (full, (_, mode)) in &dest_files {
                if !src_mapped.contains_key(full) {
                    let ft = FileType::from_mode(*mode).unwrap_or(FileType::Blob);
                    report.delete.push(FileEntry::new(full, ft));
                    writes.push((full.clone(), None));
                }
            }
        }

        if opts.dry_run || writes.is_empty() {
            let mut fs = self.clone();
            fs.changes = Some(report);
            return Ok(fs);
        }

        let extra: Vec<&Fs> = opts.parents.iter().collect();
        let msg = opts.message.unwrap_or_else(|| {
            crate::paths::format_commit_message("cp", None)
        });
        let mut new_fs = self.commit_changes_with_parents(&writes, &msg, &extra)?;
        new_fs.changes = Some(report);
        Ok(new_fs)
    }

    // -- History ------------------------------------------------------------

    /// The parent snapshot, or `None` for the initial commit.
    pub fn parent(&self) -> Result<Option<Fs>> {
        let commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;

        self.with_repo(|repo| {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            if commit.parent_count() > 0 {
                Ok(Some(commit.parent_id(0).map_err(Error::git)?))
            } else {
                Ok(None)
            }
        })?
        .map(|parent_id| {
            Fs::from_commit(Arc::clone(&self.inner), parent_id, self.ref_name.clone(), Some(self.writable))
        })
        .transpose()
    }

    /// Return an `Fs` for the given commit hash, inheriting this snapshot's
    /// ref name and writability.
    ///
    /// This is useful for navigating to a specific commit (e.g. from a log
    /// entry) while preserving the branch context.
    ///
    /// # Errors
    /// Returns [`Error::InvalidHash`] if the hash is not valid hex, or
    /// [`Error::NotFound`] if the commit does not exist.
    pub fn at_commit(&self, hash: &str) -> Result<Fs> {
        let oid = git2::Oid::from_str(hash)
            .map_err(|_| Error::invalid_hash(hash))?;
        Fs::from_commit(
            Arc::clone(&self.inner),
            oid,
            self.ref_name.clone(),
            Some(self.writable),
        )
    }

    /// Return the `Fs` at the *n*-th ancestor commit.
    pub fn back(&self, n: usize) -> Result<Fs> {
        let mut current = self.clone();
        for _ in 0..n {
            match current.parent()? {
                Some(parent) => current = parent,
                None => {
                    return Err(Error::not_found("not enough history"));
                }
            }
        }
        Ok(current)
    }

    /// Move the branch pointer back `n` commits (soft reset).
    pub fn undo(&self, n: usize) -> Result<Fs> {
        let branch = self.require_writable("undo")?;

        let mut target = self.clone();
        for _ in 0..n {
            target = target
                .parent()?
                .ok_or_else(|| Error::not_found("no parent commit to undo to"))?;
        }

        let target_oid = target
            .commit_oid
            .ok_or_else(|| Error::not_found("target has no commit"))?;

        let refname = format!("refs/heads/{}", branch);
        let inner = Arc::clone(&self.inner);

        let current_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;

        with_repo_lock(&inner.path, || {
            let repo = inner
                .repo
                .lock()
                .map_err(|e| Error::git_msg(e.to_string()))?;

            // Stale snapshot check
            let current_ref = repo
                .find_reference(&refname)
                .map_err(|_| Error::not_found(format!("branch '{}' not found", branch)))?;
            let actual_oid = current_ref.target()
                .ok_or_else(|| Error::git_msg("symbolic reference unexpected"))?;
            if actual_oid != current_oid {
                return Err(Error::stale_snapshot(format!(
                    "branch '{}' has moved: expected {}, found {}",
                    branch, current_oid, actual_oid
                )));
            }

            repo.reference(&refname, target_oid, true, "undo: move back")
                .map_err(Error::git)?;

            // Write reflog entry for undo
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = crate::reflog::write_reflog_entry(
                &inner.path,
                &refname,
                &crate::types::ReflogEntry {
                    old_sha: current_oid.to_string(),
                    new_sha: target_oid.to_string(),
                    committer: format!(
                        "{} <{}>",
                        inner.signature.name, inner.signature.email
                    ),
                    timestamp: now.as_secs(),
                    message: "undo: move back".to_string(),
                },
            );

            Ok(())
        })?;

        Ok(target)
    }

    /// Move the branch pointer forward `n` steps using the reflog.
    pub fn redo(&self, n: usize) -> Result<Fs> {
        let branch = self.require_writable("redo")?;
        let refname = format!("refs/heads/{}", branch);

        let current_hex = self
            .commit_oid
            .map(|oid| oid.to_string())
            .unwrap_or_default();

        let current_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;

        let reflog_entries = crate::reflog::read_reflog(&self.inner.path, &refname)?;

        let mut forward_sha = current_hex.clone();
        for _ in 0..n {
            let next = reflog_entries
                .iter()
                .rev()
                .find(|e| e.new_sha == forward_sha)
                .map(|e| e.old_sha.clone())
                .ok_or_else(|| Error::not_found("no redo target found in reflog"))?;
            forward_sha = next;
        }

        let forward_oid = git2::Oid::from_str(&forward_sha)
            .map_err(|e| Error::git_msg(format!("invalid oid: {}", e)))?;

        let inner = Arc::clone(&self.inner);

        with_repo_lock(&inner.path, || {
            let repo = inner
                .repo
                .lock()
                .map_err(|e| Error::git_msg(e.to_string()))?;

            // Stale snapshot check
            let current_ref = repo
                .find_reference(&refname)
                .map_err(|_| Error::not_found(format!("branch '{}' not found", branch)))?;
            let actual_oid = current_ref.target()
                .ok_or_else(|| Error::git_msg("symbolic reference unexpected"))?;
            if actual_oid != current_oid {
                return Err(Error::stale_snapshot(format!(
                    "branch '{}' has moved: expected {}, found {}",
                    branch, current_oid, actual_oid
                )));
            }

            repo.reference(&refname, forward_oid, true, "redo: move forward")
                .map_err(Error::git)?;

            // Write reflog entry for redo
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = crate::reflog::write_reflog_entry(
                &inner.path,
                &refname,
                &crate::types::ReflogEntry {
                    old_sha: current_hex.clone(),
                    new_sha: forward_sha.clone(),
                    committer: format!(
                        "{} <{}>",
                        inner.signature.name, inner.signature.email
                    ),
                    timestamp: now.as_secs(),
                    message: "redo: move forward".to_string(),
                },
            );

            Ok(())
        })?;

        Fs::from_commit(inner, forward_oid, self.ref_name.clone(), Some(self.writable))
    }

    /// Walk the commit history, returning [`CommitInfo`] entries.
    pub fn log(&self, opts: LogOptions) -> Result<Vec<CommitInfo>> {
        let mut commit_oid = self
            .commit_oid
            .ok_or_else(|| Error::not_found("no commit in snapshot"))?;

        let skip = opts.skip.unwrap_or(0);
        let limit = opts.limit.unwrap_or(usize::MAX);
        let filter_path = opts.path.as_deref().map(crate::paths::normalize_path).transpose()?;
        let match_pattern = opts.match_pattern.as_deref();
        let before = opts.before;

        let repo = self
            .inner
            .repo
            .lock()
            .map_err(|e| Error::git_msg(e.to_string()))?;

        let mut results = Vec::new();
        let mut matched = 0usize;

        loop {
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;

            let timestamp = commit.time().seconds() as u64;
            let message = commit.message().unwrap_or("").to_string();
            let tree_oid = commit.tree_id();
            let parent_oid = if commit.parent_count() > 0 {
                Some(commit.parent_id(0).map_err(Error::git)?)
            } else {
                None
            };

            let mut include = true;

            if let Some(cutoff) = before {
                if timestamp > cutoff {
                    include = false;
                }
            }

            if include {
                if let Some(pat) = match_pattern {
                    if !crate::glob::glob_match(pat, &message) {
                        include = false;
                    }
                }
            }

            if include {
                if let Some(ref filter) = filter_path {
                    let this_entry = tree::entry_at_path(&repo, tree_oid, filter)?;
                    let parent_entry = if let Some(pid) = parent_oid {
                        let parent_commit = repo.find_commit(pid).map_err(Error::git)?;
                        let parent_tree = parent_commit.tree_id();
                        tree::entry_at_path(&repo, parent_tree, filter)?
                    } else {
                        None
                    };

                    let same = match (&this_entry, &parent_entry) {
                        (Some(a), Some(b)) => a.oid == b.oid && a.mode == b.mode,
                        (None, None) => true,
                        _ => false,
                    };
                    if same {
                        include = false;
                    }
                }
            }

            if include {
                matched += 1;
                if matched > skip {
                    results.push(CommitInfo {
                        commit_hash: commit_oid.to_string(),
                        message,
                        time: Some(timestamp),
                        author_name: Some(commit.author().name().unwrap_or("").to_string()),
                        author_email: Some(commit.author().email().unwrap_or("").to_string()),
                    });
                }
            }

            if results.len() >= limit {
                break;
            }

            match parent_oid {
                Some(parent) => commit_oid = parent,
                None => break,
            }
        }

        Ok(results)
    }

    /// Create a new commit with this snapshot's tree but no history.
    ///
    /// Returns a detached (read-only) `Fs` pointing at the new commit.
    ///
    /// # Arguments
    /// * `parent` - Optional parent Fs. Creates a root commit if `None`.
    /// * `message` - Commit message (default: `"squash"`).
    pub fn squash(&self, parent: Option<&Fs>, message: Option<&str>) -> Result<Fs> {
        let msg = message.unwrap_or("squash");
        let tree_oid = self
            .tree_oid
            .ok_or_else(|| Error::git_msg("no tree"))?;

        let new_oid = {
            let repo = self
                .inner
                .repo
                .lock()
                .map_err(|e| Error::git_msg(e.to_string()))?;

            let tree = repo.find_tree(tree_oid).map_err(Error::git)?;
            let sig = git2::Signature::now(
                &self.inner.signature.name,
                &self.inner.signature.email,
            )
            .map_err(Error::git)?;

            let parent_commit = match parent {
                Some(p) => {
                    let oid = p
                        .commit_oid
                        .ok_or_else(|| Error::git_msg("parent has no commit"))?;
                    Some(repo.find_commit(oid).map_err(Error::git)?)
                }
                None => None,
            };
            let parents: Vec<&git2::Commit> = parent_commit.iter().collect();

            repo.commit(None, &sig, &sig, msg, &tree, &parents)
                .map_err(Error::git)?
        };

        Fs::from_commit(Arc::clone(&self.inner), new_oid, None, Some(false))
    }

    // -- Internal -----------------------------------------------------------

    /// Build an `Fs` from a known commit oid.
    pub(crate) fn from_commit(
        inner: Arc<GitStoreInner>,
        commit_oid: git2::Oid,
        ref_name: Option<String>,
        writable: Option<bool>,
    ) -> Result<Self> {
        let writable = writable.unwrap_or(ref_name.is_some());
        let tree_oid = {
            let repo = inner
                .repo
                .lock()
                .map_err(|e| Error::git_msg(e.to_string()))?;
            let commit = repo.find_commit(commit_oid).map_err(Error::git)?;
            commit.tree_id()
        };

        Ok(Fs {
            inner,
            commit_oid: Some(commit_oid),
            tree_oid: Some(tree_oid),
            ref_name,
            writable,
            changes: None,
        })
    }

    /// Commit accumulated changes and return the new `Fs` snapshot.
    ///
    /// `extra_parents` are advisory parent commits (e.g. merge parents) whose
    /// OIDs are appended after the branch tip (first parent). No tree merging
    /// is performed — the tree is built solely from `writes`.
    #[allow(dead_code)]
    pub(crate) fn commit_changes(
        &self,
        writes: &[(String, Option<TreeWrite>)],
        message: &str,
    ) -> Result<Fs> {
        self.commit_changes_with_parents(writes, message, &[])
    }

    /// Like [`commit_changes`] but with extra parent commits.
    pub(crate) fn commit_changes_with_parents(
        &self,
        writes: &[(String, Option<TreeWrite>)],
        message: &str,
        extra_parents: &[&Fs],
    ) -> Result<Fs> {
        let branch = self.require_writable("commit")?;
        let refname = format!("refs/heads/{}", branch);

        // Resolve extra parent OIDs upfront (before locking).
        let mut extra_oids: Vec<git2::Oid> = Vec::new();
        for ep in extra_parents {
            let oid = ep.commit_oid.ok_or_else(|| {
                Error::git_msg("extra parent has no commit".to_string())
            })?;
            extra_oids.push(oid);
        }

        let repo = self
            .inner
            .repo
            .lock()
            .map_err(|e| Error::git_msg(e.to_string()))?;

        let (new_commit_oid, new_tree_oid) = with_repo_lock(&self.inner.path, || {
            // Stale snapshot check
            let current_ref = repo
                .find_reference(&refname)
                .map_err(|_| Error::not_found(format!("branch '{}' not found", branch)))?;
            let current_oid = current_ref.target()
                .ok_or_else(|| Error::git_msg("symbolic reference unexpected"))?;

            if let Some(our_oid) = self.commit_oid {
                if current_oid != our_oid {
                    return Err(Error::stale_snapshot(format!(
                        "branch '{}' has moved: expected {}, found {}",
                        branch, our_oid, current_oid
                    )));
                }
            }

            // Rebuild tree
            let base_tree = self.tree_oid.unwrap_or_else(git2::Oid::zero);
            let new_tree_oid = tree::rebuild_tree(&repo, base_tree, writes)?;

            // No-op check: if tree didn't change, skip
            if Some(new_tree_oid) == self.tree_oid {
                return Ok((current_oid, self.tree_oid.unwrap()));
            }

            // Build commit
            let git_sig = git2::Signature::now(
                &self.inner.signature.name,
                &self.inner.signature.email,
            ).map_err(Error::git)?;

            let tree = repo.find_tree(new_tree_oid).map_err(Error::git)?;
            let parent_commit = if let Some(oid) = self.commit_oid {
                Some(repo.find_commit(oid).map_err(Error::git)?)
            } else {
                None
            };

            // Build parents list: branch tip first, then extra parents.
            let mut extra_commits: Vec<git2::Commit> = Vec::new();
            for oid in &extra_oids {
                extra_commits.push(repo.find_commit(*oid).map_err(Error::git)?);
            }
            let mut parents: Vec<&git2::Commit> = parent_commit.iter().collect();
            for c in &extra_commits {
                parents.push(c);
            }

            let new_commit_oid = repo.commit(
                None, // don't update ref yet — we do it manually for CAS
                &git_sig,
                &git_sig,
                message,
                &tree,
                &parents,
            ).map_err(Error::git)?;

            // Update ref
            let msg: String = format!("commit: {}", message);
            repo.reference(&refname, new_commit_oid, true, &msg)
                .map_err(Error::git)?;

            // Write reflog entry manually
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = crate::reflog::write_reflog_entry(
                &self.inner.path,
                &refname,
                &crate::types::ReflogEntry {
                    old_sha: current_oid.to_string(),
                    new_sha: new_commit_oid.to_string(),
                    committer: format!(
                        "{} <{}>",
                        self.inner.signature.name, self.inner.signature.email
                    ),
                    timestamp: now.as_secs(),
                    message: msg,
                },
            );

            Ok((new_commit_oid, new_tree_oid))
        })?;

        Ok(Fs {
            inner: Arc::clone(&self.inner),
            commit_oid: Some(new_commit_oid),
            tree_oid: Some(new_tree_oid),
            ref_name: self.ref_name.clone(),
            writable: self.writable,
            changes: None,
        })
    }
}

impl std::fmt::Display for Fs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let short = self.commit_oid.map(|o| o.to_string()).unwrap_or_default();
        let short = &short[..short.len().min(7)];
        let mut parts = Vec::new();
        if let Some(ref name) = self.ref_name {
            parts.push(format!("ref_name={:?}", name));
        }
        parts.push(format!("commit={}", short));
        if !self.writable {
            parts.push("readonly".into());
        }
        write!(f, "Fs({})", parts.join(", "))
    }
}

/// Retry a write operation with automatic back-off on stale-snapshot errors.
pub fn retry_write<F, T>(mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut attempt = 0u32;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(Error::StaleSnapshot(_)) if attempt < 5 => {
                let backoff = std::time::Duration::from_millis(
                    (10 * 2u64.pow(attempt)).min(200),
                );
                std::thread::sleep(backoff);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// walk_subtree helper for copy_from_ref
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;

/// Walk a subtree at `path` within a tree, returning `{rel: (oid, mode)}`.
fn walk_subtree(
    repo: &git2::Repository,
    root_tree: git2::Oid,
    path: &str,
) -> Result<BTreeMap<String, (git2::Oid, u32)>> {
    let mut result = BTreeMap::new();

    if path.is_empty() {
        let entries = tree::walk_tree(repo, root_tree)?;
        for (rel, we) in entries {
            result.insert(rel, (we.oid, we.mode));
        }
    } else {
        let entry = tree::entry_at_path(repo, root_tree, path)?;
        match entry {
            Some(e) if e.mode == MODE_TREE => {
                let entries = tree::walk_tree(repo, e.oid)?;
                for (rel, we) in entries {
                    result.insert(rel, (we.oid, we.mode));
                }
            }
            _ => {}
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Glob helper (internal)
// ---------------------------------------------------------------------------

fn iglob_recursive(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    segments: &[&str],
    prefix: &str,
    results: &mut Vec<String>,
) -> Result<()> {
    if segments.is_empty() {
        return Ok(());
    }

    let seg = segments[0];
    let rest = &segments[1..];

    let tree = repo.find_tree(tree_oid).map_err(Error::git)?;

    if seg == "**" {
        iglob_recursive(repo, tree_oid, rest, prefix, results)?;

        for i in 0..tree.len() {
            let entry = tree.get(i).unwrap();
            let name = entry.name().unwrap_or("").to_string();
            if name.starts_with('.') {
                continue;
            }
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };
            let entry_mode = entry.filemode() as u32;
            if entry_mode == MODE_TREE {
                iglob_recursive(repo, entry.id(), segments, &full, results)?;
            }
        }
    } else {
        for i in 0..tree.len() {
            let entry = tree.get(i).unwrap();
            let name = entry.name().unwrap_or("").to_string();
            if !crate::glob::glob_match(seg, &name) {
                continue;
            }
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };
            let entry_mode = entry.filemode() as u32;

            if rest.is_empty() {
                if entry_mode != MODE_TREE {
                    results.push(full);
                }
            } else if entry_mode == MODE_TREE {
                iglob_recursive(repo, entry.id(), rest, &full, results)?;
            }
        }
    }

    Ok(())
}
