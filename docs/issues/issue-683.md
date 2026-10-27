---
type: issue
state: open
created: 2026-10-26T14:08:01Z
updated: 2026-10-26T19:50:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/683
comments: 1
labels: bug, dependencies, priority:high, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-27T07:16:19.929Z
---

# [Issue 683]: [ci(deps): RUSTSEC-2026-0391 (zstd 0.13.3 via parquet 58.3) fails cargo-deny on every PR](https://github.com/vig-os/tessera/issues/683)

The \`Lint & Format\` job's \`cargo-deny\` hook started failing on 2026-10-26 (seen on #682, run 40335849387,
which touches no dependency) with a newly published advisory:

- **RUSTSEC-2026-0391**: zstd \`with_context\` stream encoders/decoders don't reset a reused context, so an
  abandoned stream's buffered input leaks into the next stream. Solution: upgrade to **zstd >= 0.14.3**.
- Path: \`zstd v0.13.3\` ← \`parquet v58.3.0\` ← \`tessera-cli\`.

\`cargo update -p zstd\` can't fix it, because parquet 58.3 requires zstd 0.13. The options are a parquet
release that depends on zstd >= 0.14.3, or a deliberate, justified \`deny.toml\` ignore (it would need
evidence about whether parquet's writer reuses a zstd context across streams). That's a policy call for the
owner, not a drive-by in an unrelated PR, so it's filed here. Every PR is red on \`Lint & Format\` until then.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 26, 2026 at 07:50 PM_

The cargo-deny failure is four advisories, not one. The advisory db added three more against the same crates after this was filed:

- RUSTSEC-2026-0391: `zstd` 0.13.3, fixed in >= 0.14.3
- RUSTSEC-2026-0398: `zstd-safe` 7.2.4, fixed in >= 8.2.0
- RUSTSEC-2026-0403: `zstd-safe` 7.2.4, fixed in >= 8.2.0
- RUSTSEC-2026-0410: `zstd-sys` 2.0.16, fixed in >= 2.1.2

**Decision (owner, 2026-10-26): guarded, dated ignores.** No bump path exists. zarrs (latest and main) is on zstd 0.13.3, and parquet >= 60 needs arrow 60, which the vortex fork (58.3) and datafusion (59) can't take. None of the four is reachable: parquet and zarrs use only the bulk API and `decode_all`, `seekable` is off, and the wasm32 graph has no zstd-sys. #687 adds the four dated ignores, a `clippy.toml` ban on the affected APIs, and a `[[bans.features]]` ban on `seekable`. #684 tracks removing the ignores and holds the evidence and exit criteria.

