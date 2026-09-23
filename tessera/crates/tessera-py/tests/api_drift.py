"""Docstring-vs-behaviour drift gate for the dtype-code surface (#412).

Issue #412's minor finding: `add_table` accepts `i1/u1/b1/str` beyond `add_array`'s
`i2..f8`, but the docstrings didn't say so — and a real `uint8` column was skipped as
"unsupported" on the strength of the docs. This test makes that drift impossible in
either direction, with **zero duplicated source of truth**:

- the *behaviour* side is probed empirically — every candidate code is fed to the live
  built module and classified accepted/rejected by what actually happens;
- the *docs* side is parsed out of the very `__doc__` strings a user reads.

If an implementation change adds or drops a code without touching the docstring (or
vice versa), the sets diverge and this fails. Runs in the `tessera-py-import` /
`tessera-wheel-import` flake checks right after smoke.py, against the same package.
"""

import re
import sys

import numpy as np
import tessera

# Superset of plausible numpy-style codes: everything either path accepts today, plus
# codes that must stay rejected (i16, c8 — complex is tracked as its own issue) so the
# probe demonstrably exercises the rejection path too. A newly *accepted* code outside
# this list still fails the gate: it would appear in the docstring (else undocumented
# elsewhere too) and trip the "documented but not probed" assertion below.
CANDIDATES = [
    "i1",
    "i2",
    "i4",
    "i8",
    "u1",
    "u2",
    "u4",
    "u8",
    "f2",
    "f4",
    "f8",
    "b1",
    "str",
    "i16",
    "c8",
]

# One row of valid little-endian payload per code (str = [u32 LE len | bytes]*).
# i16/c8 get literal bytes: numpy has no 16-byte int, and the exact bytes don't matter
# for must-stay-rejected codes — rejection happens on the code, not the payload.
PAYLOAD = {
    code: np.zeros(1, dtype="<" + code).tobytes()
    for code in CANDIDATES
    if code not in ("str", "c8", "b1", "i16")
}
PAYLOAD["b1"] = b"\x01"
PAYLOAD["str"] = (3).to_bytes(4, "little") + b"abc"
PAYLOAD["c8"] = bytes(8)
PAYLOAD["i16"] = bytes(16)

# Matches exactly the code tokens the docstrings advertise (i1..i8, u1..u8, f2..f8,
# b1, str). Deliberately narrow: prose like "u32 LE len" or ">=16-bit" cannot match.
CODE_TOKEN = re.compile(r"\b([iuf][1248]|b1|str)\b")


def documented(doc: str) -> set[str]:
    return set(CODE_TOKEN.findall(doc))


def probe(add) -> set[str]:
    accepted = set()
    for code in CANDIDATES:
        try:
            add(code, PAYLOAD[code])
            accepted.add(code)
        except Exception:
            pass
    return accepted


def check(name: str, accepted: set[str], doc: str) -> list[str]:
    advertised = documented(doc)
    errors = []
    if hidden := accepted - advertised:
        errors.append(
            f"{name}: accepted but NOT documented: {sorted(hidden)} — the #412 drift, update the docstring"
        )
    if broken := advertised - accepted:
        errors.append(
            f"{name}: documented but REJECTED: {sorted(broken)} — the docstring overpromises"
        )
    if unprobed := advertised - set(CANDIDATES):
        errors.append(
            f"{name}: documented but not in CANDIDATES: {sorted(unprobed)} — extend the probe list"
        )
    return errors


def main() -> int:
    table = probe(
        lambda code, data: tessera.Builder(
            "t", "x", "", "2026-01-01T00:00:00Z"
        ).add_table("t", [("c", code, data)], None)
    )
    array = probe(
        lambda code, data: tessera.Builder(
            "t", "x", "", "2026-01-01T00:00:00Z"
        ).add_array("a", code, [1], data)
    )

    errors = check("add_table", table, tessera.Builder.add_table.__doc__ or "")
    errors += check("add_array", array, tessera.Builder.add_array.__doc__ or "")

    # The cross-references the docs sell must themselves hold: str is table-only, f2 is
    # array-only (no native half-float in the columnar toolchain), everything else is shared.
    if extra := (array - table) - {"f2"}:
        errors.append(
            f"add_array accepts codes add_table rejects (beyond the documented f2): {sorted(extra)}"
        )
    if "f2" in table:
        errors.append(
            "add_table accepts f2 — the docstrings document it as array-only; update both"
        )
    if "str" in array:
        errors.append(
            "add_array accepts str — the docstrings document it as table-only; update both"
        )

    for e in errors:
        print(f"DRIFT: {e}", file=sys.stderr)
    if not errors:
        print(
            f"api_drift OK: add_table={sorted(table)} add_array={sorted(array)} match their docstrings"
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
