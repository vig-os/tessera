| input | bytes | minicbor 2.3.1 | cbor4ii 1.2.5 (serde) |
|---|---|---|---|
| 5 with a 1-byte argument | `1805` | accepted → 5 | accepted → 5 |
| 64 with a 2-byte argument | `190040` | accepted → 64 | accepted → 64 |
| indefinite text "a" | `7f6161ff` | refused (unexpected type indefinite string at position 0: expected text (definite length)) | accepted → "a" |
| map keys out of order {b:1,a:2} | `a2616201616102` | accepted → {"a": 2, "b": 1} | accepted → {"a": 2, "b": 1} |
| duplicate key {a:1,a:2} | `a2616101616102` | accepted → {"a": 2} | accepted → {"a": 2} |
| 1.5 as f32 (not preferred) | `fa3fc00000` | accepted → 1.5 | refused (Core(Mismatch { name: "f64", found: 250 })) |
| bignum tag 2 as u64 | `c24101` | refused (unexpected type tag at position 0: expected u64) | refused (Core(Mismatch { name: "u64", found: 194 })) |
| CESU-8 lone surrogate | `63eda080` | refused (invalid utf-8 at position 0) | refused (Core(RequireUtf8 { name: "str" })) |

encode 1.5f64: minicbor `fb3ff8000000000000`, cbor4ii `fb3ff8000000000000`, profile `f93e00`
encode {"bb":1,"c":2} (BTreeMap): minicbor `a262626201616302` (writes the map's iteration order; it does not sort), profile `a261630262626201` (RFC 8949 order puts the shorter key first)
