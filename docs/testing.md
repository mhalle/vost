# Testing

vost has five implementations — Python (primary), TypeScript, Rust, C++, and Kotlin — with
shared test coverage, a cross-language interop suite, and Deno compatibility tests
for the TypeScript port.

## Quick reference

```bash
make test-py        # Python tests only
make test-ts        # TypeScript tests only (Node.js / vitest)
make test-rs        # Rust library tests only
make test-rs-cli    # Rust CLI tests (Python test suite against Rust binary)
make test-deno      # Deno compatibility tests for the TS port
make test-interop   # cross-language interop tests
make test-all       # all of the above
```

## Python tests

- **Framework:** pytest
- **Location:** `tests/test_*.py` (~38 files, ~1537 tests)
- **Run:** `uv run python -m pytest tests/ -v`
- **Run one file:** `uv run python -m pytest tests/test_sync.py -v`
- **Run one test:** `uv run python -m pytest tests/test_sync.py -k test_basic_sync -v`

Tests use temporary bare git repos created via fixtures. No external services
required.

## TypeScript tests

- **Framework:** vitest
- **Location:** `ts/tests/*.test.ts` (16 files, ~484 tests)
- **Run:** `cd ts && npm test`
- **Run one file:** `cd ts && npx vitest run tests/sync.test.ts`
- **Run one test:** `cd ts && npx vitest run tests/sync.test.ts -t "basic sync"`

Test helpers live in `ts/tests/helpers.ts` (`freshStore`, `toBytes`, `fromBytes`,
`rmTmpDir`). Each test creates a temporary bare repo via `freshStore()` and cleans
it up in `afterEach`.

## Deno compatibility tests

The TypeScript port is tested under Deno to verify runtime compatibility.
These tests import from the compiled `dist/` (the same artifact npm consumers
use) and exercise the full API surface.

- **Framework:** `Deno.test` + `@std/assert` (no npm test runner)
- **Location:** `ts/tests/deno_compat_test.ts` (33 tests)
- **Run:** `cd ts && npm run test:deno` (or `deno task test:deno`)
- **Prerequisite:** `cd ts && npm run build` (tests import compiled JS)

The test file uses the `_test.ts` suffix (Deno convention) rather than
`.test.ts` (vitest convention), so vitest ignores it automatically.

Deno permissions required: `--allow-read --allow-write --allow-env`. The
`--allow-env` flag is needed because `isomorphic-git`'s transitive dependency
`ignore` reads `process.env` at module load time.

### What the Deno tests cover

| Category | Tests | What's verified |
|----------|-------|-----------------|
| Store creation | 4 | open, custom author, no branch, reopen |
| Read operations | 5 | read/write bytes, text, errors, ls, exists |
| File introspection | 4 | fileType, size, stat, listdir |
| Walk | 1 | recursive directory traversal |
| Batch | 2 | write+commit, write+remove |
| Branches & tags | 3 | create/delete, iteration |
| History | 3 | log, message, time |
| Copy ref | 2 | branch-to-branch, dry run |
| Copy in/out | 1 | disk ↔ repo round-trip |
| Export | 1 | tree to disk |
| Notes | 1 | set/get/delete |
| Path utilities | 2 | normalizePath, validateRefName |
| Immutability | 1 | write returns new snapshot |
| Read-only | 1 | tag write rejection |
| FUSE-readiness | 2 | treeHash, partial reads |

## Rust tests

Rust has two levels of testing: library tests and CLI tests.

### Library tests

- **Framework:** `#[test]` / cargo test
- **Location:** `rs/tests/test_*.rs` (~20 files, ~600 tests)
- **Run:** `cd rs && cargo test`
- **Run one file:** `cd rs && cargo test --test test_copy`
- **Run one test:** `cd rs && cargo test copy_in_multi -- --nocapture`

These test the Rust library API directly (`GitStore`, `Fs`, `Batch`, etc.)
using temporary repos from the `tempfile` crate.

### CLI cross-port tests

The Rust CLI is tested by running the **same Python CLI test suite** against
the Rust binary. This is controlled by the `VOST_CLI` environment variable.

```bash
# Build the Rust CLI first
cd rs && cargo build --features cli

# Run the full Python CLI test suite against the Rust binary
VOST_CLI=rust uv run python -m pytest tests/test_cli.py tests/test_cli_refs.py \
    tests/test_cli_ls.py tests/test_cli_cp.py tests/test_cli_archive.py \
    tests/test_cmp_cli.py tests/test_auto_create.py tests/test_backup_restore.py \
    tests/test_rsync_compat.py -v

# Run a single test file
VOST_CLI=rust uv run python -m pytest tests/test_cli.py -v

# Run a single test
VOST_CLI=rust uv run python -m pytest tests/test_cli.py::TestInit -v
```

Or use the Makefile target:

```bash
make test-rs-cli
```

#### How it works

The mechanism is a **runner swap** controlled by `VOST_CLI=rust`:

