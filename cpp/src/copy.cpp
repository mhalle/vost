#include "vost/fs.h"
#include "vost/gitstore.h"
#include "internal.h"

#include <git2.h>

#include <algorithm>
#include <fstream>
#include <map>
#include <set>
#include <sys/stat.h>

namespace vost {

// ---------------------------------------------------------------------------
// copy helpers
// ---------------------------------------------------------------------------

namespace copy {

/// Walk a local directory recursively, returning sorted relative paths.
std::vector<std::string>
disk_walk(const std::filesystem::path& root) {
    namespace fs = std::filesystem;
    std::vector<std::string> results;
    if (!fs::exists(root)) return results;

    for (auto& entry : fs::recursive_directory_iterator(
             root, fs::directory_options::follow_directory_symlink
                 | fs::directory_options::skip_permission_denied)) {
        auto status = fs::symlink_status(entry);
        if (fs::is_directory(status)) continue;
        auto rel = fs::relative(entry.path(), root).string();
        results.push_back(rel);
    }
    std::sort(results.begin(), results.end());
    return results;
}

/// Match a path against include/exclude filters.
/// Matches against both the filename and the full relative path.
bool matches_filters(const std::string& path,
                     const std::optional<std::vector<std::string>>& include,
                     const std::optional<std::vector<std::string>>& exclude) {
    auto path_matches = [&](const std::string& pattern) {
        // Extract filename (last component)
        auto pos = path.rfind('/');
        std::string filename = (pos != std::string::npos)
            ? path.substr(pos + 1) : path;
        return glob::glob_match(pattern, filename)
            || glob::glob_match(pattern, path);
    };

    if (include) {
        bool found = false;
        for (auto& pat : *include) {
            if (path_matches(pat)) { found = true; break; }
        }
        if (!found) return false;
    }
    if (exclude) {
        for (auto& pat : *exclude) {
            if (path_matches(pat)) return false;
        }
    }
    return true;
}

/// Detect git mode from a local file's metadata.
uint32_t mode_from_disk(const std::filesystem::path& p) {
    namespace fs = std::filesystem;
    auto status = fs::symlink_status(p);
    if (fs::is_symlink(status)) return MODE_LINK;
#ifdef __unix__
    struct ::stat st;
    if (::lstat(p.c_str(), &st) == 0) {
        if (st.st_mode & S_IXUSR) return MODE_BLOB_EXEC;
    }
#elif defined(__APPLE__)
    struct ::stat st;
    if (::lstat(p.c_str(), &st) == 0) {
        if (st.st_mode & S_IXUSR) return MODE_BLOB_EXEC;
    }
#endif
    return MODE_BLOB;
}

} // namespace copy

// ---------------------------------------------------------------------------
// Fs::copy_in
// ---------------------------------------------------------------------------

std::pair<ChangeReport, Fs>
Fs::copy_in(const std::filesystem::path& src,
            const std::string& dest,
            CopyInOptions opts) const {
    require_writable("copy_in");
    const auto& tree_hex = require_tree();
    namespace fs = std::filesystem;

    std::string dest_norm = dest.empty() ? "" : paths::normalize(dest);

    // Walk disk
    auto disk_files = copy::disk_walk(src);

    // Build existing entries map (for checksum comparison)
    std::map<std::string, std::pair<std::string, uint32_t>> existing;
    if (opts.checksum) {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        // Find the subtree at dest
        std::string sub_tree = tree_hex;
        if (!dest_norm.empty()) {
            auto entry = tree::lookup(inner_->repo, tree_hex, dest_norm);
            if (entry && entry->second == MODE_TREE) {
                sub_tree = entry->first;
            } else {
                sub_tree.clear(); // no existing subtree
            }
        }
        if (!sub_tree.empty()) {
            auto walked = tree::walk_tree(inner_->repo, sub_tree,
                                          dest_norm.empty() ? "" : dest_norm);
            for (auto& [rel_path, we] : walked) {
                // Strip dest prefix
                std::string key = rel_path;
                if (!dest_norm.empty() && rel_path.size() > dest_norm.size() + 1) {
                    key = rel_path.substr(dest_norm.size() + 1);
                }
                existing[key] = {we.oid, we.mode};
            }
        }
    }

    // Build writes and report
    ChangeReport report;
    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;

    for (auto& rel : disk_files) {
        if (!copy::matches_filters(rel, opts.include, opts.exclude)) continue;

        fs::path full = src / rel;
        uint32_t mode = copy::mode_from_disk(full);

        // Read data
        std::vector<uint8_t> data;
        if (mode == MODE_LINK) {
            auto target = fs::read_symlink(full).string();
            data.assign(target.begin(), target.end());
        } else {
            std::ifstream ifs(full, std::ios::binary);
            data.assign(std::istreambuf_iterator<char>(ifs),
                        std::istreambuf_iterator<char>());
        }

        // Checksum: compare blob hash and mode
        if (opts.checksum) {
            auto it = existing.find(rel);
            if (it != existing.end()) {
                // Compute blob hash
                std::lock_guard<std::mutex> lk(inner_->mutex);
                git_oid blob_oid;
                if (git_blob_create_from_buffer(&blob_oid, inner_->repo,
                                                data.data(), data.size()) == 0) {
                    char buf[41];
                    git_oid_tostr(buf, sizeof(buf), &blob_oid);
                    std::string blob_hex(buf, 40);
                    if (blob_hex == it->second.first && mode == it->second.second) {
                        continue; // unchanged
                    }
                }
            }
        }

        // Build store path
        std::string store_path = dest_norm.empty()
            ? rel : dest_norm + "/" + rel;

        writes.push_back({store_path, {std::move(data), mode}});

        FileEntry fe;
        fe.path = store_path;
        fe.file_type = *file_type_from_mode(mode);
        fe.src = full;
        report.add.push_back(std::move(fe));
    }

    if (opts.dry_run || writes.empty()) {
        return {std::move(report), *this};
    }

    std::string msg = paths::format_message("copy_in", opts.message);
    auto new_fs = commit_changes(writes, {}, msg, std::move(report), opts.parents);
    return {new_fs.changes().value_or(ChangeReport{}), new_fs};
}

// ---------------------------------------------------------------------------
// Fs::copy_out
// ---------------------------------------------------------------------------

ChangeReport
Fs::copy_out(const std::string& src_path,
             const std::filesystem::path& dest,
             CopyOutOptions opts) const {
    const auto& tree_hex = require_tree();
    namespace fs = std::filesystem;

    std::string src_norm = src_path.empty() ? "" : paths::normalize(src_path);

    // Walk repo tree at src
    std::vector<std::pair<std::string, WalkEntry>> entries;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        entries = tree::walk_tree(inner_->repo, tree_hex,
                                  src_norm.empty() ? "" : src_norm);
    }

