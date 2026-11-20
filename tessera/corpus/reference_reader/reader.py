"""
Independent pure-Python reader for the Tessera .tsra container format.

Implemented from the Tessera SPEC (v0). Verifies the three identity hashes
(id, content_hash, manifest_hash) and each block's digest.

Spec notes used here (only):
  - Container is a ZIP (zip64), entries STORED, in order:
      mimetype (== "application/vnd.tessera"),
      manifest.seal (the 71 ASCII bytes "blake3:" + 64 lower-case hex = digest(manifest.json)),
      manifest.json (I-JSON, nested at most 127 deep, stored as exactly its own RFC 8785 JCS bytes
      with no "manifest_hash" key, in the shape SPEC §5 declares: see MANIFEST below and README
      "What it checks"),
      blocks/<name>... (one per block, payload bytes).
    Each member name appears once (a reader resolving a repeat to its first or its last entry
    would read a different member than another reader), and every entry is STORED: an archive
    with either fault is refused before any member is read.
  - A product without manifest.seal is the pre-ADR-0066 alpha layout: refused, no migration.
  - Hashing: digest(b) = "blake3:" + hex(blake3(b))    (256-bit, lowercase)
  - merkle_root(digests) = the Merkle Mountain Range (MMR) root, domain-separated
    leaf(d)=blake3(0x00++utf8(d)) / node(l,r)=blake3(0x01++l++r); empty = blake3(b"").
  - All hashing inputs derived from JSON are RFC 8785 (JCS) canonical bytes.
  - JSON input is I-JSON (SPEC §1): a number whose value is an integer beyond ±(2^53−1) is refused.
  - id            = digest(JCS(id_inputs))
  - content_hash  = merkle_root([block.digest for block in manifest.blocks if block is primary]):
    a block ref's optional "class" is absent (primary) or exactly "derived" (an accelerator such as
    a chunk index, covered by manifest_hash but not by content_hash, ADR-0028 §4); any other value,
    "primary" and null included, is refused as outside the declared shape.
  - Block names are unique within a product (SPEC §5, exact string equality):
    a manifest listing one name twice is refused before any digest lookup.
  - manifest_hash = digest(manifest.json bytes as stored) = the content of manifest.seal
    (ADR-0066): never a re-serialisation, so the bytes must be their own JCS form.
  - A block without a digest is refused (SPEC §5: a sealed product's blocks carry one).
"""

from __future__ import annotations

import json
import re
import zipfile
import zlib
from dataclasses import dataclass
from typing import Any

import blake3
import jcs

SUPPORTED_MAJOR = 0
MIMETYPE_VALUE = b"application/vnd.tessera"
MIMETYPE_NAME = "mimetype"
MANIFEST_NAME = "manifest.json"
SEAL_NAME = "manifest.seal"
# A seal member: exactly "blake3:" + 64 lower-case hex digits, nothing else (ADR-0066 rule 3).
SEAL_FORMAT = re.compile(rb"blake3:[0-9a-f]{64}")
BLOCK_PREFIX = "blocks/"


def digest(b: bytes) -> str:
    """digest(bytes) = "blake3:" + hex(blake3(bytes))"""
    return "blake3:" + blake3.blake3(b).hexdigest()


def _leaf_hash(d: str) -> bytes:
    """Domain-separated leaf: blake3(0x00 ++ utf8(d))."""
    return blake3.blake3(b"\x00" + d.encode("utf-8")).digest()


def _node_hash(left: bytes, right: bytes) -> bytes:
    """Domain-separated interior node: blake3(0x01 ++ left ++ right)."""
    return blake3.blake3(b"\x01" + left + right).digest()


def merkle_root(digest_strings: list[str]) -> str:
    """
    Product content_hash = the Merkle Mountain Range (MMR) root over the ordered block
    digest strings (ADR-0028). Leaves and interior nodes are domain-separated (see helpers).
    Leaves fold into a list of `peaks` (perfect-subtree roots) on a binary carry; the root
    "bags" the peaks right-to-left with node(). A single leaf returns its leaf hash; the empty
    product returns blake3(b"") — identical to the Rust `tessera_core::hash::merkle_root`.
    """
    peaks: list[
        tuple[int, bytes]
    ] = []  # (height, hash), left -> right (older -> newer)
    for d in digest_strings:
        node = (0, _leaf_hash(d))
        while peaks and peaks[-1][0] == node[0]:
            _, left = peaks.pop()
            node = (node[0] + 1, _node_hash(left, node[1]))
        peaks.append(node)
    if not peaks:
        return "blake3:" + blake3.blake3(b"").hexdigest()
    acc = peaks[-1][1]
    for _, p in reversed(peaks[:-1]):
        acc = _node_hash(p, acc)
    return "blake3:" + acc.hex()


