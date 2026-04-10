#pragma once

#include "error.h"
#include "types.h"

#include <cstdint>
#include <filesystem>
#include <memory>
#include <optional>
#include <string>
#include <vector>

struct git_oid;

namespace vost {

struct GitStoreInner;
class Batch;

// ---------------------------------------------------------------------------
// Fs — a snapshot of a git-backed filesystem
// ---------------------------------------------------------------------------

/// A read-only or read-write snapshot of a git tree at a specific commit.
///
/// Cheap to copy — holds a shared_ptr<GitStoreInner> plus a few fields.
/// Write operations return a NEW Fs representing the resulting commit.
///
/// Usage:
/// @code
///     auto fs = store.branches()["main"];
///     auto text = fs.read_text("README.md");
///
///     // Reassign to advance to the new commit
///     fs = fs.write_text("note.txt", "hello");
/// @endcode
class Fs {
public:
    // -- Constructors / factory (internal use; use GitStore / RefDict) -------

    /// Construct an Fs from a raw commit hex SHA (internal).
    static Fs from_commit(std::shared_ptr<GitStoreInner> inner,
                          const std::string& commit_oid_hex,
                          std::optional<std::string> ref_name,
                          bool writable);

    /// Construct an empty Fs (no commit, no tree) for a new branch.
    static Fs empty(std::shared_ptr<GitStoreInner> inner,
                    std::string ref_name);

    // -- Identity / metadata ------------------------------------------------

    /// 40-char hex SHA of the commit, or nullopt for empty snapshots.
    std::optional<std::string> commit_hash() const;

    /// 40-char hex SHA of the root tree, or nullopt for empty snapshots.
    std::optional<std::string> tree_hash() const;

    /// Branch or tag name, or nullopt for detached snapshots.
    const std::optional<std::string>& ref_name() const { return ref_name_; }

    /// True for branch snapshots, false for tags and detached commits.
    bool writable() const { return writable_; }

    /// Commit message (trailing newline stripped).
    /// @throws NotFoundError if no commit.
    std::string message() const;

    /// Commit timestamp as POSIX epoch seconds.
    /// @throws NotFoundError if no commit.
    uint64_t time() const;

    /// Commit author name.
    std::string author_name() const;

    /// Commit author email.
    std::string author_email() const;

    /// Change report from the write operation that produced this snapshot.
    const std::optional<ChangeReport>& changes() const { return changes_; }

    // -- Read ---------------------------------------------------------------

    /// Read file contents as bytes.
    /// @throws NotFoundError if path does not exist.
    /// @throws IsADirectoryError if path is a directory.
    std::vector<uint8_t> read(const std::string& path) const;

    /// Read file contents as a UTF-8 string.
    /// @throws NotFoundError if path does not exist.
    std::string read_text(const std::string& path) const;

    /// List entry names at `path` (or root if empty).
    /// When `recursive` is true, returns a flat list of all file paths
    /// (full relative paths, no directories).
    /// @throws NotADirectoryError if path is a file.
    std::vector<std::string> ls(const std::string& path = "",
                                bool recursive = false) const;

    /// Recursively walk all directories under `path` (os.walk-style).
    /// Returns one WalkDirEntry per directory, each with dirnames and files.
    std::vector<WalkDirEntry>
    walk(const std::string& path = "") const;

    /// Return true if `path` exists (file, directory, or symlink).
    bool exists(const std::string& path) const;

    /// Return true if `path` is a directory.
    bool is_dir(const std::string& path) const;

    /// Return the FileType of `path`.
    /// @throws NotFoundError if path does not exist.
    FileType file_type(const std::string& path) const;

    /// Return the size in bytes of the object at `path`.
    /// @throws NotFoundError if path does not exist.
    /// @throws IsADirectoryError if path is a directory.
    uint64_t size(const std::string& path) const;

    /// Return the 40-char hex SHA of the object at `path`.
    std::string object_hash(const std::string& path) const;

    /// Read the target of a symlink at `path`.
    std::string readlink(const std::string& path) const;

    /// stat() — single-call getattr for FUSE.
    /// @throws NotFoundError if path does not exist.
    StatResult stat(const std::string& path = "") const;

    /// List directory entries with name, OID, and mode — for FUSE readdir.
    std::vector<WalkEntry> listdir(const std::string& path = "") const;

