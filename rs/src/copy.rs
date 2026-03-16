use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::exclude::ExcludeFilter;
use crate::fs::TreeWrite;
use crate::tree;
use crate::types::{ChangeReport, FileEntry, FileType, MODE_BLOB, MODE_LINK, MODE_TREE};

// ---------------------------------------------------------------------------
// Source resolution types
// ---------------------------------------------------------------------------

/// Mode of a resolved source entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceMode {
    /// Single file.
    File,
    /// Directory with name preserved.
    Dir,
    /// Directory contents (trailing `/` or root).
    Contents,
}

/// A resolved source entry: (path, mode, prefix).
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSource {
    pub path: String,
    pub mode: SourceMode,
    /// Intermediate path injected between dest and filename (rsync `/./` pivot).
    pub prefix: String,
}

// ---------------------------------------------------------------------------
// Disk source resolution
// ---------------------------------------------------------------------------

/// Resolve local source specs into resolved triples.
///
/// Handles trailing `/` (contents mode), `/./` pivot markers (rsync -R style),
/// and classifies each source as file, dir, or contents.
pub(crate) fn resolve_disk_sources(sources: &[&str]) -> Result<Vec<ResolvedSource>> {
    let mut resolved = Vec::new();
    for src in sources {
        // --- /./ pivot detection (rsync -R style) ---
        let normalized = src.replace(std::path::MAIN_SEPARATOR, "/");
        if let Some(idx) = normalized.find("/./") {
            if idx > 0 {
                let base = &src[..idx];
                let rest_raw = &src[idx + 3..];
                let rest = rest_raw.replace(std::path::MAIN_SEPARATOR, "/");
                let contents_mode = rest.ends_with('/');
                let rest_clean = rest.trim_end_matches('/');

                let rest_os_clean = rest_raw.trim_end_matches('/').trim_end_matches(std::path::MAIN_SEPARATOR);
                let full_path = if rest_os_clean.is_empty() {
                    PathBuf::from(base)
                } else {
                    PathBuf::from(base).join(rest_os_clean)
                };

                if !full_path.exists() {
                    return Err(Error::not_found(full_path.to_string_lossy().into_owned()));
                }

                let mode = if full_path.is_dir() {
                    if contents_mode { SourceMode::Contents } else { SourceMode::Dir }
                } else {
                    if contents_mode {
                        return Err(Error::not_a_directory(full_path.to_string_lossy().into_owned()));
                    }
                    SourceMode::File
                };

                let prefix = if rest_clean.is_empty() {
                    String::new()
                } else {
                    let parts: Vec<&str> = rest_clean.split('/').collect();
                    if parts.len() > 1 {
                        parts[..parts.len() - 1].join("/")
                    } else {
                        String::new()
                    }
                };

                resolved.push(ResolvedSource {
                    path: full_path.to_string_lossy().into_owned(),
                    mode,
                    prefix,
                });
                continue;
            }
        }

        let contents_mode = src.ends_with('/');

        if contents_mode {
            let path = src.trim_end_matches('/');
            let p = Path::new(path);
            if !p.is_dir() {
                return Err(Error::not_a_directory(path));
            }
            resolved.push(ResolvedSource {
                path: path.to_string(),
                mode: SourceMode::Contents,
                prefix: String::new(),
            });
        } else {
            let p = Path::new(*src);
            if p.is_dir() {
                resolved.push(ResolvedSource {
                    path: src.to_string(),
                    mode: SourceMode::Dir,
                    prefix: String::new(),
                });
            } else if p.exists() {
                resolved.push(ResolvedSource {
                    path: src.to_string(),
                    mode: SourceMode::File,
                    prefix: String::new(),
                });
            } else {
                return Err(Error::not_found(*src));
            }
        }
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Disk → Repo enumeration
// ---------------------------------------------------------------------------

/// Build `(local_path, repo_path)` pairs for disk → repo copy.
pub(crate) fn enum_disk_to_repo(
    resolved: &[ResolvedSource],
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
) -> Result<Vec<(PathBuf, String)>> {
    enum_disk_to_repo_ext(resolved, dest, include, exclude, false)
}

pub(crate) fn enum_disk_to_repo_ext(
    resolved: &[ResolvedSource],
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    follow_symlinks: bool,
) -> Result<Vec<(PathBuf, String)>> {
    let mut pairs = Vec::new();
    let dest_norm = if dest.is_empty() {
        String::new()
    } else {
        crate::paths::normalize_path(dest)?
    };

    for rs in resolved {
        // Build effective destination by injecting the pivot prefix
        let eff_dest = if rs.prefix.is_empty() {
            dest_norm.clone()
        } else if dest_norm.is_empty() {
            rs.prefix.clone()
        } else {
            format!("{}/{}", dest_norm, rs.prefix)
        };

        match rs.mode {
            SourceMode::File => {
                let local = Path::new(&rs.path);
                let name = local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let repo_file = if eff_dest.is_empty() {
                    name
                } else {
                    format!("{}/{}", eff_dest, name)
                };
                pairs.push((local.to_path_buf(), crate::paths::normalize_path(&repo_file)?));
            }
            SourceMode::Dir => {
                let local = Path::new(&rs.path);
                let dirname = local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let target = if eff_dest.is_empty() {
                    dirname
                } else {
                    format!("{}/{}", eff_dest, dirname)
                };
                let files = disk_glob_ext(local, include, exclude, follow_symlinks, &mut None)?;
                for rel in &files {
                    let full = local.join(rel);
                    let repo_file = format!("{}/{}", target, rel);
                    pairs.push((full, crate::paths::normalize_path(&repo_file)?));
                }
            }
            SourceMode::Contents => {
                let local = Path::new(&rs.path);
                let files = disk_glob_ext(local, include, exclude, follow_symlinks, &mut None)?;
                for rel in &files {
                    let full = local.join(rel);
                    let repo_file = if eff_dest.is_empty() {
                        rel.clone()
                    } else {
                        format!("{}/{}", eff_dest, rel)
                    };
                    pairs.push((full, crate::paths::normalize_path(&repo_file)?));
                }
            }
        }
    }
    Ok(pairs)
}

// ---------------------------------------------------------------------------
// Multi-source copy_in
// ---------------------------------------------------------------------------

/// Copy multiple local sources into a git tree.
///
/// Resolves sources, enumerates (local, repo) pairs, reads files, creates
/// blobs, and returns `(store_path, TreeWrite)` pairs plus a [`ChangeReport`].
pub(crate) fn copy_in_multi(
    repo: &git2::Repository,
    base_tree: git2::Oid,
    sources: &[&str],
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    checksum: bool,
    commit_time: Option<u64>,
    follow_symlinks: bool,
) -> Result<(Vec<(String, TreeWrite)>, ChangeReport)> {
    let resolved = resolve_disk_sources(sources)?;
    let pairs = enum_disk_to_repo_ext(&resolved, dest, include, exclude, follow_symlinks)?;

    let mut writes = Vec::new();
    let mut report = ChangeReport::new();

    // Build existing entries map when checksum is enabled
    let dest_norm = if dest.is_empty() {
        String::new()
    } else {
        crate::paths::normalize_path(dest)?
    };
    let existing: std::collections::HashMap<String, (git2::Oid, u32)> = if checksum {
        let target_oid = if dest_norm.is_empty() {
            Some(base_tree)
        } else {
            match tree::entry_at_path(repo, base_tree, &dest_norm)? {
                Some(entry) if entry.mode == MODE_TREE => Some(entry.oid),
                _ => None,
            }
        };
        match target_oid {
            Some(oid) if !oid.is_zero() => {
                tree::walk_tree(repo, oid)?
                    .into_iter()
                    .map(|(p, e)| {
                        let full = if dest_norm.is_empty() {
                            p
                        } else {
                            format!("{}/{}", dest_norm, p)
                        };
                        (full, (e.oid, e.mode))
                    })
                    .collect()
            }
            _ => std::collections::HashMap::new(),
        }
    } else {
        std::collections::HashMap::new()
    };

    for (local_path, store_path) in &pairs {
        // Mtime-based skip: only when checksum=false AND file already exists in repo
        if !checksum {
            if let Some(ct) = commit_time {
                if existing.contains_key(store_path) {
                    if let Ok(meta) = std::fs::metadata(local_path) {
                        if let Ok(mtime) = meta.modified() {
                            if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                                if dur.as_secs() <= ct {
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        }

        let mode = if follow_symlinks {
            mode_from_disk_follow(local_path)
        } else {
            tree::mode_from_disk(local_path).unwrap_or(MODE_BLOB)
        };
        let data = if mode == MODE_LINK {
            let target = std::fs::read_link(local_path).map_err(|e| Error::io(local_path, e))?;
            target.to_string_lossy().into_owned().into_bytes()
        } else {
            std::fs::read(local_path).map_err(|e| Error::io(local_path, e))?
        };

        let file_type = FileType::from_mode(mode).unwrap_or(FileType::Blob);
        let blob_oid = repo.blob(&data).map_err(Error::git)?;

        // Skip unchanged files when checksum is enabled
        if checksum {
            if let Some((existing_oid, existing_mode)) = existing.get(store_path) {
                if *existing_oid == blob_oid && *existing_mode == mode {
                    continue;
                }
            }
        }

        writes.push((
            store_path.clone(),
            TreeWrite {
                data,
                oid: blob_oid,
                mode,
            },
        ));
        report.add.push(FileEntry::with_src(store_path, file_type, local_path));
    }

    Ok((writes, report))
}

// ---------------------------------------------------------------------------
// Repo → Disk: resolve + enumerate + copy_out_multi
// ---------------------------------------------------------------------------

/// Resolve repo source specs into resolved triples.
///
/// Checks existence in the tree and classifies as file/dir/contents.
pub(crate) fn resolve_repo_sources(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    sources: &[&str],
) -> Result<Vec<ResolvedSource>> {
    let mut resolved = Vec::new();
    for src in sources {
        // --- /./ pivot detection ---
        let normalized = src.replace('\\', "/");
        if let Some(idx) = normalized.find("/./") {
            if idx > 0 {
                let base = &normalized[..idx];
                let rest = &normalized[idx + 3..];
                let contents_mode = rest.ends_with('/');
                let rest_clean = rest.trim_end_matches('/');

                let full_path = if rest_clean.is_empty() {
                    base.to_string()
                } else {
                    format!("{}/{}", base, rest_clean)
                };
                let full_path = crate::paths::normalize_path(&full_path)?;

                let entry = tree::entry_at_path(repo, tree_oid, &full_path)?
                    .ok_or_else(|| Error::not_found(&full_path))?;

                let mode = if entry.mode == MODE_TREE {
                    if contents_mode { SourceMode::Contents } else { SourceMode::Dir }
                } else {
                    if contents_mode {
                        return Err(Error::not_a_directory(&full_path));
                    }
                    SourceMode::File
                };

                let prefix = if rest_clean.is_empty() {
                    String::new()
                } else {
                    let parts: Vec<&str> = rest_clean.split('/').collect();
                    if parts.len() > 1 {
                        parts[..parts.len() - 1].join("/")
                    } else {
                        String::new()
                    }
                };

                resolved.push(ResolvedSource {
                    path: full_path,
                    mode,
                    prefix,
                });
                continue;
            }
        }

        let contents_mode = src.ends_with('/');

        if contents_mode {
            let path = src.trim_end_matches('/');
            let path = if path.is_empty() {
                String::new()
            } else {
                crate::paths::normalize_path(path)?
            };
            if !path.is_empty() {
                let entry = tree::entry_at_path(repo, tree_oid, &path)?;
                match entry {
                    Some(e) if e.mode == MODE_TREE => {}
                    Some(_) => return Err(Error::not_a_directory(&path)),
                    None => return Err(Error::not_found(&path)),
                }
            }
            resolved.push(ResolvedSource {
                path,
                mode: SourceMode::Contents,
                prefix: String::new(),
            });
        } else {
            let path = if src.is_empty() {
                String::new()
            } else {
                crate::paths::normalize_path(src)?
            };
            if path.is_empty() {
                resolved.push(ResolvedSource {
                    path: String::new(),
                    mode: SourceMode::Contents,
                    prefix: String::new(),
                });
            } else {
                let entry = tree::entry_at_path(repo, tree_oid, &path)?
                    .ok_or_else(|| Error::not_found(&path))?;
                if entry.mode == MODE_TREE {
                    resolved.push(ResolvedSource {
                        path,
                        mode: SourceMode::Dir,
                        prefix: String::new(),
                    });
                } else {
                    resolved.push(ResolvedSource {
                        path,
                        mode: SourceMode::File,
                        prefix: String::new(),
                    });
                }
            }
        }
    }
    Ok(resolved)
}

/// Build `(repo_path, local_path)` pairs for repo → disk copy.
pub(crate) fn enum_repo_to_disk(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    resolved: &[ResolvedSource],
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
) -> Result<Vec<(String, PathBuf)>> {
    let mut pairs = Vec::new();
    let dest_path = Path::new(dest);

    for rs in resolved {
        let eff_dest = if rs.prefix.is_empty() {
            dest_path.to_path_buf()
        } else {
            dest_path.join(&rs.prefix)
        };

        match rs.mode {
            SourceMode::File => {
                let name = rs.path.rsplit('/').next().unwrap_or(&rs.path);
                let local = eff_dest.join(name);
                pairs.push((rs.path.clone(), local));
            }
            SourceMode::Dir => {
                let dirname = rs.path.rsplit('/').next().unwrap_or(&rs.path);
                let target = eff_dest.join(dirname);
                let target_oid = if rs.path.is_empty() {
                    tree_oid
                } else {
                    let entry = tree::entry_at_path(repo, tree_oid, &rs.path)?
                        .ok_or_else(|| Error::not_found(&rs.path))?;
                    entry.oid
                };
                let entries = tree::walk_tree(repo, target_oid)?;
                for (rel_path, _) in &entries {
                    if !matches_filters(rel_path, include, exclude) {
                        continue;
                    }
                    let full_repo = if rs.path.is_empty() {
                        rel_path.clone()
                    } else {
                        format!("{}/{}", rs.path, rel_path)
                    };
                    let local = target.join(rel_path);
                    pairs.push((full_repo, local));
                }
            }
            SourceMode::Contents => {
                let target_oid = if rs.path.is_empty() {
                    tree_oid
                } else {
                    let entry = tree::entry_at_path(repo, tree_oid, &rs.path)?
                        .ok_or_else(|| Error::not_found(&rs.path))?;
                    entry.oid
                };
                let entries = tree::walk_tree(repo, target_oid)?;
                for (rel_path, _) in &entries {
                    if !matches_filters(rel_path, include, exclude) {
                        continue;
                    }
                    let full_repo = if rs.path.is_empty() {
                        rel_path.clone()
                    } else {
                        format!("{}/{}", rs.path, rel_path)
                    };
                    let local = eff_dest.join(rel_path);
                    pairs.push((full_repo, local));
                }
            }
        }
    }
    Ok(pairs)
}

/// Copy multiple repo sources to local disk.
pub(crate) fn copy_out_multi(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    sources: &[&str],
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    commit_time: Option<u64>,
) -> Result<ChangeReport> {
    let resolved = resolve_repo_sources(repo, tree_oid, sources)?;
    let pairs = enum_repo_to_disk(repo, tree_oid, &resolved, dest, include, exclude)?;

    let mut report = ChangeReport::new();

    for (repo_path, local_path) in &pairs {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }

        let entry = tree::entry_at_path(repo, tree_oid, repo_path)?
            .ok_or_else(|| Error::not_found(repo_path))?;

        let blob = repo.find_blob(entry.oid).map_err(Error::git)?;

        if entry.mode == MODE_LINK {
            let target = String::from_utf8_lossy(blob.content());
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let _ = std::fs::remove_file(local_path);
                symlink(target.as_ref(), local_path)
                    .map_err(|e| Error::io(local_path, e))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::write(local_path, target.as_bytes())
                    .map_err(|e| Error::io(local_path, e))?;
            }
        } else {
            std::fs::write(local_path, blob.content())
                .map_err(|e| Error::io(local_path, e))?;

            #[cfg(unix)]
            if entry.mode == crate::types::MODE_BLOB_EXEC {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(local_path, perms)
                    .map_err(|e| Error::io(local_path, e))?;
            }
        }

        // Set mtime to commit timestamp when provided
        if let Some(ct) = commit_time {
            if entry.mode != MODE_LINK {
                let ft = filetime::FileTime::from_unix_time(ct as i64, 0);
                let _ = filetime::set_file_mtime(local_path, ft);
            }
        }

        let file_type = FileType::from_mode(entry.mode).unwrap_or(FileType::Blob);
        let rel = local_path
            .strip_prefix(dest)
            .unwrap_or(local_path)
            .to_string_lossy()
            .into_owned();
        report.add.push(FileEntry::with_src(&rel, file_type, local_path));
    }

    Ok(report)
}

/// Copy files from a local directory into a git tree.
///
/// Walks `src` on disk, writes blobs to the object store, and returns a list
/// of `(store_path, TreeWrite)` pairs that the caller should apply to
/// the tree, along with a [`ChangeReport`] describing what was added.
///
/// # Arguments
/// * `repo` - The git repository to write blobs into.
/// * `base_tree` - Root tree OID of the current commit (used for checksum dedup).
/// * `src` - Local directory to copy from.
/// * `dest` - Destination path prefix inside the repo (e.g. `"data"` or `""`).
/// * `include` - Optional glob patterns; only matching files are copied.
/// * `exclude` - Optional glob patterns; matching files are skipped.
/// * `checksum` - When `true`, skip files whose blob OID and mode already
///   match the existing tree entry (content-based deduplication).
/// * `commit_time` - When `Some` and `checksum` is `false`, files with mtime
///   <= this value are skipped (mtime-based change detection).
/// * `follow_symlinks` - When `true`, follow symlinks instead of recording them.
pub fn copy_in(
    repo: &git2::Repository,
    base_tree: git2::Oid,
    src: &Path,
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    checksum: bool,
    commit_time: Option<u64>,
    follow_symlinks: bool,
) -> Result<(Vec<(String, TreeWrite)>, ChangeReport)> {
    let mut writes = Vec::new();
    let mut report = ChangeReport::new();
    let dest_norm = crate::paths::normalize_path(dest)?;

    // Build existing entries map when checksum is enabled
    let existing: std::collections::HashMap<String, (git2::Oid, u32)> = if checksum {
        let target_oid = if dest_norm.is_empty() {
            Some(base_tree)
        } else {
            match tree::entry_at_path(repo, base_tree, &dest_norm)? {
                Some(entry) if entry.mode == MODE_TREE => Some(entry.oid),
                _ => None,
            }
        };
        match target_oid {
            Some(oid) if !oid.is_zero() => {
                tree::walk_tree(repo, oid)?
                    .into_iter()
                    .map(|(p, e)| (p, (e.oid, e.mode)))
                    .collect()
            }
            _ => std::collections::HashMap::new(),
        }
    } else {
        std::collections::HashMap::new()
    };

    let disk_files = disk_glob_ext(src, include, exclude, follow_symlinks, &mut None)?;

    for rel_path in &disk_files {
        let full_disk = src.join(rel_path);
        let store_path = if dest_norm.is_empty() {
            rel_path.clone()
        } else {
            format!("{}/{}", dest_norm, rel_path)
        };

        // Mtime-based skip: only when checksum=false AND file already exists in repo
        if !checksum && existing.contains_key(&store_path) {
            if let Some(ct) = commit_time {
                if let Ok(meta) = std::fs::metadata(&full_disk) {
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                            if dur.as_secs() <= ct {
                                continue;
                            }
                        }
                    }
                }
            }
        }

        let mode = if follow_symlinks {
            // When following symlinks, treat everything as regular file/exec
            mode_from_disk_follow(&full_disk)
        } else {
            tree::mode_from_disk(&full_disk).unwrap_or(MODE_BLOB)
        };
        let data = if mode == MODE_LINK {
            let target = std::fs::read_link(&full_disk).map_err(|e| Error::io(&full_disk, e))?;
            target.to_string_lossy().into_owned().into_bytes()
        } else {
            std::fs::read(&full_disk).map_err(|e| Error::io(&full_disk, e))?
        };

        let file_type = FileType::from_mode(mode).unwrap_or(FileType::Blob);
        let blob_oid = repo.blob(&data).map_err(Error::git)?;

        // Skip unchanged files when checksum is enabled
        if checksum {
            if let Some((existing_oid, existing_mode)) = existing.get(rel_path) {
                if *existing_oid == blob_oid && *existing_mode == mode {
                    continue;
                }
            }
        }

        writes.push((
            store_path.clone(),
            TreeWrite {
                data,
                oid: blob_oid,
                mode,
            },
        ));
        report.add.push(FileEntry::with_src(&store_path, file_type, &full_disk));
    }

    Ok((writes, report))
}

/// Determine file mode when following symlinks (no symlink entries).
fn mode_from_disk_follow(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o111 != 0 {
                return crate::types::MODE_BLOB_EXEC;
            }
        }
        MODE_BLOB
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        MODE_BLOB
    }
}

/// Copy files from a git tree to a local directory.
///
/// Reads blobs from the tree rooted at `src` and writes them to `dest` on
/// disk. Symlinks and executable permissions are preserved on Unix.
///
/// # Arguments
/// * `repo` - The git repository to read objects from.
/// * `tree_oid` - Root tree OID of the commit to export from.
/// * `src` - Source path prefix inside the repo (e.g. `"data"` or `""`).
/// * `dest` - Local directory to write files into.
/// * `include` - Optional glob patterns; only matching files are copied.
/// * `exclude` - Optional glob patterns; matching files are skipped.
pub fn copy_out(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    src: &str,
    dest: &Path,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    commit_time: Option<u64>,
) -> Result<ChangeReport> {
    let mut report = ChangeReport::new();
    let src_norm = crate::paths::normalize_path(src)?;

    let target_oid = if src_norm.is_empty() {
        tree_oid
    } else {
        let entry = tree::entry_at_path(repo, tree_oid, &src_norm)?
            .ok_or_else(|| Error::not_found(&src_norm))?;
        if entry.mode != MODE_TREE {
            return Err(Error::not_a_directory(&src_norm));
        }
        entry.oid
    };

    let entries = tree::walk_tree(repo, target_oid)?;

    for (rel_path, entry) in &entries {
        if !matches_filters(rel_path, include, exclude) {
            continue;
        }

        let dest_path = dest.join(rel_path);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }

        let blob = repo.find_blob(entry.oid).map_err(Error::git)?;

        if entry.mode == MODE_LINK {
            let target = String::from_utf8_lossy(blob.content());
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let _ = std::fs::remove_file(&dest_path);
                symlink(target.as_ref(), &dest_path)
                    .map_err(|e| Error::io(&dest_path, e))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::write(&dest_path, target.as_bytes())
                    .map_err(|e| Error::io(&dest_path, e))?;
            }
        } else {
            std::fs::write(&dest_path, blob.content()).map_err(|e| Error::io(&dest_path, e))?;

            #[cfg(unix)]
            if entry.mode == crate::types::MODE_BLOB_EXEC {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&dest_path, perms)
                    .map_err(|e| Error::io(&dest_path, e))?;
            }
        }

        // Set mtime to commit timestamp when provided
        if let Some(ct) = commit_time {
            if entry.mode != MODE_LINK {
                let ft = filetime::FileTime::from_unix_time(ct as i64, 0);
                let _ = filetime::set_file_mtime(&dest_path, ft);
            }
        }

        let file_type = FileType::from_mode(entry.mode).unwrap_or(FileType::Blob);
        report.add.push(FileEntry::with_src(rel_path, file_type, &dest_path));
    }

    Ok(report)
}

