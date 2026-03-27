/*
 * Minimal SHA-256 implementation (public domain).
 * Based on the algorithm described in FIPS PUB 180-4.
 */
#ifndef SHA256_H
#define SHA256_H

#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint32_t state[8];
    uint64_t bitcount;
    unsigned char buffer[64];
} sha256_ctx;

void sha256_init(sha256_ctx* ctx);
void sha256_update(sha256_ctx* ctx, const void* data, size_t len);
void sha256_final(sha256_ctx* ctx, unsigned char hash[32]);

#endif /* SHA256_H */
