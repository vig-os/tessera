---
type: issue
state: closed
created: 2026-11-20T13:08:37Z
updated: 2026-11-20T21:26:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/939
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-21T09:10:53.596Z
---

# [Issue 939]: [nits from the #938 review (stored-seal groundwork): two surviving mutations, write_pair order, error wording](https://github.com/vig-os/tessera/issues/939)

Follow-ups from the review of PR #938 (#914 part 1). Fold these into part 2 (the cutover), which is the first code to rely on these helpers.
1. `core/src/stored.rs:57`: the hex check survives being mutated to `is_ascii_hexdigit()`. The upper-case test cases also upper-case the prefix. Pin `blake3:` followed by upper-case hex.
2. `core/src/stored.rs:102-106`: a mutation that always says "not stored as its JCS bytes" survives. Assert that the message is absent for the JCS cases.
3. `core/src/stored.rs:114`: every missing seal is reported as "a pre-ADR-0066 (alpha) layout", even for JCS bytes that carry no in-band key. Separately, `COLLECTION_LAYOUT` hard-codes `collection.seal`, which is wrong for a `<stem>.collection.json` member.
4. `error.rs` `MalformedSeal`: the doc says `found` is at most 80 bytes, but it is escaped after truncation, so it can be up to 320 characters.
5. `io/src/stored.rs` `write_pair`: it writes the document and then the seal, and neither write is atomic, so a crash in between leaves the pair inconsistent. Make the pair crash-consistent (for example, a temp file plus rename for each, with the seal last) and document the failure mode.

Refs: #914