1. `tests/conftest.py` checks `os.environ.get("VOST_CLI")`.
2. When set to `"rust"`, it imports `RustCliRunner` from `tests/rs_runner.py`
   instead of Click's `CliRunner`. The `runner` fixture returns a
   `RustCliRunner` instance.
3. `RustCliRunner.invoke(main, args, input=...)` ignores the `main` argument
   and shells out to the Rust binary at `rs/target/debug/vost` via
   `subprocess.run`.
4. The return value is a `RustResult` dataclass with the same interface as
   Click's `Result` (`.exit_code`, `.output`, `.output_bytes`).

Because both runners expose the same interface, **all CLI test code runs
unmodified** against either backend. No test duplication, no conditional
logic in the tests themselves.

#### Overriding the binary path

By default the runner uses `rs/target/debug/vost`. To test a different build
(e.g. release, or a binary installed elsewhere):

```bash
VOST_CLI=rust VOST_BINARY=/path/to/vost uv run python -m pytest tests/test_cli.py
```

#### What the CLI tests cover

The CLI test suite exercises every user-facing command through the same
entry points a real user would use:

| Test file | Commands tested |
|-----------|----------------|
| `test_cli.py` | init, destroy, gc, rm, write, log, sync, diff, undo, redo, reflog, checksum/mtime |
| `test_cli_refs.py` | branch (list/set/delete/exists/hash/current), tag, hash, ref resolution |
| `test_cli_ls.py` | ls (plain, recursive, long, glob, JSON/JSONL output) |
| `test_cli_cp.py` | cp (disk→repo, repo→disk, repo→repo, dry-run, delete, exclude, symlinks, ignore-errors) |
| `test_cli_archive.py` | zip, unzip, tar, untar, archive\_out, archive\_in |
| `test_cmp_cli.py` | cmp (repo vs repo, repo vs disk, disk vs disk) |
| `test_auto_create.py` | auto-creation of repos on write/cp/sync/archive\_in |
| `test_backup_restore.py` | backup, restore (local, bundle, ref rename) |
| `test_rsync_compat.py` | rsync-compatible --delete/--exclude behavior |

#### Writing CLI tests that work with both backends

Tests should:

- Use the `runner` fixture (not `CliRunner()` directly).
- Pass `main` as the first argument to `runner.invoke()` (the Rust runner
  ignores it, but the Python runner needs it).
- Only check `result.exit_code` and `result.output` (or `result.output_bytes`
  for binary data). Don't rely on Click-specific attributes like
  `result.exception`.
- Prefer `"text" in result.output` over exact string equality, since error
  messages may differ slightly between ports (e.g. capitalization).
- Import `main` from `vost.cli` at the top of the file (even when running
  in Rust mode, the import succeeds — it's just unused).

```python
from vost.cli import main  # always importable

class TestExample:
    def test_something(self, runner, initialized_repo):
        r = runner.invoke(main, ["ls", "--repo", initialized_repo])
        assert r.exit_code == 0
        assert "hello.txt" in r.output
```

## Interop tests

The interop suite (`interop/`) verifies that repos created by one language can be
read by the other:

1. Python writes repos → TypeScript reads them
2. TypeScript writes repos → Python reads them

Fixtures are defined in `interop/fixtures.json`. Run with `make test-interop` or
`bash interop/run.sh`.

## Test parity

The parity script compares Python and TypeScript test counts side-by-side:

```bash
bash scripts/test-parity.sh
```

Some Python modules have no TypeScript counterpart by design:

| Module | Reason |
|--------|--------|
| `auto_create` | CLI auto-create repo feature |
| `backup_restore` | requires local HTTP transport |
| `exclude` | `ExcludeFilter` not implemented in TS |
| `objsize` | dulwich-specific `ObjectSizer` |
| `ref_path` | CLI `ref:path` parsing |

## File naming convention

Python and TypeScript test files correspond by name:

| Python | TypeScript |
|--------|-----------|
| `tests/test_fs_read.py` | `ts/tests/fs-read.test.ts` |
| `tests/test_copy.py` | `ts/tests/copy.test.ts` |
| `tests/test_sync.py` | `ts/tests/sync.test.ts` |

The pattern is: `test_{module}.py` → `{module-with-hyphens}.test.ts`.

## Writing new tests

- **Python:** add `def test_*` methods inside test classes in the appropriate
  `tests/test_*.py` file.
- **TypeScript:** add `it('...')` calls inside `describe` blocks in the
  appropriate `ts/tests/*.test.ts` file. Use `freshStore()` from helpers for
  a clean repo, and `rmTmpDir()` in `afterEach` for cleanup.
- **Deno:** add `Deno.test('...')` calls in `ts/tests/deno_compat_test.ts`.
  Each test should create a temp dir via `makeTmpDir()`, clean up in a
  `finally` block via `cleanup()`, and import from `../dist/index.js`.
  Rebuild with `npm run build` before running.
- After adding tests to either side, run `bash scripts/test-parity.sh` to check
  coverage alignment.
