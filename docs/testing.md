# Testing

vost has five implementations — Python (primary), TypeScript, Rust, C++, and Kotlin — with
shared test coverage, a cross-language interop suite, and Deno compatibility tests
for the TypeScript port.

## Quick reference

```bash
make test-py        # Python tests only
make test-ts        # TypeScript tests only (Node.js / vitest)
make test-rs        # Rust tests only
make test-deno      # Deno compatibility tests for the TS port
make test-interop   # cross-language interop tests
make test-all       # all of the above
```

## Python tests

- **Framework:** pytest
- **Location:** `tests/test_*.py` (38 files, ~1537 tests)
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
