---
type: issue
state: closed
created: 2026-08-19T09:19:38Z
updated: 2026-09-29T01:03:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/387
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:42.579Z
---

# [Issue 387]: [feat(cli): non-CSV array output + row caps for `slice`/`project`/`read`](https://github.com/vig-os/tessera/issues/387)

**AX audit finding (analyst).** Array reads (`slice`/`project`) and column reads emit CSV only — fine
for a 64³ toy, a foot-gun for a 512³ volume (a 512-wide × 512-row CSV spew). No NumPy/image/JSON output
and no size guard.

## Ask
- `--format csv|npy|png|json` on `slice`/`project` (analyst wants numpy or an image, not CSV cells).
- A `--rows N` / size-preview guard, or at least a stderr hint when the output will be very large.

Refs the AX/onboarding audit (#386). (Note: the SIGPIPE-on-`| head` bug is already fixed in #386.)
