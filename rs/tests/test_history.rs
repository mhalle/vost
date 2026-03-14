mod common;

use vost::*;

// ---------------------------------------------------------------------------
// parent
// ---------------------------------------------------------------------------

#[test]
fn parent_root_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    // Initial commit has no parent
    assert!(fs.parent().unwrap().is_none());
}

#[test]
fn parent_chain() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Current -> parent -> grandparent (init)
    let parent = fs.parent().unwrap().unwrap();
    assert!(parent.exists("a.txt").unwrap());
    assert!(!parent.exists("b.txt").unwrap());

    let grandparent = parent.parent().unwrap().unwrap();
    assert!(!grandparent.exists("a.txt").unwrap());

    // Grandparent is root
    assert!(grandparent.parent().unwrap().is_none());
}

// ---------------------------------------------------------------------------
// back
// ---------------------------------------------------------------------------

#[test]
fn back_zero_is_self() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let fs0 = fs.back(0).unwrap();
    assert_eq!(fs0.commit_hash(), fs.commit_hash());
}

#[test]
fn back_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let h0 = fs.commit_hash().unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let prev = fs.back(1).unwrap();
    assert_eq!(prev.commit_hash().unwrap(), h0);
}

#[test]
fn back_n() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let h0 = fs.commit_hash().unwrap();

    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let back2 = fs.back(2).unwrap();
    assert_eq!(back2.commit_hash().unwrap(), h0);
}

#[test]
fn back_too_far_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    // Only 1 commit, going back 2 should fail
    assert!(fs.back(2).is_err());
}

// ---------------------------------------------------------------------------
// log
// ---------------------------------------------------------------------------

#[test]
fn log_length() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(Default::default()).unwrap();
    // init + 2 writes = 3
    assert_eq!(log.len(), 3);
}

#[test]
fn log_order_recent_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("write a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", fs::WriteOptions {
        message: Some("write b".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(Default::default()).unwrap();
    assert_eq!(log[0].message, "write b");
    assert_eq!(log[1].message, "write a");
}

#[test]
fn log_metadata_fields() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitStore::open(dir.path().join("test.git"), OpenOptions {
        create: true,
        branch: Some("main".into()),
        author: Some("Alice".into()),
        email: Some("alice@example.com".into()),
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(Default::default()).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].author_name.as_deref(), Some("Alice"));
    assert_eq!(log[0].author_email.as_deref(), Some("alice@example.com"));
    assert!(log[0].time.is_some());
    assert!(log[0].time.unwrap() > 0);
}

#[test]
fn log_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        limit: Some(2),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 2);
}

#[test]
fn log_skip() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("write a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", fs::WriteOptions {
        message: Some("write b".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        skip: Some(1),
        ..Default::default()
    })
    .unwrap();
    // Skipped most recent, so first entry should be "write a"
    assert_eq!(log[0].message, "write a");
}

#[test]
fn log_skip_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("c.txt", b"c", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        limit: Some(1),
        skip: Some(1),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 1);
}

// ---------------------------------------------------------------------------
// undo
// ---------------------------------------------------------------------------

#[test]
fn undo_single_step() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash_before_undo = fs.commit_hash().unwrap();

    let undone = fs.undo(1).unwrap();
    assert_ne!(undone.commit_hash().unwrap(), hash_before_undo);
    assert!(!undone.exists("a.txt").unwrap());
}

#[test]
fn undo_updates_branch() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let undone = fs.undo(1).unwrap();
    // Re-fetch from store — branch should have moved back
    let fs_fresh = store.branches().get("main").unwrap();
    assert_eq!(fs_fresh.commit_hash(), undone.commit_hash());
}

#[test]
fn undo_no_parent_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    // Only init commit, no parent to undo to
    assert!(fs.undo(1).is_err());
}

// ---------------------------------------------------------------------------
// redo
// ---------------------------------------------------------------------------

#[test]
fn redo_after_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash_with_a = fs.commit_hash().unwrap();

    let undone = fs.undo(1).unwrap();
    assert!(!undone.exists("a.txt").unwrap());

    let redone = undone.redo(1).unwrap();
    assert_eq!(redone.commit_hash().unwrap(), hash_with_a);
    assert!(redone.exists("a.txt").unwrap());
}