    ChangeReport report;

    for (auto& [rel_path, we] : entries) {
        // Strip src prefix to get relative path
        std::string rel = rel_path;
        if (!src_norm.empty() && rel.size() > src_norm.size() + 1) {
            rel = rel.substr(src_norm.size() + 1);
        }

        if (!copy::matches_filters(rel, opts.include, opts.exclude)) continue;

        fs::path dest_path = dest / rel;
        fs::create_directories(dest_path.parent_path());

        // Read blob data
        std::vector<uint8_t> data;
        {
            std::lock_guard<std::mutex> lk(inner_->mutex);
            data = tree::read_blob(inner_->repo, tree_hex, rel_path);
        }

        if (we.mode == MODE_LINK) {
            // Symlink
            std::string target(data.begin(), data.end());
            if (fs::exists(fs::symlink_status(dest_path))) {
                fs::remove(dest_path);
            }
#ifdef __APPLE__
            fs::create_symlink(target, dest_path);
#elif defined(__unix__)
            fs::create_symlink(target, dest_path);
#else
            // Write target as text on non-Unix
            std::ofstream ofs(dest_path, std::ios::binary);
            ofs.write(reinterpret_cast<const char*>(data.data()), data.size());
#endif
        } else {
            // Regular file
            std::ofstream ofs(dest_path, std::ios::binary);
            ofs.write(reinterpret_cast<const char*>(data.data()), data.size());
        }

        // Set executable bit
        if (we.mode == MODE_BLOB_EXEC) {
#if defined(__APPLE__) || defined(__unix__)
            fs::permissions(dest_path, fs::perms::owner_exec | fs::perms::group_exec,
                            fs::perm_options::add);
#endif
        }

        FileEntry fe;
        fe.path = rel;
        fe.file_type = *file_type_from_mode(we.mode);
        report.add.push_back(std::move(fe));
    }