def content_root(blocks: list[dict[str, Any]]) -> str:
    """content_hash (SPEC §4, ADR-0028 §4): the Merkle root over the primary blocks' digests, in
    manifest order. A derived block (`"class": "derived"`) is skipped; with no primary block the
    root is that of the empty list. The caller has refused blocks without a digest."""
    return merkle_root([b["digest"] for b in blocks if b.get("class") != DERIVED])


def jcs_bytes(obj: Any) -> bytes:
    """RFC 8785 canonicalization (UTF-8 bytes)."""
    return jcs.canonicalize(obj)


def duplicate_block_names(blocks: list[dict[str, Any]]) -> list[str]:
    """Every block name the list holds more than once, in first-repeat order (SPEC §5)."""
    seen: set[str] = set()
    repeated: list[str] = []
    for block in blocks:
        name = block["name"]
        if name in seen and name not in repeated:
            repeated.append(name)
        seen.add(name)
    return repeated


class NotAManifest(ValueError):
    """manifest.json is not I-JSON (SPEC §4): a repeated key in one object (refused, never last- or
    first-wins), a NaN/Infinity constant, a lone surrogate (RFC 8259 / RFC 7493 §2.1), or a number
    whose value is an integer beyond ±(2^53−1) (RFC 7493 §2.2, SPEC §1)."""


def _refuse_constant(name: str) -> Any:
    raise NotAManifest(f"{name} is not a JSON number")


# SPEC §1 (I-JSON, RFC 7493 §2.2): the largest integer magnitude a double holds exactly. JCS writes
# every number as a double (RFC 8785 §3.2.2.3), so an integer beyond it would share its canonical
# bytes, and so its seal, with a neighbour (9007199254740993 and 9007199254740992 do).
MAX_EXACT_INTEGER = 2**53 - 1


def _inexact(text: str) -> NotAManifest:
    return NotAManifest(
        f"number {text[:24]} is an integer beyond ±(2^53−1) (I-JSON, RFC 7493 §2.2)"
    )


def _finite_float(text: str) -> float:
    """A number token with a fraction or an exponent, as the double JCS writes. Refused: out of a
    double's range, or a double beyond ±(2^53−1), which is always an integer (`9007199254740993.0`,
    `1e300`; `9007199254740992.5` too, whose fraction the double has already lost)."""
    value = float(text)
    if value in (float("inf"), float("-inf")):
        raise NotAManifest(f"number {text} is out of range for a double")
    if abs(value) > MAX_EXACT_INTEGER:
        raise _inexact(text)
    return value


def _finite_int(text: str) -> int:
    """An integer token a double holds exactly: |value| ≤ 2^53−1 (SPEC §1)."""
    value = int(text)
    if abs(value) > MAX_EXACT_INTEGER:
        raise _inexact(text)
    return value


def _refuse_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise NotAManifest(f"duplicate key {key!r} in a JSON object")
        out[key] = value
    return out


# SPEC §4: a manifest nests at most this many objects and arrays (the top-level object is 1).
MAX_NESTING = 127


def _nesting(v: Any) -> int:
    """How many objects/arrays deep `v` nests, counted without recursion."""
    deepest, stack = 0, [(v, 1)]
    while stack:
        x, d = stack.pop()
        if isinstance(x, (dict, list)):
            deepest = max(deepest, d)
            stack.extend((c, d + 1) for c in (x.values() if isinstance(x, dict) else x))
    return deepest


