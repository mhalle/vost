use std::path::Path;

use crate::error::{Error, Result};
use crate::fs::{Fs, TreeWrite};
use crate::tree;
use crate::types::{MODE_BLOB, MODE_LINK};

/// Accumulates writes and removes, committing them atomically.
///
/// [`commit`](Batch::commit) takes ownership (`self`), so the compiler
/// enforces that no further writes happen after committing.
pub struct Batch {
    pub(crate) fs: Fs,
    pub(crate) writes: Vec<(String, Option<TreeWrite>)>,
    pub(crate) removes: Vec<String>,
    pub(crate) message: Option<String>,
    pub(crate) operation: Option<String>,
    pub(crate) parents: Vec<Fs>,
    pub(crate) closed: bool,
}

impl Batch {
    fn require_open(&self) -> Result<()> {
        if self.closed {
            Err(Error::BatchClosed)
        } else {
            Ok(())
        }
    }

    /// Write raw bytes to `path` with the default blob mode (`0o100644`).
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
        self.write_with_mode(path, data, MODE_BLOB)
    }

    /// Write raw bytes to `path` with an explicit file mode.
    pub fn write_with_mode(&mut self, path: &str, data: &[u8], mode: u32) -> Result<()> {
        self.require_open()?;
        let path = crate::paths::normalize_path(path)?;

        let tw = self.fs.with_repo(|repo| {
            let blob_oid = repo.blob(data).map_err(Error::git)?;
            Ok(TreeWrite {
                data: data.to_vec(),
                oid: blob_oid,
                mode,
            })
        })?;

        // Remove from removes if present
        self.removes.retain(|p| p != &path);
        // Remove existing write for same path
        self.writes.retain(|(p, _)| p != &path);
        self.writes.push((path, Some(tw)));
        Ok(())
    }

    /// Write the contents of a disk file to `path`.
    pub fn write_from_file(&mut self, path: &str, src: &Path) -> Result<()> {
        self.require_open()?;
        let data = std::fs::read(src).map_err(|e| Error::io(src, e))?;
        let mode = tree::mode_from_disk(src).unwrap_or(MODE_BLOB);
        self.write_with_mode(path, &data, mode)
    }

    /// Write a symlink at `path`.
    pub fn write_symlink(&mut self, path: &str, target: &str) -> Result<()> {
        self.require_open()?;
        let path = crate::paths::normalize_path(path)?;

        let tw = self.fs.with_repo(|repo| {
            let blob_oid = repo.blob(target.as_bytes()).map_err(Error::git)?;
            Ok(TreeWrite {
                data: target.as_bytes().to_vec(),
                oid: blob_oid,
                mode: MODE_LINK,
            })
        })?;

        self.removes.retain(|p| p != &path);
        self.writes.retain(|(p, _)| p != &path);
        self.writes.push((path, Some(tw)));
        Ok(())
    }

    /// Return a buffered [`BatchWriter`](crate::fileobj::BatchWriter) that
    /// stages to this batch on close.
    ///
    /// The writer implements [`std::io::Write`], so you can use `write_all` etc.
    pub fn writer(&mut self, path: &str) -> Result<crate::fileobj::BatchWriter<'_>> {
        self.require_open()?;
        let normalized = crate::paths::normalize_path(path)?;
        Ok(crate::fileobj::BatchWriter::new(self, normalized))
    }

    /// Mark `path` for removal.
    pub fn remove(&mut self, path: &str) -> Result<()> {
        self.require_open()?;
        let path = crate::paths::normalize_path(path)?;

        // Remove from writes if present
        self.writes.retain(|(p, _)| p != &path);

        if !self.removes.contains(&path) {
            self.removes.push(path);
        }
        Ok(())
    }

    /// Commit all accumulated writes and removes atomically.
    ///
    /// Consumes the `Batch` and returns the resulting [`Fs`] snapshot.
    /// If no writes or removes were recorded, the original `Fs` is
    /// returned unchanged (no empty commit is created).
    ///
    /// # Errors
    /// Returns an error if the batch is already closed or the commit fails.
    pub fn commit(mut self) -> Result<Fs> {
        self.closed = true;

        if self.writes.is_empty() && self.removes.is_empty() {
            return Ok(self.fs);
        }

        // Merge removes into writes as (path, None)
        let mut all_writes = self.writes;
        for path in self.removes {
            all_writes.push((path, None));
        }

        let message = self.message.unwrap_or_else(|| {
            crate::paths::format_commit_message(
                self.operation.as_deref().unwrap_or("batch"),
                None,
            )
        });

        let extra: Vec<&Fs> = self.parents.iter().collect();
        self.fs.commit_changes_with_parents(&all_writes, &message, &extra)
    }

    /// Returns `true` if `commit()` has already been called.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
