use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::fs::Fs;
use crate::notes::NoteDict;
use crate::refdict::RefDict;
use crate::types::{BackupOptions, MirrorDiff, OpenOptions, RestoreOptions, Signature};

/// Internal state shared via `Arc`.
pub(crate) struct GitStoreInner {
    pub(crate) repo: Mutex<git2::Repository>,
    pub(crate) path: PathBuf,
    pub(crate) signature: Signature,
}

impl std::fmt::Debug for GitStoreInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitStoreInner")
            .field("path", &self.path)
            .field("signature", &self.signature)
            .finish_non_exhaustive()
    }
}

/// A versioned filesystem backed by a bare git repository.
///
/// Cheap to clone (`Arc` internally).
#[derive(Clone)]
pub struct GitStore {
    pub(crate) inner: Arc<GitStoreInner>,
}

impl GitStore {
    /// Open (or create) a bare git repository at `path`.
    ///
    /// # Arguments
    /// * `path` - Path to the bare repository.
    /// * `options` - [`OpenOptions`] controlling creation, branch name, and author.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] if the repository does not exist and
    /// `options.create` is `false`.
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        let sig = Signature {
            name: options.author.unwrap_or_else(|| "vost".into()),
            email: options.email.unwrap_or_else(|| "vost@localhost".into()),
        };

        let repo = if path.exists() {
            git2::Repository::open_bare(&path).map_err(Error::git)?
        } else if options.create {
            std::fs::create_dir_all(&path).map_err(|e| Error::io(&path, e))?;
            let repo = git2::Repository::init_bare(&path).map_err(Error::git)?;

            // Enable reflogs for bare repos (matches C++ gitstore.cpp:117-119)
            repo.config()
                .map_err(Error::git)?
                .set_str("core.logAllRefUpdates", "always")
                .map_err(Error::git)?;

            if let Some(ref branch) = options.branch {
                Self::init_branch(&repo, &path, branch, &sig)?;
            }

            repo
        } else {
            return Err(Error::not_found(format!(
                "repository not found: {}",
                path.display()
            )));
        };

