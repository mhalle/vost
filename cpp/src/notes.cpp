#include "vost/notes.h"
#include "vost/fs.h"
#include "vost/gitstore.h"
#include "internal.h"

#include <git2.h>

#include <algorithm>
#include <cstring>
#include <regex>
#include <string>
#include <vector>

namespace vost {

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

namespace {

[[noreturn]] void throw_git(const std::string& ctx) {
    const git_error* e = git_error_last();
    std::string msg = ctx;
    if (e && e->message) { msg += ": "; msg += e->message; }
    throw GitError(msg);
}

std::string oid_to_hex(const git_oid* oid) {
    char buf[GIT_OID_HEXSZ + 1];
    git_oid_tostr(buf, sizeof(buf), oid);
    return std::string(buf, GIT_OID_HEXSZ);
}

git_oid hex_to_oid(const std::string& hex) {
    git_oid oid;
    if (git_oid_fromstr(&oid, hex.c_str()) != 0)
        throw InvalidHashError(hex);
    return oid;
}

/// RAII wrappers
struct TreeGuard {
    git_tree* t = nullptr;
    ~TreeGuard() { if (t) git_tree_free(t); }
};
struct BlobGuard {
    git_blob* b = nullptr;
    ~BlobGuard() { if (b) git_blob_free(b); }
};
struct CommitGuard {
    git_commit* c = nullptr;
    ~CommitGuard() { if (c) git_commit_free(c); }
};
struct BuilderGuard {
    git_treebuilder* tb = nullptr;
    ~BuilderGuard() { if (tb) git_treebuilder_free(tb); }
};
struct SigGuard {
    git_signature* s = nullptr;
    ~SigGuard() { if (s) git_signature_free(s); }
};
struct RefGuard {
    git_reference* r = nullptr;
    ~RefGuard() { if (r) git_reference_free(r); }
};
struct ObjGuard {
    git_object* o = nullptr;
    ~ObjGuard() { if (o) git_object_free(o); }
};

bool is_hex40(const std::string& s) {
    if (s.size() != 40) return false;
    for (char c : s) {
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')))
            return false;
    }
    return true;
}

void validate_hash(const std::string& hash) {
    if (!is_hex40(hash))
        throw InvalidHashError(hash);
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// NoteNamespace
// ---------------------------------------------------------------------------

NoteNamespace::NoteNamespace(std::shared_ptr<GitStoreInner> inner,
                             std::string ns_name)
    : inner_(std::move(inner))
    , namespace_(std::move(ns_name))
    , ref_name_("refs/notes/" + namespace_)
{}

std::string NoteNamespace::resolve_target(const std::string& target) const {
    if (is_hex40(target)) return target;

    std::lock_guard<std::mutex> lk(inner_->mutex);

    // Try as branch
    {
        std::string ref = "refs/heads/" + target;
        RefGuard rg;
        if (git_reference_lookup(&rg.r, inner_->repo, ref.c_str()) == 0) {
            ObjGuard og;
            if (git_reference_peel(&og.o, rg.r, GIT_OBJECT_COMMIT) == 0) {
                return oid_to_hex(git_object_id(og.o));
            }
        }
    }

    // Try as tag
    {
        std::string ref = "refs/tags/" + target;
        RefGuard rg;
        if (git_reference_lookup(&rg.r, inner_->repo, ref.c_str()) == 0) {
            ObjGuard og;
            if (git_reference_peel(&og.o, rg.r, GIT_OBJECT_COMMIT) == 0) {
                return oid_to_hex(git_object_id(og.o));
            }
        }
    }

    throw InvalidHashError(target);
}

std::optional<std::string> NoteNamespace::tip_oid() const {
    git_reference* ref = nullptr;
    if (git_reference_lookup(&ref, inner_->repo, ref_name_.c_str()) != 0)
        return std::nullopt;

    // Peel to commit
    git_object* obj = nullptr;
    int rc = git_reference_peel(&obj, ref, GIT_OBJECT_COMMIT);
    git_reference_free(ref);
    if (rc != 0) return std::nullopt;

    std::string hex = oid_to_hex(git_object_id(obj));
    git_object_free(obj);
    return hex;
}

std::optional<std::string> NoteNamespace::tree_oid() const {
    auto tip = tip_oid();
    if (!tip) return std::nullopt;

    git_oid commit_oid = hex_to_oid(*tip);
    CommitGuard cg;
    if (git_commit_lookup(&cg.c, inner_->repo, &commit_oid) != 0)
        return std::nullopt;

    const git_oid* tid = git_commit_tree_id(cg.c);
    return oid_to_hex(tid);
}

std::optional<std::string> NoteNamespace::find_note(
    const std::string& tree_hex,
    const std::string& hash) const
{
    git_oid tree_oid_val = hex_to_oid(tree_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, inner_->repo, &tree_oid_val) != 0)
        return std::nullopt;

    // Try flat: look for entry named by full 40-char hash
    {
        const git_tree_entry* entry =
            git_tree_entry_byname(tg.t, hash.c_str());
        if (entry) {
            uint32_t mode = static_cast<uint32_t>(
                git_tree_entry_filemode(entry));
            if (mode != MODE_TREE) {
                return oid_to_hex(git_tree_entry_id(entry));
            }
        }
    }

    // Try 2/38 fanout: look for prefix dir, then suffix entry
    {
        std::string prefix = hash.substr(0, 2);
        std::string suffix = hash.substr(2);

        const git_tree_entry* dir_entry =
            git_tree_entry_byname(tg.t, prefix.c_str());
        if (dir_entry) {
            uint32_t mode = static_cast<uint32_t>(
                git_tree_entry_filemode(dir_entry));
            if (mode == MODE_TREE) {
                git_oid sub_oid = *git_tree_entry_id(dir_entry);
                TreeGuard sub_tg;
                if (git_tree_lookup(&sub_tg.t, inner_->repo, &sub_oid) == 0) {
                    const git_tree_entry* blob_entry =
                        git_tree_entry_byname(sub_tg.t, suffix.c_str());
                    if (blob_entry) {
                        return oid_to_hex(git_tree_entry_id(blob_entry));
                    }
                }
            }
        }
    }

    return std::nullopt;
}

std::vector<std::pair<std::string, std::string>> NoteNamespace::iter_notes(
    const std::string& tree_hex) const
{
    std::vector<std::pair<std::string, std::string>> result;

    git_oid tree_oid_val = hex_to_oid(tree_hex);
    TreeGuard tg;
    if (git_tree_lookup(&tg.t, inner_->repo, &tree_oid_val) != 0)
        return result;

    size_t n = git_tree_entrycount(tg.t);
    for (size_t i = 0; i < n; ++i) {
        const git_tree_entry* entry = git_tree_entry_byindex(tg.t, i);
        std::string name = git_tree_entry_name(entry);
        uint32_t mode = static_cast<uint32_t>(git_tree_entry_filemode(entry));

        if (mode == MODE_TREE && name.size() == 2) {
            // Fanout subtree — iterate prefix + suffix
            git_oid sub_oid = *git_tree_entry_id(entry);
            TreeGuard sub_tg;
            if (git_tree_lookup(&sub_tg.t, inner_->repo, &sub_oid) != 0)
                continue;
            size_t sn = git_tree_entrycount(sub_tg.t);
            for (size_t j = 0; j < sn; ++j) {
                const git_tree_entry* sub_entry =
                    git_tree_entry_byindex(sub_tg.t, j);
                std::string sub_name = git_tree_entry_name(sub_entry);
                std::string full_hash = name + sub_name;
                if (is_hex40(full_hash)) {
                    result.emplace_back(full_hash,
                                        oid_to_hex(git_tree_entry_id(sub_entry)));
                }
            }
        } else if (is_hex40(name)) {
            // Flat entry
            result.emplace_back(name, oid_to_hex(git_tree_entry_id(entry)));
        }
    }

    return result;
}

std::string NoteNamespace::build_note_tree(
    const std::optional<std::string>& base_tree_hex,
    const std::vector<std::pair<std::string, std::string>>& writes,
    const std::vector<std::string>& deletes) const
{
    // Start a treebuilder from the base tree (or empty)
    BuilderGuard bg;
    if (base_tree_hex) {
        git_oid base_oid = hex_to_oid(*base_tree_hex);
        TreeGuard tg;
        if (git_tree_lookup(&tg.t, inner_->repo, &base_oid) == 0) {
            if (git_treebuilder_new(&bg.tb, inner_->repo, tg.t) != 0)
                throw_git("git_treebuilder_new (notes)");
        } else {
            if (git_treebuilder_new(&bg.tb, inner_->repo, nullptr) != 0)
                throw_git("git_treebuilder_new (notes empty)");
        }
    } else {
        if (git_treebuilder_new(&bg.tb, inner_->repo, nullptr) != 0)
            throw_git("git_treebuilder_new (notes empty)");
    }

    // Apply deletes: try flat first, then fanout
    for (auto& hash : deletes) {
        // Try flat
        const git_tree_entry* existing =
            git_treebuilder_get(bg.tb, hash.c_str());
        if (existing) {
            git_treebuilder_remove(bg.tb, hash.c_str());
            continue;
        }

        // Try fanout: remove from 2-char subtree
        std::string prefix = hash.substr(0, 2);
        std::string suffix = hash.substr(2);

        const git_tree_entry* dir_entry =
            git_treebuilder_get(bg.tb, prefix.c_str());
        if (dir_entry &&
            static_cast<uint32_t>(git_tree_entry_filemode(dir_entry)) == MODE_TREE) {
            git_oid sub_oid = *git_tree_entry_id(dir_entry);
            TreeGuard sub_tg;
            if (git_tree_lookup(&sub_tg.t, inner_->repo, &sub_oid) == 0) {
                const git_tree_entry* sub_entry =
                    git_tree_entry_byname(sub_tg.t, suffix.c_str());
                if (sub_entry) {
                    // Rebuild subtree without this entry
                    BuilderGuard sub_bg;
                    if (git_treebuilder_new(&sub_bg.tb, inner_->repo, sub_tg.t) != 0)
                        throw_git("git_treebuilder_new (fanout sub)");
                    git_treebuilder_remove(sub_bg.tb, suffix.c_str());

                    if (git_treebuilder_entrycount(sub_bg.tb) == 0) {
                        // Subtree is now empty — remove the dir entry
                        git_treebuilder_remove(bg.tb, prefix.c_str());
                    } else {
                        // Write updated subtree
                        git_oid new_sub_oid;
                        if (git_treebuilder_write(&new_sub_oid, sub_bg.tb) != 0)
                            throw_git("git_treebuilder_write (fanout sub)");
                        if (git_treebuilder_insert(nullptr, bg.tb,
                                                    prefix.c_str(),
                                                    &new_sub_oid,
                                                    GIT_FILEMODE_TREE) != 0)
                            throw_git("git_treebuilder_insert (fanout sub)");
                    }
                    continue;
                }
            }
        }

        throw KeyNotFoundError("note not found: " + hash);
    }

    // Apply writes: always flat (remove fanout entry if exists, add flat)
    for (auto& [hash, blob_hex] : writes) {
        // Remove any existing fanout entry for this hash
        std::string prefix = hash.substr(0, 2);
        std::string suffix = hash.substr(2);

        const git_tree_entry* dir_entry =
            git_treebuilder_get(bg.tb, prefix.c_str());
        if (dir_entry &&
            static_cast<uint32_t>(git_tree_entry_filemode(dir_entry)) == MODE_TREE) {
            git_oid sub_oid = *git_tree_entry_id(dir_entry);
            TreeGuard sub_tg;
            if (git_tree_lookup(&sub_tg.t, inner_->repo, &sub_oid) == 0) {
                const git_tree_entry* sub_entry =
                    git_tree_entry_byname(sub_tg.t, suffix.c_str());
                if (sub_entry) {
                    // Remove from fanout
                    BuilderGuard sub_bg;
                    if (git_treebuilder_new(&sub_bg.tb, inner_->repo, sub_tg.t) != 0)
                        throw_git("git_treebuilder_new (fanout write)");
                    git_treebuilder_remove(sub_bg.tb, suffix.c_str());

                    if (git_treebuilder_entrycount(sub_bg.tb) == 0) {
                        git_treebuilder_remove(bg.tb, prefix.c_str());
                    } else {
                        git_oid new_sub_oid;
                        if (git_treebuilder_write(&new_sub_oid, sub_bg.tb) != 0)
                            throw_git("git_treebuilder_write (fanout write)");
                        if (git_treebuilder_insert(nullptr, bg.tb,
                                                    prefix.c_str(),
                                                    &new_sub_oid,
                                                    GIT_FILEMODE_TREE) != 0)
                            throw_git("git_treebuilder_insert (fanout write)");
                    }
                }
            }
        }

        // Insert flat entry
        git_oid blob_oid = hex_to_oid(blob_hex);
        if (git_treebuilder_insert(nullptr, bg.tb, hash.c_str(),
                                    &blob_oid, GIT_FILEMODE_BLOB) != 0)
            throw_git("git_treebuilder_insert (note)");
    }

    // Write the tree
    git_oid new_tree_oid;
    if (git_treebuilder_write(&new_tree_oid, bg.tb) != 0)
        throw_git("git_treebuilder_write (notes)");
    return oid_to_hex(&new_tree_oid);
}

void NoteNamespace::commit_note_tree(const std::string& new_tree_hex,
                                      const std::string& message) {
    lock::with_repo_lock(inner_->path, [&]() {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // Re-read tip inside lock for CAS
        std::string parent_hex;
        {
            git_reference* ref = nullptr;
            if (git_reference_lookup(&ref, inner_->repo,
                                      ref_name_.c_str()) == 0) {
                git_object* obj = nullptr;
                int rc = git_reference_peel(&obj, ref, GIT_OBJECT_COMMIT);
                git_reference_free(ref);
                if (rc == 0 && obj) {
                    parent_hex = oid_to_hex(git_object_id(obj));
                    git_object_free(obj);
                }
            }
        }

        // Create commit (don't set ref yet)
        std::vector<std::string> parent_oids;
        if (!parent_hex.empty()) {
            parent_oids.push_back(parent_hex);
        }
        std::string commit_hex = tree::write_commit(
            inner_->repo, new_tree_hex, parent_oids,
            inner_->signature, message);

        // CAS ref update
        git_oid new_oid = hex_to_oid(commit_hex);
        git_reference* out_ref = nullptr;
        int rc;

        if (!parent_hex.empty()) {
            // Update existing ref
            git_reference* existing = nullptr;
            if (git_reference_lookup(&existing, inner_->repo,
                                      ref_name_.c_str()) == 0) {
                rc = git_reference_set_target(&out_ref, existing, &new_oid,
                                               message.c_str());
                git_reference_free(existing);
            } else {
                rc = git_reference_create(&out_ref, inner_->repo,
                                           ref_name_.c_str(), &new_oid,
                                           0, message.c_str());
            }
        } else {
            // Create new ref
            rc = git_reference_create(&out_ref, inner_->repo,
                                       ref_name_.c_str(), &new_oid,
                                       0, message.c_str());
        }
        if (out_ref) git_reference_free(out_ref);
        if (rc != 0) throw_git("notes ref update");
    });
}

// ---------------------------------------------------------------------------
// NoteNamespace public API
// ---------------------------------------------------------------------------

std::string NoteNamespace::get(const std::string& hash) const {
    auto h = resolve_target(hash);
    std::lock_guard<std::mutex> lk(inner_->mutex);

    auto t = tree_oid();
    if (!t) throw KeyNotFoundError(h);

    auto blob_hex = find_note(*t, h);
    if (!blob_hex) throw KeyNotFoundError(h);

    // Read blob
    git_oid blob_oid = hex_to_oid(*blob_hex);
    BlobGuard bg;
    if (git_blob_lookup(&bg.b, inner_->repo, &blob_oid) != 0)
        throw_git("git_blob_lookup (note)");

    const char* raw = static_cast<const char*>(git_blob_rawcontent(bg.b));
    size_t sz = static_cast<size_t>(git_blob_rawsize(bg.b));
    return std::string(raw, sz);
}

void NoteNamespace::set(const std::string& hash, const std::string& text) {
    auto h = resolve_target(hash);

    std::string blob_hex;
    std::optional<std::string> base_tree;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        // Create blob
        git_oid blob_oid;
        if (git_blob_create_from_buffer(&blob_oid, inner_->repo,
                                         text.data(), text.size()) != 0)
            throw_git("git_blob_create_from_buffer (note)");
        blob_hex = oid_to_hex(&blob_oid);

        base_tree = tree_oid();
    }

