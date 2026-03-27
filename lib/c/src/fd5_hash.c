/*
 * fd5_hash.c -- SHA-256 helpers, Merkle tree, and identity hash computation.
 *
 * Implements the same algorithm as Python's fd5.hash module:
 *   - _sorted_attrs_hash: sha256(sha256(key+serialize(val)) for key in sorted)
 *   - _dataset_hash: sha256(attrs_hash + data_hash)
 *   - _group_hash: sha256(attrs_hash + child_hashes...)
 *   - content_hash: sha256(root_group_hash)
 */
#include "fd5_internal.h"
#include "sha256.h"

#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ------------------------------------------------------------------ */
/* SHA-256 convenience wrappers                                        */
/* ------------------------------------------------------------------ */

void fd5_sha256(const void* data, size_t len, unsigned char out[32])
{
    sha256_ctx ctx;
    sha256_init(&ctx);
    sha256_update(&ctx, data, len);
    sha256_final(&ctx, out);
}

void fd5_sha256_hex(const unsigned char hash[32], char out[65])
{
    static const char hex[] = "0123456789abcdef";
    for (int i = 0; i < 32; i++) {
        out[i * 2]     = hex[hash[i] >> 4];
        out[i * 2 + 1] = hex[hash[i] & 0x0f];
    }
    out[64] = '\0';
}

/* ------------------------------------------------------------------ */
/* Attribute serialization (matches Python _serialize_attr)            */
/* ------------------------------------------------------------------ */

/*
 * Serialize an HDF5 attribute value to bytes matching the Python algorithm.
 * - Strings -> UTF-8 bytes
 * - Integers -> native (little-endian on x86/ARM) bytes via H5Aread
 * - Floats -> native bytes via H5Aread
 * - Arrays -> concatenated element bytes
 *
 * The Python code does:
 *   if str: value.encode("utf-8")
 *   if ndarray: value.tobytes()
 *   if np.generic: np.array(value).tobytes()
 *   else: str(value).encode("utf-8")
 *
 * HDF5 stores numeric values in the file's byte order, and H5Aread converts
 * to native order. numpy's tobytes() also returns native order.
 * So reading with native types matches Python.
 */
int fd5_serialize_attr(hid_t loc_id, const char* attr_name,
                       unsigned char** out, size_t* out_len)
{
    hid_t attr_id = H5Aopen(loc_id, attr_name, H5P_DEFAULT);
    if (attr_id < 0) return FD5_ERR_HDF5;

    hid_t type_id = H5Aget_type(attr_id);
    hid_t space_id = H5Aget_space(attr_id);
    H5T_class_t cls = H5Tget_class(type_id);

    if (cls == H5T_STRING) {
        if (H5Tis_variable_str(type_id)) {
            char* str = NULL;
            hid_t memtype = H5Tcopy(H5T_C_S1);
            H5Tset_size(memtype, H5T_VARIABLE);
            H5Aread(attr_id, memtype, &str);
            size_t slen = strlen(str);
            *out = (unsigned char*)malloc(slen);
            memcpy(*out, str, slen);
            *out_len = slen;
            H5free_memory(str);
            H5Tclose(memtype);
        } else {
            size_t sz = H5Tget_size(type_id);
            char* buf = (char*)malloc(sz + 1);
            H5Aread(attr_id, type_id, buf);
            buf[sz] = '\0';
            size_t slen = strlen(buf); /* trim trailing nulls */
            *out = (unsigned char*)malloc(slen);
            memcpy(*out, buf, slen);
            *out_len = slen;
            free(buf);
        }
    } else {
        /* Numeric or other types: read as native bytes (matches numpy tobytes) */
        hssize_t npts = H5Sget_simple_extent_npoints(space_id);
        if (npts < 1) npts = 1;

        hid_t read_type;
        if (cls == H5T_INTEGER || cls == H5T_FLOAT) {
            read_type = H5Tget_native_type(type_id, H5T_DIR_DEFAULT);
        } else {
            read_type = H5Tcopy(type_id);
        }
        size_t total = H5Tget_size(read_type) * (size_t)npts;

        *out = (unsigned char*)malloc(total);
        H5Aread(attr_id, read_type, *out);
        *out_len = total;
        H5Tclose(read_type);
    }

    H5Sclose(space_id);
    H5Tclose(type_id);
    H5Aclose(attr_id);
    return FD5_OK;
}

