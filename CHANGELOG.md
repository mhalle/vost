# Changelog

All notable changes to vost are documented in this file.

## Unreleased

## v0.78.0 / Rust v0.10.5 / vost-server v0.2.0 (2026-03-17)

**Added (all three HTTP servers — Python CLI, Rust CLI, vost-server):**

- Content-addressed blob access: `/_/blobs/{hash}` (explicit) and `/{hash}` (shorthand, blob-first with path fallback)
- `--upstream URL` — redirect (302) to upstream server on blob cache miss, enabling CDN-like server hierarchies
- Per-blob ETags on file responses (survives commits to other files)
- Range requests (`206 Partial Content`, `Accept-Ranges: bytes`)
- `--immutable` flag (`Cache-Control: public, immutable, max-age=31536000`)
- `--max-age N` flag (`Cache-Control: public, max-age=N`)
- JSON file metadata now includes `hash` field (blob SHA)
- Normalized CORS headers (`*` wildcards) and response format across all servers

**Added (vost-server only):**

- New crate: `vost-server/` — high-performance HTTP file server built on axum + tokio
- Blob cache with `DashMap` (sharded locking) + `Bytes` (ref-counted, no copy on cache hit)
- `spawn_blocking` on all git reads (async runtime stays responsive)
- `--compress` / `--no-compress-type` for selective gzip compression
- `--cache-size N` (default 4096 blob objects)
- GitHub Actions release workflow (`server-v*` tags)

**Added (Python + Rust CLI):**

- `--format json|jsonl` on `diff`, `hash`, `branch list/hash/current`, `tag list/hash`, `note list`
- `--output-format json|jsonl` on `backup --dry-run` and `restore --dry-run`

**Fixed (Python + Rust CLI):**

- `serve`: directory entries in listings now show trailing `/`
- `serve`: ~100+ file extensions served as `text/plain` instead of `application/octet-stream`

## v0.77.1 / Rust v0.10.4 (2026-03-16)

**Fixed (Python + Rust CLI):**

- `serve`: directory entries in listings now show trailing `/` (HTML and JSON)
- `serve`: ~100+ source code, config, and data file extensions (`.py`, `.rs`, `.toml`, `.sh`, etc.) now served as `text/plain` so browsers display rather than download
- `serve`: well-known extensionless filenames (`Makefile`, `Dockerfile`, `LICENSE`, etc.) also served as text

## v0.77.0 / Rust v0.10.3 (2026-03-16)

**Added (Python + Rust CLI):**

- `--format json|jsonl` on `diff`, `hash`, `branch list`, `branch hash`, `branch current`, `tag list`, `tag hash`, `note list` — all structured output is now programmatically consumable
- `--output-format json|jsonl` on `backup --dry-run` and `restore --dry-run` (separate from existing `--format bundle`)

## v0.76.0 / Rust v0.10.0 (2026-03-16)

**Added (Rust CLI):**

- Feature-identical Rust CLI behind `cli` feature gate (32 commands matching Python CLI)
- `serve` — HTTP file server with single-ref/multi-ref modes, ETag/304, CORS, JSON API, directory listings, `--base-path`, `--open`, `--max-file-size`, `--log-file` (CLF access log)
- Cross-port CLI testing: `VOST_CLI=rust` runs Python test suite against Rust binary (437 tests)
- `make test-rs-cli` target

**Added (Rust library):**

- Multi-source `copy_in(&[&str], dest, opts)` / `copy_out(&[&str], dest, opts)` — file/dir/contents mode, `/./` pivot, matching Python/TS/Kotlin API
- `CopyInOptions.follow_symlinks` — dereference symlinks during copy, with cycle detection
- `ExcludeFilter.gitignore` — per-directory `.gitignore` loading during walk
- `SyncOptions.ignore_errors` — skip unreadable files and continue
- Mtime-based skip when `checksum=false` — only re-copies files modified after the commit timestamp
- `sync_in` / `sync_out` now take string paths (not `&Path`)
- `Fs::at_commit(hash)` — navigate to a commit hash preserving ref context
- `GitStore::create_empty_branch(name)` — create a root branch without direct git2 access
- `hash_blob(data)` — compute git blob SHA without a repository
- `disk_glob_ext()` with follow_symlinks and exclude_filter
- Short hash resolution via `revparse_single` in `GitStore::fs()`

**Added (Python):**

- `serve --max-file-size` — limit served file size (default 250 MB, 413 for oversized)
- `serve --log-file` — CLF access log to file, structured error logging via `logging`

**Fixed (all ports):**

- `--delete` + `--exclude` now preserves excluded files in destination (rsync behavior). Previously excluded files were incorrectly deleted. Validated against rsync with new `test_rsync_compat.py` suite.

**Changed (Rust library — breaking):**

- `Fs::copy_in` signature changed from `(src: &Path, dest, opts)` to `(sources: &[&str], dest, opts)`
- `Fs::copy_out` signature changed from `(src: &str, dest: &Path, opts)` to `(sources: &[&str], dest: &str, opts)`
- `Fs::sync_in` signature changed from `(src: &Path, dest, opts)` to `(src: &str, dest, opts)`
- `Fs::sync_out` signature changed from `(src: &str, dest: &Path, opts)` to `(src: &str, dest: &str, opts)`

**Documentation:**

- `docs/testing.md` — cross-port CLI testing guide
- `docs/cli.md` — `--delete` + `--exclude` interaction documented
- `docs/api.md` — exclude preservation noted in copy/sync overview

## v0.75.2 / TS v0.9.9 / Rust v0.9.8 / C++ v0.9.1 / Kotlin v0.9.10 (2026-03-15)

**Added (all ports):**

- `store.pack()` — pack loose objects into a packfile. Returns number of objects packed.
- `store.gc()` — native garbage collection (clean up + pack). No longer requires `git` to be installed.
- TypeScript: `pack()` and `gc()` throw "not implemented" (isomorphic-git lacks ODB API).

**Changed (Python CLI):**

- `vost gc` now uses native `store.gc()` instead of shelling out to `git gc`. No longer requires `git`.
- Added `vost pack` CLI command for pack-only operation.

## v0.75.1 (2026-03-14)

**Added:**

- `vost branch set --append --squash` — append source tree as a new commit on branch tip
- `--append` without `--squash` reserved for future chain-replay (errors with "not yet implemented")

## v0.75.0 / TS v0.9.8 / Rust v0.9.7 / C++ v0.9.0 / Kotlin v0.9.9 (2026-03-14)

**Added (all ports):**

- `store.fs(ref)` — unified ref resolution (branches → tags → commit hash) from a single entry point
- Advisory `parents` parameter on all commit-producing methods (`write`, `apply`, `batch`, `remove`, `move`, `copy_in`, `sync_in`, `copy_from_ref`). Extra parent commits are recorded in the commit object without merging trees, enabling provenance tracking.
- `fs.squash(parent, message)` — create a new commit with the same tree but collapsed history. Optional parent for grafting onto an existing branch tip.
- Ref renaming on `backup`/`restore`/`bundle_export`/`bundle_import` — pass a dict/map (src→dst) to rename refs during transfer. CLI: `--ref src:dst` syntax.
- `bundle_export(squash=True)` — strip history when creating bundles. Each ref becomes a single parentless commit with the current tree.

**Added (Python CLI):**

- `--parent REF` option (repeatable) on `write`, `rm`, `mv`, `cp`, `sync` for advisory parent commits
- `--squash` flag on `vost branch set` to create single-commit branches
- `--squash` flag on `vost backup` for squashed bundle export
- `--ref src:dst` syntax on `vost backup` / `vost restore` for ref renaming

**Docs:**

- Added `store.fs()` to API reference
- Added `--parent` and advisory parents appendix to CLI reference

## v0.74.0 (2026-03-12)

**Fixed:**

- CLI `--repo` flag now correctly overrides `VOST_REPO` env var (previously the env var could silently win)
- Broken pipe no longer prints `Exception ignored in: <stdout>` when piping CLI output to `head`, `less`, etc.

**Changed:**

- CLI tests no longer leak the host `VOST_REPO` environment variable

**Docs:**

- Added process substitution diff examples to `cat` CLI docs

## v0.73.0 (2026-03-02)

**Added:**

- fsspec filesystem adapter (`pip install vost[fsspec]`) — use vost repos with pandas, xarray, dask, and any fsspec-aware tool via the `vost://` protocol
- `readonly` storage option to block writes even on branches
- Documentation page for fsspec integration

**Changed:**

- `RELEASING.md` version table now uses placeholder versions