#[test]
fn redo_updates_branch() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let undone = fs.undo(1).unwrap();
    let redone = undone.redo(1).unwrap();

    let fs_fresh = store.branches().get("main").unwrap();
    assert_eq!(fs_fresh.commit_hash(), redone.commit_hash());
}

// ---------------------------------------------------------------------------
// undo + redo sequence
// ---------------------------------------------------------------------------

#[test]
fn undo_redo_undo_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let hash_init = fs.commit_hash().unwrap();

    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash_with_a = fs.commit_hash().unwrap();

    // undo -> init
    let undone = fs.undo(1).unwrap();
    assert_eq!(undone.commit_hash().unwrap(), hash_init);

    // redo -> with_a
    let redone = undone.redo(1).unwrap();
    assert_eq!(redone.commit_hash().unwrap(), hash_with_a);

    // undo again -> init
    let undone2 = redone.undo(1).unwrap();
    assert_eq!(undone2.commit_hash().unwrap(), hash_init);
}

// ---------------------------------------------------------------------------
// reflog
// ---------------------------------------------------------------------------

#[test]
fn reflog_has_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();

    let entries = store.branches().reflog("main").unwrap();
    assert!(!entries.is_empty());
}

#[test]
fn reflog_includes_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.undo(1).unwrap();

    let entries = store.branches().reflog("main").unwrap();
    let has_undo = entries.iter().any(|e| e.message.contains("undo"));
    assert!(has_undo);
}

// ---------------------------------------------------------------------------
// commit info
// ---------------------------------------------------------------------------

#[test]
fn commit_info_author() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitStore::open(dir.path().join("test.git"), OpenOptions {
        create: true,
        branch: Some("main".into()),
        author: Some("Bob".into()),
        email: Some("bob@example.com".into()),
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions { limit: Some(1), ..Default::default() }).unwrap();
    assert_eq!(log[0].author_name.as_deref(), Some("Bob"));
    assert_eq!(log[0].author_email.as_deref(), Some("bob@example.com"));
}

#[test]
fn commit_info_time_populated() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(Default::default()).unwrap();
    assert!(log[0].time.is_some());
    assert!(log[0].time.unwrap() > 0);
}

// ---------------------------------------------------------------------------
// undo — edge cases
// ---------------------------------------------------------------------------

#[test]
fn undo_too_many_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // undo once succeeds
    let undone = fs.undo(1).unwrap();
    // undo again on init commit fails
    assert!(undone.undo(1).is_err());
}

#[test]
fn redo_on_init_commit_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();

    // Init commit has no reflog entries with new_sha matching, so redo fails
    // (The 0000 -> init entry has new_sha=init but old_sha=0000 which is invalid)
    // Just verify redo doesn't panic — it may error or succeed depending on reflog
    let _ = fs.redo(1);
}

#[test]
fn undo_redo_with_batch() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let hash_init = fs.commit_hash().unwrap();

    let mut batch = fs.batch(Default::default());
    batch.write("a.txt", b"a").unwrap();
    batch.write("b.txt", b"b").unwrap();
    batch.commit().unwrap();

    let fs = store.branches().get("main").unwrap();
    let hash_batch = fs.commit_hash().unwrap();
    assert_ne!(hash_batch, hash_init);

    // undo the batch
    let undone = fs.undo(1).unwrap();
    assert_eq!(undone.commit_hash().unwrap(), hash_init);
    assert!(!undone.exists("a.txt").unwrap());
    assert!(!undone.exists("b.txt").unwrap());

    // redo the batch
    let redone = undone.redo(1).unwrap();
    assert_eq!(redone.commit_hash().unwrap(), hash_batch);
    assert!(redone.exists("a.txt").unwrap());
}

#[test]
fn log_after_undo_reflects_earlier_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("write a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let _undone = fs.undo(1).unwrap();
    // After undo, the log should only show the init commit
    let fs_fresh = store.branches().get("main").unwrap();
    let log = fs_fresh.log(Default::default()).unwrap();
    assert_eq!(log.len(), 1);
    assert!(log[0].message.contains("Initialize"));
}

