---
type: issue
state: open
created: 2026-10-17T08:08:26Z
updated: 2026-10-17T08:29:51Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/587
comments: 2
labels: bug, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:50.863Z
---

# [Issue 587]: [bug(core): the JCS seal doesn't bind integers above 2^53 — an edited 64-bit value still verifies](https://github.com/vig-os/tessera/issues/587)

## The defect

The seal's canonical encoding (JCS, RFC 8785, via `serde_jcs`; the reference reader uses Python `jcs`) maps distinct
integers above 2^53 to the **same** canonical bytes, because JCS serialises numbers as IEEE-754 doubles. So a sealed
manifest value above 2^53 isn't bound by `manifest_hash`. It can be edited, and verification still passes.

## Repro (fresh reviewer probe on dev fa581f5, tessera-core)

Seal a manifest whose metadata or generation.config holds `{"seed": 9007199254740993}`, then edit the stored JSON to
`9007199254740992`:

```text
from_json_verified → verify_ok=true seal_unchanged=true
the reader now reports seed = 9007199254740992 (the sealed value was …993)
```

`serde_jcs` and Python `jcs` both canonicalise both values to `9007199254740992`.

## Why it matters

Any caller-supplied sealed JSON can carry a 64-bit integer: `metadata` fields, ADR-0058 `generation.config` (sim seeds
and DAQ counters are routinely 64-bit), and the proposed ADR-0060 `actors[].sampling`. Integrity is the format's core
promise (ADR-0020). A sealed value that can change without changing the seal is a tamper vector, not just a precision loss.

## Fix direction (needs a decision; ADR-0020 amendment)

- Refuse integers outside ±(2^53−1) in sealed JSON at write time (seal/validate), with a typed error telling the
  caller to encode the value as a string. Also refuse at verify, so a crafted file can't use the ambiguity.
- Test: the probe above fails to seal (write side), and a hand-crafted manifest is rejected at verify.
- Check existing producers (ingest, stats) for values that could exceed 2^53 (#554 is the stats-side relative).

Related: #554 (i64 stats above 2^53 lossy for JS readers), #478, ADR-0020, PR #586 (ADR-0060 §2.2's numbers rule).

---

# [Comment #1]() by [gerchowl]()

_Posted on October 17, 2026 at 08:27 AM_

From the ADR-0060 re-review (PR #586): when verify starts refusing integers >2^53, products ALREADY sealed with one (e.g. a 64-bit seed in metadata) will stop verifying. The fix must return a distinct typed error (e.g. `UnboundNumber`/`NonCanonicalNumber`, saying 'this product holds an integer the seal cannot bind') rather than `Integrity` (which tells the user 'tampered'), and say what a holder of such a product should do (re-seal with the value as a string).

---

# [Comment #2]() by [gerchowl]()

_Posted on October 17, 2026 at 08:29 AM_

A migration note from ADR-0060 §2.2 (PR #586, 6944fa3): if the fix refuses integers above 2^53 at **verify**, every product
already sealed with such a value stops verifying. The proposal is to report that as a **distinct** error,
`Error::UnboundInteger { path }` ("the seal doesn't bind this value; it may have changed since sealing"), never
`Error::Integrity`, because the product was sealed honestly and simply isn't covered at that path. `inspect` and the readers
should still open the product and name the path. The release notes for the fix should list it as a behaviour change.


