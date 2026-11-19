# /// script
# requires-python = ">=3.11"
# dependencies = ["cbor2==6.3.0", "jcs==0.2.1"]
# ///
"""Do Python's JCS and CBOR writers produce the same bytes as the Rust prototype for 200k doubles?
Usage: uv run py/floats.py out/vectors/floats.tsv"""

import struct
import sys

import cbor2
import jcs

n = jcs_ok = cbor_ok = 0
shown = 0
for line in open(sys.argv[1] if len(sys.argv) > 1 else "out/vectors/floats.tsv"):
    bits, rust_jcs, rust_cbor = line.rstrip("\n").split("\t")
    f = struct.unpack(">d", bytes.fromhex(bits))[0]
    n += 1
    pj = jcs.canonicalize(f).decode()
    pc = cbor2.dumps(f, canonical=True).hex()
    jcs_ok += pj == rust_jcs
    cbor_ok += pc == rust_cbor
    if (pj != rust_jcs or pc != rust_cbor) and shown < 5:
        shown += 1
        print(
            f"  differs: {f!r}: JCS rust {rust_jcs} python {pj}; CBOR rust {rust_cbor} python {pc}"
        )
print(f"JCS  (serde_jcs vs Python jcs 0.2.1):                  {jcs_ok}/{n} identical")
print(f"CBOR (profile vs cbor2 6.3.0 canonical=True):          {cbor_ok}/{n} identical")
