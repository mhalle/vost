"""Tests for the vost CLI — branch, tag, hash, and resolve-ref commands."""

import zipfile

import pytest
from click.testing import CliRunner

from vost.cli import main


class TestBranch:
    def test_list_default(self, runner, initialized_repo):
        result = runner.invoke(main, ["branch", "--repo", initialized_repo])
        assert result.exit_code == 0
        assert "main" in result.output

    def test_list_explicit(self, runner, initialized_repo):
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "list"])
        assert result.exit_code == 0
        assert "main" in result.output

    def test_delete(self, runner, initialized_repo):
        runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "todel", "--empty"])
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "delete", "todel"])
        assert result.exit_code == 0
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "list"])
        assert "todel" not in result.output

    def test_delete_missing(self, runner, initialized_repo):
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "delete", "ghost"])
        assert result.exit_code != 0
        assert "not found" in result.output.lower()

    def test_hash(self, runner, repo_with_files):
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        expected = store.branches["main"].commit_hash
        result = runner.invoke(main, ["branch", "--repo", repo_with_files, "hash", "main"])
        assert result.exit_code == 0
        out = result.output.strip()
        assert len(out) == 40
        assert out == expected

    def test_hash_back(self, runner, repo_with_files):
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        expected = store.branches["main"].parent.commit_hash
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "hash", "main", "--back", "1"
        ])
        assert result.exit_code == 0
        assert result.output.strip() == expected

    def test_hash_back_too_far(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "hash", "main", "--back", "100"
        ])
        assert result.exit_code != 0
        assert "history too short" in result.output.lower()

    def test_hash_path(self, runner, repo_with_files):
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["main"]
        expected = next(fs.log(path="hello.txt")).commit_hash
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "hash", "main", "--path", "hello.txt"
        ])
        assert result.exit_code == 0
        assert result.output.strip() == expected

    def test_hash_nonexistent(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "hash", "ghost"
        ])
        assert result.exit_code != 0
        assert "not found" in result.output.lower()


