---
type: issue
state: open
created: 2026-10-30T03:09:34Z
updated: 2026-10-30T03:09:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/721
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:12.747Z
---

# [Issue 721]: [feat(io): single-pass streaming blob pack (hash while copying into the container)](https://github.com/vig-os/tessera/issues/721)

**Downstream need (MorePET/visionx-rs#138, stage 0 raw pack).** A single-pass streaming blob pack: hash (blake3) **while** copying into the container.

**Today, on dev 4504efb:**
- `blob_ref_streaming` (`tessera-io/src/blob.rs:54–81`) opens and reads the source to compute blake3.
- `pack_streaming` / `pack_streaming_verified` (`container.rs:176/203`, copy loop from :294) then reopen and read it again to write the archive.

So each source file is read twice. For PET list-mode scans (single files up to about 19 GB, on shared storage that is already I/O-pressured) this doubles stage-0 read I/O.

**Proposal (for tessera to decide):** a public bounded-memory API that takes a path or an `impl Read`, streams it into the container once, and computes the blake3 block digest during the copy. It would optionally expose a digest observer, so callers can compute an extra digest in the same pass. Two properties matter:
- Fail closed: if the source changes during the copy (size or digest mismatch against an expected value, if one is given), return an error.
- The output bytes must be identical to the two-pass path.

**Not blocking.** visionx uses the two-pass `blob_ref_streaming` + `pack_streaming_verified` path for now and documents the I/O cost.