## TS v0.9.7 (2026-03-01)

**Changed (TS):**

- `autoCreateBareRepo` now uses isomorphic-git `init({ bare: true })` instead of shelling out to `git init --bare`

## C++ v0.8.9 (2026-03-01)

**Fixed (C++):**

- `bundle_export` now uses `git_revwalk` + `git_packbuilder_insert_walk` to include full commit ancestry in bundles (consistent with Rust v0.9.6 fix; `git_packbuilder_insert_commit` only packs a single commit and its tree)

## C++ v0.8.8 (2026-03-01)

**Added (C++):**

- `bundle_export()` / `bundle_import()` — public API methods on `GitStore` for creating and importing bundle files directly

## C++ v0.8.7 (2026-03-01)

**Changed (C++):**

- Bundle export/import now uses native libgit2 (`git_packbuilder`, `git_indexer`, bundle v2 header parsing) instead of shelling out to `git bundle` CLI

## Rust v0.9.6 (2026-03-01)

**Changed (Rust):**

- Mirror module now uses native git2 (libgit2) for all transport and bundle operations instead of shelling out to `git` CLI. Converted: `bundle_export`, `bundle_import`, `bundle_list_heads`, `mirror_push`, `targeted_push`, `additive_fetch`, `get_remote_refs`. Only `resolve_credentials` still uses external CLI (`git credential fill`, `gh auth token`).

**Added (Rust):**

- `bundle_export()` / `bundle_import()` — public API methods on `GitStore` and standalone functions in `vost::mirror` for creating and importing bundle files directly

## Rust v0.9.5 / C++ v0.8.6 / Kotlin v0.9.8 (2026-03-01)

**Fixed (Kotlin):**

- Root-path handling: `exists("")`, `isDir("")`, `fileType("")`, `size("")`, `objectHash("")` now behave correctly instead of throwing or returning wrong results

**Fixed (C++):**

- `RefDict::set()` now validates ref names using the full `validate_ref_name()` validator (rejects `..`, `@{`, `.lock`, trailing `.`, special characters) instead of only checking for empty names
- `Fs::apply()` now validates `WriteEntry` fields before processing — rejects entries with inconsistent data/target/mode (e.g. blob without data, symlink without target)

**Fixed (Rust, C++, Kotlin):**

- Integer overflow protection in ranged reads (`read_range` / `read_by_hash`): `offset + size` now uses saturating arithmetic to prevent wrap-around on large values

## v0.71.0 / TS v0.9.6 (2026-03-01)

**Added (Python, TypeScript):**

- `bundle_export()` / `bundleExport()` — public API method on `GitStore` and standalone function for creating bundle files directly
- `bundle_import()` / `bundleImport()` — public API method on `GitStore` and standalone function for importing bundle files directly
- Both functions are also exported from the top-level package

**Changed (Python):**

- Internal `_bundle_export` / `_bundle_import` functions now take `store: GitStore` instead of raw dulwich repo; renamed to `bundle_export` / `bundle_import` (public)

**Changed (TypeScript):**

- Bundle operations now use native isomorphic-git instead of shelling out to `git bundle` CLI
- `bundleExport` / `bundleImport` are now exported from the mirror module

## v0.70.1 / Rust v0.9.4 (2026-02-27)

**Changed (Rust):**

- Internal: migrate from `gix` + `gix-lock` to `git2` (libgit2) + `fs2`, aligning with the C++ port's backend. No public API changes. Dependency count reduced by 74% (523 → 138 crate-graph entries).

## v0.70.0 / TS v0.9.5 / Rust v0.9.3 / Kotlin v0.9.7 / C++ v0.8.5 (2026-02-27)

**Added (all ports):**

- Backup to `.bundle` files: `store.backup("backup.bundle")` creates a git bundle for offline transfer
- Restore from `.bundle` files: `store.restore("backup.bundle")` imports refs from a bundle
- Refs filtering: `refs` parameter limits backup/restore to specific branches/tags (e.g. `refs: ["main"]`)
- `format: "bundle"` option forces bundle format even without `.bundle` extension

**Changed (all ports):**

- Restore is now **additive**: local-only refs are no longer deleted during restore. Only refs present in the source are added/updated.

**API changes (Rust, C++):**

- `backup()` / `restore()` now take `BackupOptions` / `RestoreOptions` structs instead of a bare `bool dry_run`

**API changes (TypeScript):**

- `http` parameter is now optional for `backup()` / `restore()` (only required for HTTP URLs; local paths and bundles use git CLI)
- Added `refs` and `format` options to backup/restore opts

**API changes (Kotlin):**

- `backup()` / `restore()` accept new `refs: List<String>?` and `format: String?` parameters

## v0.69.0 (2026-02-27)

**Added (CLI):**

- `hash` command: print SHA hash of a commit, tree, or blob (`vost hash`, `vost hash main`, `vost hash :file.txt`)
- `log` and `diff` accept bare-ref positional syntax (`vost log main`, `vost diff dev`, `vost diff ~3`)
- `note get`, `note set`, `note delete` default to the current branch when no target is given
- `note get/set/delete` accept `:` (current branch) and `ref:` (trailing colon stripped) target syntax

**Changed (CLI):**

- Remove `note get-current` / `note set-current` commands — use `note get` / `note set` without a target instead

**Docs:**

- Rewrite CLI tutorial: add introduction, restructure section order, move cross-cutting concepts earlier (`:` syntax, commit messages, globs, dry-run, JSON output), add sections for hashes and `/./` pivot syntax, expand install options

## v0.68.4 / TS v0.9.4 / Kotlin v0.9.6 (2026-02-27)

**Fixed (Python, TypeScript, Kotlin):**

- Glob patterns with multiple wildcards (e.g. `*/*/*`) no longer crash when a wildcard matches a file instead of a directory. The glob walker now skips non-directory entries when more pattern segments remain. (Rust and C++ were already correct.)

## v0.68.3 / TS v0.9.3 / Rust v0.9.2 / Kotlin v0.9.5 / C++ v0.8.4 (2026-02-27)

**Fixed (all ports):**

- Path normalization now collapses `.` segments instead of rejecting them. Paths like `./dir/file.txt` normalize to `dir/file.txt`. Paths that resolve to empty (`.`, `./.`) are still rejected. `..` remains rejected.

## v0.68.2 / Rust v0.9.1 / Kotlin v0.9.4 / C++ v0.8.3 (2026-02-26)

**Docs (Rust, Kotlin, C++):**

- Rewrite rs/README.md with correct `vost` imports, full API examples for all operations, and error handling guide
- Add kotlin/README.md with full API examples for all operations
- Add kotlin/docs/api.md — complete Kotlin API reference covering all classes, methods, and types
- Expand cpp/README.md from API table to comprehensive code example sections
- Add cpp/docs/api.md — complete C++ API reference covering all classes, methods, and types
- Add language ports table to root README.md linking all five ports

## v0.68.1 / TS v0.9.2 / Kotlin v0.9.3 / C++ v0.8.2 (2026-02-26)

**Added (Python, TypeScript, Kotlin, C++):**

- `copy_from_ref` now accepts a branch or tag name string in addition to an FS object. Resolution tries branches first, then tags.

## v0.68.0 / TS v0.9.1 / Kotlin v0.9.3 / C++ v0.8.2 (2026-02-26)

**Added (Python, TypeScript, Kotlin, C++):**

- Notes API (`get`/`set`/`delete`/`has`/`contains` and batch equivalents) now accepts FS snapshots in addition to commit hashes and ref names. The snapshot's commit hash is used automatically.

## v0.67.2 / TS v0.9.0 (2026-02-26)

**Changed (TypeScript):**

- `GitStore.open()` no longer requires the `fs` option — defaults to Node's `node:fs` module. Pass a custom `fs` only for non-Node environments (e.g. `lightning-fs` in browsers).

## v0.67.1 / TS v0.8.2 (2026-02-26)

**Docs (TypeScript):**

- Rewrite ts/README.md with correct `@mhalle/vost` imports, full API examples, and Deno usage
- Add ts/docs/api.md — complete TypeScript API reference covering all classes, methods, types, and error classes

## v0.67.0 (2026-02-26)

**Added (all ports):**

- Notes API (`get`/`set`/`delete`/`has`/`contains` and batch equivalents) now accepts branch or tag names in addition to 40-char hex commit hashes. Ref names are resolved to the tip commit hash internally. Existing hash-based usage is unchanged.

## v0.66.2 (2026-02-26)

**Fixed:**

