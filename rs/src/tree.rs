use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::types::{WalkDirEntry, WalkEntry, MODE_BLOB, MODE_BLOB_EXEC, MODE_LINK, MODE_TREE};

/// Result of looking up a single tree entry.
#[derive(Debug, Clone)]
pub struct TreeEntryResult {
    pub oid: git2::Oid,
    pub mode: u32,
}

/// Return the `(oid, mode)` of the entry at `path`, or `None` if missing.
///
/// Walks the tree from `tree_oid` through each path segment. Returns `None`
/// when any segment is not found or an intermediate entry is not a tree.
///
/// # Arguments
/// * `repo` - The git repository.
/// * `tree_oid` - Root tree to search from.
/// * `path` - Normalized forward-slash path (e.g. `"dir/file.txt"`).
pub fn entry_at_path(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<Option<TreeEntryResult>> {
    let path = crate::paths::normalize_path(path)?;
    if path.is_empty() {
        return Ok(Some(TreeEntryResult {
            oid: tree_oid,
            mode: MODE_TREE,
        }));
    }

    let segments: Vec<&str> = path.split('/').collect();
    let mut current_oid = tree_oid;

    for (i, segment) in segments.iter().enumerate() {
        let tree = repo.find_tree(current_oid).map_err(Error::git)?;

        let entry_info = tree.get_name(segment).map(|e| (e.id(), e.filemode() as u32));

        match entry_info {
            Some((entry_oid, entry_mode)) => {
                if i == segments.len() - 1 {
                    // Last segment — return this entry
                    return Ok(Some(TreeEntryResult {
                        oid: entry_oid,
                        mode: entry_mode,
                    }));
                } else {
                    // Intermediate segment — must be a tree
                    if entry_mode != MODE_TREE {
                        return Ok(None);
                    }
                    current_oid = entry_oid;
                }
            }
            None => return Ok(None),
        }
    }

    Ok(None)
}

/// Walk to a path within a tree, returning every entry along the way.
///
/// Unlike [`entry_at_path`], this returns the full chain of
/// [`TreeEntryResult`] objects from the first segment to the last.
///
/// # Errors
/// Returns [`Error::NotFound`] if a segment is missing, or
/// [`Error::NotADirectory`] if an intermediate entry is not a tree.
pub fn walk_to(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<Vec<TreeEntryResult>> {
    let path = crate::paths::normalize_path(path)?;
    if path.is_empty() {
        return Ok(vec![TreeEntryResult {
            oid: tree_oid,
            mode: MODE_TREE,
        }]);
    }

    let segments: Vec<&str> = path.split('/').collect();
    let mut current_oid = tree_oid;
    let mut results = Vec::new();

    for (i, segment) in segments.iter().enumerate() {
        let tree = repo.find_tree(current_oid).map_err(Error::git)?;

        let entry_info = tree.get_name(segment).map(|e| (e.id(), e.filemode() as u32));

        match entry_info {
            Some((entry_oid, entry_mode)) => {
                results.push(TreeEntryResult {
                    oid: entry_oid,
                    mode: entry_mode,
                });

                if i < segments.len() - 1 {
                    if entry_mode != MODE_TREE {
                        return Err(Error::not_a_directory(segments[..=i].join("/")));
                    }
                    current_oid = entry_oid;
                }
            }
            None => {
                return Err(Error::not_found(segments[..=i].join("/")));
            }
        }
    }

    Ok(results)
}

/// Read a blob at a given path in the tree, returning its raw bytes.
///
/// # Errors
/// Returns [`Error::IsADirectory`] if the path points to a tree,
/// [`Error::NotFound`] if the path does not exist.
pub fn read_blob_at_path(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<Vec<u8>> {
    let results = walk_to(repo, tree_oid, path)?;
    let last = results
        .last()
        .ok_or_else(|| Error::not_found(path))?;

    if last.mode == MODE_TREE {
        return Err(Error::is_a_directory(path));
    }

    let blob = repo.find_blob(last.oid).map_err(Error::git)?;
    Ok(blob.content().to_vec())
}

/// List the immediate children of a tree at the given path.
///
/// Returns [`WalkEntry`] objects with `name`, `oid`, and `mode` for each
/// child. Pass an empty or root path to list the top-level tree.
///
/// # Errors
/// Returns [`Error::NotFound`] if the path does not exist, or
/// [`Error::NotADirectory`] if it is not a tree.
pub fn list_tree_at_path(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<Vec<WalkEntry>> {
    let target_oid = if crate::paths::is_root_path(path) {
        tree_oid
    } else {
        let entry = entry_at_path(repo, tree_oid, path)?
            .ok_or_else(|| Error::not_found(path))?;
        if entry.mode != MODE_TREE {
            return Err(Error::not_a_directory(path));
        }
        entry.oid
    };

    let tree = repo.find_tree(target_oid).map_err(Error::git)?;
    let mut entries = Vec::new();
    for i in 0..tree.len() {
        let e = tree.get(i).unwrap();
        entries.push(WalkEntry {
            name: e.name().unwrap_or("").to_string(),
            oid: e.id(),
            mode: e.filemode() as u32,
        });
    }
    Ok(entries)
}

/// List all entries recursively under the given path.
///
/// Returns a flat list of non-tree [`WalkEntry`] items with their names
/// (basenames, not full paths). Directories are traversed but not included
/// in the output.
pub fn list_entries_at_path(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<Vec<WalkEntry>> {
    let target_oid = if crate::paths::is_root_path(path) {
        tree_oid
    } else {
        let entry = entry_at_path(repo, tree_oid, path)?
            .ok_or_else(|| Error::not_found(path))?;
        if entry.mode != MODE_TREE {
            return Err(Error::not_a_directory(path));
        }
        entry.oid
    };

    let entries = walk_tree(repo, target_oid)?;
    Ok(entries.into_iter().map(|(_path, entry)| entry).collect())
}

/// Recursively walk a tree, returning all non-tree entries with full paths.
///
/// Each element is a `(full_path, WalkEntry)` pair where `full_path` is
/// the slash-separated path from the tree root (e.g. `"dir/sub/file.txt"`).
pub fn walk_tree(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
) -> Result<Vec<(String, WalkEntry)>> {
    let mut results = Vec::new();
    walk_tree_recursive(repo, tree_oid, "", &mut results)?;
    Ok(results)
}

fn walk_tree_recursive(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    prefix: &str,
    results: &mut Vec<(String, WalkEntry)>,
) -> Result<()> {
    let tree = repo.find_tree(tree_oid).map_err(Error::git)?;

    for i in 0..tree.len() {
        let e = tree.get(i).unwrap();
        let name = e.name().unwrap_or("").to_string();
        let full_path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", prefix, name)
        };
        let entry_mode = e.filemode() as u32;
        let entry_oid = e.id();

        if entry_mode == MODE_TREE {
            walk_tree_recursive(repo, entry_oid, &full_path, results)?;
        } else {
            results.push((
                full_path,
                WalkEntry {
                    name,
                    oid: entry_oid,
                    mode: entry_mode,
                },
            ));
        }
    }
    Ok(())
}

/// os.walk-style directory traversal: returns one [`WalkDirEntry`] per directory.
///
/// Each entry contains the directory path, a list of subdirectory names, and
/// a list of non-directory [`WalkEntry`] items (files, symlinks).
pub fn walk_tree_dirs(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
) -> Result<Vec<WalkDirEntry>> {
    let mut results = Vec::new();
    walk_tree_dirs_recursive(repo, tree_oid, "", &mut results)?;
    Ok(results)
}

fn walk_tree_dirs_recursive(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    prefix: &str,
    results: &mut Vec<WalkDirEntry>,
) -> Result<()> {
    let tree = repo.find_tree(tree_oid).map_err(Error::git)?;

    let mut entry = WalkDirEntry {
        dirpath: prefix.to_string(),
        dirnames: Vec::new(),
        files: Vec::new(),
    };

    let mut subdirs: Vec<(String, git2::Oid)> = Vec::new();

    for i in 0..tree.len() {
        let e = tree.get(i).unwrap();
        let name = e.name().unwrap_or("").to_string();
        let entry_mode = e.filemode() as u32;
        let entry_oid = e.id();

        if entry_mode == MODE_TREE {
            entry.dirnames.push(name.clone());
            subdirs.push((name, entry_oid));
        } else {
            entry.files.push(WalkEntry {
                name,
                oid: entry_oid,
                mode: entry_mode,
            });
        }
    }

    results.push(entry);

    for (dname, doid) in subdirs {
        let sub_prefix = if prefix.is_empty() {
            dname
        } else {
            format!("{}/{}", prefix, dname)
        };
        walk_tree_dirs_recursive(repo, doid, &sub_prefix, results)?;
    }

    Ok(())
}

/// Check whether an entry exists at the given path in the tree.
///
/// Returns `Ok(true)` if the path resolves to any object (blob, tree,
/// symlink), `Ok(false)` if not found.
pub fn exists_at_path(
    repo: &git2::Repository,
    tree_oid: git2::Oid,
    path: &str,
) -> Result<bool> {
    Ok(entry_at_path(repo, tree_oid, path)?.is_some())
}

/// Count immediate subdirectory entries in a tree (no recursion).
///
/// Used to compute `nlink` for directory stat results.
pub fn count_subdirs(repo: &git2::Repository, tree_oid: git2::Oid) -> Result<u32> {
    let tree = repo.find_tree(tree_oid).map_err(Error::git)?;
    let mut count = 0u32;
    for i in 0..tree.len() {
        let e = tree.get(i).unwrap();
        if e.filemode() as u32 == MODE_TREE {
            count += 1;
        }
    }
    Ok(count)
}

/// Rebuild a tree by applying writes and deletes.
///
/// Only the ancestor chain from changed leaves to root is rebuilt;
/// sibling subtrees are shared by hash reference. Empty directories
/// are automatically pruned.
///
/// # Arguments
/// * `repo` - The git repository.
/// * `base_tree` - OID of the existing tree (zero OID for empty).
/// * `writes` - Slice of `(path, Option<TreeWrite>)`. `Some` means add/update,
///   `None` means delete.
///
/// # Returns
/// OID of the new root tree.
pub fn rebuild_tree(
    repo: &git2::Repository,
    base_tree: git2::Oid,
    writes: &[(String, Option<crate::fs::TreeWrite>)],
) -> Result<git2::Oid> {
    // Group writes by first path segment
    let mut leaf_writes: BTreeMap<String, &crate::fs::TreeWrite> = BTreeMap::new();
    let mut leaf_removes: Vec<String> = Vec::new();
    let mut sub_writes: BTreeMap<String, Vec<(String, Option<&crate::fs::TreeWrite>)>> =
        BTreeMap::new();

    for (path, tw) in writes {
        if let Some(slash) = path.find('/') {
            let dir = &path[..slash];
            let rest = &path[slash + 1..];
            sub_writes
                .entry(dir.to_string())
                .or_default()
                .push((rest.to_string(), tw.as_ref()));
        } else {
            match tw {
                Some(tw) => {
                    leaf_writes.insert(path.clone(), tw);
                }
                None => {
                    leaf_removes.push(path.clone());
                }
            }
        }
    }

    // Load base tree entries into a sorted map
    let mut entries: BTreeMap<String, (git2::Oid, u32)> = BTreeMap::new();

    let is_zero = base_tree.is_zero();
    if !is_zero {
        if let Ok(tree) = repo.find_tree(base_tree) {
            for i in 0..tree.len() {
                let e = tree.get(i).unwrap();
                let name = e.name().unwrap_or("").to_string();
                entries.insert(name, (e.id(), e.filemode() as u32));
            }
        }
    }

    // Apply leaf writes
    for (name, tw) in &leaf_writes {
        entries.insert(name.clone(), (tw.oid, tw.mode));
    }

    // Apply leaf removes
    for name in &leaf_removes {
        entries.remove(name);
    }

    // Recurse into subdirectories
    for (dir, sub_changes) in &sub_writes {
        let existing_subtree = entries
            .get(dir)
            .and_then(|(oid, mode)| {
                if *mode == MODE_TREE {
                    Some(*oid)
                } else {
                    None
                }
            })
            .unwrap_or_else(git2::Oid::zero);

        // If there's a non-tree entry at this name, remove it (blob→tree transition)
        if let Some((_, mode)) = entries.get(dir) {
            if *mode != MODE_TREE {
                entries.remove(dir);
            }
        }

        // Convert sub_changes to owned format for recursion
        let owned_writes: Vec<(String, Option<crate::fs::TreeWrite>)> = sub_changes
            .iter()
            .map(|(path, tw)| (path.clone(), tw.cloned()))
            .collect();

        let new_subtree_oid = rebuild_tree(repo, existing_subtree, &owned_writes)?;

        // Check if result tree is empty (prune)
        let subtree = repo.find_tree(new_subtree_oid).map_err(Error::git)?;

        if subtree.len() == 0 {
            entries.remove(dir);
        } else {
            entries.insert(dir.clone(), (new_subtree_oid, MODE_TREE));
        }
    }

    // Build and write new tree using TreeBuilder
    let mut builder = repo.treebuilder(None).map_err(Error::git)?;

    for (name, (oid, mode)) in &entries {
        builder.insert(name, *oid, *mode as i32).map_err(Error::git)?;
    }

    let tree_oid = builder.write().map_err(Error::git)?;
    Ok(tree_oid)
}

/// Determine the git filemode for a file on disk.
///
/// Returns [`MODE_LINK`] for symlinks, [`MODE_BLOB_EXEC`] for executable
/// files (Unix only), or [`MODE_BLOB`] otherwise.
pub fn mode_from_disk(path: &std::path::Path) -> Result<u32> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.file_type().is_symlink() {
        return Ok(MODE_LINK);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Ok(MODE_BLOB_EXEC);
        }
    }
    Ok(MODE_BLOB)
}

/// Compute the git blob object hash for raw data without a repository.
///
/// Returns the 40-character lowercase hex SHA-1 that git would assign
/// to a blob containing `data`.
pub fn hash_blob(data: &[u8]) -> Result<String> {
    let oid = git2::Oid::hash_object(git2::ObjectType::Blob, data)
        .map_err(Error::git)?;
    Ok(oid.to_string())
}
