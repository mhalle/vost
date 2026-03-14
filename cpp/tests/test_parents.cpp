#include <catch2/catch_test_macros.hpp>
#include <vost/vost.h>

#include <git2.h>

#include <filesystem>
#include <string>
#include <thread>
#include <chrono>

namespace fs = std::filesystem;

static fs::path make_temp_repo() {
    auto tmp = fs::temp_directory_path() /
               ("vost_ptest_" + std::to_string(
                    std::hash<std::thread::id>{}(std::this_thread::get_id())
                    ^ static_cast<size_t>(
                          std::chrono::steady_clock::now()
                              .time_since_epoch()
                              .count())));
    return tmp;
}

static vost::GitStore open_store(const fs::path& path,
                                  const std::string& branch = "main") {
    vost::OpenOptions opts;
    opts.create = true;
    opts.branch = branch;
    return vost::GitStore::open(path, opts);
}

/// Helper: get the parent count of a commit using libgit2.
static unsigned int parent_count(const fs::path& repo_path,
                                  const std::string& commit_hex) {
    git_repository* repo = nullptr;
    REQUIRE(git_repository_open(&repo, repo_path.c_str()) == 0);

    git_oid oid;
    REQUIRE(git_oid_fromstr(&oid, commit_hex.c_str()) == 0);

    git_commit* commit = nullptr;
    REQUIRE(git_commit_lookup(&commit, repo, &oid) == 0);

    unsigned int count = git_commit_parentcount(commit);
    git_commit_free(commit);
    git_repository_free(repo);
    return count;
}

/// Helper: get the nth parent hash of a commit.
static std::string parent_hash(const fs::path& repo_path,
                                const std::string& commit_hex,
                                unsigned int n) {
    git_repository* repo = nullptr;
    REQUIRE(git_repository_open(&repo, repo_path.c_str()) == 0);

    git_oid oid;
    REQUIRE(git_oid_fromstr(&oid, commit_hex.c_str()) == 0);

    git_commit* commit = nullptr;
    REQUIRE(git_commit_lookup(&commit, repo, &oid) == 0);

    const git_oid* parent_id = git_commit_parent_id(commit, n);
    REQUIRE(parent_id != nullptr);

    char buf[GIT_OID_HEXSZ + 1];
    git_oid_tostr(buf, sizeof(buf), parent_id);
    std::string result(buf, GIT_OID_HEXSZ);

    git_commit_free(commit);
    git_repository_free(repo);
    return result;
}

// ---------------------------------------------------------------------------
// write with parents
// ---------------------------------------------------------------------------

TEST_CASE("Fs: write with advisory parents creates merge commit", "[parents]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    // Create two branches with different content
    snap = snap.write_text("a.txt", "hello");
    auto branch_a = snap;

    auto snap_b = store.branches().set_and_get("other", snap);
    snap_b = snap_b.write_text("b.txt", "world");

    // Write with advisory parent from branch_b
    vost::WriteOptions opts;
    opts.parents.push_back(*snap_b.commit_hash());
    auto result = branch_a.write_text("merged.txt", "merged", opts);

    // Verify 2 parents
    CHECK(parent_count(path, *result.commit_hash()) == 2);

    // First parent is branch_a tip
    CHECK(parent_hash(path, *result.commit_hash(), 0) == *branch_a.commit_hash());

    // Second parent is snap_b
    CHECK(parent_hash(path, *result.commit_hash(), 1) == *snap_b.commit_hash());

    // Content is correct (no tree merge — just the write)
    CHECK(result.read_text("merged.txt") == "merged");
    CHECK(result.read_text("a.txt") == "hello");

    fs::remove_all(path);
}

// ---------------------------------------------------------------------------
// batch with parents
// ---------------------------------------------------------------------------

