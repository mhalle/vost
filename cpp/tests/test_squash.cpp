#include <catch2/catch_test_macros.hpp>
#include <vost/vost.h>

#include <filesystem>
#include <string>
#include <thread>
#include <chrono>

namespace fs = std::filesystem;

static fs::path make_temp_repo() {
    auto tmp = fs::temp_directory_path() /
               ("vost_sqtest_" + std::to_string(
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

TEST_CASE("Squash: creates root commit (no parents)", "[squash]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "hello");
    snap = snap.write_text("b.txt", "world");

    auto squashed = snap.squash();

    // The squashed commit should exist
    REQUIRE(squashed.commit_hash().has_value());
    // It should differ from the original commit
    CHECK(squashed.commit_hash() != snap.commit_hash());
    // It should have no parent (root commit)
    CHECK_FALSE(squashed.parent().has_value());
    // It should be read-only (detached)
    CHECK_FALSE(squashed.writable());
    // No ref name
    CHECK_FALSE(squashed.ref_name().has_value());

    fs::remove_all(path);
}

TEST_CASE("Squash: preserves tree hash", "[squash]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "hello");
    snap = snap.write_text("b.txt", "world");

    auto squashed = snap.squash();

    CHECK(squashed.tree_hash() == snap.tree_hash());
    // Verify content is readable
    CHECK(squashed.read_text("a.txt") == "hello");
    CHECK(squashed.read_text("b.txt") == "world");

    fs::remove_all(path);
}

TEST_CASE("Squash: with parent", "[squash]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "v1");
    auto parent_snap = snap.squash(); // root squash as parent

    snap = snap.write_text("a.txt", "v2");
    auto squashed = snap.squash(parent_snap);

    // Should have a parent
    REQUIRE(squashed.parent().has_value());
    CHECK(squashed.parent()->commit_hash() == parent_snap.commit_hash());
    // Tree matches source
    CHECK(squashed.tree_hash() == snap.tree_hash());
    CHECK(squashed.read_text("a.txt") == "v2");

    fs::remove_all(path);
}

TEST_CASE("Squash: custom message", "[squash]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "hello");
    auto squashed = snap.squash(std::nullopt, "my custom message");

    CHECK(squashed.message() == "my custom message");

    fs::remove_all(path);
}

TEST_CASE("Squash: assign to branch", "[squash]") {
    auto path  = make_temp_repo();
    auto store = open_store(path);
    auto snap  = store.branches().get("main");

    snap = snap.write_text("a.txt", "hello");
    snap = snap.write_text("b.txt", "world");

    auto squashed = snap.squash();

    // Assign the squashed commit to a new branch
    store.branches().set("squashed", squashed);
    auto branch_snap = store.branches().get("squashed");

    CHECK(branch_snap.tree_hash() == snap.tree_hash());
    CHECK(branch_snap.read_text("a.txt") == "hello");
    CHECK(branch_snap.read_text("b.txt") == "world");
    // The branch snapshot should have no parent (root commit)
    CHECK_FALSE(branch_snap.parent().has_value());

    fs::remove_all(path);
}
