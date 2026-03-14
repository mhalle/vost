#pragma once

#include "error.h"
#include "fs.h"
#include "types.h"

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace vost {

// ---------------------------------------------------------------------------
// Batch — accumulate writes before committing
// ---------------------------------------------------------------------------

/// Accumulates writes and removes, then commits them atomically via commit().
///
/// Obtain a Batch via Fs::batch(). Calling commit() returns a new Fs.
///
/// Usage:
/// @code
///     auto batch = fs.batch();
///     batch.write("a.txt", data1);
///     batch.write("b.txt", data2);
///     fs = batch.commit();
/// @endcode
///
/// Fluent chaining (write() returns Batch&):
/// @code
///     fs = fs.batch()
///         .write("a.txt", data1)
///         .write("b.txt", data2)
///         .commit();
/// @endcode
class Batch {
public:
    /// Construct a Batch from an Fs snapshot.
    /// Obtain via Fs::batch() rather than constructing directly.
    explicit Batch(Fs fs, BatchOptions opts = {});

    // Non-copyable (contains move-only internal data)
    Batch(const Batch&) = delete;
    Batch& operator=(const Batch&) = delete;
    Batch(Batch&&) = default;
    Batch& operator=(Batch&&) = default;

    // -- Write staging -------------------------------------------------------

    /// Stage raw bytes at `path` with MODE_BLOB.
    /// @throws BatchClosedError if already committed.
    Batch& write(const std::string& path, const std::vector<uint8_t>& data);

    /// Stage raw bytes at `path` with an explicit mode.
    /// @throws BatchClosedError if already committed.
    Batch& write_with_mode(const std::string& path,
                           const std::vector<uint8_t>& data,
                           uint32_t mode);

    /// Stage a UTF-8 string at `path`.
    /// @throws BatchClosedError if already committed.
    Batch& write_text(const std::string& path, const std::string& text);

    /// Stage a local file from disk at `path`.
    /// @throws BatchClosedError if already committed.
    /// @throws IoError if the local file cannot be read.
    Batch& write_from_file(const std::string& path,
                           const std::filesystem::path& local_path,
                           uint32_t mode = MODE_BLOB);

    /// Stage a symlink at `path` pointing to `target`.
    /// @throws BatchClosedError if already committed.
    Batch& write_symlink(const std::string& path, const std::string& target);

    /// Stage `path` for removal.
    /// @throws BatchClosedError if already committed.
    Batch& remove(const std::string& path);

    // -- Commit --------------------------------------------------------------

    /// Commit all staged changes and return the resulting Fs.
    /// After this call the Batch is closed — further writes throw BatchClosedError.
    Fs commit();

    // -- State ---------------------------------------------------------------

    bool closed() const { return closed_; }

    size_t pending_writes()  const { return writes_.size(); }
    size_t pending_removes() const { return removes_.size(); }

    /// The result Fs after commit(). Only valid after commit() has been called.
    const std::optional<Fs>& fs() const { return result_fs_; }

private:
    void require_open() const;

    Fs                                                             fs_;
    /// Each element: (normalized_path, {data, mode}).
    /// data is empty for removes that have been superseded.
    std::vector<std::pair<std::string,
                          std::pair<std::vector<uint8_t>, uint32_t>>> writes_;
    std::vector<std::string> removes_;
    std::optional<std::string>               message_;
    std::optional<std::string>               operation_;
    std::vector<std::string>                 parents_;
    std::optional<Fs>                        result_fs_;
    bool                                     closed_ = false;
};

// ---------------------------------------------------------------------------
// BatchWriter — RAII streaming write for Batch
// ---------------------------------------------------------------------------

/// Accumulates data in memory, then stages to the batch on close().
class BatchWriter {
public:
    BatchWriter(Batch& batch, std::string path, uint32_t mode = MODE_BLOB);
    ~BatchWriter();

    /// Append raw bytes to the internal buffer.
    /// @param data Bytes to append.
    /// @return Reference to this writer for chaining.
    BatchWriter& write(const std::vector<uint8_t>& data);

    /// Append a UTF-8 string to the internal buffer.
    /// @param text String to append (encoded as UTF-8).
    /// @return Reference to this writer for chaining.
    BatchWriter& write(const std::string& text);

    /// Flush the accumulated buffer and stage the result to the batch.
    /// Called automatically by the destructor if not already closed.
    void close();

    // Non-copyable, non-movable (references a Batch)
    BatchWriter(const BatchWriter&) = delete;
    BatchWriter& operator=(const BatchWriter&) = delete;
    BatchWriter(BatchWriter&&) = delete;
    BatchWriter& operator=(BatchWriter&&) = delete;

private:
    Batch& batch_;
    std::string path_;
    uint32_t mode_;
    std::vector<uint8_t> buffer_;
    bool closed_ = false;
};

} // namespace vost
