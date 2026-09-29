// The **decode-path classification** behind the ADR-0056 §6a per-lane decoder digest (#477).
//
// Three consumers share this one implementation — `build.rs` (`include!`s it, so it must stay
// dependency-free and reference nothing from the crate), the library, and the gate test. A second
// implementation would be a second chance for the digest and the gate to disagree about what the decode
// path *is*, which is the whole defect this fixes.
//
// # What the digest answers, and the three ways it used to get it wrong
//
// `ingest_decoder.features` answers "which decoder build interpreted these bytes". Before #477 it:
//
// 1. Used **one global crate list for every lane**, so adding `zip` for `.npz` moved the
//    `manifest_hash` of every parquet and csv product. A parquet seal committed to a zip library that
//    had no part in reading it.
// 2. Read only `version` from the lockfile and **ignored `source`**, so a decode-path crate pinned to a
//    git fork at the same version was indistinguishable from the registry release — the one question the
//    digest exists to answer, wrong in exactly the case where someone deliberately changed a decoder.
// 3. **Omitted most of the decode path.** The hand-maintained list named 10 crates; the parquet lane's
//    real closure contains 114, of which 41 can change a decoded value. Absent were `thrift` and
//    `integer-encoding` (Parquet's footer and metadata format), `flatbuffers` (Arrow IPC's wire format),
//    and **every page codec** — `snap`, `zstd`, `brotli`, `flate2`, `lz4_flex`. Swapping a snappy
//    implementation moved nothing. `chrono-tz` was absent too, and §6a names `chrono-tz` as hazard H1's
//    mechanism.
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
// declares — and every candidate must be classified: either [`IN_DIGEST`] or excluded with a reason
// ([`EXCLUDED`], [`excluded_by_rule`]). The gate test fails on an unclassified candidate, which makes
// the failure mode **loud**: a new decode-path dependency stops the build until someone decides, where
// before a forgotten one silently narrowed the claim. That inversion matters more than either list.
//
// The candidate set is taken from `Cargo.lock` rather than `cargo metadata` on purpose: the lockfile is
// already read here, needs no subprocess, and works in the hermetic flake check and in a `build.rs`
// alike. It cannot distinguish dependency kinds, so the candidate net is wider than strictly necessary —
// which only makes the gate stricter, and every extra candidate is classified once and forgotten.

