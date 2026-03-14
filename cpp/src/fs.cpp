#include "vost/fs.h"
#include "vost/batch.h"
#include "vost/gitstore.h"
#include "internal.h"

#include <git2.h>

#include <algorithm>
#include <cstring>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>

namespace vost {

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

namespace {
[[noreturn]] void throw_git(const std::string& ctx) {
    const git_error* e = git_error_last();
    std::string msg = ctx;
    if (e && e->message) { msg += ": "; msg += e->message; }
    throw GitError(msg);
}
} // anonymous namespace

// ---------------------------------------------------------------------------
// Fs constructors
// ---------------------------------------------------------------------------

Fs::Fs(std::shared_ptr<GitStoreInner> inner,
       std::string commit_oid_hex,
       std::string tree_oid_hex,
       std::optional<std::string> ref_name,
       bool writable,
       std::optional<ChangeReport> changes)
    : inner_(std::move(inner))
    , commit_oid_hex_(std::move(commit_oid_hex))
    , tree_oid_hex_(std::move(tree_oid_hex))
    , ref_name_(std::move(ref_name))
    , writable_(writable)
    , changes_(std::move(changes))
{}

Fs Fs::from_commit(std::shared_ptr<GitStoreInner> inner,
                    const std::string& commit_oid_hex,
                    std::optional<std::string> ref_name,
                    bool writable) {
    std::string tree_hex;
    {
        std::lock_guard<std::mutex> lk(inner->mutex);
        tree_hex = tree::tree_oid_for_commit(inner->repo, commit_oid_hex);
    }
    return Fs(std::move(inner), commit_oid_hex, tree_hex,
              std::move(ref_name), writable);
}

Fs Fs::empty(std::shared_ptr<GitStoreInner> inner, std::string ref_name) {
    return Fs(std::move(inner), "", "", std::move(ref_name), true);
}

// ---------------------------------------------------------------------------
// Identity / metadata
// ---------------------------------------------------------------------------

std::optional<std::string> Fs::commit_hash() const {
    if (commit_oid_hex_.empty()) return std::nullopt;
    return commit_oid_hex_;
}

std::optional<std::string> Fs::tree_hash() const {
    if (tree_oid_hex_.empty()) return std::nullopt;
    return tree_oid_hex_;
}

std::string Fs::message() const {
    if (commit_oid_hex_.empty())
        throw NotFoundError("no commit in snapshot");
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::read_commit(inner_->repo, commit_oid_hex_).message;
}

uint64_t Fs::time() const {
    if (commit_oid_hex_.empty())
        throw NotFoundError("no commit in snapshot");
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::read_commit(inner_->repo, commit_oid_hex_).time;
}

std::string Fs::author_name() const {
    if (commit_oid_hex_.empty())
        throw NotFoundError("no commit in snapshot");
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::read_commit(inner_->repo, commit_oid_hex_).author_name;
}

std::string Fs::author_email() const {
    if (commit_oid_hex_.empty())
        throw NotFoundError("no commit in snapshot");
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::read_commit(inner_->repo, commit_oid_hex_).author_email;
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

const std::string& Fs::require_writable(const std::string& verb) const {
    if (!writable_) {
        if (ref_name_) {
            throw PermissionError("cannot " + verb +
                                  " read-only snapshot (ref \"" + *ref_name_ + "\")");
        } else {
            throw PermissionError("cannot " + verb + " read-only snapshot");
        }
    }
    if (!ref_name_) {
        throw PermissionError("cannot " + verb + " without a branch");
    }
    return *ref_name_;
}

const std::string& Fs::require_tree() const {
    if (tree_oid_hex_.empty())
        throw NotFoundError("no tree in snapshot");
    return tree_oid_hex_;
}

// ---------------------------------------------------------------------------
// Read operations
// ---------------------------------------------------------------------------

std::vector<uint8_t> Fs::read(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::read_blob(inner_->repo, tree, norm);
}

std::string Fs::read_text(const std::string& path) const {
    auto data = read(path);
    return std::string(data.begin(), data.end());
}

std::vector<std::string> Fs::ls(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entries = tree::list_tree(inner_->repo, tree, norm);
    std::vector<std::string> names;
    names.reserve(entries.size());
    for (auto& e : entries) names.push_back(std::move(e.name));
    return names;
}

std::vector<WalkDirEntry>
Fs::walk(const std::string& path) const {
    const auto& tree_hex = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::walk_tree_dirs(inner_->repo, tree_hex, norm);
}

bool Fs::exists(const std::string& path) const {
    if (tree_oid_hex_.empty()) return false;
    std::string norm = paths::normalize(path);
    if (norm.empty()) return true; // root always exists
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree_oid_hex_, norm);
    return entry.has_value();
}

bool Fs::is_dir(const std::string& path) const {
    if (tree_oid_hex_.empty()) return false;
    std::string norm = paths::normalize(path);
    if (norm.empty()) return true;
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree_oid_hex_, norm);
    if (!entry) return false;
    return entry->second == MODE_TREE;
}

