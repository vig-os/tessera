| product | A verify | A′ seal == A seal | golden | A′ verify | B verify | B round-trip | stored JSON | JCS | CBOR | CBOR, byte digests |
|---|---|---|---|---|---|---|---|---|---|---|
| tessera/corpus/files/blob_opaque.tsra | verifies | true | = golden | verifies | verifies | true | 1567 | 1135 | 984 | 867 |
| tessera/corpus/files/empty_no_blocks.tsra | verifies | true | = golden | verifies | verifies | true | 635 | 460 | 400 | 322 |
| tessera/corpus/files/listmode_events.tsra | verifies | true | = golden | verifies | verifies | true | 3623 | 2443 | 2020 | 1903 |
| tessera/corpus/files/multiblock_study.tsra | verifies | true | = golden | verifies | verifies | true | 5738 | 3895 | 3234 | 3078 |
| tessera/corpus/files/recon_float32_mumap.tsra | verifies | true | = golden | verifies | verifies | true | 5180 | 3550 | 2947 | 2830 |
| tessera/corpus/files/recon_int16.tsra | verifies | true | = golden | verifies | verifies | true | 5245 | 3591 | 2980 | 2863 |
| tessera/corpus/files/segmentation_uint8.tsra | verifies | true | = golden | verifies | verifies | true | 2090 | 1203 | 998 | 803 |
| tessera/corpus/files/spectrum_lifetime.tsra | verifies | true | = golden | verifies | verifies | true | 1725 | 1190 | 1019 | 902 |
| tessera/crates/tessera-cli/tests/cmd/logical_tables.in/scan.tsra | verifies | true | (no golden) | verifies | verifies | true | 5319 | 3439 | 2835 | 2562 |
| tessera/crates/tessera-cli/tests/cmd/persona_signing.in/study.tsra | verifies | true | (no golden) | verifies | verifies | true | 1200 | 748 | 630 | 513 |
| tessera/crates/tessera-cli/tests/cmd/versioning.in/study.tsra | verifies | true | (no golden) | verifies | verifies | true | 1200 | 748 | 630 | 513 |
| tessera/crates/tessera-io/tests/fixtures/legacy_flat_events.tsra | verifies | true | (no golden) | verifies | verifies | true | 4112 | 2681 | 2207 | 2012 |
| tessera/crates/tessera-py/tests/fixtures/partitioned_pairs.tsra | verifies | true | (no golden) | verifies | verifies | true | 3170 | 2020 | 1683 | 1488 |
| **total 13** | | **13/13** | | **13/13** | **13/13** | **13/13** | 40804 | 27103 (66%) | 22567 (55%) | 20656 (51%) |

`id` = digest of the `id_inputs` byte slice found verbatim inside the A′ bytes: 13/13
