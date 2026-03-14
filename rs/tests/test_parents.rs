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