FileType Fs::file_type(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree, norm);
    if (!entry) throw NotFoundError(path);
    auto ft = file_type_from_mode(entry->second);
    if (!ft) throw GitError("unknown mode for: " + path);
    return *ft;
}

uint64_t Fs::size(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree, norm);
    if (!entry) throw NotFoundError(path);
    if (entry->second == MODE_TREE) throw IsADirectoryError(path);

    git_oid oid;
    if (git_oid_fromstr(&oid, entry->first.c_str()) != 0)
        throw InvalidHashError(entry->first);
    git_blob* blob = nullptr;
    if (git_blob_lookup(&blob, inner_->repo, &oid) != 0)
        throw_git("git_blob_lookup");
    uint64_t sz = static_cast<uint64_t>(git_blob_rawsize(blob));
    git_blob_free(blob);
    return sz;
}

std::string Fs::object_hash(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree, norm);
    if (!entry) throw NotFoundError(path);
    return entry->first;
}

std::string Fs::readlink(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto entry = tree::lookup(inner_->repo, tree, norm);
    if (!entry) throw NotFoundError(path);
    if (entry->second != MODE_LINK)
        throw InvalidPathError(path + " is not a symlink");
    auto data = tree::read_blob(inner_->repo, tree, norm);
    return std::string(data.begin(), data.end());
}

StatResult Fs::stat(const std::string& path) const {
    const auto& tree_hex = require_tree();
    uint64_t mtime_val = commit_oid_hex_.empty() ? 0 : time();

    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);

    if (norm.empty()) {
        uint32_t nlink = 2 + tree::count_subdirs(inner_->repo, tree_hex);
        return StatResult{MODE_TREE, FileType::Tree, 0, tree_hex, nlink, mtime_val};
    }

    auto entry = tree::lookup(inner_->repo, tree_hex, norm);
    if (!entry) throw NotFoundError(path);

    auto ft = file_type_from_mode(entry->second);
    if (!ft) throw GitError("unknown mode for: " + path);

    if (entry->second == MODE_TREE) {
        uint32_t nlink = 2 + tree::count_subdirs(inner_->repo, entry->first);
        return StatResult{entry->second, *ft, 0, entry->first, nlink, mtime_val};
    }

    git_oid oid;
    if (git_oid_fromstr(&oid, entry->first.c_str()) != 0)
        throw InvalidHashError(entry->first);
    git_blob* blob = nullptr;
    if (git_blob_lookup(&blob, inner_->repo, &oid) != 0)
        throw_git("git_blob_lookup");
    uint64_t sz = static_cast<uint64_t>(git_blob_rawsize(blob));
    git_blob_free(blob);

    return StatResult{entry->second, *ft, sz, entry->first, 1, mtime_val};
}

std::vector<WalkEntry> Fs::listdir(const std::string& path) const {
    const auto& tree = require_tree();
    std::string norm = paths::normalize(path);
    std::lock_guard<std::mutex> lk(inner_->mutex);
    return tree::list_tree(inner_->repo, tree, norm);
}

std::vector<uint8_t> Fs::read_range(const std::string& path,
                                     size_t offset,
                                     std::optional<size_t> sz) const {
    auto data = read(path);
    size_t start = std::min(offset, data.size());
    size_t end   = sz ? std::min(start <= SIZE_MAX - *sz ? start + *sz : SIZE_MAX,
                                 data.size())
                      : data.size();
    return std::vector<uint8_t>(data.begin() + static_cast<ptrdiff_t>(start),
                                data.begin() + static_cast<ptrdiff_t>(end));
}

