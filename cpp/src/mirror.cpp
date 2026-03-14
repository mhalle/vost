#include "vost/mirror.h"
#include "vost/gitstore.h"
#include "vost/error.h"

#include <git2.h>

#include <algorithm>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <map>
#include <mutex>
#include <set>
#include <sstream>
#include <string>
#include <vector>

namespace vost {
namespace mirror {

namespace {

// ---------------------------------------------------------------------------
// libgit2 helpers (local to this TU, same pattern as gitstore.cpp)
// ---------------------------------------------------------------------------

[[noreturn]] void throw_git(const std::string& ctx) {
    const git_error* e = git_error_last();
    std::string msg = ctx;
    if (e && e->message) { msg += ": "; msg += e->message; }
    throw GitError(msg);
}

std::string oid_hex(const git_oid* o) {
    char buf[41];
    git_oid_tostr(buf, sizeof(buf), o);
    return std::string(buf);
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

bool is_local_path(const std::string& url) {
    return url.compare(0, 7, "http://") != 0 &&
           url.compare(0, 8, "https://") != 0 &&
           url.compare(0, 6, "git://") != 0 &&
           url.compare(0, 6, "ssh://") != 0;
}

void reject_scp_url(const std::string& url) {
    if (!is_local_path(url) || url.compare(0, 7, "file://") == 0) return;

    // user@host:path
    auto at_pos = url.find('@');
    if (at_pos != std::string::npos) {
        auto after_at = url.substr(at_pos + 1);
        if (after_at.find(':') != std::string::npos) {
            throw InvalidPathError("scp-style URL not supported: \"" + url +
                                   "\" \xe2\x80\x94 use ssh:// format instead");
        }
    }

    // host:path (no @)
    auto colon_pos = url.find(':');
    if (colon_pos != std::string::npos && colon_pos > 1) {
        auto prefix = url.substr(0, colon_pos);
        if (prefix.find('/') == std::string::npos &&
            prefix.find('\\') == std::string::npos) {
            throw InvalidPathError("scp-style URL not supported: \"" + url +
                                   "\" \xe2\x80\x94 use ssh:// format instead");
        }
    }
}

std::string local_path_from_url(const std::string& url) {
    if (url.compare(0, 7, "file://") == 0) return url.substr(7);
    return url;
}

void auto_create_bare_repo(const std::string& url) {
    if (!is_local_path(url)) return;
    auto path = local_path_from_url(url);
    if (std::filesystem::exists(path)) return;

    std::filesystem::create_directories(path);
    git_repository* repo = nullptr;
    if (git_repository_init(&repo, path.c_str(), 1 /*bare*/) != 0) {
        throw_git("git_repository_init (auto-create)");
    }
    git_repository_free(repo);
}

// ---------------------------------------------------------------------------
// Ref enumeration
// ---------------------------------------------------------------------------

using RefMap = std::map<std::string, std::string>;

/// Get all refs from a git_repository, excluding HEAD.
RefMap get_refs_from_repo(git_repository* repo) {
    RefMap refs;
    git_reference_iterator* iter = nullptr;
    if (git_reference_iterator_new(&iter, repo) != 0) return refs;

    git_reference* ref = nullptr;
    while (git_reference_next(&ref, iter) == 0) {
        const char* name = git_reference_name(ref);
        if (!name || std::string(name) == "HEAD") {
            git_reference_free(ref);
            continue;
        }

        // Resolve symbolic refs to get the direct OID
        git_reference* resolved = nullptr;
        if (git_reference_resolve(&resolved, ref) == 0) {
            const git_oid* oid = git_reference_target(resolved);
            if (oid) {
                refs[name] = oid_hex(oid);
            }
            git_reference_free(resolved);
        }
        git_reference_free(ref);
    }
    git_reference_iterator_free(iter);
    return refs;
}

/// Get all local refs from the inner repo.
RefMap get_local_refs(git_repository* repo) {
    return get_refs_from_repo(repo);
}

/// Get remote refs. For local paths, opens repo directly. For URLs, uses
/// git_remote_ls.
RefMap get_remote_refs(git_repository* repo, const std::string& url) {
    // Local path — open directly
    if (is_local_path(url) || url.compare(0, 7, "file://") == 0) {
        auto path = local_path_from_url(url);
        if (!std::filesystem::exists(path)) return {};

        git_repository* remote_repo = nullptr;
        if (git_repository_open_bare(&remote_repo, path.c_str()) != 0) {
            return {};
        }
        auto refs = get_refs_from_repo(remote_repo);
        git_repository_free(remote_repo);
        return refs;
    }

    // Remote URL: use git_remote_ls
    RefMap refs;
    git_remote* remote = nullptr;
    if (git_remote_create_anonymous(&remote, repo, url.c_str()) != 0) return refs;

    git_remote_callbacks cbs = GIT_REMOTE_CALLBACKS_INIT;
    if (git_remote_connect(remote, GIT_DIRECTION_FETCH, &cbs, nullptr, nullptr) != 0) {
        git_remote_free(remote);
        return refs;
    }

    const git_remote_head** heads = nullptr;
    size_t count = 0;
    if (git_remote_ls(&heads, &count, remote) == 0) {
        for (size_t i = 0; i < count; ++i) {
            std::string name = heads[i]->name;
            if (name == "HEAD") continue;
            if (name.size() >= 3 &&
                name.compare(name.size() - 3, 3, "^{}") == 0) continue;
            refs[name] = oid_hex(&heads[i]->oid);
        }
    }

    git_remote_disconnect(remote);
    git_remote_free(remote);
    return refs;
}

// ---------------------------------------------------------------------------
// Diff computation
// ---------------------------------------------------------------------------

MirrorDiff diff_refs(const RefMap& src, const RefMap& dest) {
    MirrorDiff diff;

    for (auto& [ref_name, sha] : src) {
        auto it = dest.find(ref_name);
        if (it == dest.end()) {
            diff.add.push_back({ref_name, std::nullopt, sha});
        } else if (it->second != sha) {
            diff.update.push_back({ref_name, it->second, sha});
        }
    }

    for (auto& [ref_name, sha] : dest) {
        if (src.find(ref_name) == src.end()) {
            diff.del.push_back({ref_name, sha, std::nullopt});
        }
    }

    return diff;
}

// ---------------------------------------------------------------------------
// Ref name resolution
// ---------------------------------------------------------------------------

/// Resolve short ref names to full ref paths (e.g. "main" -> "refs/heads/main").
/// Names already starting with "refs/" pass through.  Otherwise tries
/// refs/heads/, refs/tags/, refs/notes/ against available_refs.
/// If no match, defaults to refs/heads/<name>.
std::set<std::string> resolve_ref_names(
    const std::vector<std::string>& names,
    const RefMap& available)
{
    std::set<std::string> result;
    for (const auto& name : names) {
        if (name.compare(0, 5, "refs/") == 0) {
            result.insert(name);
            continue;
        }
        bool found = false;
        for (const char* prefix : {"refs/heads/", "refs/tags/", "refs/notes/"}) {
            auto candidate = std::string(prefix) + name;
            if (available.find(candidate) != available.end()) {
                result.insert(candidate);
                found = true;
                break;
            }
        }
        if (!found) {
            result.insert("refs/heads/" + name);
        }
    }
    return result;
}

// ---------------------------------------------------------------------------
// Ref map resolution
// ---------------------------------------------------------------------------

/// Resolve a single short ref name against available refs.
std::string resolve_one_ref(const std::string& name, const RefMap& available) {
    if (name.compare(0, 5, "refs/") == 0) return name;
    for (const char* prefix : {"refs/heads/", "refs/tags/", "refs/notes/"}) {
        auto candidate = std::string(prefix) + name;
        if (available.find(candidate) != available.end()) {
            return candidate;
        }
    }
    return "refs/heads/" + name;
}

/// Resolve a src->dst ref map to full ref paths on both sides.
/// Returns map<full_src, full_dst>.
/// For destination names not starting with "refs/", use the same prefix
/// as the resolved source (e.g. if source resolves to refs/tags/v1.0,
/// then dest "v2.0" becomes refs/tags/v2.0).
RefMap resolve_ref_map(
    const std::map<std::string, std::string>& map,
    const RefMap& src_available,
    const RefMap& /*dst_available*/)
{
    RefMap result;
    for (const auto& [src, dst] : map) {
        auto full_src = resolve_one_ref(src, src_available);
        std::string full_dst;
        if (dst.compare(0, 5, "refs/") == 0) {
            full_dst = dst;
        } else {
            // Infer the prefix from the resolved source
            // e.g. "refs/heads/main" -> "refs/heads/"
            //      "refs/tags/v1.0"  -> "refs/tags/"
            auto last_slash = full_src.rfind('/');
            if (last_slash != std::string::npos) {
                full_dst = full_src.substr(0, last_slash + 1) + dst;
            } else {
                full_dst = "refs/heads/" + dst;
            }
        }
        result[full_src] = full_dst;
    }
    return result;
}

// ---------------------------------------------------------------------------
// Bundle detection
// ---------------------------------------------------------------------------

bool is_bundle_path(const std::string& path) {
    if (path.size() < 7) return false;
    auto ext = path.substr(path.size() - 7);
    std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);
    return ext == ".bundle";
}

// ---------------------------------------------------------------------------
// Bundle helpers
// ---------------------------------------------------------------------------

void bundle_export_impl(git_repository* repo, const std::string& path,
                        const std::vector<std::string>& refs, const RefMap& local_refs,
                        const RefMap& rename = {}, bool squash = false) {
    // Determine which refs to include
    RefMap to_export;
    if (refs.empty()) {
        to_export = local_refs;
    } else {
        auto resolved = resolve_ref_names(refs, local_refs);
        for (const auto& [k, v] : local_refs) {
            if (resolved.count(k)) to_export[k] = v;
        }
    }
    if (to_export.empty()) {
        throw GitError("bundle_export: no refs to export");
    }

    // When squash is true, create parentless commits with the same tree
    // for each ref, and use those OIDs instead of the originals.
    RefMap effective_export;
    if (squash) {
        git_signature* sig = nullptr;
        if (git_signature_now(&sig, "vost", "vost@localhost") != 0)
            throw_git("git_signature_now");

        for (const auto& [name, sha] : to_export) {
            git_oid orig_oid;
            if (git_oid_fromstr(&orig_oid, sha.c_str()) != 0) {
                git_signature_free(sig);
                throw_git("git_oid_fromstr");
            }
            git_commit* commit = nullptr;
            if (git_commit_lookup(&commit, repo, &orig_oid) != 0) {
                git_signature_free(sig);
                throw_git("git_commit_lookup");
            }
            git_tree* tree = nullptr;
            if (git_commit_tree(&tree, commit) != 0) {
                git_commit_free(commit);
                git_signature_free(sig);
                throw_git("git_commit_tree");
            }
            git_oid squashed_oid;
            if (git_commit_create(&squashed_oid, repo,
                                  nullptr, // don't update any ref
                                  sig, sig,
                                  nullptr, // encoding
                                  "squash\n",
                                  tree,
                                  0, nullptr) != 0) { // no parents
                git_tree_free(tree);
                git_commit_free(commit);
                git_signature_free(sig);
                throw_git("git_commit_create (squash)");
            }
            git_tree_free(tree);
            git_commit_free(commit);
            effective_export[name] = oid_hex(&squashed_oid);
        }
        git_signature_free(sig);
    } else {
        effective_export = to_export;
    }

    // Build packfile containing all commits and their objects.
    // Use revwalk + insert_walk to include full ancestry (insert_commit
    // only adds a single commit and its tree, not parent commits).
    git_packbuilder* pb = nullptr;
    if (git_packbuilder_new(&pb, repo) != 0)
        throw_git("git_packbuilder_new");

    git_revwalk* walk = nullptr;
    if (git_revwalk_new(&walk, repo) != 0) {
        git_packbuilder_free(pb);
        throw_git("git_revwalk_new");
    }

    for (const auto& [name, sha] : effective_export) {
        git_oid oid;
        if (git_oid_fromstr(&oid, sha.c_str()) != 0) {
            git_revwalk_free(walk);
            git_packbuilder_free(pb);
            throw_git("git_oid_fromstr");
        }
        if (git_revwalk_push(walk, &oid) != 0) {
            git_revwalk_free(walk);
            git_packbuilder_free(pb);
            throw_git("git_revwalk_push");
        }
    }

    if (git_packbuilder_insert_walk(pb, walk) != 0) {
        git_revwalk_free(walk);
        git_packbuilder_free(pb);
        throw_git("git_packbuilder_insert_walk");
    }
    git_revwalk_free(walk);

    git_buf buf = GIT_BUF_INIT;
    if (git_packbuilder_write_buf(&buf, pb) != 0) {
        git_packbuilder_free(pb);
        throw_git("git_packbuilder_write_buf");
    }
    git_packbuilder_free(pb);

    // Build bundle v2 header (use destination names if rename map provided,
    // and squashed OIDs if squash is enabled)
    std::string header = "# v2 git bundle\n";
    for (const auto& [name, sha] : effective_export) {
        auto it = rename.find(name);
        const auto& dest_name = (it != rename.end()) ? it->second : name;
        header += sha + " " + dest_name + "\n";
    }
    header += "\n"; // blank line separates header from pack data

    // Write to file
    std::ofstream out(path, std::ios::binary);
    if (!out) {
        git_buf_dispose(&buf);
        throw GitError("bundle_export: cannot open " + path);
    }
    out.write(header.data(), static_cast<std::streamsize>(header.size()));
    out.write(buf.ptr, static_cast<std::streamsize>(buf.size));
    git_buf_dispose(&buf);
    if (!out) {
        throw GitError("bundle_export: write failed");
    }
}

/// Parse bundle v2 header, returning (refs, pack_offset).
/// pack_offset is the byte position where the packfile data starts.
std::pair<RefMap, size_t> parse_bundle_header(const std::string& data) {
    const std::string sig = "# v2 git bundle\n";
    if (data.size() < sig.size() || data.compare(0, sig.size(), sig) != 0) {
        throw GitError("not a valid v2 git bundle");
    }

    // Find the blank line that separates header from pack data
    auto sep = data.find("\n\n", sig.size());
    if (sep == std::string::npos) {
        throw GitError("bundle header: missing blank-line separator");
    }

    RefMap refs;
    size_t pos = sig.size();
    while (pos < sep) {
        auto eol = data.find('\n', pos);
        if (eol == std::string::npos || eol > sep) break;
        auto line = data.substr(pos, eol - pos);
        pos = eol + 1;

        if (line.empty()) continue;
        // Skip prerequisite lines (start with '-')
        if (line[0] == '-') continue;

        auto space = line.find(' ');
        if (space == std::string::npos) continue;
        auto sha = line.substr(0, space);
        auto name = line.substr(space + 1);
        if (name == "HEAD") continue;
        refs[name] = sha;
    }

    return {refs, sep + 2}; // +2 to skip the two newlines
}

RefMap bundle_list_heads(const std::string& path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) throw GitError("bundle_list_heads: cannot open " + path);
    std::string data((std::istreambuf_iterator<char>(in)),
                      std::istreambuf_iterator<char>());
    return parse_bundle_header(data).first;
}

void bundle_import_impl(git_repository* repo, const std::string& path,
                        const std::vector<std::string>& refs,
                        const RefMap& rename = {}) {
    // Read entire bundle file
    std::ifstream in(path, std::ios::binary);
    if (!in) throw GitError("bundle_import: cannot open " + path);
    std::string data((std::istreambuf_iterator<char>(in)),
                      std::istreambuf_iterator<char>());

    // Parse header
    auto [all_refs, pack_offset] = parse_bundle_header(data);

    RefMap refs_to_import;
    if (refs.empty()) {
        refs_to_import = all_refs;
    } else {
        auto resolved = resolve_ref_names(refs, all_refs);
        for (const auto& [k, v] : all_refs) {
            if (resolved.count(k)) refs_to_import[k] = v;
        }
    }

    if (refs_to_import.empty()) return;

    // Index the packfile into the ODB
    const char* pack_data = data.data() + pack_offset;
    size_t pack_size = data.size() - pack_offset;

    // Get ODB pack directory
    std::string repo_path_str = git_repository_path(repo);
    std::filesystem::path odb_pack = std::filesystem::path(repo_path_str) / "objects" / "pack";
    std::filesystem::create_directories(odb_pack);

    git_indexer* idx = nullptr;
#if LIBGIT2_VER_MAJOR > 1 || (LIBGIT2_VER_MAJOR == 1 && LIBGIT2_VER_MINOR >= 4)
    git_indexer_options idx_opts = GIT_INDEXER_OPTIONS_INIT;
    if (git_indexer_new(&idx, odb_pack.string().c_str(), 0, nullptr, &idx_opts) != 0)
#else
    if (git_indexer_new(&idx, odb_pack.string().c_str(), 0, nullptr, nullptr) != 0)
#endif
        throw_git("git_indexer_new");

    git_indexer_progress stats = {};
    if (git_indexer_append(idx, pack_data, pack_size, &stats) != 0) {
        git_indexer_free(idx);
        throw_git("git_indexer_append");
    }
    if (git_indexer_commit(idx, &stats) != 0) {
        git_indexer_free(idx);
        throw_git("git_indexer_commit");
    }
    git_indexer_free(idx);

    // Set refs (apply rename map if provided)
    for (const auto& [name, sha] : refs_to_import) {
        auto it = rename.find(name);
        const auto& dest_name = (it != rename.end()) ? it->second : name;
        git_oid oid;
        if (git_oid_fromstr(&oid, sha.c_str()) != 0)
            throw_git("git_oid_fromstr");
        git_reference* ref_out = nullptr;
        if (git_reference_create(&ref_out, repo, dest_name.c_str(), &oid, 1, nullptr) != 0)
            throw_git("git_reference_create");
        git_reference_free(ref_out);
    }
}

// ---------------------------------------------------------------------------
// Bundle diff helpers
// ---------------------------------------------------------------------------

MirrorDiff diff_bundle_export(git_repository* repo,
                               const std::vector<std::string>& refs,
                               const RefMap& rename = {}) {
    auto local_refs = get_local_refs(repo);
    RefMap filtered;
    if (refs.empty()) {
        filtered = local_refs;
    } else {
        auto resolved = resolve_ref_names(refs, local_refs);
        for (const auto& [k, v] : local_refs) {
            if (resolved.count(k)) filtered[k] = v;
        }
    }

    MirrorDiff diff;
    for (const auto& [name, sha] : filtered) {
        auto it = rename.find(name);
        const auto& dest_name = (it != rename.end()) ? it->second : name;
        diff.add.push_back({dest_name, std::nullopt, sha});
    }
    return diff;
}

MirrorDiff diff_bundle_import(git_repository* repo, const std::string& path,
                               const std::vector<std::string>& refs,
                               const RefMap& rename = {}) {
    auto bundle_refs = bundle_list_heads(path);
    RefMap filtered;
    if (refs.empty()) {
        filtered = bundle_refs;
    } else {
        auto resolved = resolve_ref_names(refs, bundle_refs);
        for (const auto& [k, v] : bundle_refs) {
            if (resolved.count(k)) filtered[k] = v;
        }
    }

    // Apply rename to get destination ref names for diff
    RefMap dest_filtered;
    for (const auto& [name, sha] : filtered) {
        auto it = rename.find(name);
        const auto& dest_name = (it != rename.end()) ? it->second : name;
        dest_filtered[dest_name] = sha;
    }

    auto local_refs = get_local_refs(repo);
    auto diff = diff_refs(dest_filtered, local_refs);
    diff.del.clear(); // additive: no deletes
    return diff;
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

void mirror_push(git_repository* repo, const std::string& url,
                 const RefMap& local_refs, const RefMap& remote_refs) {
    git_remote* remote = nullptr;
    if (git_remote_create_anonymous(&remote, repo, url.c_str()) != 0) {
        throw_git("git_remote_create_anonymous");
    }

    // Build refspecs: force-push all local, delete stale remote
    std::vector<std::string> refspec_strs;
    for (auto& [name, sha] : local_refs) {
        refspec_strs.push_back("+" + name + ":" + name);
    }
    for (auto& [name, sha] : remote_refs) {
        if (local_refs.find(name) == local_refs.end()) {
            refspec_strs.push_back(":" + name);  // delete
        }
    }

    std::vector<char*> refspec_ptrs;
    refspec_ptrs.reserve(refspec_strs.size());
    for (auto& s : refspec_strs) {
        refspec_ptrs.push_back(const_cast<char*>(s.c_str()));
    }

    git_strarray arr;
    arr.strings = refspec_ptrs.data();
    arr.count = refspec_ptrs.size();

    git_push_options push_opts;
    git_push_options_init(&push_opts, GIT_PUSH_OPTIONS_VERSION);

    int rc = git_remote_push(remote, &arr, &push_opts);
    git_remote_free(remote);
    if (rc != 0) throw_git("git_remote_push");
}

/// Push only refs in ref_filter (no deletes on remote).
/// If rename is non-empty, maps source ref names to destination ref names.
void targeted_push(git_repository* repo, const std::string& url,
                   const RefMap& local_refs, const std::set<std::string>& ref_filter,
                   const RefMap& rename = {}) {
    git_remote* remote = nullptr;
    if (git_remote_create_anonymous(&remote, repo, url.c_str()) != 0) {
        throw_git("git_remote_create_anonymous");
    }

    std::vector<std::string> refspec_strs;
    for (const auto& name : ref_filter) {
        if (local_refs.find(name) != local_refs.end()) {
            auto it = rename.find(name);
            const auto& dest = (it != rename.end()) ? it->second : name;
            refspec_strs.push_back("+" + name + ":" + dest);
        }
    }

    std::vector<char*> refspec_ptrs;
    refspec_ptrs.reserve(refspec_strs.size());
    for (auto& s : refspec_strs) {
        refspec_ptrs.push_back(const_cast<char*>(s.c_str()));
    }

    git_strarray arr;
    arr.strings = refspec_ptrs.data();
    arr.count = refspec_ptrs.size();

    git_push_options push_opts;
    git_push_options_init(&push_opts, GIT_PUSH_OPTIONS_VERSION);

    int rc = git_remote_push(remote, &arr, &push_opts);
    git_remote_free(remote);
    if (rc != 0) throw_git("git_remote_push");
}

/// Fetch refs additively (no deletes).  If refs_filter is non-empty,
/// only fetches refs that match the filter.
/// If rename is non-empty, maps source ref names to local ref names.
void additive_fetch(git_repository* repo, const std::string& url,
                    const RefMap& remote_refs, const std::vector<std::string>& refs,
                    const RefMap& rename = {}) {
    RefMap to_fetch;
    if (refs.empty()) {
        to_fetch = remote_refs;
    } else {
        auto resolved = resolve_ref_names(refs, remote_refs);
        for (const auto& [k, v] : remote_refs) {
            if (resolved.count(k)) to_fetch[k] = v;
        }
    }

    if (to_fetch.empty()) return;

    git_remote* remote = nullptr;
    if (git_remote_create_anonymous(&remote, repo, url.c_str()) != 0) {
        throw_git("git_remote_create_anonymous");
    }

    std::vector<std::string> refspec_strs;
    for (auto& [name, sha] : to_fetch) {
        auto it = rename.find(name);
        const auto& dest = (it != rename.end()) ? it->second : name;
        refspec_strs.push_back("+" + name + ":" + dest);
    }

    std::vector<char*> refspec_ptrs;
    refspec_ptrs.reserve(refspec_strs.size());
    for (auto& s : refspec_strs) {
        refspec_ptrs.push_back(const_cast<char*>(s.c_str()));
    }

    git_strarray arr;
    arr.strings = refspec_ptrs.data();
    arr.count = refspec_ptrs.size();

    git_fetch_options fetch_opts;
    git_fetch_options_init(&fetch_opts, GIT_FETCH_OPTIONS_VERSION);

    int rc = git_remote_fetch(remote, &arr, &fetch_opts, nullptr);
    git_remote_free(remote);
    if (rc != 0) throw_git("git_remote_fetch");

    // No deletes — that's what makes it additive
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

MirrorDiff backup(const std::shared_ptr<GitStoreInner>& inner,
                  const std::string& dest, const BackupOptions& opts) {
    reject_scp_url(dest);

    bool use_bundle = opts.format == "bundle" || is_bundle_path(dest);

    std::lock_guard<std::mutex> lk(inner->mutex);

    // ref_map takes precedence over refs
    if (!opts.ref_map.empty()) {
        auto local_refs = get_local_refs(inner->repo);
        RefMap empty_dst;
        auto resolved = resolve_ref_map(opts.ref_map, local_refs, empty_dst);

        // Build refs list from map keys for filtering
        std::vector<std::string> src_refs;
        std::set<std::string> src_set;
        for (const auto& [src, dst] : resolved) {
            src_refs.push_back(src);
            src_set.insert(src);
        }

        if (use_bundle) {
            auto diff = diff_bundle_export(inner->repo, src_refs, resolved);
            if (!opts.dry_run) {
                bundle_export_impl(inner->repo, dest, src_refs, local_refs, resolved, opts.squash);
            }
            return diff;
        }

        auto_create_bare_repo(dest);
        auto remote_refs = get_remote_refs(inner->repo, dest);

        // Build diff using destination names
        RefMap src_filtered;
        for (const auto& [k, v] : local_refs) {
            if (src_set.count(k)) src_filtered[k] = v;
        }

        // Compute diff with renamed refs
        RefMap renamed_local;
        for (const auto& [src, sha] : src_filtered) {
            auto it = resolved.find(src);
            const auto& dst = (it != resolved.end()) ? it->second : src;
            renamed_local[dst] = sha;
        }
        auto diff = diff_refs(renamed_local, remote_refs);
        diff.del.clear(); // no deletes with ref_map

        if (!opts.dry_run && !diff.in_sync()) {
            targeted_push(inner->repo, dest, local_refs, src_set, resolved);
        }
        return diff;
    }

    if (use_bundle) {
        auto diff = diff_bundle_export(inner->repo, opts.refs);
        if (!opts.dry_run) {
            auto local_refs = get_local_refs(inner->repo);
            bundle_export_impl(inner->repo, dest, opts.refs, local_refs, {}, opts.squash);
        }
        return diff;
    }

    auto_create_bare_repo(dest);

    if (!opts.refs.empty()) {
        auto local_refs = get_local_refs(inner->repo);
        auto remote_refs = get_remote_refs(inner->repo, dest);
        auto ref_filter = resolve_ref_names(opts.refs, local_refs);
        auto diff = diff_refs(local_refs, remote_refs);

        // Filter to only targeted refs, no deletes
        std::vector<RefChange> filtered_add, filtered_update;
        for (auto& r : diff.add) {
            if (ref_filter.count(r.ref_name)) filtered_add.push_back(std::move(r));
        }
        for (auto& r : diff.update) {
            if (ref_filter.count(r.ref_name)) filtered_update.push_back(std::move(r));
        }
        diff.add = std::move(filtered_add);
        diff.update = std::move(filtered_update);
        diff.del.clear();

        if (!opts.dry_run && !diff.in_sync()) {
            targeted_push(inner->repo, dest, local_refs, ref_filter);
        }
        return diff;
    }

    auto local_refs = get_local_refs(inner->repo);
    auto remote_refs = get_remote_refs(inner->repo, dest);
    auto diff = diff_refs(local_refs, remote_refs);

    if (!opts.dry_run && !diff.in_sync()) {
        mirror_push(inner->repo, dest, local_refs, remote_refs);
    }

    return diff;
}

MirrorDiff restore(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& src, const RestoreOptions& opts) {
    reject_scp_url(src);

    bool use_bundle = opts.format == "bundle" || is_bundle_path(src);

    std::lock_guard<std::mutex> lk(inner->mutex);

    // ref_map takes precedence over refs
    if (!opts.ref_map.empty()) {
        if (use_bundle) {
            auto bundle_refs = bundle_list_heads(src);
            RefMap empty_dst;
            auto resolved = resolve_ref_map(opts.ref_map, bundle_refs, empty_dst);
            // Build refs list from map keys for filtering
            std::vector<std::string> src_refs;
            for (const auto& [s, d] : resolved) {
                src_refs.push_back(s);
            }
            auto diff = diff_bundle_import(inner->repo, src, src_refs, resolved);
            if (!opts.dry_run && !diff.in_sync()) {
                bundle_import_impl(inner->repo, src, src_refs, resolved);
            }
            return diff;
        }

        auto remote_refs = get_remote_refs(inner->repo, src);
        auto local_refs = get_local_refs(inner->repo);
        RefMap empty_dst;
        auto resolved = resolve_ref_map(opts.ref_map, remote_refs, empty_dst);

        // Build filtered remote refs with renamed destinations
        RefMap renamed_remote;
        for (const auto& [src_ref, dst_ref] : resolved) {
            auto it = remote_refs.find(src_ref);
            if (it != remote_refs.end()) {
                renamed_remote[dst_ref] = it->second;
            }
        }

        auto diff = diff_refs(renamed_remote, local_refs);
        diff.del.clear(); // additive: never delete

        if (!opts.dry_run && !diff.in_sync()) {
            // Build refs list from map keys
            std::vector<std::string> src_refs;
            for (const auto& [s, d] : resolved) {
                src_refs.push_back(s);
            }
            additive_fetch(inner->repo, src, remote_refs, src_refs, resolved);
        }
        return diff;
    }

    if (use_bundle) {
        auto diff = diff_bundle_import(inner->repo, src, opts.refs);
        if (!opts.dry_run && !diff.in_sync()) {
            bundle_import_impl(inner->repo, src, opts.refs);
        }
        return diff;
    }

    auto local_refs = get_local_refs(inner->repo);
    auto remote_refs = get_remote_refs(inner->repo, src);
    auto diff = diff_refs(remote_refs, local_refs);

    if (!opts.refs.empty()) {
        auto ref_filter = resolve_ref_names(opts.refs, remote_refs);
        std::vector<RefChange> filtered_add, filtered_update;
        for (auto& r : diff.add) {
            if (ref_filter.count(r.ref_name)) filtered_add.push_back(std::move(r));
        }
        for (auto& r : diff.update) {
            if (ref_filter.count(r.ref_name)) filtered_update.push_back(std::move(r));
        }
        diff.add = std::move(filtered_add);
        diff.update = std::move(filtered_update);
    }
    diff.del.clear(); // additive: never delete

    if (!opts.dry_run && !diff.in_sync()) {
        additive_fetch(inner->repo, src, remote_refs, opts.refs);
    }

    return diff;
}

void bundle_export(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& path,
                   const std::vector<std::string>& refs,
                   const std::map<std::string, std::string>& ref_map,
                   bool squash) {
    std::lock_guard<std::mutex> lk(inner->mutex);
    auto local_refs = get_local_refs(inner->repo);
    if (!ref_map.empty()) {
        RefMap empty_dst;
        auto resolved = resolve_ref_map(ref_map, local_refs, empty_dst);
        // Build refs list from map keys for filtering
        std::vector<std::string> src_refs;
        for (const auto& [s, d] : resolved) {
            src_refs.push_back(s);
        }
        bundle_export_impl(inner->repo, path, src_refs, local_refs, resolved, squash);
    } else {
        bundle_export_impl(inner->repo, path, refs, local_refs, {}, squash);
    }
}

void bundle_import(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& path,
                   const std::vector<std::string>& refs,
                   const std::map<std::string, std::string>& ref_map) {
    std::lock_guard<std::mutex> lk(inner->mutex);
    if (!ref_map.empty()) {
        // Parse bundle to get its refs for resolving the map
        std::ifstream in_file(path, std::ios::binary);
        if (!in_file) throw GitError("bundle_import: cannot open " + path);
        std::string data((std::istreambuf_iterator<char>(in_file)),
                          std::istreambuf_iterator<char>());
        auto bundle_refs = parse_bundle_header(data).first;
        RefMap empty_dst;
        auto resolved = resolve_ref_map(ref_map, bundle_refs, empty_dst);
        // Build refs list from map keys
        std::vector<std::string> src_refs;
        for (const auto& [s, d] : resolved) {
            src_refs.push_back(s);
        }
        bundle_import_impl(inner->repo, path, src_refs, resolved);
    } else {
        bundle_import_impl(inner->repo, path, refs);
    }
}

} // namespace mirror

// ---------------------------------------------------------------------------
// resolve_credentials — in vost namespace
// ---------------------------------------------------------------------------

namespace {

std::string percent_encode(const std::string& s) {
    static const char* hex = "0123456789ABCDEF";
    std::string result;
    result.reserve(s.size());
    for (unsigned char c : s) {
        if (std::isalnum(c) || c == '-' || c == '_' || c == '.' || c == '~') {
            result += static_cast<char>(c);
        } else {
            result += '%';
            result += hex[c >> 4];
            result += hex[c & 0x0F];
        }
    }
    return result;
}

/// Run a shell command and return its stdout, or empty string on failure.
std::string run_cmd(const std::string& cmd) {
    FILE* fp = popen(cmd.c_str(), "r");
    if (!fp) return {};
    char buf[4096];
    std::string output;
    while (std::fgets(buf, sizeof(buf), fp)) output += buf;
    int status = pclose(fp);
    if (status != 0) return {};
    return output;
}

/// Trim trailing whitespace.
std::string rtrim(std::string s) {
    while (!s.empty() && (s.back() == '\n' || s.back() == '\r' || s.back() == ' '))
        s.pop_back();
    return s;
}

/// Return true if hostname contains only safe characters.
bool hostname_safe(const std::string& h) {
    for (char c : h) {
        if (!std::isalnum(static_cast<unsigned char>(c)) && c != '.' && c != '-')
            return false;
    }
    return !h.empty();
}

} // anonymous namespace

std::string resolve_credentials(const std::string& url) {
    if (url.compare(0, 8, "https://") != 0) return url;

    auto after_scheme = url.substr(8);
    auto path_start = after_scheme.find('/');
    if (path_start == std::string::npos) path_start = after_scheme.size();
    auto authority = after_scheme.substr(0, path_start);

    // Already has credentials
    if (authority.find('@') != std::string::npos) return url;

    auto host = authority; // may include :port
    auto colon_pos = host.find(':');
    auto hostname = (colon_pos != std::string::npos) ? host.substr(0, colon_pos) : host;
    auto path_and_rest = after_scheme.substr(path_start);

    // Validate hostname to prevent shell injection
    if (!hostname_safe(hostname)) return url;

    // Try git credential fill
    {
        std::string cmd = "printf 'protocol=https\\nhost=" + hostname +
                          "\\n\\n' | git credential fill 2>/dev/null";
        auto output = run_cmd(cmd);
        if (!output.empty()) {
            std::string username, password;
            std::istringstream iss(output);
            std::string line;
            while (std::getline(iss, line)) {
                auto eq = line.find('=');
                if (eq != std::string::npos) {
                    auto key = line.substr(0, eq);
                    auto val = rtrim(line.substr(eq + 1));
                    if (key == "username") username = val;
                    if (key == "password") password = val;
                }
            }
            if (!username.empty() && !password.empty()) {
                return "https://" + percent_encode(username) + ":" +
                       percent_encode(password) + "@" + host + path_and_rest;
            }
        }
    }

    // Fallback: gh auth token (GitHub-specific)
    {
        std::string cmd = "gh auth token --hostname " + hostname + " 2>/dev/null";
        auto token = rtrim(run_cmd(cmd));
        if (!token.empty()) {
            return "https://x-access-token:" + token + "@" + host + path_and_rest;
        }
    }

    return url;
}

} // namespace vost