/// The pre-image's **derivation version**. Bumped whenever the composition changes, so a reader comparing
/// digests across a change sees a declared difference rather than inferring "the decoder changed".
///
/// `v1` (implicit, unprefixed) was the single global `pins=<crate>=<version>,…`. `v2` is per-lane, carries
/// each crate's source as well as its version, and covers the whole decode path. This digest has now been
/// redefined three times (the crate's own feature set came out, `CARGO_PKG_VERSION` came out, then #477),
/// and each earlier redefinition was silent — a version marker costs 3 bytes and ends that.
pub const PREIMAGE_VERSION: &str = "v2";

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
/// three times over.
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
/// value. Reviewed once per crate; the gate refuses anything absent from both lists.
pub const EXCLUDED: &[(&str, &str)] = &[
    // ── Runs at compile time; cannot read a runtime byte ──
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
    // ── Host facts, not file contents ──
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
    // ── Containers and plumbing: they move bytes, never reinterpret them ──
    ("allocator-api2", "allocation traits"),
    ("bitflags", "typed flag sets"),
    ("bumpalo", "arena allocator"),
    ("bytes", "reference-counted byte buffer"),
    ("cfg-if", "conditional compilation"),
    ("equivalent", "key-equivalence traits"),
    ("hashbrown", "hash container"),
    ("indexmap", "insertion-ordered map"),
    ("once_cell", "lazy initialisation"),
    ("parking_lot_core", "lock primitives"),
    ("pin-project-lite", "pin projection"),
    ("slab", "slot allocator"),
    ("smallvec", "inline-capacity vector"),
    ("zerocopy", "safe transmute traits"),
    // ── Hashing and randomness used for container keying, not for decoding ──
    ("ahash", "hash container keying"),
    ("const-random", "compile-time hash seeds"),
    ("crunchy", "loop unrolling for the above"),
    ("foldhash", "hash container keying"),
    ("getrandom", "hash seeds"),
    ("rand_core", "hash seeds"),
    ("siphasher", "phf lookup hashing, not value decoding"),
    ("tiny-keccak", "const-random seeding"),
    // ── Async plumbing: arrow's optional async reader, which the sync decode path does not drive ──
    ("futures-channel", "async plumbing"),
    ("futures-core", "async plumbing"),
    ("futures-io", "async plumbing"),
    ("futures-sink", "async plumbing"),
    ("futures-task", "async plumbing"),
    ("futures-util", "async plumbing"),
    // ── Number → text: the WRITE direction. Decoding text goes through std's `str::parse`. ──
    ("itoa", "integer formatting, write path only"),
    ("ryu", "float formatting, write path only"),
    ("zmij", "float formatting, write path only"),
    // ── Diagnostics ──
    ("log", "diagnostics"),
    // ── serde ──
    // The sharpest of these, because a misread SCHEMA would change decoded values (int32 vs int64), so
    // the reason has to be about which schema this reader actually sees. `serde_json` arrives via
    // `arrow-schema`, which uses it for arrow's *optional JSON representation* of a schema — a path
    // neither lane takes: Parquet reads its schema from the Thrift footer and Arrow IPC from the
    // FlatBuffers message, both of which ARE in the digest. `serde` arrives via `chrono`, for
    // (de)serialising datetimes as text, which our reader never does (it takes raw ticks).
    (
        "serde",
        "chrono's text (de)serialisation; this reader takes raw ticks",
    ),
    ("serde_core", "as serde"),
    (
        "serde_json",
        "arrow's optional JSON schema form; both lanes read schema from Thrift/FlatBuffers",
    ),
];

/// Pattern-matched exclusions, for families where naming each member adds nothing.
pub fn excluded_by_rule(krate: &str) -> Option<&'static str> {
    const PROC_MACRO_SUFFIXES: &[&str] =
        &["-derive", "_derive", "-macro", "-macro-support", "-shared"];
    const PLATFORM_PREFIXES: &[&str] = &["windows-", "wasm-bindgen", "wasi", "wit-bindgen"];
    if PROC_MACRO_SUFFIXES.iter().any(|s| krate.ends_with(s)) {
        return Some("derive/macro crate: runs at compile time");
    }
    if PLATFORM_PREFIXES.iter().any(|p| krate.starts_with(p)) {
        return Some("platform binding: host facts, not file contents");
    }
    None
}

/// Is this candidate classified? `None` means nobody has decided, and the gate fails.
pub fn classification(krate: &str) -> Option<&'static str> {
    if IN_DIGEST.contains(&krate) {
        return Some("in the digest");
    }
    if let Some((_, why)) = EXCLUDED.iter().find(|(c, _)| *c == krate) {
        return Some(why);
    }
    excluded_by_rule(krate)
}

/// The ingest lanes and the crates each one's Cargo feature declares, read from `[features]`.
///
/// Derived rather than restated: a lane is a feature whose transitive expansion enables at least one
/// `dep:` (so `static-hdf5`, which only forwards `hdf5-metno/…`, is not a lane), minus [`NOT_A_LANE`].
/// Adding a decoder crate to a lane's feature therefore puts it in that lane's digest with no second
/// edit — and forgetting the second edit is how the global list drifted in the first place.
pub fn lane_roots(cargo_toml: &str) -> Vec<(String, Vec<String>)> {
    let mut table: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_features = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_features = t == "[features]";
            continue;
        }
        if !in_features || t.starts_with('#') || t.is_empty() {
            continue;
        }
        let Some((name, rest)) = t.split_once('=') else {
            continue;
        };
        let entries: Vec<String> = rest
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|e| e.trim().trim_matches('"').to_string())
            .filter(|e| !e.is_empty())
            .collect();
        table.push((name.trim().to_string(), entries));
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

