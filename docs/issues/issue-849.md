---
type: issue
state: open
created: 2026-11-14T01:17:54Z
updated: 2026-11-14T01:17:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/849
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-14T08:34:34.255Z
---

# [Issue 849]: [docs: the README's install command returns 404 (releases/latest skips pre-releases), and the front doors are stale](https://github.com/vig-os/tessera/issues/849)

From the 2026-11-14 whole-project review, verified: `curl -L https://github.com/vig-os/tessera/releases/latest/download/tessera-cli-installer.sh` returns **404**, because every release is a pre-release and `/releases/latest` skips them. The versioned URL works.

Fix the install docs at the source: use a versioned URL generated per release, or a stable path that release.yml or the docs-as-tests keep current. Pin it with a check, so the documented install command is tested.

The same review found stale front doors, all on dev at e2ecaa0:
- README, `docs/CONSUMING.md` and `install.md` say 'alpha.1 … held';
- `tessera/README.md`, which ships in the tarball, names branch `spike/tessera-core`;
- `tessera-core/src/lib.rs:1-10` says 'spike skeleton … stubs';
- the RFC status says 'Draft / spike';
- ROADMAP lists D4/D5/D7 as open;
- `MIGRATION.md` contains personal home paths;
- the `docs/pull-requests/` mirror is untouched since August.
Root-CHANGELOG versioning is #841.

Acceptance: a newcomer following the README installs alpha.5 or newer, and a CI check fails if the documented URL 404s.
