#pragma once

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>
#include <filesystem>

#include "vost/error.h"

namespace vost {

// ---------------------------------------------------------------------------
// Mode constants (mirror git filemode integers)
// ---------------------------------------------------------------------------

constexpr uint32_t MODE_BLOB      = 0100644; ///< Regular file.
constexpr uint32_t MODE_BLOB_EXEC = 0100755; ///< Executable file.
constexpr uint32_t MODE_LINK      = 0120000; ///< Symbolic link.
constexpr uint32_t MODE_TREE      = 0040000; ///< Directory / subtree.

// ---------------------------------------------------------------------------
// FileType
// ---------------------------------------------------------------------------

/// The type of a git tree entry.
enum class FileType : uint8_t {
    Blob,        ///< Regular file (0o100644).
    Executable,  ///< Executable file (0o100755).
    Link,        ///< Symbolic link (0o120000).
    Tree,        ///< Directory / subtree (0o040000).
};

/// Convert a raw git mode to a FileType. Returns nullopt for unknown modes.
inline std::optional<FileType> file_type_from_mode(uint32_t mode) {
    switch (mode) {
        case MODE_BLOB:      return FileType::Blob;
        case MODE_BLOB_EXEC: return FileType::Executable;
        case MODE_LINK:      return FileType::Link;
        case MODE_TREE:      return FileType::Tree;
        default:             return std::nullopt;
    }
}

/// Return the raw git filemode for a FileType.
inline uint32_t file_type_to_mode(FileType ft) {
    switch (ft) {
        case FileType::Blob:       return MODE_BLOB;
        case FileType::Executable: return MODE_BLOB_EXEC;
        case FileType::Link:       return MODE_LINK;
        case FileType::Tree:       return MODE_TREE;
    }
    return MODE_BLOB; // unreachable
}

/// Return true if the FileType represents a regular or executable file.
inline bool file_type_is_file(FileType ft) {
    return ft == FileType::Blob || ft == FileType::Executable;
}

/// Return true if the FileType represents a directory (tree).
inline bool file_type_is_dir(FileType ft) { return ft == FileType::Tree; }

/// Return true if the FileType represents a symbolic link.
inline bool file_type_is_link(FileType ft) { return ft == FileType::Link; }

// ---------------------------------------------------------------------------
// WalkEntry
// ---------------------------------------------------------------------------

/// An entry yielded when listing or walking a tree.
struct WalkEntry {
    std::string name; ///< Basename of the entry.
    std::string oid;  ///< 40-char hex SHA of the git object.
    uint32_t    mode; ///< Raw git filemode.

    /// Return the FileType for this entry, or nullopt for unknown modes.
    std::optional<FileType> file_type() const {
        return file_type_from_mode(mode);
    }
};

// ---------------------------------------------------------------------------
// WalkDirEntry
// ---------------------------------------------------------------------------

/// An entry yielded by os.walk-style directory traversal.
struct WalkDirEntry {
    std::string              dirpath;   ///< Directory path ("" for root).
    std::vector<std::string> dirnames;  ///< Subdirectory names in this directory.
    std::vector<WalkEntry>   files;     ///< Non-directory entries in this directory.
};

// ---------------------------------------------------------------------------
// StatResult
// ---------------------------------------------------------------------------

/// Result of a stat() call — single-call getattr for FUSE.
struct StatResult {
    uint32_t    mode;      ///< Raw git filemode.
    FileType    file_type; ///< Parsed file type.
    uint64_t    size;      ///< Size in bytes (blob) or entry count (dir).
    std::string hash;      ///< 40-char hex SHA of the object.
    uint32_t    nlink;     ///< Number of hard links (2 + subdirs for dirs).
    uint64_t    mtime;     ///< Commit timestamp (POSIX epoch seconds).
};

// ---------------------------------------------------------------------------
// WriteEntry
// ---------------------------------------------------------------------------

/// Data to be written to the store.
struct WriteEntry {
    std::optional<std::vector<uint8_t>> data;   ///< Raw content (for blobs).
    std::optional<std::string>          target;  ///< Symlink target.
    uint32_t                            mode;    ///< Git file mode.

    /// Create a blob entry from raw bytes.
    static WriteEntry from_bytes(std::vector<uint8_t> d) {
        return WriteEntry{std::move(d), std::nullopt, MODE_BLOB};
    }

    /// Create a blob entry from a UTF-8 string.
    static WriteEntry from_text(std::string text) {
        std::vector<uint8_t> d(text.begin(), text.end());
        return WriteEntry{std::move(d), std::nullopt, MODE_BLOB};
    }

    /// Create a symlink entry.
    static WriteEntry symlink(std::string t) {
        return WriteEntry{std::nullopt, std::move(t), MODE_LINK};
    }

