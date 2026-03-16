mod common;

use vost::*;
use std::path::Path;

fn create_disk_files(dir: &Path) {
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("file1.txt"), b"one").unwrap();
    std::fs::write(dir.join("file2.txt"), b"two").unwrap();
    std::fs::write(dir.join("sub/deep.txt"), b"deep").unwrap();
}

/// Helper: convert a directory Path to a string with trailing `/` for copy_in contents mode.
fn dir_src(p: &Path) -> String {
    let mut s = p.to_str().unwrap().to_string();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

// ---------------------------------------------------------------------------
// copy_in
// ---------------------------------------------------------------------------

#[test]
fn copy_in_basic() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    let (report, fs) = fs.copy_in(&[&src_str], "", Default::default()).unwrap();
    assert!(report.total() > 0);

    assert_eq!(fs.read_text("file1.txt").unwrap(), "one");
    assert_eq!(fs.read_text("sub/deep.txt").unwrap(), "deep");
}

#[test]
fn copy_in_nested() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert!(fs.exists("sub/deep.txt").unwrap());
}

#[test]
fn copy_in_with_dest_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "imported", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("imported/file1.txt").unwrap(), "one");
}

#[test]
fn copy_in_include_filter() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        include: Some(vec!["*.txt".into()]),
        ..Default::default()
    })
    .unwrap();

    let fs = store.branches().get("main").unwrap();
    assert!(fs.exists("file1.txt").unwrap());
}

#[test]
fn copy_in_exclude_filter() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        exclude: Some(vec!["sub/*".into()]),
        ..Default::default()
    })
    .unwrap();

    let fs = store.branches().get("main").unwrap();
    assert!(fs.exists("file1.txt").unwrap());
    assert!(!fs.exists("sub/deep.txt").unwrap());
}

// ---------------------------------------------------------------------------
// copy_out
// ---------------------------------------------------------------------------

#[test]
fn copy_out_basic() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    let report = fs.copy_out(&[""], dest_str, Default::default()).unwrap();
    assert!(report.total() > 0);
    assert_eq!(std::fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello");
}

#[test]
fn copy_out_creates_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, Default::default()).unwrap();
    assert!(dest.join("dir").is_dir());
    assert_eq!(
        std::fs::read_to_string(dest.join("dir/a.txt")).unwrap(),
        "aaa"
    );
}

#[cfg(unix)]
#[test]
fn copy_out_preserves_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("run.sh", b"#!/bin/sh", fs::WriteOptions {
        mode: Some(MODE_BLOB_EXEC),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();
    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, Default::default()).unwrap();

    let meta = std::fs::metadata(dest.join("run.sh")).unwrap();
    assert!(meta.permissions().mode() & 0o111 != 0);
}

#[cfg(unix)]
#[test]
fn copy_out_preserves_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let mut batch = fs.batch(Default::default());
    batch.write("target.txt", b"data").unwrap();
    batch.write_symlink("link", "target.txt").unwrap();
    batch.commit().unwrap();
    let fs = store.branches().get("main").unwrap();

    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();
    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, Default::default()).unwrap();

    let link_target = std::fs::read_link(dest.join("link")).unwrap();
    assert_eq!(link_target.to_string_lossy(), "target.txt");
}

#[test]
fn copy_out_include_filter() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, fs::CopyOutOptions {
        include: Some(vec!["*.txt".into()]),
        ..Default::default()
    })
    .unwrap();

    assert!(dest.join("hello.txt").exists());
}

#[test]
fn copy_out_exclude_filter() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, fs::CopyOutOptions {
        exclude: Some(vec!["dir/*".into()]),
        ..Default::default()
    })
    .unwrap();

    assert!(dest.join("hello.txt").exists());
    assert!(!dest.join("dir/a.txt").exists());
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

#[test]
fn copy_out_root_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("exported");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&[""], dest_str, fs::CopyOutOptions::default()).unwrap();
    assert_eq!(std::fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello");
    assert_eq!(std::fs::read_to_string(dest.join("dir/a.txt")).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(dest.join("dir/b.txt")).unwrap(), "bbb");
}

// ---------------------------------------------------------------------------
// sync_in / sync_out
// ---------------------------------------------------------------------------

#[test]
fn sync_in_basic() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    let (report, fs) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    assert!(report.total() > 0);

    assert_eq!(fs.read_text("file1.txt").unwrap(), "one");
}

