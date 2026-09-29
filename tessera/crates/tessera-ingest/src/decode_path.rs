// The **decode-path classification** behind the ADR-0056 §6a per-lane decoder digest (#477).
//
// Three consumers share this one implementation — `build.rs` (`include!`s it, so it must stay
// dependency-free and reference nothing from the crate), the library, and the gate test. A second
// implementation would be a second chance for the digest and the gate to disagree about what the decode
// path *is*, which is the whole defect this fixes.
//
// # What the digest answers, and the four ways it has got it wrong
//
// `ingest_decoder.features` answers "which decoder build interpreted these bytes". It has committed to the
// wrong slice of reality four times:
//
// 1. **One global crate list for every lane**, so adding `zip` for `.npz` moved the `manifest_hash` of
//    every parquet and csv product. A parquet seal committed to a zip library that had no part in reading
//    it.
// 2. **Only `version`, never `source`**, so a decode-path crate pinned to a git fork at the same version
//    was indistinguishable from the registry release — the one question the digest exists to answer, wrong
//    in exactly the case where someone deliberately changed a decoder.
// 3. **Most of the decode path missing.** The hand list named 10 crates; parquet's real closure holds 117
//    resolved packages, of which 43 names can change a decoded value. Absent were `thrift` and
//    `integer-encoding` (Parquet's footer and metadata format), `flatbuffers` (Arrow IPC's wire format),
//    and **every page codec** — `snap`, `zstd`, `brotli`, `flate2`, `lz4_flex`. `chrono-tz` was absent too,
//    and §6a names `chrono-tz` as hazard H1's mechanism.
// 4. **Lookup by NAME alone** — found in review of the fix for 1–3, and the same defect once more. A
//    lockfile legitimately holds several versions of one crate, and they are different code: `base64`
//    resolves at both 0.21.7 and 0.22.1 here, `parquet` depends on **0.22.1**, and a name-keyed scan
//    sealed `base64=0.21.7`. A real parquet `base64` bump would not have moved the digest. Following only
//    the first same-name package also dropped every crate reachable solely through a second version
//    (`foldhash`, `r-efi`, `rand_core`, `wasip2`, `wit-bindgen`, via the second `hashbrown`/`getrandom`).
//
// A package's identity is therefore `(name, version)` throughout, and a crate resolving at two versions on
// one decode path contributes **two** pins.
//
// # Derive the candidates, declare the membership, gate every candidate
//
// Membership **cannot** be derived, and it is worth recording that this was measured rather than
// assumed. Four successive derivations over this workspace: the full lockfile closure gives 114 crates
// for parquet (including `syn`, `cc`, `windows-*`); `cargo metadata` restricted to normal dependencies
// with proc-macro crates dropped gives 92 (`syn` survives as a normal dependency *of* a proc-macro
// crate); never descending into a proc-macro subtree gives 85; additionally cutting the
// `iana-time-zone` subtree gives 77 — still containing `hashbrown`, `bytes` and `slab`. `hashbrown` is a
// genuine runtime dependency of the Parquet reader that provably cannot change a decoded value, and
// **nothing in the dependency graph says so**. The question is semantic, so a human answers it.
//
// So the *candidate set* is derived — the lockfile closure of the crates each lane's Cargo feature
// declares — and every candidate must be classified: either [`IN_DIGEST`] or [`EXCLUDED`] with a reason.
// The gate test fails on an unclassified candidate, which makes the failure mode **loud**: a new
// decode-path dependency stops the build until someone decides, where before a forgotten one silently
// narrowed the claim. That inversion matters more than either list.
//
// Exclusions are **enumerated, never pattern-matched**. A `*-macro`/`*-derive` name rule read tidily and
// was wrong in kind: it would silently exclude a future crate nobody had looked at, which is the failure
// mode this gate exists to end. `seq-macro` is the cautionary case — it generates Parquet's bit-unpacking
// code, so the name pattern would have waved through something that does bear on decoding (it is excluded
// here on the narrower ground that it runs at compile time, which is a decision someone made).
//
// The candidate set is taken from `Cargo.lock` rather than `cargo metadata` on purpose: the lockfile is
// already read here, needs no subprocess, and works in the hermetic flake check and in a `build.rs`
// alike. It cannot distinguish dependency kinds, so the candidate net is wider than strictly necessary —
// which only makes the gate stricter, and every extra candidate is classified once and forgotten.