TEST_CASE("Batch: commit with advisory parents", "[parents][batch]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("base.txt", "base");

    auto snap_other = store.branches().set_and_get("other", snap);
    snap_other = snap_other.write_text("other.txt", "data");

    vost::BatchOptions bopts;
    bopts.parents.push_back(*snap_other.commit_hash());
    auto batch = snap.batch(bopts);
    batch.write_text("batch.txt", "batch-data");
    auto result = batch.commit();

    CHECK(parent_count(path, *result.commit_hash()) == 2);
    CHECK(parent_hash(path, *result.commit_hash(), 0) == *snap.commit_hash());
    CHECK(parent_hash(path, *result.commit_hash(), 1) == *snap_other.commit_hash());

    CHECK(result.read_text("batch.txt") == "batch-data");
    CHECK(result.read_text("base.txt") == "base");

    fs::remove_all(path);
}

// ---------------------------------------------------------------------------
// apply with parents
// ---------------------------------------------------------------------------

TEST_CASE("Fs: apply with advisory parents", "[parents]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("x.txt", "x");

    auto snap_other = store.branches().set_and_get("feature", snap);
    snap_other = snap_other.write_text("y.txt", "y");

    vost::ApplyOptions aopts;
    aopts.parents.push_back(*snap_other.commit_hash());

    std::vector<std::pair<std::string, vost::WriteEntry>> writes;
    writes.push_back({"z.txt", vost::WriteEntry::from_text("z")});

    auto result = snap.apply(writes, {}, aopts);

    CHECK(parent_count(path, *result.commit_hash()) == 2);
    CHECK(parent_hash(path, *result.commit_hash(), 0) == *snap.commit_hash());
    CHECK(parent_hash(path, *result.commit_hash(), 1) == *snap_other.commit_hash());

    fs::remove_all(path);
}

// ---------------------------------------------------------------------------
// first-parent lineage preserved
// ---------------------------------------------------------------------------

TEST_CASE("Fs: parent() follows first-parent lineage with advisory parents", "[parents]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "a");
    auto first_commit = snap;

    auto snap_other = store.branches().set_and_get("side", snap);
    snap_other = snap_other.write_text("s.txt", "s");

    vost::WriteOptions opts;
    opts.parents.push_back(*snap_other.commit_hash());
    auto merged = first_commit.write_text("b.txt", "b", opts);

    // parent() should return first_commit, not snap_other
    auto p = merged.parent();
    REQUIRE(p.has_value());
    CHECK(p->commit_hash() == first_commit.commit_hash());

    fs::remove_all(path);
}

// ---------------------------------------------------------------------------
// default: no parents → 1 parent
// ---------------------------------------------------------------------------

TEST_CASE("Fs: write without parents has 1 parent (default)", "[parents]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "hello");
    auto result = snap.write_text("b.txt", "world");

    CHECK(parent_count(path, *result.commit_hash()) == 1);
    CHECK(parent_hash(path, *result.commit_hash(), 0) == *snap.commit_hash());

    fs::remove_all(path);
}

// ---------------------------------------------------------------------------
// multiple advisory parents
// ---------------------------------------------------------------------------

TEST_CASE("Fs: write with multiple advisory parents", "[parents]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "a");

    auto b1 = store.branches().set_and_get("b1", snap);
    b1 = b1.write_text("b1.txt", "b1");

    auto b2 = store.branches().set_and_get("b2", snap);
    b2 = b2.write_text("b2.txt", "b2");

    vost::WriteOptions opts;
    opts.parents.push_back(*b1.commit_hash());
    opts.parents.push_back(*b2.commit_hash());
    auto result = snap.write_text("m.txt", "m", opts);

    CHECK(parent_count(path, *result.commit_hash()) == 3);
    CHECK(parent_hash(path, *result.commit_hash(), 0) == *snap.commit_hash());
    CHECK(parent_hash(path, *result.commit_hash(), 1) == *b1.commit_hash());
    CHECK(parent_hash(path, *result.commit_hash(), 2) == *b2.commit_hash());

    fs::remove_all(path);
}