/// Sync files from disk into a tree (add + update + delete).
///
/// Makes the tree subtree at `dest` identical to the local directory `src`.
/// Unlike [`copy_in`], this also deletes files in the destination tree that
/// are not present on disk, and classifies changes as add/update/delete in
/// the returned [`ChangeReport`]. Entries with `None` in the returned vec
/// represent deletions.
///
/// # Arguments
/// * `repo` - The git repository.
/// * `base_tree` - Root tree OID of the current commit.
/// * `src` - Local directory to sync from.
/// * `dest` - Destination path prefix inside the repo.
/// * `include` - Optional glob patterns; only matching files are synced.
/// * `exclude` - Optional glob patterns; matching files are skipped.
/// * `checksum` - When `true`, skip unchanged files (OID + mode comparison).
#[allow(clippy::type_complexity)]
pub fn sync_in(
    repo: &git2::Repository,
    base_tree: git2::Oid,
    src: &Path,
    dest: &str,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    checksum: bool,
    commit_time: Option<u64>,
    exclude_filter: &mut Option<ExcludeFilter>,
    ignore_errors: bool,
) -> Result<(Vec<(String, Option<TreeWrite>)>, ChangeReport)> {
    let mut writes: Vec<(String, Option<TreeWrite>)> = Vec::new();
    let mut report = ChangeReport::new();
    let dest_norm = crate::paths::normalize_path(dest)?;

    // Collect disk files (uses exclude_filter for gitignore support)
    let disk_files = disk_glob_ext(src, include, exclude, false, exclude_filter)?;
    let disk_set: std::collections::HashSet<&str> = disk_files.iter().map(|s| s.as_str()).collect();

    // Collect existing tree entries at dest
    let existing = {
        let target_oid = if dest_norm.is_empty() {
            Some(base_tree)
        } else {
            match tree::entry_at_path(repo, base_tree, &dest_norm)? {
                Some(entry) if entry.mode == MODE_TREE => Some(entry.oid),
                _ => None,
            }
        };
        match target_oid {
            Some(oid) if !oid.is_zero() => tree::walk_tree(repo, oid)?,
            _ => Vec::new(),
        }
    };

    let existing_map: std::collections::HashMap<&str, &crate::types::WalkEntry> =
        existing.iter().map(|(p, e)| (p.as_str(), e)).collect();

    // Process disk files: add or update
    for rel_path in &disk_files {
        let full_disk = src.join(rel_path);

        // Mtime-based skip: only when checksum=false AND file already exists in repo
        if !checksum && existing_map.contains_key(rel_path.as_str()) {
            if let Some(ct) = commit_time {
                if let Ok(meta) = std::fs::metadata(&full_disk) {
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                            if dur.as_secs() <= ct {
                                continue;
                            }
                        }
                    }
                }
            }
        }

        let store_path = if dest_norm.is_empty() {
            rel_path.clone()
        } else {
            format!("{}/{}", dest_norm, rel_path)
        };

        let mode = tree::mode_from_disk(&full_disk).unwrap_or(MODE_BLOB);
        let data = if mode == MODE_LINK {
            match std::fs::read_link(&full_disk) {
                Ok(target) => target.to_string_lossy().into_owned().into_bytes(),
                Err(e) if ignore_errors => {
                    report.errors.push(crate::types::ChangeError::new(
                        full_disk.to_string_lossy(), e.to_string(),
                    ));
                    continue;
                }
                Err(e) => return Err(Error::io(&full_disk, e)),
            }
        } else {
            match std::fs::read(&full_disk) {
                Ok(d) => d,
                Err(e) if ignore_errors => {
                    report.errors.push(crate::types::ChangeError::new(
                        full_disk.to_string_lossy(), e.to_string(),
                    ));
                    continue;
                }
                Err(e) => return Err(Error::io(&full_disk, e)),
            }
        };

        let blob_oid = repo.blob(&data).map_err(Error::git)?;
        let file_type = FileType::from_mode(mode).unwrap_or(FileType::Blob);

        // Check if this is an update vs add
        let is_changed = if let Some(existing_entry) = existing_map.get(rel_path.as_str()) {
            if checksum {
                existing_entry.oid != blob_oid || existing_entry.mode != mode
            } else {
                // Without checksum+mtime, treat as changed
                true
            }
        } else {
            true
        };

        if is_changed {
            writes.push((
                store_path.clone(),
                Some(TreeWrite {
                    data,
                    oid: blob_oid,
                    mode,
                }),
            ));

            if existing_map.contains_key(rel_path.as_str()) {
                report.update.push(FileEntry::with_src(&store_path, file_type, &full_disk));
            } else {
                report.add.push(FileEntry::with_src(&store_path, file_type, &full_disk));
            }
        }
    }

    // Delete files in tree that are not on disk
    for (rel_path, entry) in &existing {
        if !disk_set.contains(rel_path.as_str()) {
            // Also apply include/exclude filters to deletions
            if !matches_filters(rel_path, include, exclude) {
                continue;
            }
            // rsync behavior: when --exclude is combined with --delete,
            // excluded files in the destination are PRESERVED (not deleted).
            // This applies to both --exclude patterns and --gitignore.
            if let Some(ref ef) = exclude_filter {
                if ef.is_excluded_in_walk(rel_path, false) {
                    continue;
                }
            }
            let store_path = if dest_norm.is_empty() {
                rel_path.clone()
            } else {
                format!("{}/{}", dest_norm, rel_path)
            };
            let file_type = FileType::from_mode(entry.mode).unwrap_or(FileType::Blob);
            writes.push((store_path.clone(), None));
            report.delete.push(FileEntry::new(&store_path, file_type));
        }
    }

    Ok((writes, report))
}