/// The pre-image's **derivation version**. Bumped whenever the composition changes, so a reader comparing
/// digests across a change sees a declared difference rather than inferring "the decoder changed".
///
/// `v1` (implicit, unprefixed) was the single global `pins=<crate>=<version>,…`. `v2` is per-lane, carries
/// each crate's source as well as its version, covers the whole decode path, and identifies a package by
/// `(name, version)` so two versions of one crate are two pins. This digest has been redefined four times,
/// and every redefinition before `v2` was silent — a version marker costs three bytes and ends that.
pub const PREIMAGE_VERSION: &str = "v2";

/// The registry whose packages are pinned by bare `name=version`, because it is the default everyone means.
const DEFAULT_REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// Cargo features that are **not** ingest lanes, even though they appear in `[features]`.
///
/// `default` is a meta-feature that forwards to the real lanes, so treating it as one would recreate the
/// global list this change removes.
const NOT_A_LANE: &[&str] = &["default"];

/// Crates whose pinned version **and source** go into a lane's digest: everything on that lane's path
/// that can change a decoded value.
///
/// Erring toward inclusion is deliberate. A crate wrongly present costs churn, which is visible the first
/// time it moves; a crate wrongly absent is a silently false claim of sameness, which is what #477 found
/// four times over.
pub const IN_DIGEST: &[&str] = &[
    // ── The readers themselves ──
    "parquet",
    "arrow-array",
    "arrow-buffer",
    "arrow-data",
    "arrow-schema",
    "arrow-ipc",
    "arrow-select",
    "csv",
    // The RFC-4180 state machine `csv` wraps.
    "csv-core",
    // ── Wire formats these readers parse before any value appears ──
    // Parquet's footer, schema and page headers are Thrift; `integer-encoding` is its varint codec.
    "thrift",
    "integer-encoding",
    // Arrow IPC's record-batch and schema messages.
    "flatbuffers",
    // ── Page / buffer compression: a decoded value passes through whichever of these the file used ──
    // NOTE: `zstd-sys` and `flate2`'s backend link a C or Rust library whose identity is pinned here only
    // by the CRATE version, not by the library actually linked (see ADR-0056 §6a).
    "snap",
    "lz4_flex",
    "zstd",
    "zstd-safe",
    "zstd-sys",
    "brotli",
    "brotli-decompressor",
    "alloc-no-stdlib",
    "alloc-stdlib",
    "flate2",
    "miniz_oxide",
    "zlib-rs",
    "adler2",
    "simd-adler32",
    // ── The `.npz` archive reader (#386) ──
    //
    // `zip` decides where each member starts and ends, so it decides what bytes the `.npy` parser is
    // handed — a change in its central-directory or local-header handling can change a decoded array
    // without any value codec being involved. Its deflate stack (`flate2`/`miniz_oxide`/`zlib-rs`) is
    // already above, reached by Parquet's page codecs.
    "zip",
    // ── Integrity of the compressed stream: a wrong verdict here changes what is read ──
    "crc32fast",
    // Parquet's bloom-filter hash — wrong hashing can skip a page that should have been read.
    "twox-hash",
    // ── Numeric and textual conversion of the values themselves ──
    "half",
    "libm",
    "num-bigint",
    "num-complex",
    "num-integer",
    "num-traits",
    "bytemuck",
    "byteorder",
    "ordered-float",
    "base64",
    // `csv-core` tokenises with memchr's SIMD scanners; a field boundary is a decoded value's extent.
    "memchr",
    // ── Timestamps ──
    // Our reader takes raw ticks and never consults a tzdb (ADR-0056 H1), so in principle these cannot
    // move a value. Included anyway, because §6a names `chrono-tz` as H1's own mechanism and a digest that
    // omits the crate its hazard analysis is about is not worth much. `phf` holds chrono-tz's tables.
    "chrono",
    "chrono-tz",
    "phf",
    "phf_shared",
];