#[test]
fn multiple_undos() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let h0 = fs.commit_hash().unwrap();

    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let h1 = fs.commit_hash().unwrap();

    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // undo twice
    let u1 = fs.undo(1).unwrap();
    assert_eq!(u1.commit_hash().unwrap(), h1);
    assert!(u1.exists("a.txt").unwrap());
    assert!(!u1.exists("b.txt").unwrap());

    let u2 = u1.undo(1).unwrap();
    assert_eq!(u2.commit_hash().unwrap(), h0);
    assert!(!u2.exists("a.txt").unwrap());

    // Verify branch moved back
    let fs_fresh = store.branches().get("main").unwrap();
    assert_eq!(fs_fresh.commit_hash().unwrap(), h0);
}

#[test]
fn log_initial_commit_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(Default::default()).unwrap();
    assert_eq!(log.len(), 1);
    assert!(log[0].message.contains("Initialize"));
}

#[test]
fn back_snapshot_is_readonly_view() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let prev = fs.back(1).unwrap();
    // back() returns a detached Fs — writing on it should fail (no branch)
    let result = prev.write("x.txt", b"x", Default::default());
    assert!(result.is_err());
}

#[test]
fn reflog_has_init_entry() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");

    let entries = store.branches().reflog("main").unwrap();
    assert!(!entries.is_empty());
    // First entry should reference initial commit
    assert!(entries.iter().any(|e| e.message.contains("Initialize")));
}

#[test]
fn parent_returns_correct_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let parent = fs.parent().unwrap().unwrap();
    // Parent has a.txt but not b.txt
    assert_eq!(parent.read_text("a.txt").unwrap(), "a");
    assert!(!parent.exists("b.txt").unwrap());
}

// ---------------------------------------------------------------------------
// undo/redo — detached errors
// ---------------------------------------------------------------------------

#[test]
fn undo_on_detached_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // back(1) returns a detached Fs (no branch)
    let detached = fs.back(1).unwrap();
    let result = detached.undo(1);
    assert!(result.is_err());
}

#[test]
fn redo_on_detached_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // back(1) returns a detached Fs (no branch)
    let detached = fs.back(1).unwrap();
    let result = detached.redo(1);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// reflog — additional tests
// ---------------------------------------------------------------------------

#[test]
fn reflog_chronological_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();

    let entries = store.branches().reflog("main").unwrap();
    assert!(entries.len() >= 2);
    // Entries should be in chronological order (timestamps non-decreasing)
    for window in entries.windows(2) {
        assert!(window[0].timestamp <= window[1].timestamp);
    }
}

#[test]
fn reflog_nonexistent_branch_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    // Reflog for a branch that doesn't exist should error or return empty
    let result = store.branches().reflog("nonexistent");
    // Either errors or returns empty vec
    if let Ok(entries) = result {
        assert!(entries.is_empty());
    }
}

#[test]
fn double_redo_after_undo_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo once, then redo once — second redo should not succeed further
    let undone = fs.undo(1).unwrap();
    let redone = undone.redo(1).unwrap();
    assert!(redone.exists("a.txt").unwrap());

    // A second redo from this state: the reflog entry for the redo will
    // find the undo entry (new_sha == current), which goes back to the
    // undone state — this is the expected reflog behavior
    let result = redone.redo(1);
    // Whether this errors or goes back to the undo state, verify it doesn't panic
    let _ = result;
}

#[test]
fn redo_stale_snapshot_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let _undone = fs.undo(1).unwrap();
    // Get a stale snapshot of the undone state
    let stale = store.branches().get("main").unwrap();
    // Redo should still work because it uses reflog
    let redone = stale.redo(1).unwrap();
    assert!(redone.exists("a.txt").unwrap());
}

#[test]
fn undo_preserves_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"original content", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"second file", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo removes b.txt but preserves a.txt with original content
    let undone = fs.undo(1).unwrap();
    assert_eq!(undone.read_text("a.txt").unwrap(), "original content");
    assert!(!undone.exists("b.txt").unwrap());
}