    // Build new tree and commit
    std::string new_tree;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        new_tree = build_note_tree(base_tree, {{h, blob_hex}}, {});
    }

    commit_note_tree(new_tree, "Notes updated");
}

void NoteNamespace::del(const std::string& hash) {
    auto h = resolve_target(hash);

    std::optional<std::string> base_tree;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        base_tree = tree_oid();
    }

    if (!base_tree)
        throw KeyNotFoundError(h);

    std::string new_tree;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        new_tree = build_note_tree(base_tree, {}, {h});
    }

    commit_note_tree(new_tree, "Notes updated");
}

bool NoteNamespace::has(const std::string& hash) const {
    auto h = resolve_target(hash);
    std::lock_guard<std::mutex> lk(inner_->mutex);

    auto t = tree_oid();
    if (!t) return false;

    return find_note(*t, h).has_value();
}

// -- Fs overloads ----------------------------------------------------------

std::string NoteNamespace::get(const Fs& fs) const {
    return get(*fs.commit_hash());
}

void NoteNamespace::set(const Fs& fs, const std::string& text) {
    set(*fs.commit_hash(), text);
}

void NoteNamespace::del(const Fs& fs) {
    del(*fs.commit_hash());
}

bool NoteNamespace::has(const Fs& fs) const {
    return has(*fs.commit_hash());
}