#[test]
fn sync_out_basic() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("synced");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    let report = fs.sync_out("", dest_str, Default::default()).unwrap();
    assert!(report.total() > 0);
    assert_eq!(std::fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello");
}

// ---------------------------------------------------------------------------
// remove (disk)
// ---------------------------------------------------------------------------

#[test]
fn remove_disk_files() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("to_remove");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("a.txt"), b"a").unwrap();
    std::fs::write(target.join("b.txt"), b"b").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let report = fs.remove_from_disk(&target, Default::default()).unwrap();
    assert!(report.total() > 0);
    assert!(!target.join("a.txt").exists());
    assert!(!target.join("b.txt").exists());
}

#[test]
fn remove_with_include_filter() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("to_remove");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("a.txt"), b"a").unwrap();
    std::fs::write(target.join("keep.md"), b"keep").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.remove_from_disk(&target, fs::RemoveFromDiskOptions {
        include: Some(vec!["*.txt".into()]),
        ..Default::default()
    })
    .unwrap();

    assert!(!target.join("a.txt").exists());
    assert!(target.join("keep.md").exists());
}

// ---------------------------------------------------------------------------
// rename
// ---------------------------------------------------------------------------

#[test]
fn rename_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let (store, fs) = common::store_with_files(dir.path());
    fs.rename("hello.txt", "goodbye.txt", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert!(!fs.exists("hello.txt").unwrap());
    assert_eq!(fs.read_text("goodbye.txt").unwrap(), "hello");
}

#[test]
fn rename_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (store, fs) = common::store_with_files(dir.path());
    fs.rename("dir", "moved", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert!(!fs.exists("dir").unwrap());
    assert_eq!(fs.read_text("moved/a.txt").unwrap(), "aaa");
}

#[test]
fn rename_missing_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    assert!(fs.rename("nope.txt", "dest.txt", Default::default()).is_err());
}

// ---------------------------------------------------------------------------
// copy_in — edge cases
// ---------------------------------------------------------------------------

#[test]
fn copy_in_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("empty.txt"), b"").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read("empty.txt").unwrap(), b"");
}

#[test]
fn copy_in_binary_data() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(&src).unwrap();
    let data: Vec<u8> = (0u8..=255).collect();
    std::fs::write(src.join("binary.bin"), &data).unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read("binary.bin").unwrap(), data);
}

#[test]
fn copy_in_deep_nesting() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(src.join("a/b/c/d")).unwrap();
    std::fs::write(src.join("a/b/c/d/deep.txt"), b"deep").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("a/b/c/d/deep.txt").unwrap(), "deep");
}

#[test]
fn copy_in_custom_message() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        message: Some("import files".into()),
        ..Default::default()
    })
    .unwrap();

    let fs = store.branches().get("main").unwrap();
    let log = fs.log(fs::LogOptions { limit: Some(1), ..Default::default() }).unwrap();
    assert_eq!(log[0].message, "import files");
}

// ---------------------------------------------------------------------------
// copy_out — edge cases
// ---------------------------------------------------------------------------

#[test]
fn copy_out_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    fs.copy_out(&["dir/"], dest_str, Default::default()).unwrap();
    // copy_out("dir/") uses contents mode, so paths are relative to dir
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(dest.join("b.txt")).unwrap(), "bbb");
    // hello.txt should not be exported (it's outside "dir")
    assert!(!dest.join("hello.txt").exists());
}

#[test]
fn copy_out_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    // Export just the root which includes hello.txt
    fs.copy_out(&[""], dest_str, fs::CopyOutOptions {
        include: Some(vec!["hello.txt".into()]),
        ..Default::default()
    })
    .unwrap();

    assert!(dest.join("hello.txt").exists());
}

// ---------------------------------------------------------------------------
// sync — idempotent
// ---------------------------------------------------------------------------

#[test]
fn sync_in_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let src_str = dir_src(&src);

    // First sync
    let fs = store.branches().get("main").unwrap();
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash1 = fs.commit_hash().unwrap();

    // Second sync with same files — should be no-op
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash2 = fs.commit_hash().unwrap();

    assert_eq!(hash1, hash2);
}

// ---------------------------------------------------------------------------
// export — edge cases
// ---------------------------------------------------------------------------

#[test]
fn copy_out_root_empty_repo() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let dest = dir.path().join("exported");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    let report = fs.copy_out(&[""], dest_str, fs::CopyOutOptions::default()).unwrap();
    assert_eq!(report.total(), 0);
}

