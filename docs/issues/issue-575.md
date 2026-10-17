---
type: issue
state: open
created: 2026-10-16T21:56:47Z
updated: 2026-10-16T21:56:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/575
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:44.672Z
---

# [Issue 575]: [feat(py): Builder cannot carry a Blob block, so a Python config_ref cannot resolve](https://github.com/vig-os/tessera/issues/575)

## The gap

`Builder.with_generation(config_ref=...)` (added for #562) seals a `generation.config_ref`. That is a
`blake3:` digest of a config block carried in the same `.tsra` (ADR-0058 §2, ADR-0038 Blob). The Python
`Builder` can't produce a reference that resolves:

- It has no way to add a **Blob** block. Only `add_array` and `add_table` exist, so a vendor `.ini`/`.cfg`
  can't be carried bit-faithfully from Python.
- `add_array` / `add_table` return `None`, so the caller can't learn a carried block's digest before
  `pack` either.

So today a Python-set `config_ref` is sealed unchecked (as in Rust, where the check is
`tessera inspect --resolve-config`, `tessera-cli/src/nav.rs:751`). `--resolve-config` then reports
`UnresolvedConfigRef` for it. The `with_generation` docstring says this and steers Python callers to the
inline `config` dict.

## Expected

A Python path to carry a config file verbatim and point `config_ref` at it. One possible shape (not decided):
`Builder.add_blob(name, data, media_type=None) -> str` returning the block digest, mirroring the Rust Blob
block writer. That would close the loop with `with_generation(config_ref=digest)`.

## Related

- #562: the `with_generation` surface this completes.
- ADR-0038 (Blob block), ADR-0058 §2 (generation record).

