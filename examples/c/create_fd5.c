/**
 * Create a sealed fd5 file in C.
 *
 * Build:
 *     cd examples/c && mkdir -p build && cd build
 *     cmake .. && make
 *
 * Run:
 *     ./build/create_fd5
 */

#include <fd5.h>
#include <stdio.h>
#include <math.h>

int main(void) {
    /* Create a builder — opens temp HDF5 file, writes root attrs */
    fd5_builder* b = fd5_create(
        "/tmp/fd5-examples",
        "test/product",
        "hello-fd5-c",
        "First fd5 file created with the C builder",
        "2026-01-01T00:00:00Z"
    );
    if (!b) {
        fprintf(stderr, "fd5_create failed\n");
        return 1;
    }

    /* Write a 3D volume dataset with inline SHA-256 hashing */
    float volume[8 * 16 * 16];
    for (int i = 0; i < 8 * 16 * 16; i++)
        volume[i] = sinf((float)i * 0.01f);

    hsize_t shape[]  = {8, 16, 16};
    hsize_t chunks[] = {4, 8, 8};
    int rc = fd5_write_dataset(b, "volume", volume, shape, 3,
                               H5T_NATIVE_FLOAT, chunks);
    if (rc != FD5_OK) {
        fprintf(stderr, "fd5_write_dataset failed: %d\n", rc);
        fd5_destroy(b);
        return 1;
    }

    /* Write metadata attributes on root */
    fd5_write_attr_str(b, NULL, "scanner", "Example PET/CT");
    fd5_write_attr_str(b, NULL, "institution", "fd5 Lab");

    /* Embed a minimal JSON Schema */
    fd5_embed_schema(b, "{\"type\":\"object\",\"properties\":"
                        "{\"volume\":{\"type\":\"array\"}}}");

    /* Seal: validate → id → content_hash → atomic rename */
    const char* id_inputs[] = {"product", "name", "timestamp", NULL};
    char out_path[1024];
    rc = fd5_seal(b, id_inputs, out_path, sizeof(out_path));
    if (rc != FD5_OK) {
        fprintf(stderr, "fd5_seal failed: %d\n", rc);
        return 1;
    }
    printf("Created  : %s\n", out_path);

    /* Verify the sealed file */
    fd5_verify_status status = fd5_verify(out_path);
    printf("Verified : %s\n", status == FD5_VALID ? "PASS" : "FAIL");

    return status == FD5_VALID ? 0 : 1;
}
