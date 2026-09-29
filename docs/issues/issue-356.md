---
type: issue
state: closed
created: 2026-08-03T01:24:55Z
updated: 2026-09-28T23:30:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/356
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:45.421Z
---

# [Issue 356]: [test(array): encode_emits_a_structured_compression_trace fails in a full-suite run, passes in isolation](https://github.com/vig-os/tessera/issues/356)

`array::tests::encode_emits_a_structured_compression_trace` fails deterministically under

```
cargo test --release -p tessera-io --lib
```

with `missing encode event: ` (the captured log is empty), and passes when run alone:

```
cargo test --release -p tessera-io --lib -- --test-threads=1 array::tests::encode_emits_a_structured_compression_trace
```

Reproduced on `origin/dev` at 864f2f6, so it is not introduced by any open PR — I hit it while working on #355 and checked the base before assuming it was mine.

**Likely cause:** `tracing` caches a callsite's `Interest` **globally**, on first use. Several other tests call `array::encode` with no subscriber installed; whichever reaches the `debug!("encoded array block")` callsite first registers it against `NoSubscriber`, which returns `Interest::never`, and that verdict is cached process-wide. The test's `tracing::subscriber::with_default(...)` sets only a *thread-local* dispatcher, which never gets consulted because the callsite is already disabled.

**What I tried, that does not work:** `tracing::callsite::rebuild_interest_cache()` inside the `with_default` closure — it re-registers against the *global* dispatcher, still `NoSubscriber`, so the callsite stays disabled. Raising the unrelated `write.rs` trace test from `Level::INFO` to `TRACE` also changes nothing (it was a wrong guess at the mechanism — the two subscribers are not interfering with each other).

**What should work:** install a process-wide global default subscriber for the test binary that is interested in everything (e.g. a `TRACE`-level fmt subscriber writing to `io::sink`), once, before any test runs. The callsite then caches as enabled and each test's thread-local `with_default` capture receives events normally. Both trace tests (`array.rs` and `write.rs`) would use it.

Not fixing it inside #355 since it is unrelated to that change and the fix touches shared test setup.
---

# [Comment #1]() by [gerchowl]()

_Posted on August 3, 2026 at 01:26 AM_

**Correcting the impact statement in the issue body: this does not affect CI, only `cargo test`.**

CI runs `nix flake check`, which drives `cargo nextest run`. nextest executes each test in its **own process**, so the global callsite-interest cache is never poisoned by another test and this one passes — which is why `dev` and PR CI are green. I should have checked the runner before writing "fails deterministically" without qualification.

The bug is still real, it just has a narrower blast radius than the body implies:

- `cargo test -p tessera-io --lib` (one process, threaded) — **fails**
- `cargo test -p tessera-io --lib -- --test-threads=1` — **passes** (116/116)
- `cargo nextest run` / CI — **passes**

So it bites contributors running plain `cargo test` locally, which is the documented entry point in `flake.nix` (`cd tessera && cargo test`). Worth fixing for that reason, but it is not a CI or release-gate problem and nothing is being shipped broken.

