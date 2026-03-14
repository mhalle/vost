#include "internal.h"
#include "vost/error.h"
#include "vost/types.h"

#include <git2.h>

#include <algorithm>
#include <cassert>
#include <cstring>
#include <map>
#include <sstream>
#include <string>
#include <vector>

namespace vost {

// ---------------------------------------------------------------------------
// Internal helpers — declared here but not in a public header
// ---------------------------------------------------------------------------

namespace {

/// Convert a 20-byte raw OID to a 40-char lowercase hex string.
std::string oid_to_hex(const git_oid* oid) {
    char buf[GIT_OID_HEXSZ + 1];
    git_oid_tostr(buf, sizeof(buf), oid);
    return std::string(buf, GIT_OID_HEXSZ);
}

/// Parse a 40-char hex string into a git_oid.
/// @throws InvalidHashError on failure.
git_oid hex_to_oid(const std::string& hex) {
    git_oid oid;
    if (git_oid_fromstr(&oid, hex.c_str()) != 0) {
        throw InvalidHashError(hex);
    }
    return oid;
}

/// Throw GitError with the last libgit2 error message.
[[noreturn]] void throw_git_error(const std::string& context) {
    const git_error* err = git_error_last();
    std::string msg = context;
    if (err && err->message) {
        msg += ": ";
        msg += err->message;
    }
    throw GitError(msg);
}


/// RAII wrapper for git_tree*.
struct TreeGuard {
    git_tree* t = nullptr;
    ~TreeGuard() { if (t) git_tree_free(t); }
};

/// RAII wrapper for git_blob*.
struct BlobGuard {
    git_blob* b = nullptr;
    ~BlobGuard() { if (b) git_blob_free(b); }
};

/// RAII wrapper for git_commit*.
struct CommitGuard {
    git_commit* c = nullptr;
    ~CommitGuard() { if (c) git_commit_free(c); }
};

/// RAII wrapper for git_treebuilder*.
struct BuilderGuard {
    git_treebuilder* tb = nullptr;
    ~BuilderGuard() { if (tb) git_treebuilder_free(tb); }
};

// ---------------------------------------------------------------------------
// entry_at_path — walk tree to a path, return oid + mode
// ---------------------------------------------------------------------------

struct EntryResult {
    std::string oid_hex;
    uint32_t    mode;
};

/// Return the (oid, mode) of the entry at `norm_path`, or nullopt if missing.
std::optional<EntryResult>
entry_at_path(git_repository* repo,
              const std::string& tree_oid_hex,
              const std::string& norm_path) {
    if (norm_path.empty()) {
        return EntryResult{tree_oid_hex, MODE_TREE};
    }

    // Split path into segments
    std::vector<std::string> segs;
    {
        std::istringstream ss(norm_path);
        std::string tok;
        while (std::getline(ss, tok, '/')) {
            if (!tok.empty()) segs.push_back(tok);
        }
    }

    git_oid cur_oid = hex_to_oid(tree_oid_hex);

    for (size_t i = 0; i < segs.size(); ++i) {
        TreeGuard tg;
        if (git_tree_lookup(&tg.t, repo, &cur_oid) != 0) {
            throw_git_error("git_tree_lookup");
        }

        const git_tree_entry* entry =
            git_tree_entry_byname(tg.t, segs[i].c_str());
        if (!entry) return std::nullopt;

        cur_oid = *git_tree_entry_id(entry);
        uint32_t mode = static_cast<uint32_t>(git_tree_entry_filemode(entry));

        if (i == segs.size() - 1) {
            return EntryResult{oid_to_hex(&cur_oid), mode};
        }
        // Intermediate must be a tree
        if (mode != MODE_TREE) return std::nullopt;
    }

    return std::nullopt; // unreachable
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// Public tree API (used by Fs, Batch)
// ---------------------------------------------------------------------------

namespace tree {

/// Return (oid_hex, mode) of `norm_path` in `tree_oid_hex`, or nullopt.
std::optional<std::pair<std::string, uint32_t>>
lookup(git_repository* repo,
       const std::string& tree_oid_hex,
       const std::string& norm_path) {
    auto res = entry_at_path(repo, tree_oid_hex, norm_path);
    if (!res) return std::nullopt;
    return std::make_pair(res->oid_hex, res->mode);
}

/// Read blob at `norm_path` or throw NotFoundError / IsADirectoryError.
std::vector<uint8_t>
read_blob(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path) {
    auto entry = entry_at_path(repo, tree_oid_hex, norm_path);
    if (!entry) throw NotFoundError(norm_path);
    if (entry->mode == MODE_TREE) throw IsADirectoryError(norm_path);

    git_oid oid = hex_to_oid(entry->oid_hex);
    BlobGuard bg;
    if (git_blob_lookup(&bg.b, repo, &oid) != 0) {
        throw_git_error("git_blob_lookup");
    }
    const void* raw = git_blob_rawcontent(bg.b);
    size_t       sz  = static_cast<size_t>(git_blob_rawsize(bg.b));
    auto ptr = static_cast<const uint8_t*>(raw);
    return std::vector<uint8_t>(ptr, ptr + sz);
}

/// List immediate children of the tree at `norm_path`.
std::vector<WalkEntry>
list_tree(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path) {
    std::string target_oid_hex = tree_oid_hex;
    if (!norm_path.empty()) {
        auto entry = entry_at_path(repo, tree_oid_hex, norm_path);
        if (!entry) throw NotFoundError(norm_path);
        if (entry->mode != MODE_TREE) throw NotADirectoryError(norm_path);
        target_oid_hex = entry->oid_hex;
    }

    git_oid oid = hex_to_oid(target_oid_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, repo, &oid) != 0) {
        throw_git_error("git_tree_lookup");
    }

    size_t n = git_tree_entrycount(tg.t);
    std::vector<WalkEntry> out;
    out.reserve(n);
    for (size_t i = 0; i < n; ++i) {
        const git_tree_entry* e = git_tree_entry_byindex(tg.t, i);
        WalkEntry we;
        we.name = git_tree_entry_name(e);
        we.oid  = oid_to_hex(git_tree_entry_id(e));
        we.mode = static_cast<uint32_t>(git_tree_entry_filemode(e));
        out.push_back(std::move(we));
    }
    return out;
}

/// List immediate children of a tree given its OID hex (no path lookup).
std::vector<WalkEntry>
list_tree_by_oid(git_repository* repo,
                 const std::string& tree_oid_hex) {
    git_oid oid = hex_to_oid(tree_oid_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, repo, &oid) != 0)
        throw_git_error("git_tree_lookup");

    size_t n = git_tree_entrycount(tg.t);
    std::vector<WalkEntry> out;
    out.reserve(n);
    for (size_t i = 0; i < n; ++i) {
        const git_tree_entry* e = git_tree_entry_byindex(tg.t, i);
        WalkEntry we;
        we.name = git_tree_entry_name(e);
        we.oid  = oid_to_hex(git_tree_entry_id(e));
        we.mode = static_cast<uint32_t>(git_tree_entry_filemode(e));
        out.push_back(std::move(we));
    }
    return out;
}

/// Recursively walk all leaf entries under `norm_path`.
/// Returns (rel_path, WalkEntry) pairs.
std::vector<std::pair<std::string, WalkEntry>>
walk_tree(git_repository* repo,
          const std::string& tree_oid_hex,
          const std::string& norm_path) {
    std::string target_oid_hex = tree_oid_hex;
    if (!norm_path.empty()) {
        auto entry = entry_at_path(repo, tree_oid_hex, norm_path);
        if (!entry) throw NotFoundError(norm_path);
        if (entry->mode != MODE_TREE) throw NotADirectoryError(norm_path);
        target_oid_hex = entry->oid_hex;
    }

    std::vector<std::pair<std::string, WalkEntry>> results;

    struct Ctx {
        git_repository* repo;
        std::vector<std::pair<std::string, WalkEntry>>* results;
    } ctx{repo, &results};

    git_oid root_oid = hex_to_oid(target_oid_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, repo, &root_oid) != 0) {
        throw_git_error("git_tree_lookup");
    }