def _lone_surrogate(v: Any) -> str | None:
    """The first string or key in `v` holding an unpaired UTF-16 surrogate (U+D800..U+DFFF), which
    `json.loads` accepts from a `\\ud800` escape but UTF-8 (RFC 7493 §2.1, I-JSON) cannot hold."""
    stack = [v]
    while stack:
        x = stack.pop()
        texts: list[str] = []
        if isinstance(x, str):
            texts = [x]
        elif isinstance(x, dict):
            texts = list(x.keys())
            stack.extend(x.values())
        elif isinstance(x, list):
            stack.extend(x)
        for t in texts:
            if any(0xD800 <= ord(c) <= 0xDFFF for c in t):
                return t
    return None


def parse_strict(raw: bytes | str) -> Any:
    """Parse a sealed JSON document, refusing what is not I-JSON (SPEC §1, §4): text that is not
    UTF-8 JSON (a byte-order mark included), a duplicate key in any object at any depth, NaN or
    Infinity, a number out of a double's range or whose value is an integer beyond ±(2^53−1), a lone
    surrogate (refused explicitly, `_lone_surrogate`), and nesting deeper than MAX_NESTING."""
    try:
        # UTF-8 only, strictly: json.loads would also guess UTF-16/32 from bytes. A byte-order
        # mark decodes to U+FEFF, which json.loads then refuses, as the Rust reader does.
        text = raw.decode("utf-8") if isinstance(raw, bytes) else raw
        m = json.loads(
            text,
            object_pairs_hook=_refuse_duplicates,
            parse_constant=_refuse_constant,
            parse_float=_finite_float,
            parse_int=_finite_int,
        )
    except NotAManifest:
        raise
    except (
        ValueError,
        RecursionError,
    ) as e:  # not JSON, not UTF-8, or nested past the stack
        raise NotAManifest(f"not JSON: {e}") from e
    if _nesting(m) > MAX_NESTING:
        raise NotAManifest(
            f"nested more than {MAX_NESTING} objects/arrays deep (SPEC §4)"
        )
    surrogate = _lone_surrogate(m)
    if surrogate is not None:
        raise NotAManifest(f"lone surrogate in {surrogate!r} (RFC 7493 §2.1)")
    return m


def check_canonical(raw: bytes) -> Any:
    """`parse_strict`, then refuse bytes that are not exactly their own RFC 8785 JCS form (ADR-0066
    rule 4): key order, whitespace, escapes and number spellings other than JCS's (`64.0` for `64`)
    each give one value a second sealed byte string, which readers may read differently (#901)."""
    value = parse_strict(raw)
    canonical = jcs_bytes(value)
    if canonical != raw:
        at = next(
            (i for i, (a, b) in enumerate(zip(raw, canonical)) if a != b),
            min(len(raw), len(canonical)),
        )
        raise NotAManifest(
            f"non-canonical bytes: differs from its JCS form at byte {at} (ADR-0066)"
        )
    return value


def parse_manifest(raw: bytes | str) -> dict[str, Any]:
    """Parse manifest.json strictly (`parse_strict`) and refuse a manifest outside its declared
    shape (`MANIFEST`). Not a verification: `stored_verdict` is (the canonical check and the seal
    come first)."""
    m = parse_strict(raw)
    e = shape_error(m, MANIFEST, "")
    if e:
        raise NotAManifest(e)
    return m


# SPEC §5, the manifest's declared shape, one row per field, mirroring the Rust types field by field
# (tessera-core Manifest, BlockRef, Source, Producer, Generation, IngestTransform). A reader refuses a
# manifest whose declared field holds a value of another JSON type or shape, at any depth: a record
# as anything but an object (the array form is a second spelling, SPEC §4), a name outside its enum,
# a string where a boolean is declared, a missing required field. Keys no row declares are newer
# fields (SPEC §4 forward reading): verified as part of the seal, never type-checked. Opaque values
# (`ANY`: `schema`, `blocks[].spec`, the values of `metadata`, `extra`, `generation.config` and
# `ingest_transform[].params`) are data and hold any JSON.
#
# A type is a tuple: ("str",), ("bool",), ("any",), ("name",) (a block name, ADR-0063), ("enum",
# values), ("opt", t) (absent or null allowed), ("list", t), ("map", t) (an object whose values are
# t), ("either", t, u) (the first that fits), ("rec", fields). A record's fields map each key to
# (type, required); a missing optional field is its default, a missing required one is refused.
STR, BOOL, ANY, NAME = ("str",), ("bool",), ("any",), ("name",)


def _opt(t: tuple) -> tuple:
    return ("opt", t)