/// Sync files from a tree to disk (add + update + delete).
///
/// Makes the local directory `dest` identical to the tree subtree at `src`.
/// Unlike [`copy_out`], this also deletes local files that are not present
/// in the repo tree, prunes empty directories, and classifies all changes
/// as add/update/delete in the returned [`ChangeReport`].
///
/// # Arguments
/// * `repo` - The git repository.
/// * `tree_oid` - Root tree OID of the commit to export from.
/// * `src` - Source path prefix inside the repo.
/// * `dest` - Local directory to sync into.
/// * `include` - Optional glob patterns; only matching files are synced.
/// * `exclude` - Optional glob patterns; matching files are skipped.
/// * `checksum` - When `true`, skip unchanged files (content comparison).
pub fn sync_out(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    src: &str,
    dest: &Path,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    checksum: bool,
    commit_time: Option<u64>,
) -> Result<ChangeReport> {
    let mut report = ChangeReport::new();
    let src_norm = crate::paths::normalize_path(src)?;

    // Walk repo tree to get source files
    let target_oid = if src_norm.is_empty() {
        tree_oid
    } else {
        let entry = tree::entry_at_path(repo, tree_oid, &src_norm)?
            .ok_or_else(|| Error::not_found(&src_norm))?;
        if entry.mode != MODE_TREE {
            return Err(Error::not_a_directory(&src_norm));
        }
        entry.oid
    };

    let repo_entries = tree::walk_tree(repo, target_oid)?;
    let repo_map: std::collections::HashMap<&str, &crate::types::WalkEntry> =
        repo_entries.iter().map(|(p, e)| (p.as_str(), e)).collect();

    // Walk local destination to get existing disk files
    let disk_files = if dest.exists() {
        disk_glob(dest, None, None)?
    } else {
        Vec::new()
    };
    let disk_set: std::collections::HashSet<&str> = disk_files.iter().map(|s| s.as_str()).collect();

    // Process repo files: write new/updated files to disk
    for (rel_path, entry) in &repo_entries {
        if !matches_filters(rel_path, include, exclude) {
            continue;
        }

        let dest_path = dest.join(rel_path);
        let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
        let file_type = FileType::from_mode(entry.mode).unwrap_or(FileType::Blob);

        // Check if file exists on disk and whether it's changed
        let needs_write = if disk_set.contains(rel_path.as_str()) {
            if checksum {
                // Compare blob OID of new content vs existing file
                let existing_data = if entry.mode == MODE_LINK {
                    match std::fs::read_link(&dest_path) {
                        Ok(target) => target.to_string_lossy().into_owned().into_bytes(),
                        Err(_) => vec![], // force write if can't read
                    }
                } else {
                    std::fs::read(&dest_path).unwrap_or_default()
                };
                let existing_oid = repo.blob(&existing_data).map_err(Error::git)?;
                existing_oid != entry.oid
            } else {
                true // without checksum, always write
            }
        } else {
            true // file doesn't exist on disk
        };

        if needs_write {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }

            if entry.mode == MODE_LINK {
                let target = String::from_utf8_lossy(blob.content());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    let _ = std::fs::remove_file(&dest_path);
                    symlink(target.as_ref(), &dest_path)
                        .map_err(|e| Error::io(&dest_path, e))?;
                }
                #[cfg(not(unix))]
                {
                    std::fs::write(&dest_path, target.as_bytes())
                        .map_err(|e| Error::io(&dest_path, e))?;
                }
            } else {
                std::fs::write(&dest_path, blob.content()).map_err(|e| Error::io(&dest_path, e))?;

                #[cfg(unix)]
                if entry.mode == crate::types::MODE_BLOB_EXEC {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&dest_path, perms)
                        .map_err(|e| Error::io(&dest_path, e))?;
                }
            }

            // Set mtime to commit timestamp when provided
            if let Some(ct) = commit_time {
                if entry.mode != MODE_LINK {
                    let ft = filetime::FileTime::from_unix_time(ct as i64, 0);
                    let _ = filetime::set_file_mtime(&dest_path, ft);
                }
            }

            if disk_set.contains(rel_path.as_str()) {
                report.update.push(FileEntry::with_src(rel_path, file_type, &dest_path));
            } else {
                report.add.push(FileEntry::with_src(rel_path, file_type, &dest_path));
            }
        }
    }

    // Delete disk files not in repo tree
    for rel_path in &disk_files {
        if !matches_filters(rel_path, include, exclude) {
            continue;
        }
        if !repo_map.contains_key(rel_path.as_str()) {
            let full_path = dest.join(rel_path);
            if full_path.exists() || full_path.symlink_metadata().is_ok() {
                std::fs::remove_file(&full_path).map_err(|e| Error::io(&full_path, e))?;
                report.delete.push(FileEntry::with_src(rel_path, FileType::Blob, &full_path));
            }
        }
    }

    // Prune empty directories
    prune_empty_dirs(dest)?;

    Ok(report)
}

