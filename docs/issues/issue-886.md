---
type: issue
state: open
created: 2026-11-16T07:18:42Z
updated: 2026-11-16T07:38:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/886
comments: 1
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:33.129Z
---

# [Issue 886]: [seal: integers beyond 2^53 collide under JCS canonicalisation (sealed values not exact)](https://github.com/vig-os/tessera/issues/886)

Found by the #883 (#873) adversarial review.

JCS (RFC 8785) serialises numbers via IEEE-754 double, so two distinct integers above 2^53 canonicalise identically. A field sealed as `9007199254740993` verifies when served as `9007199254740992` (also beyond u64). Python `jcs==0.2.1` agrees, so readers don't disagree, but the seal does not pin the exact value. This hits every opaque numeric value already in the format (manifest metadata, newer fields), not only #883's paths.

Decide (design question): refuse integers outside ±(2^53−1) at parse time in `canonical::parse_strict` (SPEC: large integers must be strings), or document the limit. Refusal is the integrity-preserving option.

Refs: #573
---

# [Comment #1]() by [gerchowl]()

_Posted on November 16, 2026 at 07:38 AM_

Architect review: are integers above 2^53 needed in sealed JSON?

**Not today.** Every typed numeric field the format seals is bounded far below 2^53 (about 9.0e15):

| Field | Realistic maximum |
|---|---|
| `ArraySpec.shape` / `chunks` / `shards` | array extents |
| `BlobSpec.size` | 2^53 bytes = 8 PiB |
| `TableSpec.rows` | row count |
| partition `index` | partition count |

`ChunkStats` (i64 min/max, i128 sums) is computed in the CLI and is not sealed JSON. No tracked corpus manifest has an integer of 16 or more digits.

The real exposure is free-form metadata from ingest: epoch-nanosecond timestamps (about 1.7e18), 64-bit device serials and vendor tick counters. Bulk 64-bit data already lives in exact table and array payloads, never in JSON.

"Support larger integers" isn't available inside JCS. RFC 8785 fixes the number form to IEEE-754 doubles (I-JSON, RFC 7493 §2.2), and any other canonical number form would fork the format away from the reference readers.

**Recommendation: refuse at parse time and carry large integers as strings.**
1. `canonical::parse_strict` refuses any integral number token whose magnitude is above 2^53−1 (typed error, in every sealed document: manifest, embedded schema, collection, signature envelope). A lossy value then fails loudly instead of sealing approximately.
2. Ingest writes any vendor metadata integer outside that range as a decimal string, the way DICOM already carries UIDs as strings. Audit the DICOM, GE-HDF5 and `.ini` paths for 64-bit values and add a test with an epoch-ns timestamp.
3. ProductSchema can declare such a field as an integer encoded as a string, so validation still checks the range and type.
4. The reference reader refuses the same tokens, so readers agree. SPEC §4 states the rule.

Needs the owner's confirmation before dispatch.