def _rec(**fields: tuple) -> tuple:
    return ("rec", fields)


_REQ, _DEF = True, False
# A block ref's `class` (SPEC §5): absent for a primary block, else exactly this.
DERIVED = "derived"
BLOCK_KINDS = ("array", "table", "chunk_index", "blob")
PRODUCER = _rec(
    tool=(STR, _REQ),
    version=(STR, _REQ),
    git_commit=(_opt(STR), _DEF),
    git_repo=(_opt(STR), _DEF),
    dirty=(_opt(BOOL), _DEF),
)
MANIFEST = _rec(
    tessera_version=(STR, _REQ),
    id=(STR, _REQ),
    id_inputs=(("map", STR), _REQ),
    product=(STR, _REQ),
    name=(STR, _REQ),
    description=(STR, _REQ),
    timestamp=(STR, _REQ),
    producer=(_opt(("either", PRODUCER, STR)), _DEF),
    generation=(
        _opt(_rec(config=(("map", ANY), _DEF), config_ref=(_opt(STR), _DEF))),
        _DEF,
    ),
    schema=(_opt(ANY), _DEF),
    blocks=(
        (
            "list",
            _rec(
                name=(NAME, _REQ),
                kind=(("enum", BLOCK_KINDS), _REQ),
                digest=(_opt(STR), _DEF),
                # ADR-0028 §4: absent = primary; present, the only spelling is "derived".
                **{"class": (("enum", (DERIVED,)), _DEF)},
                spec=(ANY, _DEF),
            ),
        ),
        _DEF,
    ),
    sources=(
        (
            "list",
            _rec(
                role=(STR, _REQ), reference=(STR, _REQ), content_hash=(_opt(STR), _DEF)
            ),
        ),
        _DEF,
    ),
    study=(_opt(STR), _DEF),
    metadata=(("map", ANY), _DEF),
    extra=(("map", ANY), _DEF),
    ingest_transform=(
        ("list", _rec(name=(STR, _REQ), params=(("map", ANY), _DEF))),
        _DEF,
    ),
    content_hash=(_opt(STR), _DEF),
)


def block_name_error(name: str) -> str | None:
    """Why `name` is outside the block-name grammar (ADR-0063, SPEC §5), or None."""
    if not name:
        return "empty"
    if len(name.encode("utf-8")) > 255:
        return "longer than 255 bytes"
    for segment in name.split("/"):
        if not segment:
            return "an empty segment"
        if segment in (".", ".."):
            return f"a {segment!r} segment"
        if any(not (c.isascii() and (c.isalnum() or c in "._-")) for c in segment):
            return "a character outside [A-Za-z0-9._-]"
    return None


def shape_error(value: Any, t: tuple, at: str) -> str | None:
    """The first place, by dotted path, where `value` doesn't have the declared shape `t`."""
    kind = t[0]
    where = at or "the manifest"
    if kind == "any":
        return None
    if kind == "opt":
        return None if value is None else shape_error(value, t[1], at)
    if kind == "either":
        if shape_error(value, t[1], at) is None or shape_error(value, t[2], at) is None:
            return None
        return f"{where} is neither of its declared shapes"
    if kind in ("str", "name", "enum"):
        if not isinstance(value, str):
            return f"{where} is not a string"
        if kind == "enum" and value not in t[1]:
            return f"{where} is {value!r}, not one of {list(t[1])}"
        why = block_name_error(value) if kind == "name" else None
        return f"{where} is not a block name: {why}" if why else None
    if kind == "bool":
        return None if isinstance(value, bool) else f"{where} is not a boolean"
    if kind == "list":
        if not isinstance(value, list):
            return f"{where} is not a JSON array"
        items = (shape_error(v, t[1], f"{at}[{i}]") for i, v in enumerate(value))
        return next((e for e in items if e), None)
    if not isinstance(value, dict):
        return f"{where} is not a JSON object"
    if kind == "map":
        items = (
            shape_error(v, t[1], f"{at}.{k}" if at else k) for k, v in value.items()
        )
        return next((e for e in items if e), None)
    for key, (field, required) in t[1].items():
        path = f"{at}.{key}" if at else key
        if key not in value:
            if required:
                return f"{path} is missing"
            continue
        e = shape_error(value[key], field, path)
        if e:
            return e
    return None


