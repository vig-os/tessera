---
type: issue
state: open
created: 2026-11-16T08:31:13Z
updated: 2026-11-16T08:31:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/889
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:31.382Z
---

# [Issue 889]: [io: sequenced shuffled_pushes_from_four_threads test hung (no wake-up within 60 s) under seat load](https://github.com/vig-os/tessera/issues/889)

## Observed

On ksb-ws on 2026-11-16, at load average about 18, `cargo nextest run -p tessera-core -p tessera-io` (CARGO_BUILD_JOBS=4,
default nextest parallelism) failed once:

```text
FAIL [63.994s] tessera-io stream::sequenced::tests::shuffled_pushes_from_four_threads_seal_the_in_order_bytes
panicked at crates/tessera-io/src/stream/sequenced.rs:451:13:
hung: no wake-up within 60 s
```

Re-runs on the same tree passed: alone in 1.8 s, and in the full 508-test run. The branch was
`fix/873-collection-schema-forward-compat` on top of dev 8647527. Its diff touches only JSON parsing in
tessera-core and nothing in `stream::sequenced`.

## Why it matters

The test asserts there is no lost wake-up. A 60 s timeout under heavy load is either:
- a genuine lost wake-up that a slow scheduler exposes, or
- a test whose time budget isn't load-independent.

The first is a real bug in the #876 sequenced input. The second makes the test flaky. Either way it needs a
root cause, not a longer timeout.

## Next

Reproduce under contention, for example `taskset -c 0-1 <test-bin> shuffled_pushes --test-threads=8`
in a loop, or under `stress`. Then check the wake-up path in `sequenced.rs` for a notify that can happen
before the waiter registers.

