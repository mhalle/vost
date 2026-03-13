# vost CLI Tutorial

## What is vost?

vost is a versioned object store that uses Git as its storage backend. It lets you store, retrieve, and version files using commands that feel like familiar tools -- `cp`, `ls`, `mv`, `cat`, `rsync`, `zip`.

Compared to git, vost is much simpler. There's no staging area, no merge conflicts, no working directory to manage. Each write operation -- copying files in, removing files, renaming -- is a single atomic commit that either succeeds completely or fails without changing anything.

Under the hood, vost repositories are just bare Git repositories. This means vost inherits Git's strengths: content deduplication, compression, and full compatibility with Git CLI and GUI tools.

---

## Getting started

### Install

Install the CLI permanently:

```bash
pip install "vost[cli]"
```

Or with uv:

```bash
uv tool install "vost[cli]"       # install as a CLI tool
```

Add to a project:

```bash
uv add "vost[cli]"
```

Or run without installing:

```bash
uvx "vost[cli]" --help
```

### Set the VOST_REPO environment variable

Every vost command needs to know which repository to use. Set the environment variable once per shell session:

```bash
export VOST_REPO=~/data.git
```

Or pass `-r ~/data.git` on every call. All examples below assume `VOST_REPO` is set.

Most commands auto-create the repository on first write, so explicit `init` is rarely needed:

```bash
vost init -r ~/data.git       # optional -- most commands auto-create
```

### First example

```bash
echo "Hello, world!" > /tmp/hello.txt
vost cp /tmp/hello.txt :
vost cat hello.txt
```