    return report;
}

// ---------------------------------------------------------------------------
// Fs::sync_in
// ---------------------------------------------------------------------------

std::pair<ChangeReport, Fs>
Fs::sync_in(const std::filesystem::path& src,
            const std::string& dest,
            SyncOptions opts) const {
    require_writable("sync_in");
    const auto& tree_hex = require_tree();
    namespace fs = std::filesystem;

    std::string dest_norm = dest.empty() ? "" : paths::normalize(dest);

    // Walk disk
    auto disk_files = copy::disk_walk(src);

    // Walk existing repo entries at dest
    std::map<std::string, std::pair<std::string, uint32_t>> existing;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        std::string sub_tree = tree_hex;
        if (!dest_norm.empty()) {
            auto entry = tree::lookup(inner_->repo, tree_hex, dest_norm);
            if (entry && entry->second == MODE_TREE) {
                sub_tree = entry->first;
            } else {
                sub_tree.clear();
            }
        }
        if (!sub_tree.empty()) {
            auto walked = tree::walk_tree(inner_->repo, sub_tree,
                                          dest_norm.empty() ? "" : dest_norm);
            for (auto& [rel_path, we] : walked) {
                std::string key = rel_path;
                if (!dest_norm.empty() && rel_path.size() > dest_norm.size() + 1) {
                    key = rel_path.substr(dest_norm.size() + 1);
                }
                existing[key] = {we.oid, we.mode};
            }
        }
    }

    // Build writes, removes, and report
    ChangeReport report;
    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    std::set<std::string> disk_set; // track what's on disk

    for (auto& rel : disk_files) {
        if (!copy::matches_filters(rel, opts.include, opts.exclude)) continue;
        disk_set.insert(rel);

        fs::path full = src / rel;
        uint32_t mode = copy::mode_from_disk(full);

        std::vector<uint8_t> data;
        if (mode == MODE_LINK) {
            auto target = fs::read_symlink(full).string();
            data.assign(target.begin(), target.end());
        } else {
            std::ifstream ifs(full, std::ios::binary);
            data.assign(std::istreambuf_iterator<char>(ifs),
                        std::istreambuf_iterator<char>());
        }

        // Checksum comparison
        bool is_update = false;
        if (opts.checksum) {
            auto it = existing.find(rel);
            if (it != existing.end()) {
                std::lock_guard<std::mutex> lk(inner_->mutex);
                git_oid blob_oid;
                if (git_blob_create_from_buffer(&blob_oid, inner_->repo,
                                                data.data(), data.size()) == 0) {
                    char buf[41];
                    git_oid_tostr(buf, sizeof(buf), &blob_oid);
                    std::string blob_hex(buf, 40);
                    if (blob_hex == it->second.first && mode == it->second.second) {
                        continue; // unchanged
                    }
                }
                is_update = true;
            }
        } else {
            is_update = existing.count(rel) > 0;
        }

        std::string store_path = dest_norm.empty()
            ? rel : dest_norm + "/" + rel;

        writes.push_back({store_path, {std::move(data), mode}});

        FileEntry fe;
        fe.path = store_path;
        fe.file_type = *file_type_from_mode(mode);
        fe.src = full;
        if (is_update) {
            report.update.push_back(std::move(fe));
        } else {
            report.add.push_back(std::move(fe));
        }
    }

    // Determine deletes: repo files not on disk
    std::vector<std::string> removes;
    for (auto& [rel, oid_mode] : existing) {
        if (disk_set.count(rel) == 0) {
            // Check if it matches filters (only delete filtered-in files)
            if (!copy::matches_filters(rel, opts.include, opts.exclude)) continue;

            std::string store_path = dest_norm.empty()
                ? rel : dest_norm + "/" + rel;
            removes.push_back(store_path);

            FileEntry fe;
            fe.path = store_path;
            fe.file_type = *file_type_from_mode(oid_mode.second);
            report.del.push_back(std::move(fe));
        }
    }

    if (opts.dry_run || (writes.empty() && removes.empty())) {
        return {std::move(report), *this};
    }

    std::string msg = paths::format_message("sync_in", opts.message);
    auto new_fs = commit_changes(writes, removes, msg, std::move(report), opts.parents);
    return {new_fs.changes().value_or(ChangeReport{}), new_fs};
}

