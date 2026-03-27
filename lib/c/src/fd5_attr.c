/*
 * fd5_attr.c -- Attribute write helpers for the fd5 builder.
 */
#include "fd5_internal.h"

#include <stdlib.h>
#include <string.h>

static hid_t open_target(hid_t file_id, const char* obj_path)
{
    if (!obj_path || obj_path[0] == '\0')
        return file_id;
    return H5Oopen(file_id, obj_path, H5P_DEFAULT);
}

static void close_target(hid_t obj_id, hid_t file_id)
{
    if (obj_id != file_id)
        H5Oclose(obj_id);
}

int fd5_write_attr_str(fd5_builder* b, const char* obj_path,
                       const char* attr_name, const char* value)
{
    if (!b || !attr_name || !value) return FD5_ERR_INVALID;

    hid_t target = open_target(b->file_id, obj_path);
    if (target < 0) return FD5_ERR_HDF5;

    hid_t space = H5Screate(H5S_SCALAR);
    hid_t atype = H5Tcopy(H5T_C_S1);
    H5Tset_size(atype, H5T_VARIABLE);

    hid_t aid = H5Acreate2(target, attr_name, atype, space,
                           H5P_DEFAULT, H5P_DEFAULT);
    if (aid < 0) {
        H5Tclose(atype);
        H5Sclose(space);
        close_target(target, b->file_id);
        return FD5_ERR_HDF5;
    }

    H5Awrite(aid, atype, &value);
    H5Aclose(aid);
    H5Tclose(atype);
    H5Sclose(space);
    close_target(target, b->file_id);
    return FD5_OK;
}

int fd5_write_attr_i64(fd5_builder* b, const char* obj_path,
                       const char* attr_name, int64_t value)
{
    if (!b || !attr_name) return FD5_ERR_INVALID;

    hid_t target = open_target(b->file_id, obj_path);
    if (target < 0) return FD5_ERR_HDF5;

    hid_t space = H5Screate(H5S_SCALAR);
    hid_t aid = H5Acreate2(target, attr_name, H5T_NATIVE_INT64, space,
                           H5P_DEFAULT, H5P_DEFAULT);
    if (aid < 0) {
        H5Sclose(space);
        close_target(target, b->file_id);
        return FD5_ERR_HDF5;
    }

    H5Awrite(aid, H5T_NATIVE_INT64, &value);
    H5Aclose(aid);
    H5Sclose(space);
    close_target(target, b->file_id);
    return FD5_OK;
}

int fd5_write_attr_f64(fd5_builder* b, const char* obj_path,
                       const char* attr_name, double value)
{
    if (!b || !attr_name) return FD5_ERR_INVALID;

    hid_t target = open_target(b->file_id, obj_path);
    if (target < 0) return FD5_ERR_HDF5;

    hid_t space = H5Screate(H5S_SCALAR);
    hid_t aid = H5Acreate2(target, attr_name, H5T_NATIVE_DOUBLE, space,
                           H5P_DEFAULT, H5P_DEFAULT);
    if (aid < 0) {
        H5Sclose(space);
        close_target(target, b->file_id);
        return FD5_ERR_HDF5;
    }

    H5Awrite(aid, H5T_NATIVE_DOUBLE, &value);
    H5Aclose(aid);
    H5Sclose(space);
    close_target(target, b->file_id);
    return FD5_OK;
}