/// Candidates deliberately **outside** every digest, each with the reason it cannot change a decoded
/// value.
///
/// Enumerated, never pattern-matched: a name rule would silently exclude a future crate nobody had
/// examined, which is precisely the failure this gate exists to prevent. The gate fails both on a candidate
/// missing from here and on an entry here that is on no lane's path, so the list can neither under- nor
/// over-run the graph.
pub const EXCLUDED: &[(&str, &str)] = &[
    ("autocfg", "build-time probe"),
    ("cc", "build-time C compiler driver"),
    ("find-msvc-tools", "build-time toolchain lookup"),
    ("jobserver", "build-time job coordination"),
    ("paste", "compile-time token pasting"),
    ("pkg-config", "build-time library lookup"),
    ("proc-macro2", "compile-time token model"),
    ("quote", "compile-time code generation"),
    ("rustc_version", "build-time toolchain probe"),
    ("rustversion", "compile-time cfg selection"),
    ("semver", "build-time version comparison"),
    ("shlex", "build-time argument splitting"),
    ("syn", "compile-time parsing"),
    ("unicode-ident", "compile-time identifier validation"),
    ("version_check", "build-time toolchain probe"),
    (
        "arbitrary",
        "zip's optional fuzzing-input trait; a fuzz harness constructs inputs, it does not read ours",
    ),
    (
        "derive_arbitrary",
        "compile-time derive for the above",
    ),
    (
        "crossbeam-utils",
        "concurrency primitives reached through zip; scheduling cannot change which bytes a member \
         decodes to, only when",
    ),
    (
        "displaydoc",
        "compile-time derive of error DISPLAY strings; error text is not a decoded value",
    ),
    (
        "thiserror",
        "error types and their formatting; a decode either fails or yields bytes, and the wording of \
         the failure is not part of the bytes",
    ),
    ("thiserror-impl", "compile-time derive for the above"),
    (
        "seq-macro",
        "generates parquet's bit-unpacking code AT COMPILE TIME; the behaviour of what it \
         generates is parquet's own version, which IS pinned",
    ),
    (
        "bytemuck_derive",
        "derives casts at compile time; `bytemuck` itself is in the digest",
    ),
    ("const-random-macro", "compile-time seed generation"),
    ("futures-macro", "compile-time async codegen"),
    ("serde_derive", "compile-time (de)serialise codegen"),
    ("zerocopy-derive", "compile-time cast derivation"),
    ("wasm-bindgen-macro", "compile-time binding codegen"),
    ("wasm-bindgen-macro-support", "compile-time binding codegen"),
    ("wasm-bindgen-shared", "compile-time binding ABI constants"),
    ("windows-implement", "compile-time COM codegen"),
    ("windows-interface", "compile-time COM codegen"),
    ("android_system_properties", "host property lookup"),
    ("core-foundation-sys", "host API bindings"),
    (
        "iana-time-zone",
        "reads the HOST's local zone; never a value in the file",
    ),
    ("iana-time-zone-haiku", "host zone lookup"),
    ("js-sys", "host JS bindings"),
    ("libc", "host syscall bindings"),
    ("r-efi", "host UEFI bindings"),
    ("redox_syscall", "host syscall bindings"),
    ("wasi", "host WASI bindings"),
    ("wasip2", "host WASI bindings"),
    ("wit-bindgen", "host WASI component bindings"),
    ("wasm-bindgen", "host JS bindings"),
    ("windows-core", "host API bindings"),
    ("windows-link", "host library loading"),
    ("windows-result", "host error codes"),
    ("windows-strings", "host string conversion"),
    ("allocator-api2", "allocation traits"),
    ("bitflags", "typed flag sets"),
    ("bumpalo", "arena allocator"),
    ("bytes", "reference-counted byte buffer"),
    ("cfg-if", "conditional compilation"),
    ("equivalent", "key-equivalence traits"),
    ("hashbrown", "hash container"),
    (
        "indexmap",
        "insertion-ordered map. Since #386 it is also zip's name->entry index, which decides which \
         entry `by_name` returns when an archive holds two members of the same name — so the reason it \
         stays out needs to be stated rather than assumed. For a well-formed `.npz` (one entry per \
         name) it is a lookup index and no ordering question arises. For a duplicate-name archive the \
         choice is made by ZIP'S insert policy over IndexMap's documented contract, and `zip` is in \
         the digest, so a change in that policy moves the pin; indexmap altering its own overwrite or \
         ordering contract would be a semver break, not a silent bump. Our dependence on the collapse \
         is additionally pinned by `a_duplicated_npz_member_name_collapses_in_the_zip_reader`, so it \
         is caught by the suite and not only by a digest. Residual risk, stated rather than denied: a \
         within-contract behaviour change in a compatible indexmap bump could alter which duplicate \
         survives without moving any pin — confined to malformed input, and test-pinned. Elsewhere it \
         arrives via serde_json, where our manifests are JCS-canonical (sorted), so map order cannot \
         reach a sealed byte",
    ),
    ("once_cell", "lazy initialisation"),
    ("parking_lot_core", "lock primitives"),
    ("pin-project-lite", "pin projection"),
    ("slab", "slot allocator"),
    ("smallvec", "inline-capacity vector"),
    ("zerocopy", "safe transmute traits"),
    ("ahash", "hash container keying"),
    ("const-random", "compile-time hash seeds"),
    ("crunchy", "loop unrolling for the above"),
    ("foldhash", "hash container keying"),
    ("getrandom", "hash seeds"),
    ("siphasher", "phf lookup hashing, not value decoding"),
    ("tiny-keccak", "const-random seeding"),
    ("futures-channel", "async plumbing"),
    ("futures-core", "async plumbing"),
    ("futures-io", "async plumbing"),
    ("futures-sink", "async plumbing"),
    ("futures-task", "async plumbing"),
    ("futures-util", "async plumbing"),
    ("itoa", "integer formatting, write path only"),
    ("ryu", "float formatting, write path only"),
    ("zmij", "float formatting, write path only"),
    ("log", "diagnostics"),
    (
        "serde",
        "chrono's optional text (de)serialisation; this reader takes raw ticks",
    ),
    (
        "serde_core",
        "serde's core traits; reaches the csv lane via `csv` and the arrow lanes via `chrono`, neither \
         for values",
    ),
    (
        "serde_json",
        "arrow's optional JSON schema form; both lanes read schema from Thrift/FlatBuffers",
    ),
];

