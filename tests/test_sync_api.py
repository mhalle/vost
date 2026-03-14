"""Tests for the GitStore backup/restore API (store.backup / store.restore)."""

import pytest
from dulwich.repo import Repo as DulwichRepo

from vost import GitStore, MirrorDiff, RefChange
from vost.mirror import _diff_refs


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def store(tmp_path):
    """A GitStore with a 'main' branch and one commit."""
    p = str(tmp_path / "src.git")
    s = GitStore.open(p)
    fs = s.branches["main"]
    with fs.batch(message="add hello") as b:
        b.write("hello.txt", b"hello world\n")
    return s


@pytest.fixture
def remote(tmp_path):
    """An empty bare dulwich repo suitable as a push/fetch target."""
    p = str(tmp_path / "remote.git")
    DulwichRepo.init_bare(p, mkdir=True)
    return p


def _get_refs(repo_path):
    """Return {ref_str: sha_str} excluding HEAD."""
    repo = DulwichRepo(repo_path)
    return {
        ref.decode(): sha.decode()
        for ref, sha in repo.get_refs().items()
        if ref != b"HEAD"
    }


# ---------------------------------------------------------------------------
# TestBackupAPI
# ---------------------------------------------------------------------------

class TestBackupAPI:
    def test_backup_returns_sync_diff(self, store, remote):
        diff = store.backup(remote)
        assert isinstance(diff, MirrorDiff)
        assert len(diff.add) > 0
        assert diff.total > 0

    def test_dry_run_does_not_modify_remote(self, store, remote):
        diff = store.backup(remote, dry_run=True)
        assert diff.total > 0
        # Remote should still be empty
        assert _get_refs(remote) == {}

    def test_backup_mirrors_refs(self, store, remote):
        store.backup(remote)
        local = _get_refs(store._repo.path.rstrip("/"))
        assert local == _get_refs(remote)

    def test_backup_then_in_sync(self, store, remote):
        store.backup(remote)
        diff = store.backup(remote, dry_run=True)
        assert diff.in_sync

    def test_backup_with_tag(self, store, remote):
        # Create a tag
        fs = store.branches["main"]
        store.tags["v1"] = fs
        store.backup(remote)
        remote_refs = _get_refs(remote)
        assert any("refs/tags/v1" in r for r in remote_refs)


# ---------------------------------------------------------------------------
# TestRestoreAPI
# ---------------------------------------------------------------------------

class TestRestoreAPI:
    def test_restore_returns_sync_diff(self, store, remote):
        store.backup(remote)
        # Modify local
        fs = store.branches["main"]
        with fs.batch(message="add new") as b:
            b.write("new.txt", b"new\n")
        diff = store.restore(remote)
        assert isinstance(diff, MirrorDiff)

    def test_dry_run_does_not_modify_local(self, store, remote):
        store.backup(remote)
        fs = store.branches["main"]
        with fs.batch(message="add new") as b:
            b.write("new.txt", b"new\n")
        refs_before = _get_refs(store._repo.path.rstrip("/"))
        diff = store.restore(remote, dry_run=True)
        assert diff.total > 0
        refs_after = _get_refs(store._repo.path.rstrip("/"))
        assert refs_after == refs_before

    def test_restore_reverts_changes(self, store, remote):
        store.backup(remote)
        original = _get_refs(store._repo.path.rstrip("/"))
        # Modify local
        fs = store.branches["main"]
        with fs.batch(message="add new") as b:
            b.write("new.txt", b"new\n")
        store.restore(remote)
        assert _get_refs(store._repo.path.rstrip("/")) == original


# ---------------------------------------------------------------------------
# TestMirrorDiffStructure
# ---------------------------------------------------------------------------