- Deno compat tests: add `--no-check` flag to skip type-checking of compiled `.js` imports (fixes 24 type errors)
- TESTING.md: fix Rust test count (558 → 549), add C++ configure step to "Running everything" section, add Kotlin report path

## v0.66.1 / Kotlin v0.9.2 / C++ v0.8.1 (2026-02-26)

**Tests:**

- Kotlin: +65 tests (205 → 270) — apply, move, copy/sync/copyFromRef, store, notes, stat, glob
- C++: +78 tests (267 → 345) — apply, move, copy/sync/copyFromRef, batch, notes, stat, history, exclude filter, FsWriter
- Total: 3,165 tests across 5 ports + full 5-language interop matrix

## v0.66.0 / TS v0.8.1 / Rust v0.9.0 / Kotlin v0.9.1 / C++ v0.8.0 (2026-02-26)

**Added (Rust):**

- `backup()` / `restore()` — mirror operations (push/fetch all refs via git CLI, auto-create bare repos, stale ref deletion, dry-run mode)
- `resolve_credentials()` — inject HTTPS credentials via `git credential fill` / `gh auth token`
- 9 mirror tests (549 → 558 total)

**Added (C++):**

- `GitStore::backup()` / `GitStore::restore()` — mirror operations using libgit2 native transport (push/fetch all refs, auto-create bare repos, stale ref deletion, dry-run mode)
- `resolve_credentials()` — inject HTTPS credentials via `git credential fill` / `gh auth token`
- 9 mirror tests (258 → 267 total)

**Added (TypeScript):**

- `resolveCredentials()` — inject HTTPS credentials via `git credential fill` / `gh auth token` (async, Node.js only — returns original URL in browser)

**Added (Kotlin):**

- `resolveCredentials()` — inject HTTPS credentials via `git credential fill` / `gh auth token`

**Tests:**

- Total: 3,022 tests across 5 ports + full 5-language interop matrix

## v0.65.1 / TS v0.8.0 / Rust v0.8.0 / Kotlin v0.9.0 / C++ v0.7.2 (2026-02-26)

**Breaking (Kotlin):**

- Rename `ReadOnlyError` → `PermissionError` to match Python/TS/Rust/C++
- Rename `IsADirectoryException` → `IsADirectoryError`, `NotADirectoryException` → `NotADirectoryError` (deprecated typealiases provided)

**Added (TypeScript):**

- `ExcludeFilter` class — gitignore-style pattern matching for `copyIn`/`syncIn` operations
- 5 typed error classes: `KeyNotFoundError`, `KeyExistsError`, `InvalidRefNameError`, `InvalidPathError`, `BatchClosedError` (all extend `GitStoreError`)

**Added (Rust):**

- `ExcludeFilter` struct — gitignore-style pattern matching for `copy_in`/`sync_in` operations

**Added (Kotlin):**

- `Fs.writeFromFile()` and `Batch.writeFromFile()` — import local file into repo with auto-detect executable bit

**Fixed:**

- Python: fix duplicate `test_read_with_path` in `test_fs_read.py` (second renamed to `test_read_text_with_path`)
- Rust: fix 3 broken doctests (`GitStore::open` signature change, borrow checker in `BatchWriter` example)
- TypeScript: fix `@throws {ReadOnlyError}` → `@throws {PermissionError}` in copy.ts doc comment

**Tests:**

- C++: add `test_stat.cpp` (8), `test_apply.cpp` (7), `test_move.cpp` (6) — 237 → 258 tests
- Kotlin: expand StatTest, FsWriteTest, BatchTest, CopyTest — 164 → 205 tests
- TypeScript: add ExcludeFilter tests — 616 → 631 tests
- Rust: add ExcludeFilter unit tests — 516 → 540 tests
- Total: 2,996 tests across 5 ports + full 5-language interop matrix

## v0.65.0 / TS v0.7.1 / Rust v0.7.1 / Kotlin v0.8.1 (2025-02-25)

**Documentation (Kotlin, C++, TypeScript, Rust):**

- Add missing docstrings to ~40 public API members across all four non-Python ports, using Python as the source of truth
- Kotlin: KDoc added to `RefDict`, `Fs`, `Batch`, `Notes` classes
- C++: Doxygen `@param`/`@throws`/`@return` added to `RefDict`, `Batch`, `Fs`, `Notes`, `Types` headers
- TypeScript: JSDoc expanded on `mirror`, `copy`, `tree`, `reflog`, `fs`, `notes` modules
- Rust: `///` doc comments expanded on `store`, `refdict` modules

## v0.64.0 / Kotlin v0.8.0 (2026-02-25)

**Breaking (Kotlin):**

- Rename `Batch.result` → `Batch.fs` to match Python/TS/Rust/C++ naming; remove `resultFs` alias

**Added (Kotlin):**

- `ExcludeFilter` class — gitignore-style pattern matching for `copyIn`/`syncIn` operations (negation, dir-only, anchored patterns, load from file)
- `GitStore.backup(url)` / `GitStore.restore(url)` — mirror operations using JGit transport (push/fetch all refs, auto-create bare repos, stale ref deletion, dry-run mode)
- `MirrorDiff.inSync` and `.total` properties

**Housekeeping:**

- Add `cpp/build/`, `kotlin/build/`, `kotlin/.gradle/`, `site/` to `.gitignore`

## v0.63.0 / TS v0.7.0 / Rust v0.7.0 (2026-02-25)

**Breaking (TypeScript):**

- Rename `Batch.result` → `Batch.fs` and `FsWriter.result` → `FsWriter.fs` to match Python/C++/Rust naming

**Breaking (C++):**

- Rename `NoteNamespace::get_current()`/`set_current()` → `get_for_current_branch()`/`set_for_current_branch()`
- Rename `Fs::move_paths()` → `Fs::move()`

**Breaking (C++ + Rust):**

- `ls()` now returns name strings (`vector<string>` / `Vec<String>`); use `listdir()` for `WalkEntry` objects
- `walk()` now returns os.walk-style `WalkDirEntry` structs with `(dirpath, dirnames, files)` instead of flat `(path, entry)` pairs

**Breaking (Rust):**

- `RefDict::set_and_get()` now returns the new writable `Fs` (was `Option<Fs>` of the old value); `set_to()` deprecated

**Added (C++ + Rust):**

- `operation` parameter on `BatchOptions` and `ApplyOptions` — prefix for auto-generated commit messages
- `WalkDirEntry` type for os.walk-style directory traversal

**Added (C++):**

- `Batch::fs()` accessor — retrieve the resulting `Fs` after `commit()`

**Fixed (C++ + Rust):**

- `ChangeReport.warnings` type corrected from `string` to `ChangeError`

## v0.62.0 / TS v0.6.0 / Rust v0.6.0 (2026-02-25)

**Breaking (all languages):**

- Package renamed from `gitstore` to `vost` (Versioned Object STore) — update imports: `from vost import ...`, `use vost::`, `from 'vost'`
- Rust crate renamed: `gitstore` → `vost` in `Cargo.toml`
- npm package renamed: `gitstore` → `vost` in `package.json`
- CLI command renamed: `gitstore` → `vost`
- Environment variable renamed: `GITSTORE_REPO` → `VOST_REPO`
- Default author/email changed: `"gitstore"`/`"gitstore@localhost"` → `"vost"`/`"vost@localhost"`
- Lock file renamed: `gitstore.lock` → `vost.lock`
- The `GitStore` class name is unchanged

## v0.61.0 / TS v0.5.0 / Rust v0.5.0 (2025-02-25)

**Breaking (all languages):**

- Remove `FS.open()` and `Batch.open()` — use `FS.writer()` / `Batch.writer()` instead
- Remove `ReadableFile` class (Python) — use `fs.read()` with `offset`/`size` for partial reads

**Added (all languages):**

- `FS.writer(path)` — returns a buffered writer that commits on close
- `Batch.writer(path)` — returns a buffered writer that stages to the batch on close
- Python: text mode support via `writer(path, "w")` (UTF-8); binary mode via `writer(path, "wb")` (default)
- TypeScript: `FsWriter` and `BatchWriter` classes accept `Uint8Array` or `string`
- Rust: `FsWriter` and `BatchWriter` implement `std::io::Write`; auto-commit/stage on `Drop`

## v0.60.2 (2026-02-24)

**All languages:**

- Fix `copy_from_ref` docs: update Rust `CopyFromRefOptions` field doc and README examples for rsync semantics

## v0.60.1 (2026-02-24)

**Python:**

- Fix CLI help: add `export VOST_REPO=data.git` to quick start examples so they work as a copy-paste sequence

## v0.60.0 / TS v0.4.0 / Rust v0.4.0 (2026-02-24)