/// Is this candidate classified? `None` means nobody has decided, and the gate fails.
pub fn classification(krate: &str) -> Option<&'static str> {
    if IN_DIGEST.contains(&krate) {
        return Some("in the digest");
    }
    EXCLUDED.iter().find(|(c, _)| *c == krate).map(|(_, w)| *w)
}

/// One resolved package from `Cargo.lock`.
///
/// Identity is `(name, version)`, never the name alone: a lockfile legitimately holds several versions of
/// one crate and they are different code. Keying by name sealed `base64=0.21.7` for a lane that depends on
/// `base64 0.22.1` (#477 review).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pkg {
    pub name: String,
    pub version: String,
    /// `registry+…` / `git+…`, or `None` for a path dependency.
    pub source: Option<String>,
    /// `(name, version)` of each dependency; the version is present when the lockfile disambiguates.
    pub deps: Vec<(String, Option<String>)>,
}

impl Pkg {
    /// This package's pin, as it appears in a pre-image.
    ///
    /// A default-registry release keeps the bare `name=version` shape, so adopting `source` moved nothing
    /// on the fork-free builds that already existed. Anything else records where it came from: a git fork,
    /// or a private registry — which would otherwise read as crates.io at the same version.
    pub fn pin(&self) -> String {
        match self.source.as_deref() {
            Some(DEFAULT_REGISTRY) => format!("{}={}", self.name, self.version),
            Some(s) => format!("{}={}@{s}", self.name, self.version),
            // `Cargo.lock` records no path for a path dependency, so there is nothing further to hash.
            // The gate refuses one on a decode path rather than pretend this is a complete identity.
            None => format!("{}={}@path", self.name, self.version),
        }
    }
}

