#include "vost/batch.h"
#include "vost/fs.h"
#include "internal.h"

#include <fstream>

namespace vost {

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

Batch::Batch(Fs fs, BatchOptions opts)
    : fs_(std::move(fs))
    , message_(std::move(opts.message))
    , operation_(std::move(opts.operation))
    , parents_(std::move(opts.parents))
{}

void Batch::require_open() const {
    if (closed_) throw BatchClosedError();
}

// ---------------------------------------------------------------------------
// Write staging
// ---------------------------------------------------------------------------

Batch& Batch::write(const std::string& path, const std::vector<uint8_t>& data) {
    return write_with_mode(path, data, MODE_BLOB);
}

Batch& Batch::write_with_mode(const std::string& path,
                               const std::vector<uint8_t>& data,
                               uint32_t mode) {
    require_open();
    std::string norm = paths::normalize(path);

    // Remove from removes list if present
    removes_.erase(std::remove(removes_.begin(), removes_.end(), norm),
                   removes_.end());

    // Remove existing write for same path
    writes_.erase(
        std::remove_if(writes_.begin(), writes_.end(),
                       [&norm](const auto& kv) { return kv.first == norm; }),
        writes_.end());

    writes_.push_back({norm, {data, mode}});
    return *this;
}

Batch& Batch::write_from_file(const std::string& path,
                               const std::filesystem::path& local_path,
                               uint32_t mode) {
    namespace fss = std::filesystem;
    if (!fss::exists(local_path)) {
        throw IoError("file not found: " + local_path.string());
    }

    std::ifstream ifs(local_path, std::ios::binary);
    if (!ifs) {
        throw IoError("cannot open file: " + local_path.string());
    }
    std::vector<uint8_t> data{std::istreambuf_iterator<char>(ifs),
                               std::istreambuf_iterator<char>()};

    write_with_mode(path, data, mode);
    return *this;
}

Batch& Batch::write_text(const std::string& path, const std::string& text) {
    std::vector<uint8_t> data(text.begin(), text.end());
    return write(path, data);
}

Batch& Batch::write_symlink(const std::string& path, const std::string& target) {
    std::vector<uint8_t> data(target.begin(), target.end());
    return write_with_mode(path, data, MODE_LINK);
}

Batch& Batch::remove(const std::string& path) {
    require_open();
    std::string norm = paths::normalize(path);
    // Remove any pending write for this path
    writes_.erase(
        std::remove_if(writes_.begin(), writes_.end(),
                       [&norm](const auto& kv) { return kv.first == norm; }),
        writes_.end());

    // Add to removes if not already there
    if (std::find(removes_.begin(), removes_.end(), norm) == removes_.end()) {
        removes_.push_back(norm);
    }
    return *this;
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

Fs Batch::commit() {
    require_open();
    closed_ = true;

    std::string msg;
    if (message_) {
        msg = *message_;
    } else {
        // Auto-generate from staged operations
        std::string op = operation_.value_or("batch");
        if (!writes_.empty() && removes_.empty()) {
            msg = op + ": write " + std::to_string(writes_.size()) + " file(s)";
        } else if (writes_.empty() && !removes_.empty()) {
            msg = op + ": remove " + std::to_string(removes_.size()) + " file(s)";
        } else {
            msg = op + ": " + std::to_string(writes_.size()) + " write(s), " +
                  std::to_string(removes_.size()) + " remove(s)";
        }
    }

    // Delegate to Fs::commit_changes (internal)
    Fs result = fs_.commit_changes(writes_, removes_, msg, std::nullopt, parents_);
    result_fs_ = result;
    return result;
}

// ---------------------------------------------------------------------------
// BatchWriter
// ---------------------------------------------------------------------------

BatchWriter::BatchWriter(Batch& batch, std::string path, uint32_t mode)
    : batch_(batch)
    , path_(std::move(path))
    , mode_(mode)
{}

BatchWriter::~BatchWriter() {
    if (!closed_) {
        try { close(); } catch (...) {}
    }
}

BatchWriter& BatchWriter::write(const std::vector<uint8_t>& data) {
    if (closed_) throw BatchClosedError();
    buffer_.insert(buffer_.end(), data.begin(), data.end());
    return *this;
}

BatchWriter& BatchWriter::write(const std::string& text) {
    if (closed_) throw BatchClosedError();
    buffer_.insert(buffer_.end(), text.begin(), text.end());
    return *this;
}

void BatchWriter::close() {
    if (closed_) throw BatchClosedError();
    closed_ = true;
    batch_.write_with_mode(path_, buffer_, mode_);
}

} // namespace vost
