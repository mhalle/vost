# vost

**[Documentation](https://mhalle.github.io/vost/)**

vost is a versioned filesystem backed by bare Git repositories. Store, retrieve, and version directory trees of files with text and binary data using an immutable-snapshot API. Unlike Git, every write (or batch of writes and deletes) produces a new commit. Old snapshots remain accessible forever.

The repositories are standard Git repositories that can be manipulated with Git tools as well.

vost includes an intuitive Python API and an optional command line interface. The CLI includes operations to create repositories, copy and write into or out of them using rsync-like syntax, archive to zip or tar files, even act as an HTTP or Git server for a given snapshot or branch.

## Installation

```
pip install vost            # core library (dulwich only)
pip install "vost[cli]"    # adds the vost command-line tool
```

Or run the CLI without installing:

```
uvx "vost[cli]" -r myrepo.git ls : # run via uvx
uv tool install "vost[cli]"       # or install as a persistent tool
```

Requires Python 3.10+.

> **Note:** The `gc` CLI command shells out to an installed `git` executable. All other commands and the entire Python API are self-contained (pure dulwich).

## Quick start

```python
from vost import GitStore

# Create (or open) a repository with a "main" branch
repo = GitStore.open("data.git")

# Get a snapshot of the current branch ("main" by default)
fs = repo.branches.current

# Write a file -- returns a new immutable snapshot
fs = fs.write_text("hello.txt", "Hello, world!")

# Read it back
print(fs.read_text("hello.txt"))  # 'Hello, world!'

# Every write is a commit
print(fs.commit_hash)           # full 40-char SHA
print(fs.message)               # '+ hello.txt'
```

## Core concepts

**Bare repository.** vost uses a *bare* Git repository -- one that contains only Git's internal object database, with no working directory or checked-out files. You won't see your stored files by browsing the repo directory; all data lives inside Git's content-addressable object store and is accessed exclusively through the vost API. This is by design: it avoids filesystem conflicts, keeps the storage compact, and lets Git handle deduplication and integrity.

**`GitStore`** opens or creates a bare repository. It exposes `branches` and `tags` as [`MutableMapping`](https://docs.python.org/3/library/collections.abc.html#collections.abc.MutableMapping) objects (supporting `.get`, `.keys`, `.values`, `.items`, etc.).

**`FS`** is an immutable snapshot of a committed tree. Reading methods (`read`, `ls`, `walk`, `exists`, `open`) never mutate state. Writing methods (`write`, `write_from_file`, `remove`, `batch`) return a *new* `FS` pointing at the new commit -- the original `FS` is unchanged.

Snapshots obtained from **branches** are writable (`fs.writable == True`). Snapshots obtained from **tags** are read-only (`fs.writable == False`).

## API

### Opening a repository

```python
repo = GitStore.open("data.git")                         # create or open (default branch: "main")
repo = GitStore.open("data.git", create=False)            # open only
repo = GitStore.open("data.git", branch="dev")            # custom default branch
repo = GitStore.open("data.git", branch=None)             # branchless
repo = GitStore.open("data.git", author="alice",          # custom author
                     email="alice@example.com")
```

### Branches and tags

```python
fs = repo.branches["main"]
repo.branches["experiment"] = fs   # fork a branch
del repo.branches["experiment"]    # delete a branch

repo.tags["v1.0"] = fs            # create a tag
snapshot = repo.tags["v1.0"]       # read-only FS

repo.branches.current_name           # "main"
fs = repo.branches.current           # FS for the current branch
repo.branches.current = "dev"        # set current branch

for name in repo.branches:
    print(name)
"main" in repo.branches           # True
```

### Reading

```python
data = fs.read("path/to/file.bin")           # bytes
text = fs.read_text("config.json")           # str (UTF-8)
chunk = fs.read("big.bin", offset=100, size=50)  # partial read (50 bytes at offset 100)
chunk = fs.read_by_hash(sha, offset=0, size=1024)  # read blob by SHA, bypasses tree walk

entries = fs.ls()                             # root listing — list of name strings
entries = fs.ls("src")                        # subdirectory listing
details = fs.listdir("src")                  # list of WalkEntry (name, oid, mode)
exists = fs.exists("path/to/file.bin")        # bool
info = fs.stat("path/to/file.bin")           # StatResult (mode, file_type, size, hash, nlink, mtime)
ftype = fs.file_type("run.sh")               # FileType.EXECUTABLE
nbytes = fs.size("path/to/file.bin")         # int (bytes)
sha = fs.object_hash("path/to/file.bin")     # 40-char hex SHA
tree_sha = fs.tree_hash                      # root tree 40-char hex SHA

# Walk the tree (like os.walk)
for dirpath, dirnames, file_entries in fs.walk():
    for entry in file_entries:
        print(entry.name, entry.file_type)    # WalkEntry with name, oid, mode

# Glob
matches = fs.glob("**/*.py")                 # sorted list of matching paths

# Partial read (offset + size)
header = fs.read("data.bin", offset=0, size=4)
```

### Writing

Every write auto-commits and returns a new snapshot:

```python
from vost import FileType

fs = fs.write_text("config.json", '{"key": "value"}')
fs = fs.write_text("script.sh", "#!/bin/sh\n", mode=FileType.EXECUTABLE)
fs = fs.write_text("config.json", "{}", message="Reset")   # custom commit message
fs = fs.write("image.png", raw_bytes)                       # binary data
fs = fs.write_from_file("big.bin", "/data/big.bin")         # from disk
fs = fs.write_symlink("link", "target")                     # symlink
fs = fs.remove("old-file.txt")

# Buffered write (commits on close)
with fs.writer("big.bin") as f:
    f.write(chunk1)
    f.write(chunk2)
fs = f.fs

# Text mode
with fs.writer("log.txt", "w") as f:
    f.write("line 1\n")
    f.write("line 2\n")
fs = f.fs

# Inside a batch
with fs.batch() as b:
    with b.writer("streamed.bin") as f:
        for chunk in source:
            f.write(chunk)
```

The original `FS` is never mutated:

```python
fs1 = repo.branches["main"]
fs2 = fs1.write("new.txt", b"data")
assert not fs1.exists("new.txt")  # fs1 is unchanged
assert fs2.exists("new.txt")
```

### Batch writes

Multiple writes/removes in a single commit:

```python
with fs.batch(message="Import dataset v2") as b:
    b.write("a.txt", b"alpha")
    b.write_from_file("big.bin", "/data/big.bin")
    b.write_symlink("link.txt", "a.txt")
    b.remove("old.txt")
fs = b.fs  # new snapshot after the batch commits
```

If an exception occurs inside the batch, nothing is committed.

### History

```python
parent = fs.parent                               # FS or None
ancestor = fs.back(3)                            # 3 commits back

for snapshot in fs.log():                        # full commit log
    print(snapshot.commit_hash, snapshot.message)

for snapshot in fs.log("config.json"):           # file history
    print(snapshot.commit_hash, snapshot.message)

for snapshot in fs.log(match="deploy*"):         # message filter
    ...

for snapshot in fs.log(before=cutoff):           # date filter
    ...

fs = fs.undo()                                   # move branch back 1 commit
fs = fs.redo()                                   # move branch forward 1 reflog step

# Reflog — branch movement history
for entry in repo.branches.reflog("main"):
    print(entry.old_sha, entry.new_sha, entry.message)
```

### Copy and sync

```python
# Disk to repo (current branch)
fs = fs.copy_in(["./data/"], "backup")
print(fs.changes.add)                            # [FileEntry(...), ...]

# Repo to disk
fs.copy_out(["docs"], "./local-docs")

# Work with a non-default branch
dev = repo.branches["dev"]
dev = dev.copy_in(["./features/"], "src")

# Copy between branches (atomic, no disk I/O)
main = repo.branches["main"]
dev = dev.copy_from_ref(main, "config")               # dir mode: config/ → config/
dev = dev.copy_from_ref(main, "config/", "imported")  # contents mode: config/* → imported/
dev = dev.copy_from_ref(main, "config", "imported")   # dir mode: config/ → imported/config/

# Sync (make identical, including deletes)
fs = fs.sync_in("./local", "data")
fs.sync_out("data", "./local")

# Expand globs on disk (same dotfile rules as fs.glob)
from vost import disk_glob
files = disk_glob("./data/**/*.csv")

# Remove and move within repo
fs = fs.remove(["old-dir"], recursive=True)
fs = fs.move(["old.txt"], "new.txt")
```

### Atomic apply

Apply multiple writes and removes in a single commit without a context manager:

```python
from vost import WriteEntry

fs = fs.apply(
    writes={
        "config.json": b'{"v": 2}',
        "script.sh": WriteEntry(data=b"#!/bin/sh\n", mode=0o100755),
        "link": WriteEntry(target="config.json"),          # symlink
    },
    removes=["old.txt", "deprecated/"],
    message="Update config and clean up",
)
```

### Snapshot properties

```python
fs.commit_hash           # str -- full 40-character commit SHA
fs.ref_name              # str | None -- ref name (branch or tag), or None for detached
fs.message               # str -- commit message
fs.time                  # datetime -- commit timestamp (timezone-aware)
fs.author_name           # str -- commit author name
fs.author_email          # str -- commit author email
fs.changes               # ChangeReport | None -- changes from last operation
```

### Backup and restore

```python
diff = repo.backup("https://github.com/user/repo.git")    # MirrorDiff
diff = repo.restore("https://github.com/user/repo.git")   # MirrorDiff
diff = repo.backup(url, dry_run=True)                      # preview only
diff = repo.backup("/backups/store.git")                   # local path
diff = repo.backup("backup.bundle")                        # bundle file
diff = repo.backup(url, refs=["main", "v1.0"])             # specific refs only
```

### Bundle export and import

```python
repo.bundle_export("backup.bundle")                        # export all refs
repo.bundle_export("backup.bundle", refs=["main"])         # export specific refs
repo.bundle_import("backup.bundle")                        # import all refs (additive)
repo.bundle_import("backup.bundle", refs=["main"])         # import specific refs
```

## Concurrency safety

vost uses an advisory file lock (`vost.lock` in the repo directory) to make the stale-snapshot check and ref update atomic on a single machine. If a branch advances after you obtain a snapshot, attempting to write from the stale snapshot raises `StaleSnapshotError`:

```python
from vost import StaleSnapshotError

fs = repo.branches["main"]
_ = fs.write("a.txt", b"a")     # advances the branch

try:
    fs.write("b.txt", b"b")     # fs is now stale
except StaleSnapshotError:
    fs = repo.branches["main"]  # re-fetch and retry
```

For single-file writes, `retry_write` handles the re-fetch-and-retry loop automatically with exponential backoff:

```python
from vost import retry_write
fs = retry_write(repo, "main", "file.txt", data)
```

**Guarantees and limitations:**

- Single-machine, multi-process writes to the same branch are serialized by the file lock and will never silently lose commits.
- When a stale write is rejected, the commit object is created but unreferenced. These dangling objects are harmless and will be cleaned up by `git gc`.
- Cross-machine coordination (e.g. NFS-mounted repos) is not supported -- file locks are not reliable over network filesystems.

**Maintenance:** vost repos are standard bare Git repositories. Run `vost gc` (or `git gc` directly) to repack loose objects and prune unreferenced data. This is optional but can reduce disk usage for long-lived repos.

## Error handling

| Exception | When |
|-----------|------|
| `FileNotFoundError` | `read`/`remove` on a missing path; `write_from_file` with a missing local file; opening a missing repo with `create=False` |
| `IsADirectoryError` | `read` on a directory path; `write_from_file` with a directory; `remove` on a directory |
| `NotADirectoryError` | `ls`/`walk` on a file path |
| `PermissionError` | Writing to a tag snapshot |
| `KeyError` | Accessing a missing branch/tag; overwriting an existing tag |
| `ValueError` | Invalid path (`..`, empty segments); unsupported open mode |
| `TypeError` | Assigning a non-`FS` value to a branch or tag |
| `RuntimeError` | Writing/removing on a closed `Batch` |
| `StaleSnapshotError` | Writing from a snapshot whose branch has moved forward |

## CLI

vost includes a command-line interface. Install with `pip install "vost[cli]"` or `uv tool install "vost[cli]"`.

```bash
export VOST_REPO=/path/to/repo.git    # or pass --repo/-r per command
```

### Repo paths and the `:` prefix

Because vost commands work with both local files and files stored in the repo, you need a way to tell them apart. **A leading `:` marks a repo path.** Without it, the argument is a local filesystem path.

```
:file.txt              repo path on the current branch
:                      repo root
main:file.txt          repo path on the "main" branch
v1.0:data/             repo path on the "v1.0" tag
main~3:file.txt        3 commits back on main
```

This applies to `cp`, `sync`, `rm`, `mv`, `ls`, `cat`, and other commands. For `ls`, `cat`, `rm`, and `write` the `:` is optional (arguments are always repo paths), but it is **required** for `cp`, `sync`, and `mv` to distinguish repo paths from local paths.

For full details on path parsing, ancestor syntax (`~N`), and interaction with flags, see [Path Syntax](https://mhalle.github.io/vost/paths/).

```bash
# Repository management
vost init
vost destroy -f
vost gc

# Copy files (disk <-> repo, repo <-> repo)
vost cp local-file.txt :                        # disk to repo root
vost cp ./mydir :dest                            # copy mydir into dest/mydir
vost cp ./mydir/ :dest                           # trailing / = contents only
vost cp '/data/./logs/app' :backup               # /./  pivot: → backup/logs/app/...
vost cp './src/*.py' :backup                     # glob
vost cp :file.txt ./local.txt                    # repo to disk
vost cp -n ./mydir :dest                         # dry run

# Sync (make identical, including deletes)
vost sync ./local :repo_path
vost sync :repo_path ./local
vost sync --watch ./dir :data                    # continuous watch mode

# Browse
vost ls
vost ls -R :src
vost cat file.txt

# Write stdin
echo "hello" | vost write file.txt
cmd | vost write log.txt -p | grep error         # passthrough (tee)

# Remove and move within repo
vost rm old-file.txt
vost rm -R :dir
vost mv :old.txt :new.txt
vost mv ':*.txt' :archive/

# History
vost log
vost log --path file.txt --format jsonl
vost diff --back 3
vost undo
vost redo

# Branches and tags
vost branch set dev --ref main
vost branch exists dev
vost tag set v1.0
vost tag delete v1.0

# Archives
vost archive_out out.zip
vost archive_in data.tar.gz

# Mirror (backup/restore all refs)
vost backup https://github.com/user/repo.git
vost restore https://github.com/user/repo.git
vost backup -n https://github.com/user/repo.git  # dry run

# Serve files over HTTP
vost serve                                        # single branch
vost serve --all --cors                           # all refs with CORS

# Serve repo over Git protocol (read-only)
vost gitserve
```

For full CLI documentation, see [CLI Reference](https://mhalle.github.io/vost/cli/).

## Git notes

Attach metadata to commits without modifying history. Notes can be
addressed by commit hash or ref name (branch/tag):

```python
# Default namespace (refs/notes/commits)
ns = repo.notes.commits

# By commit hash
ns[fs.commit_hash] = "reviewed by Alice"
print(ns[fs.commit_hash])                       # "reviewed by Alice"

# By branch or tag name (resolves to tip commit)
ns["main"] = "deployed to staging"
print(ns["main"])                                # "deployed to staging"

del ns[fs.commit_hash]

# Custom namespaces
reviews = repo.notes["reviews"]
reviews["main"] = "LGTM"

# Shortcut: note for the current HEAD commit
ns.for_current_branch = "deployed to staging"
print(ns.for_current_branch)

# Batch writes (single commit)
with repo.notes.commits.batch() as b:
    b["main"] = "note for main"
    b["dev"] = "note for dev"

# Iteration (yields commit hashes)
for commit_hash, text in ns.items():
    print(commit_hash, text)
```

## Documentation

- [Documentation](https://mhalle.github.io/vost/) -- quick start and navigation
- [Python API Reference](https://mhalle.github.io/vost/api/) -- classes, methods, and data types
- [CLI Reference](https://mhalle.github.io/vost/cli/) -- the `vost` command-line tool
- [CLI Tutorial](https://mhalle.github.io/vost/cli-tutorial/) -- learn the CLI step by step
- [Path Syntax](https://mhalle.github.io/vost/paths/) -- how `ref:path` works across commands
- [fsspec Integration](https://mhalle.github.io/vost/fsspec/) -- use vost with pandas, xarray, dask
- [GitHub Repository](https://github.com/mhalle/vost) -- source code, issues, and releases

## Language ports

vost is available in five languages. Each port provides the same core API:

| Language | Directory | Backend | Package |
|----------|-----------|---------|---------|
| **Python** | (this directory) | [dulwich](https://www.dulwich.io/) | `pip install vost` |
| **TypeScript** | [`ts/`](ts/) | [isomorphic-git](https://isomorphic-git.org/) | `npm install @mhalle/vost` |
| **Rust** | [`rs/`](rs/) | [libgit2](https://libgit2.org/) (via [git2](https://crates.io/crates/git2)) | `cargo add vost` |
| **Kotlin** | [`kotlin/`](kotlin/) | [JGit](https://www.eclipse.org/jgit/) | source dependency |
| **C++** | [`cpp/`](cpp/) | [libgit2](https://libgit2.org/) | CMake / vcpkg |

## Development

```bash
uv sync --dev       # install with dev dependencies (includes CLI)
uv run python -m pytest -v
```