    auto callback = [](const char* root,
                        const git_tree_entry* entry,
                        void* payload) -> int {
        auto* c = static_cast<Ctx*>(payload);
        uint32_t mode = static_cast<uint32_t>(git_tree_entry_filemode(entry));
        if (mode == MODE_TREE) return 0; // recurse, don't add dirs

        std::string rel = std::string(root) + git_tree_entry_name(entry);
        // Strip trailing slash that git_tree_walk adds to root
        if (!rel.empty() && rel.back() == '/') rel.pop_back();

        WalkEntry we;
        we.name = git_tree_entry_name(entry);
        we.oid  = oid_to_hex(git_tree_entry_id(entry));
        we.mode = mode;
        c->results->emplace_back(std::move(rel), std::move(we));
        return 0;
    };

    if (git_tree_walk(tg.t, GIT_TREEWALK_PRE, callback, &ctx) != 0) {
        throw_git_error("git_tree_walk");
    }

    if (!norm_path.empty()) {
        // Prefix all paths with norm_path
        for (auto& [p, e] : results) {
            p = norm_path + "/" + p;
        }
    }

    return results;
}

/// os.walk-style directory traversal: returns WalkDirEntry per directory.
std::vector<WalkDirEntry>
walk_tree_dirs(git_repository* repo,
               const std::string& tree_oid_hex,
               const std::string& norm_path) {
    std::string target_oid_hex = tree_oid_hex;
    if (!norm_path.empty()) {
        auto entry = entry_at_path(repo, tree_oid_hex, norm_path);
        if (!entry) throw NotFoundError(norm_path);
        if (entry->mode != MODE_TREE) throw NotADirectoryError(norm_path);
        target_oid_hex = entry->oid_hex;
    }

    std::vector<WalkDirEntry> results;

    // Recursive helper
    std::function<void(const std::string&, const std::string&)> recurse =
        [&](const std::string& oid_hex, const std::string& prefix) {
        git_oid oid = hex_to_oid(oid_hex);
        TreeGuard tg;
        if (git_tree_lookup(&tg.t, repo, &oid) != 0)
            throw_git_error("git_tree_lookup");

        WalkDirEntry entry;
        entry.dirpath = prefix;

        size_t n = git_tree_entrycount(tg.t);
        // Collect dirs for recursion after we finish this level
        std::vector<std::pair<std::string, std::string>> subdirs; // (name, oid_hex)
        for (size_t i = 0; i < n; ++i) {
            const git_tree_entry* e = git_tree_entry_byindex(tg.t, i);
            std::string name = git_tree_entry_name(e);
            uint32_t mode = static_cast<uint32_t>(git_tree_entry_filemode(e));
            std::string eid = oid_to_hex(git_tree_entry_id(e));

            if (mode == MODE_TREE) {
                entry.dirnames.push_back(name);
                subdirs.push_back({name, eid});
            } else {
                WalkEntry we;
                we.name = name;
                we.oid = eid;
                we.mode = mode;
                entry.files.push_back(std::move(we));
            }
        }
        results.push_back(std::move(entry));

        // Recurse into subdirectories
        for (auto& [dname, doid] : subdirs) {
            std::string sub_prefix = prefix.empty() ? dname : prefix + "/" + dname;
            recurse(doid, sub_prefix);
        }
    };

    recurse(target_oid_hex, norm_path);
    return results;
}

/// Count direct subdirectory entries in a tree (for nlink calculation).
uint32_t count_subdirs(git_repository* repo, const std::string& tree_oid_hex) {
    git_oid oid = hex_to_oid(tree_oid_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, repo, &oid) != 0) {
        throw_git_error("git_tree_lookup");
    }
    size_t n = git_tree_entrycount(tg.t);
    uint32_t count = 0;
    for (size_t i = 0; i < n; ++i) {
        const git_tree_entry* e = git_tree_entry_byindex(tg.t, i);
        if (static_cast<uint32_t>(git_tree_entry_filemode(e)) == MODE_TREE) {
            ++count;
        }
    }
    return count;
}