// ---------------------------------------------------------------------------
// Fs::sync_out
// ---------------------------------------------------------------------------

ChangeReport
Fs::sync_out(const std::string& src_path,
             const std::filesystem::path& dest,
             SyncOptions opts) const {
    const auto& tree_hex = require_tree();
    namespace fs = std::filesystem;

    std::string src_norm = src_path.empty() ? "" : paths::normalize(src_path);

    // Walk repo tree at src
    std::vector<std::pair<std::string, WalkEntry>> entries;
    {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        entries = tree::walk_tree(inner_->repo, tree_hex,
                                  src_norm.empty() ? "" : src_norm);
    }

    // Walk local disk at dest
    std::set<std::string> repo_rels;

    ChangeReport report;

    // Copy repo → disk (add/update)
    for (auto& [rel_path, we] : entries) {
        std::string rel = rel_path;
        if (!src_norm.empty() && rel.size() > src_norm.size() + 1) {
            rel = rel.substr(src_norm.size() + 1);
        }

        if (!copy::matches_filters(rel, opts.include, opts.exclude)) continue;
        repo_rels.insert(rel);

        fs::path dest_path = dest / rel;
        fs::create_directories(dest_path.parent_path());

        std::vector<uint8_t> data;
        {
            std::lock_guard<std::mutex> lk(inner_->mutex);
            data = tree::read_blob(inner_->repo, tree_hex, rel_path);
        }

        if (we.mode == MODE_LINK) {
            std::string target(data.begin(), data.end());
            if (fs::exists(fs::symlink_status(dest_path))) {
                fs::remove(dest_path);
            }
            fs::create_symlink(target, dest_path);
        } else {
            std::ofstream ofs(dest_path, std::ios::binary);
            ofs.write(reinterpret_cast<const char*>(data.data()), data.size());
        }

        if (we.mode == MODE_BLOB_EXEC) {
#if defined(__APPLE__) || defined(__unix__)
            fs::permissions(dest_path, fs::perms::owner_exec | fs::perms::group_exec,
                            fs::perm_options::add);
#endif
        }

        FileEntry fe;
        fe.path = rel;
        fe.file_type = *file_type_from_mode(we.mode);
        report.add.push_back(std::move(fe));
    }

    // Delete extra local files not in repo
    auto local_files = copy::disk_walk(dest);
    for (auto& local_rel : local_files) {
        if (!copy::matches_filters(local_rel, opts.include, opts.exclude)) continue;
        if (repo_rels.count(local_rel) == 0) {
            fs::path to_remove = dest / local_rel;
            fs::remove(to_remove);

            FileEntry fe;
            fe.path = local_rel;
            fe.file_type = FileType::Blob; // best guess
            report.del.push_back(std::move(fe));
        }
    }

    // Prune empty directories
    // Walk bottom-up by collecting all dirs and sorting reverse
    std::vector<fs::path> dirs;
    if (fs::exists(dest)) {
        for (auto& entry : fs::recursive_directory_iterator(dest)) {
            if (fs::is_directory(entry.status())) {
                dirs.push_back(entry.path());
            }
        }
    }
    std::sort(dirs.begin(), dirs.end(), std::greater<>());
    for (auto& d : dirs) {
        if (fs::is_empty(d)) {
            fs::remove(d);
        }
    }

    return report;
}

