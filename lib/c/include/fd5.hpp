#ifndef FD5_HPP
#define FD5_HPP

#include "fd5.h"
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace fd5 {

class BuilderError : public std::runtime_error {
    using std::runtime_error::runtime_error;
};

class Builder {
    fd5_builder* b_ = nullptr;

public:
    Builder(const char* out_dir, const char* product,
            const char* name, const char* description,
            const char* timestamp)
        : b_(fd5_create(out_dir, product, name, description, timestamp))
    {
        if (!b_) throw BuilderError("fd5_create failed");
    }

    ~Builder() { if (b_) fd5_destroy(b_); }

    /* Move-only */
    Builder(Builder&& o) noexcept : b_(std::exchange(o.b_, nullptr)) {}
    Builder& operator=(Builder&& o) noexcept {
        if (b_) fd5_destroy(b_);
        b_ = std::exchange(o.b_, nullptr);
        return *this;
    }
    Builder(const Builder&) = delete;
    Builder& operator=(const Builder&) = delete;

    void write_dataset(const char* path, const void* data,
                       const hsize_t* shape, int ndims, hid_t dtype,
                       const hsize_t* chunks = nullptr) {
        if (fd5_write_dataset(b_, path, data, shape, ndims, dtype, chunks) != FD5_OK)
            throw BuilderError("fd5_write_dataset failed");
    }

    void write_attr(const char* obj_path, const char* attr, const char* value) {
        if (fd5_write_attr_str(b_, obj_path, attr, value) != FD5_OK)
            throw BuilderError("fd5_write_attr_str failed");
    }

    void write_attr(const char* obj_path, const char* attr, int64_t value) {
        if (fd5_write_attr_i64(b_, obj_path, attr, value) != FD5_OK)
            throw BuilderError("fd5_write_attr_i64 failed");
    }

    void embed_schema(const char* json) {
        if (fd5_embed_schema(b_, json) != FD5_OK)
            throw BuilderError("fd5_embed_schema failed");
    }

    std::string seal(const std::vector<std::string>& id_inputs) {
        std::vector<const char*> keys;
        for (auto& k : id_inputs) keys.push_back(k.c_str());
        keys.push_back(nullptr);

        char path[1024];
        if (fd5_seal(b_, keys.data(), path, sizeof(path)) != FD5_OK)
            throw BuilderError("fd5_seal failed");
        b_ = nullptr; /* consumed */
        return std::string(path);
    }
};

inline bool verify(const char* path) {
    return fd5_verify(path) == FD5_VALID;
}

} /* namespace fd5 */
#endif
