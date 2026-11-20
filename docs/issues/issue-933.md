---
type: issue
state: open
created: 2026-11-19T22:37:24Z
updated: 2026-11-19T22:37:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/933
comments: 0
labels: chore, priority:medium, area:testing, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:07.808Z
---

# [Issue 933]: [test(core): pin from_stored's id/content_hash checks and S5b's strict-parse stage (#930 mutants)](https://github.com/vig-os/tessera/issues/933)

Follow-ups from the review of PR #930 (#913). The checks are correct today, but nothing pins them: three mutants survive. Fix this before or in #914, which wires readers to `from_stored`.

1. **Uncaught mutants in `from_stored`.** If you delete `m.verify_received_id_and_content()?` from `Manifest::from_stored` (manifest.rs:476) or from `Collection::from_stored` (collection.rs:513), every test still passes. The reviewer forged sealed JCS bytes with a bad `content_hash`, with no `content_hash`, and with a bad `id`. Each was refused with `Integrity`. Add those three as tests for both constructors.
2. **The S5b tests don't isolate step 1.** If you delete `check_i_json` from `parse_strict_bytes` (canonical.rs:709), only the older `integers_beyond_2_pow_53…` test fails. The new S5b tests still pass, because `to_bytes` runs `check_i_json` again. Make the S5b test show that the strict parse itself refuses the input, for example by asserting that the error comes from that stage.
3. **The proptest `to_bytes_output_is_always_canonical` adds little.** Its final `prop_assert_eq` is implied by `check_canonical` returning Ok. Strengthen it with a value comparison that treats an integral double and the integer JCS writes as equal. That restores the round-trip property without the `[0.0]` false failure.

Refs: #897

