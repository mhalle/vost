#include "vost/gitstore.h"
#include "vost/fs.h"
#include "vost/mirror.h"
#include "internal.h"

#include <git2.h>

#include <cstring>
#include <stdexcept>
#include <string>

namespace vost {

// ---------------------------------------------------------------------------
// libgit2 lifecycle — initialise once per process
// ---------------------------------------------------------------------------

namespace {
struct LibGit2Init {
    LibGit2Init()  { git_libgit2_init(); }
    ~LibGit2Init() { git_libgit2_shutdown(); }
};
static LibGit2Init s_libgit2;

[[noreturn]] void throw_git(const std::string& ctx) {
    const git_error* e = git_error_last();
    std::string msg = ctx;
    if (e && e->message) { msg += ": "; msg += e->message; }
    throw GitError(msg);
}

std::string oid_hex(const git_oid* o) {
    char buf[GIT_OID_HEXSZ + 1];
    git_oid_tostr(buf, sizeof(buf), o);
    return std::string(buf, GIT_OID_HEXSZ);
}
} // anonymous namespace

// ---------------------------------------------------------------------------
// GitStoreInner
// ---------------------------------------------------------------------------

GitStoreInner::GitStoreInner(git_repository* r,
                              std::filesystem::path p,
                              Signature sig)
    : repo(r), path(std::move(p)), signature(std::move(sig)) {}

GitStoreInner::~GitStoreInner() {
    if (repo) git_repository_free(repo);
}

// ---------------------------------------------------------------------------
// GitStore::open
// ---------------------------------------------------------------------------

static void init_branch(git_repository* repo,
                         const std::filesystem::path& path,
                         const std::string& branch,
                         const Signature& sig) {
    // Write empty tree
    git_treebuilder* tb = nullptr;
    if (git_treebuilder_new(&tb, repo, nullptr) != 0)
        throw_git("git_treebuilder_new");
    git_oid tree_oid;
    if (git_treebuilder_write(&tree_oid, tb) != 0) {
        git_treebuilder_free(tb);
        throw_git("git_treebuilder_write");
    }
    git_treebuilder_free(tb);

    git_tree* tree = nullptr;
    if (git_tree_lookup(&tree, repo, &tree_oid) != 0)
        throw_git("git_tree_lookup");

    git_signature* author = nullptr;
    if (git_signature_now(&author, sig.name.c_str(), sig.email.c_str()) != 0) {
        git_tree_free(tree);
        throw_git("git_signature_now");
    }

    std::string msg = "Initialize " + branch;
    std::string refname = "refs/heads/" + branch;

    git_oid commit_oid;
    int rc = git_commit_create(&commit_oid, repo, refname.c_str(),
                                author, author, "UTF-8",
                                msg.c_str(), tree, 0, nullptr);
    git_signature_free(author);
    git_tree_free(tree);
    if (rc != 0) throw_git("git_commit_create");

    // Set HEAD → refs/heads/<branch>
    if (git_repository_set_head(repo, refname.c_str()) != 0)
        throw_git("git_repository_set_head");
}

GitStore GitStore::open(const std::filesystem::path& path, OpenOptions opts) {
    Signature sig;
    if (opts.author) sig.name  = *opts.author;
    if (opts.email)  sig.email = *opts.email;

    git_repository* repo = nullptr;
    bool existed = std::filesystem::exists(path);

    if (existed) {
        if (git_repository_open_bare(&repo, path.string().c_str()) != 0) {
            throw_git("git_repository_open_bare");
        }
    } else if (opts.create) {
        std::filesystem::create_directories(path);
        if (git_repository_init(&repo, path.string().c_str(), 1 /*bare*/) != 0) {
            throw_git("git_repository_init");
        }
        // Enable reflogs in bare repos (needed for undo/redo)
        {
            git_config* cfg = nullptr;
            if (git_repository_config(&cfg, repo) == 0) {
                git_config_set_string(cfg, "core.logAllRefUpdates", "always");
                git_config_free(cfg);
            }
        }
        if (opts.branch) {
            try {
                init_branch(repo, path, *opts.branch, sig);
            } catch (...) {
                git_repository_free(repo);
                throw;
            }
        }
    } else {
        throw NotFoundError("repository not found: " + path.string());
    }

    auto inner = std::make_shared<GitStoreInner>(repo, path, sig);
    return GitStore(std::move(inner));
}

GitStore::GitStore(std::shared_ptr<GitStoreInner> inner)
    : inner_(std::move(inner)) {}

// ---------------------------------------------------------------------------
// GitStore methods
// ---------------------------------------------------------------------------

RefDict GitStore::branches() {
    return RefDict(inner_, "refs/heads/", true);
}

RefDict GitStore::tags() {
    return RefDict(inner_, "refs/tags/", false);
}

NoteDict GitStore::notes() {
    return NoteDict(inner_);
}

Fs GitStore::fs(const std::string& ref) {
    // Try branch first
    auto br = branches();
    if (br.contains(ref)) {
        return br.get(ref);
    }
    // Try tag
    auto tg = tags();
    if (tg.contains(ref)) {
        return tg.get(ref);
    }
    // Fall back to commit hash
    git_oid oid;
    if (git_oid_fromstr(&oid, ref.c_str()) != 0)
        throw NotFoundError("ref not found: " + ref);

    git_commit* commit = nullptr;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        if (git_commit_lookup(&commit, inner_->repo, &oid) != 0)
            throw NotFoundError("ref not found: " + ref);
    }
    const git_oid* tid = git_commit_tree_id(commit);
    std::string tree_hex = oid_hex(tid);
    git_commit_free(commit);