        #[allow(clippy::arc_with_non_send_sync)]
        Ok(GitStore {
            inner: Arc::new(GitStoreInner {
                repo: Mutex::new(repo),
                path,
                signature: sig,
            }),
        })
    }

    /// Create the initial commit on `branch` with an empty tree.
    fn init_branch(repo: &git2::Repository, path: &std::path::Path, branch: &str, sig: &Signature) -> Result<()> {
        // Write empty tree
        let builder = repo.treebuilder(None).map_err(Error::git)?;
        let tree_oid = builder.write().map_err(Error::git)?;
        let tree = repo.find_tree(tree_oid).map_err(Error::git)?;

        // Build commit
        let git_sig = git2::Signature::now(&sig.name, &sig.email).map_err(Error::git)?;
        let msg = format!("Initialize {}", branch);
        let refname = format!("refs/heads/{}", branch);

        let commit_oid = repo.commit(
            Some(&refname),
            &git_sig,
            &git_sig,
            &msg,
            &tree,
            &[], // no parents
        ).map_err(Error::git)?;

        // Write reflog entry for the initial commit
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let log_msg = format!("commit: Initialize {}", branch);
        let _ = crate::reflog::write_reflog_entry(
            path,
            &refname,
            &crate::types::ReflogEntry {
                old_sha: crate::reflog::ZERO_SHA.to_string(),
                new_sha: commit_oid.to_string(),
                committer: format!("{} <{}>", sig.name, sig.email),
                timestamp: now.as_secs(),
                message: log_msg,
            },
        );

        // Set HEAD as symbolic ref to the branch
        repo.set_head(&refname).map_err(Error::git)?;

        Ok(())
    }

    /// Return an [`Fs`] for any ref string (branch, tag, or commit hash).
    ///
    /// Resolution order: branches → tags → commit hash.
    /// Branches return a writable `Fs`; tags and hashes return read-only.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] if the ref cannot be resolved.
    pub fn fs(&self, ref_str: &str) -> Result<Fs> {
        // Try branch first
        if let Ok(fs) = self.branches().get(ref_str) {
            return Ok(fs);
        }
        // Try tag
        if let Ok(fs) = self.tags().get(ref_str) {
            return Ok(fs);
        }
        // Fall back to commit hash
        let oid = git2::Oid::from_str(ref_str)
            .map_err(|_| Error::not_found(format!("ref not found: '{}'", ref_str)))?;
        {
            let repo = self.inner.repo.lock()
                .map_err(|e| Error::git_msg(e.to_string()))?;
            // Verify it's a commit
            let obj = repo.find_object(oid, None)
                .map_err(|_| Error::not_found(format!("ref not found: '{}'", ref_str)))?;
            if obj.kind() != Some(git2::ObjectType::Commit) {
                return Err(Error::not_found(format!("ref not found: '{}'", ref_str)));
            }
        }
        Fs::from_commit(Arc::clone(&self.inner), oid, None, Some(false))
    }

    /// Return a [`RefDict`] for branches (`refs/heads/`).
    ///
    /// Supports `get`, `set`, `delete`, `contains`, `keys`, iteration,
    /// and `current`/`set_current` for HEAD management.
    pub fn branches(&self) -> RefDict<'_> {
        RefDict::new(self, "refs/heads/")
    }

    /// Return a [`RefDict`] for tags (`refs/tags/`).
    ///
    /// Tags are read-only snapshots — `set` creates a tag but the returned
    /// [`Fs`] is not writable.
    pub fn tags(&self) -> RefDict<'_> {
        RefDict::new(self, "refs/tags/")
    }

    /// Return a [`NoteDict`] for accessing git notes namespaces.
    ///
    /// Use `notes().commits()` for the default `refs/notes/commits` namespace,
    /// or `notes().ns("custom")` for a custom namespace.
    pub fn notes(&self) -> NoteDict<'_> {
        NoteDict::new(self)
    }

    /// Path to the bare repository on disk.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The default signature used for commits.
    pub fn signature(&self) -> &Signature {
        &self.inner.signature
    }

    /// Push refs to `dest` (or write a bundle file).
    ///
    /// Without `opts.refs` this is a full mirror: remote-only refs are deleted.
    /// With `opts.refs` only the specified refs are pushed (no deletes).
    ///
    /// Supports local paths, remote URLs (SSH, HTTPS, git), and bundle files.
    /// Auto-creates a bare repository at local destinations.
    ///
    /// # Arguments
    /// * `dest` - Destination URL, local path, or bundle file path.
    /// * `opts` - [`BackupOptions`] controlling dry-run, refs filter, and format.
    pub fn backup(&self, dest: &str, opts: &BackupOptions) -> Result<MirrorDiff> {
        crate::mirror::backup(&self.inner.path, dest, opts)
    }

    /// Fetch refs from `src` (or import a bundle file).
    ///
    /// Restore is **additive**: it adds and updates refs but never deletes
    /// local-only refs. Supports local paths, remote URLs (SSH, HTTPS, git),
    /// and bundle files.
    ///
    /// # Arguments
    /// * `src` - Source URL, local path, or bundle file path.
    /// * `opts` - [`RestoreOptions`] controlling dry-run, refs filter, and format.
    pub fn restore(&self, src: &str, opts: &RestoreOptions) -> Result<MirrorDiff> {
        crate::mirror::restore(&self.inner.path, src, opts)
    }

    /// Export a bundle file containing the specified refs (or all refs).
    ///
    /// Creates a self-contained v2 git bundle at `path` that can be
    /// imported with [`bundle_import`](Self::bundle_import).
    ///
    /// # Arguments
    /// * `path` - Destination file path for the bundle.
    /// * `refs` - Optional list of ref names to include. `None` exports all refs.
    /// * `rename` - Optional map of source→destination ref names for renaming refs
    ///   in the bundle header.
    /// * `squash` - If true, each ref gets a parentless commit with the same tree,
    ///   stripping all history.
    pub fn bundle_export(
        &self,
        path: &str,
        refs: Option<&[String]>,
        rename: Option<&std::collections::HashMap<String, String>>,
        squash: bool,
    ) -> Result<()> {
        crate::mirror::bundle_export(&self.inner.path, path, refs, rename, squash)
    }

    /// Import refs from a bundle file (additive — no deletes).
    ///
    /// Reads a v2 git bundle created by [`bundle_export`](Self::bundle_export)
    /// and adds its objects and refs to this repository.
    ///
    /// # Arguments
    /// * `path` - Source bundle file path.
    /// * `refs` - Optional list of ref names to import. `None` imports all refs.
    /// * `rename` - Optional map of bundle ref names→local ref names for renaming
    ///   refs during import.
    pub fn bundle_import(
        &self,
        path: &str,
        refs: Option<&[String]>,
        rename: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<()> {
        crate::mirror::bundle_import(&self.inner.path, path, refs, rename)
    }
}
