---
type: issue
state: closed
created: 2026-10-16T22:38:04Z
updated: 2026-10-25T14:38:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/576
comments: 1
labels: bug, security, priority:blocking, effort:small, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:50.844Z
---

# [Issue 576]: [security(cli): verify-sig trusts ./.tessera/trust/ from the current directory — a planted key widens trust](https://github.com/vig-os/tessera/issues/576)

## Context

Dogfooding from MorePET/anatomist#86 (ADR-0007, PR MorePET/anatomist#103) at tessera `dev` `fa581f5`, with synthetic data only.

## Problem

`verify-sig` without `--pubkey` trusts every key in `trust_dirs()` (`tessera-cli/src/trust.rs:137`), and the first of those is **`./.tessera/trust/` relative to the current working directory**. Whoever can write to the directory a verifier runs in can therefore widen what that verifier trusts. Examples: a shared job output directory, an unpacked archive, or a dataset view.

Repro (synthetic keys, empty user store):

```console
$ tessera keygen keys/other                        # an attacker's key
$ tessera sign forged.tsra --key keys/other --signer anatomist-operator
$ HOME=$PWD/home tessera verify-sig forged.tsra    # exit 1: nothing trusted (good)
$ HOME=$PWD/home tessera trust add --repo --name planted keys/other.pub
$ HOME=$PWD/home tessera verify-sig forged.tsra    # exit 0: accepted through ./.tessera/trust/
```

`--signer` is attribution only, so the claimed `anatomist-operator` identity is accepted as well.

`verify-sig --pubkey <key> --require-signer <id>` fails closed (unsigned, wrong key and wrong signer all exit 1), so consumers can protect themselves. The default is the footgun.

## Ask (pick one or more)

1. Don't consult a cwd-relative store implicitly. Require `--trust-dir <path>` (or `TESSERA_TRUST_DIR`) for a repo store, or look it up only from the git root of an explicitly named repo.
2. When a cwd store contributes the accepting key, say so on stderr (`accepted via ./.tessera/trust/planted.pub`). Optionally refuse it under a `--strict` flag.
3. Document in ADR-0037 and the book that automated verifiers should pin `--pubkey` together with `--require-signer`.

## Linked

ADR-0037 (trust model), MorePET/anatomist#86.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:12 AM_

**Triage (architect, 2026-10-25): P0, decided. All three asks, in this form:**

1. **No implicit cwd-relative trust.** `verify-sig` (and every other verb that consults the trust store) no longer reads `./.tessera/trust/` on its own.
   - A repo store is consulted only when named explicitly: `--trust-dir <path>` (repeatable) or `TESSERA_TRUST_DIR`.
   - The default set is the user store (`~/.config/tessera/trust`) only.
   - `tessera trust add/list/remove --repo` keeps managing `./.tessera/trust/`, and its output says the store must be passed explicitly to take effect.
   - There is no git-root discovery: anything implicit is a planted-key surface again.
2. **Always say which key accepted, and from where.** On success, stderr names the accepting key's fingerprint and the store path. A `--pubkey` override is reported as `--pubkey`.
3. **Docs.** Amend ADR-0037 (the trust-model section) and the book: automated verifiers pin `--pubkey` with `--require-signer`; the store-resolution order is written down; and this is listed as a deliberate behaviour change in the CHANGELOG.

Not doing: `--strict`. Once the cwd store is no longer implicit, a strict flag guards nothing.

Acceptance:
- The issue's repro becomes a test (trycmd or an integration test) that exits 0 before the fix (planted key accepted) and exits 1 after it.
- A test where an explicit `--trust-dir` accepts the same key.
- A test where `TESSERA_TRUST_DIR` is honoured.
- A test that the accepting store is reported on stderr.
- Existing persona tests (`persona_signing.trycmd`, `tests/lifecycle.rs`) are updated to pass the store explicitly. Any test that silently relied on the cwd store is listed in the PR.