/// Remove empty directories under `root`, bottom-up. Silently skips
/// directories that still contain files.
fn prune_empty_dirs(root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    // Collect all directories first, then try to remove bottom-up
    let mut dirs = Vec::new();
    collect_dirs(root, root, &mut dirs)?;
    // Sort by depth (deepest first) for bottom-up removal
    dirs.sort_by_key(|b| std::cmp::Reverse(b.len()));
    for dir in dirs {
        let full = root.join(&dir);
        // Try to remove — will fail silently if not empty
        let _ = std::fs::remove_dir(&full);
    }
    Ok(())
}

fn collect_dirs(root: &Path, dir: &Path, results: &mut Vec<String>) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    for entry in read_dir {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            results.push(rel);
            collect_dirs(root, &path, results)?;
        }
    }
    Ok(())
}

/// Remove files from disk that match the given include/exclude patterns.
///
/// # Arguments
/// * `dest` - Root directory to scan for files.
/// * `include` - Optional glob patterns; only matching files are removed.
/// * `exclude` - Optional glob patterns; matching files are kept.
pub fn remove_from_disk(
    dest: &Path,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
) -> Result<ChangeReport> {
    let mut report = ChangeReport::new();
    let files = disk_glob(dest, include, exclude)?;
    for rel in &files {
        let full = dest.join(rel);
        if full.exists() {
            std::fs::remove_file(&full).map_err(|e| Error::io(&full, e))?;
            report.delete.push(FileEntry::with_src(rel.as_str(), FileType::Blob, &full));
        }
    }
    Ok(report)
}

