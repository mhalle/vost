# vost

A versioned filesystem backed by bare Git repositories. Store, retrieve, and version directory trees of files with text and binary data using an immutable-snapshot API. Every write produces a new commit. Old snapshots remain accessible forever.

This is the Rust port of [vost](https://github.com/mhalle/vost), using [gitoxide (gix)](https://github.com/GitoxideLabs/gitoxide) as the git backend. The repositories are standard Git repos that can be manipulated with Git tools as well.

## Installation

```toml
[dependencies]
vost = "0.9.0"
```

Or via the command line:

```bash
cargo add vost
```

Requires Rust 1.75+.

## Quick start

```rust
use vost::{GitStore, OpenOptions};
use vost::fs::WriteOptions;

let store = GitStore::open("/tmp/my-repo.git", OpenOptions {
    create: true,
    branch: Some("main".into()),
    ..Default::default()
})?;

// Get a snapshot of the "main" branch
let fs = store.branches().get("main")?;

// Write a file -- returns a new immutable snapshot
let fs = fs.write_text("hello.txt", "Hello, world!", WriteOptions::default())?;

// Read it back
assert_eq!(fs.read_text("hello.txt")?, "Hello, world!");

// Every write is a commit
println!("{}", fs.commit_hash().unwrap());  // full 40-char SHA
println!("{}", fs.message()?);              // "+ hello.txt"
```

## Core concepts

**Bare repository.** vost uses a bare Git repository with no working directory. All data lives inside Git's content-addressable object store and is accessed through the vost API.

**`GitStore`** opens or creates a bare repository. It exposes `branches()`, `tags()`, and `notes()`.

**`Fs`** is an immutable snapshot of a committed tree. Reading methods (`read`, `ls`, `walk`, `exists`) never mutate state. Writing methods (`write`, `write_text`, `remove`, `batch`) return a *new* `Fs` pointing at the new commit -- the original `Fs` is unchanged. `Fs` is cheap to clone (`Arc` internally) and can be stored in structs, returned from functions, and sent across threads.

Snapshots from **branches** are writable (`fs.writable() == true`). Snapshots from **tags** are read-only (`fs.writable() == false`).

**All operations are synchronous** and return `Result<T, vost::Error>`.

## API

### Opening a repository

```rust
use vost::{GitStore, OpenOptions};

// Create or open
let store = GitStore::open("data.git", OpenOptions {
    create: true,
    branch: Some("main".into()),
    ..Default::default()
})?;

// Open only (fails if missing)
let store = GitStore::open("data.git", OpenOptions::default())?;

// Custom author identity
let store = GitStore::open("data.git", OpenOptions {
    create: true,
    branch: Some("main".into()),
    author: Some("alice".into()),
    email: Some("alice@example.com".into()),
})?;

// Inspect
store.path();        // &Path -- repo directory
store.signature();   // &Signature -- { name, email }
```

### Branches and tags

`store.branches()` and `store.tags()` both return a `RefDict`. Branches yield writable `Fs` snapshots; tags yield read-only ones.

```rust
let fs = store.branches().get("main")?;
store.branches().set("experiment", &fs)?;        // fork a branch
store.branches().delete("experiment")?;           // delete a branch

store.tags().set("v1.0", &fs)?;                  // create a tag (immutable)
let tagged = store.tags().get("v1.0")?;           // read-only Fs

let name = store.branches().get_current_name()?;  // Option<String>
let current = store.branches().get_current()?;    // Option<Fs>
store.branches().set_current("dev")?;             // set HEAD

let names = store.branches().list()?;             // Vec<String>, sorted
let has = store.branches().has("main")?;          // bool

// set_and_get: set + get in one call, returns writable Fs
let fs = store.branches().set_and_get("copy", &fs)?;

// Iterate all (name, Fs) pairs
for (name, fs) in store.branches().iter()? {
    println!("{}: {}", name, fs.commit_hash().unwrap_or_default());
}

// Reflog
let entries = store.branches().reflog("main")?;   // Vec<ReflogEntry>
for entry in &entries {
    println!("{} -> {} {}", entry.old_sha, entry.new_sha, entry.message);
}
```

### Reading

```rust
let data: Vec<u8> = fs.read("path/to/file.bin")?;                   // raw bytes
let text: String = fs.read_text("config.json")?;                     // UTF-8 string
let chunk = fs.read_range("big.bin", 100, Some(50))?;                // partial read
let chunk = fs.read_by_hash(&sha, 0, Some(1024))?;                  // read blob by SHA

let names: Vec<String> = fs.ls("")?;                                 // root listing
let names: Vec<String> = fs.ls("src")?;                              // subdirectory
let entries: Vec<WalkEntry> = fs.listdir("src")?;                    // name + oid + mode
let exists: bool = fs.exists("path/to/file.bin")?;
let info: StatResult = fs.stat("path/to/file.bin")?;                 // mode, size, hash, ...
let ft: FileType = fs.file_type("run.sh")?;                          // Blob, Executable, Link, Tree
let nbytes: u64 = fs.size("path/to/file.bin")?;
let sha: String = fs.object_hash("path/to/file.bin")?;               // 40-char hex SHA
let link_target: String = fs.readlink("symlink")?;
let is_dir: bool = fs.is_dir("src")?;

// Root tree hash
let tree_sha: Option<String> = fs.tree_hash();

// Walk the tree (os.walk-style)
for entry in fs.walk("")? {                                           // Vec<WalkDirEntry>
    println!("dir: {}", entry.dirpath);
    for file in &entry.files {
        println!("  {} mode={:#o}", file.name, file.mode);
    }
}

// Glob (dotfile-aware, supports *, ?, **)
let matches: Vec<String> = fs.glob("**/*.rs")?;                      // sorted
let matches: Vec<String> = fs.iglob("**/*.rs")?;                     // unsorted (faster)
```

### Writing

Every write auto-commits and returns a new snapshot:

```rust
use vost::fs::WriteOptions;
use vost::types::MODE_BLOB_EXEC;

let fs = fs.write_text("config.json", "{\"key\": \"value\"}", WriteOptions::default())?;

let fs = fs.write_text("script.sh", "#!/bin/sh\n", WriteOptions {
    mode: Some(MODE_BLOB_EXEC),
    ..Default::default()
})?;

let fs = fs.write_text("config.json", "{}", WriteOptions {
    message: Some("Reset config".into()),
    ..Default::default()
})?;

let fs = fs.write("image.png", &raw_bytes, WriteOptions::default())?;

let fs = fs.write_from_file(
    "big.bin",
    Path::new("/data/big.bin"),
    WriteOptions::default(),
)?;

let fs = fs.write_symlink("link", "target", WriteOptions::default())?;

// Remove files
use vost::fs::RemoveOptions;
let fs = fs.remove(&["old-file.txt"], RemoveOptions::default())?;
let fs = fs.remove(&["old-dir"], RemoveOptions { recursive: true, ..Default::default() })?;

// Rename
let fs = fs.rename("old.txt", "new.txt", WriteOptions::default())?;

// Move (POSIX mv semantics)
use vost::fs::MoveOptions;
let fs = fs.move_paths(&["a.txt", "b.txt"], "dest-dir", MoveOptions {
    recursive: true,
    ..Default::default()
})?;
```

The original `Fs` is never mutated:

```rust
let fs1 = store.branches().get("main")?;
let fs2 = fs1.write_text("new.txt", "data", WriteOptions::default())?;
assert!(!fs1.exists("new.txt")?);   // fs1 is unchanged
assert!(fs2.exists("new.txt")?);    // fs2 has the new file
```

Buffered writers implement `std::io::Write`:

```rust
// FsWriter -- commits on close
let mut w = fs.writer("output.bin")?;
w.write_all(b"chunk 1")?;
w.write_all(b"chunk 2")?;
let fs = w.close()?;

// BatchWriter -- stages to a Batch
let mut batch = fs.batch(Default::default());
let mut bw = batch.writer("output.bin")?;
bw.write_all(b"data")?;
bw.close()?;
let fs = batch.commit()?;
```

### Batch writes

Multiple writes and removes in a single atomic commit:

```rust
use vost::fs::BatchOptions;

let mut batch = fs.batch(BatchOptions {
    message: Some("Import dataset v2".into()),
    ..Default::default()
});

batch.write("a.txt", b"alpha")?;
batch.write_with_mode("script.sh", b"#!/bin/sh\n", MODE_BLOB_EXEC)?;  // explicit mode
batch.write_from_file("big.bin", Path::new("/data/big.bin"))?;
batch.write_symlink("link.txt", "a.txt")?;
batch.remove("old.txt")?;

assert!(!batch.is_closed());
let fs = batch.commit()?;  // single atomic commit; consumes the Batch
```

`Batch::commit(self)` takes ownership, so the compiler prevents writes after committing.
`is_closed()` returns `true` after `commit()`.

### Atomic apply

Apply multiple writes and removes in a single commit without a Batch:

```rust
use vost::{WriteEntry, fs::ApplyOptions};

let fs = fs.apply(
    &[
        ("config.json", WriteEntry::from_text("{\"v\": 2}")),
        ("script.sh", WriteEntry { mode: MODE_BLOB_EXEC, ..WriteEntry::from_text("#!/bin/sh\n") }),
        ("link", WriteEntry::symlink("config.json")),
    ],
    &["old.txt", "deprecated/file.txt"],   // removes
    ApplyOptions { message: Some("Update config and clean up".into()), ..Default::default() },
)?;
```

### History

```rust
use vost::fs::LogOptions;

let parent: Option<Fs> = fs.parent()?;                // parent snapshot
let ancestor: Fs = fs.back(3)?;                        // 3 commits back

// Commit log
let entries = fs.log(LogOptions::default())?;          // Vec<CommitInfo>
for entry in &entries {
    println!("{} {}", entry.commit_hash, entry.message);
}

// Filtered log
let entries = fs.log(LogOptions {
    path: Some("config.json".into()),                  // only commits touching this file
    limit: Some(10),
    ..Default::default()
})?;

// Undo / redo (move branch pointer via reflog)
let fs = fs.undo(1)?;                                 // move branch back 1 commit
let fs = fs.redo(1)?;                                 // move branch forward 1 reflog step
```

### Copy and sync

```rust
use vost::fs::{CopyInOptions, CopyOutOptions, SyncOptions, CopyFromRefOptions};
use std::path::Path;

// Disk to repo
let (report, fs) = fs.copy_in(
    Path::new("./data"),
    "backup",
    CopyInOptions::default(),
)?;
println!("added {} files", report.add.len());

// Repo to disk
let report = fs.copy_out("docs", Path::new("./local-docs"), CopyOutOptions::default())?;

// Copy between branches (atomic, no disk I/O -- blobs shared by OID)
let main = store.branches().get("main")?;
let dev = store.branches().get("dev")?;
let dev = dev.copy_from_ref(&main, &["config"], "imported", CopyFromRefOptions::default())?;

// Sync (make identical, including deletes)
let (report, fs) = fs.sync_in(
    Path::new("./local"),
    "data",
    SyncOptions::default(),
)?;
let report = fs.sync_out("data", Path::new("./local"), SyncOptions::default())?;
```

All copy/sync methods accept `include`/`exclude` glob filters and a `checksum` flag for content-based deduplication. The `dry_run` flag previews changes without committing.

```rust
// Remove files from disk (reverse of copy_in)
use vost::fs::RemoveFromDiskOptions;

let report = fs.remove_from_disk(
    Path::new("./local-docs"),
    RemoveFromDiskOptions::default(),
)?;
println!("deleted {} files", report.delete.len());
```

### Snapshot properties

```rust
fs.commit_hash()     // Option<String> -- full 40-char commit SHA
fs.tree_hash()       // Option<String> -- root tree SHA
fs.ref_name()        // Option<&str> -- branch or tag name
fs.writable()        // bool -- true for branches
fs.changes()         // Option<&ChangeReport> -- set after write/copy/sync/remove

// These read from the commit object
fs.message()?        // String -- commit message
fs.time()?           // u64 -- commit timestamp (epoch seconds)
fs.author_name()?    // String
fs.author_email()?   // String
```

### Git notes

Attach metadata to commits without modifying history. Notes can be addressed by commit hash or ref name (branch/tag):

```rust
// Default namespace (refs/notes/commits)
let ns = store.notes().commits();

// By commit hash
ns.set(&fs.commit_hash().unwrap(), "reviewed by Alice")?;
println!("{}", ns.get(&fs.commit_hash().unwrap())?);    // "reviewed by Alice"

// By branch or tag name (resolves to tip commit)
ns.set("main", "deployed to staging")?;
println!("{}", ns.get("main")?);                         // "deployed to staging"

ns.delete(&fs.commit_hash().unwrap())?;

// Custom namespaces
let reviews = store.notes().namespace("reviews");
reviews.set("main", "LGTM")?;

// Batch writes (single commit)
let mut batch = ns.batch();
batch.set("main", "note for main")?;
batch.set("dev", "note for dev")?;
batch.commit()?;

// Query
let hashes: Vec<String> = ns.list()?;            // all annotated commit hashes
let count: usize = ns.len()?;
let empty: bool = ns.is_empty()?;
let has: bool = ns.has("main")?;

// Current branch helpers
ns.set_for_current_branch(&store, "deployed")?;
let note = ns.get_for_current_branch(&store)?;
```

### Backup and restore

Mirror all refs to or from a remote repository:

```rust
use vost::types::{BackupOptions, RestoreOptions};

let diff = store.backup("https://github.com/user/repo.git", &BackupOptions::default())?;
let diff = store.restore("https://github.com/user/repo.git", &RestoreOptions::default())?;

// Dry run
let diff = store.backup(url, &BackupOptions { dry_run: true, ..Default::default() })?;

// Bundle file (auto-detected from .bundle extension)
let diff = store.backup("backup.bundle", &BackupOptions::default())?;
let diff = store.restore("backup.bundle", &RestoreOptions::default())?;

// Specific refs only
let diff = store.backup(url, &BackupOptions {
    refs: Some(vec!["main".into(), "v1.0".into()]),
    ..Default::default()
})?;

println!("added: {}, updated: {}, deleted: {}", diff.add.len(), diff.update.len(), diff.delete.len());
```

### Bundle export and import

Create and import bundle files directly:

```rust
store.bundle_export("backup.bundle", None, None, false)?;                      // all refs
store.bundle_export("backup.bundle", Some(&["main".into()]), None, false)?;   // specific refs
store.bundle_import("backup.bundle", None)?;                                  // import all (additive)
store.bundle_import("backup.bundle", Some(&["main".into()]))?;                // specific refs
```

## Concurrency safety

vost uses an advisory file lock (`vost.lock`) to make the stale-snapshot check and ref update atomic. If a branch advances after you obtain a snapshot, writing from the stale snapshot returns `Err(Error::StaleSnapshot(_))`:

```rust
use vost::fs::{retry_write, WriteOptions};

let fs = store.branches().get("main")?;
let _ = fs.write_text("a.txt", "a", WriteOptions::default())?;  // advances the branch

match fs.write_text("b.txt", "b", WriteOptions::default()) {
    Err(vost::Error::StaleSnapshot(_)) => {
        // Re-fetch and retry
        let fs = store.branches().get("main")?;
        let _ = fs.write_text("b.txt", "b", WriteOptions::default())?;
    }
    other => { other?; }
}

// Or use retry_write for automatic retry with exponential backoff (up to 5 attempts)
let fs = retry_write(|| {
    let fs = store.branches().get("main")?;
    fs.write_text("file.txt", "data", WriteOptions::default())
})?;
```

## Error handling

All fallible operations return `Result<T, vost::Error>`. The `Error` enum has the following variants:

| Variant | When |
|---------|------|
| `NotFound` | `read`/`remove` on a missing path; repo does not exist |
| `IsADirectory` | `read` on a directory path |
| `NotADirectory` | `ls`/`walk` on a file path |
| `Permission` | Writing to a tag or detached snapshot |
| `StaleSnapshot` | Writing from a snapshot whose branch has moved forward |
| `KeyNotFound` | Accessing a missing branch, tag, or note |
| `KeyExists` | Overwriting an existing tag |
| `InvalidPath` | Invalid path (`..`, empty segments, etc.) |
| `InvalidHash` | Malformed 40-char hex SHA |
| `InvalidRefName` | Invalid characters in branch/tag name |
| `BatchClosed` | Writing to a `Batch` after `commit()` |
| `Git` | Low-level gitoxide operation failure |
| `Io` | Filesystem I/O error |

Pattern matching on the error variant:

```rust
match fs.read("missing.txt") {
    Ok(data) => { /* use data */ }
    Err(vost::Error::NotFound(_)) => println!("file does not exist"),
    Err(vost::Error::IsADirectory(_)) => println!("path is a directory"),
    Err(e) => return Err(e),
}
```

## Documentation

- Run `cargo doc --open` in the `rs/` directory for full API docs generated from source.
- [Python version](https://github.com/mhalle/vost) -- the reference implementation with CLI.
- [TypeScript version](https://github.com/mhalle/vost/tree/master/ts) -- isomorphic-git port.

## License

Apache-2.0 -- see [LICENSE](../LICENSE) for details.