/* ------------------------------------------------------------------ */
/* _sorted_attrs_hash                                                  */
/* ------------------------------------------------------------------ */

static int cmp_strings(const void* a, const void* b)
{
    return strcmp(*(const char**)a, *(const char**)b);
}

static int is_excluded(const char* name, const char** excluded, int nexcl)
{
    for (int i = 0; i < nexcl; i++) {
        if (strcmp(name, excluded[i]) == 0) return 1;
    }
    return 0;
}

/*
 * Matches Python's _sorted_attrs_hash:
 *   h = sha256()
 *   for key in sorted(attrs):
 *       if key in excluded: continue
 *       inner = sha256(key.encode("utf-8") + serialize(val)).hexdigest()
 *       h.update(inner.encode("utf-8"))
 *   return h.hexdigest()
 */
void fd5_sorted_attrs_hash(hid_t obj_id, const char** excluded, int nexcl,
                           char hex_out[65])
{
    int nattrs = (int)H5Aget_num_attrs(obj_id);

    char** names = (char**)calloc((size_t)nattrs, sizeof(char*));
    int count = 0;
    for (int i = 0; i < nattrs; i++) {
        hid_t aid = H5Aopen_idx(obj_id, (unsigned)i);
        if (aid < 0) continue;
        ssize_t nlen = H5Aget_name(aid, 0, NULL);
        names[count] = (char*)malloc((size_t)(nlen + 1));
        H5Aget_name(aid, (size_t)(nlen + 1), names[count]);
        H5Aclose(aid);
        count++;
    }

    qsort(names, (size_t)count, sizeof(char*), cmp_strings);

    sha256_ctx outer;
    sha256_init(&outer);

    for (int i = 0; i < count; i++) {
        if (is_excluded(names[i], excluded, nexcl)) {
            free(names[i]);
            continue;
        }

        unsigned char* val_bytes = NULL;
        size_t val_len = 0;
        fd5_serialize_attr(obj_id, names[i], &val_bytes, &val_len);

        /* inner = sha256(key_bytes + value_bytes) */
        sha256_ctx inner;
        sha256_init(&inner);
        sha256_update(&inner, names[i], strlen(names[i]));
        if (val_bytes && val_len > 0)
            sha256_update(&inner, val_bytes, val_len);

        unsigned char inner_hash[32];
        sha256_final(&inner, inner_hash);

        char inner_hex[65];
        fd5_sha256_hex(inner_hash, inner_hex);

        sha256_update(&outer, inner_hex, 64);

        free(val_bytes);
        free(names[i]);
    }
    free(names);

    unsigned char final_hash[32];
    sha256_final(&outer, final_hash);
    fd5_sha256_hex(final_hash, hex_out);
}

/* ------------------------------------------------------------------ */
/* _dataset_hash                                                       */
/* ------------------------------------------------------------------ */

static const char* find_cache_entry(fd5_hash_entry* cache, const char* path)
{
    for (fd5_hash_entry* e = cache; e; e = e->next) {
        if (strcmp(e->path, path) == 0) return e->hex;
    }
    return NULL;
}

/*
 * Matches Python's _dataset_hash / _dataset_hash_cached:
 *   data_hash = sha256(ds[...].tobytes()).hexdigest()
 *   attrs_h = _sorted_attrs_hash(ds)
 *   return sha256((attrs_h + data_hash).encode("utf-8")).hexdigest()
 */
