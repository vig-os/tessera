---
type: issue
state: open
created: 2026-09-28T23:55:20Z
updated: 2026-09-29T06:52:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/477
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:27.360Z
---

# [Issue 477]: [ingest: the sealed decoder digest commits to the wrong slice — global across lanes, and blind to a crate's source](https://github.com/vig-os/tessera/issues/477)

## Finding 1 — the crate list is global across lanes

### What happens now

ADR-0056 §6a's sealed `ingest_decoder` triple carries a digest over the **decode-path crate pins**, and
that list (`DECODE_PATH_CRATES` in `tessera-ingest/build.rs`) is **global across every lane**. So adding
a dependency for one lane moves the `manifest_hash` of products sealed by every *other* lane.

Observed concretely while landing the array lane: adding `zip` for `.npz` moved the `manifest_hash` of
every **parquet** and **csv** fixture in the ingest corpus. Nothing about how those files were decoded
changed, and nothing about their bytes changed — `content_hash` was untouched, correctly. Only the
recorded claim about the decoder moved.

## Why it is wrong rather than merely noisy

The digest exists so a reader can tell "this file was interpreted by *that* decoder build". A parquet
product's `ingest_decoder` should describe the parquet decode path. Today it also commits to the presence
and version of a zip library that had no part in reading it, which means:

- **False churn.** Every lane's goldens regenerate when any lane gains a dependency, so the signal that
  should mean "a decoder changed" fires when one demonstrably did not. That is the same class of noise
  ADR-0052 §1 / #336 removed when `CARGO_PKG_VERSION` came out of this digest, and the same reason the
  crate's own resolved feature set came out of it during #386 (it made a Parquet seal depend on whether
  the CSV lane was compiled in).
- **A weaker claim.** A digest that covers unrelated crates is less useful for the thing it is for: two
  products whose digests differ may have been decoded identically, so a reviewer cannot read a difference
  as meaningful without checking which crate moved and whether it was even on the path.

## Proposed shape

A **per-lane** decode-path crate list, so the sealed digest for a product covers only the crates that
actually decoded it:

- `parquet` lane → `parquet`, `arrow-*`, `half`, …
- `csv` lane → `csv`, `csv-core`
- `npy`/`npz` lane → `zip` (+ its deflate backend)
- vendor lanes → their own, once #454 retro-gates them

Then adding `zip` for `.npz` moves the npz lane's digest and nothing else.

Open questions for whoever picks this up:

1. Shared crates (`half`, `arrow-buffer`) belong to several lanes — list them per lane rather than
   once, accepting duplication for clarity?
2. Is a lane's digest also sensitive to crates reachable only via *another* lane through feature
   unification? (The #386 fix already removed the crate's own feature set from the digest for exactly
   this reason, so the precedent says no.)
3. This is a **format event**: it moves every existing `manifest_hash` once, deliberately. Worth pairing
   with any other pending digest change so the corpus regenerates once rather than twice.

`content_hash` is unaffected in every case, which is what keeps this a cleanup rather than a correctness
fix — the bytes were always right; only the provenance claim was too broad.

Milestone: 0.1.0-beta.



---

## Finding 2 — the pins ignore `source`, so a same-version git pin is invisible

Same class, same fix site, found while working out whether pinning vortex to a fork would move the
digest (it does not — vortex is the *encoder* and correctly absent from `DECODE_PATH_CRATES`; that
question is what turned this up).

`lock_version` in `tessera-ingest/build.rs` parses **only** `version = "…"` out of `Cargo.lock` and
ignores the `source` field:

```rust
} else if in_wanted_package {
    if let Some(version) = line.strip_prefix("version = ") {
        return Some(version.trim_matches('"').to_owned());
    }
}
```

so a pin is identified by its version string alone, and the emitted pre-image is
`pins=parquet=58.3.0,arrow-array=58.3.0,…`.

**The hole.** A decode-path crate pinned to a **git rev at the same version** is indistinguishable from
the registry release. Pin `parquet` to a fork at `58.3.0` to fix a decode bug — which is exactly the
manoeuvre this repo is now using for vortex, so it is established practice rather than hypothetical — and
every sealed product keeps claiming the unpatched decoder while actually having been read by the patched
one. The digest exists to answer "which decoder build interpreted these bytes", and this is the one
question it would get wrong, silently, in the case where the answer matters most (a decoder whose
behaviour was deliberately changed).

It is the same shape as the two defects #386 already fixed in this digest — the crate's own resolved
feature set, and `CARGO_PKG_VERSION` — each of which committed to the wrong slice of reality. This one
commits to too *little*.

