#ifndef FD5_H
#define FD5_H

#include <hdf5.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque builder handle */
typedef struct fd5_builder fd5_builder;

/* Error codes */
#define FD5_OK           0
#define FD5_ERR_IO      -1
#define FD5_ERR_HDF5    -2
#define FD5_ERR_INVALID -3
#define FD5_ERR_HASH    -4

/* Verification status */
typedef enum {
    FD5_VALID = 0,
    FD5_INVALID = 1,
    FD5_NOT_FD5 = 2,
    FD5_VERIFY_ERROR = 3,
} fd5_verify_status;

/*
 * Create a new fd5 builder. Opens a temp HDF5 file and writes required
 * root attributes (product, name, description, timestamp, _schema_version).
 * Returns NULL on failure.
 */
fd5_builder* fd5_create(const char* out_dir,
                        const char* product,
                        const char* name,
                        const char* description,
                        const char* timestamp);

/*
 * Write an N-dimensional dataset with inline SHA-256 hashing.
 * path: HDF5 path (e.g., "volume" or "events/timestamps")
 * data: pointer to contiguous row-major data
 * shape: array of dimension sizes
 * ndims: number of dimensions
 * dtype: HDF5 datatype (e.g., H5T_NATIVE_FLOAT)
 * chunk_shape: chunk dimensions (NULL for contiguous)
 */
int fd5_write_dataset(fd5_builder* b,
                      const char* path,
                      const void* data,
                      const hsize_t* shape,
                      int ndims,
                      hid_t dtype,
                      const hsize_t* chunk_shape);

/*
 * Write a string attribute at the given HDF5 path.
 * obj_path: path to group or dataset (NULL or "" for root)
 */
int fd5_write_attr_str(fd5_builder* b,
                       const char* obj_path,
                       const char* attr_name,
                       const char* value);

/*
 * Write an int64 attribute.
 */
int fd5_write_attr_i64(fd5_builder* b,
                       const char* obj_path,
                       const char* attr_name,
                       int64_t value);

/*
 * Write a float64 attribute.
 */
int fd5_write_attr_f64(fd5_builder* b,
                       const char* obj_path,
                       const char* attr_name,
                       double value);

/*
 * Create a group at the given path. Intermediate groups are created as needed.
 */
int fd5_create_group(fd5_builder* b, const char* path);

/*
 * Embed a JSON Schema string as the _schema root attribute.
 */
int fd5_embed_schema(fd5_builder* b, const char* json_schema);

/*
 * Seal the file: validate required attrs, compute id from id_inputs,
 * compute Merkle-tree content_hash, write final attrs, close file,
 * and atomically rename to deterministic filename.
 *
 * id_inputs: NULL-terminated array of attribute key names for identity hash
 * out_path: buffer to receive the final file path (at least out_len bytes)
 * Returns FD5_OK on success.
 */
int fd5_seal(fd5_builder* b,
             const char** id_inputs,
             char* out_path,
             size_t out_len);

/*
 * Destroy builder and free resources. If not sealed, deletes the temp file.
 */
void fd5_destroy(fd5_builder* b);

/*
 * Verify an existing fd5 file's content_hash.
 * Returns FD5_VALID if hash matches, FD5_INVALID if tampered,
 * FD5_NOT_FD5 if no content_hash attribute, FD5_VERIFY_ERROR on I/O error.
 */
fd5_verify_status fd5_verify(const char* path);

#ifdef __cplusplus
}
#endif

#endif /* FD5_H */
