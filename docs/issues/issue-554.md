---
type: issue
state: open
created: 2026-10-05T09:40:29Z
updated: 2026-10-05T09:40:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/554
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:32:59.243Z
---

# [Issue 554]: [fix(core): i64 stats near the extremes are serialized as JSON numbers above 2^53 — lossy in JS/wasm readers](https://github.com/vig-os/tessera/issues/554)

Found by the independent review of ADR-0059's equal-width histogram (#539, commit f50f976).

The chunk-index statistics (`lo`/`hi`, `min`/`max`, and potentially `sum`/`count`-adjacent integer fields) are serde-serialized as JSON **numbers**. For int64 data near the extremes these exceed 2^53, which many JSON readers (JavaScript `JSON.parse`, and so the `tessera-wasm`/browser path if it parses into f64) cannot represent exactly: `i64::MAX` round-trips as `9223372036854775808`.

This is **pre-existing** (the i64 stats were JSON numbers before ADR-0059) and not a regression of #539. The histogram `width` is fine (its maximum is exactly 2^52).

Decide: encode i64/u64 stats as decimal strings when |v| > 2^53 (or always), or document that consumers must use a big-int-safe JSON parser, and add a round-trip test through the wasm/JS reader. Interacts with the sealed format, so it is a format decision; it would ideally ride the same format event as ADR-0059 C4 if it is cheap, otherwise a later one.
