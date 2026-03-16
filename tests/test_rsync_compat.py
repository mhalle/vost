"""Tests validating rsync-compatible --delete/--exclude behavior.

Each test scenario is first verified against rsync (when available),
then the same behavior is expected from vost cp/sync.

Runs with both Python and Rust CLI backends:
    uv run python -m pytest tests/test_rsync_compat.py -v
    VOST_CLI=rust uv run python -m pytest tests/test_rsync_compat.py -v
"""

import os
import shutil
import subprocess

import pytest

from tests.conftest import main


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _has_rsync():
    """Return True if rsync is available on the system."""
    return shutil.which("rsync") is not None


def _rsync(args: list[str]):
    """Run rsync with the given arguments."""
    subprocess.run(["rsync"] + args, check=True)


def _repo_ls(runner, repo: str) -> set[str]:
    """Return the set of files in the repo (recursive ls)."""
    r = runner.invoke(main, ["ls", "--repo", repo, "-R"])
    assert r.exit_code == 0, r.output
    return {line.strip() for line in r.output.splitlines() if line.strip()}


def _setup_src(tmp_path):
    """Create a standard source directory with .py, .pyc, .log files."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("app code")
    (src / "app.pyc").write_text("compiled app")
    (src / "lib.py").write_text("lib code")
    (src / "data.log").write_text("log data")
    return src


def _init_repo(runner, repo_path: str) -> str:
    """Initialize a bare repo with a main branch."""
    r = runner.invoke(main, ["init", "--repo", repo_path, "--branch", "main"])
    assert r.exit_code == 0, r.output
    return repo_path


# ---------------------------------------------------------------------------
# Scenario 1: --delete without --exclude
# ---------------------------------------------------------------------------

class TestDeleteBasic:
    """--delete removes extra files from destination."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: extra dst files are deleted."""
        src = tmp_path / "src"
        dst = tmp_path / "dst"
        src.mkdir()
        dst.mkdir()
        (src / "a.txt").write_text("a")
        (src / "b.txt").write_text("b")

        # Initial sync
        _rsync(["-a", str(src) + "/", str(dst) + "/"])
        assert (dst / "a.txt").exists()

        # Add extra file to dst
        (dst / "extra.txt").write_text("extra")
        assert (dst / "extra.txt").exists()

        # rsync --delete should remove extra.txt
        _rsync(["-a", "--delete", str(src) + "/", str(dst) + "/"])
        assert (dst / "a.txt").exists()
        assert (dst / "b.txt").exists()
        assert not (dst / "extra.txt").exists()

    def test_vost_cp_delete(self, runner, tmp_path):
        """vost cp --delete removes files not in source."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = tmp_path / "src"
        src.mkdir()
        (src / "a.txt").write_text("a")
        (src / "b.txt").write_text("b")

        # Initial copy
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        # Write an extra file directly into the repo
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output
        assert "extra.txt" in _repo_ls(runner, repo)

        # cp --delete should remove extra.txt
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "a.txt" in files
        assert "b.txt" in files
        assert "extra.txt" not in files

    def test_vost_sync_delete(self, runner, tmp_path):
        """vost sync (implicit --delete) removes files not in source."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = tmp_path / "src"
        src.mkdir()
        (src / "a.txt").write_text("a")

        # Write extra file first
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output

        # Sync should delete extra.txt
        r = runner.invoke(main, [
            "sync", "--repo", repo,
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "a.txt" in files
        assert "extra.txt" not in files


# ---------------------------------------------------------------------------
# Scenario 2: --exclude without --delete
# ---------------------------------------------------------------------------

class TestExcludeBasic:
    """--exclude prevents files from being copied."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: excluded files not copied."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        _rsync(["-a", "--exclude", "*.pyc", str(src) + "/", str(dst) + "/"])
        assert (dst / "app.py").exists()
        assert (dst / "lib.py").exists()
        assert not (dst / "app.pyc").exists()
        assert (dst / "data.log").exists()

    def test_vost_cp_exclude(self, runner, tmp_path):
        """vost cp --exclude prevents excluded files from being copied."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        r = runner.invoke(main, [
            "cp", "--repo", repo, "--exclude", "*.pyc",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" not in files
        assert "data.log" in files


# ---------------------------------------------------------------------------
# Scenario 3: --delete with --exclude (key rsync behavior)
# ---------------------------------------------------------------------------

class TestDeleteWithExclude:
    """--delete combined with --exclude: excluded files are PRESERVED."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: --delete --exclude preserves excluded files."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        # Initial full sync (copies everything including .pyc)
        _rsync(["-a", str(src) + "/", str(dst) + "/"])
        assert (dst / "app.pyc").exists()

        # Now sync with --delete --exclude '*.pyc'
        _rsync(["-a", "--delete", "--exclude", "*.pyc",
                str(src) + "/", str(dst) + "/"])

        # app.pyc must be PRESERVED
        assert (dst / "app.py").exists()
        assert (dst / "lib.py").exists()
        assert (dst / "app.pyc").exists()  # preserved!
        assert (dst / "data.log").exists()

    def test_vost_cp_delete_exclude_preserves(self, runner, tmp_path):
        """vost cp --delete --exclude preserves excluded files in dest."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy (all files including .pyc)
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output
        assert "app.pyc" in _repo_ls(runner, repo)

        # cp --delete --exclude '*.pyc'
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete", "--exclude", "*.pyc",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "data.log" in files
        # Key assertion: app.pyc is PRESERVED (rsync behavior)
        assert "app.pyc" in files

    def test_vost_cp_delete_exclude_preserves_deep(self, runner, tmp_path):
        """Excluded files in subdirectories are also preserved."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = tmp_path / "src"
        src.mkdir()
        sub = src / "pkg"
        sub.mkdir()
        (sub / "mod.py").write_text("module")
        (sub / "mod.pyc").write_text("compiled")
        (src / "main.py").write_text("main")

        # Initial copy
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output
        files = _repo_ls(runner, repo)
        assert "pkg/mod.pyc" in files

        # cp --delete --exclude '*.pyc'
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete", "--exclude", "*.pyc",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "main.py" in files
        assert "pkg/mod.py" in files
        # Preserved in subdirectory too
        assert "pkg/mod.pyc" in files


# ---------------------------------------------------------------------------
# Scenario 4: --delete --exclude with extra unexcluded files deleted
# ---------------------------------------------------------------------------

class TestDeleteExcludeMixed:
    """--delete --exclude: extra (non-excluded) files deleted, excluded preserved."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: extra deleted, excluded preserved."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        # Initial full sync
        _rsync(["-a", str(src) + "/", str(dst) + "/"])
        # Add extra non-excluded file
        (dst / "extra.txt").write_text("extra")

        _rsync(["-a", "--delete", "--exclude", "*.pyc",
                str(src) + "/", str(dst) + "/"])

        assert (dst / "app.py").exists()
        assert (dst / "app.pyc").exists()  # preserved (excluded)
        assert not (dst / "extra.txt").exists()  # deleted (not excluded)

    def test_vost_cp_delete_exclude_mixed(self, runner, tmp_path):
        """Extra non-excluded files deleted, excluded files preserved."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy (all files)
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        # Add extra file to repo
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output
        assert "extra.txt" in _repo_ls(runner, repo)

        # cp --delete --exclude '*.pyc'
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete", "--exclude", "*.pyc",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" in files  # preserved (excluded)
        assert "extra.txt" not in files  # deleted (not excluded)


# ---------------------------------------------------------------------------
# Scenario 5: Multiple --exclude patterns
# ---------------------------------------------------------------------------

class TestMultipleExcludePatterns:
    """Multiple --exclude flags combined with --delete."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: multiple excludes all preserved."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        # Initial full sync
        _rsync(["-a", str(src) + "/", str(dst) + "/"])

        # Sync with multiple excludes
        _rsync(["-a", "--delete",
                "--exclude", "*.pyc", "--exclude", "*.log",
                str(src) + "/", str(dst) + "/"])

        assert (dst / "app.py").exists()
        assert (dst / "lib.py").exists()
        assert (dst / "app.pyc").exists()  # preserved
        assert (dst / "data.log").exists()  # preserved

    def test_vost_cp_multiple_excludes(self, runner, tmp_path):
        """Multiple --exclude patterns with --delete: all excluded preserved."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy (all files)
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        # cp --delete with multiple excludes
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete",
            "--exclude", "*.pyc", "--exclude", "*.log",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" in files  # preserved
        assert "data.log" in files  # preserved

    def test_vost_cp_multiple_excludes_no_delete(self, runner, tmp_path):
        """Multiple --exclude without --delete: excluded files not copied."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        r = runner.invoke(main, [
            "cp", "--repo", repo,
            "--exclude", "*.pyc", "--exclude", "*.log",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" not in files  # not copied
        assert "data.log" not in files  # not copied


# ---------------------------------------------------------------------------
# Scenario 6: --exclude-from file
# ---------------------------------------------------------------------------

class TestExcludeFrom:
    """--exclude-from file works with --delete."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """Verify with actual rsync: --exclude-from + --delete preserves."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        excludes_file = tmp_path / "excludes.txt"
        excludes_file.write_text("*.pyc\n*.log\n")

        # Initial full sync
        _rsync(["-a", str(src) + "/", str(dst) + "/"])

        _rsync(["-a", "--delete",
                "--exclude-from", str(excludes_file),
                str(src) + "/", str(dst) + "/"])

        assert (dst / "app.py").exists()
        assert (dst / "app.pyc").exists()  # preserved
        assert (dst / "data.log").exists()  # preserved

    def test_vost_cp_exclude_from_with_delete(self, runner, tmp_path):
        """vost cp --delete --exclude-from: excluded files preserved."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        excludes_file = tmp_path / "excludes.txt"
        excludes_file.write_text("*.pyc\n*.log\n")

        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete",
            "--exclude-from", str(excludes_file),
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" in files  # preserved
        assert "data.log" in files  # preserved


# ---------------------------------------------------------------------------
# Scenario 7: sync command (implicit --delete) with --exclude
# ---------------------------------------------------------------------------

class TestSyncDeleteExclude:
    """sync command (implicit --delete) with --exclude."""

    @pytest.mark.skipif(not _has_rsync(), reason="rsync not available")
    def test_rsync_reference(self, tmp_path):
        """rsync --delete --exclude: sync is equivalent."""
        src = _setup_src(tmp_path)
        dst = tmp_path / "dst"
        dst.mkdir()

        # Initial full sync
        _rsync(["-a", str(src) + "/", str(dst) + "/"])
        (dst / "extra.txt").write_text("extra")

        _rsync(["-a", "--delete", "--exclude", "*.pyc",
                str(src) + "/", str(dst) + "/"])

        assert (dst / "app.pyc").exists()  # preserved
        assert not (dst / "extra.txt").exists()  # deleted

    def test_vost_sync_with_exclude(self, runner, tmp_path):
        """vost sync --exclude: excluded files preserved, extras deleted."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy (all files)
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        # Add extra file
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output

        # sync with --exclude (sync = cp --delete)
        r = runner.invoke(main, [
            "sync", "--repo", repo, "--exclude", "*.pyc",
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "data.log" in files
        assert "app.pyc" in files  # preserved (excluded)
        assert "extra.txt" not in files  # deleted (not excluded)

    def test_vost_sync_exclude_into_subdir(self, runner, tmp_path):
        """sync --exclude into a subdirectory preserves excluded files."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial sync into subdir
        r = runner.invoke(main, [
            "sync", "--repo", repo,
            str(src), ":build",
        ])
        assert r.exit_code == 0, r.output
        assert "build/app.pyc" in _repo_ls(runner, repo)

        # Sync with --exclude
        r = runner.invoke(main, [
            "sync", "--repo", repo, "--exclude", "*.pyc",
            str(src), ":build",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "build/app.py" in files
        assert "build/app.pyc" in files  # preserved

    def test_vost_sync_multiple_excludes(self, runner, tmp_path):
        """sync with multiple --exclude patterns."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial sync
        r = runner.invoke(main, [
            "sync", "--repo", repo,
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        # Sync with multiple excludes
        r = runner.invoke(main, [
            "sync", "--repo", repo,
            "--exclude", "*.pyc", "--exclude", "*.log",
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        files = _repo_ls(runner, repo)
        assert "app.py" in files
        assert "lib.py" in files
        assert "app.pyc" in files  # preserved
        assert "data.log" in files  # preserved


# ---------------------------------------------------------------------------
# Dry-run validation
# ---------------------------------------------------------------------------

class TestDryRunDeleteExclude:
    """Dry-run correctly reports --delete + --exclude behavior."""

    def test_vost_cp_dry_run_delete_exclude(self, runner, tmp_path):
        """Dry-run with --delete --exclude: excluded files not in delete list."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial copy (all files)
        r = runner.invoke(main, ["cp", "--repo", repo, str(src) + "/", ":"])
        assert r.exit_code == 0, r.output

        # Add extra file
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output

        # Dry-run: --delete --exclude '*.pyc'
        r = runner.invoke(main, [
            "cp", "--repo", repo, "--delete", "--exclude", "*.pyc",
            "--dry-run",
            str(src) + "/", ":",
        ])
        assert r.exit_code == 0, r.output

        # extra.txt should be reported as delete, app.pyc should NOT be
        assert "extra.txt" in r.output
        assert "app.pyc" not in r.output

        # Verify repo is unchanged (dry-run)
        files = _repo_ls(runner, repo)
        assert "app.pyc" in files
        assert "extra.txt" in files

    def test_vost_sync_dry_run_exclude(self, runner, tmp_path):
        """Sync dry-run with --exclude: excluded files not in delete list."""
        repo = _init_repo(runner, str(tmp_path / "test.git"))
        src = _setup_src(tmp_path)

        # Initial sync
        r = runner.invoke(main, [
            "sync", "--repo", repo,
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        # Add extra
        extra = tmp_path / "extra.txt"
        extra.write_text("extra")
        r = runner.invoke(main, ["cp", "--repo", repo, str(extra), ":extra.txt"])
        assert r.exit_code == 0, r.output

        # Dry-run sync with --exclude
        r = runner.invoke(main, [
            "sync", "--repo", repo, "--exclude", "*.pyc",
            "--dry-run",
            str(src), ":",
        ])
        assert r.exit_code == 0, r.output

        # extra.txt deleted, app.pyc preserved
        assert "extra.txt" in r.output
        assert "app.pyc" not in r.output