**Breaking (all languages):**

- `copy_from_ref` now follows rsync trailing-slash conventions (matching `copy_in`/`copy_out`):
  - `"config"` = directory mode (copies `config/` *as* `config/` under dest)
  - `"config/"` = contents mode (pours contents into dest)
  - `"file.txt"` = file mode (copies the single file)
  - `""` or `"/"` = root contents mode (copies everything)
- New signature accepts multiple sources:
  - Python: `copy_from_ref(source, sources="", dest="", *, delete, dry_run, message)`
  - TypeScript: `copyFromRef(source, sources?, dest?, opts?)`
  - Rust: `copy_from_ref(&self, source, sources: &[&str], dest: &str, opts)`
- `sources` replaces old `src_path`; accepts `str | list[str]` (Python/TS) or `&[&str]` (Rust)
- `dest` replaces old `dest_path`; defaults to `""` (root) instead of mirroring src_path
- Nonexistent source paths now raise `FileNotFoundError` / `Error::NotFound` (previously silent noop)

**TypeScript:**

- Export `resolveRepoSources` and `ResolvedRepoSource` from `copy.ts`

## v0.59.5 (2026-02-24)

**All languages:**

- Improve `copy_from_ref` docstrings: document subtree-prefix semantics vs rsync trailing-slash conventions, note tree-object splicing internals

**Python:**

- README: add trailing-slash and `/./` pivot examples to CLI cp section
- README: fix `copy_from_ref` example to show both same-path and different-path usage

## v0.59.4 (2026-02-24)

**Python:**

- README: rewrite intro to highlight API, CLI, and Git compatibility
- README: fix `uvx` CLI syntax example

## v0.59.3 (2026-02-24)

**Python:**

- README: add `uvx` / `uv tool install` examples for CLI
- README: note that `gc` and `backup`/`restore` shell out to `git`; all other commands are self-contained
- README: use `branches.current` in quick start example

## v0.59.2 (2026-02-24)

**Python:**

- README: show `branches.current` usage, note default branch name is "main"

## v0.59.1 (2026-02-24)

**Python:**

- Fix `[project.urls]` table ordering in pyproject.toml
- Add GitHub Actions workflow for PyPI trusted publishing (automatic on tag push)

## v0.59.0 (2026-02-24)

**Breaking:**

- Rename `copy_ref` → `copy_from_ref` across Python, TypeScript, and Rust (including `CopyRefOptions` → `CopyFromRefOptions` in Rust)

**Python:**

- Add project URLs to pyproject.toml (Homepage, Repository, Issues, Changelog visible on PyPI)
- Add GitHub repository link to README Documentation section
- Bump dulwich minimum to `>=1.0.0`

## v0.58.5 (2026-02-24)

**Python:**

- README: prefer `write_text`/`read_text` in examples, use `FileType.EXECUTABLE` instead of raw octal, fix "Git" capitalization

## v0.58.4 (2026-02-24)

**Python:**

- Update README with missing API docs (partial reads, stat, listdir, apply, copy_from_ref, disk_glob, reflog, git notes)
- Fix stale API names in README (`branches.default` → `.current`, `fs.branch` → `.ref_name`)
- Add `readme` and `description` fields to pyproject.toml for PyPI
- Add sdist excludes for `rs/target`, `ts/node_modules`, `site`, `tmp`

## v0.58.3 / TS v0.3.3 / Rust v0.3.3 (2026-02-24)

**Python:**

- Add `py.typed` marker file (PEP 561) for typed package support
- Add pyproject.toml classifiers (Beta status, Python versions, license, topics)

**TypeScript:**

- Add package README

**Rust:**

- Fix clippy warnings (redundant closures, useless conversions, collapsible ifs, etc.)

## v0.58.2 / TS v0.3.2 / Rust v0.3.2 (2026-02-24)

**TypeScript & Rust:**

- Add comprehensive inline API documentation (JSDoc / rustdoc) to all public types, structs, enums, functions, and methods — matching the Python docstrings added in v0.58.1

## v0.58.1 / TS v0.3.1 / Rust v0.3.1 (2026-02-24)

**Rust bug fixes:**

- Fix `sync_out` to delete extra local files and prune empty directories (was only copying, not syncing)
- Fix `undo()`/`redo()` to reject stale snapshots via branch tip check before ref update
- Fix notes writes to use CAS (`ExistingMustMatch`/`MustNotExist`) instead of unconditional overwrite
- Fix `RefDict::set()` to validate ref names, reject cross-repo Fs, prevent tag overwrites, and write reflog entries
- Fix `log(path=...)` filter to compare both OID and mode (catches mode-only changes)
- Fix `tags().set_current()`/`reflog()` to return errors and `get_current()` to return `None`
- Fix `copy_in` checksum parameter to skip unchanged files via blob OID comparison

**Python:**

- Add comprehensive docstrings with Args/Attributes/Returns/Raises to all public API classes and methods
- Add `mfusepy` as optional FUSE dependency

## v0.58.0 / TS v0.3.0 / Rust v0.3.0 (2026-02-24)

**API changes:**

- Remove `FS.export()` method from all three languages — use `copy_out("/", dest)` instead **[breaking]**

**CLI changes:**

- Rename `archive` → `archive_out` and `unarchive` → `archive_in` for consistent directionality matching `copy_in`/`copy_out` **[breaking]**

## Rust v0.1.1 (2026-02-24)

**Cross-platform API consistency:**

- Rename `ChangeError.message` → `.error` to match Python/TS **[breaking]**
- Rename `FileType::to_mode()` → `.filemode()` to follow git naming conventions (Python: `.filemode`, TS: `fileModeFromType()`) **[breaking]**
- Re-export `disk_glob` from crate root for public use

## v0.57.0 (2026-02-24)

**API changes:**

- Rename `WalkEntry.filemode` → `.mode` for cross-language consistency (TS/Rust already use `mode`) **[breaking]**
- Add `Batch.commit()` method for explicit commit without context manager (matches TS/Rust API)
- Export `Batch`, `Signature`, `RefDict`, `BlobOid`, `GitError` from top-level `vost` package

**CLI changes:**

- Rename `branch default` → `branch current` to match `store.branches.current` API **[breaking]**
- Add `note` command group: `get`, `set`, `delete`, `list`, `get-current`, `set-current`

**Internal:**

- Rename `_default_branch()` → `_current_branch()` CLI helper

## v0.56.0 (2026-02-24)

**New features:**

- Add git notes support with `NoteDict`, `NoteNamespace`, and `NotesBatch` — per-namespace mapping of commit hashes to UTF-8 text, with batch mode for single-commit bulk operations
- Port git notes to TypeScript and Rust with full API parity (93 new TS tests, 46 new Rust tests)
- Add cross-language interop tests for git notes (py↔ts↔rs) covering unicode and multiline text

**API changes:**

- Add `FS.ref_name` property (replaces `.branch`) — returns ref name for branches and tags
- Add `FS.writable` property — `True` for branches, `False` for tags/detached commits
- Add `RefDict.current` / `current_name` (replaces `.default`) — access HEAD branch
- Port `FS.ref_name`, `.writable`, and `RefDict.current` API to TypeScript and Rust
- Add `FS.copy_from_ref()` for branch-to-branch atomic copy; port to TypeScript and Rust (42 new tests each)
- Rename notes `current_ref` → `for_current_branch` across all bindings for clarity

## v0.55.0 (2026-02-22)

**New features:**

- Add TypeScript port (`ts/`) using isomorphic-git — full API parity with Python including FS read/write, batch, copy/sync, glob, undo/redo, reflog, mirror, and 483 tests
- Add Rust port (`rs/`) using gitoxide (gix) — aligned API with Fs metadata accessors, apply with removes, repo-level remove, multi-source move, log filtering, true sync_in with add/update/delete detection, and 398 tests
- Add cross-language interop test suite (`interop/`) — Python writes fixtures, TypeScript and Rust read and verify
- Add `Makefile` with `test-all`, `test-py`, `test-ts`, `test-rs`, and `test-interop` targets
- Add `scripts/test-parity.sh` for API parity checking across implementations

**Python changes:**

- Replace internal dulwich access in tests with public vost API (`fs.file_type()`)
- Guard `_Repository.path` trailing slash with `os.path.isdir`
- Add `tests/test_move.py` with 13 tests for move/rename operations

## v0.54.2 (2026-02-16)

**Internal:**

- Use dulwich's reflog API (`set_if_equals` with message, `Repo.read_reflog()`) instead of direct file I/O, making reflog handling backend-agnostic and enabling `dulwich-sqlite` support