    /// Validate that data/target/mode are consistent.
    void validate() const {
        if (mode == MODE_LINK) {
            if (!target) throw InvalidPathError("symlink entry requires a target");
            if (data)    throw InvalidPathError("symlink entry must not have data");
        } else if (mode == MODE_BLOB || mode == MODE_BLOB_EXEC) {
            if (!data)   throw InvalidPathError("blob entry requires data");
            if (target)  throw InvalidPathError("blob entry must not have a symlink target");
        } else {
            throw InvalidPathError("unsupported mode: " + std::to_string(mode));
        }
    }
};

// ---------------------------------------------------------------------------
// FileEntry
// ---------------------------------------------------------------------------

/// Describes a file in a change report.
struct FileEntry {
    std::string                           path;      ///< Relative path.
    FileType                              file_type; ///< Type of the file.
    std::optional<std::filesystem::path> src;        ///< Source path on disk.

    bool operator<(const FileEntry& o) const { return path < o.path; }
};

// ---------------------------------------------------------------------------
// ChangeReport
// ---------------------------------------------------------------------------

/// Kinds of change actions.
enum class ChangeActionKind : uint8_t {
    Add,    ///< A new file was added.
    Update, ///< An existing file was modified.
    Delete, ///< A file was removed.
};

/// A single change action (kind + path).
struct ChangeAction {
    ChangeActionKind kind;
    std::string      path;

    bool operator<(const ChangeAction& o) const { return path < o.path; }
};

/// An error encountered during a change operation.
struct ChangeError {
    std::string path;
    std::string error;
};

/// Report summarising the outcome of a sync / copy / import operation.
struct ChangeReport {
    std::vector<FileEntry>   add;
    std::vector<FileEntry>   update;
    std::vector<FileEntry>   del;      ///< Named 'del' to avoid C++ keyword.
    std::vector<ChangeError> errors;
    std::vector<ChangeError> warnings;

    bool in_sync() const {
        return add.empty() && update.empty() && del.empty();
    }

    size_t total() const {
        return add.size() + update.size() + del.size();
    }

    std::vector<ChangeAction> actions() const {
        std::vector<ChangeAction> out;
        out.reserve(total());
        for (auto& fe : add)    out.push_back({ChangeActionKind::Add,    fe.path});
        for (auto& fe : update) out.push_back({ChangeActionKind::Update, fe.path});
        for (auto& fe : del)    out.push_back({ChangeActionKind::Delete, fe.path});
        std::sort(out.begin(), out.end());
        return out;
    }
};

// ---------------------------------------------------------------------------
// Signature
// ---------------------------------------------------------------------------

/// Author/committer identity used for commits.
struct Signature {
    std::string name  = "vost";
    std::string email = "vost@localhost";
};

// ---------------------------------------------------------------------------
// ReflogEntry
// ---------------------------------------------------------------------------

/// A single reflog entry recording a branch movement.
struct ReflogEntry {
    std::string old_sha;    ///< Previous 40-char hex commit SHA.
    std::string new_sha;    ///< New 40-char hex commit SHA.
    std::string committer;  ///< Identity string.
    uint64_t    timestamp;  ///< POSIX epoch seconds.
    std::string message;    ///< Reflog message.
};

// ---------------------------------------------------------------------------
// RefChange / MirrorDiff
// ---------------------------------------------------------------------------

/// Describes a reference change during backup/restore.
struct RefChange {
    std::string                ref_name;   ///< Full ref name.
    std::optional<std::string> old_target; ///< Previous SHA (nullopt = created).
    std::optional<std::string> new_target; ///< New SHA (nullopt = deleted).
};

/// Summary of differences between two repositories.
struct MirrorDiff {
    std::vector<RefChange> add;
    std::vector<RefChange> update;
    std::vector<RefChange> del;