/// Rename a path within a tree, returning tree writes for the move.
///
/// Handles both single-file renames and directory renames (moving all
/// children). Each returned entry is either a deletion (`None`) of the
/// old path or a write (`Some(TreeWrite)`) at the new path.
///
/// # Arguments
/// * `repo` - The git repository.
/// * `base_tree` - Root tree OID of the current commit.
/// * `src` - Normalized source path in the tree.
/// * `dest` - Normalized destination path in the tree.
///
/// # Errors
/// Returns [`Error::NotFound`] if `src` does not exist in the tree.
pub fn rename(
    repo: &git2::Repository,
    base_tree: git2::Oid,
    src: &str,
    dest: &str,
) -> Result<Vec<(String, Option<TreeWrite>)>> {
    let src_norm = crate::paths::normalize_path(src)?;
    let dest_norm = crate::paths::normalize_path(dest)?;

    let entry = tree::entry_at_path(repo, base_tree, &src_norm)?
        .ok_or_else(|| Error::not_found(&src_norm))?;

    let mut writes = Vec::new();

    if entry.mode == MODE_TREE {
        // Rename directory: move all entries and delete originals
        let sub_entries = tree::walk_tree(repo, entry.oid)?;
        for (rel_path, we) in &sub_entries {
            let old_path = format!("{}/{}", src_norm, rel_path);
            let new_path = format!("{}/{}", dest_norm, rel_path);
            let blob = repo.find_blob(we.oid).map_err(Error::git)?;
            // Delete old path
            writes.push((old_path, None));
            // Write new path
            writes.push((
                new_path,
                Some(TreeWrite {
                    data: blob.content().to_vec(),
                    oid: we.oid,
                    mode: we.mode,
                }),
            ));
        }
    } else {
        // Rename single file: delete old, write new
        let blob = repo.find_blob(entry.oid).map_err(Error::git)?;
        writes.push((src_norm, None));
        writes.push((
            dest_norm,
            Some(TreeWrite {
                data: blob.content().to_vec(),
                oid: entry.oid,
                mode: entry.mode,
            }),
        ));
    }

    Ok(writes)
}

