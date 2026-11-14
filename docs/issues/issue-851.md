---
type: issue
state: open
created: 2026-11-14T01:17:57Z
updated: 2026-11-14T01:17:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/851
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-14T08:34:33.343Z
---

# [Issue 851]: [cli: error messages for a missing file, an unbuilt feature, and the version banner mislead binary users](https://github.com/vig-os/tessera/issues/851)

From the 2026-11-14 review, verified on the alpha.5 release binary:
- `tessera inspect nope.tsra` prints `error: io: No such file or directory` without the path. `inspect s3://…` in the cloud-less build gives the same message, where it should say the feature isn't compiled in.
- `sql` and `push` in the default binary fail with 'invalid product: … rebuild: cargo build --features …'. That is the wrong error category (the product isn't invalid), and the advice is useless to someone who installed a binary. Also decide whether release binaries should ship with sql+cloud enabled.
- `inspect` prints `tessera 0.0.0` for alpha.5.
- Help text says `tsra help`, but only `tessera` ships.
- `ingest table --name demo` yields a block called `data`.

Acceptance: each case has a trycmd test with the corrected message.
