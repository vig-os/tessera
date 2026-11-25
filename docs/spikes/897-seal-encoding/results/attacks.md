## Summary over the 13 tracked products (count × outcome)

| id | attack | A (alpha.6) | A′ | B |
|---|---|---|---|---|
| T1 | #573 splice a newer-format top-level key (`actors`), old seal kept | 13× caught: seal mismatch | 13× caught: seal mismatch | 13× caught: seal mismatch |
| T2 | #573 splice a newer-format key into blocks[0], old seal kept | 12× caught: seal mismatch | 12× caught: seal mismatch | 12× caught: seal mismatch |
| T3 | edit a value (name), old seal kept | 13× caught: seal mismatch | 13× caught: seal mismatch | 13× caught: seal mismatch |
| T4 | reorder keys, old seal kept | 13× verifies | 13× refused: non-canonical bytes | 13× refused: CBOR key order |
| T5 | change whitespace (A: compact + key-sorted rewrite), old seal kept | 13× verifies | 13× refused: non-canonical bytes | 13× impossible (no such form) |
| T6 | respell a string with a \u escape, old seal kept | 13× verifies | 13× refused: non-canonical bytes | 13× impossible (no such form) |
| T7 | respell a number (64 → 64.0 / non-shortest CBOR int), old seal kept | 7× verifies; typed read refuses | 7× refused: non-canonical bytes | 7× refused: CBOR non-shortest int |
| T8 | duplicate key prepended, old seal kept | 13× refused: duplicate key | 13× refused: duplicate key | 13× refused: duplicate key |
| T9 | seal stripped | 13× refused: seal missing | 13× refused: seal missing | 13× refused: seal missing |
| T10 | #886 bytes with 2^53+1 sealed, served with 2^53 | 13× refused: I-JSON integer | 13× refused: I-JSON integer | 13× caught: seal mismatch |
| S1 | duplicate key, sealed by the attacker (last-wins) | 13× refused: duplicate key | 13× refused: duplicate key | 13× refused: duplicate key |
| S2 | #734 duplicate block name, sealed | 12× refused: duplicate block name (typed rule) | 12× refused: duplicate block name (typed rule) | 12× refused: duplicate block name (typed rule) |
| S3 | #873 BlockRef in array form, sealed | 12× refused: typed layer (declared form) | 12× refused: typed layer (declared form) | 12× refused: typed layer (declared form) |
| S4 | #873 kind spelled as an object, sealed | 12× refused: typed layer (declared form) | 12× refused: typed layer (declared form) | 12× refused: typed layer (declared form) |
| S5 | #886 epoch-ns 1730000000123456789 as a number, sealed | 13× refused: I-JSON integer | 13× refused: I-JSON integer | 13× verifies |
| S5b | integer beyond 2^53 that a double holds exactly (1730000000123456800) | 13× refused: I-JSON integer | 13× refused: I-JSON integer | 13× verifies |
| S6 | lone surrogate U+D800 in description (A: original seal kept) | 13× refused: lone surrogate (JSON parser) | 13× refused: lone surrogate (JSON parser) | 13× refused: invalid UTF-8 |
| S7 | array shape 64 written as the float 64.0, sealed | 7× verifies; typed read refuses | 7× refused: non-canonical bytes | 7× refused: integral float (numeric reduction) |
| S8 | non-canonical layout (A: the stored pretty file / pretty JSON / unsorted CBOR map) | 13× verifies | 13× refused: non-canonical bytes | 13× refused: CBOR key order |

S7 under profile B0 (no numeric reduction): 7× verifies; typed read refuses

## Verbatim verdicts for tessera/corpus/files/recon_int16.tsra

### T1 — #573 splice a newer-format top-level key (`actors`), old seal kept

- A: caught: seal mismatch
- A′: caught: seal mismatch
- B: caught: seal mismatch

### T2 — #573 splice a newer-format key into blocks[0], old seal kept

- A: caught: seal mismatch
- A′: caught: seal mismatch
- B: caught: seal mismatch

### T3 — edit a value (name), old seal kept

- A: caught: seal mismatch
- A′: caught: seal mismatch
- B: caught: seal mismatch

### T4 — reorder keys, old seal kept

- A: verifies
- A′: refused: non-canonical bytes: differ from their JCS form at byte 2
- B: refused: strict CBOR decoder: byte 23: map key "content_hash" out of canonical order

### T5 — change whitespace (A: compact + key-sorted rewrite), old seal kept

- A: verifies
- A′: refused: non-canonical bytes: differ from their JCS form at byte 1
- B: impossible: the encoding has no such form

