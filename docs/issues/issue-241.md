---
type: issue
state: open
created: 2026-06-30T18:55:33Z
updated: 2026-09-28T17:59:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/241
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:01.456Z
---

# [Issue 241]: [research: time-gated / provider-mediated field decryption (KMS handshake) — revocable + audited access (ADR-0040 §3 escalation)](https://github.com/vig-os/tessera/issues/241)

Escalation of ADR-0040 §3 (static `age`-to-recipient field encryption). Instead of the decryption key living *with* the recipient (decrypt forever, offline, no audit), the sensitive-field DEK is **wrapped by a KEK held in an external key provider** (KMS / Vault / key-broker), and decryption requires an **online handshake** the provider gates by policy.

## What this buys (real, valuable)
- **Time-gating** — key releasable only inside a policy window (until date X, business hours, study-active).
- **Revocation** — provider stops releasing the key → future reads denied; a leaked copy that didn't cache plaintext goes dark.
- **Audit** — every authorized decrypt is logged (who/when/purpose) — accountability + breach detection.
- **Crypto-shred at scale** — revoke the KEK → *all* distributed ciphertext copies become permanently undecryptable (the strongest erasure; the consent-withdrawal endgame from ADR-0040 §2.2, now reaching copies off your hosts).
- **Consent-linked** — the provider checks consent status before unwrapping → withdrawal = denial = effective erasure everywhere.

## What it does NOT buy (the honest limit — the user's own question)
**It cannot prevent an *authorized* recipient from exporting/copying the plaintext during a valid read.** Once the DEK is released and the field is decrypted in the recipient's process, they have the cleartext — screenshot, memory dump, re-export all remain (the **"analog hole"**). The provider gates **key release + access over time**, not what happens to plaintext after decryption.
- The next rung that *partially* addresses copying is **TEE / confidential computing**: decrypt only inside an attested enclave that enforces output policy — heavyweight, and even then a screen photo defeats it. Diminishing returns.

## The confidentiality ladder (frame for ADR-0040)
1. **Static `age`-to-recipient** (§3) — offline, simplest; "coded/anon by default if leaked", no revocation/audit.
2. **Provider-mediated (this issue)** — online, **revocable + time-gated + audited**, crypto-shred-at-scale. Defends: leaked copies, revoked recipients, consent withdrawal. Does NOT defend: authorized-recipient copying.
3. **TEE / enclave** — partial copy-defense at high cost; still analog-hole-bound.

## Scope
Research/design now (a rung on ADR-0040's ladder), not implementation. Pins the honest threat model so we don't oversell "encrypted" as "copy-proof". Key-provider shape (KMS vs Vault vs a Tessera key-broker) + the decrypt-as-a-service vs unwrap-DEK choice are open.
