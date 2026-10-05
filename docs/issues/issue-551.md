---
type: issue
state: open
created: 2026-10-05T07:34:22Z
updated: 2026-10-05T07:34:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/551
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T08:17:42.265Z
---

# [Issue 551]: [test(ingest): pin DICOM header metadata against a real implicit-VR file — dicom-rs 0.10 can move manifest_hash with content_hash unchanged](https://github.com/vig-os/tessera/issues/551)

Found by the independent review of #543 (dicom-rs 0.9 → 0.10).

The DICOM golden (`tessera-ingest/src/dicom.rs:1019`) pins only `content_hash` (voxels). The header recorded in product metadata (`header_to_json`, `dicom.rs:377`) is not golden-pinned against real-world files, and all fixtures are synthetic **explicit-VR**. dicom-rs 0.10 changed things that can move that header (and so `manifest_hash`) without moving `content_hash`:

- updated tag dictionary (dicom-rs #790): for **implicit-VR** files the VR comes from the dictionary, so the `"vr"` strings in the header JSON can change on re-ingest;
- non-recursive data-set reader (#774): nested sequences;
- adaptive VR decoder (#756): more lenient parsing, so some files that used to be rejected may now be accepted.

That is the manifest-only divergence class (content identical, manifest silently different) that #538 found three instances of.

**Do:** add a small, non-PHI, licence-clear implicit-VR DICOM fixture with at least one nested sequence, golden-pin its full header JSON and `manifest_hash` (not only `content_hash`), and A/B it across the 0.9.1 → 0.10.0 bump to establish whether anything actually moved. No real patient data.

Also stale after #543: `tessera/deny.toml:16` (RUSTSEC-2021-0153 comment says "dicom 0.9"; dicom-encoding 0.10.0 still pulls `encoding`, so the ignore stays) and `crates/tessera-cli/src/info.rs:13` doc example (`dicom 0.9.7`).
