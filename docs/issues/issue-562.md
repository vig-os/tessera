---
type: issue
state: closed
created: 2026-10-16T16:50:27Z
updated: 2026-10-25T14:43:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/562
comments: 2
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:56.587Z
---

# [Issue 562]: [feat(py): expose with_producer / with_generation on Builder — ADR-0058 §4 is unreachable from Python](https://github.com/vig-os/tessera/issues/562)

## The gap

`tessera-py`'s `Builder` has no Python-side surface for `with_producer` / `with_generation`,
even though the Rust `ProductBuilder` exposes both and ADR-0058 §4 explicitly says
"The write API requires the caller to supply `Producer` + `Generation` when authoring a
block, so a DAQ/SIM **cannot seal an anonymous product**." A Python caller can seal exactly
that anonymous product today.

The current Builder pymethods (observed through `dir(tessera.Builder)` on the wheel):

```
['add_array', 'add_source', 'add_table', 'pack', 'set_field']
```

`set_field(key, value_json)` only populates `Manifest.metadata` — not `producer` or
`generation` (confirmed: product.rs:`with_field` writes into `self.manifest.metadata`).
`docs/book/src/bindings.md` matches: it names the five pymethods above and no others.

## Why it matters — a concrete adoption case

anatomist (vet-CT pipeline, see MorePET/anatomist#86) is evaluating `.tsra` as its agentic
provenance format. Its mapping needs:

- `producer` ← the orchestrator (anatomist/0.1.0, `git_commit`, `nar_hash`-style identity),
  not the stock `tessera/0.0.0` tessera stamps itself with when Python authors the product.
- `generation.config` ← step parameters, the template `{version, git_commit}`, the agent bag
  `{harness, model, effort, brief_sha, …}` — the recipe the owner wants sealed.

With no Python surface for either, the current workaround is to stuff everything into
`metadata.*` under ad-hoc keys. That has three problems:

1. The product's `producer` is wrong (always `tessera/0.0.0`, pointing at the packager,
   which is the exact anti-pattern ADR-0058 §Context's #2 names).
2. `generation` stays `None`, so a schema with `requires_generation = true` (ADR-0058 §3)
   is unenforceable against Python-authored products.
3. `tessera inspect` does not print `metadata` keys (post-#417 it prints `generation`
   now, but the metadata workaround is still invisible to the operator surface).

## Repro — hermetic, from the Nix wheel

```
# tessera dev sha fa581f56041b38a48a8d20f4a9d033505dad8185
nix build github:vig-os/tessera/<sha>#wheel
nix shell nixpkgs#python312.withPackages.{numpy,pip} --command bash -c '
  mkdir -p /tmp/s && pip install --no-index --target=/tmp/s result/tessera-0.1.0a1-cp39-abi3-linux_x86_64.whl
  PYTHONPATH=/tmp/s LD_LIBRARY_PATH=$(nix eval --impure --raw --expr "(import <nixpkgs> {}).stdenv.cc.cc.lib + \"/lib\"") \
    python3 -c "
import tessera
b = tessera.Builder(\"recon\", \"demo\", \"\", \"2024-01-01T00:00:00Z\")
# Try to set the sealed Producer / Generation ADR-0058 defines.
for attr in (\"with_producer\", \"with_generation\", \"set_producer\", \"set_generation\"):
    print(attr, \"->\", hasattr(b, attr))     # all False
"
'
```

Writing a product and inspecting it:

```
producer      tessera/0.0.0            # stock stamp, not the real generator
generation    None                     # ADR-0058 §2 recipe is unauthored
metadata      {step, parameters, …}    # the workaround — invisible to inspect
```

Environment: nix 2.34.8 on NixOS 6.18.55; python 3.12.15; tessera `0.1.0-alpha.1` built from
the above sha; `TESSERA_VERSION` still "0.0.0".

## Expected

Builder gains two pymethods and (for ergonomics) one keyword path:

```python
b.with_producer(tool="anatomist", version="0.1.0", git_commit="…", git_repo="MorePET/anatomist", dirty=False)
b.with_generation(config={"step": "trees", "template_version": "canine/v1", "agent": {"model": "…"}})
# or (preferred for Python):
b.with_generation(config_ref="blake3:…")  # when a config block is carried
```

The simplest shape matches Rust: `Producer{tool, version, git_commit=None, git_repo=None, dirty=None}`
and `Generation{config: dict | None, config_ref: str | None}`. Validation that `generation` is
non-empty when the schema sets `requires_generation` already lives in `ProductSchema::validate` —
the Python path only needs to pass the structs through.

## Proposed shape

Add to `tessera-py/src/lib.rs`:

```rust
#[pyo3(signature = (tool, version, git_commit=None, git_repo=None, dirty=None))]
fn with_producer(&mut self, tool: &str, version: &str, git_commit: Option<&str>,
                 git_repo: Option<&str>, dirty: Option<bool>) -> PyResult<()> {
    self.builder()?.with_producer(tessera_core::provenance::Producer {
        tool: tool.into(), version: version.into(),
        git_commit: git_commit.map(Into::into), git_repo: git_repo.map(Into::into), dirty,
    });
    Ok(())
}

#[pyo3(signature = (config=None, config_ref=None))]
fn with_generation(&mut self, config: Option<&PyDict>, config_ref: Option<&str>) -> PyResult<()> {
    let mut g = tessera_core::provenance::Generation::default();
    if let Some(c) = config { /* serde_json via pythonize or json.dumps round-trip */ }
    if let Some(r) = config_ref { g = g.with_config_ref(r); }
    self.builder()?.with_generation(g);
    Ok(())
}
```

Add a tests/write_example.py anchor that calls both — the existing anchors are the only
canonical write-side doc, and `bindings.md` is `{{#include}}`ing them, so until an anchor exists
the shape stays undocumented.

## Related

- #533 — same pattern (Rust method hidden from Python), different method.
- #417 — same ADR-0058 ground, CLI side.
- #210 — the P7 bindings epic this sits under.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 11:56 AM_

Fixed on dev by #585 (merge eb12822): `Builder.with_producer(tool, version, git_commit=None, git_repo=None, dirty=None)` and `Builder.with_generation(config=None, config_ref=None)` expose ADR-0058 from Python. Ships in 0.1.0-alpha.2.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 25, 2026 at 02:43 PM_

Correction to the closing comment: the alpha.2 cut failed on macOS (#647), so the tag `v0.1.0-alpha.2` was never published as a release. This change ships in **[v0.1.0-alpha.3](https://github.com/vig-os/tessera/releases/tag/v0.1.0-alpha.3)** (tag at 9424f90, which contains the #585 merge eb12822).

