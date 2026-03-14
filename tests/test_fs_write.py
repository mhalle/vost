"""Tests for FS write operations."""

import stat

import pytest

from vost import GitStore, StaleSnapshotError, retry_write
from vost.copy._types import FileType


@pytest.fixture
def repo_fs(tmp_path):
    repo = GitStore.open(tmp_path / "test.git")
    fs = repo.branches["main"]
    return repo, fs


class TestWriteText:
    def test_write_text_roundtrip(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_text("hello.txt", "Hello!")
        assert fs2.read_text("hello.txt") == "Hello!"
        assert fs2.read("hello.txt") == b"Hello!"

    def test_write_text_encoding(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_text("latin.txt", "café", encoding="latin-1")
        assert fs2.read("latin.txt") == "café".encode("latin-1")

    def test_write_text_message(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_text("a.txt", "data", message="custom msg")
        assert fs2.message == "custom msg"


class TestWrite:
    def test_write_returns_new_fs(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"a")
        assert fs2.commit_hash != fs.commit_hash

    def test_old_fs_unchanged(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"a")
        assert not fs.exists("a.txt")
        assert fs2.exists("a.txt")

    def test_written_data_readable(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("data.bin", b"\x00\x01\x02")
        assert fs2.read("data.bin") == b"\x00\x01\x02"

    def test_nested_path_creates_dirs(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a/b/c.txt", b"deep")
        assert fs2.read("a/b/c.txt") == b"deep"
        assert fs2.exists("a/b")
        assert fs2.exists("a")

    def test_branch_advances(self, repo_fs):
        repo, fs = repo_fs
        fs2 = fs.write("a.txt", b"a")
        # Getting branch again should see latest commit
        latest = repo.branches["main"]
        assert latest.commit_hash == fs2.commit_hash

    def test_write_custom_message(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"a", message="custom msg")
        assert fs2.message == "custom msg"

    def test_write_with_mode(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("run.sh", b"#!/bin/sh", mode=FileType.EXECUTABLE)
        assert fs2.file_type("run.sh") == FileType.EXECUTABLE

    def test_write_on_tag_raises(self, tmp_path):
        repo = GitStore.open(tmp_path / "test.git")
        fs = repo.branches["main"]
        repo.tags["v1"] = fs
        tag_fs = repo.tags["v1"]
        with pytest.raises(PermissionError):
            tag_fs.write("x.txt", b"x")


class TestRemove:
    def test_remove(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"a")
        fs3 = fs2.remove("a.txt")
        assert not fs3.exists("a.txt")

    def test_remove_missing_raises(self, repo_fs):
        _, fs = repo_fs
        with pytest.raises(FileNotFoundError):
            fs.remove("nope.txt")

    def test_remove_on_tag_raises(self, tmp_path):
        repo = GitStore.open(tmp_path / "test.git")
        fs = repo.branches["main"].write("x.txt", b"x")
        repo.tags["v1"] = fs
        with pytest.raises(PermissionError):
            repo.tags["v1"].remove("x.txt")

    def test_remove_directory_raises(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("dir/file.txt", b"data")
        with pytest.raises(IsADirectoryError):
            fs2.remove("dir")


class TestLog:
    def test_filemode_only_change_detected(self, repo_fs):
        """log(at=path) should detect filemode-only changes (no content change)."""
        from vost.copy._types import FileType
        _, fs = repo_fs
        # Write a file with default mode (644)
        fs2 = fs.write("script.sh", b"#!/bin/sh\necho hi")
        # Re-write with same content but executable mode (755)
        fs3 = fs2.write(
            "script.sh", b"#!/bin/sh\necho hi",
            mode=FileType.EXECUTABLE, message="Make executable",
        )
        # log --at script.sh should see both commits (content write + mode change)
        entries = list(fs3.log(path="script.sh"))
        messages = [e.message for e in entries]
        assert "Make executable" in messages
        assert "+ script.sh" in messages


class TestStaleSnapshot:
    def test_stale_write_raises(self, repo_fs):
        _, fs = repo_fs
        # Advance the branch behind fs's back
        fs.write("first.txt", b"first")
        with pytest.raises(StaleSnapshotError):
            fs.write("second.txt", b"second")

    def test_stale_remove_raises(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"a")
        # fs2 is now stale because branch advanced past fs
        fs2.write("b.txt", b"b")
        with pytest.raises(StaleSnapshotError):
            fs2.remove("a.txt")

    def test_stale_batch_raises(self, repo_fs):
        _, fs = repo_fs
        # Advance the branch behind fs's back
        fs.write("first.txt", b"first")
        with pytest.raises(StaleSnapshotError):
            with fs.batch() as b:
                b.write("second.txt", b"second")


class TestWriteFrom:
    def test_write_from_basic(self, repo_fs, tmp_path):
        _, fs = repo_fs
        local = tmp_path / "data.bin"
        local.write_bytes(b"\x00\x01\x02\x03")
        fs2 = fs.write_from_file("data.bin", local)
        assert fs2.read("data.bin") == b"\x00\x01\x02\x03"

    def test_write_from_preserves_executable(self, repo_fs, tmp_path):
        _, fs = repo_fs
        local = tmp_path / "run.sh"
        local.write_bytes(b"#!/bin/sh\necho hi")
        local.chmod(local.stat().st_mode | stat.S_IXUSR)
        fs2 = fs.write_from_file("run.sh", local)
        assert fs2.file_type("run.sh") == FileType.EXECUTABLE

    def test_write_from_mode_override(self, repo_fs, tmp_path):
        _, fs = repo_fs
        local = tmp_path / "script.sh"
        local.write_bytes(b"#!/bin/sh")
        # File is NOT executable on disk, but we override
        fs2 = fs.write_from_file("script.sh", local, mode=FileType.EXECUTABLE)
        assert fs2.file_type("script.sh") == FileType.EXECUTABLE

    def test_write_from_custom_message(self, repo_fs, tmp_path):
        _, fs = repo_fs
        local = tmp_path / "file.txt"
        local.write_bytes(b"content")
        fs2 = fs.write_from_file("file.txt", local, message="Import file")
        assert fs2.message == "Import file"

    def test_write_from_on_tag_raises(self, tmp_path):
        repo = GitStore.open(tmp_path / "test.git")
        fs = repo.branches["main"]
        repo.tags["v1"] = fs
        tag_fs = repo.tags["v1"]
        local = tmp_path / "file.txt"
        local.write_bytes(b"data")
        with pytest.raises(PermissionError):
            tag_fs.write_from_file("file.txt", local)

    def test_write_from_missing_file(self, repo_fs):
        _, fs = repo_fs
        with pytest.raises(FileNotFoundError):
            fs.write_from_file("x.txt", "/nonexistent/path/file.txt")

    def test_write_from_directory_raises(self, repo_fs, tmp_path):
        _, fs = repo_fs
        with pytest.raises(IsADirectoryError):
            fs.write_from_file("x.txt", str(tmp_path))


class TestSymlink:
    def test_write_symlink_basic(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt")
        assert fs2.readlink("link.txt") == "target.txt"

    def test_write_symlink_filemode(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt")
        assert fs2.file_type("link.txt") == FileType.LINK

    def test_write_symlink_nested_target(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("shortcut", "a/b/c.txt")
        assert fs2.readlink("shortcut") == "a/b/c.txt"

    def test_write_symlink_custom_message(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt", message="add link")
        assert fs2.message == "add link"

    def test_write_symlink_default_message(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt")
        assert fs2.message == "+ link.txt (link)"

    def test_write_symlink_on_tag_raises(self, tmp_path):
        repo = GitStore.open(tmp_path / "test.git")
        fs = repo.branches["main"]
        repo.tags["v1"] = fs
        tag_fs = repo.tags["v1"]
        with pytest.raises(PermissionError):
            tag_fs.write_symlink("link.txt", "target.txt")

    def test_readlink_missing_raises(self, repo_fs):
        _, fs = repo_fs
        with pytest.raises(FileNotFoundError):
            fs.readlink("nonexistent")

    def test_readlink_on_regular_file_raises(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write("regular.txt", b"data")
        with pytest.raises(ValueError):
            fs2.readlink("regular.txt")

    def test_read_returns_symlink_target_bytes(self, repo_fs):
        """read() on a symlink returns the raw target as bytes."""
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt")
        assert fs2.read("link.txt") == b"target.txt"

    def test_remove_symlink(self, repo_fs):
        _, fs = repo_fs
        fs2 = fs.write_symlink("link.txt", "target.txt")
        fs3 = fs2.remove("link.txt")
        assert not fs3.exists("link.txt")


class TestNoOpCommit:
    def test_write_identical_content_no_new_commit(self, repo_fs):
        """Writing the same data to the same path should not create a new commit."""
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"hello")
        fs3 = fs2.write("a.txt", b"hello")
        assert fs3.commit_hash == fs2.commit_hash

    def test_write_identical_via_batch_no_new_commit(self, repo_fs):
        """Batch-writing identical content should not create a new commit."""
        _, fs = repo_fs
        fs2 = fs.write("a.txt", b"hello")
        with fs2.batch() as b:
            b.write("a.txt", b"hello")
        assert b.fs.commit_hash == fs2.commit_hash


class TestUndo:
    def test_undo_single_step(self, repo_fs):
        """Undo should go back 1 commit."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        fs_back = fs2.undo()
        assert fs_back.commit_hash == fs1.commit_hash
        assert fs_back.exists("a.txt")
        assert not fs_back.exists("b.txt")

    def test_undo_multiple_steps(self, repo_fs):
        """Undo should go back N commits."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs3 = fs2.write("c.txt", b"c")

        fs_back = fs3.undo(2)
        assert fs_back.commit_hash == fs1.commit_hash
        assert fs_back.exists("a.txt")
        assert not fs_back.exists("b.txt")
        assert not fs_back.exists("c.txt")

    def test_undo_updates_branch(self, repo_fs):
        """Undo should update the branch pointer."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        fs_back = fs2.undo()
        latest = repo.branches["main"]
        assert latest.commit_hash == fs_back.commit_hash

    def test_undo_zero_raises(self, repo_fs):
        """Undo with steps=0 should raise ValueError."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        with pytest.raises(ValueError, match="steps must be >= 1"):
            fs1.undo(0)

    def test_undo_negative_raises(self, repo_fs):
        """Undo with negative steps should raise ValueError."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        with pytest.raises(ValueError, match="steps must be >= 1"):
            fs1.undo(-3)

    def test_undo_too_many_raises(self, repo_fs):
        """Undo beyond history should raise ValueError."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        with pytest.raises(ValueError, match="Cannot undo 5 steps"):
            fs2.undo(5)

    def test_undo_on_tag_raises(self, repo_fs):
        """Undo on read-only snapshot should raise PermissionError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        repo.tags["v1"] = fs1

        tag_fs = repo.tags["v1"]
        with pytest.raises(PermissionError, match="read-only"):
            tag_fs.undo()


class TestRedo:
    def test_redo_after_undo(self, repo_fs):
        """Redo should go forward after undo."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        fs_back = fs2.undo()
        fs_forward = fs_back.redo()
        assert fs_forward.commit_hash == fs2.commit_hash
        assert fs_forward.exists("b.txt")

    def test_redo_multiple_steps(self, repo_fs):
        """Redo should go forward N reflog steps."""
        _, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs3 = fs2.write("c.txt", b"c")

        # Do two separate undo operations (creates 2 reflog entries)
        fs_back1 = fs3.undo()  # fs3 -> fs2
        fs_back2 = fs_back1.undo()  # fs2 -> fs1

        # Redo 2 steps should get us back to fs3
        fs_forward = fs_back2.redo(2)
        assert fs_forward.commit_hash == fs3.commit_hash
        assert fs_forward.exists("c.txt")

    def test_redo_updates_branch(self, repo_fs):
        """Redo should update the branch pointer."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        fs_back = fs2.undo()
        fs_forward = fs_back.redo()
        latest = repo.branches["main"]
        assert latest.commit_hash == fs_forward.commit_hash

    def test_redo_zero_raises(self, repo_fs):
        """Redo with steps=0 should raise ValueError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs_back = fs2.undo()
        with pytest.raises(ValueError, match="steps must be >= 1"):
            fs_back.redo(0)

    def test_redo_negative_raises(self, repo_fs):
        """Redo with negative steps should raise ValueError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs_back = fs2.undo()
        with pytest.raises(ValueError, match="steps must be >= 1"):
            fs_back.redo(-1)

    def test_redo_too_many_raises(self, repo_fs):
        """Redo beyond available history should raise ValueError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        # Try to redo when there's no previous undo
        # The reflog has only 1 entry (the commit), and we're at that commit
        # Going back 1 step means looking at old_sha of that entry (empty tree)
        # Going back 2 steps should fail
        with pytest.raises(ValueError, match="Cannot redo 2 steps"):
            fs1.redo(2)

    def test_redo_on_tag_raises(self, repo_fs):
        """Redo on read-only snapshot should raise PermissionError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs_back = fs2.undo()
        repo.tags["v1"] = fs_back

        tag_fs = repo.tags["v1"]
        with pytest.raises(PermissionError, match="read-only"):
            tag_fs.redo()


class TestReflog:
    def test_reflog_shows_entries(self, repo_fs):
        """Reflog should show all branch movements."""
        repo, fs = repo_fs
        fs.write("a.txt", b"a")
        fs2 = repo.branches["main"]
        fs2.write("b.txt", b"b")

        entries = repo.branches.reflog("main")
        assert len(entries) >= 2
        assert all(hasattr(e, "message") for e in entries)
        assert all(hasattr(e, "new_sha") for e in entries)
        assert all(hasattr(e, "old_sha") for e in entries)

    def test_reflog_includes_undo(self, repo_fs):
        """Reflog should include undo operations."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs2.undo()

        entries = repo.branches.reflog("main")
        assert len(entries) >= 3
        # Last entry should be the undo
        assert "undo:" in entries[-1].message

    def test_reflog_nonexistent_branch_raises(self, repo_fs):
        """Reflog on nonexistent branch should raise KeyError."""
        repo, _ = repo_fs
        with pytest.raises(KeyError):
            repo.branches.reflog("nonexistent")

    def test_reflog_on_tags_raises(self, repo_fs):
        """Reflog on tags should raise ValueError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        repo.tags["v1"] = fs1

        with pytest.raises(ValueError, match="Tags do not have reflog"):
            repo.tags.reflog("v1")


class TestUndoRedoEdgeCases:
    def test_divergent_history(self, repo_fs):
        """After undo + new commit, redo goes through reflog chronologically."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs3 = fs2.write("c.txt", b"c")

        # Undo then make new commit (divergent history)
        fs_back = fs3.undo(2)
        fs_new = fs_back.write("d.txt", b"d")

        # Redo goes back through reflog, not to orphaned commits
        fs_redo = fs_new.redo()
        assert fs_redo.commit_hash == fs_back.commit_hash
        assert not fs_redo.exists("d.txt")

    def test_redo_after_normal_commit(self, repo_fs):
        """Redo works after normal commits (not just undo)."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        # Redo without undo goes to previous reflog entry
        fs_redo = fs2.redo()
        assert fs_redo.commit_hash == fs1.commit_hash
        assert not fs_redo.exists("b.txt")

    def test_undo_redo_undo_sequence(self, repo_fs):
        """Complex sequence: undo → redo → undo works correctly."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs3 = fs2.write("c.txt", b"c")

        fs_undo1 = fs3.undo()
        fs_redo = fs_undo1.redo()
        assert fs_redo.commit_hash == fs3.commit_hash

        fs_undo2 = fs_redo.undo()
        assert fs_undo2.commit_hash == fs2.commit_hash

    def test_multiple_undos_then_multiple_redos(self, repo_fs):
        """Multiple individual undos followed by multiple redo steps."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs3 = fs2.write("c.txt", b"c")
        fs4 = fs3.write("d.txt", b"d")

        # Do 3 separate undos (creates 3 reflog entries)
        fs_b1 = fs4.undo()
        fs_b2 = fs_b1.undo()
        fs_b3 = fs_b2.undo()
        assert fs_b3.commit_hash == fs1.commit_hash

        # Redo 3 steps should get back to fs4
        fs_forward = fs_b3.redo(3)
        assert fs_forward.commit_hash == fs4.commit_hash
        assert fs_forward.exists("d.txt")

    def test_redo_at_initial_commit(self, repo_fs):
        """Redo from first commit goes to initialization."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        # Redo from first commit goes to empty tree
        fs_redo = fs1.redo()
        assert fs_redo.message == "Initialize main"
        assert fs_redo.ls() == []

    def test_redo_past_creation_raises(self, repo_fs):
        """Redo past branch creation (zero-SHA reflog entry) raises ValueError."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        # Create a new branch — its reflog starts with a ZERO_SHA old entry
        repo.branches.set("dev", fs1)
        dev_fs = repo.branches["dev"]
        dev_fs2 = dev_fs.write("b.txt", b"b")

        # redo(2) reaches the create-ref entry whose old_sha is ZERO_SHA
        with pytest.raises(ValueError, match="branch creation"):
            dev_fs2.redo(2)

    def test_reflog_chronological_order(self, repo_fs):
        """Reflog entries should be in chronological order."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")
        fs2.undo()

        entries = repo.branches.reflog("main")
        timestamps = [e.timestamp for e in entries]
        assert timestamps == sorted(timestamps), "Timestamps should increase"

    def test_undo_redo_with_batch(self, repo_fs):
        """Undo/redo should work with batch operations."""
        repo, fs = repo_fs

        with fs.batch("Batch 1") as b:
            b.write("a.txt", b"a")
            b.write("b.txt", b"b")

        with b.fs.batch("Batch 2") as b2:
            b2.write("c.txt", b"c")

        fs_before_undo = b2.fs
        fs_after_undo = fs_before_undo.undo()

        # Should have only files from batch 1
        assert sorted(fs_after_undo.ls()) == ["a.txt", "b.txt"]

        # Redo should restore batch 2
        fs_after_redo = fs_after_undo.redo()
        assert sorted(fs_after_redo.ls()) == ["a.txt", "b.txt", "c.txt"]

    def test_redo_with_stale_snapshot_raises(self, repo_fs):
        """Redo with stale snapshot should give helpful error."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        # fs is now stale (points to init commit, branch moved to fs1)
        with pytest.raises(StaleSnapshotError, match="advanced"):
            fs.redo()


class TestBranchesSet:
    def test_set_returns_writable_fs(self, repo_fs):
        """branches.set() should return writable FS bound to new branch."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        fs_new = repo.branches.set("feature", fs1)

        assert fs_new.ref_name == "feature"
        assert fs_new.commit_hash == fs1.commit_hash
        assert fs_new is not fs1  # New object
        assert fs_new.writable

    def test_set_creates_new_branch(self, repo_fs):
        """branches.set() should create branch if it doesn't exist."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        fs_feature = repo.branches.set("feature", fs1)

        assert "feature" in repo.branches
        assert repo.branches["feature"].commit_hash == fs1.commit_hash

    def test_set_updates_existing_branch(self, repo_fs):
        """branches.set() should update existing branch."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")
        fs2 = fs1.write("b.txt", b"b")

        repo.branches["feature"] = fs1
        fs_updated = repo.branches.set("feature", fs2)

        assert fs_updated.commit_hash == fs2.commit_hash
        assert fs_updated.ref_name == "feature"

    def test_set_with_readonly_snapshot(self, repo_fs):
        """branches.set() should accept read-only snapshots."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        repo.tags["v1"] = fs1
        tag_fs = repo.tags["v1"]

        # Should accept read-only tag snapshot
        fs_branch = repo.branches.set("from-tag", tag_fs)

        assert fs_branch.ref_name == "from-tag"
        assert fs_branch.writable
        assert fs_branch.commit_hash == tag_fs.commit_hash

    def test_set_result_is_writable(self, repo_fs):
        """FS returned by set() should be writable."""
        repo, fs = repo_fs
        fs1 = fs.write("a.txt", b"a")

        fs_branch = repo.branches.set("feature", fs1)
        fs2 = fs_branch.write("b.txt", b"b")

        # Should update the 'feature' branch
        assert fs2.ref_name == "feature"
        assert repo.branches["feature"].commit_hash == fs2.commit_hash


class TestRetryWrite:
    def test_retry_write_succeeds_on_first_try(self, repo_fs):
        """Basic happy path — no contention."""
        repo, fs = repo_fs
        new_fs = retry_write(repo, "main", "file.txt", b"hello")
        assert new_fs.read("file.txt") == b"hello"
        assert repo.branches["main"].commit_hash == new_fs.commit_hash

    def test_retry_write_retries_on_stale(self, repo_fs):
        """Concurrent modification should be retried transparently."""
        repo, fs = repo_fs
        # Write once so the branch has content
        fs.write("a.txt", b"a")

        # Simulate: first attempt hits stale, second succeeds
        call_count = 0
        orig_write = type(fs).write

        def patched_write(self, path, data, *, message=None, mode=None, parents=None):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise StaleSnapshotError("simulated stale")
            return orig_write(self, path, data, message=message, mode=mode, parents=parents)

        import unittest.mock
        with unittest.mock.patch.object(type(fs), 'write', patched_write):
            new_fs = retry_write(repo, "main", "retried.txt", b"data", retries=3)

        assert new_fs.read("retried.txt") == b"data"
        assert call_count == 2

    def test_retry_write_raises_after_exhaustion(self, repo_fs):
        """All retries fail → StaleSnapshotError propagates."""
        repo, _ = repo_fs

        import unittest.mock

        def always_stale(self, path, data, *, message=None, mode=None, parents=None):
            raise StaleSnapshotError("always stale")

        from vost.fs import FS
        with unittest.mock.patch.object(FS, 'write', always_stale):
            with pytest.raises(StaleSnapshotError):
                retry_write(repo, "main", "x.txt", b"x", retries=2)
