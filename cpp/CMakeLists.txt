cmake_minimum_required(VERSION 3.20)
project(vost VERSION 0.9.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# ---- Dependencies ----------------------------------------------------------

find_package(libgit2 CONFIG REQUIRED)

# ---- Library target --------------------------------------------------------

add_library(vost
    src/gitstore.cpp
    src/fs.cpp
    src/batch.cpp
    src/tree.cpp
    src/lock.cpp
    src/paths.cpp
    src/glob.cpp
    src/copy.cpp
    src/notes.cpp
    src/mirror.cpp
)

target_include_directories(vost
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>
)

# libgit2 vcpkg target name
target_link_libraries(vost PUBLIC libgit2::libgit2package)

# POSIX file locking
if(UNIX)
    target_compile_definitions(vost PRIVATE VOST_POSIX_LOCK=1)
endif()

# ---- Compiler warnings -----------------------------------------------------

if(MSVC)
    target_compile_options(vost PRIVATE /W4 /WX)
else()
    target_compile_options(vost PRIVATE -Wall -Wextra -Wpedantic)
endif()

# ---- Tests -----------------------------------------------------------------

option(VOST_BUILD_TESTS "Build tests" ON)

if(VOST_BUILD_TESTS)
    enable_testing()
    add_subdirectory(tests)
endif()

# ---- Interop programs (optional) -------------------------------------------

option(VOST_BUILD_INTEROP "Build interop test programs" OFF)

if(VOST_BUILD_INTEROP)
    set(INTEROP_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../interop")

    add_executable(cpp_write "${INTEROP_DIR}/cpp_write.cpp")
    target_link_libraries(cpp_write PRIVATE vost)
    target_include_directories(cpp_write PRIVATE "${INTEROP_DIR}")

    add_executable(cpp_read "${INTEROP_DIR}/cpp_read.cpp")
    target_link_libraries(cpp_read PRIVATE vost)
    target_include_directories(cpp_read PRIVATE "${INTEROP_DIR}")

    # nlohmann/json is header-only and uses -Wpedantic-unfriendly constructs,
    # so relax warnings for interop targets.
    if(NOT MSVC)
        target_compile_options(cpp_write PRIVATE -Wno-pedantic)
        target_compile_options(cpp_read  PRIVATE -Wno-pedantic)
    endif()
endif()
