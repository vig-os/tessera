---
type: issue
state: open
created: 2026-10-16T22:45:09Z
updated: 2026-10-16T22:45:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/577
comments: 0
labels: feature, security
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:43.448Z
---

# [Issue 577]: [feat(cli): verify-sig --keyring — a pinned key list with validity windows, for rotation without a trust store](https://github.com/vig-os/tessera/issues/577)

## Context

Dogfooding from MorePET/anatomist#86 (ADR-0007, PR MorePET/anatomist#103) at tessera `dev` `fa581f5`. Related to #576.

An automated verifier should pin exactly which keys it trusts, and not inherit a store from `$HOME` or the cwd (#576). Today the only pinned form is `verify-sig --pubkey <one key file> [--require-signer <id>]`. Key rotation breaks that: once the operator rotates its signing key, products signed with the old key fail against the new pinned key, and products signed with the new key fail against the old one. Each consumer then has to write its own selection logic: read `aux/signatures/<key_id>.sig.json`, take `key_id` and `signed_at`, pick the matching key from its own list, and call `--pubkey`.

## Ask

A pinned **keyring** option, e.g. `verify-sig --keyring <file>`, where the file is a committed, reviewable list:

```toml
[[key]]
key_id = "<hex>"
signer = "anatomist-operator"
valid_from = "2026-11-01"
valid_until = "2027-11-01"   # optional; absent = still valid
```

Semantics:

- Only the listed keys are consulted. No user store, no cwd store, fail-closed.
- The signature's `key_id` selects the entry.
- Its `signer` must equal the entry's `signer`.
- `signed_at` must fall inside the entry's window. That makes a signature with a revoked key, dated after revocation, fail. `signed_at` is the signer's claim, so document it as such.
- Exit nonzero naming the rule that failed (unknown key, signer mismatch, outside window, bad signature).

That gives rotation without trust-store state, and composes with #576's fix.

## Linked

#576, ADR-0037, MorePET/anatomist#86.