def version_supported(version: str) -> bool:
    """Whether a reader of major SUPPORTED_MAJOR reads `version`: its major (the text before the
    first '.') is an unsigned decimal integer (an optional '+', ASCII digits) no larger than ours."""
    major = version.split(".", 1)[0]
    if not re.fullmatch(r"\+?[0-9]+", major) or int(major) >= 2**64:
        return False
    return int(major) <= SUPPORTED_MAJOR


def stored_verdict(raw: bytes | str, seal: bytes | str | None) -> str:
    """A stored manifest's verdict over its two members, `manifest.json` (`raw`) and `manifest.seal`
    (`seal`, None when the container has none), without block payloads: "ok", "tampered" or
    "refused". In ADR-0066 rule 4's order, so a manifest with two faults gets the verdict of the
    first:

    refused: no seal (the alpha layout), a seal not exactly "blake3:" + 64 lower-case hex, not
    I-JSON, not its own JCS form, a "manifest_hash" key in band; tampered: the seal is not the
    digest of the bytes; then the typed layer: refused outside the declared shape, for an
    unsupported major tessera_version, a block name listed twice or a block without a digest;
    tampered for an `id` or `content_hash` (a missing one too) that doesn't match."""
    raw = raw.encode("utf-8") if isinstance(raw, str) else raw
    seal = seal.encode("utf-8") if isinstance(seal, str) else seal
    if seal is None or not SEAL_FORMAT.fullmatch(seal):
        return "refused"
    try:
        m = check_canonical(raw)
    except NotAManifest:
        return "refused"
    if isinstance(m, dict) and "manifest_hash" in m:
        return "refused"
    if digest(raw).encode("ascii") != seal:
        return "tampered"
    if shape_error(m, MANIFEST, ""):
        return "refused"
    if not version_supported(m["tessera_version"]):
        return "refused"
    # SPEC §5: a block name listed twice is refused before any digest is looked at.
    blocks = m.get("blocks", [])
    if duplicate_block_names(blocks):
        return "refused"
    if digest(jcs_bytes(m["id_inputs"])) != m["id"]:
        return "tampered"
    if any(b.get("digest") is None for b in blocks):
        return "refused"
    if content_root(blocks) != m.get("content_hash"):
        return "tampered"
    return "ok"


@dataclass
class VerifyResult:
    fixture: str
    ok: bool
    computed_id: str
    computed_content_hash: str
    computed_manifest_hash: str
    block_digest_ok: list[tuple[str, bool]]
    notes: list[str]


def verify_tsra(path: str) -> VerifyResult:
    """Verify a `.tsra` file. Always returns a result: a file that can't be read as a product (not
    a zip, a missing member, a manifest `parse_manifest` refuses) is `ok=False` with the reason."""
    try:
        return _verify_tsra(path)
    except (
        NotAManifest,
        zipfile.BadZipFile,
        KeyError,
        OSError,
        EOFError,
        RuntimeError,
        NotImplementedError,
        zlib.error,
    ) as e:
        # What zipfile raises for an archive it can't read: KeyError for a missing member
        # (manifest.json, blocks/<name>), RuntimeError for an encrypted one, NotImplementedError
        # for an unknown compression method, EOFError/zlib.error for a truncated or corrupt one.
        return VerifyResult(path, False, "", "", "", [], [f"refused: {e!r}"])


