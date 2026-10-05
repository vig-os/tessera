#!/usr/bin/env python3
"""Generate `implicit_vr_nested.dcm` — the #551 golden fixture. **No dependencies, not even pydicom.**

Why bytes by hand rather than pydicom (or dicom-rs itself):

An **implicit-VR** data set does not store VRs at all — every element is `tag || length || value`, and the
reader recovers the VR from its own tag dictionary. That is precisely what #551 is about: dicom-rs 0.10
shipped a new dictionary (dicom-rs #790), so the `"vr"` strings in our recorded header JSON are a function
of the *reader*, not of the file.

A fixture written by any DICOM library therefore must not be allowed to bake that library's dictionary into
the expected answer. Writing the bytes directly keeps the file a fixed, library-independent input, so the
A/B across 0.9.1 -> 0.10.0 measures the reader and nothing else. It also means the fixture cannot drift when
the writer is upgraded, and it needs no package that this hermetic shell does not have.

Deterministic: no timestamps, no UIDs generated at run time, no randomness. Re-running must produce a
byte-identical file, which `dicom_implicit_vr_fixture_is_byte_stable` checks by regenerating and comparing.

Synthetic and PHI-free by construction: there is no PatientName, PatientID, or any other PS3.15 identifying
tag — a 4x4 gradient and structural metadata only. Nothing here derives from a real study.

    python3 gen_implicit_vr_dicom.py [OUT]     # default: ./implicit_vr_nested.dcm
"""

import struct
import sys
from pathlib import Path

IMPLICIT_VR_LE = "1.2.840.10008.1.2"
CT_IMAGE_STORAGE = "1.2.840.10008.5.1.4.1.1.2"

ITEM = (0xFFFE, 0xE000)
ITEM_DELIM = (0xFFFE, 0xE00D)
SEQ_DELIM = (0xFFFE, 0xE0DD)
UNDEFINED = 0xFFFFFFFF


def pad_even(b: bytes, pad: bytes = b"\x00") -> bytes:
    """DICOM values occupy an even number of bytes; UIDs pad with NUL, text with space."""
    return b if len(b) % 2 == 0 else b + pad


def ui(s: str) -> bytes:
    return pad_even(s.encode("ascii"), b"\x00")


def txt(s: str) -> bytes:
    """CS / DA / TM / DS / IS / LO — all padded with a space."""
    return pad_even(s.encode("ascii"), b"\x20")


def us(*vals: int) -> bytes:
    return struct.pack(f"<{len(vals)}H", *vals)


def implicit(tag: tuple, value: bytes) -> bytes:
    """One implicit-VR LE element: group, element, 32-bit length, value. **No VR on the wire.**"""
    return struct.pack("<HHI", tag[0], tag[1], len(value)) + value


def explicit(tag: tuple, vr: str, value: bytes) -> bytes:
    """One explicit-VR LE element — only the file-meta group, which PS3.10 requires be explicit."""
    if vr in ("OB", "OW", "OF", "SQ", "UT", "UN"):
        return struct.pack("<HH2sHI", tag[0], tag[1], vr.encode(), 0, len(value)) + value
    return struct.pack("<HH2sH", tag[0], tag[1], vr.encode(), len(value)) + value


def item(body: bytes) -> bytes:
    """A sequence item with an EXPLICIT length — the simpler of the two encodings."""
    return struct.pack("<HHI", ITEM[0], ITEM[1], len(body)) + body


def item_undefined(body: bytes) -> bytes:
    """A sequence item with an UNDEFINED length, closed by an item-delimiter."""
    return (
        struct.pack("<HHI", ITEM[0], ITEM[1], UNDEFINED)
        + body
        + struct.pack("<HHI", ITEM_DELIM[0], ITEM_DELIM[1], 0)
    )


def build_dataset() -> bytes:
    # The INNER sequence, explicit length. Nested one level inside the outer item.
    inner_item = item(
        implicit((0x0008, 0x1150), ui(CT_IMAGE_STORAGE))
        + implicit((0x0008, 0x1155), ui("1.2.826.0.1.3680043.9.7.551.2"))
    )
    inner_seq = implicit((0x0008, 0x1140), inner_item)

    # The OUTER sequence, UNDEFINED length with an undefined-length item, so the fixture exercises both
    # encodings and the delimiter handling that dicom-rs #774's non-recursive reader rewrote.
    outer_body = implicit((0x0020, 0x000E), ui("1.2.826.0.1.3680043.9.7.551.3")) + inner_seq
    outer_seq = (
        struct.pack("<HHI", 0x0008, 0x1115, UNDEFINED)
        + item_undefined(outer_body)
        + struct.pack("<HHI", SEQ_DELIM[0], SEQ_DELIM[1], 0)
    )

    rows, cols = 4, 4
    pixels = bytes((r * cols + c) * 16 % 256 for r in range(rows) for c in range(cols))

    # Ascending tag order, as a conforming data set requires.
    return b"".join(
        [
            implicit((0x0008, 0x0008), txt("DERIVED\\SECONDARY")),
            implicit((0x0008, 0x0016), ui(CT_IMAGE_STORAGE)),
            implicit((0x0008, 0x0018), ui("1.2.826.0.1.3680043.9.7.551.1")),
            implicit((0x0008, 0x0020), txt("20240101")),
            implicit((0x0008, 0x0030), txt("120000")),
            implicit((0x0008, 0x0060), txt("CT")),
            outer_seq,
            implicit((0x0018, 0x0050), txt("1.0")),
            implicit((0x0020, 0x0013), txt("1")),
            implicit((0x0028, 0x0002), us(1)),
            implicit((0x0028, 0x0004), txt("MONOCHROME2")),
            implicit((0x0028, 0x0010), us(rows)),
            implicit((0x0028, 0x0011), us(cols)),
            implicit((0x0028, 0x0100), us(8)),
            implicit((0x0028, 0x0101), us(8)),
            implicit((0x0028, 0x0102), us(7)),
            implicit((0x0028, 0x0103), us(0)),
            implicit((0x7FE0, 0x0010), pad_even(pixels)),
        ]
    )


def build_file_meta() -> bytes:
    """The (0002,…) group: explicit VR LE, with (0002,0000) carrying the length of the rest."""
    body = b"".join(
        [
            explicit((0x0002, 0x0001), "OB", b"\x00\x01"),
            explicit((0x0002, 0x0002), "UI", ui(CT_IMAGE_STORAGE)),
            explicit((0x0002, 0x0003), "UI", ui("1.2.826.0.1.3680043.9.7.551.1")),
            explicit((0x0002, 0x0010), "UI", ui(IMPLICIT_VR_LE)),
            explicit((0x0002, 0x0012), "UI", ui("1.2.826.0.1.3680043.9.7.551.0")),
        ]
    )
    return explicit((0x0002, 0x0000), "UL", struct.pack("<I", len(body))) + body


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "implicit_vr_nested.dcm")
    out.write_bytes(b"\x00" * 128 + b"DICM" + build_file_meta() + build_dataset())
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
