---
type: issue
state: open
created: 2026-10-16T22:38:04Z
updated: 2026-10-16T22:38:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/576
comments: 0
labels: bug, security
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:44.213Z
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