    /// Read with optional byte-range (for FUSE partial reads).
    std::vector<uint8_t> read_range(const std::string& path,
                                    size_t offset,
                                    std::optional<size_t> size = std::nullopt) const;

    /// Read raw blob data by its hex hash, bypassing tree lookup.
    std::vector<uint8_t> read_by_hash(const std::string& hash,
                                      size_t offset = 0,
                                      std::optional<size_t> size = std::nullopt) const;

    /// Glob for matching paths. Returns results sorted.
    std::vector<std::string> glob(const std::string& pattern) const;

    /// Glob for matching paths. Returns results unsorted (faster).
    std::vector<std::string> iglob(const std::string& pattern) const;

    // -- Write --------------------------------------------------------------

    /// Write `data` to `path` and commit, returning a new Fs.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs write(const std::string& path,
             const std::vector<uint8_t>& data,
             WriteOptions opts = {}) const;

    /// Write a UTF-8 string to `path` and commit, returning a new Fs.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs write_text(const std::string& path,
                  const std::string& text,
                  WriteOptions opts = {}) const;

    /// Write a local file from disk into the store.
    /// @throws IoError if the local file cannot be read.
    Fs write_from_file(const std::string& path,
                       const std::filesystem::path& local_path,
                       WriteOptions opts = {}) const;

    /// Write a symlink at `path` pointing to `target`.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs write_symlink(const std::string& path,
                     const std::string& target,
                     WriteOptions opts = {}) const;

    /// Apply a batch of writes and removes in a single atomic commit.
    /// @param writes  Vector of (path, WriteEntry) pairs to write.
    /// @param removes Paths to delete from the repo.
    /// @param opts    Options including optional message and operation name.
    /// @return New Fs snapshot with all changes committed.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch has advanced since this snapshot.
    Fs apply(const std::vector<std::pair<std::string, WriteEntry>>& writes,
             const std::vector<std::string>& removes = {},
             ApplyOptions opts = {}) const;

    /// Remove one or more paths and commit.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs remove(const std::vector<std::string>& paths,
              RemoveOptions opts = {}) const;

    // -- Move ---------------------------------------------------------------

    /// Move files/directories within the repo (POSIX mv semantics).
    /// Supports multiple sources into a directory destination.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws NotFoundError if a source path does not exist.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs move(const std::vector<std::string>& sources,
            const std::string& dest,
            MoveOptions opts = {}) const;

    // -- Copy ---------------------------------------------------------------

    /// Copy files from one ref to another within the same repo.
    /// Reuses blob OIDs for efficiency — no data is read into memory.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs copy_from_ref(const Fs& source,
                     const std::vector<std::string>& sources = {""},
                     const std::string& dest = "",
                     CopyFromRefOptions opts = {}) const;

    /// Copy files from a named branch or tag into this branch.
    /// Resolves the name to an Fs (tries branches first, then tags),
    /// then delegates to copy_from_ref(const Fs&, ...).
    /// @throws InvalidHashError if the name is not a known branch or tag.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs copy_from_ref(const std::string& source_name,
                     const std::vector<std::string>& sources = {""},
                     const std::string& dest = "",
                     CopyFromRefOptions opts = {}) const;

    /// Copy files from local disk `src` into the store at `dest`.
    /// Returns the ChangeReport and a new Fs with the committed changes.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    std::pair<ChangeReport, Fs>
    copy_in(const std::filesystem::path& src,
            const std::string& dest = "",
            CopyInOptions opts = {}) const;

    /// Copy files from the store at `src` to local disk `dest`.
    /// @throws NotFoundError if `src` does not exist in the store.
    ChangeReport
    copy_out(const std::string& src,
             const std::filesystem::path& dest,
             CopyOutOptions opts = {}) const;

    /// Sync local disk `src` into the store at `dest` (copy + delete extras).
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    std::pair<ChangeReport, Fs>
    sync_in(const std::filesystem::path& src,
            const std::string& dest = "",
            SyncOptions opts = {}) const;

    /// Sync from the store at `src` to local disk `dest` (copy + delete extras).
    /// @throws NotFoundError if `src` does not exist in the store.
    ChangeReport
    sync_out(const std::string& src,
             const std::filesystem::path& dest,
             SyncOptions opts = {}) const;

    // -- Batch --------------------------------------------------------------

    /// Return a Batch accumulator for this snapshot.
    Batch batch(BatchOptions opts = {}) const;

    // -- History navigation -------------------------------------------------

    /// Return the parent Fs, or nullopt if this is an initial commit.
    std::optional<Fs> parent() const;

    /// Return an Fs `n` commits behind HEAD on the same branch.
    Fs back(size_t n) const;

    /// Return commit history matching the given filters.
    std::vector<CommitInfo> log(LogOptions opts = {}) const;

    /// Undo the last `n` commits by resetting the branch to its n-th ancestor.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    /// @throws NotFoundError if there is insufficient history.
    Fs undo(size_t n = 1) const;

    /// Rename a file or directory from `src` to `dest`.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws NotFoundError if `src` does not exist.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    Fs rename(const std::string& src, const std::string& dest,
              WriteOptions opts = {}) const;

    /// Redo the last `n` undone commits using the reflog.
    /// @throws PermissionError if this snapshot is read-only.
    /// @throws StaleSnapshotError if the branch tip has advanced.
    /// @throws NotFoundError if no redo history is found.
    Fs redo(size_t n = 1) const;

    // -- Squash -------------------------------------------------------------

    /// Create a new commit with this snapshot's tree but no history.
    /// Returns a detached (read-only) Fs.
    Fs squash(std::optional<Fs> parent = std::nullopt,
              const std::string& message = "squash") const;

    // -- Internal -----------------------------------------------------------

    /// Access the shared store inner (used by Batch, RefDict, tree functions).
    std::shared_ptr<GitStoreInner> inner() const { return inner_; }

    /// Raw commit OID hex (internal — may be empty string for empty snapshots).
    const std::string& commit_oid_hex() const { return commit_oid_hex_; }

    /// Raw tree OID hex (internal — may be empty string).
    const std::string& tree_oid_hex() const { return tree_oid_hex_; }

    // -- Internal factory ---------------------------------------------------

    /// Build an Fs from a raw commit oid hex, resolving tree automatically.
    /// Used by commit_changes() and RefDict::get().
    Fs(std::shared_ptr<GitStoreInner> inner,
       std::string commit_oid_hex,
       std::string tree_oid_hex,
       std::optional<std::string> ref_name,
       bool writable,
       std::optional<ChangeReport> changes = std::nullopt);

    friend class Batch;

