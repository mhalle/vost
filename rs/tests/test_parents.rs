mod common;

use vost::*;
use vost::fs;

/// Helper: open the bare repo and return the parent count for a commit hash.
fn parent_count(store: &GitStore, commit_hash: &str) -> usize {
    let repo = git2::Repository::open_bare(store.path()).unwrap();
    let oid = git2::Oid::from_str(commit_hash).unwrap();
    let commit = repo.find_commit(oid).unwrap();
    commit.parent_count()
}

/// Helper: return parent commit hashes for a given commit.
fn parent_hashes(store: &GitStore, commit_hash: &str) -> Vec<String> {
    let repo = git2::Repository::open_bare(store.path()).unwrap();
    let oid = git2::Oid::from_str(commit_hash).unwrap();
    let commit = repo.find_commit(oid).unwrap();
    (0..commit.parent_count())
        .map(|i| commit.parent_id(i).unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// write with parents
// ---------------------------------------------------------------------------

#[test]
fn write_with_parents_adds_extra_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();

    // Create a second branch by pointing it at main's tip, then writing to it.
    store.branches().set("other", &fs_main).unwrap();
    let fs_other = store.branches().get("other").unwrap();
    let fs_other = fs_other
        .write("other.txt", b"other", Default::default())
        .unwrap();

    // Write on main with other's tip as extra parent.
    let new_fs = fs_main
        .write(
            "hello.txt",
            b"hello",
            fs::WriteOptions {
                parents: vec![fs_other.clone()],
                ..Default::default()
            },
        )
        .unwrap();

    let hash = new_fs.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 2);

    // First parent is the old main tip, second is the other branch tip.
    let parents = parent_hashes(&store, &hash);
    assert_eq!(parents[0], fs_main.commit_hash().unwrap());
    assert_eq!(parents[1], fs_other.commit_hash().unwrap());
}

#[test]
fn write_without_parents_has_single_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();

    let new_fs = fs
        .write("hello.txt", b"hello", Default::default())
        .unwrap();

    let hash = new_fs.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 1);
}

// ---------------------------------------------------------------------------
// batch with parents
// ---------------------------------------------------------------------------

#[test]
fn batch_with_parents_adds_extra_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();

    // Create a second branch.
    store.branches().set("other", &fs_main).unwrap();
    let fs_other = store.branches().get("other").unwrap();
    let fs_other = fs_other
        .write("other.txt", b"other", Default::default())
        .unwrap();

    let mut batch = fs_main.batch(fs::BatchOptions {
        parents: vec![fs_other.clone()],
        ..Default::default()
    });
    batch.write("a.txt", b"aaa").unwrap();
    batch.write("b.txt", b"bbb").unwrap();
    let result = batch.commit().unwrap();

    let hash = result.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 2);

    let parents = parent_hashes(&store, &hash);
    assert_eq!(parents[1], fs_other.commit_hash().unwrap());
}

// ---------------------------------------------------------------------------
// apply with parents
// ---------------------------------------------------------------------------

#[test]
fn apply_with_parents_adds_extra_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();

    store.branches().set("other", &fs_main).unwrap();
    let fs_other = store.branches().get("other").unwrap();
    let fs_other = fs_other
        .write("other.txt", b"other", Default::default())
        .unwrap();

    let new_fs = fs_main
        .apply(
            &[("x.txt", WriteEntry::from_text("x"))],
            &[],
            fs::ApplyOptions {
                parents: vec![fs_other.clone()],
                ..Default::default()
            },
        )
        .unwrap();

    let hash = new_fs.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 2);
    assert_eq!(parent_hashes(&store, &hash)[1], fs_other.commit_hash().unwrap());
}

// ---------------------------------------------------------------------------
// first-parent lineage preserved
// ---------------------------------------------------------------------------

#[test]
fn first_parent_lineage_preserved_with_extra_parents() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();
    let init_hash = fs_main.commit_hash().unwrap();

    store.branches().set("other", &fs_main).unwrap();
    let fs_other = store.branches().get("other").unwrap();
    let fs_other = fs_other
        .write("other.txt", b"other", Default::default())
        .unwrap();

    let new_fs = fs_main
        .write(
            "hello.txt",
            b"hello",
            fs::WriteOptions {
                parents: vec![fs_other],
                ..Default::default()
            },
        )
        .unwrap();

    // parent() walks first-parent lineage and should return the init commit.
    let parent_fs = new_fs.parent().unwrap().unwrap();
    assert_eq!(parent_fs.commit_hash().unwrap(), init_hash);
}

// ---------------------------------------------------------------------------
// multiple extra parents
// ---------------------------------------------------------------------------