// ---------------------------------------------------------------------------
// Fs::copy_from_ref
// ---------------------------------------------------------------------------

Fs Fs::copy_from_ref(const Fs& source,
                      const std::vector<std::string>& sources,
                      const std::string& dest,
                      CopyFromRefOptions opts) const {
    require_writable("copy_from_ref");

    // Source must have a tree
    if (source.tree_oid_hex().empty()) {
        throw NotFoundError("source has no tree");
    }

    std::string dest_norm = dest.empty() ? "" : paths::normalize(dest);

    // Collect writes from source
    std::vector<std::pair<std::string, std::pair<std::vector<uint8_t>, uint32_t>>> writes;
    std::set<std::string> source_dest_paths;

    {
        std::lock_guard<std::mutex> lk(inner_->mutex);

        for (auto& src_path : sources) {
            std::string src_norm = src_path.empty() ? "" : paths::normalize(src_path);

            // Check if source path has trailing slash (contents mode)
            bool contents_mode = !src_path.empty() && src_path.back() == '/';

            // Walk source tree at src_norm
            std::vector<std::pair<std::string, WalkEntry>> src_entries;
            if (src_norm.empty()) {
                src_entries = tree::walk_tree(inner_->repo, source.tree_oid_hex(), "");
            } else {
                auto entry = tree::lookup(inner_->repo, source.tree_oid_hex(), src_norm);
                if (!entry) throw NotFoundError(src_norm);

                if (entry->second == MODE_TREE) {
                    src_entries = tree::walk_tree(inner_->repo, source.tree_oid_hex(), src_norm);
                } else {
                    // Single file
                    auto data = tree::read_blob(inner_->repo, source.tree_oid_hex(), src_norm);
                    // Determine target name
                    std::string target;
                    if (dest_norm.empty()) {
                        auto slash = src_norm.rfind('/');
                        target = (slash != std::string::npos) ? src_norm.substr(slash + 1) : src_norm;
                    } else {
                        // Check if dest is a directory in target
                        auto dest_entry = tree::lookup(inner_->repo, tree_oid_hex_, dest_norm);
                        if (dest_entry && dest_entry->second == MODE_TREE) {
                            auto slash = src_norm.rfind('/');
                            std::string basename = (slash != std::string::npos)
                                ? src_norm.substr(slash + 1) : src_norm;
                            target = dest_norm + "/" + basename;
                        } else {
                            target = dest_norm;
                        }
                    }
                    writes.push_back({target, {std::move(data), entry->second}});
                    source_dest_paths.insert(target);
                    continue;
                }
            }

            // Map source entries to dest paths
            for (auto& [rel_path, we] : src_entries) {
                std::string rel;
                if (src_norm.empty()) {
                    rel = rel_path;
                } else {
                    if (rel_path.size() > src_norm.size() + 1) {
                        rel = rel_path.substr(src_norm.size() + 1);
                    } else {
                        continue; // shouldn't happen
                    }
                }

                std::string target;
                if (contents_mode || src_norm.empty()) {
                    // Contents mode: copy contents directly into dest
                    target = dest_norm.empty() ? rel : dest_norm + "/" + rel;
                } else {
                    // Directory mode: copy the directory itself into dest
                    auto slash = src_norm.rfind('/');
                    std::string dir_name = (slash != std::string::npos)
                        ? src_norm.substr(slash + 1) : src_norm;
                    target = dest_norm.empty()
                        ? dir_name + "/" + rel
                        : dest_norm + "/" + dir_name + "/" + rel;
                }

                auto data = tree::read_blob(inner_->repo, source.tree_oid_hex(), rel_path);
                writes.push_back({target, {std::move(data), we.mode}});
                source_dest_paths.insert(target);
            }
        }
    }

    // If delete_extra, find files at dest that are not in source
    std::vector<std::string> removes;
    if (opts.delete_extra && !tree_oid_hex_.empty()) {
        std::lock_guard<std::mutex> lk(inner_->mutex);
        std::vector<std::pair<std::string, WalkEntry>> dest_entries;
        if (dest_norm.empty()) {
            dest_entries = tree::walk_tree(inner_->repo, tree_oid_hex_, "");
        } else {
            auto entry = tree::lookup(inner_->repo, tree_oid_hex_, dest_norm);
            if (entry && entry->second == MODE_TREE) {
                dest_entries = tree::walk_tree(inner_->repo, tree_oid_hex_, dest_norm);
            }
        }

        for (auto& [rel_path, we] : dest_entries) {
            if (source_dest_paths.count(rel_path) == 0) {
                removes.push_back(rel_path);
            }
        }
    }

    if (opts.dry_run || (writes.empty() && removes.empty())) {
        return *this;
    }

    std::string msg = paths::format_message("copy_from_ref", opts.message);
    return commit_changes(writes, removes, msg, std::nullopt, opts.parents);
}