// ---------------------------------------------------------------------------
// rename — content preservation
// ---------------------------------------------------------------------------

#[test]
fn rename_preserves_content() {
    let dir = tempfile::tempdir().unwrap();
    let (store, fs) = common::store_with_files(dir.path());
    fs.rename("dir/a.txt", "dir/renamed_a.txt", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("dir/renamed_a.txt").unwrap(), "aaa");
    assert!(!fs.exists("dir/a.txt").unwrap());
    // b.txt still present
    assert_eq!(fs.read_text("dir/b.txt").unwrap(), "bbb");
}

#[test]
fn rename_to_different_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (store, fs) = common::store_with_files(dir.path());
    fs.rename("hello.txt", "newdir/hello.txt", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert!(!fs.exists("hello.txt").unwrap());
    assert_eq!(fs.read_text("newdir/hello.txt").unwrap(), "hello");
}

#[test]
fn rename_custom_message() {
    let dir = tempfile::tempdir().unwrap();
    let (store, fs) = common::store_with_files(dir.path());
    fs.rename("hello.txt", "moved.txt", fs::WriteOptions {
        message: Some("move hello".into()),
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();
    let log = fs.log(fs::LogOptions { limit: Some(1), ..Default::default() }).unwrap();
    assert_eq!(log[0].message, "move hello");
}

// ---------------------------------------------------------------------------
// copy_in — overwrites
// ---------------------------------------------------------------------------

#[test]
fn copy_in_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"new content").unwrap();

    let (store, _fs) = common::store_with_files(dir.path());
    // Store already has hello.txt = "hello"
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("hello.txt").unwrap(), "hello");

    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("hello.txt").unwrap(), "new content");
}

#[test]
fn copy_in_unicode_filenames() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("café.txt"), b"latte").unwrap();
    std::fs::write(src.join("日本語.txt"), b"nihongo").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("café.txt").unwrap(), "latte");
    assert_eq!(fs.read_text("日本語.txt").unwrap(), "nihongo");
}

// ---------------------------------------------------------------------------
// copy_out — empty store
// ---------------------------------------------------------------------------

#[test]
fn copy_out_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    let report = fs.copy_out(&[""], dest_str, Default::default()).unwrap();
    assert_eq!(report.total(), 0);
}

// ---------------------------------------------------------------------------
// sync_out — with include filter
// ---------------------------------------------------------------------------

#[test]
fn sync_out_basic_with_filters() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("synced");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();
    let report = fs.sync_out("", dest_str, fs::SyncOptions {
        include: Some(vec!["hello.txt".into()]),
        ..Default::default()
    })
    .unwrap();
    assert!(report.total() > 0);
    assert!(dest.join("hello.txt").exists());
    // dir/a.txt should not be synced due to include filter
    assert!(!dest.join("dir/a.txt").exists());
}

// ---------------------------------------------------------------------------
// remove — edge cases
// ---------------------------------------------------------------------------

#[test]
fn remove_nonexistent_path() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("does_not_exist");
    // Don't create the directory

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let result = fs.remove_from_disk(&target, Default::default());
    // Either returns empty report or errors — either is acceptable
    if let Ok(report) = result {
        assert_eq!(report.total(), 0);
    }
}

#[test]
fn remove_with_exclude_filter() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("to_remove");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("delete.txt"), b"bye").unwrap();
    std::fs::write(target.join("keep.md"), b"keep").unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.remove_from_disk(&target, fs::RemoveFromDiskOptions {
        exclude: Some(vec!["*.md".into()]),
        ..Default::default()
    })
    .unwrap();

    assert!(!target.join("delete.txt").exists());
    assert!(target.join("keep.md").exists());
}

// ---------------------------------------------------------------------------
// copy_in — preserves executable (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn copy_in_preserves_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("run.sh"), b"#!/bin/sh").unwrap();
    std::fs::set_permissions(
        src.join("run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);
    fs.copy_in(&[&src_str], "", Default::default()).unwrap();

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.file_type("run.sh").unwrap(), FileType::Executable);
}

// ---------------------------------------------------------------------------
// copy_in — dry run
// ---------------------------------------------------------------------------