class TestMirrorDiffStructure:
    def test_empty_diff_is_in_sync(self):
        diff = MirrorDiff()
        assert diff.in_sync
        assert diff.total == 0

    def test_ref_change_fields(self):
        c = RefChange(ref="refs/heads/main", old_target="abc1234", new_target=None)
        assert c.ref == "refs/heads/main"
        assert c.old_target == "abc1234"
        assert c.new_target is None

    def test_total_counts_all_categories(self):
        diff = MirrorDiff(
            add=[RefChange(ref="a", new_target="1")],
            update=[RefChange(ref="b", old_target="3", new_target="2")],
            delete=[RefChange(ref="c", old_target="4"), RefChange(ref="d", old_target="5")],
        )
        assert diff.total == 4
        assert not diff.in_sync


class TestScpStyleUrl:
    def test_scp_style_with_user_raises(self, store):
        with pytest.raises(ValueError, match="scp-style URL not supported"):
            _diff_refs(store._repo._drepo,"git@github.com:org/repo.git", "push")

    def test_scp_style_without_user_raises(self, store):
        """host:path (no @) is also scp-style and must be rejected."""
        with pytest.raises(ValueError, match="scp-style URL not supported"):
            _diff_refs(store._repo._drepo,"github.com:org/repo.git", "push")

    def test_scp_style_suggests_ssh(self, store):
        with pytest.raises(ValueError, match="ssh:// format"):
            _diff_refs(store._repo._drepo,"git@github.com:org/repo.git", "pull")

    def test_ssh_url_not_rejected(self, store):
        """ssh:// URLs should not be caught by scp detection."""
        # Will fail at the network level, but must not be the scp guard
        try:
            _diff_refs(store._repo._drepo,"ssh://git@github.com/org/repo.git", "pull")
        except ValueError as exc:
            assert "scp-style" not in str(exc), f"scp guard fired on ssh:// URL: {exc}"
        except Exception:
            pass  # network error is expected

    def test_file_url_not_rejected(self, store, tmp_path):
        """file:// URLs should not be caught by scp detection."""
        target = str(tmp_path / "remote.git")
        # Should not raise ValueError — will auto-create for push
        _diff_refs(store._repo._drepo,f"file://{target}", "push")


# ---------------------------------------------------------------------------
# TestRefRenaming
# ---------------------------------------------------------------------------

