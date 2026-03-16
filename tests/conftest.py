"""Shared fixtures for vost tests."""

import os
import pytest

# ---------------------------------------------------------------------------
# CLI backend selection: VOST_CLI=rust → Rust binary, else Python/Click
# ---------------------------------------------------------------------------

_USE_RUST = os.environ.get("VOST_CLI", "").lower() == "rust"

if _USE_RUST:
    from tests.rs_runner import RustCliRunner as _Runner

    # A no-op sentinel so `runner.invoke(main, ...)` compiles — the
    # RustCliRunner ignores this argument entirely.
    def main(*_a, **_kw):
        raise RuntimeError("main() should not be called in Rust mode")
else:
    from click.testing import CliRunner as _Runner
    from vost.cli import main  # noqa: F811 — re-exported for test modules

from vost.repo import init_repository


@pytest.fixture
def bare_repo(tmp_path):
    """Create a bare repository."""
    repo_path = str(tmp_path / "test.git")
    return init_repository(repo_path, bare=True)


# ---------------------------------------------------------------------------
# CLI fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def runner():
    return _Runner(env={"VOST_REPO": ""})


@pytest.fixture
def repo_path(tmp_path):
    """Return a path to a not-yet-created repo."""
    return str(tmp_path / "test.git")


@pytest.fixture
def initialized_repo(tmp_path, runner):
    """Create a repo with a 'main' branch and return its path."""
    p = str(tmp_path / "test.git")
    result = runner.invoke(main, ["init", "--repo", p, "--branch", "main"])
    assert result.exit_code == 0, result.output
    return p


@pytest.fixture
def repo_with_files(tmp_path, runner):
    """Repo with hello.txt and data/data.bin on 'main'."""
    p = str(tmp_path / "test.git")
    r = runner.invoke(main, ["init", "--repo", p, "--branch", "main"])
    assert r.exit_code == 0, r.output

    hello = tmp_path / "hello.txt"
    hello.write_text("hello world\n")
    r = runner.invoke(main, ["cp", "--repo", p, str(hello), ":hello.txt"])
    assert r.exit_code == 0, r.output

    data_dir = tmp_path / "datadir"
    data_dir.mkdir()
    (data_dir / "data.bin").write_bytes(b"\x00\x01\x02")
    r = runner.invoke(main, ["cp", "--repo", p, str(data_dir) + "/", ":data"])
    assert r.exit_code == 0, r.output

    return p


@pytest.fixture
def repo_with_tree(tmp_path, runner):
    """Repo with a deeper tree for glob/recursive tests.

    Tree:
        readme.txt, setup.py, .hidden,
        src/main.py, src/util.py, src/sub/deep.txt,
        docs/guide.md, docs/api.md
    """
    p = str(tmp_path / "tree.git")
    r = runner.invoke(main, ["init", "--repo", p, "--branch", "main"])
    assert r.exit_code == 0, r.output

    # Create files on disk
    root = tmp_path / "treefiles"
    root.mkdir()
    (root / "readme.txt").write_text("readme")
    (root / "setup.py").write_text("setup")
    (root / ".hidden").write_text("hidden")

    src = root / "src"
    src.mkdir()
    (src / "main.py").write_text("main")
    (src / "util.py").write_text("util")
    sub = src / "sub"
    sub.mkdir()
    (sub / "deep.txt").write_text("deep")

    docs = root / "docs"
    docs.mkdir()
    (docs / "guide.md").write_text("guide")
    (docs / "api.md").write_text("api")

    # Copy entire tree into repo root (trailing / = contents mode)
    r = runner.invoke(main, ["cp", "--repo", p, str(root) + "/", ":"])
    assert r.exit_code == 0, r.output

    return p
