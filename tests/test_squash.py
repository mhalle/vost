"""Tests for FS.squash()."""
import pytest
from vost import GitStore


@pytest.fixture
def store(tmp_path):
    return GitStore.open(str(tmp_path / "test.git"), branch="main")


def test_squash_creates_root_commit(store):
    fs = store.branches["main"].write("a.txt", b"a")
    fs = fs.write("b.txt", b"b")  # 3 commits total

    squashed = fs.squash()
    assert squashed.read("a.txt") == b"a"
    assert squashed.read("b.txt") == b"b"
    assert squashed.parent is None  # root commit
    assert squashed.writable is False


def test_squash_preserves_tree_hash(store):
    fs = store.branches["main"].write("data.txt", b"hello")
    squashed = fs.squash()
    assert squashed.tree_hash == fs.tree_hash


def test_squash_with_parent(store):
    tip = store.branches["main"]
    fs = tip.write("a.txt", b"a")
    fs = fs.write("b.txt", b"b")

    squashed = fs.squash(parent=tip)
    assert squashed.parent is not None
    assert squashed.parent.commit_hash == tip.commit_hash
    assert squashed.read("b.txt") == b"b"


def test_squash_with_message(store):
    fs = store.branches["main"].write("a.txt", b"a")
    squashed = fs.squash(message="Custom squash message")
    assert squashed.message.startswith("Custom squash message")


def test_squash_default_message(store):
    fs = store.branches["main"].write("a.txt", b"a")
    squashed = fs.squash()
    assert "squash" in squashed.message.lower()


def test_squash_assign_to_branch(store):
    fs = store.branches["main"].write("a.txt", b"a")
    fs = fs.write("b.txt", b"b")

    squashed = fs.squash()
    store.branches["squashed"] = squashed

    result = store.branches["squashed"]
    assert result.read("a.txt") == b"a"
    assert result.parent is None


def test_squash_in_place(store):
    """Squash a branch in place."""
    fs = store.branches["main"].write("a.txt", b"a")
    fs = fs.write("b.txt", b"b")

    squashed = fs.squash()
    store.branches["main"] = squashed

    result = store.branches["main"]
    assert result.read("b.txt") == b"b"
    assert result.parent is None
