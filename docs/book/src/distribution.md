# Distribution: OCI & cloud

A sealed `.tsra` distributes two ways, both without unpacking it: as an **OCI artifact** (registry-native
addressing) and via **cloud range-reads** (fetch only the blocks a query needs).

> **Note — the snippets in this chapter are *not* executed in-page.** They need a live OCI registry or an
> S3/MinIO bucket, which a hermetic doc build can't provide. Each is exercised end-to-end by a Nix flake
> check instead (named below), so the guarantee is tested — just not inline. This is the "referenced
> check" evidence form from the [Introduction](./intro.md).

## OCI artifact (push / pull)

A `.tsra` maps to an OCI 1.1 image manifest: `artifactType` `vnd.tessera.product.v1+json`, an empty
config, and the `.tsra` as a single sha256-addressed layer, with `id` / `manifest_hash` annotations.

```console
$ tessera push ./study.tsra localhost:5000/studies/duplet-07:v1
$ tessera pull localhost:5000/studies/duplet-07:v1 ./pulled.tsra   # byte-identical
```

*Referenced check:* `oci-roundtrip` — spins up a `distribution` registry, `oras push`es the `.tsra`,
asserts the stored manifest matches the Tessera OCI constants, and `oras pull`s it back byte-identical.

## Cloud range-reads

Because the `.tsra` is a STORED zip64 with its central directory as an index, a reader opens it over the
network and fetches only the byte ranges it needs — a 64 KiB tail-prefetch strips the directory GETs, and
a cohort query that misses a product's stat range **never fetches that product's data block**.

```console
$ tessera inspect s3://bucket/studies/duplet-07.tsra    # env-resolved AWS_* / TESSERA_S3_ENDPOINT
$ tessera verify https://host/path/duplet-07.tsra
```

*Referenced check:* `minio-range-read` — spins up loopback MinIO and proves, via a GET counter, that
prune-before-fetch skips the non-matching product's data block entirely. Both are behind the
`--features cloud` build flag; the default build and the WASM core are unaffected.