class TestBranchSet:
    # -- empty branch tests (formerly branch create) --

    def test_set_empty(self, runner, initialized_repo):
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "dev", "--empty"])
        assert result.exit_code == 0
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "list"])
        assert "dev" in result.output
        # Empty branch should have no files
        result = runner.invoke(main, ["ls", "--repo", initialized_repo, "-b", "dev"])
        assert result.exit_code == 0
        assert result.output.strip() == ""

    def test_set_empty_duplicate_error(self, runner, initialized_repo):
        runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "dup", "--empty"])
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "dup", "--empty"])
        assert result.exit_code != 0
        assert "already exists" in result.output.lower()

    def test_set_empty_force_overwrites(self, runner, initialized_repo):
        runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "dev", "--empty"])
        result = runner.invoke(main, ["branch", "--repo", initialized_repo, "set", "dev", "--empty", "-f"])
        assert result.exit_code == 0, result.output

    def test_set_empty_rejects_snapshot_options(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "set", "dev", "--empty", "--ref", "main"
        ])
        assert result.exit_code != 0
        assert "--empty cannot be combined" in result.output

    # -- fork tests (formerly branch fork) --

    def test_set_default_ref(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev"
        ])
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["branch", "--repo", repo_with_files, "list"])
        assert "dev" in result.output

    def test_set_explicit_ref(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev", "--ref", "main"
        ])
        assert result.exit_code == 0, result.output

    def test_set_from_tag(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "set", "from-tag", "--ref", "v1"
        ])
        assert result.exit_code == 0, result.output

    def test_set_has_content(self, runner, repo_with_files):
        runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev"
        ])
        result = runner.invoke(main, ["ls", "--repo", repo_with_files, "-b", "dev"])
        assert result.exit_code == 0
        assert "hello.txt" in result.output

    def test_set_duplicate_error(self, runner, repo_with_files):
        runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev"
        ])
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev"
        ])
        assert result.exit_code != 0
        assert "already exists" in result.output.lower()

    def test_set_force_overwrites(self, runner, repo_with_files):
        runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev"
        ])
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev", "-f"
        ])
        assert result.exit_code == 0, result.output

    def test_set_append_squash(self, runner, repo_with_files):
        """branch set --append --squash appends source tree onto branch tip."""
        from vost import GitStore
        # Create a feature branch with different content
        runner.invoke(main, ["branch", "--repo", repo_with_files, "set", "feature"])
        store = GitStore.open(repo_with_files, create=False)
        store.branches["feature"].write("new.txt", b"feature data")

        # Get main's current commit hash
        main_before = store.branches["main"].commit_hash

        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "main",
            "--append", "--ref", "feature"
        ])
        assert result.exit_code == 0, result.output

        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["main"]
        assert fs.read("new.txt") == b"feature data"
        assert fs.parent is not None
        assert fs.parent.commit_hash == main_before

    def test_set_append_requires_existing_branch(self, runner, repo_with_files):
        """--append on a nonexistent branch fails."""
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "nonexistent",
            "--append", "--ref", "main"
        ])
        assert result.exit_code != 0
        assert "existing branch" in result.output.lower()

    def test_set_squash(self, runner, repo_with_files):
        """branch set --squash creates a single-commit branch."""
        from vost import GitStore
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "squashed",
            "--ref", "main", "--squash"
        ])
        assert result.exit_code == 0, result.output
        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["squashed"]
        assert fs.read("hello.txt") == b"hello world\n"
        assert fs.parent is None  # squashed = root commit

    def test_set_squash_in_place(self, runner, repo_with_files):
        """branch set --squash -f squashes a branch in place."""
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "main",
            "--squash", "-f"
        ])
        assert result.exit_code == 0, result.output
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["main"]
        assert fs.read("hello.txt") == b"hello world\n"
        assert fs.parent is None

    def test_set_unknown_ref_error(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "set", "bad", "--ref", "nonexistent"
        ])
        assert result.exit_code != 0
        assert "Unknown ref" in result.output

    def test_set_path_filter(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "at-test",
            "--ref", "main", "--path", "hello.txt"
        ])
        assert result.exit_code == 0, result.output

    def test_set_path_with_default_ref(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "at-test",
            "--path", "hello.txt"
        ])
        assert result.exit_code == 0, result.output

    # -- old branch set tests (with --ref) --

    def test_set_existing_branch(self, runner, repo_with_files):
        # Create a branch, then set it to main
        runner.invoke(main, ["branch", "--repo", repo_with_files, "set", "dev", "--empty"])
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "dev", "--ref", "main", "-f"
        ])
        assert result.exit_code == 0, result.output
        # Now dev should have main's content
        result = runner.invoke(main, ["ls", "--repo", repo_with_files, "-b", "dev"])
        assert "hello.txt" in result.output

    def test_set_creates_new(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "newbranch", "--ref", "main"
        ])
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["branch", "--repo", repo_with_files, "list"])
        assert "newbranch" in result.output

    def test_set_from_tag_with_ref(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "set", "tagged", "--ref", "v1"
        ])
        assert result.exit_code == 0, result.output

    def test_set_path_filter_with_ref(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "branch", "--repo", repo_with_files, "set", "filtered",
            "--ref", "main", "--path", "hello.txt"
        ])
        assert result.exit_code == 0, result.output


class TestBranchExists:
    def test_exists_true(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "exists", "main"
        ])
        assert result.exit_code == 0

    def test_exists_false(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "branch", "--repo", initialized_repo, "exists", "ghost"
        ])
        assert result.exit_code == 1