private:
    std::shared_ptr<GitStoreInner> inner_;
    std::string                    commit_oid_hex_; ///< 40-char hex or empty.
    std::string                    tree_oid_hex_;   ///< 40-char hex or empty.
    std::optional<std::string>     ref_name_;
    bool                           writable_;
    std::optional<ChangeReport>    changes_;

    // -- Helpers ------------------------------------------------------------

    /// Throw PermissionError + return ref_name if writable.
    const std::string& require_writable(const std::string& verb) const;

    /// Throw NotFoundError("no tree in snapshot") if tree is absent.
    const std::string& require_tree() const;

    /// Commit pending writes/removes and return new Fs.
    Fs commit_changes(
        const std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>>& writes,
        const std::vector<std::string>& removes,
        const std::string& message,
        std::optional<ChangeReport> report = std::nullopt,
        const std::vector<std::string>& extra_parent_oids = {}) const;
};

// ---------------------------------------------------------------------------
// FsWriter — RAII streaming write
// ---------------------------------------------------------------------------

/// Accumulates data in memory, then writes to the repo on close().
///
/// Usage:
/// @code
///     auto w = FsWriter(fs, "data.bin");
///     w.write(chunk1);
///     w.write(chunk2);
///     fs = w.close();
/// @endcode
class FsWriter {
public:
    FsWriter(Fs fs, std::string path, WriteOptions opts = {});
    ~FsWriter();

    /// Append raw bytes.
    FsWriter& write(const std::vector<uint8_t>& data);

    /// Append a UTF-8 string.
    FsWriter& write(const std::string& text);

    /// Flush and commit. Returns the resulting Fs.
    Fs close();

    /// The resulting Fs (only valid after close()).
    const Fs& fs() const { return fs_; }

    // Non-copyable, movable
    FsWriter(const FsWriter&) = delete;
    FsWriter& operator=(const FsWriter&) = delete;
    FsWriter(FsWriter&&) = default;
    FsWriter& operator=(FsWriter&&) = default;

private:
    Fs fs_;
    std::string path_;
    WriteOptions opts_;
    std::vector<uint8_t> buffer_;
    bool closed_ = false;
};

} // namespace vost