std::vector<std::string> NoteNamespace::list() const {
    std::lock_guard<std::mutex> lk(inner_->mutex);

    auto t = tree_oid();
    if (!t) return {};

    auto notes = iter_notes(*t);
    std::vector<std::string> result;
    result.reserve(notes.size());
    for (auto& [hash, _] : notes) {
        result.push_back(hash);
    }
    std::sort(result.begin(), result.end());
    return result;
}

size_t NoteNamespace::size() const {
    std::lock_guard<std::mutex> lk(inner_->mutex);

    auto t = tree_oid();
    if (!t) return 0;

    return iter_notes(*t).size();
}

bool NoteNamespace::empty() const {
    return size() == 0;
}

std::string NoteNamespace::get_for_current_branch() const {
    std::string tip_commit;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        git_reference* head = nullptr;
        if (git_repository_head(&head, inner_->repo) != 0)
            throw NotFoundError("HEAD not resolvable");

        git_object* obj = nullptr;
        int rc = git_reference_peel(&obj, head, GIT_OBJECT_COMMIT);
        git_reference_free(head);
        if (rc != 0) throw NotFoundError("HEAD commit not found");

        tip_commit = oid_to_hex(git_object_id(obj));
        git_object_free(obj);
    }
    return get(tip_commit);
}