**Fix, alongside finding 1.** Hash version **and** source per decode-path crate: the registry versus
`git+<url>#<rev>`, so a fork is a different pin. A registry release should keep its current pre-image
shape where possible, so this does not gratuitously move every existing `manifest_hash` for builds that
use no forks — worth checking whether that is achievable, since otherwise the two findings together
become one deliberate corpus event, which is fine but should be planned as such.

Note it does **not** bite today: no decode-path crate is currently pinned to a git rev. It will bite the
first time one is.

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 06:05 AM_

## Design proposal — measured, not yet implemented

Both findings confirmed on the current `dev` (`5a1c465`), plus **a third** that changes what the fix
should be. All numbers below come from this repo's `Cargo.lock` / `cargo metadata`.

### Finding 1, quantified in one number

All **9** ingest-corpus fixtures — 6 parquet, 1 arrow-ipc, 2 csv — carry the **same** `features` digest
`blake3:cc9064975209e…`. Three lanes, one digest. That is the whole of Finding 1: the digest cannot
distinguish the parquet decoder from the CSV one, because it does not describe either.

### Finding 2 confirmed latent

27 crates in the lockfile have a `git+` source (the whole `gerchowl/vortex` fork, shape
`git+<url>?rev=<rev>#<rev>`). **None is on a decode path** — vortex is the encoder, correctly absent —
so nothing is wrong today and everything is wrong the first time a decode-path crate is forked.

### Finding 3 (new) — the list is also materially *incomplete* per lane

This is the one that changes the design. Comparing today's 10-crate global list against each lane's real
dependency closure:

| Lane | On the decode path | In today's digest | **Absent from the digest** |
| --- | --- | --- | --- |
| csv | 3 | 2 of them | `memchr` (the SIMD scanner `csv-core` tokenises with) |
| arrow-ipc | 21 | 7 | **`flatbuffers`** (the IPC wire format), `zstd`/`zstd-safe`/`zstd-sys`, `lz4_flex`, `chrono`, `chrono-tz`, `bytemuck`, `num-*`, `libm` |
| parquet | 38 | 8 | **`thrift`** + `integer-encoding` (the footer/metadata format), **every page codec** — `snap`, `zstd*`, `brotli*`, `flate2`/`miniz_oxide`/`zlib-rs`, `lz4_flex` — plus `crc32fast` (page checksums), `base64`, `byteorder`, `ordered-float`, `twox-hash` (bloom filters), `num-bigint` (decimals) |

So the digest omits the Thrift reader that parses Parquet's metadata, the FlatBuffers reader that parses
Arrow IPC, and *every decompressor*. Swap a snappy implementation and the digest does not move. Pointedly:
**`chrono-tz` is absent**, and `chrono-tz` is the exact crate ADR-0056 §6a names as hazard H1's mechanism.

Today's failure mode is therefore **silent under-claiming** — forget to add a crate and the digest quietly
covers less than it says. That is Finding 2's shape again, and it is the property the fix should invert.

### 1. Per-lane lists: derive the *candidate set*, declare the *membership*

I tried to derive membership outright and **it does not work** — worth recording so nobody retries it:

| Derivation attempt | parquet | arrow-ipc | csv | Why it fails |
| --- | --- | --- | --- | --- |
| Full lockfile closure | 114 | 95 | 11 | Includes `syn`, `quote`, `cc`, `jobserver`, `windows-*`, `wasm-bindgen` |
| `cargo metadata`, normal deps only, proc-macro crates dropped | 92 | 70 | 10 | `syn`/`quote` survive as normal deps *of* proc-macro crates |
| …plus: never descend into a proc-macro subtree | 85 | 63 | 6 | Platform shims still arrive via `chrono → iana-time-zone` |
| …plus: cut the `iana-time-zone` subtree | **77** | **55** | **6** | Still includes `hashbrown`, `bytes`, `slab`, `indexmap`, `futures-*` |

`hashbrown` is a genuine runtime dependency of the Parquet reader that provably cannot change a decoded
value, and **nothing in the dependency graph says so**. Membership is a judgement about *semantics*, and
no amount of metadata yields it.

What *is* derivable is the candidate set, so the proposal splits the two:

- **Derived (mechanical):** per lane, the normal-dependency closure of the crates its Cargo feature
  declares — `parquet = ["arrow", "dep:parquet"]`, `arrow = ["dep:arrow-array", …]`, `csv = ["dep:csv"]`.
  The lanes are already feature-gated with explicit `dep:` entries, so the roots are read from
  `Cargo.toml` rather than restated. Never descend into a proc-macro subtree (compile-time code cannot
  read a runtime byte).
