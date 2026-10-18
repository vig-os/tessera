---
type: issue
state: open
created: 2026-10-17T08:13:05Z
updated: 2026-10-17T08:13:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/590
comments: 0
labels: bug, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:49.871Z
---

# [Issue 590]: [bug(cli): verify-sig --require-signer checks the signer-chosen label, not the trust store's pinned identity](https://github.com/vig-os/tessera/issues/590)

## The defect

`tessera verify-sig --require-signer <id>` checks the envelope's `signer` label, which the signer picks
(`tessera-cli/src/trust.rs:322-328`: `env.signer.as_deref() != Some(req)`). It never looks at the identity the
operator **pinned** for the verifying key when they trusted it: `tessera trust add --signer` writes
`<name>.signer` (`trust.rs:176`), and `entries()` loads it into `TrustEntry.signer` (`trust.rs:156`, `trust.rs:207`).
So any key in the trust store can sign as `signer = <someone else's ORCID>` and pass `--require-signer <that ORCID>`.

The signature does bind `signer` (ADR-0037's envelope), so the label can't be changed *after* signing. But the person
who chooses it is the key holder, and `--require-signer` is a policy about who the key holder is.

## Expected

- With a trust-store key, `--require-signer X` passes only if the matched entry's pinned identity is `X`. It refuses
  when the entry has no pinned identity, and also when the envelope `signer` differs from the pin (a mislabel).
- With `--pubkey` (no trust entry, `trust.rs:283-287`), nothing is pinned, so `--require-signer` refuses with a typed
  error asking for a trust-store entry.

## Test

Trust key A pinned as `https://orcid.org/A`, then sign with A using `signer = https://orcid.org/B`:
`--require-signer https://orcid.org/B` must fail, and so must `--require-signer https://orcid.org/A` (label/pin
mismatch). A key with no pin must fail any `--require-signer`.

Found while designing ADR-0060 §6.4 (PR #586), whose P3b check uses the pinned identity, not the label.