// ---------------------------------------------------------------------------
// Tree rebuild — apply writes/removes to produce a new root tree OID
// ---------------------------------------------------------------------------

/// Rebuild the tree rooted at `base_tree_oid_hex`, applying:
///   writes:  map<norm_path, {blob_data, mode}>
///   removes: list<norm_path>
/// Returns the new root tree OID as a 40-char hex string.
std::string rebuild_tree(
    git_repository* repo,
    const std::string& base_tree_oid_hex,
    const std::vector<std::pair<std::string,
                                std::pair<std::vector<uint8_t>, uint32_t>>>& writes,
    const std::vector<std::string>& removes)
{
    // Build a recursive representation of the tree mutations:
    // We process path by path, rebuilding trees bottom-up.

    // Helper: split path into segments
    auto split = [](const std::string& p) {
        std::vector<std::string> segs;
        std::istringstream ss(p);
        std::string tok;
        while (std::getline(ss, tok, '/')) {
            if (!tok.empty()) segs.push_back(tok);
        }
        return segs;
    };

    // Write blobs first and collect (path, oid_hex, mode)
    struct PendingWrite {
        std::vector<std::string> segs;
        std::string              oid_hex;
        uint32_t                 mode;
    };

    std::vector<PendingWrite> pending;
    for (auto& [norm_path, data_mode] : writes) {
        auto& [data, mode] = data_mode;

        // Write blob
        git_oid blob_oid;
        if (git_blob_create_from_buffer(&blob_oid, repo,
                                        data.data(), data.size()) != 0) {
            throw_git_error("git_blob_create_from_buffer");
        }
        pending.push_back({split(norm_path), oid_to_hex(&blob_oid), mode});
    }

    // Set of paths to remove (as segment vectors)
    std::vector<std::vector<std::string>> remove_segs;
    for (auto& p : removes) {
        remove_segs.push_back(split(p));
    }

    // Recursive tree builder
    // Returns new tree oid hex.
    // `prefix` tracks the path segments leading to the current subtree.
    std::function<std::string(const std::string&,
                              const std::vector<std::string>&)> rebuild;
    rebuild = [&](const std::string& cur_tree_oid_hex,
                  const std::vector<std::string>& prefix)
        -> std::string
    {
        int depth = static_cast<int>(prefix.size());

        // Helper: check if a path's first `depth` segments match `prefix`
        auto matches_prefix = [&](const std::vector<std::string>& segs) {
            if (static_cast<int>(segs.size()) <= depth) return false;
            for (int i = 0; i < depth; ++i) {
                if (segs[i] != prefix[i]) return false;
            }
            return true;
        };

        // Start from the current tree
        git_oid base_oid = hex_to_oid(cur_tree_oid_hex);
        BuilderGuard bg;
        {
            TreeGuard tg;
            // If oid is all-zeros (sentinel for empty tree), init empty builder
            bool is_empty = (cur_tree_oid_hex ==
                             std::string(GIT_OID_HEXSZ, '0'));
            if (!is_empty && git_tree_lookup(&tg.t, repo, &base_oid) == 0) {
                if (git_treebuilder_new(&bg.tb, repo, tg.t) != 0) {
                    throw_git_error("git_treebuilder_new");
                }
            } else {
                if (git_treebuilder_new(&bg.tb, repo, nullptr) != 0) {
                    throw_git_error("git_treebuilder_new (empty)");
                }
            }
        }

        // Process writes/removes whose path prefix matches the current subtree.

        // Entries to insert at this level: name → (oid_hex, mode)
        std::map<std::string, std::pair<std::string, uint32_t>> inserts;
        // Names to insert as subtrees (subdirectory mutations)
        std::map<std::string, std::string> subtree_writes; // name → cur subtree oid

        for (auto& pw : pending) {
            if (!matches_prefix(pw.segs)) continue;
            if (pw.segs.size() == static_cast<size_t>(depth + 1)) {
                // Leaf at this level
                inserts[pw.segs[depth]] = {pw.oid_hex, pw.mode};
            } else {
                // Goes deeper — record that we need to recurse into subtree
                std::string name = pw.segs[depth];
                if (subtree_writes.find(name) == subtree_writes.end()) {
                    // Get current subtree oid (if exists)
                    const git_tree_entry* e =
                        git_treebuilder_get(bg.tb, name.c_str());
                    if (e && static_cast<uint32_t>(
                            git_tree_entry_filemode(e)) == MODE_TREE) {
                        subtree_writes[name] = oid_to_hex(git_tree_entry_id(e));
                    } else {
                        subtree_writes[name] = std::string(GIT_OID_HEXSZ, '0');
                    }
                }
            }
        }

        for (auto& rv : remove_segs) {
            if (!matches_prefix(rv)) continue;
            if (rv.size() == static_cast<size_t>(depth + 1)) {
                // Remove at this level
                git_treebuilder_remove(bg.tb, rv[depth].c_str());
            } else {
                // Goes deeper
                std::string name = rv[depth];
                if (subtree_writes.find(name) == subtree_writes.end()) {
                    const git_tree_entry* e =
                        git_treebuilder_get(bg.tb, name.c_str());
                    if (e && static_cast<uint32_t>(
                            git_tree_entry_filemode(e)) == MODE_TREE) {
                        subtree_writes[name] = oid_to_hex(git_tree_entry_id(e));
                    } else {
                        subtree_writes[name] = std::string(GIT_OID_HEXSZ, '0');
                    }
                }
            }
        }

        // Recurse into subtrees
        for (auto& [name, sub_oid] : subtree_writes) {
            auto child_prefix = prefix;
            child_prefix.push_back(name);
            std::string new_sub_oid = rebuild(sub_oid, child_prefix);
            git_oid sub_git_oid = hex_to_oid(new_sub_oid);
            git_filemode_t fm = GIT_FILEMODE_TREE;
            if (git_treebuilder_insert(nullptr, bg.tb, name.c_str(),
                                       &sub_git_oid, fm) != 0) {
                throw_git_error("git_treebuilder_insert subtree");
            }
        }

        // Insert leaf writes
        for (auto& [name, oid_mode] : inserts) {
            git_oid ins_oid = hex_to_oid(oid_mode.first);
            git_filemode_t fm = static_cast<git_filemode_t>(oid_mode.second);
            if (git_treebuilder_insert(nullptr, bg.tb, name.c_str(),
                                       &ins_oid, fm) != 0) {
                throw_git_error("git_treebuilder_insert blob");
            }
        }

        // Write the tree
        git_oid new_tree_oid;
        if (git_treebuilder_write(&new_tree_oid, bg.tb) != 0) {
            throw_git_error("git_treebuilder_write");
        }
        return oid_to_hex(&new_tree_oid);
    };

    return rebuild(base_tree_oid_hex, {});
}

