.PHONY: test-py test-ts test-rs test-rs-cli test-deno test-interop test-all

test-py:
	uv run python -m pytest tests/ -v

test-ts:
	cd ts && npm test

test-rs:
	cd rs && cargo test

test-rs-cli:
	cd rs && cargo build --features cli
	VOST_CLI=rust uv run python -m pytest tests/test_cli.py tests/test_cli_refs.py \
		tests/test_cli_ls.py tests/test_cli_cp.py tests/test_cli_archive.py \
		tests/test_cmp_cli.py tests/test_auto_create.py tests/test_backup_restore.py \
		tests/test_rsync_compat.py -v

test-deno:
	cd ts && npm run build && npm run test:deno

test-interop:
	bash interop/run.sh

test-all: test-py test-ts test-rs test-rs-cli test-deno test-interop
