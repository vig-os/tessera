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
  - merkle_root(digests) = "blake3:" + hex(blake3(concat utf-8(d_i)))
    Empty list hashes the empty input.
  - All hashing inputs derived from JSON are RFC 8785 (JCS) canonical bytes.
  - id            = digest(JCS(id_inputs))
  - content_hash  = merkle_root([block.digest for block in manifest.blocks])
  - manifest_hash = digest(JCS(manifest without "manifest_hash" key))
"""

from __future__ import annotations

import io
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


def merkle_root(digest_strings: list[str]) -> str:
    """
    merkle_root over an ordered list of digest strings d[0..n]:
        "blake3:" + hex(blake3(d[0] ++ d[1] ++ ... ++ d[n-1]))
    where each d[i] is hashed as its UTF-8 string bytes, concatenated in order.
    The empty list hashes the empty input.
    """
    h = blake3.blake3()
    for d in digest_strings:
        h.update(d.encode("utf-8"))
    return "blake3:" + h.hexdigest()


def jcs_bytes(obj: Any) -> bytes:
    """RFC 8785 canonicalization (UTF-8 bytes)."""
    return jcs.canonicalize(obj)


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
        manifest = json.loads(manifest_raw)

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
        manifest_for_seal = {k: v for k, v in manifest.items() if k != "manifest_hash"}
        computed_manifest_hash = digest(jcs_bytes(manifest_for_seal))

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