/// Write a new commit and return its 40-char hex SHA.
std::string write_commit(
    git_repository* repo,
    const std::string& tree_oid_hex,
    const std::vector<std::string>& parent_oids,  ///< May be empty for initial.
    const Signature&   sig,
    const std::string& message)
{
    git_oid tree_oid = hex_to_oid(tree_oid_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, repo, &tree_oid) != 0) {
        throw_git_error("git_tree_lookup (write_commit)");
    }

    // Build signature
    git_signature* author_sig = nullptr;
    {
        int rc = git_signature_now(&author_sig,
                                    sig.name.c_str(),
                                    sig.email.c_str());
        if (rc != 0) throw_git_error("git_signature_now");
    }
    // RAII for signature
    struct SigGuard {
        git_signature* s;
        ~SigGuard() { git_signature_free(s); }
    } sg{author_sig};

    // Parents — look up each parent commit object
    std::vector<git_commit*> parent_commits;
    // RAII cleanup for all parent commits
    struct ParentsGuard {
        std::vector<git_commit*>& commits;
        ~ParentsGuard() {
            for (auto* c : commits) {
                if (c) git_commit_free(c);
            }
        }
    } pg{parent_commits};

    std::vector<const git_commit*> parents_vec;
    for (const auto& oid_hex : parent_oids) {
        if (oid_hex.empty()) continue;
        git_oid parent_oid = hex_to_oid(oid_hex);
        git_commit* c = nullptr;
        if (git_commit_lookup(&c, repo, &parent_oid) != 0) {
            throw_git_error("git_commit_lookup (parent)");
        }
        parent_commits.push_back(c);
        parents_vec.push_back(c);
    }

    git_oid new_commit_oid;
    int rc = git_commit_create(
        &new_commit_oid,
        repo,
        nullptr, // don't update a ref here — we do CAS separately
        author_sig,
        author_sig,
        "UTF-8",
        message.c_str(),
        tg.t,
        static_cast<size_t>(parents_vec.size()),
        parents_vec.empty() ? nullptr : parents_vec.data()
    );
    if (rc != 0) throw_git_error("git_commit_create");

    return oid_to_hex(&new_commit_oid);
}