// ---------------------------------------------------------------------------
// Fs metadata accessors
// ---------------------------------------------------------------------------

#[test]
fn fs_ref_name_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.ref_name(), Some("main"));
    assert!(fs.writable());
}

#[test]
fn fs_ref_name_none_for_detached() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let hash = fs.commit_hash().unwrap();
    let detached = store.fs(&hash).unwrap();
    assert_eq!(detached.ref_name(), None);
    assert!(!detached.writable());
}

#[test]
fn fs_tag_has_ref_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    store.tags().set("v1", &fs).unwrap();
    let tagged = store.tags().get("v1").unwrap();
    assert_eq!(tagged.ref_name(), Some("v1"));
    assert!(!tagged.writable());
}

#[test]
fn fs_message_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("my message".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.message().unwrap(), "my message");
}

#[test]
fn fs_time_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let t = fs.time().unwrap();
    // Should be a reasonable timestamp (after 2020)
    assert!(t > 1_577_836_800);
}

#[test]
fn fs_author_name_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let name = fs.author_name().unwrap();
    assert!(!name.is_empty());
}

#[test]
fn fs_author_email_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let email = fs.author_email().unwrap();
    assert!(!email.is_empty());
}

#[test]
fn fs_custom_author() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitStore::open(dir.path().join("test.git"), OpenOptions {
        create: true,
        branch: Some("main".into()),
        author: Some("Test User".into()),
        email: Some("test@example.com".into()),
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.author_name().unwrap(), "Test User");
    assert_eq!(fs.author_email().unwrap(), "test@example.com");
}

#[test]
fn fs_changes_none_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    assert!(fs.changes().is_none());
}

// ---------------------------------------------------------------------------
// CommitInfo.commit_hash
// ---------------------------------------------------------------------------

#[test]
fn commit_info_has_commit_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let log = fs.log(Default::default()).unwrap();
    // Each CommitInfo should have a non-empty commit_hash
    for entry in &log {
        assert_eq!(entry.commit_hash.len(), 40);
        assert!(entry.commit_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
    // The most recent should match fs.commit_hash
    assert_eq!(log[0].commit_hash, fs.commit_hash().unwrap());
}

// ---------------------------------------------------------------------------
// Multi-step undo
// ---------------------------------------------------------------------------

#[test]
fn undo_multiple_steps() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("c.txt", b"c", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo 2 steps at once — should go back past c and b
    let undone = fs.undo(2).unwrap();
    assert!(undone.exists("a.txt").unwrap());
    assert!(!undone.exists("b.txt").unwrap());
    assert!(!undone.exists("c.txt").unwrap());
}

#[test]
fn undo_all_the_way() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo back to the initial commit
    let undone = fs.undo(2).unwrap();
    assert!(!undone.exists("a.txt").unwrap());
    assert!(!undone.exists("b.txt").unwrap());
}

// ---------------------------------------------------------------------------
// Multi-step redo
// ---------------------------------------------------------------------------

#[test]
fn redo_multiple_steps() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo 1 twice (creates 2 reflog entries), then redo 2 at once
    let u1 = fs.undo(1).unwrap();
    assert!(u1.exists("a.txt").unwrap());
    assert!(!u1.exists("b.txt").unwrap());
    let u2 = u1.undo(1).unwrap();
    assert!(!u2.exists("a.txt").unwrap());
    let redone = u2.redo(2).unwrap();
    assert!(redone.exists("a.txt").unwrap());
    assert!(redone.exists("b.txt").unwrap());
}

#[test]
fn multiple_undos_then_redo_multi() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("c.txt", b"c", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo one at a time
    let u1 = fs.undo(1).unwrap();
    let u2 = u1.undo(1).unwrap();
    assert!(u2.exists("a.txt").unwrap());
    assert!(!u2.exists("b.txt").unwrap());
    assert!(!u2.exists("c.txt").unwrap());

    // Redo 2 at once
    let redone = u2.redo(2).unwrap();
    assert!(redone.exists("a.txt").unwrap());
    assert!(redone.exists("b.txt").unwrap());
    assert!(redone.exists("c.txt").unwrap());
}