void NoteNamespace::set_for_current_branch(const std::string& text) {
    std::string tip_commit;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        git_reference* head = nullptr;
        if (git_repository_head(&head, inner_->repo) != 0)
            throw NotFoundError("HEAD not resolvable");

        git_object* obj = nullptr;
        int rc = git_reference_peel(&obj, head, GIT_OBJECT_COMMIT);
        git_reference_free(head);
        if (rc != 0) throw NotFoundError("HEAD commit not found");

        tip_commit = oid_to_hex(git_object_id(obj));
        git_object_free(obj);
    }
    set(tip_commit, text);
}

NotesBatch NoteNamespace::batch() {
    return NotesBatch(*this);
}

// ---------------------------------------------------------------------------
// NotesBatch
// ---------------------------------------------------------------------------

NotesBatch::NotesBatch(NoteNamespace ns)
    : ns_(std::move(ns))
{}

void NotesBatch::set(const std::string& hash, const std::string& text) {
    if (committed_) throw BatchClosedError();
    auto h = ns_.resolve_target(hash);

    // Remove from deletes if present
    deletes_.erase(
        std::remove(deletes_.begin(), deletes_.end(), h),
        deletes_.end());

    // Replace existing write for same hash
    writes_.erase(
        std::remove_if(writes_.begin(), writes_.end(),
                       [&h](const auto& kv) { return kv.first == h; }),
        writes_.end());

    writes_.emplace_back(h, text);
}

