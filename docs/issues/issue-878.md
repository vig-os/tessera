---
type: issue
state: open
created: 2026-11-15T21:30:13Z
updated: 2026-11-15T21:30:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/878
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:35.865Z
---

# [Issue 878]: [follow-up(#573): Python manifest_json and tessera import drop sealed newer fields; reference reader gaps](https://github.com/vig-os/tessera/issues/878)

Nits from the adversarial review of PR #875 (head 8470752, merged). None of them is a seal bypass.

1. **Python drops sealed keys.** `Reader.manifest_json()` (`tessera-py/src/lib.rs:74`) returns the parsed struct, not the stored bytes, so a product with newer fields silently loses sealed keys. Same class as #874.
2. **`tessera import` refuses valid products.** The path is `version.rs:27` → `repo.put_manifest` → `to_sealed_json`. It cannot import a valid product that has newer fields, `"study": null`, or no `sources`. SPEC §4 says copying the bytes unchanged is always allowed, so store the stored bytes.
3. **The two readers still disagree on four inputs.** Rust refuses and Python accepts: a BOM prefix, `"metadata": null`, `"sources": null`, and nesting of 126 levels or more. A Rust writer can also emit nesting its own reader refuses (#589). Decide the rule once in SPEC §4 and make both readers follow it.
4. **The reference reader crashes on huge integers.** A 401-digit integer raises `OverflowError` instead of returning "refused". Its I-JSON check also misses integers outside a double's range.

Refs: #573