#[test]
fn write_with_multiple_extra_parents() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();

    store.branches().set("br2", &fs_main).unwrap();
    let fs2 = store.branches().get("br2").unwrap();
    let fs2 = fs2
        .write("f2.txt", b"two", Default::default())
        .unwrap();

    store.branches().set("br3", &fs_main).unwrap();
    let fs3 = store.branches().get("br3").unwrap();
    let fs3 = fs3
        .write("f3.txt", b"three", Default::default())
        .unwrap();

    let new_fs = fs_main
        .write(
            "hello.txt",
            b"hello",
            fs::WriteOptions {
                parents: vec![fs2.clone(), fs3.clone()],
                ..Default::default()
            },
        )
        .unwrap();

    let hash = new_fs.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 3);

    let parents = parent_hashes(&store, &hash);
    assert_eq!(parents[0], fs_main.commit_hash().unwrap());
    assert_eq!(parents[1], fs2.commit_hash().unwrap());
    assert_eq!(parents[2], fs3.commit_hash().unwrap());
}

// ---------------------------------------------------------------------------
// remove with parents
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// squash
// ---------------------------------------------------------------------------

#[test]
fn squash_creates_root_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let fs = fs
        .write("hello.txt", b"hello", Default::default())
        .unwrap();

    let squashed = fs.squash(None, None).unwrap();

    // Root commit has zero parents.
    let hash = squashed.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 0);

    // Tree is preserved.
    assert_eq!(squashed.tree_hash(), fs.tree_hash());

    // Content is readable.
    assert_eq!(squashed.read("hello.txt").unwrap(), b"hello");

    // Detached / read-only.
    assert!(!squashed.writable());
    assert!(squashed.ref_name().is_none());
}

#[test]
fn squash_preserves_tree_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let fs = fs
        .write("a.txt", b"aaa", Default::default())
        .unwrap();
    let fs = fs
        .write("b.txt", b"bbb", Default::default())
        .unwrap();

    let squashed = fs.squash(None, None).unwrap();
    assert_eq!(squashed.tree_hash(), fs.tree_hash());

    // Different commit hash (new commit object).
    assert_ne!(squashed.commit_hash(), fs.commit_hash());
}

#[test]
fn squash_with_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let fs = fs
        .write("hello.txt", b"hello", Default::default())
        .unwrap();

    // Create a parent commit via squash(None).
    let parent = fs.squash(None, None).unwrap();
    // Now squash again with that as parent.
    let child = fs.squash(Some(&parent), None).unwrap();

    let hash = child.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 1);

    let parents = parent_hashes(&store, &hash);
    assert_eq!(parents[0], parent.commit_hash().unwrap());
}

#[test]
fn squash_with_custom_message() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let fs = fs
        .write("hello.txt", b"hello", Default::default())
        .unwrap();

    let squashed = fs.squash(None, Some("custom squash message")).unwrap();
    let hash = squashed.commit_hash().unwrap();

    // Verify the message via git2.
    let repo = git2::Repository::open_bare(store.path()).unwrap();
    let oid = git2::Oid::from_str(&hash).unwrap();
    let commit = repo.find_commit(oid).unwrap();
    assert_eq!(commit.message().unwrap(), "custom squash message");
}

#[test]
fn squash_assign_to_branch() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let fs = fs
        .write("hello.txt", b"hello", Default::default())
        .unwrap();

    let squashed = fs.squash(None, None).unwrap();

    // Assign to a new branch.
    store.branches().set("squashed", &squashed).unwrap();
    let branch_fs = store.branches().get("squashed").unwrap();
    assert_eq!(branch_fs.tree_hash(), fs.tree_hash());
    assert_eq!(branch_fs.read("hello.txt").unwrap(), b"hello");
    assert_eq!(parent_count(&store, &branch_fs.commit_hash().unwrap()), 0);
}

// ---------------------------------------------------------------------------
// remove with parents
// ---------------------------------------------------------------------------

#[test]
fn remove_with_parents_adds_extra_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs_main = store.branches().get("main").unwrap();

    // Write a file so we have something to remove.
    let fs_main = fs_main
        .write("to_delete.txt", b"gone", Default::default())
        .unwrap();

    // Create a second branch with its own commit.
    store.branches().set("other", &fs_main).unwrap();
    let fs_other = store.branches().get("other").unwrap();
    let fs_other = fs_other
        .write("other.txt", b"other", Default::default())
        .unwrap();

    // Remove with other's tip as extra parent.
    let new_fs = fs_main
        .remove(
            &["to_delete.txt"],
            fs::RemoveOptions {
                parents: vec![fs_other.clone()],
                ..Default::default()
            },
        )
        .unwrap();

    // File should be gone.
    assert!(new_fs.read("to_delete.txt").is_err());

    // Commit should have two parents.
    let hash = new_fs.commit_hash().unwrap();
    assert_eq!(parent_count(&store, &hash), 2);

    let parents = parent_hashes(&store, &hash);
    assert_eq!(parents[0], fs_main.commit_hash().unwrap());
    assert_eq!(parents[1], fs_other.commit_hash().unwrap());
}