std::vector<uint8_t> Fs::read_by_hash(const std::string& hash,
                                       size_t offset,
                                       std::optional<size_t> sz) const {
    git_oid oid;
    if (git_oid_fromstr(&oid, hash.c_str()) != 0)
        throw InvalidHashError(hash);

    std::lock_guard<std::mutex> lk(inner_->mutex);
    git_blob* blob = nullptr;
    if (git_blob_lookup(&blob, inner_->repo, &oid) != 0)
        throw_git("git_blob_lookup");

    const void* raw = git_blob_rawcontent(blob);
    size_t       rawsz = static_cast<size_t>(git_blob_rawsize(blob));
    auto ptr = static_cast<const uint8_t*>(raw);
    std::vector<uint8_t> data(ptr, ptr + rawsz);
    git_blob_free(blob);

    size_t start = std::min(offset, data.size());
    size_t end   = sz ? std::min(start <= SIZE_MAX - *sz ? start + *sz : SIZE_MAX,
                                 data.size())
                      : data.size();
    return std::vector<uint8_t>(data.begin() + static_cast<ptrdiff_t>(start),
                                data.begin() + static_cast<ptrdiff_t>(end));
}

// ---------------------------------------------------------------------------
// Write helpers
// ---------------------------------------------------------------------------

Fs Fs::commit_changes(
    const std::vector<std::pair<std::string,
                                std::pair<std::vector<uint8_t>, uint32_t>>>& writes,
    const std::vector<std::string>& removes,
    const std::string& message,
    std::optional<ChangeReport> report,
    const std::vector<std::string>& extra_parent_oids) const
{
    const std::string& ref = require_writable("write");
    std::string refname = "refs/heads/" + ref;

    std::string new_commit_hex;
    std::string new_tree_hex;

    // Hold the repo lock while rebuilding tree + creating commit + CAS ref update
    lock::with_repo_lock(inner_->path, [&]() {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // CAS check: branch tip must still match our commit_oid
        {
            git_reference* cur_ref = nullptr;
            if (git_reference_lookup(&cur_ref, inner_->repo, refname.c_str()) == 0) {
                git_object* obj = nullptr;
                git_reference_peel(&obj, cur_ref, GIT_OBJECT_COMMIT);
                git_reference_free(cur_ref);
                if (obj) {
                    char buf[GIT_OID_HEXSZ + 1];
                    git_oid_tostr(buf, sizeof(buf), git_object_id(obj));
                    git_object_free(obj);
                    std::string cur_hex(buf, GIT_OID_HEXSZ);
                    if (cur_hex != commit_oid_hex_) {
                        throw StaleSnapshotError(
                            "branch '" + ref + "' has advanced (concurrent write)");
                    }
                }
            }
        }

        // Rebuild tree
        std::string base_tree = tree_oid_hex_.empty()
            ? std::string(GIT_OID_HEXSZ, '0')
            : tree_oid_hex_;

        new_tree_hex = tree::rebuild_tree(inner_->repo, base_tree, writes, removes);

        // Create commit — build full parents list (branch tip + extras)
        std::vector<std::string> all_parents;
        if (!commit_oid_hex_.empty()) {
            all_parents.push_back(commit_oid_hex_);
        }
        all_parents.insert(all_parents.end(),
                           extra_parent_oids.begin(),
                           extra_parent_oids.end());
        new_commit_hex = tree::write_commit(inner_->repo, new_tree_hex,
                                             all_parents,
                                             inner_->signature,
                                             message);

        // Update ref (CAS)
        git_oid new_oid;
        if (git_oid_fromstr(&new_oid, new_commit_hex.c_str()) != 0)
            throw GitError("invalid new commit oid");

        git_reference* out_ref = nullptr;
        int rc;
        if (!commit_oid_hex_.empty()) {
            // CAS update: old must be commit_oid_hex_
            git_oid old_oid;
            if (git_oid_fromstr(&old_oid, commit_oid_hex_.c_str()) != 0)
                throw GitError("invalid old commit oid");

            git_reference* existing = nullptr;
            if (git_reference_lookup(&existing, inner_->repo, refname.c_str()) == 0) {
                rc = git_reference_set_target(&out_ref, existing, &new_oid, message.c_str());
                git_reference_free(existing);
            } else {
                rc = git_reference_create(&out_ref, inner_->repo,
                                          refname.c_str(), &new_oid,
                                          0 /*no force*/, message.c_str());
            }
        } else {
            // Initial commit — create ref
            rc = git_reference_create(&out_ref, inner_->repo,
                                       refname.c_str(), &new_oid,
                                       0 /*no force*/, message.c_str());
        }
        if (out_ref) git_reference_free(out_ref);
        if (rc != 0) throw_git("git_reference update");
    });

    return Fs(inner_, new_commit_hex, new_tree_hex, ref_name_, true, std::move(report));
}

