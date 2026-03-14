# Python API Reference

```python
from vost import GitStore, FS, Batch, WriteEntry, StaleSnapshotError, retry_write
from vost import ChangeReport, ChangeAction, ChangeActionKind, ChangeError, FileEntry, FileType
from vost import MirrorDiff, RefChange, ReflogEntry, Signature, WalkEntry
from vost import NoteDict, NoteNamespace, NotesBatch
from vost import ExcludeFilter, BlobOid, StatResult, disk_glob
```

---

## GitStore

::: vost.GitStore
    options:
      members:
        - open
        - fs
        - branches
        - tags
        - notes
        - backup
        - restore
        - bundle_export
        - bundle_import

---

## Operations Overview

vost has four ways to move data. They differ in **what** they transfer and **how much** they replace at the destination.

| Operation | What it transfers | Destination behavior | Scope |
|-----------|-------------------|---------------------|-------|
| **copy** | Individual files and directories | Additive — existing files are kept unless `--delete` is passed | Selected paths |
| **sync** | A directory tree | Exact mirror — destination matches source, extras are deleted | One directory tree |
| **archive** | A snapshot as a single file | Exports to (or imports from) a `.zip`/`.tar` archive | One branch or tag |
| **backup / restore** | The entire repository | All branches, tags, and history are pushed to (or fetched from) a remote git repo or bundle file | Whole repo |

**copy** and **sync** work with individual files between disk and repo (or within the repo). The difference is that copy is additive by default — it only adds or updates files — while sync makes the destination an exact replica of the source, deleting anything extra.

**archive** serializes one snapshot (branch, tag, or historical commit) into a single archive file, or imports one back. No git history is preserved — just the file tree at that point in time.

**backup** and **restore** operate at the git level. They push or fetch all refs (branches, tags) and their full commit history to another git repository. This is for disaster recovery and replication, not for working with individual files.

| I want to... | Use |
|--------------|-----|
| Copy specific files into or out of the repo | `copy_in` / `copy_out` (`cp`) |
| Make a repo directory match a local directory | `sync_in` / `sync_out` (`sync`) |
| Export a snapshot as a zip/tar | `archive_out` / `archive_in` |
| Replicate the entire repo to another location | `backup` / `restore` |
| Create a portable bundle file | `bundle_export` / `bundle_import` |

---

## FS (Snapshot)

::: vost.FS
    options:
      members: false

### Snapshot Properties

::: vost.FS
    options:
      show_root_heading: false
      members:
        - writable
        - commit_hash
        - ref_name
        - tree_hash
        - message
        - time
        - author_name
        - author_email
        - changes

### Querying Files

::: vost.FS
    options:
      show_root_heading: false
      members:
        - exists
        - is_dir
        - file_type
        - size
        - object_hash
        - stat

### Reading Files

::: vost.FS
    options:
      show_root_heading: false
      members:
        - read
        - read_text
        - read_by_hash
        - readlink

### Listing & Search

::: vost.FS
    options:
      show_root_heading: false
      members:
        - ls
        - listdir
        - walk
        - glob
        - iglob

### Writing Files

::: vost.FS
    options:
      show_root_heading: false
      members:
        - write
        - write_text
        - write_from_file
        - write_symlink
        - writer
        - batch

### Bulk Operations

::: vost.FS
    options:
      show_root_heading: false
      members:
        - copy_in
        - copy_out
        - sync_in
        - sync_out
        - remove
        - move
        - copy_from_ref
        - apply

### History & Navigation

::: vost.FS
    options:
      show_root_heading: false
      members:
        - parent
        - back
        - log
        - undo
        - redo

### Lifecycle

::: vost.FS
    options:
      show_root_heading: false
      members:
        - close

---

## Batch

::: vost.Batch
    options:
      members:
        - write
        - write_from_file
        - write_text
        - write_symlink
        - remove
        - writer
        - commit

---

## Branches & Tags

::: vost.RefDict
    options:
      members:
        - set
        - current
        - current_name
        - reflog

::: vost.Signature

::: vost.ReflogEntry

---

## Notes

::: vost.NoteDict

::: vost.NoteNamespace

::: vost.NotesBatch

---

## Backup & Restore

See `GitStore.backup()` and `GitStore.restore()`.

For direct bundle file operations without the full backup/restore logic, see `GitStore.bundle_export()` and `GitStore.bundle_import()`.

---

## Exclude Filter

::: vost.ExcludeFilter

---

## Exceptions

::: vost.StaleSnapshotError

---

## Utility Functions

::: vost.retry_write

::: vost.resolve_credentials

::: vost.disk_glob

---

## Data Types

Types returned by API methods. Most are opaque — you rarely need to construct or import them directly.

::: vost.StatResult

::: vost.WalkEntry

::: vost.FileType

::: vost.FileEntry

::: vost.BlobOid

::: vost.WriteEntry

::: vost.ChangeReport

::: vost.ChangeAction

::: vost.ChangeActionKind

::: vost.ChangeError

::: vost.MirrorDiff

::: vost.RefChange