class TestTag:
    def test_list(self, runner, initialized_repo):
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert result.exit_code == 0

    def test_create(self, runner, initialized_repo):
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        assert result.exit_code == 0
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "v1" in result.output

    def test_duplicate_error(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        assert result.exit_code != 0
        assert "already exists" in result.output.lower()

    def test_delete(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v2"])
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "delete", "v2"])
        assert result.exit_code == 0
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "v2" not in result.output

    def test_delete_missing(self, runner, initialized_repo):
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "delete", "ghost"])
        assert result.exit_code != 0
        assert "not found" in result.output.lower()

    def test_list_shows_all(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "alpha"])
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "beta"])
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "alpha" in result.output
        assert "beta" in result.output

    def test_at_flag(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "tag", "--repo", repo_with_files, "set", "v-at",
            "--path", "hello.txt"
        ])
        assert result.exit_code == 0

    def test_create_from_commit_hash(self, runner, repo_with_files):
        # Get commit hash from log
        result = runner.invoke(main, ["log", "--repo", repo_with_files])
        first_line = result.output.strip().split("\n")[0]
        short_hash = first_line.split()[0]

        # Get the full hash via the library
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["main"]
        full_hash = fs.commit_hash

        result = runner.invoke(main, [
            "tag", "--repo", repo_with_files, "set", "from-hash", "--ref", full_hash
        ])
        assert result.exit_code == 0

    def test_default_invocation_lists(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "t1"])
        result = runner.invoke(main, ["tag", "--repo", initialized_repo])
        assert "t1" in result.output

    def test_hash(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        from vost import GitStore
        store = GitStore.open(initialized_repo, create=False)
        expected = store.tags["v1"].commit_hash
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "hash", "v1"])
        assert result.exit_code == 0
        out = result.output.strip()
        assert len(out) == 40
        assert out == expected

    def test_hash_nonexistent(self, runner, initialized_repo):
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "hash", "ghost"])
        assert result.exit_code != 0
        assert "not found" in result.output.lower()


class TestTagSet:
    def test_set_default_ref(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "set", "v1"
        ])
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "v1" in result.output

    def test_set_creates_new(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "set", "v1", "--ref", "main"
        ])
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "v1" in result.output

    def test_set_duplicate_error(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "set", "v1"
        ])
        assert result.exit_code != 0
        assert "already exists" in result.output.lower()

    def test_set_force_overwrites(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "set", "v1", "-f"
        ])
        assert result.exit_code == 0, result.output

    def test_set_from_tag(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "set", "v2", "--ref", "v1"
        ])
        assert result.exit_code == 0, result.output

    def test_set_path_filter(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "tag", "--repo", repo_with_files, "set", "v1",
            "--ref", "main", "--path", "hello.txt"
        ])
        assert result.exit_code == 0, result.output


class TestTagExists:
    def test_exists_true(self, runner, initialized_repo):
        runner.invoke(main, ["tag", "--repo", initialized_repo, "set", "v1"])
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "exists", "v1"
        ])
        assert result.exit_code == 0

    def test_exists_false(self, runner, initialized_repo):
        result = runner.invoke(main, [
            "tag", "--repo", initialized_repo, "exists", "ghost"
        ])
        assert result.exit_code == 1