## v0.54.1 (2026-02-13)

**Bug fixes:**

- Fix symlink parent directory escape: `copy_out`/`sync_out` could write outside the destination when a parent directory was a symlink
- Fix `ignore_existing` overwriting dangling symlinks instead of skipping them
- Fix `.gitignore` negation precedence: nested negation patterns (`!file`) now correctly override parent exclusions (deepest matching rule wins)

## v0.54.0 (2026-02-13)

**New features:**

- Add `WriteEntry` dataclass for describing file writes (bytes, str, Path, or symlink target with optional mode)
- Add `FS.apply()` method for atomic multi-write + multi-remove in a single commit
- `WriteEntry` and `FS.apply()` exported from top-level `vost` package

## v0.53.1 (2026-02-13)

**Enhancements:**

- `ls -l` now shows object hashes (7-char short hash by default, `--full-hash` for full 40-char)
- JSON output from `ls -l` always includes the full hash

## v0.53.0 (2026-02-13)

**Breaking changes:**

- Remove `glob` parameter from `copy_in()`, `copy_out()`, `remove()`, and `move()` — callers now expand patterns before calling these methods using `fs.glob()` or `disk_glob()`
- Rename `_expand_disk_glob` to `disk_glob` and export from top-level `vost` package

**Enhancements:**

- `fs.glob()` and `fs.iglob()` now preserve `/./` pivot markers (rsync `-R` style) in results
- `disk_glob()` naturally preserves `/./` pivots via filesystem traversal
- Simplify CLI glob helpers to trivial loops (pivot logic now handled by globs themselves)

## v0.52.0 (2026-02-12)

**New features:**

- Add `fs.file_type(path)` — return the `FileType` of a path (`BLOB`, `EXECUTABLE`, `LINK`, `TREE`)
- Add `fs.size(path)` — return file size in bytes without reading the full blob
- Add `fs.object_hash(path)` — return the 40-char hex SHA of a blob or tree

## v0.51.5 (2026-02-12)

**New features:**

- Add `branches.default` read/write property to `RefDict` for discovering and setting the default branch
- CLI `branch default` now uses the public `branches.default` property

## v0.51.4 (2026-02-12)

**Documentation:**

- Add dedicated "Repo paths and the `:` prefix" section near the top of README and CLI reference, covering syntax, when `:` is required, direction detection, and ref writability

## v0.51.3 (2026-02-12)

**Bug fixes:**

- Fix repo-to-repo `cp --delete` with non-root dest path deleting all destination files due to key-space mismatch between `_walk_repo` (relative) and `_enum_repo_to_repo` (absolute) paths
- Fix repo-to-repo `cp --delete --ignore-existing` incorrectly skipping files that were scheduled for deletion
- Fix implicit source in repo-to-repo `cp` resolving from the destination branch instead of the default branch
- Fix single-file repo-to-disk `cp` dropping the executable bit on `0o755` files

## v0.51.2 (2026-02-12)

**New features:**

- Add `--no-glob` flag to `ls` CLI command

## v0.51.1 (2026-02-12)

**New features:**

- Add `--no-glob` flag to `cp`, `rm`, and `mv` CLI commands — treats source paths as literal (no `*` or `?` expansion)

## v0.51.0 (2026-02-12)

**Breaking changes:**

- Replace 12 standalone copy/sync/remove/move functions with 6 FS methods:
  - `copy_to_repo()` / `copy_to_repo_dry_run()` → `fs.copy_in(..., dry_run=False)`
  - `copy_from_repo()` / `copy_from_repo_dry_run()` → `fs.copy_out(..., dry_run=False)`
  - `sync_to_repo()` / `sync_to_repo_dry_run()` → `fs.sync_in(..., dry_run=False)`
  - `sync_from_repo()` / `sync_from_repo_dry_run()` → `fs.sync_out(..., dry_run=False)`
  - `remove_in_repo()` / `remove_in_repo_dry_run()` → `fs.remove(..., dry_run=False)`
  - `move_in_repo()` / `move_in_repo_dry_run()` → `fs.move(..., dry_run=False)`
- All methods return `FS` with `.changes` set (dry-run variants previously returned `ChangeReport | None`)
- `fs.remove()` now accepts glob patterns, `recursive`, and `dry_run` (replaces the old single-file-only `fs.remove(path)`)
- `fs.export_tree()` renamed to `fs.export()`
- Standalone functions removed from `vost` and `vost.copy` public exports

**New features:**

- Add `glob` parameter (`bool`, default `True`) to `copy_in`, `copy_out`, `remove`, and `move` — when `False`, source paths are treated as literal (no `*`/`?` expansion)
- `copy_in`, `copy_out`, `remove`, and `move` now accept `str | list[str]` for sources (bare string auto-wrapped)

## v0.50.1 (2026-02-12)

**Bug fixes:**

- Fix single-file `cp` disk→repo silently dereferencing symlinks — now preserves symlinks unless `--follow-symlinks` is set
- Fix single-file `cp` repo→disk failing with `FileExistsError` when destination already exists (symlink or regular file)
- Fix `copy_from_repo(..., ignore_errors=True)` not raising `RuntimeError` when all write-phase operations fail, despite the documented contract
- Fix `undo()` / `redo()` accepting 0 and negative steps, which produced no-op reflog mutations

## v0.50.0 (2026-02-12)

**Breaking changes:**

- Rename public API for clarity and consistency:
  - `FS.hash` → `FS.commit_hash`
  - `FS.report` → `FS.changes` (returns `ChangeReport | None`)
  - `FS.dump()` → `FS.export_tree()`
  - `FS.write_from()` → `FS.write_from_file()`
  - `CopyReport` → `ChangeReport`, `CopyAction` → `ChangeAction`, `CopyError` → `ChangeError`
  - `SyncDiff` → `MirrorDiff`
  - `remove_from_repo()` → `remove_in_repo()` (param `patterns` → `sources`)
  - `move_from_repo()` → `move_in_repo()`
- CLI is now an optional dependency — `pip install vost` installs the core library only (`dulwich`); `pip install vost[cli]` adds `click` and `watchfiles`

**New features:**

- Add `read_text()` and `write_text()` convenience methods to `FS` and `Batch`
- Add `WalkEntry` named tuple with `name`, `oid`, `filemode` fields and `file_type` property — returned by `FS.walk()` file entries
- Add `ObjectSizer` for efficient blob size queries without full object reads
- Add `FileType` enum (`BLOB`, `EXECUTABLE`, `LINK`, `TREE`) — unifies file type representation across the API
- Add `branch exists` and `tag exists` CLI subcommands

**Improvements:**

- Consolidate `branch` and `tag` CLI into single `set` commands (replaces `fork`/`set` split)
- `FileEntry.file_type` now uses `FileType` enum instead of single-character strings

**Documentation:**

- Rewrite `docs/api.md` to match current v0.50 API (all names, signatures, data types)
- Rewrite `README.md` — updated API examples, trimmed CLI section, added new features
- Update `docs/cli.md` install instructions for `vost[cli]`

## v0.49.1 (2026-02-12)

**Internal:**

- Extract `_resolve_same_branch()` helper — shared by `rm` and `mv` for cross-branch validation
- Extract `_copy_blob_to_batch()` helper — shared by `mv` and repo-to-repo `cp`
- Fix bug in `rm` cross-branch detection that compared against the default branch instead of tracking the first explicit ref

## v0.49.0 (2026-02-12)

**Features:**

- `mv` command — move/rename files within a branch in one atomic commit
  - POSIX mv semantics: single-file rename, directory rename, multi-source move into directory
  - Supports globs, `-R` for directories, `-n` dry run, `ref:path` syntax
  - Same-branch only — cross-branch moves are rejected with a clear error

## v0.48.0 (2026-02-12)

**Features:**

- `ref:path` syntax for cross-branch CLI operations — `main:file.txt`, `dev:data/`, `v1.0:config.json`
- Ancestor syntax `ref~N:path` to read from historical commits (e.g., `main~3:file.txt`)
- Repo-to-repo `cp` and `sync` — copy files between branches without touching disk
- Per-path ref resolution in `ls` and `cat` — list/read from multiple branches in one command
- `write` and `rm` accept explicit `ref:path` to target a specific branch
- `log` and `diff` accept positional `ref:path` target (e.g., `vost log main~3:config.json`)
- Snapshot filters (`--back`, `--before`, `--path`, `--match`) work with explicit `ref:path`
- Ref name validation — branch/tag names containing `:`, space, tab, or newline are rejected