#[test]
fn copy_in_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let hash_before = fs.commit_hash().unwrap();

    let src_str = dir_src(&src);
    let (report, _) = fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        dry_run: true,
        ..Default::default()
    })
    .unwrap();

    // Report shows what would be added
    assert!(report.total() > 0);

    // But no commit was made
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.commit_hash().unwrap(), hash_before);
    assert!(!fs.exists("file1.txt").unwrap());
}

// ---------------------------------------------------------------------------
// sync_in — true sync behavior (add + update + delete)
// ---------------------------------------------------------------------------

#[test]
fn sync_in_detects_updates() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("file1.txt").unwrap(), "one");

    // Modify a file on disk
    std::fs::write(src.join("file1.txt"), b"updated").unwrap();

    // Second sync should detect the update
    let (report, _) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    assert!(!report.update.is_empty());
    assert!(report.update.iter().any(|f| f.path == "file1.txt"));

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("file1.txt").unwrap(), "updated");
}

#[test]
fn sync_in_detects_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync
    let (_, _) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert!(fs.exists("file2.txt").unwrap());

    // Delete a file from disk
    std::fs::remove_file(src.join("file2.txt")).unwrap();

    // Second sync should detect the deletion
    let (report, _) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    assert!(!report.delete.is_empty());
    assert!(report.delete.iter().any(|f| f.path == "file2.txt"));

    let fs = store.branches().get("main").unwrap();
    assert!(!fs.exists("file2.txt").unwrap());
}

#[test]
fn sync_in_detects_adds() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    // Add a new file on disk
    std::fs::write(src.join("new_file.txt"), b"new").unwrap();

    // Second sync should detect the addition
    let (report, _) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    assert!(!report.add.is_empty());
    assert!(report.add.iter().any(|f| f.path == "new_file.txt"));

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("new_file.txt").unwrap(), "new");
}

#[test]
fn sync_in_noop_when_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash_after_first = fs.commit_hash().unwrap();

    // Second sync with no changes — should be no-op
    let (report, _) = fs.sync_in(&src_str, "", Default::default()).unwrap();
    assert_eq!(report.total(), 0);

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.commit_hash().unwrap(), hash_after_first);
}

#[test]
fn sync_in_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync
    fs.sync_in(&src_str, "", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    let hash_after_first = fs.commit_hash().unwrap();

    // Modify disk
    std::fs::write(src.join("file1.txt"), b"modified").unwrap();
    std::fs::remove_file(src.join("file2.txt")).unwrap();

    // Dry run: should report changes but not commit
    let (report, _) = fs.sync_in(&src_str, "", fs::SyncOptions {
        dry_run: true,
        ..Default::default()
    })
    .unwrap();
    assert!(report.total() > 0);

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.commit_hash().unwrap(), hash_after_first);
    // Original content unchanged
    assert_eq!(fs.read_text("file1.txt").unwrap(), "one");
    assert!(fs.exists("file2.txt").unwrap());
}

// ---------------------------------------------------------------------------
// sync_out — deletes extra local files (Fix 1)
// ---------------------------------------------------------------------------

#[test]
fn sync_out_deletes_extra_files() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fs) = common::store_with_files(dir.path());
    let dest = dir.path().join("synced");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();

    // First sync: writes hello.txt, dir/a.txt, dir/b.txt
    fs.sync_out("", dest_str, Default::default()).unwrap();
    assert!(dest.join("hello.txt").exists());
    assert!(dest.join("dir/a.txt").exists());

    // Create extra files on disk that are NOT in the repo
    std::fs::write(dest.join("extra.txt"), b"extra").unwrap();
    std::fs::create_dir_all(dest.join("orphan_dir")).unwrap();
    std::fs::write(dest.join("orphan_dir/stale.txt"), b"stale").unwrap();

    // Second sync should delete the extra files
    let report = fs.sync_out("", dest_str, Default::default()).unwrap();
    assert!(!dest.join("extra.txt").exists());
    assert!(!dest.join("orphan_dir/stale.txt").exists());
    // Orphan dir should be pruned
    assert!(!dest.join("orphan_dir").exists());
    // Original files still present
    assert!(dest.join("hello.txt").exists());
    assert!(dest.join("dir/a.txt").exists());
    // Report should include deletes
    assert!(report.delete.len() >= 2);
}

