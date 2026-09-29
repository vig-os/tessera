---
type: issue
state: closed
created: 2026-02-25T22:29:37Z
updated: 2026-09-28T17:54:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/138
comments: 1
labels: discussion, priority:high
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:07.517Z
---

# [Issue 138]: [[DISCUSSION] Scope reduction — focus on medical imaging for v0.1](https://github.com/vig-os/tessera/issues/138)

### Description

The whitepaper positions fd5 as domain-agnostic, with examples spanning medical imaging, genomics, remote sensing, materials science, and AI training pipelines. The product schemas, ingest loaders, and CLI all carry this breadth.

For v0.1, it may be worth explicitly scoping the supported domains to medical imaging and nuclear/positron physics — the domains where fd5 originated and where the product schemas are most developed — and framing other domains as future work rather than current capability.

### Context / Motivation

- The initial use case (issue #1) is DICOM frustration from PET/CT scanners
- All 8 implemented product schemas (`recon`, `listmode`, `sinogram`, `sim`, `transform`, `calibration`, `spectrum`, `roi`, `device_data`) come from medical imaging / nuclear physics
- The whitepaper includes genomics, remote sensing, and materials science examples but no corresponding product schemas or ingest loaders exist
- Mentioning unsupported domains in the spec creates implicit promises; new users from those domains will find nothing they can use today
- Focusing communication on the working use case makes the pitch clearer and more credible

### Options / Alternatives

1. **Scope v0.1 to medical imaging** — whitepaper keeps domain-agnostic design principles, but README/docs/examples focus exclusively on the working use case. Other domains mentioned as "designed for extensibility, not yet implemented."
2. **Keep broad scope** — continue positioning as domain-agnostic from day one. Risk: breadth without depth may dilute the message.
3. **Split the whitepaper** — extract a concise "fd5 core conventions" document and separate "fd5-imaging product schemas" document. The core stays domain-agnostic; the schemas are explicitly domain-specific.

### Open Questions

- Is there a concrete near-term user for any domain outside medical imaging?
- Would narrowing the pitch for v0.1 hurt or help adoption conversations?
- Should non-imaging examples be removed from the whitepaper or just clearly labeled as "illustrative, not yet supported"?

### Related Issues

Related to #1, #108

### Changelog Category

No changelog needed
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5-era discussion; Tessera's scope is set by ADRs 0025/0033/0055/0056/0057 and by the released 0.1.0-alpha.1.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