def _verify_tsra(path: str) -> VerifyResult:
    notes: list[str] = []
    with zipfile.ZipFile(path, "r") as zf:
        names = zf.namelist()

        # (a) SPEC §6's member set, from the central directory, before any member is read: each
        #     name once (zipfile resolves a repeated name to its last entry, another reader to its
        #     first, so the two would verify different members), and every entry STORED.
        repeated = sorted({n for n in names if names.count(n) > 1})
        if repeated:
            raise NotAManifest(f"member names listed more than once: {repeated}")
        compressed = [
            i.filename for i in zf.infolist() if i.compress_type != zipfile.ZIP_STORED
        ]
        if compressed:
            raise NotAManifest(f"entries not STORED (SPEC §6): {compressed}")

        # (b) mimetype: first entry, exactly application/vnd.tessera
        if not names or names[0] != MIMETYPE_NAME:
            notes.append(f"first entry must be {MIMETYPE_NAME!r}; got {names[:1]}")
        if zf.read(MIMETYPE_NAME) != MIMETYPE_VALUE:
            notes.append("mimetype content mismatch")

        # (c) manifest.seal then manifest.json, entries 2 and 3 (ADR-0066 rule 3). No seal is the
        #     alpha layout: refused, no migration (rule 10).
        if SEAL_NAME not in names:
            raise NotAManifest(
                f"no {SEAL_NAME}: a pre-ADR-0066 (alpha) layout, not read (re-ingest the product)"
            )
        if names[1:3] != [SEAL_NAME, MANIFEST_NAME]:
            notes.append(
                f"entries 2 and 3 must be {SEAL_NAME}, {MANIFEST_NAME}; got {names[1:3]}"
            )
        raw = zf.read(MANIFEST_NAME)
        seal = zf.read(SEAL_NAME)
        if not SEAL_FORMAT.fullmatch(seal):
            raise NotAManifest(
                f"{SEAL_NAME} is not blake3:<64 lower-case hex>: {seal[:80]!r}"
            )
        # The canonical check and the in-band refusal, before the seal and the typed layer.
        value = check_canonical(raw)
        if isinstance(value, dict) and "manifest_hash" in value:
            raise NotAManifest(
                "manifest.json carries its seal in band (`manifest_hash`)"
            )
        # (d) the seal: the digest of the stored bytes, never a re-serialisation.
        computed_manifest_hash = digest(raw)
        if computed_manifest_hash.encode("ascii") != seal:
            notes.append(f"{SEAL_NAME} {seal.decode('ascii')} != digest(manifest.json)")
        manifest = parse_manifest(raw)

        # (e) version gate
        if not version_supported(manifest["tessera_version"]):
            notes.append(f"unsupported tessera_version {manifest['tessera_version']!r}")

        # (f) recompute id
        computed_id = digest(jcs_bytes(manifest["id_inputs"]))

        # (g) recompute content_hash from the primary blocks' digests in manifest order;
        #     also verify every stored block's bytes (derived ones too) against its digest.
        blocks = manifest.get("blocks", [])
        duplicates = duplicate_block_names(blocks)
        if duplicates:
            # SPEC §5/§7: block names are unique within a product (exact string equality), and a
            # repeat is refused before any digest is looked up: a name resolves to one listed
            # digest, so a second entry's digest would cover no bytes. No payload is read.
            notes.append(
                f"block names listed more than once (SPEC §5 requires unique names): {duplicates}"
            )
        undigested = [b["name"] for b in blocks if b.get("digest") is None]
        if undigested:
            notes.append(f"blocks without a digest (SPEC §5): {undigested}")
        block_digest_ok: list[tuple[str, bool]] = []
        for block in [] if duplicates or undigested else blocks:
            name = block["name"]
            recorded = block["digest"]
            payload = zf.read(BLOCK_PREFIX + name)
            actual = digest(payload)
            block_digest_ok.append((name, actual == recorded))
            if actual != recorded:
                notes.append(
                    f"block {name!r}: payload digest {actual} != recorded {recorded}"
                )

        computed_content_hash = "" if undigested else content_root(blocks)

        ok = (
            not notes
            and computed_id == manifest["id"]
            and computed_content_hash == manifest.get("content_hash")
            and all(b[1] for b in block_digest_ok)
        )

        return VerifyResult(
            fixture=path,
            ok=ok,
            computed_id=computed_id,
            computed_content_hash=computed_content_hash,
            computed_manifest_hash=computed_manifest_hash,
            block_digest_ok=block_digest_ok,
            notes=notes,
        )


# ---------------------------------------------------------------------------------------------
# Number rules of a product schema (SPEC §5 "Field rules", #895). Not part of the seal: a schema's
# contract, checked where a product is made or validated. `run.py` holds this to the Rust reader's
# verdicts over `corpus/schema-cases.json`.

# FieldSpec dtypes: the array dtypes plus the metadata kinds (SPEC §5).
_NUMBER_DTYPES = {
    "int8",
    "int16",
    "int32",
    "uint8",
    "uint16",
    "uint32",
    "float16",
    "float32",
    "float64",
}
_WIDE_DTYPES = {"int64": (-(2**63), 2**63 - 1), "uint64": (0, 2**64 - 1)}
_OTHER_DTYPES = {"bool", "string", "coded", "json"}
_DECIMAL = re.compile(r"-?(0|[1-9][0-9]*)")


