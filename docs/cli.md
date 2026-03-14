# CLI Reference

## Setup

Set the repository once per shell session:

```bash
export VOST_REPO=/path/to/repo.git
```

Or pass `--repo`/`-r` per command. Use `--branch`/`-b` to target a branch (default: repo's current branch).

Pass `-v` before any command for status messages on stderr.

### Command index

| Category | Commands |
|----------|----------|
| [Repository Management](#repository-management) | [init](#init), [destroy](#destroy), [gc](#gc) |
| [Everyday Commands](#everyday-commands) | [cp](#cp), [sync](#sync), [ls](#ls), [cat](#cat), [hash](#hash), [rm](#rm), [mv](#mv), [write](#write) |
| [History](#history) | [log](#log), [diff](#diff), [cmp](#cmp), [undo](#undo), [redo](#redo), [reflog](#reflog) |
| [Refs](#refs) | [branch](#branch), [tag](#tag), [note](#note) |
| [Archives](#archives) | [archive_out / archive_in](#archive_out--archive_in), [zip / unzip / tar / untar](#zip--unzip--tar--untar) |
| [Mirror](#mirror) | [backup](#backup), [restore](#restore) |
| [Server](#server) | [serve](#serve), [gitserve](#gitserve), [mount](#mount) |

---

## Repo paths and the `:` prefix

Commands like `cp` and `sync` work with both local files and files inside the repo. **A leading `:` marks a repo path.** Without it, the argument is treated as a local filesystem path.

| Syntax | Meaning |
|--------|---------|
| `file.txt` | Local file on disk |
| `:file.txt` | Repo file on the current branch |
| `:` | Repo root on the current branch |
| `:data/` | Repo directory (trailing `/` = contents mode in `cp`) |
| `main:file.txt` | Repo file on the `main` branch |
| `main:` | Repo root on `main` |
| `v1.0:data/file` | Repo file on the `v1.0` tag (read-only) |
| `main~3:file.txt` | 3 commits back on `main` |
| `~2:file.txt` | 2 commits back on the current branch |

### When is `:` required?

- **`cp`, `sync`, `mv`** -- the `:` is how the command knows which arguments are repo paths and which are local paths. It is required.
- **`ls`, `cat`, `rm`, `write`** -- arguments are always repo paths, so the `:` is optional. `vost cat file.txt` and `vost cat :file.txt` are equivalent. However, the `:` is required to use explicit ref syntax (`main:file.txt`).
- **`hash`, `log`, `diff`** -- a bare string (no `:`) is treated as a **ref** (branch, tag, or commit hash), not a path. Use `:path` for repo paths: `vost hash main` = commit hash, `vost hash :file.txt` = blob hash.

### Direction detection in `cp`

The `:` prefix on each argument determines the copy direction:

```bash
vost cp file.txt :             # local  -> repo  (disk to repo)
vost cp :file.txt ./out        # :repo  -> local (repo to disk)
vost cp :a.txt :backup/        # :repo  -> :repo (repo to repo)
vost cp main:a.txt dev:backup/ # cross-branch repo to repo
```

### Writing to refs

Only branches are writable. Tags, commit hashes, and historical commits (`~N`) are read-only:

```bash
vost cp file.txt dev:          # OK -- writes to dev branch
vost cp file.txt v1.0:         # ERROR -- cannot write to a tag
vost cp file.txt main~1:       # ERROR -- cannot write to history
```

For the full path syntax specification, see [Path Syntax](paths.md).

---

## Repository Management

### init

Create a new bare git repository.

```bash
vost init                    # or --repo <path>
vost init --branch dev
vost init -f                 # destroy and recreate
```

| Option | Description |
|--------|-------------|
| `--branch`, `-b` | Initial branch (default: `main`). |
| `-f`, `--force` | Destroy existing repo and recreate. |

### destroy

Remove a bare git repository.

```bash
vost destroy                 # fails if repo has data
vost destroy -f              # force
```

### gc

Run garbage collection on the repository. Removes unreachable objects (orphaned blobs, etc.) and repacks the object store. Requires `git` to be installed.

```bash
vost gc
```

---

## Everyday Commands

### cp

Copy files and directories between disk and repo. The last argument is the destination; all preceding arguments are sources. Prefix repo-side paths with `:`.

```bash
# Disk to repo
vost cp file.txt :                        # keep name at repo root
vost cp file.txt :dest/file.txt           # explicit dest
vost cp f1.txt f2.txt :dir                # multiple files
vost cp ./mydir :dest                     # directory (name preserved)
vost cp ./mydir/ :dest                    # contents mode (trailing /)
vost cp './src/*.py' :backup              # glob

# Pivot (/./): preserve partial source path
vost cp /data/./logs/app :backup          # → backup/logs/app/...
vost cp /data/./logs/app/ :backup         # → backup/logs/...
vost cp /home/user/./projects/f.txt :bak  # → bak/projects/f.txt

# Repo to disk
vost cp :file.txt ./local.txt
vost cp ':docs/*.md' ./local-docs

# Pivot (/./): preserve partial repo path
vost cp ':data/./logs/app' ./backup          # → backup/logs/app/...
vost cp ':data/./logs/app/' ./backup         # → backup/logs/...
vost cp ':src/./lib/utils.py' ./dest         # → dest/lib/utils.py

# Options
vost cp -n ./mydir :dest                  # dry run
vost cp --delete ./src/ :code             # remove extra repo files
vost cp --ref v1.0 :data ./local          # from tag/branch/hash
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref` | Branch, tag, or commit hash to read from. |
| `--path` | Latest commit that changed this path. |
| `--match` | Latest commit matching message pattern (`*`, `?`). |
| `--before` | Latest commit on or before this date (ISO 8601). |
| `--back N` | Walk back N commits from tip. |
| `-m`, `--message` | Commit message (supports [placeholders](#message-placeholders)). |
| `--type` | `blob` or `executable` (default: auto-detect from disk permissions). |
| `--follow-symlinks` | Dereference symlinks (disk→repo only). |
| `-n`, `--dry-run` | Preview without writing. |
| `--ignore-existing` | Skip existing destination files. |
| `--delete` | Delete dest files not in source (rsync `--delete`). |
| `--no-glob` | Treat source paths as literal (no `*` or `?` expansion). |
| `--exclude PATTERN` | Exclude files matching pattern (gitignore syntax, repeatable; disk→repo only; see [Exclude patterns](#exclude-patterns)). |
| `--exclude-from FILE` | Read exclude patterns from file (disk→repo only; see [Exclude patterns](#exclude-patterns)). |
| `--ignore-errors` | Skip failed files and continue. |
| `-c`, `--checksum` | Compare files by checksum instead of mtime (slower, exact). |
| `--parent REF` | Additional parent ref (branch/tag/hash). Repeatable. Advisory only — no tree merging (disk→repo only). |
| `--no-create` | Don't auto-create the repo. |
| `--tag` | Create a tag at the resulting commit (disk→repo only). |
| `--force-tag` | Overwrite tag if it already exists. |

#### Copy behavior

| Source | Result at `:dest` |
|--------|-------------------|
| `file.txt` | `dest/file.txt` |
| `dir` | `dest/dir/...` (name preserved) |
| `dir/` | `dest/...` (contents poured, including dotfiles) |
| `'dir/*'` | `dest/a.txt ...` (glob, no dotfiles) |
| `'**/*.py'` | `dest/matched...` (recursive glob) |
| `/base/./sub/dir` | `dest/sub/dir/...` (pivot) |
| `/base/./sub/dir/` | `dest/sub/...` (pivot + contents) |

#### /./  pivot

An embedded `/./` in a source path (rsync `-R` style) splits the path into a locator and a preserved suffix. Everything before `/./` locates the source (on disk or in the repo); everything after becomes the destination-relative path. Works in both disk→repo and repo→disk directions.

A leading `./` (e.g. `./mydir`) is a normal relative path and does **not** trigger pivot mode.

Glob patterns (`*`, `?`, `**`) in the segment after `/./` are not supported — use them before the pivot or as separate sources.

### sync

Make one path identical to another (like rsync `--delete`).

```bash
vost sync ./dir                           # sync dir to repo root
vost sync ./local :repo_path              # disk → repo
vost sync :repo_path ./local              # repo → disk
vost sync -n ./local :repo_path           # dry run
vost sync :data ./local --ref v1.0        # from tag
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref` | Branch, tag, or commit hash. |
| `--path` | Latest commit that changed this path. |
| `--match` | Latest commit matching message pattern. |
| `--before` | Latest commit on or before this date. |
| `--back N` | Walk back N commits from tip. |
| `-m`, `--message` | Commit message (supports [placeholders](#message-placeholders)). |
| `-n`, `--dry-run` | Preview without writing. |
| `--exclude PATTERN` | Exclude files matching pattern (gitignore syntax, repeatable; disk→repo only; see [Exclude patterns](#exclude-patterns)). |
| `--exclude-from FILE` | Read exclude patterns from file (disk→repo only; see [Exclude patterns](#exclude-patterns)). |
| `--gitignore` | Read `.gitignore` files from source tree (disk→repo only). |
| `--ignore-errors` | Skip failed files. |
| `-c`, `--checksum` | Compare files by checksum instead of mtime (slower, exact). |
| `--parent REF` | Additional parent ref (branch/tag/hash). Repeatable. Advisory only — no tree merging (disk→repo only). |
| `--no-create` | Don't auto-create the repo. |
| `--tag` | Create a tag at the resulting commit (disk→repo only). |
| `--force-tag` | Overwrite tag if it already exists. |
| `--watch` | Watch for changes and sync continuously (disk→repo only). |
| `--debounce MS` | Debounce delay in ms for `--watch` (default: 2000). |

#### Watch mode

With `--watch`, continuously watches the local directory and syncs on changes:

```bash
vost sync --watch ./dir :data
vost sync --watch --debounce 5000 ./dir
vost sync --watch -c ./src :code       # checksum mode
```

Requires: `pip install vost[cli]`

### ls

List files and directories. Accepts multiple paths and glob patterns — results are coalesced and deduplicated.

```bash
vost ls                                   # root
vost ls subdir                            # subdirectory
vost ls --ref v1.0                        # at a tag
vost ls '*.txt'                           # glob (quote to avoid shell expansion)
vost ls 'src/*.py'                        # glob in subdirectory
vost ls '**/*.py'                         # ** matches all depths
vost ls 'src/**/*.txt'                    # ** under a prefix
vost ls '*.txt' '*.py'                    # multiple globs
vost ls :src :docs                        # multiple directories
vost ls -R                                # all files recursively
vost ls -R :src :docs                     # recursive under multiple dirs
vost ls -R 'src/*'                        # glob + recursive expansion
```

| Option | Description |
|--------|-------------|
| `-R`, `--recursive` | List all files recursively with full paths. |
| `-l`, `--long` | Show file sizes, types, and hashes. |
| `--full-hash` | Show full 40-character hashes (default: 7-char; requires `-l`). |
| `--format` | Output format: `text` (default), `json`, `jsonl`. |
| `--no-glob` | Disable glob expansion — treat `*` and `?` as literal characters. |
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

### cat

Print file contents to stdout. Accepts multiple paths — output is concatenated.

```bash
vost cat file.txt
vost cat file.txt --ref v1.0
vost cat :a.txt :b.txt                    # concatenate multiple files
vost cat main:f.txt dev:f.txt             # files from different refs
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

#### Diffing with process substitution

Use shell process substitution (`<(...)`) to diff any two snapshots with standard `diff`:

```bash
diff <(vost cat :file.py) <(vost cat ~1:file.py)        # vs previous commit
diff <(vost cat :file.py) <(vost cat tag-name:file.py)   # vs any tag
diff <(vost cat ~3:file.py) <(vost cat ~0:file.py)       # any two snapshots
```

### hash

Print the SHA hash of a commit, tree, or blob. An optional `TARGET` argument is interpreted as a ref (bare string) or `ref:path` (with colon).

```bash
vost hash                                 # current branch commit hash
vost hash main                            # main branch commit hash
vost hash v1.0                            # tag commit hash
vost hash ~3                              # 3 commits back
vost hash :config.json                    # blob hash on current branch
vost hash main:src/                       # tree hash of directory
vost hash main~1:file.txt                 # blob hash from one commit back
vost hash --match "deploy*"               # commit matching message
vost hash :file.txt --path file.txt       # blob hash at commit that last changed file
```

A bare string (no `:`) is treated as a ref — branch name, tag name, or commit hash. With `:path`, the object hash at that path is printed instead of the commit hash.

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

### rm

Remove files from the repo. Accepts multiple paths and glob patterns. Directories require `-R`.

```bash
vost rm old-file.txt
vost rm old-file.txt -m "Clean up"
vost rm ':*.txt'                     # glob (quote for shell)
vost rm -R :dir                      # directory
vost rm -n :file.txt                 # dry run
vost rm :a.txt :b.txt               # multiple
```

| Option | Description |
|--------|-------------|
| `-R`, `--recursive` | Remove directories recursively. |
| `-n`, `--dry-run` | Show what would change without writing. |
| `--no-glob` | Treat source paths as literal (no `*` or `?` expansion). |
| `-b`, `--branch` | Branch (default: current branch). |
| `-m`, `--message` | Commit message. |
| `--parent REF` | Additional parent ref (branch/tag/hash). Repeatable. Advisory only — no tree merging. |
| `--tag` | Create a tag at the resulting commit. |
| `--force-tag` | Overwrite tag if it already exists. |

### mv

Move/rename files within the repo. All arguments are repo paths (colon prefix required). The last argument is the destination.

```bash
vost mv :old.txt :new.txt               # rename
vost mv ':*.txt' :archive/              # glob move
vost mv -R :src :dest                   # move directory
vost mv :a.txt :b.txt :dest/            # multiple -> dir
vost mv -n :old.txt :new.txt            # dry run
vost mv dev:old.txt dev:new.txt         # explicit branch
```

| Option | Description |
|--------|-------------|
| `-R`, `--recursive` | Move directories recursively. |
| `-n`, `--dry-run` | Show what would change without writing. |
| `--no-glob` | Treat source paths as literal (no `*` or `?` expansion). |
| `-b`, `--branch` | Branch (default: current branch). |
| `-m`, `--message` | Commit message. |
| `--parent REF` | Additional parent ref (branch/tag/hash). Repeatable. Advisory only — no tree merging. |
| `--tag` | Create a tag at the resulting commit. |
| `--force-tag` | Overwrite tag if it already exists. |

All paths must target the same branch. Cross-branch moves are not supported — use `cp` + `rm` instead.

### write

Write stdin to a file in the repo.

```bash
echo "hello" | vost write file.txt
cat data.json | vost write :config.json
cat image.png | vost write :assets/logo.png -m "Add logo"

# Passthrough (tee mode) — data flows to stdout AND into the repo
cmd | vost write log.txt -p | grep error
tail -f /var/log/app.log | vost write log.txt --passthrough
```

| Option | Description |
|--------|-------------|
| `-p`, `--passthrough` | Echo stdin to stdout (tee mode for pipelines). |
| `-b`, `--branch` | Branch (default: current branch). |
| `-m`, `--message` | Commit message. |
| `--parent REF` | Additional parent ref (branch/tag/hash). Repeatable. Advisory only — no tree merging. |
| `--no-create` | Don't auto-create the repo. |
| `--tag` | Create a tag at the resulting commit. |
| `--force-tag` | Overwrite tag if it already exists. |

---

## History

### log

Show commit history. An optional `TARGET` argument selects the ref to start from. A bare string (no `:`) is treated as a ref; with `:` it supports `ref:path` syntax.

```bash
vost log
vost log main                             # log of main branch
vost log v1.0                             # log starting from tag
vost log main~3                           # log from 3 commits back on main
vost log --path file.txt
vost log --match "deploy*"
vost log --before 2024-06-01
vost log --format json                    # or jsonl

# ref:path syntax
vost log main:config.json                 # --ref main --path config.json
vost log main~3:                          # --ref main --back 3
vost log ~3:config.json                   # --back 3 --path config.json
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref` | Start from branch, tag, or hash. |
| `--path` | Commits that changed this path. |
| `--match` | Commits matching message pattern. |
| `--before` | Commits on or before this date. |
| `--back N` | Walk back N commits from tip. |
| `--format` | `text` (default), `json`, `jsonl`. |

Text format: `SHORT_HASH  ISO_TIMESTAMP  MESSAGE`

### diff

Compare current branch HEAD against another snapshot. An optional `BASELINE` argument selects the comparison target. A bare string (no `:`) is treated as a ref; with `:` it supports `ref:path` syntax. Output uses git-style `--name-status` format (old → new by default):

```
A  new-file.txt          # Added since baseline
M  changed-file.txt      # Modified since baseline
D  removed-file.txt      # Deleted since baseline
```

```bash
vost diff dev                         # what's different vs dev branch
vost diff v1.0                        # what changed since tag
vost diff ~3                          # what changed in last 3 commits
vost diff main~2                      # vs 2 commits back on main
vost diff --before 2025-01-01         # what changed since Jan 1
vost diff --back 3 --reverse          # swap direction (new → old)
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters (select baseline). |
| `--reverse` | Swap comparison direction (A↔D flipped, M stays M). |

### cmp

Compare two files by content hash. Files can be repo paths, local disk paths, or a mix. Exit code follows POSIX `cmp` convention: 0 = identical, 1 = different.

```bash
vost cmp :file1.txt :file2.txt           # two repo files
vost cmp main:f.txt dev:f.txt             # cross-branch
vost cmp main~3:f.txt main:f.txt          # ancestor
vost cmp :data.bin /tmp/data.bin           # repo vs disk
vost cmp /tmp/a.txt /tmp/b.txt             # two disk files
vost -v cmp :old.txt :new.txt             # verbose — show hashes on stderr
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Files are identical (same blob SHA). |
| `1` | Files differ. |

With `-v` (verbose, before the subcommand), both hashes are printed to stderr.

### undo

```bash
vost undo                                 # back 1 commit
vost undo 3                               # back 3 commits
vost undo -b dev                          # undo on 'dev' branch
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |

### redo

```bash
vost redo                                 # forward 1 reflog step
vost redo 2                               # forward 2 steps
vost redo -b dev                          # redo on 'dev' branch
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |

### reflog

```bash
vost reflog
vost reflog -n 10                         # last 10 entries
vost reflog -b dev                        # entries for 'dev' branch
vost reflog --format json
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `-n`, `--limit` | Limit number of entries shown. |
| `--format` | `text` (default), `json`, `jsonl`. |

---

## Refs

### branch

```bash
vost branch                               # list
vost branch list                          # same
vost branch set dev                       # fork from default branch
vost branch set dev --ref main            # fork from specific ref
vost branch set dev --ref main --path config.json
vost branch set dev -f                    # overwrite existing
vost branch set dev --empty               # empty orphan branch
vost branch exists dev                    # exit 0 if exists, 1 if not
vost branch current                       # show current branch
vost branch current -b dev                # set current branch to 'dev'
vost branch delete dev
vost branch hash main                     # tip commit SHA
vost branch hash main --back 3            # 3 commits before tip
vost branch hash main --path config.json  # last commit that changed file
```

#### branch set options

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Source branch (default: current branch). |
| `-f`, `--force` | Overwrite if branch already exists. |
| `--empty` | Create an empty root branch (no parent commit). Cannot combine with other options. |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

#### branch current

With no arguments, prints the current branch name. With `-b NAME`, sets the current branch (HEAD) to NAME. Note: `-b` here means "set current branch to" rather than its usual "operate on branch" meaning.

#### branch exists

Exits with code 0 if the branch exists, 1 if it does not. No output.

#### branch hash options

The positional NAME selects the branch. `--ref` is accepted but has no effect — use `--path`, `--match`, `--before`, and `--back` to select a specific commit.

| Option | Description |
|--------|-------------|
| `--path`, `--match`, `--before`, `--back` | Snapshot filters (see [Snapshot filters](#snapshot-filters)). |

### tag

```bash
vost tag                                  # list
vost tag list                             # same
vost tag set v1.0                         # tag from default branch
vost tag set v1.0 --ref main              # tag from specific ref
vost tag set v1.0 --before 2024-06-01     # tag a historical commit
vost tag set v1.0 -f                      # overwrite existing tag
vost tag exists v1.0                      # exit 0 if exists, 1 if not
vost tag hash v1.0                        # commit SHA
vost tag delete v1.0
```

#### tag set options

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Source branch (default: current branch). |
| `-f`, `--force` | Overwrite if tag already exists. |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

#### tag exists

Exits with code 0 if the tag exists, 1 if it does not. No output.

### note

Manage git notes on commits. Notes are stored in namespaces (default: `commits`).

```bash
vost note get [TARGET]                    # get note (default: current branch)
vost note set [TARGET] "text"             # set note (default: current branch)
vost note delete [TARGET]                 # delete note (default: current branch)
vost note list                            # list commits that have notes
```

#### note get / set / delete

Operate on a specific commit. TARGET can be a 40-char hex commit hash, a branch/tag name (resolved to its tip commit), `:` (current branch), or `ref:` (strip trailing colon). When TARGET is omitted, the current branch is used.

| Option | Description |
|--------|-------------|
| `-N`, `--namespace` | Notes namespace (default: `commits`). |

#### note list

List all commit hashes that have notes in the given namespace.

| Option | Description |
|--------|-------------|
| `-N`, `--namespace` | Notes namespace (default: `commits`). |

---

## Archives

### archive_out / archive_in

Format auto-detected from extension (`.zip`, `.tar`, `.tar.gz`, `.tar.bz2`, `.tar.xz`).

```bash
vost archive_out out.zip
vost archive_out out.tar.gz
vost archive_out - --format tar | gzip > a.tar.gz   # stdout
vost archive_in data.zip
vost archive_in data.tar.gz
vost archive_in --format tar < archive.tar           # stdin
```

#### archive_out options

| Option | Description |
|--------|-------------|
| `--format` | `zip` or `tar` (overrides auto-detect; required for stdout). |
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |

#### archive_in options

| Option | Description |
|--------|-------------|
| `--format` | `zip` or `tar` (overrides auto-detect; required for stdin). |
| `-b`, `--branch` | Branch (default: current branch). |
| `-m`, `--message` | Commit message. |
| `--no-create` | Don't auto-create the repo. |
| `--tag` | Create a tag at the resulting commit. |
| `--force-tag` | Overwrite tag if it already exists. |

### zip / unzip / tar / untar

Aliases with a fixed format. Same options as archive_out/archive_in (including `--tag` / `--force-tag` for import commands).

```bash
vost zip out.zip                          # = archive_out --format zip
vost unzip data.zip                       # = archive_in --format zip
vost tar out.tar.gz                       # = archive_out --format tar
vost untar data.tar.gz                    # = archive_in --format tar
```

---

## Mirror

### backup

Push refs to a remote URL or write a bundle file. Without `--ref` this is a full mirror: remote-only refs are deleted.

```bash
vost backup https://github.com/user/repo.git
vost backup /path/to/other.git
vost backup -n https://github.com/user/repo.git    # dry run
vost backup /tmp/backup.bundle                      # bundle file
vost backup backup.bundle --ref main --ref v1.0     # only specific refs
```

### restore

Fetch refs from a remote URL or import a bundle file. Restore is **additive**: refs are added and updated but local-only refs are never deleted.

```bash
vost restore https://github.com/user/repo.git
vost restore -n https://github.com/user/repo.git   # dry run
vost restore /tmp/backup.bundle                     # bundle file
vost restore backup.bundle --ref main               # only specific refs
```

HEAD (the current branch) is not restored; use `vost branch current -b NAME` afterwards if needed.

| Option | Description |
|--------|-------------|
| `-n`, `--dry-run` | Preview without transferring data. |
| `--ref REF` | Ref to include (repeatable). Short names resolved to branches/tags. Omit for all refs. |
| `--format bundle` | Force bundle format (auto-detected from `.bundle` extension). |
| `--no-create` | Don't auto-create the repo (restore only). |

---

## Server

### serve

Serve repository files over HTTP with content negotiation. Content is resolved live on each request — new commits are visible immediately. Browsers see HTML directory listings and raw file contents; API clients requesting `Accept: application/json` get JSON metadata.

```bash
vost serve                                   # serve HEAD branch at http://127.0.0.1:8000/
vost serve -b dev                            # serve a different branch
vost serve --ref v1.0                        # serve a tag snapshot
vost serve --back 3                          # serve 3 commits before tip
vost serve --all                             # multi-ref: /<branch-or-tag>/<path>
vost serve --all --cors                      # multi-ref with CORS headers
vost serve --base-path /data -p 9000         # mount under /data on port 9000
vost serve --open --no-cache                 # open browser, disable caching
vost serve -q                                # suppress per-request log output
```

| Option | Description |
|--------|-------------|
| `--host` | Bind address (default: `127.0.0.1`). |
| `-p`, `--port` | Port to listen on (default: `8000`, use `0` for OS-assigned). |
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |
| `--all` | Multi-ref mode: expose all branches and tags via `/<ref>/<path>`. |
| `--cors` | Add `Access-Control-Allow-Origin: *` and related CORS headers. |
| `--no-cache` | Send `Cache-Control: no-store` on every response (disables caching entirely). |
| `--base-path PREFIX` | URL prefix to mount under (e.g. `/data`). |
| `--open` | Open the URL in the default browser on start. |
| `-q`, `--quiet` | Suppress per-request log output. |

**Modes:**

- **Single-ref** (default): serves one branch or snapshot. URLs are plain repo paths (`/file.txt`, `/dir/`).
- **Multi-ref** (`--all`): first URL segment selects the branch or tag (`/main/file.txt`, `/v1/dir/`). The root (`/`) lists all branches and tags.

**Content negotiation:**

- `Accept: application/json` returns JSON metadata (path, ref, size, type, entries).
- Otherwise: raw file bytes with MIME types, or HTML directory listings.

**Caching:**

- All 200 responses include `ETag` (commit hash) and `Cache-Control: no-cache`, so browsers always revalidate but get a lightweight **304 Not Modified** when the content hasn't changed.
- `--no-cache` overrides to `Cache-Control: no-store`, disabling caching entirely.

**Response headers:**

- `ETag` is set to the commit hash on all 200 responses.
- JSON, XML, GeoJSON, and YAML files are served as `text/plain` so browsers display them inline.

### gitserve

Serve the repository read-only over HTTP. Standard git clients can clone and fetch from the URL. Pushes are rejected.

```bash
vost gitserve                                # serve at 127.0.0.1:8000
vost gitserve -p 9000                        # custom port
vost gitserve --host 0.0.0.0 -p 8080         # bind all interfaces
git clone http://127.0.0.1:8000/                 # clone from another terminal
```

| Option | Description |
|--------|-------------|
| `--host` | Bind address (default: `127.0.0.1`). |
| `-p`, `--port` | Port to listen on (default: `8000`, use `0` for OS-assigned). |

### mount

Mount a branch or tag as a read-only FUSE filesystem. Requires `pip install vost[fuse]`.

```bash
vost mount /tmp/mnt                          # mount current branch
vost mount /tmp/mnt -b dev                   # mount a different branch
vost mount /tmp/mnt --ref v1.0               # mount a tag
vost mount /tmp/mnt --back 2                 # mount 2 commits before tip
vost mount /tmp/mnt -f                       # run in foreground
```

| Option | Description |
|--------|-------------|
| `-b`, `--branch` | Branch (default: current branch). |
| `--ref`, `--path`, `--match`, `--before`, `--back` | Snapshot filters. |
| `-f`, `--foreground` | Run in foreground (default: daemonize). |
| `--debug` | Enable FUSE debug output. |
| `--nothreads` | Single-threaded mode. |
| `--allow-other` | Allow other users to access the mount. |

---

## Appendix

### Snapshot filters

Several commands accept filters to select a specific commit:

| Option | Description |
|--------|-------------|
| `--ref REF` | Branch, tag, or commit hash. |
| `--path PATH` | Latest commit that changed this file. |
| `--match PATTERN` | Latest commit matching message pattern (`*`, `?`). |
| `--before DATE` | Latest commit on or before this date (ISO 8601). |
| `--back N` | Walk back N commits from tip. |

Filters combine with AND. Available on `cp`, `sync`, `ls`, `cat`, `log`, `diff`, `cmp`, `branch set`, `branch hash`, `tag set`, `archive_out`, `zip`, `tar`, `serve`, `mount`.

### Dry-run output format

```
+ :path/to/new-file       (add)
~ :path/to/changed-file   (update)
- :path/to/removed-file   (delete)
```

### Message placeholders

The `-m` option accepts placeholders that expand at commit time:

| Placeholder | Expands to |
|-------------|------------|
| `{default}` | Full auto-generated message. |
| `{add_count}` | Number of additions. |
| `{update_count}` | Number of updates. |
| `{delete_count}` | Number of deletions. |
| `{total_count}` | Total changed files. |
| `{op}` | Operation name (`cp`, `sync`, `rm`, `mv`, `ar` (archive), or empty). |

```bash
vost cp dir/ :dest -m "Deploy: {default}"
vost sync ./src :code -m "Sync {total_count} files"
```

A message without `{` is used as-is. Unknown placeholders raise an error.

### Exclude patterns

The `--exclude` and `--exclude-from` options use gitignore syntax:

| Pattern | Matches |
|---------|---------|
| `*.pyc` | Any `.pyc` file at any depth |
| `build/` | Directories named `build` (not files) |
| `/build` | `build` at root only (anchored) |
| `!important.log` | Negation — un-excludes a previously excluded file |
| `__pycache__/` | `__pycache__` directories and all their contents |

Multiple `--exclude` flags combine. `--exclude-from` reads one pattern per line (blank lines and `#` comments are skipped).

The `--gitignore` flag (sync only) automatically reads `.gitignore` files from the source directory tree. Each `.gitignore` applies to files in its own directory and below. When active, `.gitignore` files themselves are excluded from the repo.

### Dry-run commands

Available on: `cp`, `sync`, `rm`, `mv`, `backup`, `restore`.

### Advisory parents

The `--parent REF` option (repeatable) adds extra parent commits without merging trees. The branch tip stays the first parent; advisory parents are appended. This records provenance (e.g. "this commit incorporates data from these sources") visible to `git log --graph`. Available on: `write`, `rm`, `mv`, `cp`, `sync`.

### Tag-on-commit commands

The `--tag` and `--force-tag` options create a tag at the resulting commit. Available on: `cp`, `sync`, `rm`, `mv`, `write`, `archive_in`, `unzip`, `untar`.

### Copy behavior

See [Copy behavior](#copy-behavior) in the `cp` section.

---

See also: [Path Syntax](paths.md) | [Python API Reference](api.md) | [CLI Tutorial](cli-tutorial.md) | [README](https://github.com/mhalle/vost#readme)