// ---------------------------------------------------------------------------
// Glob
// ---------------------------------------------------------------------------

namespace {

/// Recursive iglob helper. Operates on a tree OID and pattern segments.
void iglob_recursive(git_repository* repo,
                     const std::string& tree_oid_hex,
                     const std::vector<std::string>& segments,
                     size_t seg_idx,
                     const std::string& prefix,
                     std::vector<std::string>& results) {
    if (seg_idx >= segments.size()) return;

    const std::string& seg = segments[seg_idx];
    auto entries = tree::list_tree_by_oid(repo, tree_oid_hex);

    if (seg == "**") {
        // Match zero directory levels: try remaining segments at this level
        iglob_recursive(repo, tree_oid_hex, segments, seg_idx + 1,
                        prefix, results);

        // Match one or more directory levels: descend into non-dotfile dirs
        for (auto& e : entries) {
            if (e.name.empty() || e.name[0] == '.') continue;
            std::string full = prefix.empty() ? e.name : prefix + "/" + e.name;
            if (e.mode == MODE_TREE) {
                iglob_recursive(repo, e.oid, segments, seg_idx,
                                full, results);
            }
        }
    } else {
        bool is_last = (seg_idx + 1 == segments.size());
        for (auto& e : entries) {
            if (!glob::glob_match(seg, e.name)) continue;
            std::string full = prefix.empty() ? e.name : prefix + "/" + e.name;
            if (is_last) {
                // Last segment: add files only (not directories)
                if (e.mode != MODE_TREE) {
                    results.push_back(full);
                }
            } else if (e.mode == MODE_TREE) {
                // More segments: recurse into directories
                iglob_recursive(repo, e.oid, segments, seg_idx + 1,
                                full, results);
            }
        }
    }
}

} // anonymous namespace

std::vector<std::string> Fs::iglob(const std::string& pattern) const {
    const auto& tree_hex = require_tree();

    // Split pattern by '/'
    std::vector<std::string> segments;
    {
        std::istringstream iss(pattern);
        std::string seg;
        while (std::getline(iss, seg, '/')) {
            if (!seg.empty()) segments.push_back(seg);
        }
    }
    if (segments.empty()) return {};

    std::vector<std::string> results;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        iglob_recursive(inner_->repo, tree_hex, segments, 0, "", results);
    }
    return results;
}

std::vector<std::string> Fs::glob(const std::string& pattern) const {
    auto results = iglob(pattern);
    std::sort(results.begin(), results.end());
    return results;
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

Fs Fs::write(const std::string& path,
              const std::vector<uint8_t>& data,
              WriteOptions opts) const {
    std::string norm = paths::normalize(path);
    uint32_t mode = opts.mode.value_or(MODE_BLOB);
    std::string msg = paths::format_message("write: " + norm, opts.message);

    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    writes.push_back({norm, {data, mode}});
    return commit_changes(writes, {}, msg, std::nullopt, opts.parents);
}

Fs Fs::write_text(const std::string& path,
                   const std::string& text,
                   WriteOptions opts) const {
    std::vector<uint8_t> data(text.begin(), text.end());
    return write(path, data, std::move(opts));
}

Fs Fs::write_from_file(const std::string& path,
                        const std::filesystem::path& local_path,
                        WriteOptions opts) const {
    namespace fss = std::filesystem;
    if (!fss::exists(local_path)) {
        throw IoError("file not found: " + local_path.string());
    }

    std::ifstream ifs(local_path, std::ios::binary);
    if (!ifs) {
        throw IoError("cannot open file: " + local_path.string());
    }
    std::vector<uint8_t> data{std::istreambuf_iterator<char>(ifs),
                               std::istreambuf_iterator<char>()};

    uint32_t mode = opts.mode.value_or(copy::mode_from_disk(local_path));
    opts.mode = mode;
    return write(path, data, std::move(opts));
}

Fs Fs::write_symlink(const std::string& path,
                      const std::string& target,
                      WriteOptions opts) const {
    std::string norm = paths::normalize(path);
    std::string msg = paths::format_message("symlink: " + norm, opts.message);
    std::vector<uint8_t> data(target.begin(), target.end());

    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    writes.push_back({norm, {data, MODE_LINK}});
    return commit_changes(writes, {}, msg, std::nullopt, opts.parents);
}

Fs Fs::apply(const std::vector<std::pair<std::string, WriteEntry>>& writes,
              const std::vector<std::string>& removes,
              ApplyOptions opts) const {
    std::string msg = paths::format_message(opts.operation.value_or("apply"), opts.message);

    std::vector<std::pair<std::string,
                          std::pair<std::vector<uint8_t>, uint32_t>>> internal;
    internal.reserve(writes.size());
    for (auto& [p, we] : writes) {
        we.validate();
        std::string norm = paths::normalize(p);
        std::vector<uint8_t> data;
        if (we.data) data = *we.data;
        else if (we.target) data = std::vector<uint8_t>(
            we.target->begin(), we.target->end());
        internal.push_back({norm, {data, we.mode}});
    }

    std::vector<std::string> norm_removes;
    norm_removes.reserve(removes.size());
    for (auto& r : removes) norm_removes.push_back(paths::normalize(r));

    return commit_changes(internal, norm_removes, msg, std::nullopt, opts.parents);
}

Fs Fs::remove(const std::vector<std::string>& paths_in, RemoveOptions opts) const {
    require_writable("remove");
    const auto& tree_hex = require_tree();
    std::string msg = paths::format_message("remove", opts.message);

    std::vector<std::string> to_remove;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        for (auto& p : paths_in) {
            std::string norm = paths::normalize(p);
            auto entry = tree::lookup(inner_->repo, tree_hex, norm);
            if (!entry) throw NotFoundError(norm);

            if (entry->second == MODE_TREE) {
                if (!opts.recursive) {
                    throw IsADirectoryError(norm);
                }
                // Remove the directory entry itself (rebuild_tree handles subtree)
                to_remove.push_back(norm);
            } else {
                to_remove.push_back(norm);
            }
        }
    }

    if (opts.dry_run) {
        return *this;
    }

    return commit_changes({}, to_remove, msg, std::nullopt, opts.parents);
}