class TestTagOption:
    def test_write_tag(self, runner, initialized_repo):
        result = runner.invoke(
            main, ["write", "--repo", initialized_repo, ":hello.txt", "--tag", "v1"],
            input=b"hello",
        )
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "v1" in result.output

    def test_rm_tag(self, runner, repo_with_files):
        result = runner.invoke(
            main, ["rm", "--repo", repo_with_files, ":hello.txt", "--tag", "after-rm"],
        )
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", repo_with_files, "list"])
        assert "after-rm" in result.output

    def test_cp_tag(self, runner, initialized_repo, tmp_path):
        f = tmp_path / "file.txt"
        f.write_text("data")
        result = runner.invoke(
            main, ["cp", "--repo", initialized_repo, str(f), ":", "--tag", "cp-v1"],
        )
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "cp-v1" in result.output

    def test_unzip_tag(self, runner, initialized_repo, tmp_path):
        zpath = tmp_path / "test.zip"
        with zipfile.ZipFile(str(zpath), "w") as zf:
            zf.writestr("a.txt", "aaa")
        result = runner.invoke(
            main, ["unzip", "--repo", initialized_repo, str(zpath), "--tag", "zip-v1"],
        )
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "zip-v1" in result.output

    def test_duplicate_tag_error(self, runner, initialized_repo):
        runner.invoke(
            main, ["write", "--repo", initialized_repo, ":a.txt", "--tag", "dup"],
            input=b"one",
        )
        result = runner.invoke(
            main, ["write", "--repo", initialized_repo, ":b.txt", "--tag", "dup"],
            input=b"two",
        )
        assert result.exit_code != 0
        assert "already exists" in result.output.lower()

    def test_force_tag_overwrites(self, runner, initialized_repo):
        runner.invoke(
            main, ["write", "--repo", initialized_repo, ":a.txt", "--tag", "rel"],
            input=b"one",
        )
        result = runner.invoke(
            main, ["write", "--repo", initialized_repo, ":b.txt", "--tag", "rel", "--force-tag"],
            input=b"two",
        )
        assert result.exit_code == 0, result.output
        # Tag should point at the second commit
        from vost import GitStore
        store = GitStore.open(initialized_repo, create=False)
        fs = store.tags["rel"]
        assert fs.read("b.txt") == b"two"

    def test_sync_tag(self, runner, initialized_repo, tmp_path):
        d = tmp_path / "syncdir"
        d.mkdir()
        (d / "x.txt").write_text("x")
        result = runner.invoke(
            main, ["sync", "--repo", initialized_repo, str(d), ":", "--tag", "sync-v1"],
        )
        assert result.exit_code == 0, result.output
        result = runner.invoke(main, ["tag", "--repo", initialized_repo, "list"])
        assert "sync-v1" in result.output

    def test_cp_tag_rejected_repo_to_disk(self, runner, repo_with_files, tmp_path):
        result = runner.invoke(
            main, ["cp", "--repo", repo_with_files, ":hello.txt", str(tmp_path / "out.txt"), "--tag", "nope"],
        )
        assert result.exit_code != 0
        assert "only applies when writing to repo" in result.output.lower()

    def test_sync_tag_rejected_repo_to_disk(self, runner, repo_with_files, tmp_path):
        dest = tmp_path / "out"
        dest.mkdir()
        result = runner.invoke(
            main, ["sync", "--repo", repo_with_files, ":", str(dest), "--tag", "nope"],
        )
        assert result.exit_code != 0
        assert "only applies when writing to repo" in result.output.lower()


class TestResolveRef:
    def test_non_commit_hash_rejected(self, runner, repo_with_files):
        """Passing a tree/blob hash should produce a clear error."""
        from vost import GitStore
        store = GitStore.open(repo_with_files, create=False)
        fs = store.branches["main"]
        # Get the tree OID (not a commit)
        tree_oid = fs._tree_oid.decode()
        result = runner.invoke(main, [
            "tag", "--repo", repo_with_files, "set", "bad-ref", "--ref", tree_oid
        ])
        assert result.exit_code != 0
        assert "not a commit" in result.output.lower()


