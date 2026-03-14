#pragma once
/// Internal helpers shared between vost source files.
/// Not part of the public API.

#include "vost/error.h"
#include "vost/types.h"

#include <filesystem>
#include <functional>
#include <optional>
#include <string>
#include <utility>
#include <vector>

struct git_repository;

namespace vost {

// ---------------------------------------------------------------------------
// paths — path normalization and validation
// ---------------------------------------------------------------------------

namespace paths {

std::string normalize(const std::string& path);
void        validate_ref_name(const std::string& name);
bool        is_root(const std::string& path);
std::string format_message(const std::string& op,
                           const std::optional<std::string>& msg);

} // namespace paths

// ---------------------------------------------------------------------------
// lock — advisory file lock
// ---------------------------------------------------------------------------

namespace lock {

void with_repo_lock(const std::filesystem::path& gitdir,
                    std::function<void()> fn);

} // namespace lock

// ---------------------------------------------------------------------------
// tree — libgit2-based tree operations
// ---------------------------------------------------------------------------

namespace tree {

std::optional<std::pair<std::string, uint32_t>>
lookup(git_repository* repo,
       const std::string& tree_oid_hex,
       const std::string& norm_path);

std::vector<uint8_t>
read_blob(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path);

std::vector<WalkEntry>
list_tree(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path);

std::vector<std::pair<std::string, WalkEntry>>
walk_tree(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path);

std::vector<WalkDirEntry>
walk_tree_dirs(git_repository* repo,
               const std::string& tree_oid_hex,
               const std::string& norm_path);

uint32_t count_subdirs(git_repository* repo,
                        const std::string& tree_oid_hex);

/// List immediate children of a tree given its OID hex.
std::vector<WalkEntry>
list_tree_by_oid(git_repository* repo,
                 const std::string& tree_oid_hex);

std::string rebuild_tree(
    git_repository* repo,
    const std::string& base_tree_oid_hex,
    const std::vector<std::pair<std::string,
                                std::pair<std::vector<uint8_t>, uint32_t>>>& writes,
    const std::vector<std::string>& removes);

std::string write_commit(git_repository* repo,
                          const std::string& tree_oid_hex,
                          const std::vector<std::string>& parent_oids,
                          const Signature& sig,
                          const std::string& message);

std::string tree_oid_for_commit(git_repository* repo,
                                 const std::string& commit_oid_hex);

struct CommitMeta {
    std::string message;
    uint64_t    time;
    std::string author_name;
    std::string author_email;
    std::string parent_oid_hex;
    std::string tree_oid_hex;
};

CommitMeta read_commit(git_repository* repo,
                        const std::string& commit_oid_hex);

} // namespace tree

// ---------------------------------------------------------------------------
// glob — pattern matching helpers
// ---------------------------------------------------------------------------

namespace glob {

/// Match a glob pattern segment against a name.
bool fnmatch(const std::string& pattern, const std::string& name);

/// Match a glob pattern against a name (dot-awareness).
bool glob_match(const std::string& pattern, const std::string& name);

} // namespace glob

// ---------------------------------------------------------------------------
// copy — disk ↔ repo helpers
// ---------------------------------------------------------------------------

namespace copy {

/// Walk a local directory recursively, returning relative paths.
std::vector<std::string>
disk_walk(const std::filesystem::path& root);

/// Check if a relative path matches include/exclude filter sets.
bool matches_filters(const std::string& path,
                     const std::optional<std::vector<std::string>>& include,
                     const std::optional<std::vector<std::string>>& exclude);

/// Detect git mode from a local file's metadata.
uint32_t mode_from_disk(const std::filesystem::path& p);

} // namespace copy

} // namespace vost