/// Resolve the tree OID for a commit.
std::string tree_oid_for_commit(git_repository* repo,
                                 const std::string& commit_oid_hex) {
    git_oid commit_oid = hex_to_oid(commit_oid_hex);
    CommitGuard cg;
    if (git_commit_lookup(&cg.c, repo, &commit_oid) != 0) {
        throw_git_error("git_commit_lookup (tree_oid_for_commit)");
    }
    const git_oid* tid = git_commit_tree_id(cg.c);
    return oid_to_hex(tid);
}

CommitMeta read_commit(git_repository* repo, const std::string& commit_oid_hex) {
    git_oid commit_oid = hex_to_oid(commit_oid_hex);
    CommitGuard cg;
    if (git_commit_lookup(&cg.c, repo, &commit_oid) != 0) {
        throw_git_error("git_commit_lookup (read_commit)");
    }

    CommitMeta meta;
    const char* msg = git_commit_message(cg.c);
    meta.message = msg ? msg : "";
    // Strip trailing newline
    while (!meta.message.empty() && meta.message.back() == '\n') {
        meta.message.pop_back();
    }

    meta.time = static_cast<uint64_t>(git_commit_time(cg.c));

    const git_signature* author = git_commit_author(cg.c);
    if (author) {
        meta.author_name  = author->name  ? author->name  : "";
        meta.author_email = author->email ? author->email : "";
    }

    meta.tree_oid_hex = oid_to_hex(git_commit_tree_id(cg.c));

    if (git_commit_parentcount(cg.c) > 0) {
        meta.parent_oid_hex = oid_to_hex(git_commit_parent_id(cg.c, 0));
    }

    return meta;
}

} // namespace tree
} // namespace vost
