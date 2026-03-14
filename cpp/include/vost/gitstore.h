#pragma once

#include "error.h"
#include "notes.h"
#include "types.h"

#include <filesystem>
#include <memory>
#include <mutex>
#include <string>

// Forward-declare libgit2 types to avoid pulling the header into every TU.
struct git_repository;

namespace vost {

class Fs;
class RefDict;

// ---------------------------------------------------------------------------
// GitStoreInner — shared state (analogous to Rust's Arc<GitStoreInner>)
// ---------------------------------------------------------------------------

/// Internal state shared via shared_ptr across Fs copies.
/// Not part of the public API.
struct GitStoreInner {
    git_repository*      repo;      ///< Raw libgit2 handle (owned).
    std::filesystem::path path;     ///< Path to the bare repository.
    Signature             signature; ///< Default commit signature.
    std::mutex            mutex;    ///< Thread-level serialization.

    // Non-copyable / non-movable — always accessed via shared_ptr.
    GitStoreInner(const GitStoreInner&) = delete;
    GitStoreInner& operator=(const GitStoreInner&) = delete;

    ~GitStoreInner();
    GitStoreInner(git_repository* r, std::filesystem::path p, Signature sig);
};

// ---------------------------------------------------------------------------
// GitStore
// ---------------------------------------------------------------------------

/// A versioned filesystem backed by a bare git repository.
///
/// Cheap to copy — internally holds a shared_ptr<GitStoreInner>.
///
/// Usage:
/// @code
///     auto store = vost::GitStore::open("/path/to/repo.git");
///     auto fs    = store.branches()["main"];
///     auto text  = fs.read_text("README.md");
/// @endcode
class GitStore {
public:
    // -- Construction -------------------------------------------------------

    /// Open (or create) a bare git repository at `path`.
    ///
    /// @param path  Path to the bare repository directory.
    /// @param opts  OpenOptions controlling creation and defaults.
    /// @throws NotFoundError if the repo does not exist and opts.create is false.
    /// @throws GitError on libgit2 failures.
    static GitStore open(const std::filesystem::path& path,
                         OpenOptions opts = {});

    // -- Navigation ---------------------------------------------------------

    /// Return a RefDict for branches (refs/heads/).
    RefDict branches();

    /// Return a RefDict for tags (refs/tags/).
    RefDict tags();

    /// Return an Fs for any ref (branch, tag, or commit hash).
    ///
    /// Resolution: branches → tags → commit hash.
    /// Writable for branches, read-only for tags and hashes.
    /// @throws NotFoundError if the ref cannot be resolved.
    Fs fs(const std::string& ref);

    /// Return a NoteDict for accessing git notes.
    NoteDict notes();

    // -- Mirror -------------------------------------------------------------

    /// Push local refs to @p dest, creating a mirror or bundle.
    ///
    /// Without refs filtering this is a full mirror: remote-only refs
    /// are deleted.  With ``opts.refs`` only the specified refs are
    /// pushed (no deletes).  Bundle format is auto-detected from
    /// ``.bundle`` extension or forced via ``opts.format``.
    ///
    /// @param dest  Destination URL, local path, or bundle file path.
    /// @param opts  BackupOptions (dry_run, refs filter, format).
    /// @return MirrorDiff describing what changed (or would change).
    MirrorDiff backup(const std::string& dest,
                      const BackupOptions& opts = {});

    /// Fetch refs from @p src additively (no deletes).
    ///
    /// Restore is **additive**: it adds and updates refs but never
    /// deletes local-only refs.  Bundle format is auto-detected from
    /// ``.bundle`` extension or forced via ``opts.format``.
    ///
    /// @param src   Source URL, local path, or bundle file path.
    /// @param opts  RestoreOptions (dry_run, refs filter, format).
    /// @return MirrorDiff describing what changed (or would change).
    MirrorDiff restore(const std::string& src,
                       const RestoreOptions& opts = {});

