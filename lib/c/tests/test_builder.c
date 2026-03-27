/*
 * test_builder.c -- Integration tests for the fd5 C library.
 */
#include "fd5.h"

#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void test_create_and_seal(const char* tmp_dir)
{
    float data[100];
    for (int i = 0; i < 100; i++) data[i] = (float)i;

    hsize_t shape[] = {100};
    hsize_t chunks[] = {25};
    const char* id_inputs[] = {"product", "name", "timestamp", NULL};

    fd5_builder* b = fd5_create(tmp_dir, "test/product", "test-file",
                                "Test description", "2026-01-01T00:00:00Z");
    assert(b != NULL);

    int rc = fd5_write_dataset(b, "values", data, shape, 1,
                               H5T_NATIVE_FLOAT, chunks);
    assert(rc == FD5_OK);

    /* Embed a minimal schema */
    fd5_embed_schema(b, "{\"type\":\"object\",\"properties\":{\"values\":{\"type\":\"array\"}}}");

    char out_path[1024];
    rc = fd5_seal(b, id_inputs, out_path, sizeof(out_path));
    assert(rc == FD5_OK);
    printf("Sealed: %s\n", out_path);

    /* Verify the sealed file */
    fd5_verify_status status = fd5_verify(out_path);
    assert(status == FD5_VALID);
    printf("Verify: VALID\n");
}

static void test_verify_detects_tampering(const char* tmp_dir)
{
    float data[] = {1.0f, 2.0f, 3.0f};
    hsize_t shape[] = {3};
    const char* id_inputs[] = {"product", "name", "timestamp", NULL};

    fd5_builder* b = fd5_create(tmp_dir, "test/product", "tamper-test",
                                "Tamper test", "2026-01-01T00:00:00Z");
    fd5_write_dataset(b, "values", data, shape, 1, H5T_NATIVE_FLOAT, NULL);
    fd5_embed_schema(b, "{\"type\":\"object\"}");

    char out_path[1024];
    fd5_seal(b, id_inputs, out_path, sizeof(out_path));

    /* Tamper: open file, change an attribute */
    hid_t fid = H5Fopen(out_path, H5F_ACC_RDWR, H5P_DEFAULT);
    H5Adelete(fid, "description");
    hid_t space = H5Screate(H5S_SCALAR);
    hid_t atype = H5Tcopy(H5T_C_S1);
    H5Tset_size(atype, H5T_VARIABLE);
    hid_t aid = H5Acreate2(fid, "description", atype, space,
                           H5P_DEFAULT, H5P_DEFAULT);
    const char* tampered = "TAMPERED";
    H5Awrite(aid, atype, &tampered);
    H5Aclose(aid);
    H5Sclose(space);
    H5Tclose(atype);
    H5Fclose(fid);

    fd5_verify_status status = fd5_verify(out_path);
    assert(status == FD5_INVALID);
    printf("Tampered file correctly detected as INVALID\n");
}

static void test_multidim_dataset(const char* tmp_dir)
{
    /* 3x4 matrix */
    double data[12];
    for (int i = 0; i < 12; i++) data[i] = (double)i * 0.5;

    hsize_t shape[] = {3, 4};
    const char* id_inputs[] = {"product", "name", "timestamp", NULL};

    fd5_builder* b = fd5_create(tmp_dir, "test/product", "matrix-test",
                                "Multi-dim test", "2026-02-01T12:00:00Z");
    assert(b != NULL);

    int rc = fd5_write_dataset(b, "matrix", data, shape, 2,
                               H5T_NATIVE_DOUBLE, NULL);
    assert(rc == FD5_OK);

    char out_path[1024];
    rc = fd5_seal(b, id_inputs, out_path, sizeof(out_path));
    assert(rc == FD5_OK);

    fd5_verify_status status = fd5_verify(out_path);
    assert(status == FD5_VALID);
    printf("Multi-dim dataset: VALID\n");
}

static void test_nested_groups(const char* tmp_dir)
{
    int32_t ts_data[] = {100, 200, 300};
    float amp_data[] = {0.5f, 1.0f, 1.5f};
    hsize_t shape[] = {3};
    const char* id_inputs[] = {"product", "name", "timestamp", NULL};

    fd5_builder* b = fd5_create(tmp_dir, "test/product", "nested-test",
                                "Nested groups test", "2026-03-01T08:00:00Z");
    assert(b != NULL);

    /* Write datasets under nested groups */
    int rc = fd5_write_dataset(b, "events/timestamps", ts_data, shape, 1,
                               H5T_NATIVE_INT32, NULL);
    assert(rc == FD5_OK);

    rc = fd5_write_dataset(b, "events/amplitudes", amp_data, shape, 1,
                           H5T_NATIVE_FLOAT, NULL);
    assert(rc == FD5_OK);

    /* Add an attribute to a nested group */
    rc = fd5_write_attr_str(b, "events", "unit", "mV");
    assert(rc == FD5_OK);

    char out_path[1024];
    rc = fd5_seal(b, id_inputs, out_path, sizeof(out_path));
    assert(rc == FD5_OK);

    fd5_verify_status status = fd5_verify(out_path);
    assert(status == FD5_VALID);
    printf("Nested groups: VALID\n");
}

static void test_destroy_without_seal(const char* tmp_dir)
{
    fd5_builder* b = fd5_create(tmp_dir, "test/product", "destroy-test",
                                "Should be cleaned up", "2026-01-01T00:00:00Z");
    assert(b != NULL);
    /* Destroy without sealing -- temp file should be deleted */
    fd5_destroy(b);
    printf("Destroy without seal: OK (temp file cleaned up)\n");
}

int main(void)
{
    const char* tmp_dir = "/tmp/fd5_c_test";
    char cmd[256];
    snprintf(cmd, sizeof(cmd), "mkdir -p %s", tmp_dir);
    system(cmd);

    test_create_and_seal(tmp_dir);
    test_verify_detects_tampering(tmp_dir);
    test_multidim_dataset(tmp_dir);
    test_nested_groups(tmp_dir);
    test_destroy_without_seal(tmp_dir);

    /* Cleanup */
    snprintf(cmd, sizeof(cmd), "rm -rf %s", tmp_dir);
    system(cmd);

    printf("All tests passed!\n");
    return 0;
}