/// One `[[package]]`'s `version` and `source` from a `Cargo.lock`.
///
/// Hand-scanned to keep `build.rs` dependency-free: a `[build-dependencies]` entry would move the
/// resolved feature graph, which is an ADR-0057 §5 Gate B event — a disproportionate price for reading two
/// strings. The lockfile's shape is fixed by cargo (one key per line, no nested tables).
pub fn lock_pin(lock: &str, package: &str) -> Option<(String, Option<String>)> {
    let mut wanted = false;
    let mut version = None;
    for line in lock.lines() {
        let t = line.trim();
        if t == "[[package]]" {
            if wanted {
                break; // the wanted package ended without a source (a path dependency)
            }
            version = None;
        } else if let Some(n) = t.strip_prefix("name = ") {
            wanted = n.trim_matches('"') == package;
        } else if wanted {
            if let Some(v) = t.strip_prefix("version = ") {
                version = Some(v.trim_matches('"').to_owned());
            } else if let Some(s) = t.strip_prefix("source = ") {
                return version.map(|v| (v, Some(s.trim_matches('"').to_owned())));
            }
        }
    }
    version.map(|v| (v, None))
}

/// Every crate reachable from `roots` through the lockfile's dependency lists — a lane's candidate set.
pub fn lock_closure(lock: &str, roots: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut stack: Vec<String> = roots.to_vec();
    while let Some(c) = stack.pop() {
        if seen.contains(&c) {
            continue;
        }
        seen.push(c.clone());
        stack.extend(lock_deps(lock, &c));
    }
    seen.sort();
    seen
}

/// The `dependencies = [...]` names of one `[[package]]`.
fn lock_deps(lock: &str, package: &str) -> Vec<String> {
    let mut wanted = false;
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in lock.lines() {
        let t = line.trim();
        if t == "[[package]]" {
            if wanted && in_deps {
                break;
            }
            wanted = false;
            in_deps = false;
        } else if let Some(n) = t.strip_prefix("name = ") {
            wanted = n.trim_matches('"') == package;
        } else if wanted && t == "dependencies = [" {
            in_deps = true;
        } else if in_deps {
            if t == "]" {
                break;
            }
            // Entries are `"name"`, or `"name version"` / `"name version (source)"` when ambiguous.
            let e = t.trim_end_matches(',').trim_matches('"');
            if let Some(first) = e.split_whitespace().next() {
                out.push(first.to_owned());
            }
        }
    }
    out
}

/// The `v2` pre-image for one lane: its decode-path crates, each with version and — when it is not a
/// plain registry release — its source, so a git fork at the same version is a different pin.
///
/// Sorted by crate name, so the string depends only on what resolved and never on iteration order.
pub fn lane_preimage(lock: &str, roots: &[String]) -> Option<String> {
    let mut pins: Vec<String> = Vec::new();
    for c in lock_closure(lock, roots) {
        if !IN_DIGEST.contains(&c.as_str()) {
            continue;
        }
        let Some((version, source)) = lock_pin(lock, &c) else {
            continue;
        };
        match source.as_deref() {
            // A plain crates.io release keeps the bare `crate=version` shape, so adopting `source`
            // moves nothing on a fork-free build: only a fork reads differently, which is the point.
            Some(s) if s.starts_with("registry+") => pins.push(format!("{c}={version}")),
            Some(s) => pins.push(format!("{c}={version}@{s}")),
            // A path dependency has no source; say so rather than implying a registry release.
            None => pins.push(format!("{c}={version}@path")),
        }
    }
    if pins.is_empty() {
        return None;
    }
    Some(format!("{PREIMAGE_VERSION};pins={}", pins.join(",")))
}
