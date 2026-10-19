---
type: issue
state: closed
created: 2026-10-16T16:50:32Z
updated: 2026-10-18T11:56:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/563
comments: 1
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:18:05.260Z
---

# [Issue 563]: [feat(py): Builder.add_source cannot pin content_hash — Python-written edges fail --require-complete](https://github.com/vig-os/tessera/issues/563)

## The gap

`tessera-py`'s `Builder.add_source` cannot pin the parent's `content_hash`. The Rust API
has it (`Source::new(role, reference).with_content_hash(hash)`), but the pyo3 wrapper
is:

```rust
fn add_source(&mut self, relation: &str, reference: &str) -> PyResult<()> {
    self.builder()?.add_source(tessera_core::provenance::Source::new(relation, reference));
    Ok(())
}
```

Two positional strings, no kwargs. So every edge Python writes serialises with no
`content_hash` field, and the chain cannot close.

## Why it matters — the integrity chain breaks on Python-written edges

Writing a parent product + a derived child from Python, then walking the chain with the
Rust CLI, produces:

```
recon · frame-fictus-20260101  [blake3:15d5ea8e2db9…]
  ↳ derived_from table · anat-smoke-fictus-20260101  [no pinned version]
      file      /tmp/dogfood-outs/smoke.tsra
      producer  tessera/0.0.0
    ↳ ingested_from dataset://…                       [external leaf]

2 hop(s) · 1 external leaf/leaves · 1 gap(s) — chain INCOMPLETE
```

`tessera provenance --require-complete` then exits with a typed error:

```
error: invalid product: provenance chain for /tmp/dogfood-outs/frame.tsra is incomplete
       (--require-complete): some parents resolved to a different version or were not found.
```

The Python caller cannot avoid this — passing the parent's `manifest_hash` as the second
arg puts it in `reference`, not `content_hash`, so the walk still reports "no pinned
version" (reference is the lineage handle; the pin is the orthogonal field).

docs/book/src/provenance-signing.md states the chain contract: "Because each edge commits
to the parent's seal, chain verification walks from a product back to its root, checking
every hop." Python-written edges cannot comply.

## Repro — hermetic, from the Nix wheel

```
# tessera dev sha fa581f56041b38a48a8d20f4a9d033505dad8185
nix build github:vig-os/tessera/<sha>#wheel
# then, after installing the wheel into a venv with numpy:
python3 - <<'PY'
import tessera, numpy as np, pathlib, tempfile
out = pathlib.Path(tempfile.mkdtemp())
pb = tessera.Builder("recon", "parent", "", "2024-01-01T00:00:00Z")
pb.add_array("volume", "i2", [2,2,2], np.zeros(8, "<i2").tobytes())
pid = pb.pack(str(out/"p.tsra"))

p = tessera.open(out/"p.tsra").manifest()
cb = tessera.Builder("recon", "child", "", "2024-01-01T00:00:01Z")
cb.add_array("volume", "i2", [2,2,2], np.ones(8, "<i2").tobytes())
# No way to pass content_hash:
cb.add_source("derived_from", p["manifest_hash"])    # second arg is "reference", not pin
cb.pack(str(out/"c.tsra"))

# The edge Python wrote has no content_hash:
print(tessera.open(out/"c.tsra").manifest()["sources"])
# -> [{'role': 'derived_from', 'reference': 'blake3:…'}]   # no 'content_hash' key
PY
```

Then `tessera provenance --require-complete <child>.tsra` fails with the output above.

Environment: nix 2.34.8 on NixOS 6.18.55; python 3.12.15; tessera `0.1.0-alpha.1`.

## Proposed shape

```rust
#[pyo3(signature = (relation, reference, content_hash=None))]
fn add_source(&mut self, relation: &str, reference: &str, content_hash: Option<&str>) -> PyResult<()> {
    let mut s = tessera_core::provenance::Source::new(relation, reference);
    if let Some(h) = content_hash {
        s = s.with_content_hash(h);
    }
    self.builder()?.add_source(s);
    Ok(())
}
```

Default stays `None` for back-compat — existing calls are unaffected.

## Related

- #533 — Rust method hidden from Python, same pattern.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 11:56 AM_

Fixed on dev by #585 (merge eb12822): `Builder.add_source(relation, reference, content_hash=None)` pins the parent's content hash, so Python-written `derived_from` chains pass `provenance --require-complete`. Ships in 0.1.0-alpha.2.