### T6 — respell a string with a \u escape, old seal kept

- A: verifies
- A′: refused: non-canonical bytes: differ from their JCS form at byte 3549
- B: impossible: the encoding has no such form

### T7 — respell a number (64 → 64.0 / non-shortest CBOR int), old seal kept

- A: verifies, then typed block read: invalid type: floating point `64.0`, expected u64
- A′: refused: non-canonical bytes: differ from their JCS form at byte 263
- B: refused: strict CBOR decoder: byte 184: argument 64 not in its shortest form (additional information 27)

### T8 — duplicate key prepended, old seal kept

- A: refused: serialize: duplicate key "name" in a JSON object (refused: hashed JSON must be I-JSON) at line 10 column 8
- A′: refused: serialize: duplicate key "name" in a JSON object (refused: hashed JSON must be I-JSON) at line 1 column 658
- B: refused: strict CBOR decoder: byte 94: map key "name" appears twice

### T9 — seal stripped

- A: refused: seal missing: integrity: manifest_hash mismatch — expected (absent), got blake3:4fd0b90d0aa3c2bddbaefec7e244b1…
- A′: refused: seal missing
- B: refused: seal missing

### T10 — #886 bytes with 2^53+1 sealed, served with 2^53

- A: refused: metadata.seed = 9007199254740992 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is an IEEE-75…
- A′: refused: metadata.seed = 9007199254740992 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is an IEEE-75…
- B: caught: seal mismatch

### S1 — duplicate key, sealed by the attacker (last-wins)

- A: refused: serialize: duplicate key "name" in a JSON object (refused: hashed JSON must be I-JSON) at line 10 column 8
- A′: refused: serialize: duplicate key "name" in a JSON object (refused: hashed JSON must be I-JSON) at line 1 column 658
- B: refused: strict CBOR decoder: byte 94: map key "name" appears twice

### S2 — #734 duplicate block name, sealed

- A: refused: block name 'volume' is listed more than once; block names are unique within a product (ADR-0063)
- A′: refused: block name 'volume' is listed more than once; block names are unique within a product (ADR-0063)
- B: refused: block name 'volume' is listed more than once; block names are unique within a product (ADR-0063)

### S3 — #873 BlockRef in array form, sealed

- A: refused: serialize: `blocks[0]` is an array where the format defines an object (one spelling per value: a record is an …
- A′: refused: serialize: `blocks[0]` is an array where the format defines an object (one spelling per value: a record is an …
- B: refused: serialize: `blocks[0]` is an array where the format defines an object (one spelling per value: a record is an …

### S4 — #873 kind spelled as an object, sealed

- A: refused: serialize: `blocks[0].kind` is an object where the format defines a string (one spelling per value: a record i…
- A′: refused: serialize: `blocks[0].kind` is an object where the format defines a string (one spelling per value: a record i…
- B: refused: serialize: `blocks[0].kind` is an object where the format defines a string (one spelling per value: a record i…

### S5 — #886 epoch-ns 1730000000123456789 as a number, sealed

- A: refused: metadata.daq_start_ns = 1730000000123456789 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is…
- A′: refused: metadata.daq_start_ns = 1730000000123456789 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is…
- B: verifies

### S5b — integer beyond 2^53 that a double holds exactly (1730000000123456800)

- A: refused: metadata.daq_start_ns = 1730000000123456800 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is…
- A′: refused: metadata.daq_start_ns = 1730000000123456800 exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2): a sealed JSON number is…
- B: verifies

### S6 — lone surrogate U+D800 in description (A: original seal kept)

- A: refused: serialize: unexpected end of hex escape at line 32 column 25
- A′: refused: serialize: unexpected end of hex escape at line 1 column 397
- B: refused: strict CBOR decoder: byte 2856: text string is not valid UTF-8 (invalid utf-8 sequence of 1 bytes from index 0…

### S7 — array shape 64 written as the float 64.0, sealed

- A: verifies, then typed block read: invalid type: floating point `64.0`, expected u64
- A′: refused: non-canonical bytes: differ from their JCS form at byte 263
- B: refused: strict CBOR decoder: byte 184: float 64 is integral: numeric reduction writes it as an integer

### S8 — non-canonical layout (A: the stored pretty file / pretty JSON / unsorted CBOR map)

- A: verifies
- A′: refused: non-canonical bytes: differ from their JCS form at byte 1
- B: refused: strict CBOR decoder: byte 23: map key "content_hash" out of canonical order