class TestRefRenaming:
    """Tests for ref renaming in backup/restore/bundle operations."""

    def test_restore_with_rename(self, tmp_path):
        """Restore with ref renaming via dict."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.restore(str(tmp_path / "src.git"), refs={"main": "imported-main"})

        assert "imported-main" in dst.branches
        assert "main" not in dst.branches
        assert dst.branches["imported-main"].read("f.txt") == b"data"

    def test_backup_with_rename(self, tmp_path):
        """Backup with ref renaming via dict."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        dst_path = str(tmp_path / "dst.git")
        GitStore.open(dst_path, branch=None)
        src.backup(dst_path, refs={"main": "their-main"})

        dst = GitStore.open(dst_path, create=False)
        assert "their-main" in dst.branches
        assert "main" not in dst.branches

    def test_bundle_export_with_rename(self, tmp_path):
        """Bundle export renames refs in the bundle."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        bundle_path = str(tmp_path / "test.bundle")
        src.bundle_export(bundle_path, refs={"main": "renamed"})

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path)
        assert "renamed" in dst.branches
        assert "main" not in dst.branches
        assert dst.branches["renamed"].read("f.txt") == b"data"

    def test_bundle_import_with_rename(self, tmp_path):
        """Bundle import renames refs on import."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        bundle_path = str(tmp_path / "test.bundle")
        src.bundle_export(bundle_path)

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path, refs={"main": "local-main"})
        assert "local-main" in dst.branches
        assert "main" not in dst.branches
        assert dst.branches["local-main"].read("f.txt") == b"data"

    def test_refs_list_backward_compatible(self, tmp_path):
        """List form still works (no renaming)."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.restore(str(tmp_path / "src.git"), refs=["main"])
        assert "main" in dst.branches
        assert dst.branches["main"].read("f.txt") == b"data"

    def test_backup_list_backward_compatible(self, tmp_path):
        """Backup with list form still works."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        dst_path = str(tmp_path / "dst.git")
        GitStore.open(dst_path, branch=None)
        src.backup(dst_path, refs=["main"])

        dst = GitStore.open(dst_path, create=False)
        assert "main" in dst.branches

    def test_rename_multiple_refs(self, tmp_path):
        """Rename multiple refs at once."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data1")
        src.branches["dev"] = src.branches["main"]
        src.branches["dev"].write("g.txt", b"data2")

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.restore(str(tmp_path / "src.git"),
                     refs={"main": "their-main", "dev": "their-dev"})

        assert "their-main" in dst.branches
        assert "their-dev" in dst.branches
        assert "main" not in dst.branches
        assert "dev" not in dst.branches

    def test_backup_bundle_with_rename(self, tmp_path):
        """Backup to bundle with ref renaming."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        bundle_path = str(tmp_path / "out.bundle")
        src.backup(bundle_path, refs={"main": "exported"})

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.restore(bundle_path)
        assert "exported" in dst.branches
        assert "main" not in dst.branches

    def test_restore_bundle_with_rename(self, tmp_path):
        """Restore from bundle with ref renaming."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        bundle_path = str(tmp_path / "out.bundle")
        src.backup(bundle_path)

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.restore(bundle_path, refs={"main": "imported"})
        assert "imported" in dst.branches
        assert "main" not in dst.branches
        assert dst.branches["imported"].read("f.txt") == b"data"

    def test_bundle_export_squash(self, tmp_path):
        """Squashed bundle has parentless commits."""
        src = GitStore.open(str(tmp_path / "src.git"))
        fs = src.branches["main"]
        fs = fs.write("a.txt", b"first")
        fs = fs.write("b.txt", b"second")  # 3 commits total (init + 2)

        bundle_path = str(tmp_path / "squashed.bundle")
        src.bundle_export(bundle_path, squash=True)

        # Import into fresh repo
        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path)

        # Should have the data
        dst_fs = dst.branches["main"]
        assert dst_fs.read("a.txt") == b"first"
        assert dst_fs.read("b.txt") == b"second"

        # Should have only 1 commit (the squashed root)
        assert dst_fs.parent is None  # no parent = single commit

    def test_bundle_export_squash_preserves_tree(self, tmp_path):
        """Squashed commit has same tree hash as original."""
        src = GitStore.open(str(tmp_path / "src.git"))
        fs = src.branches["main"].write("data.txt", b"hello")

        original_tree = fs.tree_hash
        bundle_path = str(tmp_path / "sq.bundle")
        src.bundle_export(bundle_path, squash=True)

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path)
        assert dst.branches["main"].tree_hash == original_tree

    def test_bundle_export_squash_with_rename(self, tmp_path):
        """Squashed bundle with ref renaming."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        bundle_path = str(tmp_path / "sq.bundle")
        src.bundle_export(bundle_path, refs={"main": "renamed"}, squash=True)

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path)
        assert "renamed" in dst.branches
        assert "main" not in dst.branches
        assert dst.branches["renamed"].parent is None
        assert dst.branches["renamed"].read("f.txt") == b"data"

    def test_backup_squash(self, tmp_path):
        """backup with squash=True produces squashed bundle."""
        src = GitStore.open(str(tmp_path / "src.git"))
        fs = src.branches["main"].write("f.txt", b"data")
        fs.write("g.txt", b"more")

        bundle_path = str(tmp_path / "backup.bundle")
        src.backup(bundle_path, squash=True)

        dst = GitStore.open(str(tmp_path / "dst.git"), branch=None)
        dst.bundle_import(bundle_path)
        assert dst.branches["main"].parent is None

    def test_backup_squash_non_bundle_raises(self, tmp_path):
        """squash=True on non-bundle backup raises ValueError."""
        src = GitStore.open(str(tmp_path / "src.git"))
        src.branches["main"].write("f.txt", b"data")

        import pytest
        with pytest.raises(ValueError, match="squash is only supported for bundle"):
            src.backup(str(tmp_path / "remote.git"), squash=True)
