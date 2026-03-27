/*
 * fd5_verify.c -- Read-only verification of an fd5 file's content_hash.
 */
#include "fd5_internal.h"

#include <stdlib.h>
#include <string.h>

fd5_verify_status fd5_verify(const char* path)
{
    if (!path) return FD5_VERIFY_ERROR;

    hid_t file_id = H5Fopen(path, H5F_ACC_RDONLY, H5P_DEFAULT);
    if (file_id < 0) return FD5_VERIFY_ERROR;

    /* Read stored content_hash attribute */
    if (!H5Aexists(file_id, "content_hash")) {
        H5Fclose(file_id);
        return FD5_NOT_FD5;
    }

    hid_t attr_id = H5Aopen(file_id, "content_hash", H5P_DEFAULT);
    if (attr_id < 0) {
        H5Fclose(file_id);
        return FD5_VERIFY_ERROR;
    }

    char* stored = NULL;
    hid_t memtype = H5Tcopy(H5T_C_S1);
    H5Tset_size(memtype, H5T_VARIABLE);
    H5Aread(attr_id, memtype, &stored);
    H5Tclose(memtype);
    H5Aclose(attr_id);

    if (!stored) {
        H5Fclose(file_id);
        return FD5_VERIFY_ERROR;
    }

    /* Recompute content_hash (no cache -- full re-read) */
    char computed[71];
    int rc = fd5_compute_content_hash(file_id, NULL, computed);
    H5Fclose(file_id);

    if (rc != FD5_OK) {
        H5free_memory(stored);
        return FD5_VERIFY_ERROR;
    }

    fd5_verify_status status = (strcmp(stored, computed) == 0)
                                   ? FD5_VALID
                                   : FD5_INVALID;
    H5free_memory(stored);
    return status;
}