**Documentation:**

- New `docs/paths.md` — comprehensive path syntax reference covering parsing rules, per-command behavior, flag interaction, writability, and cross-branch workflows

## v0.47.8 (2026-02-11)

**Improvements:**

- Sync operations now use `Batch sync:` commit message prefix instead of `Batch cp:`

## v0.47.7 (2026-02-11)

**Documentation:**

- Document `gc` subcommand in README and CLI reference

## v0.47.6 (2026-02-11)

**Security:**

- Fix XSS in `serve` HTML output — escape display text and percent-encode/attribute-escape href values

**Bug fixes:**

- Reject scp-style SSH URLs (`git@host:path`, `host:path`) in mirror operations with a clear error suggesting `ssh://` format
- Guard `redo()` against zero-SHA reflog entries at branch creation point
- `glob()` now returns sorted results (was unordered); docstring updated to match

**New features:**

- Add `gc` subcommand — runs `git gc` to prune unreachable objects (orphaned blobs, etc.); requires git on PATH

## v0.47.5 (2026-02-11)

**Improvements:**

- Add `Cache-Control: no-cache` and 304 Not Modified support — browsers always revalidate via ETag and skip re-downloading unchanged content

## v0.47.4 (2026-02-11)

**Improvements:**

- `serve` now resolves snapshots live on each request — branches, `--back`, `--before`, `--path`, and `--match` all track the moving branch tip instead of pinning at startup

## v0.47.3 (2026-02-11)

**Improvements:**

- Add `--version` flag to CLI
- Document `serve` command in `docs/cli.md` and `README.md`

## v0.47.2 (2026-02-11)

**New features:**

- `--cors` flag adds permissive CORS headers (`Access-Control-Allow-Origin: *`) to all responses
- `--no-cache` flag sends `Cache-Control: no-store` on every response
- `--base-path` mounts the server under a URL prefix (e.g. `/data`)
- `--open` opens the URL in the default browser on start
- `--quiet` / `-q` suppresses per-request log output

**Tests:**

- Add 17 tests: CORS middleware (6), no-cache middleware (5), base-path middleware (7), CLI help assertions for new flags (4 new checks)

## v0.47.1 (2026-02-11)

**Bug fixes:**

- Fix HTML directory links in single-ref mode — no longer prefixed with the ref name
- Serve JSON, XML, GeoJSON, YAML as text so browsers display inline instead of downloading
- Register `.geojson` extension with Python's `mimetypes` module

**Improvements:**

- Add `ETag` header (commit hash) to all 200 responses

**Tests:**

- Add 14 tests: link correctness (single-ref and multi-ref), ETag presence/correctness, MIME overrides for JSON/XML/GeoJSON

## v0.47.0 (2026-02-11)

**New features:**

- Add `vost serve` command — HTTP file server for repo contents using stdlib `wsgiref`
- Content negotiation: `Accept: application/json` returns JSON metadata, otherwise raw bytes with MIME types or HTML directory listings
- Default: single-ref mode on HEAD branch, with shared `--branch`/`--ref`/`--back`/`--before`/`--match`/`--path` snapshot options
- `--all` flag enables multi-ref mode exposing all branches and tags via `/<ref>/<path>`

**Tests:**

- Add 27 tests for `serve` WSGI app covering single-ref mode, multi-ref mode, content negotiation, 404s, symlinks, MIME types, and CLI registration

## v0.46.0 (2026-02-11)

**New features:**

- Add `--exclude PATTERN` option to `cp` and `sync` — gitignore-style pattern matching, repeatable (disk→repo only)
- Add `--exclude-from FILE` option to `cp` and `sync` — read exclude patterns from a file (disk→repo only)
- Add `--gitignore` flag to `sync` — auto-reads `.gitignore` files from source tree with nested directory scoping; `.gitignore` files themselves are excluded (disk→repo only)
- New `ExcludeFilter` class in public API (`vost.copy.ExcludeFilter`) using `dulwich.ignore.IgnoreFilter`

**Tests:**

- Add 30 tests: 17 unit tests for `ExcludeFilter` and `_walk_local_paths` integration, 13 CLI tests for exclude/gitignore options

**Docs:**

- Document `--exclude`, `--exclude-from`, `--gitignore` options in CLI reference
- Add "Exclude patterns" section explaining gitignore syntax

## v0.45.0 (2026-02-11)

**New features:**

- Add `--watch` flag to `sync` command — continuously monitors a local directory for filesystem changes and auto-syncs to repo after a debounce delay (default 2s); uses `watchfiles` (Rust-based FSEvents/inotify); install via `pip install vost[watch]`
- Add `--debounce` option to `sync --watch` — configurable debounce delay in milliseconds (minimum 100ms)

**Tests:**

- Add 16 tests for watch mode: unit tests for import fallback, summary formatting, sync cycles, error recovery; CLI validation tests for incompatible flag combos

**Docs:**

- Document `--watch` and `--debounce` options in CLI reference

## v0.44.1 (2026-02-11)

**Bug fixes:**

- Fix inflated update counts in commit messages — `_build_report_from_changes` now compares new blob OID and filemode against the existing entry, so unchanged files are excluded from the report (e.g. re-importing 7 files with 1 changed now says `~ b.txt` instead of `~7`)

## v0.44.0 (2026-02-11)

**Bug fixes:**

- Fix TOCTOU race in `undo()`/`redo()` — stale check + ref update now run atomically under a single `repo_lock`, matching `_commit_changes`
- Fix `dump()` O(n^2) performance — filemodes are now read from tree entries during the walk instead of re-traversing from root per file
- Fix hardcoded reflog committer identity — reflog entries now use the actual `author`/`email` configured via `GitStore.open()` instead of `gitstore <gitstore@localhost>`
- Remove misleading `skipped` counter from zip import path (was always 0)
- Remove redundant local imports in `fs.py` and `repo.py`

**Tests:**

- Add 16 CLI tests: `TestUndo`, `TestRedo`, `TestReflogCLI`, `TestSnapshotFilterCombined`

**Docs:**

- Document `gitserve` command in CLI reference

## v0.43.1 (2026-02-11)

**Docs:**

- Document `diff` command in README and CLI reference
- Add `diff` to snapshot filters appendix

## v0.43.0 (2026-02-11)

**New features:**

- Add `diff` CLI command — compare HEAD against a baseline snapshot with git-style `A`/`M`/`D` output; supports all snapshot options (`--ref`, `--back`, `--before`, `--path`, `--match`) and `--reverse` to swap direction

## v0.42.0 (2026-02-11)

**New features:**

- Add `--passthrough`/`-p` flag to `write` CLI command — tee mode that echoes stdin to stdout for pipeline use (`cmd | vost write log.txt -p | grep error`)
- Add `retry_write()` library function — writes a single file to a branch with automatic retry on concurrent modification (exponential backoff + jitter, 5 attempts by default)
- `write` command now uses two-stage open: reads stdin before fetching the branch FS, minimizing the staleness window for long-running pipes

**Docs:**

- Fix `--hash` → `--ref` throughout README (matching actual CLI option name)
- Fix `branch` docs: document `fork`, `set`, `default` subcommands (replacing outdated `create --from` syntax)
- Fix `tag` docs: document `fork`, `set`, `hash` subcommands (replacing outdated `create` syntax)
- Add `--back` to all command option tables and Snapshot Filters appendix
- Add `-R/--recursive` and `-n/--dry-run` to `rm` options table
- Add `-b/--branch` to `undo`/`redo` docs; add options table for `reflog`
- Document `retry_write()` in API reference
- Document `--no-create` for `restore` in README

## v0.41.1 (2026-02-11)

**Internal:**

- Factor out all repeated CLI options into shared decorators in `_helpers.py`: `_branch_option`, `_message_option`, `_dry_run_option`, `_checksum_option`, `_ignore_errors_option`, `_format_option`, `_archive_format_option`
- Switch `branch set`, `tag set`, and `branch hash` to use `_snapshot_options` (adds `--back` support for free)
- No user-facing behavior changes

## v0.41.0 (2026-02-11)

**New features:**

- Add `branch default` subcommand — show or set the repo's default branch (`vost branch default`, `vost branch default -b dev`)
- HEAD is now set at repo creation to match the initial branch, fixing `git clone` and tools that read HEAD
- All CLI `--branch/-b` and `--ref` options now default to the repo's HEAD branch instead of hardcoded "main"

**Internal:**

- Add `get_head_branch()` / `set_head_branch()` helpers to `_compat.py` Repository
- Add `_default_branch()` CLI helper for HEAD-based branch resolution
- Simplify `_fix_head` in `_serve.py` to use new `_compat` helpers

