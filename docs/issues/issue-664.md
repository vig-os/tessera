---
type: issue
state: closed
created: 2026-10-25T10:37:33Z
updated: 2026-10-25T12:50:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/664
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:23.149Z
---

# [Issue 664]: [fix(deps): cargo-deny fails every PR on four new RustSec advisories (rustls-webpki, webpki-root-certs, tokio, basic-toml)](https://github.com/vig-os/tessera/issues/664)

## Defect

Every PR's `Lint & Format` job fails today (2026-10-25) at the `cargo-deny` pre-commit hook on four RustSec
advisories published against crates already in `tessera/Cargo.lock`. The failures don't depend on the PR's
diff: docs-only #661 fails the same way as #632's branch (`8fd89a4`) and #576's branch (`3f9f92e`).

| Advisory | Crate (locked) | Kind |
|---|---|---|
| RUSTSEC-2026-0374 | `basic-toml` 0.1.10 | unmaintained |
| RUSTSEC-2026-0382 | `rustls-webpki` 0.103.15 | vulnerability (distrusted intermediates accepted) |
| RUSTSEC-2026-0383 | `webpki-root-certs` 1.0.8 | vulnerability (distrusted emSign intermediates) |
| RUSTSEC-2026-0386 | `tokio` 1.52.3 | unsound (`AsyncFd::new` I/O safety) |

Evidence: run 40182218773, job `Lint & Format`, step "Run pre-commit hooks" (`hook id: cargo-deny`, exit 1).

## Fix direction

Bump each crate to a release that carries the fix (a lockfile update), or, where no fixed release exists, record a
reasoned, dated `ignore` in `tessera/deny.toml` naming the advisory and why it doesn't reach us. Don't use a blanket
ignore. Check the Gate B feature snapshots if a bump moves a crate on the seal path.

## Acceptance

`cargo deny check` is clean on `dev`, and `Lint & Format` goes green on a no-op PR.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 12:50 PM_

Fixed by #676 (merged as 81944d1): rustls-webpki 0.103.15→0.103.16, webpki-root-certs 1.0.8→1.0.10, tokio 1.52.3→1.53.3; the basic-toml (RUSTSEC-2026-0374) ignore is narrow and dated, and removal is tracked in #665. cargo-deny is clean on dev.