    bool   in_sync() const { return add.empty() && update.empty() && del.empty(); }
    size_t total()   const { return add.size() + update.size() + del.size(); }
};

// ---------------------------------------------------------------------------
// OpenOptions
// ---------------------------------------------------------------------------

/// Options for opening or creating a GitStore.
struct OpenOptions {
    bool                       create = false; ///< Create if not found.
    std::optional<std::string> branch;         ///< Default branch name.
    std::optional<std::string> author;         ///< Default author name.
    std::optional<std::string> email;          ///< Default author email.
};

// ---------------------------------------------------------------------------
// WriteOptions
// ---------------------------------------------------------------------------

/// Options for Fs::write / write_text / write_symlink.
struct WriteOptions {
    std::optional<std::string> message; ///< Commit message.
    std::optional<uint32_t>    mode;    ///< Git filemode override.
    std::vector<std::string>   parents; ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// ApplyOptions
// ---------------------------------------------------------------------------

/// Options for Fs::apply.
struct ApplyOptions {
    std::optional<std::string> message;
    std::optional<std::string> operation; ///< Operation prefix for auto-generated messages.
    std::vector<std::string>   parents;   ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// RemoveOptions
// ---------------------------------------------------------------------------

/// Options for Fs::remove.
struct RemoveOptions {
    bool                       recursive = false;
    bool                       dry_run   = false;
    std::optional<std::string> message;
    std::vector<std::string>   parents;   ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// BatchOptions
// ---------------------------------------------------------------------------

/// Options for Fs::batch.
struct BatchOptions {
    std::optional<std::string> message;
    std::optional<std::string> operation; ///< Operation prefix for auto-generated messages.
    std::vector<std::string>   parents;   ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// LogOptions / CommitInfo
// ---------------------------------------------------------------------------

/// Options for Fs::log().
struct LogOptions {
    std::optional<size_t>      limit;         ///< Max entries to return.
    std::optional<size_t>      skip;          ///< Skip this many matches.
    std::optional<std::string> path;          ///< Only commits that change this path.
    std::optional<std::string> match_pattern; ///< Glob pattern on commit message.
    std::optional<uint64_t>    before;        ///< Only commits before this epoch time.
};

/// Information about a single commit.
struct CommitInfo {
    std::string                commit_hash;
    std::string                message;
    std::optional<uint64_t>    time;
    std::optional<std::string> author_name;
    std::optional<std::string> author_email;
};

// ---------------------------------------------------------------------------
// CopyInOptions
// ---------------------------------------------------------------------------

/// Options for Fs::copy_in.
struct CopyInOptions {
    std::optional<std::vector<std::string>> include; ///< Glob patterns to include.
    std::optional<std::vector<std::string>> exclude; ///< Glob patterns to exclude.
    std::optional<std::string>              message; ///< Commit message.
    bool                                    dry_run   = false;
    bool                                    checksum  = true; ///< Skip unchanged files.
    std::vector<std::string>                parents;  ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// CopyOutOptions
// ---------------------------------------------------------------------------

/// Options for Fs::copy_out.
struct CopyOutOptions {
    std::optional<std::vector<std::string>> include;
    std::optional<std::vector<std::string>> exclude;
};

// ---------------------------------------------------------------------------
// SyncOptions
// ---------------------------------------------------------------------------

/// Options for Fs::sync_in / sync_out.
struct SyncOptions {
    std::optional<std::vector<std::string>> include;
    std::optional<std::vector<std::string>> exclude;
    std::optional<std::string>              message;
    bool                                    dry_run   = false;
    bool                                    checksum  = true;
    std::vector<std::string>                parents;  ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// MoveOptions
// ---------------------------------------------------------------------------

/// Options for Fs::move.
struct MoveOptions {
    bool                       recursive = false;
    bool                       dry_run   = false;
    std::optional<std::string> message;
    std::vector<std::string>   parents;   ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// CopyFromRefOptions
// ---------------------------------------------------------------------------

/// Options for Fs::copy_from_ref.
struct CopyFromRefOptions {
    bool                       delete_extra = false; ///< Delete files in dest not in source.
    bool                       dry_run      = false;
    std::optional<std::string> message;
    std::vector<std::string>   parents;   ///< Advisory extra parent commit hashes.
};

// ---------------------------------------------------------------------------
// ExcludeFilter
// ---------------------------------------------------------------------------

/// Gitignore-style exclude filter for copy_in/sync_in operations.
class ExcludeFilter {
public:
    ExcludeFilter() = default;

    /// Add exclude patterns using gitignore syntax.
    /// @param patterns Vector of gitignore-style patterns (e.g. "*.log", "build/").
    void add_patterns(const std::vector<std::string>& patterns);

    /// Load patterns from a file (one pattern per line, gitignore syntax).
    /// @param path Path to the patterns file (e.g. ".gitignore").
    void load_from_file(const std::filesystem::path& path);

    /// Check if a relative path should be excluded.
    /// @param rel_path Relative path to check.
    /// @param is_dir   True if the path is a directory (matches dir-only patterns).
    /// @return True if the path matches an exclude pattern.
    bool is_excluded(const std::string& rel_path, bool is_dir = false) const;

    /// True if any filtering is configured.
    bool active() const { return !patterns_.empty(); }

private:
    struct Pattern {
        std::string raw;
        bool negated  = false;
        bool dir_only = false;
    };
    std::vector<Pattern> patterns_;

    static bool match_pattern(const std::string& pattern,
                              const std::string& path);
};

// ---------------------------------------------------------------------------
// BackupOptions / RestoreOptions
// ---------------------------------------------------------------------------

/// Options for backup operations.
struct BackupOptions {
    bool dry_run = false;                  ///< Compute diff without pushing.
    std::vector<std::string> refs;         ///< Limit to specific refs (empty = all).
    std::string format;                    ///< Force format: "bundle" or empty.
    /// Rename refs during backup.  Keys are source ref names, values are
    /// destination ref names (both may be short or full).  When set, takes
    /// precedence over `refs`.
    std::map<std::string, std::string> ref_map;
    /// If true, each ref gets a parentless commit with the same tree,
    /// stripping all history from the exported bundle.
    bool squash = false;
};

/// Options for restore operations.
struct RestoreOptions {
    bool dry_run = false;                  ///< Compute diff without fetching.
    std::vector<std::string> refs;         ///< Limit to specific refs (empty = all).
    std::string format;                    ///< Force format: "bundle" or empty.
    /// Rename refs during restore.  Keys are source ref names, values are
    /// destination ref names (both may be short or full).  When set, takes
    /// precedence over `refs`.
    std::map<std::string, std::string> ref_map;
};

} // namespace vost
