---
type: issue
state: closed
created: 2026-08-19T09:19:40Z
updated: 2026-09-29T02:38:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/389
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:41.811Z
---

# [Issue 389]: [docs: ingest cookbook + product-schema reference + Python write example; fix cli.md `{{#include}}` stubs](https://github.com/vig-os/tessera/issues/389)

**AX audit findings (engineer).** Onboarding-doc gaps:
- No enumerated **built-in products** (`listmode`/`recon`/`blob`/…) with their required/recommended
  `--meta` fields and block roles.
- The **`--spec` TOML** format has no shipped reference/minimal example (only an in-repo link).
- The **Python write path** is claimed ("read/verify/**write**") but never shown — no `tessera.write(...)`
  snippet/signature.
- `docs/book/src/cli.md` is mostly `{{#include …trycmd}}` directives → renders empty outside an mdBook
  build (the raw docs a user is handed don't stand alone).

## Ask
An "ingest cookbook" chapter: a 10-line minimal `--spec`, a Python write snippet, a product→required-
fields table; and ship real command transcripts (or a standalone quickstart-ingest page) instead of
include-stubs. Refs the AX/onboarding audit (#386).
