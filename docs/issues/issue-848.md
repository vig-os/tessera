---
type: issue
state: open
created: 2026-11-13T22:22:36Z
updated: 2026-11-18T14:52:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/848
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:15.598Z
---

# [Issue 848]: [feat(io): single-pass multi-table streaming: unknown-length push_table, concurrent table handles, header after stream (downstream: visionx-rs#39)](https://github.com/vig-os/tessera/issues/848)

Filed by the [private-repo] architect on 2026-11-13, at the owner's direction. The owner decided visionx-rs#39 (the streaming scan writer) **waits for this** rather than working around it with a two-pass writer.

## The producer
The Siemens list-mode decoder runs one parallel pass over a file of up to about 19 GB. That pass emits three tables at once, interleaved in packet order:
- `pairs`: up to about 2.4 G rows;
- `markers`;
- `ms_stats`.

It also produces payload-derived header fields: mode evidence (`ModeStatistics`) and a per-file decode summary. Memory must stay bounded: 8 GiB measured as cgroup `memory.current`.

## What alpha.5 does
- **Row count up front:** `StreamWriter::push_table` needs the exact `spec.rows` before the first row (`tessera-io/src/stream.rs:862`, `:1001-1026`).
- **One table per call:** each call takes `&mut self` and pulls synchronously (`stream.rs:927-934`), so the three tables cannot be fed at the same time.
- **Header fixed first:** the spine is set before `StreamWriter::new` (`write.rs:638-686`), so header fields known only after the stream cannot be recorded.

## Ask
Any design that lets one pass write several bounded streamed tables. We'd suggest:
1. **Unknown-length `push_table`.** `spec.rows` is fixed at the end, using the "future unknown-length partitioning" that ADR-0064 §1/§2 leave room for.
2. **Several open table handles per `WriteSession`** (`pairs`, `markers`, `ms_stats`), sharing the writer's W+1 staging bound.
3. **Header and provenance fields set after streaming and before seal**, or a documented sealed-block pattern for payload-derived evidence.

We read this from the alpha.5 source and from ADR-0064, and would be glad to review an ADR.

Downstream issues: [private-repo]#39, #40 and #56. Context: tessera#716 (thread from 2026-11-13) and [private-repo]#221.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 02:18 AM_

#847 (closing #806) delivers the first ask: a table of unknown length (`open_table`, one leaf of lookahead, byte-identical to `TableMultiBlockSink`/`push_table`). It also lets several tables be open at once (`W + T` staged leaves). Check whether that covers your second ask. 'Header after the stream' (the third) is not in it; it comes with ADR-0065 plan item #831 (`StreamWriter::finish_with`, header patch at the seal).