#[test]
fn sync_out_updates_changed_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("file.txt", b"original", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let dest = dir.path().join("synced");
    std::fs::create_dir(&dest).unwrap();

    let dest_str = dest.to_str().unwrap();

    // First sync
    fs.sync_out("", dest_str, Default::default()).unwrap();
    assert_eq!(std::fs::read_to_string(dest.join("file.txt")).unwrap(), "original");

    // Modify the local file to something different
    std::fs::write(dest.join("file.txt"), b"modified locally").unwrap();

    // Sync again — should overwrite with repo content
    let report = fs.sync_out("", dest_str, Default::default()).unwrap();
    assert_eq!(std::fs::read_to_string(dest.join("file.txt")).unwrap(), "original");
    assert!(!report.update.is_empty());
}

#[test]
fn sync_out_prunes_empty_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    fs.write("keep.txt", b"keep", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();

    let dest = dir.path().join("synced");
    std::fs::create_dir(&dest).unwrap();
    let dest_str = dest.to_str().unwrap();
    fs.sync_out("", dest_str, Default::default()).unwrap();

    // Create a deep directory structure with files not in repo
    std::fs::create_dir_all(dest.join("a/b/c")).unwrap();
    std::fs::write(dest.join("a/b/c/orphan.txt"), b"orphan").unwrap();

    // Sync should delete the orphan and prune the empty dirs
    fs.sync_out("", dest_str, Default::default()).unwrap();
    assert!(!dest.join("a/b/c/orphan.txt").exists());
    assert!(!dest.join("a/b/c").exists());
    assert!(!dest.join("a/b").exists());
    assert!(!dest.join("a").exists());
}

// ---------------------------------------------------------------------------
// copy_in — checksum optimization (Fix 7)
// ---------------------------------------------------------------------------

#[test]
fn copy_in_checksum_skips_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // First copy_in
    let (report1, _) = fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: true,
        ..Default::default()
    })
    .unwrap();
    assert!(report1.total() > 0);
    let fs = store.branches().get("main").unwrap();
    let hash_after_first = fs.commit_hash().unwrap();

    // Second copy_in with same files + checksum=true → should be no-op
    let (report2, _) = fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: true,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(report2.total(), 0);
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.commit_hash().unwrap(), hash_after_first);
}

#[test]
fn copy_in_no_checksum_always_writes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // First copy_in with checksum=false
    fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: false,
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    // Touch files so their mtime is newer than the commit time
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for name in &["file1.txt", "file2.txt", "sub/deep.txt"] {
        let p = src.join(name);
        let content = std::fs::read(&p).unwrap();
        std::fs::write(&p, &content).unwrap();
    }

    // Second copy_in with checksum=false → should still report all files
    let (report, _) = fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: false,
        ..Default::default()
    })
    .unwrap();
    // Without checksum, all files with newer mtime are included in the report
    assert!(report.total() > 0);
}

#[test]
fn copy_in_checksum_detects_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // First copy_in
    fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: true,
        ..Default::default()
    })
    .unwrap();
    let fs = store.branches().get("main").unwrap();

    // Modify a file
    std::fs::write(src.join("file1.txt"), b"changed content").unwrap();

    // Second copy_in with checksum → should detect the change
    let (report, _) = fs.copy_in(&[&src_str], "", fs::CopyInOptions {
        checksum: true,
        ..Default::default()
    })
    .unwrap();
    // Changed file should appear in the report
    assert!(report.add.iter().any(|f| f.path == "file1.txt"));
    // Unchanged files should NOT be in the report
    assert!(!report.add.iter().any(|f| f.path == "file2.txt"));
    assert!(!report.add.iter().any(|f| f.path == "sub/deep.txt"));
}

#[test]
fn sync_in_with_dest_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_files");
    create_disk_files(&src);

    let store = common::create_store(dir.path(), "main");
    let fs = store.branches().get("main").unwrap();
    let src_str = dir_src(&src);

    // Initial sync into a prefix
    fs.sync_in(&src_str, "data", Default::default()).unwrap();
    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("data/file1.txt").unwrap(), "one");

    // Add and remove files on disk
    std::fs::write(src.join("new.txt"), b"new").unwrap();
    std::fs::remove_file(src.join("file2.txt")).unwrap();

    let (report, _) = fs.sync_in(&src_str, "data", Default::default()).unwrap();
    assert!(report.add.iter().any(|f| f.path == "data/new.txt"));
    assert!(report.delete.iter().any(|f| f.path == "data/file2.txt"));

    let fs = store.branches().get("main").unwrap();
    assert_eq!(fs.read_text("data/new.txt").unwrap(), "new");
    assert!(!fs.exists("data/file2.txt").unwrap());
}
