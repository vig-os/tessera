"""
Independent pure-Python reader for the Tessera .tsra container format.

Implemented from the Tessera SPEC (v0). Verifies the three identity hashes
(id, content_hash, manifest_hash) and each block's digest.

Spec notes used here (only):
  - Container is a ZIP (zip64), entries STORED, in order:
      mimetype (== "application/vnd.tessera"),
      manifest.json (I-JSON, nested at most 127 deep, in the shape SPEC §5 declares: see
      MANIFEST below and README "What it checks"),
      blocks/<name>... (one per block, payload bytes).
  - Hashing: digest(b) = "blake3:" + hex(blake3(b))    (256-bit, lowercase)
  - merkle_root(digests) = the Merkle Mountain Range (MMR) root, domain-separated
    leaf(d)=blake3(0x00++utf8(d)) / node(l,r)=blake3(0x01++l++r); empty = blake3(b"").
  - All hashing inputs derived from JSON are RFC 8785 (JCS) canonical bytes.
  - id            = digest(JCS(id_inputs))
  - content_hash  = merkle_root([block.digest for block in manifest.blocks])
  - Block names are unique within a product (SPEC §5, exact string equality):
    a manifest listing one name twice is refused before any digest lookup.
  - manifest_hash = digest(JCS(manifest without "manifest_hash" key))
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
    first-wins), a NaN/Infinity constant, or a lone surrogate (RFC 8259 / RFC 7493 §2.1)."""


def _refuse_constant(name: str) -> Any:
    raise NotAManifest(f"{name} is not a JSON number")


def _finite_float(text: str) -> float:
    value = float(text)
    if value in (float("inf"), float("-inf")):
        raise NotAManifest(f"number {text} is out of range for a double")
    return value


def _finite_int(text: str) -> int:
    """An integer literal a double can hold the magnitude of (a JSON number is a double, SPEC §4)."""
    value = int(text)
    try:
        float(value)
    except OverflowError:
        raise NotAManifest(
            f"number {text[:20]}… is out of range for a double"
        ) from None
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


def parse_manifest(raw: bytes | str) -> dict[str, Any]:
    """Parse manifest.json, refusing text that is not UTF-8 JSON (a byte-order mark included), a
    duplicate key in any object at any depth, NaN/Infinity, a number out of a double's range,
    (checked by encoding the text back to UTF-8) a lone surrogate escape, nesting deeper than MAX_NESTING, and a manifest outside its
    declared shape (`MANIFEST`)."""
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
    try:
        json.dumps(m, ensure_ascii=False).encode("utf-8")
    except UnicodeEncodeError as e:
        raise NotAManifest(f"lone surrogate: {e}") from e
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
    manifest_hash=(_opt(STR), _DEF),
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


def manifest_seal(manifest: dict[str, Any]) -> str:
    """manifest_hash: JCS over the manifest object WITHOUT the manifest_hash key (SPEC §4)."""
    return digest(
        jcs_bytes({k: v for k, v in manifest.items() if k != "manifest_hash"})
    )


def version_supported(version: str) -> bool:
    """Whether a reader of major SUPPORTED_MAJOR reads `version`: its major (the text before the
    first '.') is an unsigned decimal integer (an optional '+', ASCII digits) no larger than ours."""
    major = version.split(".", 1)[0]
    if not re.fullmatch(r"\+?[0-9]+", major) or int(major) >= 2**64:
        return False
    return int(major) <= SUPPORTED_MAJOR


def manifest_verdict(raw: bytes | str) -> str:
    """A manifest's own three hashes, without block payloads: "ok", "tampered" or "refused".
    Refused: not I-JSON, outside its declared shape, a major tessera_version this reader doesn't
    support, a block name listed twice, or a block without a digest (SPEC §5: a sealed product's
    blocks carry one). The checks run in SPEC §7's order, the hashes `id`, `content_hash`,
    `manifest_hash` last, so a manifest with two faults gets the verdict of the first."""
    try:
        m = parse_manifest(raw)
    except NotAManifest:
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
    if merkle_root([b["digest"] for b in blocks]) != m.get("content_hash"):
        return "tampered"
    return "ok" if manifest_seal(m) == m.get("manifest_hash") else "tampered"


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

        # (a) mimetype: first entry, exactly application/vnd.tessera, STORED
        if not names or names[0] != MIMETYPE_NAME:
            notes.append(f"first entry must be {MIMETYPE_NAME!r}; got {names[:1]}")
        mt_info = zf.getinfo(MIMETYPE_NAME)
        if mt_info.compress_type != zipfile.ZIP_STORED:
            notes.append("mimetype is not STORED")
        if zf.read(MIMETYPE_NAME) != MIMETYPE_VALUE:
            notes.append("mimetype content mismatch")

        # (b) manifest.json, refused outside its declared shape (raises NotAManifest)
        manifest = parse_manifest(zf.read(MANIFEST_NAME))

        # (c) version gate
        if not version_supported(manifest["tessera_version"]):
            notes.append(f"unsupported tessera_version {manifest['tessera_version']!r}")

        # (d) recompute id
        computed_id = digest(jcs_bytes(manifest["id_inputs"]))

        # (e) recompute content_hash from block digests in manifest order;
        #     also verify each stored block's bytes against its digest.
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

        computed_content_hash = (
            "" if undigested else merkle_root([b["digest"] for b in blocks])
        )

        # (f) recompute manifest_hash: JCS over manifest WITHOUT manifest_hash key.
        computed_manifest_hash = manifest_seal(manifest)

        ok = (
            not notes
            and computed_id == manifest["id"]
            and computed_content_hash == manifest.get("content_hash")
            and computed_manifest_hash == manifest.get("manifest_hash")
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
