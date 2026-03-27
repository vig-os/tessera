#ifndef FD5_INTERNAL_H
#define FD5_INTERNAL_H

#include "fd5.h"
#include <hdf5.h>

/* Hash cache entry: maps dataset HDF5 path -> SHA-256 hex string */
typedef struct fd5_hash_entry {
    char path[512];
    char hex[65]; /* 64 hex chars + null */
    struct fd5_hash_entry* next;
} fd5_hash_entry;

struct fd5_builder {
    hid_t file_id;
    char tmp_path[1024];
    char out_dir[1024];
    char product[256];
    char name[256];
    char description[1024];
    char timestamp[64];
    char* schema_json; /* dynamically allocated, NULL if not set */
    int sealed;
    fd5_hash_entry* hash_cache; /* linked list of dataset data hashes */
};

/* SHA-256 primitives */
void fd5_sha256(const void* data, size_t len, unsigned char out[32]);
void fd5_sha256_hex(const unsigned char hash[32], char out[65]);

/* Merkle tree computation */
int fd5_compute_content_hash(hid_t file_id, fd5_hash_entry* cache, char out[71]);

/* Identity hash */
int fd5_compute_id(hid_t file_id, const char** keys, int count, char out[71]);

/* Attribute serialization for deterministic hashing */
int fd5_serialize_attr(hid_t loc_id, const char* attr_name,
                       unsigned char** out, size_t* out_len);

/* Sorted-attrs hash helper (used by hash and verify) */
void fd5_sorted_attrs_hash(hid_t obj_id, const char** excluded, int nexcl,
                           char hex_out[65]);

/* Dataset hash: attrs + data */
void fd5_dataset_hash(hid_t ds_id, fd5_hash_entry* cache, char hex_out[65]);

/* Group hash: attrs + sorted children */
void fd5_group_hash(hid_t grp_id, fd5_hash_entry* cache, char hex_out[65]);

#endif /* FD5_INTERNAL_H */
