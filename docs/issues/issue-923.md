---
type: issue
state: open
created: 2026-11-19T13:42:52Z
updated: 2026-11-19T13:42:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/923
comments: 0
labels: chore, priority:low, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:09.575Z
---

# [Issue 923]: [chore(ingest): #894 follow-up nits — NIfTI raw-vs-sealed scope, --meta composite paths, column-meta wording, TOML datetime leak](https://github.com/vig-os/tessera/issues/923)

Nits from the fresh review of PR #894, round 3 (head b580323f). The owner rule is that numbers which cannot be sealed are refused. None of these break that rule; they are scope and wording issues.

1. **The NIfTI door refuses some fields more broadly than needed.** It checks raw header fields whose sealed value would be in range, or which are never sealed:
   - `toffset=1e16` in msec would seal as 1e13 s;
   - a singular sform with `srow_x[3]=1e16` falls back to the qform;
   - `pixdim[4]` = NaN or ≤−2^53 with a time unit, where nothing is sealed.

   Decide whether to validate the sealed values (narrower) or the raw fields (current). Either way, write the rule down in the NIfTI docs and in ADR-0020.
2. **Some `--meta` refusals name the wrong path or use a different message.**
   - An underflow inside a composite value names `--meta k = 1e-400` instead of the inner path (`k.a.b`).
   - `--meta k=[1e400]` uses a different message family ("is JSON a seal cannot hold … number out of range") from bare `1e400`.
3. **`--column-meta` wording is inconsistent.** The underflow refusal names `a.scale`, while the other refusals say `column 'a' scale`.
4. **The string advice is triggered too broadly.** `as_string` checks `path.has_key("metadata")` on any path segment, so a typed field under a key named `metadata` (for example a column called "metadata") would get the string advice. No reachable case was found. Match the free-form metadata door exactly.
5. **TOML datetimes leak an internal key (not from #894).** A TOML datetime in metadata seals as `{"$__toml_private_datetime": …}`, which is an internal serde key. It should be refused or mapped to a documented string form.

Refs #886.