/// Recursively list all files under `root`, filtered by include/exclude
/// glob patterns. Returns sorted relative paths.
///
/// # Arguments
/// * `root` - Directory to walk.
/// * `include` - Optional glob patterns; only matching files are returned.
/// * `exclude` - Optional glob patterns; matching files are excluded.
pub fn disk_glob(
    root: &Path,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
) -> Result<Vec<String>> {
    let mut results = Vec::new();
    walk_disk_full(root, root, &mut results, false, &mut None, &mut std::collections::HashSet::new())?;

    if include.is_some() || exclude.is_some() {
        results.retain(|path| matches_filters(path, include, exclude));
    }

    results.sort();
    Ok(results)
}

/// Extended disk_glob with `follow_symlinks` and optional `ExcludeFilter`.
pub fn disk_glob_ext(
    root: &Path,
    include: Option<&[&str]>,
    exclude: Option<&[&str]>,
    follow_symlinks: bool,
    exclude_filter: &mut Option<ExcludeFilter>,
) -> Result<Vec<String>> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if follow_symlinks {
        // Seed with root's real path for cycle detection
        if let Ok(real) = std::fs::canonicalize(root) {
            seen.insert(real);
        }
    }
    walk_disk_full(root, root, &mut results, follow_symlinks, exclude_filter, &mut seen)?;

    // Filter by include/exclude
    if include.is_some() || exclude.is_some() {
        results.retain(|path| matches_filters(path, include, exclude));
    }

    results.sort();
    Ok(results)
}