void fd5_dataset_hash(hid_t ds_id, fd5_hash_entry* cache, char hex_out[65])
{
    char ds_path[512];
    H5Iget_name(ds_id, ds_path, sizeof(ds_path));

    char data_hex[65];
    const char* cached = find_cache_entry(cache, ds_path);

    if (cached) {
        memcpy(data_hex, cached, 65);
    } else {
        hid_t space = H5Dget_space(ds_id);
        hid_t dtype = H5Dget_type(ds_id);
        hid_t native = H5Tget_native_type(dtype, H5T_DIR_DEFAULT);

        hssize_t npts = H5Sget_simple_extent_npoints(space);
        size_t elem_sz = H5Tget_size(native);
        size_t total = (size_t)npts * elem_sz;

        void* buf = malloc(total);
        H5Dread(ds_id, native, H5S_ALL, H5S_ALL, H5P_DEFAULT, buf);

        unsigned char hash[32];
        fd5_sha256(buf, total, hash);
        fd5_sha256_hex(hash, data_hex);

        free(buf);
        H5Tclose(native);
        H5Tclose(dtype);
        H5Sclose(space);
    }

    char attrs_hex[65];
    fd5_sorted_attrs_hash(ds_id, NULL, 0, attrs_hex);

    sha256_ctx ctx;
    sha256_init(&ctx);
    sha256_update(&ctx, attrs_hex, 64);
    sha256_update(&ctx, data_hex, 64);

    unsigned char final_hash[32];
    sha256_final(&ctx, final_hash);
    fd5_sha256_hex(final_hash, hex_out);
}

/* ------------------------------------------------------------------ */
/* _group_hash                                                         */
/* ------------------------------------------------------------------ */

static int is_chunk_hashes(const char* name)
{
    const char* suffix = "_chunk_hashes";
    size_t nlen = strlen(name);
    size_t slen = strlen(suffix);
    if (nlen < slen) return 0;
    return strcmp(name + nlen - slen, suffix) == 0;
}

/*
 * Matches Python's _group_hash / _group_hash_cached:
 *   h = sha256()
 *   h.update(sorted_attrs_hash(group))
 *   for key in sorted(keys):
 *       if chunk_hashes or external_link: continue
 *       if group: h.update(group_hash(child))
 *       if dataset: h.update(dataset_hash(child))
 *   return h.hexdigest()
 */
void fd5_group_hash(hid_t grp_id, fd5_hash_entry* cache, char hex_out[65])
{
    static const char* excl[] = {"content_hash"};

    sha256_ctx ctx;
    sha256_init(&ctx);

    char attrs_hex[65];
    fd5_sorted_attrs_hash(grp_id, excl, 1, attrs_hex);
    sha256_update(&ctx, attrs_hex, 64);

    hsize_t num_objs = 0;
    H5Gget_num_objs(grp_id, &num_objs);

    char** child_names = (char**)calloc((size_t)num_objs, sizeof(char*));
    int nchildren = 0;

    for (hsize_t i = 0; i < num_objs; i++) {
        ssize_t nlen = H5Gget_objname_by_idx(grp_id, i, NULL, 0);
        child_names[nchildren] = (char*)malloc((size_t)(nlen + 1));
        H5Gget_objname_by_idx(grp_id, i, child_names[nchildren],
                              (size_t)(nlen + 1));
        nchildren++;
    }

    qsort(child_names, (size_t)nchildren, sizeof(char*), cmp_strings);

    for (int i = 0; i < nchildren; i++) {
        const char* name = child_names[i];

        if (is_chunk_hashes(name)) {
            free(child_names[i]);
            continue;
        }

        H5L_info_t linfo;
        if (H5Lget_info(grp_id, name, &linfo, H5P_DEFAULT) >= 0) {
            if (linfo.type == H5L_TYPE_EXTERNAL) {
                free(child_names[i]);
                continue;
            }
        }

        H5O_info_t oinfo;
#if H5_VERSION_GE(1, 12, 0)
        H5Oget_info_by_name3(grp_id, name, &oinfo, H5O_INFO_BASIC, H5P_DEFAULT);
#else
        H5Oget_info_by_name(grp_id, name, &oinfo, H5P_DEFAULT);
#endif

        char child_hex[65];
        if (oinfo.type == H5O_TYPE_GROUP) {
            hid_t child_id = H5Gopen2(grp_id, name, H5P_DEFAULT);
            fd5_group_hash(child_id, cache, child_hex);
            H5Gclose(child_id);
            sha256_update(&ctx, child_hex, 64);
        } else if (oinfo.type == H5O_TYPE_DATASET) {
            hid_t child_id = H5Dopen2(grp_id, name, H5P_DEFAULT);
            fd5_dataset_hash(child_id, cache, child_hex);
            H5Dclose(child_id);
            sha256_update(&ctx, child_hex, 64);
        }

        free(child_names[i]);
    }
    free(child_names);

    unsigned char final_hash[32];
    sha256_final(&ctx, final_hash);
    fd5_sha256_hex(final_hash, hex_out);
}