// ---------------------------------------------------------------------------
// Move
// ---------------------------------------------------------------------------

Fs Fs::move(const std::vector<std::string>& sources,
                   const std::string& dest,
                   MoveOptions opts) const {
    require_writable("move");
    const auto& tree_hex = require_tree();
    std::string norm_dest = paths::normalize(dest);

    if (sources.empty()) {
        throw InvalidPathError("move: no sources provided");
    }

    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    std::vector<std::string> removes;

    {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // Check if dest is an existing directory
        bool dest_is_dir = false;
        if (!norm_dest.empty()) {
            auto dest_entry = tree::lookup(inner_->repo, tree_hex, norm_dest);
            if (dest_entry && dest_entry->second == MODE_TREE) {
                dest_is_dir = true;
            }
        } else {
            dest_is_dir = true; // root is always a directory
        }

        // Multiple sources → dest must be a directory
        if (sources.size() > 1 && !dest_is_dir) {
            throw NotADirectoryError("move: destination must be a directory for multiple sources");
        }

        for (auto& src : sources) {
            std::string norm_src = paths::normalize(src);
            if (norm_src.empty()) {
                throw InvalidPathError("cannot move root");
            }

            auto entry = tree::lookup(inner_->repo, tree_hex, norm_src);
            if (!entry) throw NotFoundError(norm_src);

            // Determine the target path
            std::string target;
            if (dest_is_dir) {
                // Move into directory: use source basename
                auto slash = norm_src.rfind('/');
                std::string basename = (slash != std::string::npos)
                    ? norm_src.substr(slash + 1) : norm_src;
                target = norm_dest.empty() ? basename : norm_dest + "/" + basename;
            } else {
                // Single source, dest doesn't exist as dir → rename
                target = norm_dest;
            }

            if (entry->second == MODE_TREE) {
                if (!opts.recursive) {
                    throw IsADirectoryError(norm_src);
                }
                // Walk all children under the source directory
                auto children = tree::walk_tree(inner_->repo, tree_hex, norm_src);
                for (auto& [rel_path, we] : children) {
                    std::string new_path = target + rel_path.substr(norm_src.size());
                    auto data = tree::read_blob(inner_->repo, tree_hex, rel_path);
                    writes.push_back({new_path, {std::move(data), we.mode}});
                }
                removes.push_back(norm_src);
            } else {
                auto data = tree::read_blob(inner_->repo, tree_hex, norm_src);
                writes.push_back({target, {std::move(data), entry->second}});
                removes.push_back(norm_src);
            }
        }
    }

    if (opts.dry_run) {
        return *this;
    }

    std::string msg = paths::format_message("move", opts.message);
    return commit_changes(writes, removes, msg, std::nullopt, opts.parents);
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

Batch Fs::batch(BatchOptions opts) const {
    return Batch(*this, std::move(opts));
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

std::optional<Fs> Fs::parent() const {
    if (commit_oid_hex_.empty()) return std::nullopt;
    std::lock_guard<std::mutex> lk(inner_->mutex);
    auto meta = tree::read_commit(inner_->repo, commit_oid_hex_);
    if (meta.parent_oid_hex.empty()) return std::nullopt;
    std::string parent_tree;
    {
        git_oid poid;
        if (git_oid_fromstr(&poid, meta.parent_oid_hex.c_str()) != 0)
            return std::nullopt;
        git_commit* c = nullptr;
        if (git_commit_lookup(&c, inner_->repo, &poid) != 0) return std::nullopt;
        char buf[GIT_OID_HEXSZ + 1];
        git_oid_tostr(buf, sizeof(buf), git_commit_tree_id(c));
        parent_tree = std::string(buf, GIT_OID_HEXSZ);
        git_commit_free(c);
    }
    return Fs(inner_, meta.parent_oid_hex, parent_tree, ref_name_, writable_);
}

Fs Fs::back(size_t n) const {
    Fs cur = *this;
    for (size_t i = 0; i < n; ++i) {
        auto p = cur.parent();
        if (!p) throw NotFoundError("not enough history (requested " +
                                     std::to_string(n) + " commits back)");
        cur = std::move(*p);
    }
    return cur;
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

Fs Fs::rename(const std::string& src, const std::string& dest,
              WriteOptions opts) const {
    require_writable("rename");
    const auto& tree_hex = require_tree();
    std::string norm_src = paths::normalize(src);
    std::string norm_dest = paths::normalize(dest);

    if (norm_src.empty()) throw InvalidPathError("cannot rename root");
    if (norm_dest.empty()) throw InvalidPathError("cannot rename to root");

    std::string msg = paths::format_message(
        "rename: " + norm_src + " -> " + norm_dest, opts.message);

    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    std::vector<std::string> removes;

    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        auto entry = tree::lookup(inner_->repo, tree_hex, norm_src);
        if (!entry) throw NotFoundError(norm_src);

        if (entry->second == MODE_TREE) {
            // Directory: walk all children, move them
            auto children = tree::walk_tree(inner_->repo, tree_hex, norm_src);
            for (auto& [rel_path, we] : children) {
                // rel_path is like "src/child" — replace src prefix with dest
                std::string new_path = norm_dest + rel_path.substr(norm_src.size());
                auto data = tree::read_blob(inner_->repo, tree_hex, rel_path);
                writes.push_back({new_path, {std::move(data), we.mode}});
            }
            // Remove the source directory entry itself (rebuild_tree removes the subtree)
            removes.push_back(norm_src);
        } else {
            // File/symlink: read data, write at new path, remove old
            auto data = tree::read_blob(inner_->repo, tree_hex, norm_src);
            uint32_t mode = opts.mode.value_or(entry->second);
            writes.push_back({norm_dest, {std::move(data), mode}});
            removes.push_back(norm_src);
        }
    }

    return commit_changes(writes, removes, msg, std::nullopt, opts.parents);
}

// ---------------------------------------------------------------------------
// Undo / Redo
// ---------------------------------------------------------------------------

Fs Fs::undo(size_t n) const {
    const std::string& ref = require_writable("undo");
    if (commit_oid_hex_.empty())
        throw NotFoundError("no commit to undo");
    if (n == 0) return *this;

    // Walk back n parents to find the target commit
    std::string target_hex;
    std::string target_tree_hex;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        std::string cur_hex = commit_oid_hex_;
        for (size_t i = 0; i < n; ++i) {
            auto meta = tree::read_commit(inner_->repo, cur_hex);
            if (meta.parent_oid_hex.empty())
                throw NotFoundError("not enough history to undo " +
                                     std::to_string(n) + " commit(s)");
            cur_hex = meta.parent_oid_hex;
        }
        target_hex = cur_hex;
        target_tree_hex = tree::tree_oid_for_commit(inner_->repo, target_hex);
    }

    std::string refname = "refs/heads/" + ref;

    lock::with_repo_lock(inner_->path, [&]() {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // Stale-snapshot check
        {
            git_reference* cur_ref = nullptr;
            if (git_reference_lookup(&cur_ref, inner_->repo, refname.c_str()) == 0) {
                git_object* obj = nullptr;
                git_reference_peel(&obj, cur_ref, GIT_OBJECT_COMMIT);
                git_reference_free(cur_ref);
                if (obj) {
                    char buf[GIT_OID_HEXSZ + 1];
                    git_oid_tostr(buf, sizeof(buf), git_object_id(obj));
                    git_object_free(obj);
                    std::string cur_hex(buf, GIT_OID_HEXSZ);
                    if (cur_hex != commit_oid_hex_) {
                        throw StaleSnapshotError(
                            "branch '" + ref + "' has advanced (concurrent write)");
                    }
                }
            }
        }

        // Update ref to target
        git_oid target_oid;
        if (git_oid_fromstr(&target_oid, target_hex.c_str()) != 0)
            throw GitError("invalid target oid");

        git_reference* existing = nullptr;
        if (git_reference_lookup(&existing, inner_->repo, refname.c_str()) != 0)
            throw_git("git_reference_lookup");

        git_reference* out_ref = nullptr;
        std::string msg = "undo: " + std::to_string(n) + " commit(s)";
        int rc = git_reference_set_target(&out_ref, existing, &target_oid, msg.c_str());
        git_reference_free(existing);
        if (out_ref) git_reference_free(out_ref);
        if (rc != 0) throw_git("git_reference_set_target (undo)");
    });

    return Fs(inner_, target_hex, target_tree_hex, ref_name_, true);
}

Fs Fs::redo(size_t n) const {
    const std::string& ref = require_writable("redo");
    if (n == 0) return *this;

    std::string refname = "refs/heads/" + ref;
    std::string target_hex;
    std::string target_tree_hex;

    // Read the reflog to find redo targets.
    // After an undo, the reflog has an entry where new_sha == (current commit).
    // The old_sha of that entry is the commit we want to redo to.
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        git_reflog* rlog = nullptr;
        if (git_reflog_read(&rlog, inner_->repo, refname.c_str()) != 0)
            throw NotFoundError("no reflog for redo");

        // RAII guard for reflog
        struct ReflogGuard {
            git_reflog* r;
            ~ReflogGuard() { if (r) git_reflog_free(r); }
        } rg{rlog};

        size_t entry_count = git_reflog_entrycount(rlog);
        std::string cur_hex = commit_oid_hex_.empty()
            ? std::string(GIT_OID_HEXSZ, '0') : commit_oid_hex_;

        size_t redo_found = 0;
        for (size_t i = 0; i < entry_count && redo_found < n; ++i) {
            const git_reflog_entry* e = git_reflog_entry_byindex(rlog, i);

            // Only consider entries from undo/redo operations
            const char* msg_raw = git_reflog_entry_message(e);
            std::string entry_msg = msg_raw ? msg_raw : "";
            if (entry_msg.substr(0, 5) != "undo:" &&
                entry_msg.substr(0, 5) != "redo:") {
                continue;
            }

            char new_buf[GIT_OID_HEXSZ + 1];
            git_oid_tostr(new_buf, sizeof(new_buf),
                          git_reflog_entry_id_new(e));
            std::string entry_new(new_buf, GIT_OID_HEXSZ);

            if (entry_new == cur_hex) {
                char old_buf[GIT_OID_HEXSZ + 1];
                git_oid_tostr(old_buf, sizeof(old_buf),
                              git_reflog_entry_id_old(e));
                std::string entry_old(old_buf, GIT_OID_HEXSZ);

                if (entry_old != std::string(GIT_OID_HEXSZ, '0')) {
                    cur_hex = entry_old;
                    ++redo_found;
                }
            }
        }

        if (redo_found < n)
            throw NotFoundError("not enough redo history");

        target_hex = cur_hex;
        target_tree_hex = tree::tree_oid_for_commit(inner_->repo, target_hex);
    }

    lock::with_repo_lock(inner_->path, [&]() {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // Stale-snapshot check
        {
            git_reference* cur_ref = nullptr;
            if (git_reference_lookup(&cur_ref, inner_->repo, refname.c_str()) == 0) {
                git_object* obj = nullptr;
                git_reference_peel(&obj, cur_ref, GIT_OBJECT_COMMIT);
                git_reference_free(cur_ref);
                if (obj) {
                    char buf[GIT_OID_HEXSZ + 1];
                    git_oid_tostr(buf, sizeof(buf), git_object_id(obj));
                    git_object_free(obj);
                    std::string cur_hex(buf, GIT_OID_HEXSZ);
                    if (cur_hex != commit_oid_hex_) {
                        throw StaleSnapshotError(
                            "branch '" + ref + "' has advanced (concurrent write)");
                    }
                }
            }
        }

        // Update ref to target
        git_oid target_oid;
        if (git_oid_fromstr(&target_oid, target_hex.c_str()) != 0)
            throw GitError("invalid target oid");

        git_reference* existing = nullptr;
        if (git_reference_lookup(&existing, inner_->repo, refname.c_str()) != 0)
            throw_git("git_reference_lookup");

        git_reference* out_ref = nullptr;
        std::string msg = "redo: " + std::to_string(n) + " commit(s)";
        int rc = git_reference_set_target(&out_ref, existing, &target_oid, msg.c_str());
        git_reference_free(existing);
        if (out_ref) git_reference_free(out_ref);
        if (rc != 0) throw_git("git_reference_set_target (redo)");
    });

    return Fs(inner_, target_hex, target_tree_hex, ref_name_, true);
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

std::vector<CommitInfo> Fs::log(LogOptions opts) const {
    if (commit_oid_hex_.empty()) return {};

    std::vector<CommitInfo> results;
    size_t skipped = 0;
    std::string cur_hex = commit_oid_hex_;

    std::lock_guard<std::mutex> lk(inner_->mutex);

    while (!cur_hex.empty()) {
        auto meta = tree::read_commit(inner_->repo, cur_hex);

        // Apply filters (AND logic)
        bool matches = true;

        if (matches && opts.before) {
            if (meta.time > *opts.before) matches = false;
        }

        if (matches && opts.match_pattern) {
            if (!glob::glob_match(*opts.match_pattern, meta.message))
                matches = false;
        }

        if (matches && opts.path) {
            // Compare entry at path between this commit and its parent
            std::string norm_path = paths::normalize(*opts.path);
            auto this_entry = tree::lookup(inner_->repo, meta.tree_oid_hex, norm_path);

            if (!meta.parent_oid_hex.empty()) {
                auto parent_meta = tree::read_commit(inner_->repo, meta.parent_oid_hex);
                auto parent_entry = tree::lookup(inner_->repo, parent_meta.tree_oid_hex, norm_path);

                // Match if entry differs (oid OR mode) between parent and this commit
                if (this_entry && parent_entry) {
                    if (this_entry->first == parent_entry->first &&
                        this_entry->second == parent_entry->second) {
                        matches = false;
                    }
                } else if (!this_entry && !parent_entry) {
                    matches = false;
                }
                // else: one exists and the other doesn't → it changed → matches
            }
            // Initial commit: if file exists in this commit, it was added → matches
            // If file doesn't exist in initial commit → doesn't match
            else if (!this_entry) {
                matches = false;
            }
        }

        if (matches) {
            if (opts.skip && skipped < *opts.skip) {
                ++skipped;
            } else {
                CommitInfo ci;
                ci.commit_hash = cur_hex;
                ci.message     = meta.message;
                ci.time        = meta.time;
                ci.author_name = meta.author_name;
                ci.author_email = meta.author_email;
                results.push_back(std::move(ci));

                if (opts.limit && results.size() >= *opts.limit) break;
            }
        }

        cur_hex = meta.parent_oid_hex;
    }

    return results;
}

// ---------------------------------------------------------------------------
// FsWriter
// ---------------------------------------------------------------------------

FsWriter::FsWriter(Fs fs, std::string path, WriteOptions opts)
    : fs_(std::move(fs))
    , path_(std::move(path))
    , opts_(std::move(opts))
{}

FsWriter::~FsWriter() {
    if (!closed_) {
        try { close(); } catch (...) {}
    }
}

FsWriter& FsWriter::write(const std::vector<uint8_t>& data) {
    if (closed_) throw BatchClosedError();
    buffer_.insert(buffer_.end(), data.begin(), data.end());
    return *this;
}

FsWriter& FsWriter::write(const std::string& text) {
    if (closed_) throw BatchClosedError();
    buffer_.insert(buffer_.end(), text.begin(), text.end());
    return *this;
}

Fs FsWriter::close() {
    if (closed_) throw BatchClosedError();
    closed_ = true;
    fs_ = fs_.write(path_, buffer_, opts_);
    return fs_;
}

} // namespace vost
