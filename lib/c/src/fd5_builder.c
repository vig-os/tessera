/*
 * fd5_builder.c -- Builder: create, write_dataset, seal, destroy.
 */
#include "fd5_internal.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Copy src to dst (up to dst_sz), replacing every occurrence of 'from' with 'to'. */
static void slugify(char* dst, size_t dst_sz, const char* src, char from, char to)
{
    snprintf(dst, dst_sz, "%s", src);
    for (char* p = dst; *p; p++) {
        if (*p == from) *p = to;
    }
}

/* ------------------------------------------------------------------ */
/* fd5_create                                                          */
/* ------------------------------------------------------------------ */

fd5_builder* fd5_create(const char* out_dir, const char* product,
                        const char* name, const char* description,
                        const char* timestamp)
{
    if (!out_dir || !product || !name || !description || !timestamp)
        return NULL;

    fd5_builder* b = (fd5_builder*)calloc(1, sizeof(fd5_builder));
    if (!b) return NULL;

    snprintf(b->out_dir, sizeof(b->out_dir), "%s", out_dir);
    snprintf(b->product, sizeof(b->product), "%s", product);
    snprintf(b->name, sizeof(b->name), "%s", name);
    snprintf(b->description, sizeof(b->description), "%s", description);
    snprintf(b->timestamp, sizeof(b->timestamp), "%s", timestamp);

    char slug[256];
    slugify(slug, sizeof(slug), product, '/', '_');
    snprintf(b->tmp_path, sizeof(b->tmp_path), "%s/.fd5_%s.h5.tmp",
             out_dir, slug);

    b->file_id = H5Fcreate(b->tmp_path, H5F_ACC_TRUNC, H5P_DEFAULT,
                           H5P_DEFAULT);
    if (b->file_id < 0) {
        free(b);
        return NULL;
    }

    fd5_write_attr_str(b, NULL, "product", product);
    fd5_write_attr_str(b, NULL, "name", name);
    fd5_write_attr_str(b, NULL, "description", description);
    fd5_write_attr_str(b, NULL, "timestamp", timestamp);
    fd5_write_attr_i64(b, NULL, "_schema_version", 1);

    return b;
}

/* ------------------------------------------------------------------ */
/* fd5_create_group                                                    */
/* ------------------------------------------------------------------ */

int fd5_create_group(fd5_builder* b, const char* path)
{
    if (!b || !path) return FD5_ERR_INVALID;

    hid_t lcpl = H5Pcreate(H5P_LINK_CREATE);
    H5Pset_create_intermediate_group(lcpl, 1);

    hid_t gid = H5Gcreate2(b->file_id, path, lcpl, H5P_DEFAULT, H5P_DEFAULT);
    H5Pclose(lcpl);
    if (gid < 0) return FD5_ERR_HDF5;
    H5Gclose(gid);
    return FD5_OK;
}

/* ------------------------------------------------------------------ */
/* fd5_write_dataset                                                   */
/* ------------------------------------------------------------------ */

int fd5_write_dataset(fd5_builder* b, const char* path, const void* data,
                      const hsize_t* shape, int ndims, hid_t dtype,
                      const hsize_t* chunk_shape)
{
    if (!b || !path || !data || !shape || ndims < 1)
        return FD5_ERR_INVALID;

    const char* last_slash = strrchr(path, '/');
    if (last_slash) {
        char parent[512];
        size_t plen = (size_t)(last_slash - path);
        memcpy(parent, path, plen);
        parent[plen] = '\0';

        htri_t exists = H5Lexists(b->file_id, parent, H5P_DEFAULT);
        if (exists <= 0) {
            int rc = fd5_create_group(b, parent);
            if (rc != FD5_OK) return rc;
        }
    }

    hid_t space = H5Screate_simple(ndims, shape, NULL);
    if (space < 0) return FD5_ERR_HDF5;

    hid_t dcpl = H5P_DEFAULT;
    if (chunk_shape) {
        dcpl = H5Pcreate(H5P_DATASET_CREATE);
        H5Pset_chunk(dcpl, ndims, chunk_shape);
    }

    hid_t ds = H5Dcreate2(b->file_id, path, dtype, space, H5P_DEFAULT,
                          dcpl, H5P_DEFAULT);
    if (ds < 0) {
        H5Sclose(space);
        if (dcpl != H5P_DEFAULT) H5Pclose(dcpl);
        return FD5_ERR_HDF5;
    }

    herr_t err = H5Dwrite(ds, dtype, H5S_ALL, H5S_ALL, H5P_DEFAULT, data);
    if (err < 0) {
        H5Dclose(ds);
        H5Sclose(space);
        if (dcpl != H5P_DEFAULT) H5Pclose(dcpl);
        return FD5_ERR_HDF5;
    }

    hsize_t total_elems = 1;
    for (int i = 0; i < ndims; i++) total_elems *= shape[i];
    size_t elem_sz = H5Tget_size(dtype);
    size_t total_bytes = (size_t)total_elems * elem_sz;

    unsigned char hash[32];
    fd5_sha256(data, total_bytes, hash);

    fd5_hash_entry* entry = (fd5_hash_entry*)calloc(1, sizeof(fd5_hash_entry));
    H5Iget_name(ds, entry->path, sizeof(entry->path));
    fd5_sha256_hex(hash, entry->hex);
    entry->next = b->hash_cache;
    b->hash_cache = entry;

    H5Dclose(ds);
    H5Sclose(space);
    if (dcpl != H5P_DEFAULT) H5Pclose(dcpl);
    return FD5_OK;
}