/* ------------------------------------------------------------------ */
/* compute_content_hash                                                */
/* ------------------------------------------------------------------ */

/*
 * content_hash = "sha256:" + sha256(root_group_hash).hexdigest()
 */
int fd5_compute_content_hash(hid_t file_id, fd5_hash_entry* cache, char out[71])
{
    hid_t root = H5Gopen2(file_id, "/", H5P_DEFAULT);
    if (root < 0) return FD5_ERR_HDF5;

    char root_hex[65];
    fd5_group_hash(root, cache, root_hex);
    H5Gclose(root);

    unsigned char final_hash[32];
    fd5_sha256(root_hex, 64, final_hash);

    char final_hex[65];
    fd5_sha256_hex(final_hash, final_hex);

    snprintf(out, 71, "sha256:%s", final_hex);
    return FD5_OK;
}

/* ------------------------------------------------------------------ */
/* compute_id                                                          */
/* ------------------------------------------------------------------ */

/*
 * Python: payload = "\0".join(inputs[k] for k in sorted(inputs))
 *         digest = sha256(payload.encode("utf-8")).hexdigest()
 *         return f"sha256:{digest}"
 *
 * Keys are attribute names; values are read from the file.
 */
int fd5_compute_id(hid_t file_id, const char** keys, int count, char out[71])
{
    const char** sorted_keys = (const char**)malloc(sizeof(char*) * (size_t)count);
    memcpy(sorted_keys, keys, sizeof(char*) * (size_t)count);
    qsort(sorted_keys, (size_t)count, sizeof(char*), cmp_strings);

    sha256_ctx ctx;
    sha256_init(&ctx);

    for (int i = 0; i < count; i++) {
        hid_t attr_id = H5Aopen(file_id, sorted_keys[i], H5P_DEFAULT);
        if (attr_id < 0) {
            free(sorted_keys);
            return FD5_ERR_HDF5;
        }

        hid_t type_id = H5Aget_type(attr_id);
        char* val = NULL;
        int is_vlen = 0;

        if (H5Tget_class(type_id) == H5T_STRING) {
            if (H5Tis_variable_str(type_id)) {
                is_vlen = 1;
                hid_t memtype = H5Tcopy(H5T_C_S1);
                H5Tset_size(memtype, H5T_VARIABLE);
                H5Aread(attr_id, memtype, &val);
                H5Tclose(memtype);
            } else {
                size_t sz = H5Tget_size(type_id);
                val = (char*)calloc(sz + 1, 1);
                H5Aread(attr_id, type_id, val);
            }
        } else {
            hid_t native = H5Tget_native_type(type_id, H5T_DIR_DEFAULT);
            if (H5Tget_class(type_id) == H5T_INTEGER) {
                int64_t ival;
                H5Aread(attr_id, H5T_NATIVE_INT64, &ival);
                val = (char*)malloc(32);
                snprintf(val, 32, "%lld", (long long)ival);
            } else {
                double dval;
                H5Aread(attr_id, H5T_NATIVE_DOUBLE, &dval);
                val = (char*)malloc(32);
                snprintf(val, 32, "%g", dval);
            }
            H5Tclose(native);
        }

        H5Tclose(type_id);
        H5Aclose(attr_id);

        if (i > 0) {
            sha256_update(&ctx, "\0", 1);
        }
        if (val) {
            sha256_update(&ctx, val, strlen(val));
            if (is_vlen)
                H5free_memory(val);
            else
                free(val);
        }
    }

    unsigned char hash[32];
    sha256_final(&ctx, hash);

    char hex[65];
    fd5_sha256_hex(hash, hex);

    snprintf(out, 71, "sha256:%s", hex);
    free(sorted_keys);
    return FD5_OK;
}
