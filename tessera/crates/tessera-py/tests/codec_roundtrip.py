"""Array-codec round-trip through the public API (#533).

Proves three things the `codec` argument on `Builder.add_array` promises:

1. every accepted codec round-trips bit-exact through `Reader.array`, and a partial read
   (`array_roi`) works whichever was used — all three are per-chunk codecs;
2. the manifest records the CONCRETE codec: `"auto"` resolves at write time to `"pcodec"` or
   `"zstd"`, and a reader never sees `"auto"`;
3. the DEFAULT is unchanged. `DEFAULT_CONTENT_HASH` was produced by the binding *before* the
   argument existed (dev @ c674ef1) and by this one, and the two were byte-identical. A default
   `add_array` call must keep producing exactly it; if this moves, the default codec changed,
   which is a format decision rather than an API tweak.
"""

from __future__ import annotations

import os
import sys
import tempfile

import numpy as np

import tessera

DEFAULT_CONTENT_HASH = (
    "blake3:10713a7b8b439ab769a41cb11d6e54d479df49b580a4894fc78942afa03d4777"
)
TS = "2024-01-01T00:00:00Z"


def _pack(vol, *codec):
    b = tessera.Builder("recon", "tessera-py", "codec default pin", TS)
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes(), *codec)
    p = os.path.join(tempfile.mkdtemp(), "x.tsra")
    b.pack(p)
    return tessera.open(p)


def _recorded(r) -> str:
    return next(b for b in r.manifest()["blocks"] if b["name"] == "volume")["spec"][
        "codec"
    ]


def main() -> int:
    errors: list[str] = []
    pin = (
        (np.arange(4096, dtype=np.int64) % 900 - 400).astype("<i2").reshape(16, 16, 16)
    )

    # 3. default unchanged, and equal to an explicit "pcodec"
    got = _pack(pin).manifest()["content_hash"]
    if got != DEFAULT_CONTENT_HASH:
        errors.append(
            f"default add_array content_hash moved: {got} != {DEFAULT_CONTENT_HASH}"
        )
    explicit = _pack(pin, "pcodec").manifest()["content_hash"]
    if explicit != got:
        errors.append(
            "add_array() and add_array(codec='pcodec') differ — default is not pcodec"
        )

    # 1 + 2. round-trip and concrete manifest codec, on data where each codec should win
    ramp = (
        np.arange(32, dtype=np.int64)[:, None, None] * 8
        + np.arange(32, dtype=np.int64)[None, :, None] * 2
        + np.zeros((1, 1, 32), dtype=np.int64)
    ).astype("<i2")
    rng = np.random.default_rng(7)
    noisy = np.clip(rng.normal(40, 25, size=(32, 32, 32)), -1024, 3071).astype("<i2")
    for label, vol in (("ramp", ramp), ("noisy", noisy)):
        for codec in ("pcodec", "zstd", "auto"):
            r = _pack(vol, codec)
            if not np.array_equal(r.array("volume"), vol):
                errors.append(f"{label}/{codec}: full read not bit-exact")
            roi = r.array_roi("volume", [3, 5, 7], [4, 6, 8])
            if not np.array_equal(roi, vol[3:7, 5:11, 7:15]):
                errors.append(f"{label}/{codec}: array_roi not bit-exact")
            rec = _recorded(r)
            if codec == "auto":
                if rec not in ("pcodec", "zstd"):
                    errors.append(
                        f"{label}/auto: manifest records {rec!r}, not a concrete codec"
                    )
            elif rec != codec:
                errors.append(f"{label}/{codec}: manifest records {rec!r}")

    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    if not errors:
        print(
            "codec_roundtrip OK: pcodec/zstd/auto bit-exact (full + ROI), concrete codec "
            "recorded, default content_hash pinned and unchanged"
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