    /// Export refs to a git bundle file.
    ///
    /// @param path     Path to the bundle file to write.
    /// @param refs     Ref names to export (empty = all refs).
    /// @param ref_map  Rename map: source ref -> destination ref name in bundle
    ///                 (empty = no renaming).
    void bundle_export(const std::string& path,
                       const std::vector<std::string>& refs = {},
                       const std::map<std::string, std::string>& ref_map = {},
                       bool squash = false);

    /// Import refs from a git bundle file.
    ///
    /// @param path     Path to the bundle file to read.
    /// @param refs     Ref names to import (empty = all refs).
    /// @param ref_map  Rename map: bundle ref name -> local ref name
    ///                 (empty = no renaming).
    void bundle_import(const std::string& path,
                       const std::vector<std::string>& refs = {},
                       const std::map<std::string, std::string>& ref_map = {});

    // -- Metadata -----------------------------------------------------------

    /// Path to the bare repository on disk.
    const std::filesystem::path& path() const;

    /// The default signature used for commits.
    const Signature& signature() const;

    // -- Internal -----------------------------------------------------------

    /// Access the shared inner state (used by Fs, RefDict, Batch).
    std::shared_ptr<GitStoreInner> inner() const { return inner_; }

private:
    explicit GitStore(std::shared_ptr<GitStoreInner> inner);

    std::shared_ptr<GitStoreInner> inner_;
};

// ---------------------------------------------------------------------------
// RefDict
// ---------------------------------------------------------------------------

/// A transient view over a set of git references sharing a common prefix
/// (e.g. refs/heads/ or refs/tags/).
///
/// Obtained via store.branches() or store.tags().
class RefDict {
public:
    /// Get the Fs snapshot for the named branch or tag.
    /// @throws NotFoundError if the ref does not exist.
    Fs get(const std::string& name);

    /// Convenience: same as get().
    /// @param name Branch or tag name.
    /// @throws NotFoundError if the ref does not exist.
    Fs operator[](const std::string& name);

    /// Point the named ref at the commit of `fs`.
    /// @param name Branch or tag name.
    /// @param fs   Fs snapshot whose commit to point to.
    /// @throws InvalidRefNameError for bad ref names.
    /// @throws KeyExistsError when overwriting a tag.
    /// @throws std::invalid_argument if fs belongs to a different repository.
    void set(const std::string& name, const Fs& fs);

    /// Point the named ref at the commit of `fs` and return a new writable Fs
    /// bound to it. Equivalent to set() followed by get().
    /// @param name Branch name.
    /// @param fs   Fs snapshot to set (can be read-only).
    /// @return New writable Fs bound to the branch.
    Fs set_and_get(const std::string& name, const Fs& fs);

    /// Delete the named ref.
    /// @param name Branch or tag name.
    /// @throws KeyNotFoundError if the ref does not exist.
    void del(const std::string& name);

    /// Return true if the named ref exists.
    /// @param name Branch or tag name.
    bool contains(const std::string& name);

    /// Return all ref names under this prefix (without the prefix).
    std::vector<std::string> keys();

    /// Return Fs snapshots for all refs under this prefix.
    std::vector<Fs> values();

    /// Get the current branch name (HEAD), or nullopt if not set.
    /// Only meaningful for branches().
    std::optional<std::string> current_name();

    /// Get the current branch Fs (HEAD), or nullopt if not set.
    std::optional<Fs> current();

    /// Set HEAD to point at `name`. Only valid for branches().
    void set_current(const std::string& name);

    /// Return the reflog for the named ref (most-recent first).
    std::vector<ReflogEntry> reflog(const std::string& name);

    // -- Internal -----------------------------------------------------------
    RefDict(std::shared_ptr<GitStoreInner> inner, std::string prefix,
            bool writable);

private:
    std::shared_ptr<GitStoreInner> inner_;
    std::string                    prefix_;   ///< e.g. "refs/heads/"
    bool                           writable_; ///< true for branches
};

} // namespace vost