/// Walk disk with optional `follow_symlinks` and `ExcludeFilter`.
///
/// When `follow_symlinks` is true, symlinks are dereferenced (files read
/// through symlinks appear as regular files/dirs).  When an `ExcludeFilter`
/// with `gitignore=true` is provided, `.gitignore` files are loaded per
/// directory and matching entries are skipped.
fn walk_disk_full(
    root: &Path,
    dir: &Path,
    results: &mut Vec<String>,
    follow_symlinks: bool,
    exclude_filter: &mut Option<ExcludeFilter>,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io(dir, e)),
    };

    // Enter directory for gitignore loading
    let rel_dir = dir
        .strip_prefix(root)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .into_owned();
    if let Some(ref mut ef) = exclude_filter {
        if ef.gitignore {
            ef.enter_directory(dir, &rel_dir);
        }
    }

    for entry in read_dir {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let path = entry.path();
        let meta = if follow_symlinks {
            match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error::io(&path, e)),
            }
        } else {
            std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?
        };

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();

        if meta.is_dir() {
            // Check gitignore/exclude_filter exclusion for directories
            if let Some(ref ef) = exclude_filter {
                if ef.is_excluded_in_walk(&rel, true) {
                    continue;
                }
            }
            // Cycle detection for follow_symlinks mode
            if follow_symlinks {
                if let Ok(real) = std::fs::canonicalize(&path) {
                    if !seen.insert(real) {
                        continue; // cycle detected, skip
                    }
                }
            }
            walk_disk_full(root, &path, results, follow_symlinks, exclude_filter, seen)?;
        } else {
            // Check gitignore/exclude_filter exclusion for files
            if let Some(ref ef) = exclude_filter {
                if ef.is_excluded_in_walk(&rel, false) {
                    continue;
                }
            }
            results.push(rel);
        }
    }
    Ok(())
}

fn matches_filters(path: &str, include: Option<&[&str]>, exclude: Option<&[&str]>) -> bool {
    if let Some(patterns) = include {
        if !patterns.iter().any(|pat| path_matches_glob(path, pat)) {
            return false;
        }
    }
    if let Some(patterns) = exclude {
        if patterns.iter().any(|pat| path_matches_glob(path, pat)) {
            return false;
        }
    }
    true
}

fn path_matches_glob(path: &str, pattern: &str) -> bool {
    // Simple: match the filename part against the pattern
    let filename = path.rsplit('/').next().unwrap_or(path);
    crate::glob::glob_match(pattern, filename) || crate::glob::glob_match(pattern, path)
}
