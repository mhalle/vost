find_package(Catch2 CONFIG REQUIRED)

add_executable(vost_tests
    test_store.cpp
    test_fs_read.cpp
    test_fs_write.cpp
    test_batch.cpp
    test_history.cpp
    test_glob.cpp
    test_copy.cpp
    test_notes.cpp
    test_stat.cpp
    test_apply.cpp
    test_move.cpp
    test_mirror.cpp
    test_parents.cpp
)

target_link_libraries(vost_tests
    PRIVATE
        vost
        Catch2::Catch2WithMain
)

include(CTest)
include(Catch)
catch_discover_tests(vost_tests)
