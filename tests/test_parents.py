"""Tests for advisory parent commits."""

import pytest
from vost import GitStore


@pytest.fixture
def store(tmp_path):
    return GitStore.open(str(tmp_path / "test.git"), branch="main")


def test_write_with_parents(store):
    """Extra parents are recorded in commit."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["other"] = fs_main
    fs_other = store.branches["other"].write("b.txt", b"b")

    fs_result = fs_main.write("c.txt", b"c", parents=[fs_other])
    # Check the commit has 2 parents
    commit = store._repo[fs_result._commit_oid]
    assert len(commit.parents) == 2
    assert commit.parents[0] == fs_main._commit_oid  # first parent = branch tip
    assert commit.parents[1] == fs_other._commit_oid  # second = advisory


def test_write_with_multiple_parents(store):
    """Multiple extra parents."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["b1"] = fs_main
    store.branches["b2"] = fs_main
    fs_b1 = store.branches["b1"].write("b1.txt", b"b1")
    fs_b2 = store.branches["b2"].write("b2.txt", b"b2")

    fs_result = fs_main.write("c.txt", b"c", parents=[fs_b1, fs_b2])
    commit = store._repo[fs_result._commit_oid]
    assert len(commit.parents) == 3


def test_parent_first_parent_lineage(store):
    """back() follows first-parent only."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["other"] = fs_main
    fs_other = store.branches["other"].write("b.txt", b"b")

    fs_result = fs_main.write("c.txt", b"c", parents=[fs_other])
    parent = fs_result.parent
    assert parent.commit_hash == fs_main.commit_hash


def test_batch_with_parents(store):
    """Batch.commit() with parents."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["other"] = fs_main
    fs_other = store.branches["other"].write("b.txt", b"b")

    with fs_main.batch(parents=[fs_other]) as b:
        b.write("c.txt", b"c")
        b.write("d.txt", b"d")

    commit = store._repo[b.fs._commit_oid]
    assert len(commit.parents) == 2


def test_apply_with_parents(store):
    """apply() with parents."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["other"] = fs_main
    fs_other = store.branches["other"].write("b.txt", b"b")

    fs_result = fs_main.apply({"c.txt": b"c"}, parents=[fs_other])
    commit = store._repo[fs_result._commit_oid]
    assert len(commit.parents) == 2


def test_no_parents_default(store):
    """Without parents, commits have exactly 1 parent."""
    fs = store.branches["main"].write("a.txt", b"a")
    commit = store._repo[fs._commit_oid]
    assert len(commit.parents) == 1


def test_write_text_with_parents(store):
    """write_text passes parents through."""
    fs_main = store.branches["main"].write("a.txt", b"a")
    store.branches["other"] = fs_main
    fs_other = store.branches["other"].write("b.txt", b"b")

    fs_result = fs_main.write_text("c.txt", "c", parents=[fs_other])
    commit = store._repo[fs_result._commit_oid]
    assert len(commit.parents) == 2
