---
type: issue
state: open
created: 2026-10-17T08:11:43Z
updated: 2026-10-18T00:38:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/589
comments: 2
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:50.209Z
---

# [Issue 589]: [bug: a manifest nested deeper than the reader's parse limit seals but can't be opened](https://github.com/vig-os/tessera/issues/589)

## The defect

A writer can seal a manifest nested deeper than any tessera reader will parse. The product is sealed and
written, and then `tessera.open`, `tessera verify`, and every other read path fail on it. The writer gets no
error, so it's silent data loss at write time.

The cause is serde_json's default recursion limit (128) on the **read** side (manifest parse) with no matching
limit on the **write** side. Any free-form JSON slot reaches it:

- Rust: `ProductBuilder::with_field`, `with_extra`, `with_generation` (`Generation.config`), and ingest specs.
- Python: `Builder.set_field` (on dev today) and `Builder.with_generation(config=...)` (#562, PR #585).

## Repro (Python, PR #585's binding; `set_field` on dev reproduces it the same way)

```python
import numpy as np, tessera
for depth in (125, 126):
    cfg, cur = {}, None
    cur = cfg
    for _ in range(depth - 1):
        cur["n"] = {}; cur = cur["n"]
    b = tessera.Builder("t_product", "d", "", "2024-01-01T00:00:00Z")
    b.add_array("v", "i2", [1], np.zeros(1, "<i2").tobytes())
    b.with_generation(config=cfg)
    b.pack(f"d{depth}.tsra")          # succeeds for both
    tessera.open(f"d{depth}.tsra")    # 125 opens; 126 raises
```

```text
126 SEALED BUT UNREADABLE: TesseraError serialize: recursion limit exceeded at line 143 column 260
$ tessera verify d126.tsra
error: serialize: recursion limit exceeded at line 143 column 260
```

The threshold (125 levels of `config`) is 128 minus the manifest's own nesting above `generation.config`.

## Expected

Whatever is sealed can be opened. This is a format decision, so it isn't fixed here. Options:

1. **Writer-side guard in `tessera-core`.** `seal()` rejects a manifest whose depth exceeds the reader's parse
   limit, with a typed error naming the slot. The limit becomes a documented format constant (SPEC), and every
   writer (Rust, Python, ingest) inherits it at once.
2. **Reader-side lift.** Parse with serde_json's `unbounded_depth` + `serde_stacker`. That's a new dependency and
   feature, and it moves the denial-of-service bound somewhere else, which an untrusted-input reader needs anyway.

Option 1 fixes it at the root and leaves the reader's protection against hostile input in place.

## Related

- #562 / PR #585: surfaced while adding the Python `with_generation` recursion guard. That PR stops the
  interpreter crash on cycles and deep nesting, but deliberately doesn't duplicate this limit in the binding.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 17, 2026 at 08:26 AM_

More evidence, from PR #585's fix round: the **write** path has the matching native-recursion hazard.

PR #585's `config` converter is now iterative, so the conversion itself never recurses. The converted
`serde_json::Value` is a different matter. Its `Drop`, and serde_json's serializer at seal, recurse on the native stack.
Measured with a debug build of `tessera-py`:

| depth | thread stack | recursion limit | result |
| --- | --- | --- | --- |
| 1000 | default (8 MiB) | default | converts, seals, drops: OK |
| 1000 | 256 KiB | default | converts, then SIGSEGV when the builder is dropped |
| 20 000 | default | raised to 1e6 | OK |
| 200 000 | default | raised to 1e6 | converts, then SIGSEGV when the builder is dropped |

With default settings no writer can trigger it, but it's another reason for the writer-side depth bound (option 1). A
bound near the reader's limit (about 128) keeps every recursive pass (drop, serialize, JCS, parse) shallow on any stack.


---

# [Comment #2]() by [gerchowl]()

_Posted on October 18, 2026 at 12:38 AM_

Update from PR #585 (head c7faab5): Python `Builder.with_generation` now caps `config` at a fixed **125 levels**. That's
exactly the reader's limit: serde_json parses at most 127 nested levels, and `config` sits under `manifest` →
`generation`. Deeper raises `TesseraError` instead of sealing an unopenable product, whatever `sys.getrecursionlimit()`
is set to. The bound also keeps the recursive Drop/serialize hazard from my earlier comment shallow on that path.

**Still open, so this issue stays open:** the other writers reach it. Measured on the same build with Python `set_field`:

```text
set_field depth 125 -> opens
set_field depth 126 -> SEALED BUT UNREADABLE: serialize: recursion limit exceeded at line 164 column 260
set_field depth 127 -> SEALED BUT UNREADABLE: serialize: recursion limit exceeded at line 164 column 260
```

(`set_field`'s own JSON parse accepts 127 levels, and the value lands one level below the root under `metadata`.)
Rust `with_field` / `with_extra` / `with_generation` and ingest specs have no bound at all. The root fix is still
option 1, a writer-side depth bound in `tessera-core::seal()`. Once that lands, the binding's constant should be derived
from the core's.