- **Declared (judgement):** which candidates are in the digest. Initial membership = the 38/21/3 above.
- **The gate, which is the actual fix:** every candidate must be either *in the digest* or *in a declared
  exclusion list with a reason*. A new decode-path dependency then fails the build until someone decides,
  instead of silently narrowing the claim. **The failure mode flips from silent under-claiming to a loud
  build failure.** That is worth more than either list.

Exclusions collapse into ~6 reviewable categories (compile-time, platform/tz shims, container plumbing,
write-path-only `itoa`/`ryu`, hashing/RNG, async plumbing) — roughly 39 entries for parquet, reviewed once.

Answering the issue's open questions: **(1)** yes, per lane with duplication — shared crates like `half`
must appear in every lane that uses them, and since the candidate set is derived there is no maintenance
cost to the duplication. **(2)** no — a lane's digest covers only its own closure, per the #386 precedent.
This does **not** close §6a's stated residual (a feature flipped *inside* the shared arrow tree by an
unrelated crate); it captures each crate's version and source, not its resolved features. ADR-0057 Gate A
remains the thing guarding that, and §6a's residual paragraph should keep saying so.

### 2. Hash version **and** source

Per crate, emit `name=version` when the source is the default registry, and
`name=version@git+<url>#<rev>` otherwise. A registry-only build therefore keeps the per-crate shape it has
today, so forks are the only thing that moves — which is what Finding 2 asked for.

### 3. Version the pre-image itself (addition, not in the issue)

Prefix each pre-image `v2;pins=…`. This digest has now been redefined three times (the crate's own
feature set out, `CARGO_PKG_VERSION` out, and now this). Without a version marker, a reader comparing
digests across any of those boundaries sees "the decoder changed" when only the *derivation* did. With
one, that is a declared difference. It also removes the need to preserve the old pre-image shape for
compatibility, since the change stops being silent — worth having before the next redefinition rather
than after.

### Migration impact — measured

**What moves: 9 `manifest_hash` values, in one file.**

| Surface | Moves? | Detail |
| --- | --- | --- |
| `corpus/ingest-corpus.json` | **yes** | 9 fixtures: 9 `manifest_hash` + 9 `ingest_decoder.features` |
| `corpus/corpus.json` | **no** | all 8 conformance entries are non-ingest and carry no decoder triple |
| Every `content_hash` | **no** | structural: `content_hash` is the Merkle over *block digests*; the triple lives in the manifest's generation bag |
| Every `id` | **no** | hashed over `id_inputs` (product/name/timestamp) |
| Vendor lanes (dicom, dicom-series, hdf-compound, nifti, raw, blob, blob-series) | **no** | only 3 lanes record a triple at all (`engine.rs:696/711/741`) |
| `crates/tessera-ingest/tests/{generic_table,ingest_corpus}.rs`, `ingest_spec.trycmd` | **no** | they pin zero literal hashes; they compare against the corpus file |
| `info.trycmd` | shape only | it wildcards both the pre-image and the digest (`"[..]"`), so it needs updating only because one JSON key becomes three |
| ADR-0056 §6a, `FEATURE-MATRIX.md` | prose | no hashes |

**The migration is self-verifying.** Since `content_hash` is structurally independent of the manifest's
generation bag, regenerating the corpus must move 9 `manifest_hash`es and **zero** `content_hash`es. If
any `content_hash` moves, the change is wrong and the gate says so — the check comes free.

### The owner decisions

**(a) Is moving 9 `manifest_hash`es acceptable now, or does it need a format-version bump?**

No format field is added, removed or renamed, and `features` remains an opaque digest whose pre-image
composition is explicitly not part of the format contract (it is printed by `tessera info`, not declared
in a schema). By the letter of it, no bump is required. Options:

1. **Take it now, pre-1.0** (milestone is 0.1.0-beta, and #477 already frames it as a deliberate format
   event). Cheapest, and the `v2;` prefix makes the boundary legible.
2. **Bump the format version too**, treating any change to a sealed digest's derivation as a format event.
   Most conservative, and the most honest signal to anything already comparing digests — but it spends a
   version number on something no schema change accompanies.
3. **Defer and batch** with any other pending digest change, so the corpus regenerates once. #477's own
   open question 3 asks for this; my reading is that findings 1, 2 and 3 *are* the batch, and I know of no
   other pending change to this digest — but you would know better.

My recommendation: **(1) with the `v2;` prefix**, and land findings 1+2+3 as a single corpus event.

**(b) How wide should the digest be?** The 38-crate parquet membership is complete but moves on any
decode-path bump, where today's 8 rarely move and under-claim. Completeness is the digest's entire purpose
— a Parquet reader that silently swapped its snappy implementation is precisely the case it exists to
record — and the cost is 9 lines in one JSON file, regenerated by a command. I recommend the wide set, but
it is a churn-versus-claim trade and it is yours.

No code written; nothing pushed.