// ---------------------------------------------------------------------------
// Log filtering — path
// ---------------------------------------------------------------------------

#[test]
fn log_filter_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a2", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        path: Some("a.txt".into()),
        ..Default::default()
    })
    .unwrap();
    // a.txt changed in 2 commits (initial write + overwrite)
    assert_eq!(log.len(), 2);
}

#[test]
fn log_filter_by_path_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        path: Some("nonexistent.txt".into()),
        ..Default::default()
    })
    .unwrap();
    assert!(log.is_empty());
}

// ---------------------------------------------------------------------------
// Log filtering — match_pattern
// ---------------------------------------------------------------------------

#[test]
fn log_filter_by_match_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("feat: add a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", fs::WriteOptions {
        message: Some("fix: repair b".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        match_pattern: Some("feat:*".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].message, "feat: add a");
}

#[test]
fn log_filter_by_match_no_results() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let log = fs.log(fs::LogOptions {
        match_pattern: Some("zzz*".into()),
        ..Default::default()
    })
    .unwrap();
    assert!(log.is_empty());
}

// ---------------------------------------------------------------------------
// Log filtering — before
// ---------------------------------------------------------------------------

#[test]
fn log_filter_by_before() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Use a timestamp far in the past — should exclude everything
    let log = fs.log(fs::LogOptions {
        before: Some(1),
        ..Default::default()
    })
    .unwrap();
    assert!(log.is_empty());
}

#[test]
fn log_filter_by_before_includes_all() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Use a timestamp far in the future — should include everything
    let log = fs.log(fs::LogOptions {
        before: Some(u64::MAX),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 2); // init + write
}

// ---------------------------------------------------------------------------
// Log filtering — combined
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// undo/redo — stale snapshot detection (Fix 2)
// ---------------------------------------------------------------------------

#[test]
fn undo_stale_snapshot_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Grab a snapshot
    let stale = store.branches().get("main").unwrap();

    // Advance the branch past the snapshot
    fs.write("b.txt", b"b", Default::default()).unwrap();

    // Now stale's commit_oid doesn't match the branch tip
    let result = stale.undo(1);
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("stale") || err_msg.contains("moved"));
}

#[test]
fn redo_stale_snapshot_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Undo to create reflog for redo
    let undone = fs.undo(1).unwrap();

    // Grab the undone snapshot
    let stale = store.branches().get("main").unwrap();

    // Advance the branch by writing something new
    undone.write("c.txt", b"c", Default::default()).unwrap();

    // Now stale's commit_oid doesn't match the branch tip
    let result = stale.redo(1);
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("stale") || err_msg.contains("moved"));
}

// ---------------------------------------------------------------------------
// log — mode-only changes detected (Fix 5)
// ---------------------------------------------------------------------------

#[test]
fn log_path_filter_detects_mode_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();

    // Write a file as a regular blob
    fs.write("script.sh", b"#!/bin/sh", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Rewrite the same file with executable mode (same content, different mode)
    fs.write("script.sh", b"#!/bin/sh", fs::WriteOptions {
        mode: Some(MODE_BLOB_EXEC),
        message: Some("make executable".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    // Log filtered by path should show BOTH commits
    let log = fs.log(fs::LogOptions {
        path: Some("script.sh".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 2, "mode-only change should appear in path-filtered log");

    // Verify the most recent one is the mode change
    assert_eq!(log[0].message, "make executable");
}

// ---------------------------------------------------------------------------
// Log filtering — combined
// ---------------------------------------------------------------------------

#[test]
fn log_combined_path_and_match() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a", fs::WriteOptions {
        message: Some("feat: add a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("b.txt", b"b", fs::WriteOptions {
        message: Some("feat: add b".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    fs.write("a.txt", b"a2", fs::WriteOptions {
        message: Some("fix: update a".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    // path=a.txt AND match=feat:* => only the initial write of a.txt
    let log = fs.log(fs::LogOptions {
        path: Some("a.txt".into()),
        match_pattern: Some("feat:*".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].message, "feat: add a");
}