Fs Fs::copy_from_ref(const std::string& source_name,
                      const std::vector<std::string>& sources,
                      const std::string& dest,
                      CopyFromRefOptions opts) const {
    // Resolve name to Fs via branches, then tags
    RefDict branches(inner_, "refs/heads/", true);
    try {
        auto src_fs = branches[source_name];
        return copy_from_ref(src_fs, sources, dest, opts);
    } catch (const KeyNotFoundError&) {}
    RefDict tags(inner_, "refs/tags/", false);
    try {
        auto src_fs = tags[source_name];
        return copy_from_ref(src_fs, sources, dest, opts);
    } catch (const KeyNotFoundError&) {}
    throw InvalidHashError(source_name);
}

// ---------------------------------------------------------------------------
// ExcludeFilter
// ---------------------------------------------------------------------------

void ExcludeFilter::add_patterns(const std::vector<std::string>& patterns) {
    for (auto& raw : patterns) {
        if (raw.empty() || raw[0] == '#') continue;

        Pattern p;
        std::string s = raw;

        if (s[0] == '!') {
            p.negated = true;
            s = s.substr(1);
        }

        if (!s.empty() && s.back() == '/') {
            p.dir_only = true;
            s.pop_back();
        }

        p.raw = s;
        patterns_.push_back(std::move(p));
    }
}

void ExcludeFilter::load_from_file(const std::filesystem::path& path) {
    std::ifstream ifs(path);
    if (!ifs) return;
    std::vector<std::string> lines;
    std::string line;
    while (std::getline(ifs, line)) {
        // Trim trailing whitespace
        while (!line.empty() && (line.back() == ' ' || line.back() == '\r')) {
            line.pop_back();
        }
        if (!line.empty()) lines.push_back(line);
    }
    add_patterns(lines);
}

bool ExcludeFilter::match_pattern(const std::string& pattern,
                                   const std::string& path) {
    // If pattern contains /, match against full path; otherwise match basename
    if (pattern.find('/') != std::string::npos) {
        return glob::fnmatch(pattern, path);
    }
    // Match against basename
    auto slash = path.rfind('/');
    std::string basename = (slash != std::string::npos)
        ? path.substr(slash + 1) : path;
    return glob::fnmatch(pattern, basename);
}

bool ExcludeFilter::is_excluded(const std::string& rel_path, bool is_dir) const {
    bool excluded = false;
    for (auto& p : patterns_) {
        if (p.dir_only && !is_dir) continue;
        if (match_pattern(p.raw, rel_path)) {
            excluded = !p.negated;
        }
    }
    return excluded;
}

} // namespace vost