    return Fs(inner_, ref, tree_hex, std::nullopt, false);
}

MirrorDiff GitStore::backup(const std::string& dest, const BackupOptions& opts) {
    return mirror::backup(inner_, dest, opts);
}

MirrorDiff GitStore::restore(const std::string& src, const RestoreOptions& opts) {
    return mirror::restore(inner_, src, opts);
}

void GitStore::bundle_export(const std::string& path,
                             const std::vector<std::string>& refs,
                             const std::map<std::string, std::string>& ref_map,
                             bool squash) {
    mirror::bundle_export(inner_, path, refs, ref_map, squash);
}

void GitStore::bundle_import(const std::string& path,
                             const std::vector<std::string>& refs,
                             const std::map<std::string, std::string>& ref_map) {
    mirror::bundle_import(inner_, path, refs, ref_map);
}

const std::filesystem::path& GitStore::path() const {
    return inner_->path;
}

const Signature& GitStore::signature() const {
    return inner_->signature;
}

// ---------------------------------------------------------------------------
// RefDict
// ---------------------------------------------------------------------------

RefDict::RefDict(std::shared_ptr<GitStoreInner> inner,
                 std::string prefix,
                 bool writable)
    : inner_(std::move(inner)), prefix_(std::move(prefix)),
      writable_(writable) {}


Fs RefDict::operator[](const std::string& name) { return get(name); }

Fs RefDict::get(const std::string& name) {
    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reference* ref = nullptr;
    if (git_reference_lookup(&ref, inner_->repo, refname.c_str()) != 0) {
        throw KeyNotFoundError(name);
    }

    // Peel to commit
    git_object* obj = nullptr;
    int rc = git_reference_peel(&obj, ref, GIT_OBJECT_COMMIT);
    git_reference_free(ref);
    if (rc != 0) throw_git("git_reference_peel (commit)");

    std::string commit_hex = oid_hex(git_object_id(obj));
    git_commit* commit = reinterpret_cast<git_commit*>(obj);

    const git_oid* tid = git_commit_tree_id(commit);
    std::string tree_hex = oid_hex(tid);
    git_object_free(obj);

    return Fs(inner_, commit_hex, tree_hex, name, writable_);
}

Fs RefDict::set_and_get(const std::string& name, const Fs& fs) {
    set(name, fs);
    return get(name);
}

void RefDict::set(const std::string& name, const Fs& fs) {
    // Validate ref name
    paths::validate_ref_name(name);

    // Same-repo check
    if (inner_.get() != fs.inner().get()) {
        // Check by path
        auto p1 = std::filesystem::weakly_canonical(inner_->path);
        auto p2 = std::filesystem::weakly_canonical(fs.inner()->path);
        if (p1 != p2) {
            throw InvalidPathError("Fs belongs to a different repository");
        }
    }

    auto commit_hex = fs.commit_hash();
    if (!commit_hex) throw GitError("Fs has no commit");

    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reference* existing = nullptr;
    bool ref_exists = (git_reference_lookup(&existing, inner_->repo,
                                             refname.c_str()) == 0);
    if (ref_exists) {
        if (!writable_) {
            git_reference_free(existing);
            throw KeyExistsError("tag '" + name + "' already exists");
        }
        git_reference_free(existing);
    }

    git_oid new_oid;
    if (git_oid_fromstr(&new_oid, commit_hex->c_str()) != 0)
        throw InvalidHashError(*commit_hex);

    git_reference* out_ref = nullptr;
    int rc = git_reference_create(&out_ref, inner_->repo,
                                   refname.c_str(), &new_oid,
                                   1 /*force*/, "refdict: set");
    if (rc != 0) {
        throw_git("git_reference_create");
    }
    git_reference_free(out_ref);
}