class TestHash:
    """Tests for the --ref option on read commands."""

    @staticmethod
    def _get_commit_hash(repo_path):
        """Get the full commit hash of HEAD on main."""
        from vost import GitStore
        store = GitStore.open(repo_path, create=False)
        fs = store.branches["main"]
        return fs.commit_hash

    @staticmethod
    def _get_parent_hash(repo_path):
        """Get the full commit hash of HEAD~1 on main."""
        from vost import GitStore
        store = GitStore.open(repo_path, create=False)
        fs = store.branches["main"]
        return fs.parent.commit_hash

    def test_cat_by_hash(self, runner, repo_with_files):
        commit_hash = self._get_commit_hash(repo_with_files)
        result = runner.invoke(main, [
            "cat", "--repo", repo_with_files, "hello.txt", "--ref", commit_hash
        ])
        assert result.exit_code == 0
        assert "hello world" in result.output

    def test_ls_by_hash(self, runner, repo_with_files):
        commit_hash = self._get_commit_hash(repo_with_files)
        result = runner.invoke(main, [
            "ls", "--repo", repo_with_files, "--ref", commit_hash
        ])
        assert result.exit_code == 0
        assert "hello.txt" in result.output
        assert "data" in result.output

    def test_cat_by_tag(self, runner, repo_with_files):
        # Create a tag first
        runner.invoke(main, ["tag", "--repo", repo_with_files, "set", "v1.0"])
        result = runner.invoke(main, [
            "cat", "--repo", repo_with_files, "hello.txt", "--ref", "v1.0"
        ])
        assert result.exit_code == 0
        assert "hello world" in result.output

    def test_cat_by_short_hash(self, runner, repo_with_files):
        commit_hash = self._get_commit_hash(repo_with_files)
        short = commit_hash[:7]
        result = runner.invoke(main, [
            "cat", "--repo", repo_with_files, "hello.txt", "--ref", short
        ])
        # pygit2 resolves short hashes, so this should succeed
        assert result.exit_code == 0
        assert "hello world" in result.output

    def test_cp_repo_to_disk_by_hash(self, runner, repo_with_files, tmp_path):
        commit_hash = self._get_commit_hash(repo_with_files)
        dest = tmp_path / "out.txt"
        result = runner.invoke(main, [
            "cp", "--repo", repo_with_files, ":hello.txt", str(dest),
            "--ref", commit_hash
        ])
        assert result.exit_code == 0
        assert dest.read_text() == "hello world\n"

    def test_cp_dir_repo_to_disk_by_hash(self, runner, repo_with_files, tmp_path):
        commit_hash = self._get_commit_hash(repo_with_files)
        dest = tmp_path / "export"
        dest.mkdir()
        result = runner.invoke(main, [
            "cp", "--repo", repo_with_files, ":data/", str(dest),
            "--ref", commit_hash
        ])
        assert result.exit_code == 0
        assert (dest / "data.bin").read_bytes() == b"\x00\x01\x02"

    def test_zip_by_hash(self, runner, repo_with_files, tmp_path):
        # Get hash of the commit that added hello.txt (parent of HEAD)
        parent_hash = self._get_parent_hash(repo_with_files)
        out = str(tmp_path / "archive.zip")
        result = runner.invoke(main, [
            "zip", "--repo", repo_with_files, out, "--ref", parent_hash
        ])
        assert result.exit_code == 0, result.output
        with zipfile.ZipFile(out, "r") as zf:
            names = zf.namelist()
            assert "hello.txt" in names
            # data/ tree was added after hello.txt, so shouldn't be here
            assert "data/data.bin" not in names

    def test_tar_by_hash(self, runner, repo_with_files, tmp_path):
        import tarfile
        parent_hash = self._get_parent_hash(repo_with_files)
        out = str(tmp_path / "archive.tar")
        result = runner.invoke(main, [
            "tar", "--repo", repo_with_files, out, "--ref", parent_hash
        ])
        assert result.exit_code == 0, result.output
        with tarfile.open(out, "r") as tf:
            names = tf.getnames()
            assert "hello.txt" in names
            assert "data/data.bin" not in names

    def test_log_by_hash(self, runner, repo_with_files):
        parent_hash = self._get_parent_hash(repo_with_files)
        result = runner.invoke(main, [
            "log", "--repo", repo_with_files, "--ref", parent_hash
        ])
        assert result.exit_code == 0
        lines = result.output.strip().split("\n")
        # Parent commit + init commit = at least 2, but NOT the latest commit
        assert len(lines) >= 2
        # The latest commit hash should not appear
        head_hash = self._get_commit_hash(repo_with_files)
        assert head_hash[:7] not in result.output

    def test_hash_overrides_branch(self, runner, repo_with_files):
        # Create a dev branch with different content
        runner.invoke(main, ["branch", "--repo", repo_with_files, "set", "dev"])
        commit_hash = self._get_commit_hash(repo_with_files)
        # Use --branch dev but --ref pointing to main's commit
        result = runner.invoke(main, [
            "cat", "--repo", repo_with_files, "hello.txt",
            "-b", "dev", "--ref", commit_hash
        ])
        assert result.exit_code == 0
        assert "hello world" in result.output

    def test_hash_invalid_ref(self, runner, repo_with_files):
        result = runner.invoke(main, [
            "cat", "--repo", repo_with_files, "hello.txt",
            "--ref", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        ])
        assert result.exit_code != 0
        assert "Unknown ref" in result.output

    def test_cp_disk_to_repo_with_hash_error(self, runner, repo_with_files, tmp_path):
        commit_hash = self._get_commit_hash(repo_with_files)
        f = tmp_path / "new.txt"
        f.write_text("data")
        result = runner.invoke(main, [
            "cp", "--repo", repo_with_files, str(f), ":new.txt",
            "--ref", commit_hash
        ])
        assert result.exit_code != 0
        assert "only apply when reading from repo" in result.output