/// Parse every `[[package]]` out of a `Cargo.lock`.
///
/// Hand-scanned to keep `build.rs` dependency-free: a `[build-dependencies]` entry would move the resolved
/// feature graph, which is an ADR-0057 §5 Gate B event — a disproportionate price for reading a lockfile.
/// The shape is fixed by cargo (one key per line, no nested tables).
pub fn parse_lock(lock: &str) -> Vec<Pkg> {
    let mut out: Vec<Pkg> = Vec::new();
    let mut cur: Option<Pkg> = None;
    let mut in_deps = false;
    for line in lock.lines() {
        let t = line.trim();
        if t == "[[package]]" {
            if let Some(p) = cur.take() {
                out.push(p);
            }
            cur = Some(Pkg {
                name: String::new(),
                version: String::new(),
                source: None,
                deps: Vec::new(),
            });
            in_deps = false;
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        if in_deps {
            if t == "]" {
                in_deps = false;
                continue;
            }
            // `"name"`, `"name version"`, or `"name version (source)"`.
            let e = t.trim_end_matches(',').trim_matches('"');
            let mut parts = e.split_whitespace();
            if let Some(n) = parts.next() {
                p.deps.push((n.to_owned(), parts.next().map(str::to_owned)));
            }
        } else if let Some(v) = t.strip_prefix("name = ") {
            p.name = v.trim_matches('"').to_owned();
        } else if let Some(v) = t.strip_prefix("version = ") {
            p.version = v.trim_matches('"').to_owned();
        } else if let Some(v) = t.strip_prefix("source = ") {
            p.source = Some(v.trim_matches('"').to_owned());
        } else if t == "dependencies = [" {
            in_deps = true;
        } else if t.starts_with('[') {
            // A non-`[[package]]` table ends the current package.
            out.extend(cur.take());
        }
    }
    out.extend(cur);
    out
}

/// Resolve one dependency edge. An entry carrying a version names it exactly; one without is unambiguous
/// by construction (cargo only omits the version when a single package has that name).
pub fn resolve<'a>(pkgs: &'a [Pkg], name: &str, version: Option<&str>) -> Option<&'a Pkg> {
    match version {
        Some(v) => pkgs.iter().find(|p| p.name == name && p.version == v),
        None => {
            let mut it = pkgs.iter().filter(|p| p.name == name);
            let first = it.next()?;
            // Ambiguous without a version: refuse to guess rather than seal an arbitrary one.
            it.next().is_none().then_some(first)
        }
    }
}

/// Every package reachable from `roots` — a lane's candidate set, keyed by `(name, version)` and sorted so
/// the pre-image depends only on what resolved, never on iteration order.
pub fn closure<'a>(pkgs: &'a [Pkg], roots: &[String]) -> Vec<&'a Pkg> {
    let mut seen: Vec<&Pkg> = Vec::new();
    let mut stack: Vec<&Pkg> = roots
        .iter()
        .filter_map(|r| resolve(pkgs, r, None))
        .collect();
    while let Some(p) = stack.pop() {
        if seen
            .iter()
            .any(|s| s.name == p.name && s.version == p.version)
        {
            continue;
        }
        seen.push(p);
        for (dn, dv) in &p.deps {
            if let Some(d) = resolve(pkgs, dn, dv.as_deref()) {
                stack.push(d);
            }
        }
    }
    seen.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    seen
}