void RefDict::del(const std::string& name) {
    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reference* ref = nullptr;
    if (git_reference_lookup(&ref, inner_->repo, refname.c_str()) != 0)
        throw KeyNotFoundError(name);

    int rc = git_reference_delete(ref);
    git_reference_free(ref);
    if (rc != 0) throw_git("git_reference_delete");
}

bool RefDict::contains(const std::string& name) {
    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);
    git_reference* ref = nullptr;
    bool found = (git_reference_lookup(&ref, inner_->repo, refname.c_str()) == 0);
    if (found) git_reference_free(ref);
    return found;
}

std::vector<std::string> RefDict::keys() {
    std::vector<std::string> result;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reference_iterator* iter = nullptr;
    if (git_reference_iterator_glob_new(&iter, inner_->repo,
                                         (prefix_ + "*").c_str()) != 0)
        return result;

    git_reference* ref = nullptr;
    while (git_reference_next(&ref, iter) == 0) {
        std::string full = git_reference_name(ref);
        if (full.size() > prefix_.size()) {
            result.push_back(full.substr(prefix_.size()));
        }
        git_reference_free(ref);
    }
    git_reference_iterator_free(iter);
    return result;
}

std::vector<Fs> RefDict::values() {
    auto ks = keys();
    std::vector<Fs> out;
    out.reserve(ks.size());
    for (auto& k : ks) out.push_back(get(k));
    return out;
}

std::optional<std::string> RefDict::current_name() {
    if (!writable_) return std::nullopt;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reference* head = nullptr;
    if (git_repository_head(&head, inner_->repo) != 0) return std::nullopt;

    if (!git_reference_is_branch(head)) {
        git_reference_free(head);
        return std::nullopt;
    }

    std::string name = git_reference_shorthand(head);
    git_reference_free(head);

    // Make sure it's under our prefix
    std::string full = prefix_ + name;
    git_reference* check = nullptr;
    bool ok = (git_reference_lookup(&check, inner_->repo, full.c_str()) == 0);
    if (ok) git_reference_free(check);
    return ok ? std::optional<std::string>(name) : std::nullopt;
}

std::optional<Fs> RefDict::current() {
    auto name = current_name();
    if (!name) return std::nullopt;
    try {
        return get(*name);
    } catch (...) {
        return std::nullopt;
    }
}

void RefDict::set_current(const std::string& name) {
    if (!writable_) throw PermissionError("cannot set_current on tags");
    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);
    if (git_repository_set_head(inner_->repo, refname.c_str()) != 0)
        throw_git("git_repository_set_head");
}

std::vector<ReflogEntry> RefDict::reflog(const std::string& name) {
    std::string refname = prefix_ + name;
    std::lock_guard<std::mutex> lk(inner_->mutex);

    git_reflog* rlog = nullptr;
    if (git_reflog_read(&rlog, inner_->repo, refname.c_str()) != 0)
        return {};

    size_t n = git_reflog_entrycount(rlog);
    std::vector<ReflogEntry> result;
    result.reserve(n);

    for (size_t i = 0; i < n; ++i) {
        const git_reflog_entry* e = git_reflog_entry_byindex(rlog, i);
        ReflogEntry re;
        re.old_sha   = oid_hex(git_reflog_entry_id_old(e));
        re.new_sha   = oid_hex(git_reflog_entry_id_new(e));
        const git_signature* sig = git_reflog_entry_committer(e);
        if (sig) {
            re.committer = std::string(sig->name ? sig->name : "") +
                           " <" + (sig->email ? sig->email : "") + ">";
            re.timestamp = static_cast<uint64_t>(sig->when.time);
        }
        const char* msg = git_reflog_entry_message(e);
        re.message = msg ? msg : "";
        result.push_back(std::move(re));
    }
    git_reflog_free(rlog);
    return result;
}

} // namespace vost