The `:` at the end means "repo root on the current branch" (a more detailed description of `:` syntax can be found [below](#the--prefix)). The file is now stored in the repo.

Output: `Hello, world!`

---

## Writing and reading files

### Copy files from disk into the repo

```bash
echo "Hello, world!" > /tmp/hello.txt
echo '{"name": "tutorial"}' > /tmp/config.json

vost cp /tmp/hello.txt /tmp/config.json :
```

Both files are now stored in the repo.

### Read files back

```bash
vost cat hello.txt
```

Output: `Hello, world!`

### Write from stdin

```bash
echo "Log entry: $(date)" | vost write log.txt
```

This reads stdin and stores it as `log.txt` in the repo.

### Passthrough mode

The `-p` flag echoes stdin to stdout while also writing to the repo -- useful in pipelines:

```bash
echo "important data" | vost write capture.txt -p | wc -c
```

---

## The `:` prefix

Commands like `cp` and `sync` work with both local files and files inside the repo. A leading `:` marks a repo path. Without it, the argument is a local filesystem path.

| Syntax | Meaning |
|--------|---------|
| `file.txt` | Local file on disk |
| `:file.txt` | Repo file on the current branch |
| `:` | Repo root on the current branch |
| `main:file.txt` | Repo file on the `main` branch |
| `main:` | Root of the `main` branch |
| `v1.0:data/file` | Repo file on the `v1.0` tag (read-only) |
| `main~3:file.txt` | 3 commits back on `main` |
| `~2:file.txt` | 2 commits back on the current branch |

### When is `:` required?

- **`cp`, `sync`, `mv`** -- the `:` is how the command distinguishes repo paths from local paths. It is required.
- **`ls`, `cat`, `rm`, `write`** -- arguments are always repo paths, so `:` is optional. `vost cat file.txt` and `vost cat :file.txt` are equivalent. However, `:` is needed for explicit ref syntax (`main:file.txt`).
- **`hash`, `log`, `diff`** -- a bare string (no `:`) is treated as a **ref** (branch, tag, or commit hash), not a path. Use `:path` for repo paths: `vost hash main` = commit hash, `vost hash :file.txt` = blob hash.

---

## Commit messages

Write commands (`cp`, `sync`, `rm`, `mv`, `write`) accept `-m` for a custom commit message:

```bash
echo "data" | vost write data.txt -m "Import raw data"
vost cp ./src :code -m "Deploy v2.1"
vost rm :old.txt -m "Clean up deprecated files"
```

### Message placeholders

Use `{default}` to include the auto-generated message:

```bash
vost cp ./src :code -m "Deploy: {default}"
```

Other placeholders:

| Placeholder | Expands to |
|-------------|------------|
| `{default}` | Full auto-generated message |
| `{add_count}` | Number of additions |
| `{update_count}` | Number of updates |
| `{delete_count}` | Number of deletions |
| `{total_count}` | Total changed files |

```bash
vost sync ./data :data -m "Sync {total_count} files"
```

---

## Listing and exploring

### Basic listing

```bash
vost ls
```

Output:

```
capture.txt
config.json
hello.txt
log.txt
```

### Recursive listing

```bash
vost ls -R
```

Lists all files, including those in subdirectories, with full paths.

### Long format

```bash
vost ls -l
```

Shows file sizes, types, and abbreviated hashes:

```
blob     14 a5c1903 hello.txt
blob     24 e4b7c2f config.json
```

Use `--full-hash` to see the full 40-character object hash.

### Glob patterns

Glob patterns work for both repo paths and disk paths. Always quote them to prevent shell expansion:

```bash
vost ls '*.txt'          # all .txt files at root
vost ls '**/*.txt'       # all .txt files at any depth
vost ls 'config.*'       # config.json, config.yaml, etc.
```

Standard patterns: `*` matches any characters within a name, `?` matches a single character, `**` matches across directories.

Globs do not match dotfiles (files starting with `.`) -- this is intentional.

Glob patterns work the same way in `cp`, `rm`, and `mv`. Either kind of wildcarding can be used with local files as well as repo paths:

```bash
vost rm ':*.log'                    # remove all .log files at root
vost cp ':docs/**' /tmp/out         # copy repo files to disk
vost cp '/tmp/data/**/*.csv' :      # copy all CSVs at any depth from disk
```

---

## Organizing with directories

### Copy a directory from disk

```bash
mkdir -p /tmp/docs
echo "# Guide" > /tmp/docs/guide.md
echo "# FAQ" > /tmp/docs/faq.md

vost cp /tmp/docs :
```

This creates `docs/guide.md` and `docs/faq.md` in the repo (the directory name is preserved).

### Copy directory contents (trailing slash)

```bash
vost cp /tmp/docs/ :reference
```

With a trailing `/`, the *contents* of `docs/` are placed directly into `reference/` -- so you get `reference/guide.md` and `reference/faq.md` (not `reference/docs/...`).

This follows rsync conventions: source name included by default, trailing `/` means contents only.

Note: Git does not support empty directories, so empty subdirectories on disk are silently skipped during copy and sync operations.

### List a subdirectory

```bash
vost ls docs
vost ls -R docs
```

### Delete extra files on copy

By default, `cp` only adds and updates files. The `--delete` flag also removes destination files not present in the source, like rsync's `--delete`:

```bash
vost cp --delete ./current/ :data
```

This makes `:data` an exact mirror of `./current/`. If you want this delete-on-copy behavior by default, use `sync` instead (next section).

---

## Syncing directories

`sync` is a shortcut for `cp --delete` -- it makes a destination identical to a source, including deleting files that don't exist in the source (like `rsync --delete`).

### Disk to repo

```bash
mkdir -p /tmp/project/src
echo "main()" > /tmp/project/src/app.py
echo "test()" > /tmp/project/src/test.py

vost sync /tmp/project/src :code
```

### Repo to disk

```bash
vost sync :code /tmp/output
ls /tmp/output/    # app.py  test.py
```

### Exclude patterns

```bash
vost sync /tmp/project :code --exclude '*.pyc' --exclude '__pycache__/'
```

Or read patterns from a file:

```bash
vost sync /tmp/project :code --exclude-from .gitignore
```

The `--gitignore` flag reads `.gitignore` files from the source tree automatically:

```bash
vost sync --gitignore /tmp/project :code
```

### Watch mode

Continuously watch a directory and sync changes:

```bash
vost sync --watch /tmp/project/src :code
```

Every time a file changes on disk, it's synced to the repo. Use `--debounce` to control the delay (default: 2000ms):

```bash
vost sync --watch --debounce 5000 /tmp/project/src :code
```

Press Ctrl+C to stop.

---

## Preserving directory structure with `/./`

When copying files from a deep directory tree, you often want to preserve part of the source path at the destination. An embedded `/./` in a source path (like rsync's `-R` flag) splits the path into two parts: everything before `/./` locates the source, and everything after becomes the destination-relative path.

```bash
vost cp /var/log/./app/errors.log :logs
# result: logs/app/errors.log
```

Without the pivot, only the filename would be preserved:

```bash
vost cp /var/log/app/errors.log :logs
# result: logs/errors.log
```

This works with directories too. Trailing `/` (contents mode) combines naturally with the pivot:

```bash
vost cp /data/./archive/2025 :backup
# result: backup/archive/2025/...   (directory name preserved)

vost cp /data/./archive/2025/ :backup
# result: backup/2025/...           (contents mode, archive/ dropped)
```

The pivot works in both directions -- disk to repo and repo to disk:

```bash
vost cp ':src/./lib/utils.py' ./dest
# result: dest/lib/utils.py
```

Note: a leading `./` (e.g. `./mydir`) is a normal relative path and does not trigger pivot mode.

---

## Previewing changes

Any write command supports `--dry-run` (or `-n`) to preview what would change without actually writing:

```bash
vost cp --dry-run ./bigdir :dest
vost sync --dry-run ./src :code
vost rm --dry-run ':*.log'
vost mv --dry-run :archive :backup
```

The output shows what would change:

```
+ :path/to/new-file
~ :path/to/changed-file
- :path/to/removed-file
```

---

## History and time travel

### Build some history

```bash
echo "v1" | vost write data.txt -m "First version"
echo "v2" | vost write data.txt -m "Second version"
echo "v3" | vost write data.txt -m "Third version"
```

### View the log

```bash
vost log
```

Output:

```
a1b2c3d  2026-02-27T10:03:00-05:00  Third version
e4f5678  2026-02-27T10:02:00-05:00  Second version
9a0b1c2  2026-02-27T10:01:00-05:00  First version
...
```

### Log with bare-ref syntax

You can pass a branch or tag name directly:

```bash
vost log main                    # log of main branch
vost log main:data.txt           # log filtered to data.txt on main
vost log ~3:                     # log starting 3 commits back
```

### Filter the log

```bash
vost log --path data.txt              # only commits that changed data.txt
vost log --match "First*"             # commits matching a message pattern
vost log --before 2026-02-27T10:02    # commits on or before a date
```

### Read old versions

```bash
vost cat data.txt --back 1     # one commit ago: "v2"
vost cat data.txt --back 2     # two commits ago: "v1"
```

The `--back N` flag walks back N commits from the branch tip.

### Ancestor syntax in paths

Instead of `--back`, you can embed the ancestor in the path:

```bash
vost cat main~1:data.txt       # same as --back 1
vost cat main~2:data.txt       # same as --back 2
```

### Undo and redo

```bash
vost undo                      # moves branch back 1 commit
vost cat data.txt              # now shows "v2"
vost redo                      # moves branch forward again
vost cat data.txt              # back to "v3"
```

Undo multiple steps:

```bash
vost undo 2                    # back 2 commits
vost redo 2                    # forward 2 steps
```

### Reflog

The reflog shows the full timeline of branch pointer movements, including undos:

```bash
vost reflog
vost reflog -n 5               # last 5 entries
```

---

## Hashes

The `hash` command prints the SHA hash of a commit, tree, or blob.

```bash
vost hash                        # current branch commit hash
vost hash main                   # main branch commit hash
vost hash v1.0                   # tag commit hash
vost hash :file.txt              # blob hash on current branch
vost hash main:src/              # tree hash of a directory
vost hash ~3:                    # commit hash 3 back
```

A bare string (no `:`) is treated as a ref name (branch, tag, or hash). Use `:path` for object hashes.

---

## Comparing and diffing

### Diff against history

```bash
vost diff --back 3             # what changed in the last 3 commits
```

Output uses git-style status letters:

```
A  feature.txt
M  data.txt
A  docs/guide.md
```

`A` = added, `M` = modified, `D` = deleted.

### Diff with bare-ref syntax

You can pass a branch or ancestor directly:

```bash
vost diff dev                    # diff current branch against dev
vost diff ~3                     # what changed in last 3 commits
vost diff main~2:                # diff against main, 2 commits back
```

### Compare individual files

```bash
vost cmp :data.txt main~2:data.txt
```

Exit code 0 means identical, 1 means different. Add `-v` (before the subcommand) to see the hashes:

```bash
vost -v cmp :data.txt main~2:data.txt
```

### Mix repo and disk files

```bash
echo "v3" > /tmp/local.txt
vost cmp :data.txt /tmp/local.txt
```

---

## Branches

A branch is a named, ongoing series of commits describing historical changes to a directory tree and its files. Branches are mutable -- new commits can be appended to update their contents.

On creation, vost repos have an empty `main` branch as the current (default) branch.

### List branches

```bash
vost branch list
```

Output: `main`

### Create a branch

```bash
vost branch set dev
```

This forks `dev` from the current branch (`main`). Both branches now have the same files.

To create an empty branch with no history:

```bash
vost branch set scratch --empty
```

### Show and set the current branch

```bash
vost branch current            # prints: main
vost branch current -b dev     # switch to dev
vost branch current            # prints: dev
```

### Work on a branch

```bash
echo "dev feature" | vost write -b dev feature.txt
vost ls -b dev
vost ls -b main       # main doesn't have feature.txt
```

### Copy between branches

```bash
vost cp dev:feature.txt :
```

This copies `feature.txt` from `dev` to the current branch.

### Switch back

```bash
vost branch current -b main
```

---

## Tags and snapshots

Tags are read-only labels for the state of a directory tree at a specific point in time, like a snapshot. Unlike branches, tags cannot be modified -- they permanently point to a single commit.

### Tag the current state

```bash
vost tag set v1
```

This creates a lightweight tag pointing at the current branch's HEAD commit.

### List and inspect tags

```bash
vost tag list
vost tag hash v1               # prints the commit SHA
```

### Read from a tag

```bash
vost cat v1:data.txt
vost ls v1:
vost cp v1:data.txt /tmp/old-data.txt
```

Tags are read-only -- you cannot write to them.

### Tag a historical commit

```bash
vost tag set v0 --back 5       # tag the commit 5 back from tip
vost tag set release --before 2026-01-01
```

### Delete a tag

```bash
vost tag delete v0
```

### Tag on write

Write commands can tag the resulting commit in one step:

```bash
echo "release data" | vost write release.txt --tag v2
```

---

## Moving and removing

### Rename a file

```bash
vost mv :hello.txt :greeting.txt
```

### Bulk move with globs

```bash
vost mv ':*.txt' :archive/
```

Moves all `.txt` files at the root into `archive/`.

### Move a directory

```bash
vost mv -R :docs :documentation
```

### Remove files

```bash
vost rm capture.txt
vost rm -R :reference          # remove a directory
```

---

## Archives

### Export to a zip file

```bash
vost zip /tmp/backup.zip
```

### Import from a zip file

```bash
vost unzip /tmp/backup.zip -b restored
```

### Export to tar (with compression)

```bash
vost tar /tmp/backup.tar.gz
```

Compression is auto-detected from the extension (`.tar.gz`, `.tar.bz2`, `.tar.xz`).

### Pipe to stdout

```bash
vost tar - | gzip > /tmp/piped.tar.gz
```

Use `-` as the filename to write to stdout.

### Import from stdin

```bash
cat /tmp/piped.tar.gz | gunzip | vost untar
vost untar --format tar < archive.tar
```

### Generic archive commands

`archive_out` and `archive_in` auto-detect format from the file extension:

```bash
vost archive_out /tmp/data.tar.bz2
vost archive_in /tmp/data.tar.bz2
```

---

## Git notes

Notes attach metadata to commits without modifying history.

### Set a note

```bash
vost note set main "Deployed to production"
```

This attaches a note to the tip commit of `main`. You can also use a commit hash:

```bash
vost note set abc1234... "Reviewed by Alice"
```

### Read a note

```bash
vost note get main
```

Output: `Deployed to production`

### Note on the current branch

When no target is given, `get`, `set`, and `delete` default to the current branch:

```bash
vost note set "Latest build passed"
vost note get                          # prints: Latest build passed
```

You can also use `:` or `main:` to be explicit:

```bash
vost note get :                        # current branch
vost note set main: "Deployed"         # main branch
```

### Custom namespaces

Notes are organized into namespaces (default: `commits`):

```bash
vost note set main "LGTM" -N reviews
vost note get main -N reviews
vost note list -N reviews
```

### List and delete

```bash
vost note list                  # list commit hashes with notes
vost note delete main           # remove the note
```

---

## Backup and restore

### Mirror to a local path

```bash
vost backup /tmp/mirror.git
```

This pushes all branches, tags, and objects to the target. It's a full mirror -- remote-only refs are deleted to match.

### Preview with dry run

```bash
vost backup --dry-run /tmp/mirror.git
```

Shows what would be added, updated, or deleted without transferring data.

### Restore from a mirror

```bash
vost restore /tmp/mirror.git
```

Adds and updates refs from the source. Restore is **additive** -- local-only refs are kept. Use `--dry-run` to preview.

### Remote URLs

Backup and restore work with any Git-compatible URL:

```bash
vost backup https://github.com/user/data-backup.git
vost restore git@github.com:user/data-backup.git
```

### Bundle files

Export all refs as a single portable file:

```bash
vost backup /tmp/backup.bundle
```

The `.bundle` extension triggers bundle format automatically. Import with:

```bash
vost restore /tmp/backup.bundle
```

### Scoping to specific refs

Use `--ref` (repeatable) to back up or restore only certain branches or tags:

```bash
vost backup backup.bundle --ref main --ref v1.0
vost restore backup.bundle --ref main
```

This works with URLs too:

```bash
vost backup /tmp/mirror.git --ref main
vost restore /tmp/mirror.git --ref v1
```

---

## Serving files

### HTTP file server

```bash
vost serve
```

Opens an HTTP server at `http://127.0.0.1:8000/` serving the current branch. Browse to see directory listings and download files.

### Serve a specific branch or tag

```bash
vost serve -b dev
vost serve --ref v1
```

### Multi-ref mode

```bash
vost serve --all
```

Exposes all branches and tags. URLs become `/<ref>/<path>`:

```
http://127.0.0.1:8000/main/data.txt
http://127.0.0.1:8000/v1/data.txt
```

### Options

```bash
vost serve --cors                    # enable CORS headers
vost serve --open                    # open browser automatically
vost serve --no-cache                # disable caching
vost serve -p 9000                   # custom port
vost serve --base-path /data         # mount under /data prefix
vost serve -q                        # suppress request logs
```

### Git server

Serve the repository for cloning by standard Git clients:

```bash
vost gitserve
```

From another terminal:

```bash
git clone http://127.0.0.1:8000/
```

The server is read-only -- pushes are rejected.

---

## FUSE mounting

Mount a branch as a read-only filesystem. Requires FUSE support:

```bash
pip install "vost[fuse]"
```

### Mount

```bash
mkdir -p /tmp/mnt
vost mount /tmp/mnt
```

Now browse with normal tools:

```bash
ls /tmp/mnt/
cat /tmp/mnt/data.txt
```

### Mount options

```bash
vost mount /tmp/mnt -b dev           # mount a specific branch
vost mount /tmp/mnt --ref v1         # mount a tag
vost mount /tmp/mnt --back 3         # mount 3 commits before tip
vost mount /tmp/mnt -f               # run in foreground
```

### Unmount

```bash
umount /tmp/mnt                      # macOS/Linux
```

---

## JSON output for scripting

By default, vost acts like a standard UNIX CLI, with output suitable for human viewing. Commands like `ls`, `log`, and `reflog` support `--format json` or `--format jsonl` for structured output suitable for scripting:

```bash
vost ls -l --format json | jq '.[].name'
vost log --format jsonl | jq -r '.hash'
vost reflog --format json
```

Use `json` for a single JSON array, `jsonl` for one JSON object per line (useful for streaming or line-by-line processing with `jq`).

---

## Tips and patterns

### Checksum mode for exact comparisons

By default, `cp` and `sync` compare files by modification time for speed. Use `-c` for byte-exact comparison:

```bash
vost sync -c ./data :data
```

### Verbose mode

Add `-v` before any command for status messages on stderr:

```bash
vost -v cp ./data :backup
vost -v sync ./src :code
```

---

## Clean up

When you're done with a repository:

```bash
vost gc                # repack objects and prune unreachable data
vost destroy -f        # remove the repository entirely
```

`gc` requires `git` to be installed. `destroy` requires `-f` if the repo contains any branches or tags.

---

## Next steps

- [CLI Reference](cli.md) -- complete command reference with all options
- [Path Syntax](paths.md) -- detailed path parsing rules and edge cases
- [Python API Reference](api.md) -- use vost as a Python library
- [README](https://github.com/mhalle/vost#readme) -- core concepts and library quick start