## v0.40.1 (2026-02-11)

- `FS.back()` now defaults to `n=1`, matching `undo()`

## v0.40.0 (2026-02-11)

**New features:**

- Add `--back N` option to all read-oriented CLI commands (`ls`, `cat`, `log`, `cp`, `sync`, `zip`, `tar`, `archive`) — walk back N commits from HEAD before reading
- Add `FS.back(n)` API method — return the FS at the nth ancestor commit

**Internal:**

- Add `_resolve_fs()` CLI helper — consolidates branch/ref + snapshot filter + `--back` resolution into a single call
- Refactor `branch hash --back` to use `FS.back()` instead of inline loop
- Drop deprecated `--at` option from `zip` and `tar` commands

## v0.39.1 (2026-02-10)

**Enhancements:**

- `cat` now accepts multiple paths and concatenates their output

## v0.39.0 (2026-02-10)

**Breaking changes:**

- `branch create` no longer accepts `--from` or snapshot filters — it only creates empty branches
- `tag create` renamed to `tag fork`; `--from` renamed to `--ref` (defaults to `main`)

**New features:**

- Add `branch fork NAME` — create a new branch from an existing ref (`--ref` defaults to `main`, `-f`/`--force` to overwrite)
- Add `branch set NAME --ref REF` — point a branch at an existing ref (creates or updates)
- Add `tag fork NAME` — create a new tag from an existing ref (`--ref` defaults to `main`)
- Add `tag set NAME --ref REF` — point a tag at an existing ref (creates or updates)

## v0.38.1 (2026-02-10)

**Performance:**

- Pass tree OIDs down `_iglob_walk` recursion — each directory is now read directly via `repo[oid]` instead of re-walking from root
- Avoid double directory reads for `**` + rest patterns (e.g. `**/*.py`)
- Drop sorting from CLI `ls` output

**Cleanup:**

- Remove dead `_ls_typed` method (no longer called after iglob refactor)

## v0.38.0 (2026-02-10)

**New features:**

- Add `fs.iglob(pattern)` — streaming generator that yields unique matches without sorting or materializing the full list

**Performance:**

- Convert internal `_glob_walk` (list-builder) to `_iglob_walk` (generator) — eliminates intermediate list allocations at every recursion level
- `glob()` no longer sorts results; use `sorted(fs.glob(...))` if order matters
- CLI `ls` uses `iglob()` for streaming dedup

## v0.37.1 (2026-02-10)

**Bug fixes:**