void NotesBatch::del(const std::string& hash) {
    if (committed_) throw BatchClosedError();
    auto h = ns_.resolve_target(hash);

    // Remove from writes if present
    writes_.erase(
        std::remove_if(writes_.begin(), writes_.end(),
                       [&h](const auto& kv) { return kv.first == h; }),
        writes_.end());

    // Add to deletes if not already there
    if (std::find(deletes_.begin(), deletes_.end(), h) == deletes_.end()) {
        deletes_.push_back(h);
    }
}

void NotesBatch::set(const Fs& fs, const std::string& text) {
    set(*fs.commit_hash(), text);
}

void NotesBatch::del(const Fs& fs) {
    del(*fs.commit_hash());
}

void NotesBatch::commit() {
    if (committed_) throw BatchClosedError();
    committed_ = true;

    if (writes_.empty() && deletes_.empty()) return;

    // Create blobs for all writes
    std::vector<std::pair<std::string, std::string>> blob_writes;
    std::optional<std::string> base_tree;
    {
        std::lock_guard<std::mutex> lk(ns_.inner()->mutex);

        base_tree = ns_.tree_oid();

        for (auto& [hash, text] : writes_) {
            git_oid blob_oid;
            if (git_blob_create_from_buffer(&blob_oid, ns_.inner()->repo,
                                             text.data(), text.size()) != 0)
                throw_git("git_blob_create_from_buffer (notes batch)");
            blob_writes.emplace_back(hash, oid_to_hex(&blob_oid));
        }
    }

    // Build tree and commit
    std::string new_tree;
    {
        std::lock_guard<std::mutex> lk(ns_.inner()->mutex);
        new_tree = ns_.build_note_tree(base_tree, blob_writes, deletes_);
    }

    size_t total = writes_.size() + deletes_.size();
    std::string msg = "Notes batch update (" + std::to_string(total) + " changes)";
    ns_.commit_note_tree(new_tree, msg);
}

// ---------------------------------------------------------------------------
// NoteDict
// ---------------------------------------------------------------------------

NoteDict::NoteDict(std::shared_ptr<GitStoreInner> inner)
    : inner_(std::move(inner))
{}

NoteNamespace NoteDict::operator[](const std::string& ns_name) {
    return NoteNamespace(inner_, ns_name);
}

NoteNamespace NoteDict::ns(const std::string& ns_name) {
    return NoteNamespace(inner_, ns_name);
}

} // namespace vost