def _is_number(v: Any) -> bool:
    """A JSON number (Python's `bool` is an `int`, and JSON's true/false are not numbers)."""
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def field_definition_error(f: dict[str, Any]) -> str | None:
    """Why the FieldSpec `f` is refused, or None: an unknown dtype; int64/uint64 without
    `"encoding": "string"`; an encoding on any other dtype; a range on a non-numeric dtype,
    beyond ±(2^53−1), or empty (minimum above maximum)."""
    dtype, encoding = f.get("dtype"), f.get("encoding")
    lo, hi = f.get("minimum"), f.get("maximum")
    if (
        dtype not in _NUMBER_DTYPES
        and dtype not in _WIDE_DTYPES
        and dtype not in _OTHER_DTYPES
    ):
        return f"dtype {dtype!r} is not a FieldSpec dtype"
    if dtype in _WIDE_DTYPES and encoding != "string":
        return f"dtype {dtype} can exceed ±(2^53−1) and declares no string encoding"
    if dtype not in _WIDE_DTYPES and encoding is not None:
        return f"encoding {encoding!r} on dtype {dtype}"
    if lo is None and hi is None:
        return None
    if dtype in _OTHER_DTYPES:
        return f"a range on dtype {dtype}"
    for name, bound in (("minimum", lo), ("maximum", hi)):
        if bound is not None and not (
            _is_number(bound) and abs(bound) <= MAX_EXACT_INTEGER
        ):
            return f"{name} {bound!r} is outside ±(2^53−1)"
    if lo is not None and hi is not None and lo > hi:
        return f"minimum {lo} above maximum {hi}"
    return None


def _default_error(f: dict[str, Any]) -> str | None:
    """Why the field's `default` breaks its own rules, or None (absent or JSON `null`)."""
    if f.get("default") is None:
        return None
    why = field_value_error(f, f["default"])
    return f"default: {why}" if why else None


def field_value_error(f: dict[str, Any], value: Any) -> str | None:
    """Why `value` breaks the FieldSpec `f` (whose definition passed), or None. A string-encoded
    field takes the canonical decimal string of an integer of its dtype, or a non-empty array of
    them; a ranged field a number, or a non-empty array of numbers, each within
    [minimum, maximum]. Python compares an int with a float exactly, as the rule requires."""
    wide = _WIDE_DTYPES.get(f["dtype"]) if f.get("encoding") == "string" else None
    lo, hi = f.get("minimum"), f.get("maximum")
    if wide is None and lo is None and hi is None:
        return None
    items = value if isinstance(value, list) else [value]
    if not items:
        return "an empty array"
    for item in items:
        if wide is not None:
            if not (
                isinstance(item, str) and _DECIMAL.fullmatch(item) and item != "-0"
            ):
                return f"{item!r} is not a decimal string"
            n = int(item)
            if not wide[0] <= n <= wide[1]:
                return f"{item} is outside {f['dtype']}"
        elif _is_number(item):
            n = item
        else:
            return f"{item!r} is not a number"
        if lo is not None and n < lo:
            return f"{item} is below the minimum {lo}"
        if hi is not None and n > hi:
            return f"{item} is above the maximum {hi}"
    return None


def schema_verdict(schema: dict[str, Any], metadata: dict[str, Any]) -> str:
    """`ok`, `schema` (a field declaration is refused: a repeated id, a rule above, or a `default`
    its own rules refuse) or `value` (a metadata value is refused): every declaration is checked
    before any value, values in metadata key order. `schema` is one the shape and newer-field
    checks already accept (SPEC §4/§5): a key this reader doesn't model, or a value of the wrong
    JSON type, is refused there, before this function is asked."""
    fields = schema.get("fields", [])
    ids = [f["id"] for f in fields]
    if len(set(ids)) != len(ids):
        return "schema"  # a field id names one field
    if any(field_definition_error(f) or _default_error(f) for f in fields):
        return "schema"
    by_id = {f["id"]: f for f in fields}
    for key in sorted(metadata):
        if key in by_id and field_value_error(by_id[key], metadata[key]):
            return "value"
    return "ok"