- Pivot + trailing slash on file now raises `NotADirectoryError` — `base/./file.txt/` no longer silently treats a file as `mode="file"`, matching non-pivot behavior
- Normalize path separators for pivot detection on Windows — `base\.\sub\file` is now found correctly without mangling `\\?\` extended-length paths or literal backslashes in POSIX repo entries

**Performance:**

- Avoid quadratic `is_dir` calls in `**` glob — new `_ls_typed()` method reads the tree once per directory instead of N separate `_entry_at_path` lookups

**Documentation:**

- Document that glob patterns after `/./` pivot are unsupported (matches rsync behavior)

**Tests:**

- Add 6 tests for pivot edge cases: trailing-slash error, backslash normalization, glob-after-pivot (both disk→repo and repo→disk)

## v0.37.0 (2026-02-10)

**New features:**

- Add `**` glob support — `fs.glob("**/*.py")` matches files at any depth, skipping dot-named entries (consistent with `*`)
- Add `/./` pivot for repo→disk copies — `cp :src/./lib/utils.py ./dest` → `dest/lib/utils.py`; mirrors the existing disk→repo pivot

**Cleanup:**

- Remove dead `_parse_repo_path` helper from CLI

**Tests:**

- Add 9 tests for `**` glob: all, extension, prefix, middle, no-dotfiles, no-duplicates, empty-repo, at-root, sorted
- Add 6 tests for repo-side `/./` pivot: directory, contents, file, leading-dot-slash, dry-run, not-found
- Add 2 CLI tests for repo-side pivot

## v0.36.0 (2026-02-10)

**New features:**

- Add `--tag` and `--force-tag` options to all write commands (`write`, `rm`, `cp`, `sync`, `unzip`, `untar`, `unarchive`) — create a tag at the resulting commit without a separate `tag create` step
- `--tag` on repo→disk `cp`/`sync` is rejected with a clear error

**Tests:**

- Add 9 tests for `--tag`/`--force-tag`: write, rm, cp, sync, unzip, duplicate error, force overwrite, cp/sync repo→disk rejection

## v0.35.0 (2026-02-10)

**New features:**

- Default to mtime-based change detection for `cp --delete` and `sync` — skips hashing files whose mtime predates the commit timestamp (like rsync)
- Add `-c`/`--checksum` flag to `cp` and `sync` for exact SHA-1 comparison when needed (backdated mtime, archive extraction, etc.)

**Tests:**

- Add 6 tests for mtime vs checksum mode: unchanged skip, new mtime detection, cp --delete, backdated mtime tradeoff, dry-run/real-run agreement

## v0.34.0 (2026-02-10)

**New features:**

- Add rsync-style `/./` pivot marker for `cp` source paths — controls which part of the source path is preserved at the destination
  - `cp /data/./logs/app :backup` → `backup/logs/app/...`
  - `cp /data/./logs/app/ :backup` → `backup/logs/...` (contents mode)
  - Leading `./` (e.g. `./mydir`) does not trigger pivot mode

**Documentation:**

- Rewrite `docs/api.md` and `docs/cli.md` in terse, scannable man-page style
- Fix stale `create="main"` in `docs/index.md`

**Tests:**

- Add 6 tests for `/./` pivot: directory, contents, file, leading-dot-slash, not-found, dry-run

## v0.33.0 (2026-02-09)

**Breaking changes:**

- Simplify `GitStore.open()` API: `create` is now a plain `bool` (default `True`), `branch` defaults to `"main"`
  - Old: `GitStore.open(path, create="main")` / `GitStore.open(path, create=True, branch="main")`
  - New: `GitStore.open(path)` (creates with "main" branch if missing, opens if exists)
  - `create=False` raises `FileNotFoundError` when the repo is missing
  - `branch=None` creates a bare repo with no branches
  - `open()` is now idempotent — no more `FileExistsError`

## v0.32.0 (2026-02-09)

**Documentation:**

- Expand `vost --help` with quick-start examples, grouped command reference, and usage tips

## v0.31.0 (2026-02-09)

**New features:**

- Add `branch hash NAME` command — prints the 40-char commit SHA for a branch, with `--back`, `--path`, `--match`, `--before` filters
- Add `tag hash NAME` command — prints the 40-char commit SHA for a tag

**Tests:**

- Add 7 tests for `branch hash` and `tag hash` commands

## v0.30.0 (2026-02-09)

**New features:**

- Add message template placeholders for `--message` / `-m` flag
  - `{default}` — full auto-generated message
  - `{add_count}`, `{update_count}`, `{delete_count}`, `{total_count}` — file counts
  - `{op}` — operation name (`cp`, `ar`, or empty)
  - Example: `--message "Deploy v2: {default}"` → `Deploy v2: Batch cp: +3 ~1`
  - Messages without `{` are returned as-is (backward compatible)
- Add `--message` long flag (previously `-m` only) to `cp`, `sync`, `rm`, `unzip`, `untar`, `unarchive`

**Breaking changes:**

- `tag create` now uses `--from` option instead of positional `FROM` argument (consistent with `branch create`)
- Rename `--hash` to `--ref` on all read commands (`cp`, `sync`, `ls`, `cat`, `log`, `archive`, `zip`, `tar`)

**Documentation:**

- Document message placeholders in CLI and API docs

**Tests:**

- Add 13 tests for `format_commit_message` placeholder substitution

## v0.29.0 (2026-02-09)

**Bug fixes:**

- Preserve executable bit (0o755) when extracting files from repo via `copy_from_repo` and `fs.dump`
- Fix stale-snapshot check bypass when `_commit_changes` produces an identical tree (no-op write on a moved branch now raises `StaleSnapshotError`)
- Add stale-snapshot check to `fs.undo()` and `fs.redo()` to prevent overwriting concurrent branch updates
- Fix `sync_to_repo` delete-file path: report now shows the actual file path instead of `""`
- Fix file-to-directory conflicts in non-delete `copy_from_repo` (clear blocking parent files before `mkdir`)
- Detect mode-only changes (e.g. exec-bit flip) in delete-mode sync/copy, both directions
- Fix symlink mode regression: symlinks already in sync no longer reported as false updates
- Fix `follow_symlinks=True` in delete-mode copy: hash file content instead of link target to avoid perpetual updates
- Add base guard to path-clearing in `_write_files_to_disk` to prevent deleting files above the destination root

**Documentation:**

- Fix README comment claiming `write_from` avoids loading files into memory (dulwich requires full data for SHA-1)
- Add docstring to `create_blob_fromdisk` documenting memory limitation

**Tests:**

- Add 6 tests: symlink in-sync (4), follow_symlinks delete-mode (2)

## v0.28.0 (2026-02-09)

**New features:**

- Add undo/redo functionality with reflog support
  - `fs.undo(steps=1)` - Move branch back N commits
  - `fs.redo(steps=1)` - Move branch forward using reflog
  - `repo.branches.reflog(name)` - Read branch movement history
  - CLI commands: `vost undo`, `vost redo`, `vost reflog`
  - Reflog supports text, JSON, and JSONL output formats
- Add `repo.branches.set(name, fs)` method to solve chained assignment footgun
  - Returns writable FS bound to the branch (unlike bracket assignment)
  - Avoids confusion where `fs2 = repo.branches['x'] = fs1` leaves fs2 bound to old branch
- Document old snapshot semantics: readable bookmarks that can reset/create branches but cannot write

**Tests:**
- Add 22 comprehensive tests for undo/redo/reflog including edge cases
- Add 5 tests for `branches.set()` method

## v0.27.0 (2026-02-09)

**Breaking API change:** `copy_to_repo()` and `sync_to_repo()` now return just `FS` instead of `tuple[FS, CopyReport | None]`. Access the report via `fs.report` property.

- Add `FileEntry` dataclass with `path`, `type` (B/E/L), and `src` (source location) tracking
- `CopyReport` now uses `list[FileEntry]` instead of `list[str]` for add/update/delete operations
- Centralize commit message generation with `+/-/~` notation and operation prefixes (Batch cp:, Batch ar:)
- Add `FS.report` property to access operation report without tuple unpacking
- Fix `fs.report` to match tuple return value (both now reference same object with source tracking)
- **API simplification:** `copy_to_repo()` and `sync_to_repo()` return `FS` only; report via `fs.report`
- Export `FileEntry` from `vost` package
- Update documentation for new API

## v0.26.2 (2026-02-09)

- Allow `--repo` option at both main group level and subcommand level for flexibility

## v0.26.1 (2026-02-09)

- Auto-create destination repository when backing up to non-existent local path
- Fix help text for `cp` and `sync` commands to clarify `--repo` requirement
- Add test for backup auto-create behavior

## v0.26.0 (2026-02-09)

- Extract shared `_glob_match` into `_glob.py` (deduplicate from `fs.py` and `copy.py`)
- Split `copy.py` (1093 lines) into `copy/` subpackage: `_types`, `_resolve`, `_io`, `_ops`
- Split `cli.py` (1361 lines) into `cli/` subpackage: `_helpers`, `_basic`, `_cp`, `_sync`, `_refs`, `_archive`, `_mirror`
- Zero public API changes; all backward-compatible imports preserved

## v0.25.0 (2026-02-09)

- Unify `CopyPlan` and `list[CopyError]` into `CopyReport` dataclass with `add`, `update`, `delete`, `errors`, and `warnings` fields
- All copy/sync functions now return `CopyReport | None` (`None` when nothing to report)
- Overlap collisions reported as warnings instead of errors (CLI exits 0 for warnings-only)
- Fix `sync_to_repo_dry_run` file-at-dest producing wrong plan path
- Fix `copy_from_repo` delete mode using wrong source for hash comparison on overlapping destinations
- Fix contents-mode (`"symlink_dir/"`) silently producing zero pairs for symlinked directories
- Fix `copy_from_repo_dry_run` delete mode not deduplicating overlapping sources
- Update `docs/api.md` for new `CopyReport` API
- Backward-compatible aliases: `CopyPlan = CopyReport`, `SyncPlan = CopyReport`

## v0.24.0 (2026-02-09)

- Add `docs/` directory with API and CLI reference documentation
- Fix stale pygit2 references in README

## v0.23.0 (2026-02-09)

- Add `sync` CLI command for syncing files between disk and repo
- Add `--path`, `--match`, and `--before` filters to `ls`, `cat`, `cp`, and `sync`
- Add `ignore_errors` option to copy/sync operations
- Factor out backup/restore into dedicated `mirror.py` module
- Remove standalone `sync.py`; update `cptree` references to `cp`

## v0.22.0 (2026-02-09)

- Add `sync` module with optimized content-hash-based file synchronization
- Enhance `cp` with directory targets, trailing-slash semantics, glob patterns, and `--dry-run`

## v0.21.0 (2026-02-09)

- Auto-create repositories on write commands (no separate `init` step needed)

## v0.20.0 (2026-02-09)

- Move backup/restore logic from CLI into the GitStore API

## v0.19.0 (2026-02-08)

- Version bump only (consolidation release after v0.18.0)

## v0.18.0 (2026-02-08)

- Add `backup` and `restore` CLI commands for pushing/pulling to remote repos
- Add HTTPS credential support for remote operations

## v0.17.0 (2026-02-08)

- Migrate git backend from pygit2 to dulwich via a compatibility layer (`_compat.py`)
- Skip no-op commits when the tree is unchanged

## v0.16.0 (2026-02-08)

- Add `write_symlink()` and `readlink()` to FS and Batch APIs

## v0.15.0 (2026-02-08)

- Add `archive` and `unarchive` CLI commands
- Fix bug where `unzip` silently skipped files

## v0.14.0 (2026-02-08)

- Handle symlinks in `cp` and `cptree`
- Harden zip/tar import against malformed archives
- Document `write_from` in FS reference

## v0.13.0 (2026-02-08)

- Add `write_from()` for writing disk files directly into the store
- Add eager blob creation in Batch for `write_from()`
- CLI now uses the batch API for disk writes with normalized error handling
- Document `--match`/`--before` for branch/tag create and `git gc` maintenance

## v0.12.0 (2026-02-08)

- Unify snapshot resolution: remove internal `_resolve_with_at`
- Add `--match` and `--before` options to `branch create` and `tag create`

## v0.11.0 (2026-02-08)

- Add `--before` date filter to `log`, `zip`, and `tar` commands

## v0.10.0 (2026-02-08)

- Add `tar` and `untar` CLI commands
- Rename `--at` to `--path` (keep `--at` as hidden alias)
- Add `--hash` option to read commands for content-addressable lookups
- Extract shared CLI helpers (`_normalize_at_path`, `_resolve_snapshot`, `_commit_writes`)

## v0.9.0 (2026-02-08)

- Make `:` prefix optional for `ls`, `cat`, `rm`, and `--at` arguments

## v0.8.0 (2026-02-08)

- Change CLI from positional `REPO` argument to `--repo`/`-r` option
- Add `message` parameter to `batch()` for custom commit messages

## v0.7.0 (2026-02-08)

- Add `message` and `mode` keyword arguments to `fs.write()`

## v0.6.0 (2026-02-08)

- Keep CLI as a core dependency (reverted experiment with optional `vost[cli]` extra)

## v0.5.0 (2026-02-08)

- `branch create` now supports empty branches and `--from` to fork from an existing ref

## v0.4.0 (2026-02-08)

- Add `zip` and `unzip` CLI commands; preserve file permissions in round-trips
- Add `--at` and `--match` filters to `log` command
- Peel annotated tags to commits; validate `--at` paths
- Harden `rm` semantics across FS, Batch, and CLI
- Make CLI quiet by default

## v0.3.0 (2026-02-07)

- Support multiple sources in `cp` command
- Make bare `:` destination mean repo root (keep original filename)
- Add `--format json/jsonl` to `log` command
- Add `--mode 644/755` flag to `cp` command
- Drop auto-generated commit messages from write commands
- Default `init` to create a `main` branch

## v0.2.0 (2026-02-07)

- Add CLI with `cp`, `cptree`, `ls`, `cat`, `rm`, `log`, `branch`, and `tag` commands
- Add Apache 2.0 license
- Add commit metadata properties and path-filtered log
- Harden CLI input handling and exception reporting

## v0.1.0 (2026-02-07)

- Initial implementation of gitstore: git-backed file store with FS, Batch, and GitStore APIs
- Stale-snapshot detection, tag safety, binary mode strings
- Cross-repo refs, locking, `close()`, batch finality, Windows path normalization
- `src/` package layout with comprehensive README and test suite
