# Tessera reference reader (independent, pure-Python) — the v1.0 conformance gate

This is a **second, independent** implementation of the Tessera read path, written in pure Python
**from `tessera/docs/SPEC.md` alone** — no access to the Rust engine. It exists to prove the spec is
sufficient: a fresh implementer, given only the spec, must be able to open every `.tsra` test vector
and reproduce its golden `id` / `content_hash` / `manifest_hash` (and verify each block's digest).

This is the **v1.0 release gate** (ROADMAP P8 / #211). It was produced by a spec-only agent and
reproduced all 6 corpus goldens on the first attempt; the clarifications it surfaced are folded back
into `SPEC.md`.

## What it depends on
Only the container (§6) + canonical hashing (§2–§4): Python stdlib `zipfile` + `json`, plus `blake3`
and an RFC 8785 `jcs` encoder. It does **not** decode array/table payloads — reproducing the identity
triple needs only the stored bytes, not the pcodec/Vortex codecs.

## Run it
```sh
python -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python run.py     # prints a PASS/FAIL table over corpus/files/*.tsra
```
Exit code 0 ⇔ all fixtures reproduce their goldens.

## Why it matters
Until a second codebase reads Tessera correctly from the spec alone, "the spec" is really "whatever
the Rust does." This reader closes that gap — and re-running it after any spec or corpus change keeps
the spec honest.
