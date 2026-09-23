# ADR-0057 — Ingest topology: the thin waist, the feature graph, and the release matrix

**Status:** Proposed (2026-08-20, spike #398). The **packaging/topology** axis of the decision whose
**semantics** are ADR-0056 (generic ingest — the normalise-vs-preserve ladder). Where ADR-0056 said
*what* ingest may and may not do to values, this ADR says *where the code lives, what compiles into
which binary, and what we ship*. Extends **ADR-0035** (declarative spec engine / closed-backend
dispatch) and **ADR-0052** (versioning + release pipeline). Bounded by **ADR-0034** (wasm boundary),
**ADR-0042** (sealed vs `aux/`), **ADR-0020** (identity/determinism). Phase-0 gate for
[#386](https://github.com/vig-os/tessera/issues/386).

> Design-only ADR from a five-lens fresh-context spike (Rust crate-graph/cargo maintainer ·
> cargo-dist/release engineer · determinism & reproducibility · FAIR/clinical operator · API
> ergonomics). **Two claims were probed empirically rather than asserted** — §4 (cargo-dist can emit
> two feature-differentiated binaries) and §5 (conformance goldens are decoder-independent). Both
> probes ran in the nix devShell against the real workspace; §5's probe found a live hazard that was
> not visible from reading the code. No ingest code was written; §10 is the landing plan.

## Context — what ADR-0056 left open, and why it cannot stay open

ADR-0056 settled the value semantics of generic ingest and, in §12, sketched a feature layout. But it
left the load-bearing structural question unanswered: **the crate split dictates where #386's code
goes**, and re-homing a decoder after it ships is the expensive kind of refactor — it moves module
paths, feature names, and (if done carelessly) sealed bytes.

ADR-0056 §12 also asserted a constraint without proving it:

> *"A feature may decide whether a format is **readable**; it may never change how one **encodes**.
> Building without `ingest-parquet` must produce a clean 'unsupported source format' error — never
> different bytes for the same input."*

That sentence is the whole determinism story of a feature-gated ingest layer. This ADR tests it, and
finds it **true today but unenforced, with a live path to becoming false** (§5).

## §1 — The thin waist, stated precisely

The premise on the table was: `tessera-core` owns the determinism-critical hot path
`Arrow (tables) / flat-buffer (arrays) → canonicalise → seal`, with decoders upstream and pluggable.

**The premise is accepted in substance and corrected in placement.** The waist is real; it is not in
`tessera-core`. The corrected statement:

- **`tessera-core` owns the *primitive spec*** — the dtype vocabulary, the canonical column/buffer
  layout, block descriptors, the manifest, and the seal contract. It stays dependency-diet.
- **`tessera-io` owns the *encode + seal*** — `ColumnData`/`ArrayData` → Vortex/Zarr bytes →
  `content_hash`. This is where the sealed bytes are actually produced today.
- **`tessera-ingest` owns the *canonicalisation*** — the ADR-0056 §2 `arrow → primitive` type map and
  the §5 H1–H9 rules — behind an `arrow` feature, plus the decoders that feed it.

The waist is therefore the pair **(`ColumnData` + `ArrayData`)**, and it already exists in
`tessera-io` (`crates/tessera-io/src/table.rs:66`). ADR-0057's job is not to invent it but to make it
*reachable from outside* and to stop new decoders from routing around it (§8).

**The claimed array/table asymmetry is confirmed and is load-bearing.** The array side is a flat,
C-order, single-dtype element buffer, so its canonicalisation surface is four rules (endianness,
C-vs-F order, `fill_value`/NaN, dtype map). The table side is the whole ADR-0056 §2 table. The
consequence for topology: **`ingest array` needs no `arrow` dependency at all**. Only the table lane
does. This is why the `arrow` feature (§3) is a *lane* gate, not a global one, and why the long tail
(FITS, ROOT TTree, netCDF, `.mat`, EDF/BDF) reaches the array lane far more cheaply than the table
lane.

## §2 (Q1) — One `tessera-ingest` with feature-gated modules. Do not split into per-format crates now

**Decision: `tessera-ingest` stays one crate.** Decoders become feature-gated *modules*, not
separate crates. `tessera-dicom` / `tessera-nifti` / `tessera-hdf5` are **not** created by this ADR.

The concrete cost of splitting now, which the proposal under-counted:

| Cost | Split now (N crates) | Feature-gated modules (1 crate) |
| --- | --- | --- |
| crates.io coordinates owned forever | N names to publish, doc-link, yank, deprecate | 1 |
| release-plz | N lockstep versions that only *look* independent | 1 workspace version (already true) |
| CI matrix | N × feature combinations for clippy + doctests | 1 × feature combinations |
| Compile wall-clock | no gain — the DAG serialises at the leaf link | same |
| Consumer `Cargo.toml` | 5 names pretending to be independent | `tessera-ingest = { features = [...] }` |

Splitting is cheap when justified and **not retractable** once published. Nothing justifies it today:
no decoder has an independent release cadence, an operator outside the CLI, or a consumer who wants
one without the others.

**Named extraction triggers** — extract a decoder into its own crate when *any* holds. This is the
migration note, stated up front so the decision is falsifiable rather than permanent:

1. the module exceeds ~2k SLOC of decoder-specific logic;
2. it acquires a consumer outside `tessera-cli` (a second binary, an external crate);
3. it needs a release cadence independent of the workspace version;
4. an embedder needs to `cargo deny`-audit it in isolation and the feature gate is not enough
   (i.e. the dependency is present in `Cargo.lock` even when the feature is off, and that is a
   compliance problem for them).

**One exception is pre-authorised: `tessera-arrow`.** The `arrow → primitive` type map is a genuinely
separable concept with an obvious third-party audience ("I have a `RecordBatch`, I want a `.tsra`").
It is *not* extracted now, because §3's feature gating already delivers the ergonomic property that
would justify extraction. Trigger 2 above is the one to watch.

## §3 (Q3) — `arrow` stays out of `tessera-core`. Canonicalisation lives in `tessera-ingest`

**Decision: `tessera-core` takes no `arrow` dependency, now or later.**

`tessera-core` is the **only** dependency of `tessera-wasm`, and (with `tessera-io`) of the pure-abi3
Python wheel. The arrow tree brings ~14 crates including `chrono`, `getrandom`, and — via
`arrow-ipc`'s default features — `zstd-sys`. Two of those are exactly the wasm blockers ADR-0034
identified and the `wasm-core` flake check exists to keep out. An `arrow` dep in core would reopen a
closed question and inflate the browser verifier for a code path the verifier never executes: **a
reader verifying a seal never canonicalises anything. Only a writer does.**

So the boundary is drawn as:

```text
tessera-core     primitive spec: dtypes, column/buffer layout, manifest, seal contract.  no arrow.
     ↑
tessera-io       ColumnData / ArrayData  →  Vortex / Zarr bytes  →  content_hash.   [THE WAIST]
     ↑
tessera-ingest   [feature "arrow"]  arrow RecordBatch → ColumnData   (ADR-0056 §2 type map)
                 [feature "parquet"] [feature "npy"] [feature "dicom"] [feature "hdf5"] …
```

**The determinism gate's tests live with the canonicalisation, in `tessera-ingest`** — not in core.
This is safe because the gate is a *corpus*, not a crate boundary (§5): what protects the property is
the golden-hash comparison, and that comparison can be run from wherever the code sits.

**Rejected: a middle `tessera-arrow` crate (now).** The ergonomics reviewer's objection was concrete
and good — a third party who wants only `RecordBatch → ColumnData` should not have to take
`hdf5-metno-sys`, which needs a *native library present at build time*. That objection is real, and
§2's feature gating **dissolves it**: with the decoders optional,
`tessera-ingest = { default-features = false, features = ["arrow"] }` is an arrow-only dependency
with no native libs. A separate crate buys nothing that a feature does not already buy, and costs a
7th workspace member. Revisit under §2 trigger 2.

**Rejected: canonicalisation in `tessera-io`.** `tessera-io` already carries the arrow tree
transitively via Vortex, so this is free on dependency grounds. It loses on *responsibility*
grounds: `tessera-io` is the crate whose output is sealed, and mixing "decode foreign types" into
"produce the canonical bytes" is precisely the confusion the waist exists to prevent. Keeping
foreign-type handling strictly upstream of the waist is what makes the §5 gate meaningful.

## §4 (Q2) — cargo-dist CAN ship two flavors. We are not going to. One binary, everything on

This is the question the spike was most likely to get wrong by assertion, so it was probed against
real `cargo-dist 0.32.0` (the exact repo pin) in a scale-model workspace.

### What the probe proved

**The mechanism works.** Two packages, each with per-package dist metadata:

```toml
# crates/tessera-cli/Cargo.toml
[package.metadata.dist]
dist = true
features = []

# crates/tessera-cli-full/Cargo.toml   (bin name "tessera-full")
[package.metadata.dist]
dist = true
features = ["dicom", "hdf5", "nifti"]
bin-aliases = { "tessera-full" = ["tessera"] }
```

`dist build` emits, verbatim from the probe log:

```text
building x86_64-unknown-linux-gnu target, using cargo profile dist --package=tcli)
building x86_64-unknown-linux-gnu target, using cargo profile dist --package=tcli-full)
```

— **one `cargo build --package=X` invocation per dist app**. Unpacking and *running* the two shipped
binaries confirmed the split is genuine: lean printed `["parquet", "npy"]`, full printed
`["parquet", "npy", "dicom"]`. Both landed under one announcement tag (`v0.1.0-alpha.1`), which
release-plz's single workspace version already guarantees. `bin-aliases` is accepted by the config
parser and the generated full installer symlinks `tessera` → `tessera-full` at install time.

Four mechanical constraints the probe also established, recorded so a future attempt does not
rediscover them:

1. **Two *packages* are mandatory.** A cargo package has exactly one dist feature set; two `[[bin]]`
   targets in one package share it.
2. **The two packages must not share a cargo bin name.** With both emitting a bin literally named
   `tessera`, `cargo build --workspace` warns `output filename collision`
   ([rust-lang/cargo#6313](https://github.com/rust-lang/cargo/issues/6313), *"may become a hard error
   in the future"*) and `target/debug/tessera` is whichever target won the race. Distinct cargo bin
   names plus `bin-aliases` is the only clean form.
3. **`cargo build --workspace` feature-unifies and poisons the lean binary.** Proven directly: the
   same `tessera` binary reported `["parquet","npy"]` from `cargo build -p tcli` and
   `["parquet","npy","dicom"]` from `cargo build --workspace`. This is resolver-2 behaviour, not a
   bug, and it is the sharp edge — see the consequence below.
4. Two apps produce two installers (`…-installer.sh` each), two receipts, and 2× archives+checksums
   per target.

### Why we decline it anyway

Constraint 3 is the one that decides this. The repo's own gates are single-invocation workspace
builds: `flake.nix:249` runs clippy `--all-targets --all-features` and `flake.nix:252` runs
`cargoNextest` over the workspace. The `trycmd` docs-as-tests resolve the binary via
`CARGO_BIN_EXE_tessera`. **Under a two-package split, every one of those exercises a binary whose
feature set matches neither shipped flavor** — and the test that matters most (does the lean build
produce a good missing-backend error?) would silently pass against a build that has the backend.

That is fixable with build discipline, but it buys a thing we do not want. Two independent reviewers
— the release engineer and the FAIR/clinical operator — reached the same verdict from opposite ends,
and ADR-0056 §12 already said it in its own words:

> *"Defaults on for everything P1 ships. A downloaded `tessera` that cannot read Parquet is a bad
> binary; the gates exist for embedders and CI, not for end users. Release channels enable the full
> set."*

The three stated reasons to gate decoders (ADR-0056 §12) are each served *without* forking the
artifact set:

- **CI cost** — the ~90 min x86_64 flake check is dominated by `static-hdf5` building HDF5 from
  source inside the `--all-features` clippy gate. The fix is to drop `static-hdf5` from the *PR*
  feature matrix and keep it release-only. Forking the shipped binaries saves nothing here; the
  expensive leg still runs in the full flavor.
- **Supply-chain audit surface** — served by Cargo features for source consumers and embedders. A
  prebuilt binary is not audited with `cargo deny`.
- **The determinism story** — served by the feature gate itself plus §5's corpus, regardless of how
  many binaries ship.

Meanwhile the cost is concentrated on the user least able to pay it. The clinical operator on a
locked-down hospital research VM — no compiler, no crates.io egress, no admin rights — who downloads
`tessera`, points it at a DICOM study, and is told *"rebuild with `--features dicom`"* has hit a wall
they cannot climb. Rust has no stable ABI, so there is no runtime install path; the flavor is a
permanent property of the download.

**Decision:**

- **The cargo-dist broad channel ships ONE app**, `tessera-cli`, built with the full decoder set
  (`features = ["full", "static-hdf5"]`). One `curl | sh`, one binary, one README install line.
- **The lean/full axis is a source and nix axis**, expressed as Cargo features and as nix packages
  (`nix run .#tessera` full, `.#tessera-lean` gated). nix builds each derivation separately, so it
  has *no* unification hazard and is the natural home for flavor fan-out.
- **The two-flavor cargo-dist config above is recorded as a proven, ready escape hatch.** If a real
  demand appears (container-size-sensitive deployment, an embedder needing a signed lean binary), it
  is a config change with known mechanics — not a redesign. Constraints 1–4 are the spec for doing
  it.

**Note this contradicts the proposal in #398**, which asked for a lean default plus a `tessera-full`
flavor. The mechanism question was answered *yes*; the product question is answered *no*. Domain
flavors (`clinical = dicom+nifti`) are likewise not created — the proposal already called them
premature and nothing found here argues otherwise.

### `static-hdf5` becomes an implementation detail of `hdf5`

Today `static-hdf5` forwards `tessera-cli → tessera-ingest → hdf5-metno/static` and only switches how
libhdf5 *links*. With HDF5 behind an optional `hdf5` feature, `static-hdf5` without `hdf5` is
meaningless. Make it impossible by construction rather than a runtime error:

```toml
static-hdf5 = ["hdf5", "hdf5-metno/static", "hdf5-metno/zlib"]
```

It remains a **build-mode** flag, deliberately excluded from the `full` capability feature (§5), and
set only by the release channels.

## §5 (Q4) — The feature graph, and the determinism gate that must enforce it

### The feature graph

On `tessera-ingest`:

```toml
[features]
default  = ["arrow", "parquet", "npy", "nifti", "raw", "blob"]
full     = ["default", "dicom", "hdf5"]

arrow    = ["dep:arrow"]              # the ADR-0056 §2 table-lane type map. already in-tree via Vortex
parquet  = ["arrow", "dep:parquet"]   # + thrift, snap, brotli
npy      = []                         # in-tree header parse + memcpy; no ndarray dep
raw      = []                         # headerless array; in-tree
blob     = []                         # ADR-0038 opaque tier; in-tree, never gated off in practice
nifti    = []                         # in-tree parser
dicom    = ["dep:dicom", "dep:dicom-transfer-syntax-registry"]
hdf5     = ["dep:hdf5-metno", "dep:hdf5-metno-sys"]
static-hdf5 = ["hdf5", "hdf5-metno/static", "hdf5-metno/zlib"]   # build mode, not a capability
csv      = ["arrow", "dep:csv"]       # ADR-0056 §8, P2
tiff     = ["dep:tiff"]               # #394
```

`tessera-cli` forwards each by the same name, plus its existing `cloud` and `sql`. Rules:

- **Every optional feature forwards to `tessera-ingest/<name>` and never to `tessera-core`.** Core
  gains no features from this ADR.
- **No mutually-exclusive features.** Cargo features are additive; a pair that cannot coexist is a
  latent unification bug. Enforced by construction, not by documentation.
- **`full` is the capability set; `static-hdf5` is not in it.** The release channels set both.
- **`default` is generous.** A `cargo install tessera-cli` with no flags reads Parquet, Arrow, npy,
  NIfTI, raw and blob. Only the two heavy native-dep decoders are opt-in, and the shipped binary
  turns them on (§4).

### The hazard the probe actually found

`cargo test --all-features` staying green is table stakes and is already gated
(`flake.nix:249`). The interesting question is whether it stays *deterministic*, and here the probe
found something not visible from reading the code.

**Finding 1 — today's goldens are decoder-independent.** `gen_corpus` was run in four configurations
in the nix devShell and the output byte-compared:

| # | Configuration | Goldens |
| --- | --- | --- |
| 1 | `-p tessera-io` (default; no decoders in the graph at all) | `07022df465e252c867…` |
| 2 | `-p tessera-io --features cloud` (object_store + tokio + reqwest in the *seal* crate) | identical |
| 3 | `-p tessera-cli` probe binary — dicom + hdf5 + nifti + raw all linked | identical |
| 4 | as 3, `--features sql` — DataFusion 54 and a second `arrow` entry point | identical |

All four are byte-identical to each other **and** to the committed `tessera/corpus/corpus.json`. The
ADR-0056 §12 constraint holds today.

**Finding 2 — it holds by luck, and the luck is already partly spent.** A structural diff of resolved
features (`cargo tree -p tessera-cli -e features`, default vs `--features sql`) shows the optional
`sql` feature turning on features **inside the shared arrow tree that the ingest path will use**:

```text
arrow-array  feature "chrono-tz"
arrow-schema feature "canonical_extension_types"
arrow-ipc    feature "zstd" / "lz4"
arrow-cast   feature "prettyprint"
```

`arrow-array/chrono-tz` is the one that matters. ADR-0056 §5 lists as hazard **H1**, rated
*high × fatal*:

> *"Timezone/timestamp normalisation — **tzdb is a filesystem dependency** (`chrono-tz` compiled-in vs
> host `/usr/share/zoneinfo`); two hosts, two `i64`s."*

So an **optional, unrelated feature (`sql`) already changes the timezone machinery compiled into the
arrow crate that ADR-0056's table lane will canonicalise through.** It is harmless *only* because
nothing ingests Arrow yet. The moment #386 lands, `tessera` and `tessera --features sql` are two
builds whose tz-handling capability differs — which is hazard H1 arriving through a feature rather
than through a host, the one door ADR-0056 §5 did not think to close.

The same class of hazard exists on the Vortex side and is arguably worse: the workspace deliberately
pins `vortex-btrblocks = { features = ["pco"] }` to **exclude ALP**, because ALP was not
byte-deterministic (#380/#384). Feature unification is monotonic — it can only *add*. Any future
optional decoder that transitively enables `vortex-btrblocks/alp` re-registers the ALP scheme in the
compressor dictionary and silently re-encodes **every float column in every table**, ingested or not.

### The gate — two checks, both required

Finding 1 is a passing probe. A passing probe is not an invariant. ADR-0057 mandates two CI gates:

**Gate A — behavioural. Catches a golden that moved.**

```bash
# from tessera/ ; run in the flake check
for cfg in "-p tessera-io" \
           "-p tessera-io --features cloud" \
           "-p tessera-cli --features full" \
           "-p tessera-cli --all-features"; do
  cargo run -q $cfg --example gen_corpus > "$OUT/corpus.$(echo "$cfg" | tr ' /-' '_').json"
done
# every config must agree with every other AND with the committed golden
for f in "$OUT"/corpus.*.json; do cmp "$f" corpus/corpus.json || exit 1; done
```

Failure condition: any pair diverges, or any diverges from `corpus/corpus.json`.

**Gate B — structural. Catches the hazard *before* it moves a golden.** Gate A tells you the day the
hashes changed; Gate B tells you the day the *risk* was introduced, on the PR that introduced it.

```bash
# snapshot the resolved features of every crate on the seal path
for c in blake3 zarrs pcodec zstd vortex-btrblocks vortex-file vortex-array vortex-buffer \
         arrow-array arrow-buffer arrow-schema; do
  cargo tree -e features -i "$c" --workspace --all-features | sort > tests/feature-snapshots/"$c".txt
done
git diff --exit-code tests/feature-snapshots/
```

Failure condition: any snapshot changed. A changed snapshot is not necessarily wrong — it is a
**deliberate corpus event**, and the PR must either show no golden moved (Gate A still green, snapshot
updated with a stated reason) or carry the corpus regeneration alongside. This is the same discipline
ADR-0056 §5 already prescribes for `arrow`/`parquet` `=` pins, applied one level deeper.

Gate B is what would have flagged the `chrono-tz` drift above on the PR that added the `sql` feature,
rather than on the release that shipped an ingest path through it.

### Where ingest fixtures live, and the anti-vacuity guard

Today's corpus lives in `tessera-io`, which has no decoders, so it is feature-invariant by
construction. ADR-0056 §5 adds `ingest_*` fixtures whose source files are generated at build time by
a pinned writer — and those *do* need a decoder, so the corpus becomes feature-conditional.

**A corpus that skips fixtures when a feature is off is a corpus that passes vacuously.** This repo
has already been bitten by exactly this: the trycmd docs-as-tests ran **zero cases** in CI for a
period, because the `.trycmd` files were not in the crane source filter, and a zero-case pass is
indistinguishable from a green run.

The guard, therefore, is a **declared expected count per configuration**:

- the existing seal corpus stays in `tessera-io`, unconditional;
- ingest fixtures live in `tessera-ingest::corpus`, feature-gated;
- `corpus/manifest.json` lists every fixture with its required-features predicate **and a numeric
  expected count for each named configuration** (`default`, `full`, `all-features`);
- the test asserts `actual_count == expected_count[config]` before comparing any hash. A `full` build
  that expects 12 fixtures and runs 3 fails loudly instead of passing quietly.

## §6 (Q7) — The spec engine: a closed *and total* enum, with the handler feature-gated

`FormatOptions` (`crates/tessera-ingest/src/spec.rs:137`) is a `#[serde(tag = "format")]` enum
dispatched in `engine::run`. The question is how a feature-gated backend registers without the engine
taking a hard dependency on it.

**Decision: keep the enum closed and *total* — every variant always parses, on every build — and
feature-gate only the *handler* arm in `engine::run`. An unavailable backend is a clean runtime
error, never a parse error.**

The load-bearing reason is archival, and it is specific to this repo. ADR-0035 hashes the parsed spec
(JCS-canonical) and writes that `spec_hash` into each member's `ingested_via_spec` provenance edge. A
TOML spec is therefore **an archival artifact whose meaning must not depend on which binary reads
it**. If variants were `#[cfg]`-gated, a spec containing `format = "dicom"` would parse on one build
and fail on another, and a lean build could not even *compute* the `spec_hash` of a spec it cannot
run — breaking provenance portability, not just convenience.

Rejected alternatives, with the error text each would actually produce:

- **`#[cfg(feature)]` on enum variants.** →
  `TOML parse error at line 3: unknown variant 'dicom', expected one of 'nifti', 'raw', 'blob'`.
  Indistinguishable from a typo, points at the file instead of the build, and destroys spec
  portability and `spec_hash` stability. Rejected.
- **`inventory`-style link-time registry.** Tempting because `dicom` already enables
  `inventory-registry`, so the dependency is in-tree. Rejected: link-time collection is fragile under
  aggressive LTO and static linking — and `profile.dist` sets `lto = "thin"` while `static-hdf5`
  links statically. The failure mode is a **green build with silently missing backends at runtime**,
  which is the worst available failure mode for this property.
- **A `dyn Backend` trait-object registry.** → `no backend registered for format='dicom'`. Loses on
  serde: `FormatOptions` derives `Deserialize`, and a `dyn` registry means hand-rolling that, plus
  inheriting init-order and test-isolation problems, in exchange for extensibility no one has asked
  for (§7A).

The winning shape:

```rust
// spec.rs — total, build-invariant. Every variant parses on every build.
pub enum FormatOptions { Dicom{..}, DicomSeries{..}, HdfCompound{..}, Nifti{..},
                         Raw{..}, Blob{..}, Parquet{..}, Npy{..}, ArrowIpc{..} }

// engine.rs — the handler is what's gated.
match opts {
    FormatOptions::Dicom(o) => {
        #[cfg(feature = "dicom")] { crate::dicom::run(o) }
        #[cfg(not(feature = "dicom"))] { Err(Error::BackendNotCompiled("dicom")) }
    }
    // …
}
```

Two constants fall out of this and drive §7: `BACKENDS_ALL` (every variant name, always) and
`BACKENDS_ENABLED` (feature-gated pushes). One grep locates every backend, satisfying ADR-0056 §4's
"`--from <name>` and TOML `from = "<name>"` are one string set".

## §7 (Q6) — Missing-backend UX, and `tessera info`

Even with one shipped flavor (§4), lean builds exist and will produce these errors: source installs
with `--no-default-features`, the nix lean package, embedders, and CI configurations. The error must
be diagnosable by someone who did not build the binary.

**Two distinct errors, two distinct exit codes**, so cookbook recipes and scripts can branch:

```text
$ tessera ingest array study.dcm --from dicom        # exit 3 — known backend, not compiled in
error: backend 'dicom' is not compiled into this build.

  compiled-in backends: arrow, parquet, npy, nifti, raw, blob
  known but disabled:   dicom, hdf5

  This is a lean build. The released binary carries every backend:
    curl -LsSf https://github.com/vig-os/tessera/releases/latest/download/\
tessera-cli-installer.sh | sh
  or build from source with it enabled:
    cargo install tessera-cli --features dicom
  inspect any build:
    tessera info
```

```text
$ tessera ingest table data.pq --from parqet         # exit 2 — genuinely unknown name
error: unknown backend 'parqet'; did you mean 'parquet'?
  known backends: arrow, parquet, npy, csv, nifti, dicom, dicom-series, hdf-compound, raw, blob, tiff
```

**`tessera info` is mandated by this ADR** and must land in the same phase as the first feature gate.
There is no such surface today — `--version` prints a bare version — and without it every
"why did this fail?" costs a support round-trip. It lists the version, the resolved backend set with
pinned decoder versions, and the build-mode flags, with a `--json` form suitable for embedding in
`aux/provenance.json`:

```text
$ tessera info
tessera 0.1.0-alpha.1  (format tessera_version v0)
backends:   arrow 58.3.0 · parquet 58.3.0 · npy · nifti · raw · blob · dicom 0.9.1 · hdf5 1.14.6
disabled:   csv · tiff
build:      static-hdf5=yes · cloud=yes · sql=no
```

**The binary flavor is NOT sealed.** ADR-0056 §6.2 already seals `ingest_decoder` (decoder name +
version), which is the thing that actually determined the bytes. The *flavor* is a compile-time
bundle label that does not affect the bytes, and sealing it would invite the false inference
"same flavor ⇒ same bytes". Flavor and the full backend list go in `aux/provenance.json` — the
ADR-0042 line holds: **sealed = what changed the bytes; `aux/` = who was in the room.**

**The guarantee that makes this safe, stated so it can be cited:**

> For any input `F` and any tessera version `V`, `tessera ingest F` produces the same `content_hash`
> under every distributed build of `V`. Feature selection may change which formats are **readable**;
> it must never change the **bytes** produced for a readable one.

§5's Gate A is its enforcement, and §5's Gate B is its early warning. Without both, `content_hash`
becomes a function of `(version, flavor, feature-unified dependency graph)` — three inputs, two of
them unnamed — and "the hash changed" becomes an event no data steward can explain.

## §8 (Q5) — Vendor refactor: land the seam now, migrate the bodies as a follow-up

The existing `dicom.rs` / `ge_hdf5.rs` / `nifti.rs` / `raw.rs` each reach the seal path their own way.
If #386 adds generic ingest beside them, we get two parallel subsystems and the next determinism
question has to be answered twice.

**Decision: ADR-0057 mandates the *seam* now; the body migration is a follow-up.** The seam is a
single output type that every backend must return, and a rule that backends may not seal:

```rust
/// Every ingest backend returns this. Backends MUST NOT call the seal path directly.
pub enum IngestOutput {
    Table { columns: Vec<(Column, ColumnData)>, spec: TableSpec },
    Array { spec: ArraySpec, data: ArrayData },
    Blob  { bytes: Vec<u8>, media_type: Option<String> },
}

pub trait Backend {
    fn decode(&self, opts: &FormatOptions) -> Result<(IngestOutput, Provenance)>;
}
```

`tessera-io` keeps the single `→ canonicalise → seal` path behind it. Landing the seam now — even
with the current inline code still in place behind it — is what makes the later migration
**mechanical**. Defer it, and the second time anyone touches `dicom.rs` or `ge_hdf5.rs` they will be
re-deriving `content_hash` under scrutiny and re-freezing the conformance corpus.

**The acceptance test for the seam is dogfood, and it is deliberately strict:** `ge_hdf5.rs` and
`nifti.rs` must be rewritten on top of the public waist functions, with their existing golden hashes
unchanged. If Tessera's own decoders cannot sit on the waist, no third party can either — the
dogfood *is* the audit (§9A).

## §9 — What the thin waist is, honestly

**A. It is an internal boundary today, not a public extension point.** Nothing external consumes it,
and the ADR should not pretend otherwise. Making the pitch — *"hand me Arrow, get a sealed `.tsra`"* —
true for a caller who is not `tessera-ingest` requires exactly three public functions in
`tessera-io`:

```rust
pub fn seal_table(spec: &TableSpec, cols: &TableData, prov: ProvenanceOptions) -> Result<Manifest>;
pub fn seal_array(spec: &ArraySpec, data: &ArrayData, prov: ProvenanceOptions) -> Result<Manifest>;
pub fn seal_blob (bytes: &[u8], media_type: Option<&str>, prov: ProvenanceOptions) -> Result<Manifest>;
```

and it requires `ColumnData`, `Column`/`TableSpec`, `ArrayData`/`ArraySpec` and `WriteConfig` to be
treated as **public, unstable-until-0.1.0** input types. They are all `pub` today; none is SemVer
committed. Stating that openly is the honest position pre-1.0 — the alternative, shipping the waist
as an internal boundary while *calling* it a public extension point, is the ergonomics failure this
ADR most wants to avoid.

**B. The long tail is the argument, and it is a good one.** FITS, ROOT TTree, netCDF, MATLAB `.mat`,
EDF/BDF, ORC, Avro will never all be bundled. Per §1 the *array* lane reaches most of them cheaply —
a flat buffer plus shape and dtype — which is why `seal_array` is the highest-leverage of the three
functions above and why bundling more decoders is the wrong response to the tail.

## Alternatives considered and rejected

- **Split into `tessera-dicom` / `tessera-nifti` / `tessera-hdf5` now.** N crates.io coordinates, N
  lockstep versions pretending to be independent, an N × features CI matrix — all irreversible once
  published, for zero present benefit. §2 replaces it with named extraction triggers.
- **`arrow` as a `tessera-core` dependency.** Poisons the wasm graph and the pure-abi3 wheel for a
  code path the verifier never runs. §3.
- **A `tessera-arrow` crate now.** Its only real benefit — an arrow-only dependency without
  `hdf5-metno-sys` — is already delivered by `default-features = false, features = ["arrow"]`. §3.
- **Two prebuilt flavors on the broad channel.** Mechanically proven to work (§4) and declined:
  it saves no CI (the expensive `static-hdf5` leg still runs), splits the `curl | sh` story in two,
  and lands its cost on the locked-down clinical VM that cannot rebuild. §4.
- **Both flavors sharing the cargo bin name `tessera`.** `output filename collision`
  (rust-lang/cargo#6313) and an ambiguous `CARGO_BIN_EXE_tessera` for the trycmd suite. §4.
- **`#[cfg(feature)]` on `FormatOptions` variants.** Makes a TOML spec's meaning build-dependent and
  breaks `spec_hash` portability. §6.
- **`inventory` link-time backend registry.** Fragile under `lto = "thin"` + static linking; fails
  green with silently missing backends. §6.
- **Sealing the binary flavor in the manifest.** Adds a coordinate that did not change the bytes and
  invites "same flavor ⇒ same bytes". `aux/provenance.json` per ADR-0042. §7.
- **Trusting ADR-0056 §12's "a feature may never change how one encodes" as self-enforcing.** §5's
  probe found `sql` already flipping `arrow-array/chrono-tz` — hazard H1's exact mechanism, one
  feature away. Gates A and B replace the assertion.

## Consequences

- `tessera-ingest` grows a feature graph and stays one crate. No new workspace members.
- `tessera-core` is now *explicitly* forbidden from depending on `arrow`; the `wasm-core` check keeps
  it honest.
- The release page keeps exactly one `curl | sh` line and one binary — no user-facing change from
  today, which is the point.
- `static-hdf5` stops being independently settable and becomes an implementation detail of `hdf5`.
- Two new CI gates land. Gate A is cheap (four `gen_corpus` runs). Gate B is nearly free
  (`cargo tree`, no compilation) and is the higher-value of the two.
- A `tests/feature-snapshots/` directory becomes a reviewed artifact: changing it is a deliberate act
  that a reviewer must reason about, like changing the corpus.
- `tessera info` is new surface the CLI must carry forever. It is worth it: it is the precondition for
  every diagnosable ingest failure.
- The `IngestOutput` seam lands before #386's code, so generic ingest is built *on* it rather than
  beside it — and the vendor decoders acquire a migration target instead of a rewrite.
- The PR clippy matrix drops `static-hdf5`, which should take a large bite out of the ~90 min x86_64
  flake check. Release builds still exercise it.

## Landing plan (phased migration)

- **Phase 0 — this ADR, plus the two things that must exist before any gate is meaningful.**
  (a) `tessera info` + `BACKENDS_ALL`/`BACKENDS_ENABLED`; (b) Gate B (`tests/feature-snapshots/`) —
  land it *now*, against today's graph, so the `chrono-tz` drift is captured as the baseline rather
  than discovered later.
- **Phase 1 — the feature graph (§5) + `static-hdf5` folded into `hdf5`.** Mechanical; no behaviour
  change with `full` on. Gate A lands here, since it needs the configs to exist. Drop `static-hdf5`
  from the PR clippy matrix.
- **Phase 2 — the `IngestOutput` seam (§8) + the §6 total-enum/gated-handler dispatch + the §7 error
  surfaces.** Bodies stay where they are; only the boundary moves. Existing goldens must not move —
  that is the test.
- **Phase 3 — #386 P1 builds on the waist** (ADR-0056 §10 P1). The `arrow`/`parquet`/`npy` features
  and the ingest corpus with its expected-count guard land here.
- **Phase 4 (follow-up) — vendor bodies migrate onto the seam**, `ge_hdf5.rs` and `nifti.rs` first,
  as the §8 dogfood test. Golden-hash-preserving by construction.

## Open gaps this ADR does not close

- **`ingest_decoder` as a semver string vs. a decoder *profile id*.** The determinism lens argued for
  sealing a stable profile id (`arrow-primitive-v1`) naming the H1–H9 contract, so a patch bump of
  arrow-rs moves nothing — instead of the arrow-rs version, which ADR-0056 §6.2 deliberately chose.
  Both readings have force and the field is ADR-0056's, not this ADR's. Filed as a follow-up against
  ADR-0056 rather than silently reversed here.
- **`cargo deny` and features-off dependency presence.** A feature-gated dep still appears in
  `Cargo.lock`. Whether that satisfies the embedder-audit motivation (§2 trigger 4) is untested
  against a real `cargo deny` policy.
- **Gate B's crate list is hand-maintained.** Adding a codec crate to the seal path without adding it
  to `tests/feature-snapshots/` leaves a blind spot. Deriving the list from the dependency graph is
  better and is not designed here.
- **Cross-*version* determinism** remains unproven for ingest, as it does for the format generally
  (inherited from ADR-0056).
- **The public waist functions (§9A) are unstable pre-1.0** and no external consumer has exercised
  them. The first genuine third-party integration will find ergonomic bugs the dogfood test cannot.
