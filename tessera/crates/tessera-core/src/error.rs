//! Error type for the Tessera core.
//!
//! `#[non_exhaustive]` so adding a variant is not a breaking change, and the core **never
//! panics** on bad input — every fallible path returns a typed [`Error`]. Integrity failures
//! name the field plus the expected and actual values so a caller (or auditor) can see exactly
//! what diverged.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),

    /// Canonical (RFC 8785 JCS) serialization failed — the manifest could not be reduced to
    /// its deterministic byte form for hashing.
    #[error("canonicalization: {0}")]
    Canonicalization(String),

    /// A JSON number in a sealed document (manifest with its embedded schema and metadata,
    /// collection, `id_inputs`, signature envelope) is an integer beyond ±(2^53−1), the range in
    /// which an IEEE-754 double holds every integer exactly ([`crate::canonical::MAX_EXACT_INTEGER`]).
    ///
    /// RFC 8785 (JCS) serialises every number as a double and requires its input to be I-JSON
    /// (RFC 7493 §2.2), so two such integers can canonicalise to the same bytes (`9007199254740993`
    /// and `9007199254740992` do): the seal would not pin the value. Writers refuse it before any
    /// hash is computed and readers refuse it on parse (SPEC §1, §4; #886). A free-form field can carry the
    /// value as a decimal string instead; a typed field cannot. `path` names the value from the
    /// document's root (rendered as
    /// [`crate::canonical::check_i_json`] documents), or as the person typed it for a value read by
    /// [`crate::canonical::parse_user_value`] (`--set daq_start_ns`); `value` is the number as parsed,
    /// or the person's own text for a typed number.
    #[error(
        "{path} = {value} is outside the range a sealed JSON number can hold (±(2^53−1), I-JSON \
         RFC 7493 §2.2): a sealed JSON number is an IEEE-754 double (RFC 8785 §3.2.2.3), which \
         can't hold this integer exactly{advice}",
        advice = string_advice(.as_string)
    )]
    InexactInteger {
        path: String,
        value: String,
        /// The value sits in a free-form field that accepts a string (`metadata`, or a number a
        /// person typed for one), so "write it as a string" is advice they can follow. A typed
        /// field (a `scale`, an affine, a transform) has no string form: its message is neutral.
        as_string: bool,
    },

    /// A number headed for a sealed document is NaN or ±infinity. JSON (RFC 8259 §6) has no spelling
    /// for either, and `serde_json` would write them as `null`, so a seal would silently pin a
    /// different value than the one given (#886). Writers refuse it before any value is built; the
    /// writer's door that read the number names its source in its own words (a DICOM tag, a NIfTI
    /// header field, a TOML key, a `--set` flag). `what` names the value and `value` is its text.
    #[error(
        "{what} = {value} is not a finite number: JSON has no spelling for NaN or infinity \
         (RFC 8259 §6), so a seal cannot hold it"
    )]
    NonFiniteNumber { what: String, value: String },

    /// The stored bytes of a sealed JSON document (`manifest.json`, `collection.json`) are valid
    /// I-JSON but are **not their own RFC 8785 (JCS) form**: keys out of JCS order, whitespace, a
    /// `\u` escape JCS would not write, a number in another spelling (`64.0` for `64`, `1E2` for
    /// `100`), or a pretty layout. Raised by [`crate::canonical::check_canonical`], so by
    /// [`crate::Manifest::from_stored`] and [`crate::Collection::from_stored`] before the seal is
    /// compared (ADR-0066 rules 1 and 4).
    ///
    /// The seal is a hash of the stored bytes, so a respelling made after sealing fails the seal
    /// anyway; this refusal is what stops a respelling the attacker sealed. Without it one value
    /// would have several sealed byte strings, and readers that interpret the spelling differently
    /// (Rust refuses `64.0` for a `u64`, JavaScript reads `64`, Python a float) would disagree under
    /// one valid seal (spike #897, case S7; #901).
    ///
    /// `offset` is the 0-based index of the first byte where the stored bytes and their JCS form
    /// differ (the shorter length when one is a prefix of the other). `found` and `canonical` are up
    /// to 16 bytes of each from `offset`, ASCII-escaped, so the message shows the difference; either
    /// is empty where its bytes end at `offset`. A writer that emits "sorted, compact JSON" without
    /// a JCS library meets this error (Python's `json.dumps` writes `1.0` and `1e-07`): write the
    /// bytes with a JCS serialiser.
    #[error(
        "non-canonical bytes: the stored document differs from its own RFC 8785 (JCS) form at byte \
         {offset} (stored `{found}`, JCS `{canonical}`); a sealed JSON document must be stored as \
         exactly its JCS bytes (ADR-0066)"
    )]
    NonCanonical {
        /// 0-based index of the first byte where the stored bytes and their JCS form differ.
        offset: usize,
        /// Up to 16 stored bytes from `offset`, ASCII-escaped.
        found: String,
        /// Up to 16 bytes of the JCS form from `offset`, ASCII-escaped.
        canonical: String,
    },

    /// Stored bytes in the ADR-0066 layout (the seal lives outside the document) carry the in-band
    /// seal key `manifest_hash` at their top level: the layout of a product or collection written
    /// before ADR-0066, or a document whose writer put the seal inside the bytes it seals. Raised by
    /// [`crate::Manifest::from_stored`] and [`crate::Collection::from_stored`] after the canonical
    /// check, before the seal is compared. `document` names the kind (`manifest`, `collection`).
    #[error(
        "the stored {document} carries a top-level `manifest_hash` key; under ADR-0066 the seal is \
         blake3 over the stored bytes and is kept outside them, so the stored bytes carry no \
         `manifest_hash`"
    )]
    InBandSeal {
        /// The kind of stored document: `manifest` or `collection`.
        document: &'static str,
    },

    /// A recomputed hash did not match the one recorded in the manifest — tampering, corruption,
    /// or a producer bug. `what` names the field (`id` / `content_hash` / `manifest_hash`).
    ///
    /// For stored bytes ([`crate::Manifest::from_stored`], [`crate::Collection::from_stored`]),
    /// `what` is `manifest_hash`, `expected` is the seal the caller passed (as recorded beside the
    /// bytes) and `actual` is `blake3` over the stored bytes.
    #[error("integrity: {what} mismatch — expected {expected}, got {actual}")]
    Integrity {
        what: &'static str,
        expected: String,
        actual: String,
    },

    /// A block's stored payload failed its integrity check while reading/verifying a `.tsra` —
    /// names the container `file` and the `block` so an operator sees exactly what is corrupt,
    /// instead of a bare `io: Invalid checksum` with no locus (#268). `detail` carries the
    /// underlying cause (the zip CRC error, or an `Integrity` mismatch with expected/actual).
    #[error("integrity: block '{block}' in {file} failed verification: {detail}")]
    BlockIntegrity {
        file: String,
        block: String,
        detail: String,
    },

    /// A manifest, a collection or an embedded product schema carries fields from a **newer format
    /// revision** that this build does not model ([`crate::Manifest::newer_fields`],
    /// [`crate::Collection::newer_fields`], [`crate::ProductSchema::from_value`]; ADR-0060 §2.2), and
    /// the operation would build a new seal from it, re-emit it, or validate against it. The fields
    /// are sealed and verified, but this build can't carry or evaluate them, so it refuses instead of
    /// silently dropping sealed content or reporting "valid" against a rule it never checked. Read and
    /// verify still work; use a build that understands the fields to validate or derive a new version.
    #[error(
        "fields from a newer format revision are not interpreted by this build: {}; refusing to \
         re-seal, re-emit or validate against it without them",
        fields.join(", ")
    )]
    NewerFormatFields { fields: Vec<String> },

    /// The manifest's spec version is unparseable or newer than this reader supports.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(String),

    /// A block reference is missing its content digest (cannot be sealed or verified).
    #[error("block '{0}' has no digest")]
    MissingDigest(String),

    /// A `.tsra` container (zip) could not be read or written, or is malformed (bad magic,
    /// missing manifest, truncated central directory).
    #[error("container: {0}")]
    Container(String),

    /// A provenance edge resolved to the **right lineage but a different version**: the parent's `id`
    /// matched the edge's `reference`, but the seal it carries is not the one the edge pinned.
    ///
    /// Deliberately NOT [`Error::Integrity`]. ADR-0036 makes `id` a *lineage handle* that addresses
    /// many `manifest_hash`es, so resolving an edge against a store holding a newer commit of the
    /// parent produces exactly this — a version mismatch, not tampering or corruption. A caller that
    /// cannot tell the two apart either cries corruption over a routine version skew or, worse,
    /// learns to ignore real corruption reported the same way.
    #[error(
        "provenance edge '{role}' -> '{reference}' resolved to a different version — \
         the edge pins {pinned}, the resolved parent seals {resolved}"
    )]
    ProvenanceVersionSkew {
        role: String,
        reference: String,
        pinned: String,
        resolved: String,
    },

    /// A `generation.config_ref` digest names no block carried in the product being read — the block
    /// was dropped, or the reference points into a *different* `.tsra` (ADR-0058 §2 pins the config
    /// **inside** the product it describes). Raised only where a caller explicitly asked for the
    /// config to be dereferenced: the reference is sealed and the rest of the product is valid, so a
    /// plain read must keep working and report the digest as-is.
    #[error(
        "config_ref {digest} names no block carried in {file} — \
         the product carries {blocks} block(s), none with that digest"
    )]
    UnresolvedConfigRef {
        file: String,
        digest: String,
        blocks: usize,
    },

    /// A storage-block codec (array pcodec/zarr, table Vortex) failed to encode or decode a
    /// payload, or was asked for an unsupported dtype/codec combination.
    #[error("codec: {0}")]
    Codec(String),

    /// A code path not implemented yet (e.g. a storage backend behind a feature flag).
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),

    /// Mutating a product after it has been sealed (products are immutable).
    #[error("product is sealed and immutable")]
    Sealed,

    /// Structurally invalid product (bad dtype, rank mismatch, missing required field, ...).
    #[error("invalid product: {0}")]
    Invalid(String),

    /// A block's spec breaks the column contract its product schema declares for the block's role
    /// (ADR-0065 §8): raised by `ProductSchema::check_table` / `check_array`. `violation` names the
    /// role and the column.
    #[error("schema '{product}': {violation}")]
    Contract {
        product: String,
        violation: crate::schema::ContractViolation,
    },

    /// A block name that fails the block-name grammar (ADR-0063, [`crate::block::name`]): empty,
    /// `.` or `..`, an absolute path, an empty segment, a `\`, `:` or NUL, a character outside
    /// `[A-Za-z0-9._-]`, or over 255 bytes. Refused wherever a name enters — a builder, a manifest,
    /// a write-session journal — because a name becomes a path under a base directory (#685).
    #[error("invalid block name '{name}': {reason}")]
    InvalidBlockName {
        name: String,
        reason: crate::block::name::NameError,
    },

    /// Two block names of one product break the **leaf rule** (ADR-0063): `parent` is a parent
    /// prefix of `child` (`volume` and `volume/1`), so the product cannot be laid out as files,
    /// where a name is either a file or a directory. Refused at seal, by a write session, and when
    /// exploding a product to a directory; reading such a product in-zip still works.
    #[error(
        "block names '{parent}' and '{child}' conflict: a block name cannot also be the parent of \
         another (the leaf rule, ADR-0063)"
    )]
    BlockNameConflict { parent: String, child: String },

    /// One product lists block `name` more than once (ADR-0063 §2a, SPEC §5). Block names are unique
    /// within a product, by exact byte equality. A reader resolves a name to one listed digest, so a
    /// second entry's digest would cover no bytes. Refused by every writer (`ProductBuilder::seal`, a
    /// write session's append, `pack`, `pack_streaming`) and by every verification
    /// (`Manifest::verify`, so `Reader::open`, and the deep verify's member check), before any
    /// digest is looked up. Names that differ only in letter case are distinct here (#690).
    #[error(
        "block name '{name}' is listed more than once; block names are unique within a product \
         (ADR-0063)"
    )]
    DuplicateBlockName { name: String },

    /// `name` does not resolve to a logical table, or a partitioned node is invalid (ADR-0064 §2,
    /// §3): refused by every reader resolving the name and by the seal, never skipped or merged.
    /// Every block stays readable by its full name.
    #[error("logical table '{name}': {reason}")]
    LogicalTable {
        name: String,
        reason: crate::block::partition::LogicalTableError,
    },

    /// A streaming table writer refused its input or its call (ADR-0064 §5.3, §5.4): the rows
    /// disagree with the declared `spec.rows`, a row-group is too large, or a single-block writer
    /// was given a table that must be partitioned.
    #[error("streamed table '{name}': {reason}")]
    StreamedTable {
        name: String,
        reason: crate::block::partition::StreamedTableError,
    },

    /// A line of a write-session journal that is **complete** (newline-terminated), so a commit,
    /// but not a valid one: not UTF-8, not JSON, not a block reference, or a block committed a
    /// second time. `WriteSession::recover` refuses the journal rather than drop such a line as a
    /// torn tail (only an unterminated last line is torn), and changes nothing on disk. `line` is
    /// 1-based; `path` is the journal file.
    #[error("write-session journal {} line {line} is complete but not a valid commit: {detail}", path.display())]
    CorruptJournal {
        path: std::path::PathBuf,
        line: usize,
        detail: String,
    },

    /// An append to a write session whose block `name` is `quarantined`, or its parent or child (the
    /// leaf rule, ADR-0063), where `quarantined` is held in the recovery quarantine directory
    /// `quarantine`: a journal tail `WriteSession::recover` could not replay, kept for repair by
    /// hand. The name stays taken until that quarantine is repaired or deleted and the session is
    /// recovered again, so a repair never finds two blocks of one name.
    #[error(
        "block '{name}' collides with block '{quarantined}', held in the write-session quarantine \
         {}: repair or delete that quarantine, then recover the session",
        quarantine.display()
    )]
    QuarantinedBlockName {
        name: String,
        quarantined: String,
        quarantine: std::path::PathBuf,
    },

    /// An ingest backend whose **name is known but whose handler is not compiled into this build**
    /// (ADR-0057 §6/§7).
    ///
    /// ADR-0057 §6 keeps `FormatOptions` closed *and total*: every backend name parses on every
    /// build, because a TOML ingest spec is an archival artifact whose `spec_hash` must not depend on
    /// which binary read it. A feature gate switches off the **handler**, not the name — so "this
    /// build cannot run that backend" is a distinct, *typed* condition rather than a parse failure or
    /// a string to grep for. `tessera` maps it to its own exit code (3), which is what lets a cookbook
    /// recipe branch on "wrong build" versus "wrong invocation" (exit 2) without parsing stderr.
    #[error("backend '{0}' is not compiled into this build")]
    BackendNotCompiled(&'static str),
}

/// The advice an [`Error::InexactInteger`] adds only for a free-form field.
fn string_advice(as_string: &bool) -> &'static str {
    if *as_string {
        "; write it as a string"
    } else {
        ""
    }
}
