"""
Independent pure-Python reader for the Tessera .tsra container format.

Implemented from the Tessera SPEC (v0). Verifies the three identity hashes
(id, content_hash, manifest_hash) and each block's digest.

Spec notes used here (only):
  - Container is a ZIP (zip64), entries STORED, in order:
      mimetype (== "application/vnd.tessera"),
      manifest.json (any valid JSON),
      blocks/<name>... (one per block, payload bytes).
  - Hashing: digest(b) = "blake3:" + hex(blake3(b))    (256-bit, lowercase)
  - merkle_root(digests) = the Merkle Mountain Range (MMR) root, domain-separated
    leaf(d)=blake3(0x00++utf8(d)) / node(l,r)=blake3(0x01++l++r); empty = blake3(b"").
  - All hashing inputs derived from JSON are RFC 8785 (JCS) canonical bytes.
  - id            = digest(JCS(id_inputs))
  - content_hash  = merkle_root([block.digest for block in manifest.blocks])
  - manifest_hash = digest(JCS(manifest without "manifest_hash" key))
"""

from __future__ import annotations

import json
import zipfile
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


def _refuse_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise NotAManifest(f"duplicate key {key!r} in a JSON object")
        out[key] = value
    return out


def parse_manifest(raw: bytes | str) -> dict[str, Any]:
    """Parse manifest.json, refusing a duplicate key in any object at any depth, NaN/Infinity, a
    number out of a double's range, and (checked by encoding the text back to UTF-8) a lone
    surrogate escape."""
    m = json.loads(
        raw,
        object_pairs_hook=_refuse_duplicates,
        parse_constant=_refuse_constant,
        parse_float=_finite_float,
    )
    try:
        json.dumps(m, ensure_ascii=False).encode("utf-8")
    except UnicodeEncodeError as e:
        raise NotAManifest(f"lone surrogate: {e}") from e
    return m


def manifest_seal(manifest: dict[str, Any]) -> str:
    """manifest_hash: JCS over the manifest object WITHOUT the manifest_hash key (SPEC §4)."""
    return digest(
        jcs_bytes({k: v for k, v in manifest.items() if k != "manifest_hash"})
    )


def manifest_verdict(raw: bytes | str) -> str:
    """A manifest's own three hashes, without block payloads: "ok", "tampered" or "refused"
    (not I-JSON, or a major tessera_version this reader doesn't support)."""
    try:
        m = parse_manifest(raw)
    except NotAManifest:
        return "refused"
    try:
        major = int(str(m.get("tessera_version", "")).split(".", 1)[0])
    except ValueError:
        return "refused"
    if major > SUPPORTED_MAJOR:
        return "refused"
    ok = (
        digest(jcs_bytes(m.get("id_inputs", {}))) == m.get("id")
        and merkle_root([b["digest"] for b in m.get("blocks", [])])
        == m.get("content_hash")
        and manifest_seal(m) == m.get("manifest_hash")
    )
    return "ok" if ok else "tampered"


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

        # (b) manifest.json
        manifest_raw = zf.read(MANIFEST_NAME)
        manifest = parse_manifest(manifest_raw)

        # (c) version gate
        ver = manifest.get("tessera_version", "0.0.0")
        try:
            major = int(ver.split(".", 1)[0])
        except Exception:
            major = 0
        if major > SUPPORTED_MAJOR:
            notes.append(f"unsupported tessera_version major {major}")

        # (d) recompute id
        id_inputs = manifest.get("id_inputs", {})
        computed_id = digest(jcs_bytes(id_inputs))

        # (e) recompute content_hash from block digests in manifest order;
        #     also verify each stored block's bytes against its digest.
        blocks = manifest.get("blocks", [])
        block_digest_ok: list[tuple[str, bool]] = []
        block_digests: list[str] = []
        for block in blocks:
            name = block["name"]
            recorded = block["digest"]
            block_digests.append(recorded)
            payload = zf.read(BLOCK_PREFIX + name)
            actual = digest(payload)
            block_digest_ok.append((name, actual == recorded))
            if actual != recorded:
                notes.append(
                    f"block {name!r}: payload digest {actual} != recorded {recorded}"
                )

        computed_content_hash = merkle_root(block_digests)

        # (f) recompute manifest_hash: JCS over manifest WITHOUT manifest_hash key.
        computed_manifest_hash = manifest_seal(manifest)

        ok = (
            computed_id == manifest.get("id")
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