/// The ingest lanes and the crates each one's Cargo feature declares, read from `[features]`.
///
/// Derived rather than restated: a lane is a feature whose transitive expansion enables at least one
/// `dep:` (so `static-hdf5`, which only forwards `hdf5-metno/…`, is not a lane), minus [`NOT_A_LANE`].
/// Adding a decoder crate to a lane's feature therefore puts it in that lane's digest with no second
/// edit — and forgetting the second edit is how the global list drifted in the first place.
///
/// Handles a feature array spanning several lines, which a single-line scan silently truncated.
pub fn lane_roots(cargo_toml: &str) -> Vec<(String, Vec<String>)> {
    let mut table: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_features = false;
    let mut pending: Option<(String, String)> = None;
    for line in cargo_toml.lines() {
        let t = line.split('#').next().unwrap_or("").trim();
        if let Some((name, acc)) = pending.as_mut() {
            acc.push_str(t);
            if acc.contains(']') {
                let (n, a) = (name.clone(), acc.clone());
                table.push((n, split_entries(&a)));
                pending = None;
            }
            continue;
        }
        if t.starts_with('[') && !t.starts_with("[[") {
            in_features = t == "[features]";
            continue;
        }
        if !in_features || t.is_empty() {
            continue;
        }
        let Some((name, rest)) = t.split_once('=') else {
            continue;
        };
        let (name, rest) = (name.trim().to_string(), rest.trim().to_string());
        if rest.contains(']') {
            table.push((name, split_entries(&rest)));
        } else {
            // The array continues on following lines.
            pending = Some((name, rest));
        }
    }

    // Expand each feature to the `dep:` crates it enables, following feature→feature edges.
    let mut lanes = Vec::new();
    for (name, _) in &table {
        if NOT_A_LANE.contains(&name.as_str()) {
            continue;
        }
        let mut deps: Vec<String> = Vec::new();
        let mut stack = vec![name.clone()];
        let mut seen: Vec<String> = Vec::new();
        while let Some(f) = stack.pop() {
            if seen.contains(&f) {
                continue;
            }
            seen.push(f.clone());
            let Some((_, entries)) = table.iter().find(|(n, _)| *n == f) else {
                continue;
            };
            for e in entries {
                if let Some(d) = e.strip_prefix("dep:") {
                    if !deps.contains(&d.to_string()) {
                        deps.push(d.to_string());
                    }
                } else if !e.contains('/') {
                    stack.push(e.clone());
                }
            }
        }
        if !deps.is_empty() {
            deps.sort();
            lanes.push((name.clone(), deps));
        }
    }
    lanes.sort();
    lanes
}

/// The quoted entries of a (possibly multi-line, now joined) feature array.
fn split_entries(s: &str) -> Vec<String> {
    s.trim()
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or("")
        .split(',')
        .map(|e| e.trim().trim_matches('"').to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// The `v2` pre-image for one lane: every in-digest package on its decode path, with version and — unless
/// it is a plain crates.io release — its source. **One pin per resolved version**, so a crate present at
/// two versions is named twice.
pub fn lane_preimage(lock: &str, roots: &[String]) -> Option<String> {
    let pkgs = parse_lock(lock);
    let pins: Vec<String> = closure(&pkgs, roots)
        .into_iter()
        .filter(|p| IN_DIGEST.contains(&p.name.as_str()))
        .map(Pkg::pin)
        .collect();
    if pins.is_empty() {
        return None;
    }
    Some(format!("{PREIMAGE_VERSION};pins={}", pins.join(",")))
}