/* ------------------------------------------------------------------ */
/* fd5_embed_schema                                                    */
/* ------------------------------------------------------------------ */

int fd5_embed_schema(fd5_builder* b, const char* json_schema)
{
    if (!b || !json_schema) return FD5_ERR_INVALID;
    free(b->schema_json);
    b->schema_json = strdup(json_schema);
    return b->schema_json ? FD5_OK : FD5_ERR_IO;
}

/* ------------------------------------------------------------------ */
/* fd5_seal                                                            */
/* ------------------------------------------------------------------ */

int fd5_seal(fd5_builder* b, const char** id_inputs,
             char* out_path, size_t out_len)
{
    if (!b || !id_inputs || !out_path) return FD5_ERR_INVALID;
    if (b->sealed) return FD5_ERR_INVALID;

    if (b->name[0] == '\0' || b->description[0] == '\0' ||
        b->timestamp[0] == '\0')
        return FD5_ERR_INVALID;

    if (b->schema_json) {
        fd5_write_attr_str(b, NULL, "_schema", b->schema_json);
    }

    int nkeys = 0;
    while (id_inputs[nkeys]) nkeys++;

    char id_str[71];
    int rc = fd5_compute_id(b->file_id, id_inputs, nkeys, id_str);
    if (rc != FD5_OK) return rc;

    fd5_write_attr_str(b, NULL, "id", id_str);

    /* id_inputs description: "key1 + key2 + ..." */
    char id_desc[512] = "";
    for (int i = 0; i < nkeys; i++) {
        if (i > 0) strncat(id_desc, " + ", sizeof(id_desc) - strlen(id_desc) - 1);
        strncat(id_desc, id_inputs[i], sizeof(id_desc) - strlen(id_desc) - 1);
    }
    fd5_write_attr_str(b, NULL, "id_inputs", id_desc);

    char content_hash[71];
    rc = fd5_compute_content_hash(b->file_id, b->hash_cache, content_hash);
    if (rc != FD5_OK) return rc;

    fd5_write_attr_str(b, NULL, "content_hash", content_hash);

    H5Fclose(b->file_id);
    b->file_id = -1;
    b->sealed = 1;

    char product_slug[256];
    slugify(product_slug, sizeof(product_slug), b->product, '/', '-');

    const char* id_hex = id_str + 7; /* skip "sha256:" prefix */
    char id8[9];
    snprintf(id8, sizeof(id8), "%.8s", id_hex);

    /* YYYY-MM-DDTHH:MM:SS -> YYYY-MM-DD_HH-MM-SS */
    char ts_part[32] = "";
    if (strlen(b->timestamp) >= 19) {
        /* Copy YYYY-MM-DD */
        memcpy(ts_part, b->timestamp, 10);
        ts_part[10] = '_';
        /* Copy HH-MM-SS from HH:MM:SS */
        ts_part[11] = b->timestamp[11];
        ts_part[12] = b->timestamp[12];
        ts_part[13] = '-';
        ts_part[14] = b->timestamp[14];
        ts_part[15] = b->timestamp[15];
        ts_part[16] = '-';
        ts_part[17] = b->timestamp[17];
        ts_part[18] = b->timestamp[18];
        ts_part[19] = '\0';
    }

    char filename[512];
    if (ts_part[0]) {
        snprintf(filename, sizeof(filename), "%s_%s-%s.h5",
                 ts_part, product_slug, id8);
    } else {
        snprintf(filename, sizeof(filename), "%s-%s.h5",
                 product_slug, id8);
    }

    char final_path[1024];
    snprintf(final_path, sizeof(final_path), "%s/%s", b->out_dir, filename);

    if (rename(b->tmp_path, final_path) != 0)
        return FD5_ERR_IO;

    snprintf(out_path, out_len, "%s", final_path);
    return FD5_OK;
}

/* ------------------------------------------------------------------ */
/* fd5_destroy                                                         */
/* ------------------------------------------------------------------ */

void fd5_destroy(fd5_builder* b)
{
    if (!b) return;

    if (!b->sealed && b->file_id >= 0) {
        H5Fclose(b->file_id);
        remove(b->tmp_path);
    }

    fd5_hash_entry* e = b->hash_cache;
    while (e) {
        fd5_hash_entry* next = e->next;
        free(e);
        e = next;
    }

    free(b->schema_json);
    free(b);
}
