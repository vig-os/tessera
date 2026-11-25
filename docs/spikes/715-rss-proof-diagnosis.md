# #715 r2 — why ADR-0064 §8.1 criteria 2 and 6 fail on ksb (diagnosis, no fixes)

Status: measured 2026-11-03 on branch `test/715-ksb-rss-proof`. This note attributes; it changes neither
`push_table`, the criteria, τ nor the setup. The proof (`tessera/bench/ksb/677-rss-proof/run.py`) failed criterion 2
(plateau) and criterion 6 (parallelism) in `tessera-715-sweep-1` (exit 1) and `tessera-715-diag-1` (exit 1);
criteria 1, 3, 4 and 5 passed. The tooling was `diag.py` and `diag.patch` next to `run.py` (the flake's `diag` app;
see its docstring), at commit `252af87` of that branch. It wasn't merged: the proof's evaluator doesn't use it, and
the method of each section says what it measured. Tessera under test: `a3df683`. Consumer: visionx-rs `c4aff94`.
Rows 268,435,456 = 64 leaves, 536,870,912 = 128, 1,073,741,824 = 256; every leaf is `BLOCK_ROWS` = 4,194,304 rows.

## Verdict

- **Criterion 2, W = 1** (`RssAnon` +~25 KB per leaf). Each committed leaf's `BlockRef` stays in
  `WriteSession::blocks` until the seal, its spec as a `serde_json::Value` tree (~12.8 KB live per leaf for this
  schema), plus about as much again of allocator retention. **Writer**: a real `c_leaf · n` term, not a leak.
- **Criterion 2, W = 4** (+5.8 %, +14.4 %). The live encode peak alone varies by up to +14.4 % between identical
  runs (163.09–186.53 MB at 256 leaves), plus per-thread arena retention; the per-leaf term is the W = 1 one.
  **Neither**: noise above τ at this `p_block`; criterion 1's ceiling holds.
- **Criterion 6** (W = 4 0.46× and 0.76× of W = 1). The scratch device, shared by every pod on the node, is
  saturated (79–98 % busy); on tmpfs W = 4 is **2.78×** faster. **Setup.**

## 1. W = 1 per-leaf growth

**Method.** heaptrack 1.5 (LD_PRELOAD, unmodified tessera; the reproducer built with line tables, unstripped)
on ksb, `--cpu 4 --exclusive-cpus --mem 8Gi`. `heaptrack_print --print-flamegraph --flamegraph-cost-type peak`
gives each stack's bytes live at the peak; `diag.py heap` diffs the larger run against the smaller per stack,
keyed by its first six frames outside std/alloc/hashbrown files. heaptrack prints decimal units (162,295,236 B
is "162.30M").

**Peak live heap, W = 1** (heaptrack "peak heap memory consumption"):

| Leaves | Runs (job) | Peak live heap |
| ---: | --- | ---: |
| 64 | heap-w1, heap2-w1, heap3-w1, heap4-w1, heap5-w1 | 49.06, 49.08, 48.81, 48.83, 49.16 MB |
| 128 | heap4-w1, heap5-w1 | 50.10, 49.95 MB |
| 256 | heap2-w1, heap3-w1 | 51.40, 51.49 MB |

Live slope: (51.45 − 48.99) MB / 192 leaves = **12.8 KB per leaf**. `RssAnon` in the proof runs (sweep-1,
diag-1): 59.5 and 60.0 MiB at 64 leaves, 64.1 and 64.7 MiB at 256, 71.7 MiB at 573, so **~25.4 KB per leaf**,
and within the 256-leaf run it rose steadily (59.4 → 64.7 MiB over the input's tenths). The encoder's bytes at
the peak don't move: `vortex_btrblocks` held 21,094,170 B at both 64 and 256 leaves (heap3-w1); the whole
growth is `serde_json` (429,728 → 2,994,667 B).

**Where (heap5-w1, 64 → 128 leaves, bytes live at the peak):**

| Stack | 64 leaves | 128 leaves | Per added leaf |
| --- | ---: | ---: | ---: |
| `indexmap::Bucket` clone (`clone_into`, `extend_from_slice`) | 629,280 | 1,258,560 | 9,833 B |
| `IndexMap` bucket clone, second site | 136,173 | 272,346 | 2,128 B |
| `append_staged (write.rs) <- commit_loop (stream.rs)` | 24,866 | 49,661 | 387 B |

Both bucket stacks exactly double with the leaves. Reading the code they come from: `stream.rs:1151` commits
each leaf with `session.append_staged(bref, staged)`; `WriteSession::append_staged` passes
`block_ref.clone()` to `commit` (`write.rs:763`) and returns the original, which the caller drops; `commit`
pushes the clone onto `self.blocks` (`write.rs:880`, the field `blocks: Vec<BlockRef>` at `write.rs:227`). So
the session holds one `BlockRef` per committed leaf until the seal, and `BlockRef.spec` is a
`serde_json::Value`: for the consumer schema that is 3,053 B as JSON (12 columns with 1,682 characters of
descriptions) and ~12 KB as an IndexMap tree of small heap strings.

**Classification, per leaf, for this schema:**

- **(a) Necessary until seal: ~3.1 KB.** The manifest lists every leaf with its name, kind, digest and full spec
  (ADR-0064 §1–2: the marker and `spec.rows` are per leaf, and the format stores each leaf's columns), so the
  content of each `BlockRef` must reach the manifest the seal writes: 3,053 B of spec JSON, the digest (71 B)
  and name. The Merkle accumulator's per-leaf state and the `blocks` Vec slot are in the 387 B row above.
- **(b) Retained but not necessary in this form: ~9.7 KB.** The 12.8 KB live per leaf less its 3.1 KB of
  content: the spec held as an IndexMap/String tree (the two bucket stacks, 12.0 KB per leaf). Of the
  content, everything but `rows`, `partition`, name
  and digest is identical in every leaf: the 12 column specs. Site: `WriteSession::blocks` (`write.rs:227`,
  filled at `write.rs:880` from the clone at `write.rs:763`). It isn't a leak: there is one copy per leaf,
  freed at the seal, and the clone's original is dropped.
- **(c) Allocator retention, not live heap: ~12.6 KB.** `RssAnon` grows ~25.4 KB per leaf against 12.8 KB of
  live heap. The long-lived small allocations of each leaf's `Value` are interleaved with the encoder's large
  transient buffers, so freed space can't be returned.

At the consumer's 573 leaves this is ~7.3 MB live and ~14.6 MB of `RssAnon`, the 59.1 → 71.7 MiB rise the
sweep measured. It scales with the spec's size, so a `c_leaf · n` term is schema-dependent.

## 2. W = 4 transient spikes

Peak live heap at W = 4 for the same input: 64 leaves 159.61, 162.30, 164.73 MB (heap-w4, heap2-w4, heap3-w4);
256 leaves 163.86, 186.53, 163.09 MB. So **live heap alone varies by up to +14.4 % between identical runs** (at
256 leaves; +3.2 % at 64), and by +14.9 % from 64 to 256 leaves within one job (heap2-w4, 162.30 → 186.53 MB),
the size of the spikes the proof saw (+5.8 %, +14.4 %). At the peak the live heap is the Vortex encode of the 4
leaves in flight: in heap3-w4, `vortex_btrblocks` buffers 85.7/80.5 MB and compressor buffers in generic frames
77.0/79.6 MB, including `typed_int_stats` hash maps of 23.3/25.8 MB; between 64 and 256 leaves these moved by
−5.2 and +2.6 MB, with no consistent sign. The only systematic term is the W = 1 per-leaf one (~2.5 MB over 192
leaves). The peaks fell at arbitrary row-groups during steady encoding (348, 2,112, 6,021, 16,013), not at the
seal: the spike is the 4 encoders' own peaks coinciding.

`RssAnon` at W = 4 was 189.9–226.0 MiB (199–237 MB) against live peaks of 159.6–186.5 MB, so roughly 40–75 MB
above the live heap, against ~13 MB at W = 1 (59.5 MiB = 62.4 MB against 49 MB): glibc keeps one arena per
allocating thread (4 workers and the committer). Every W = 4 point stayed under
criterion 1's ceiling, 298.4 MiB; the highest `RssAnon` measured was 236,961,792 B (226.0 MiB, par-c4).

## 3. Parallelism at 268 M rows

**Method.** `diag.py par` samples, every 100 ms, each thread's CPU time and state (threads are unnamed and
listed in creation order: main (generator and stage write), the reproducer's sampler, the W workers (stage read
and encode), the committer), the session device's sectors and busy time, and the cgroup's PSI stall totals.

| Job | CPUs | Volume | W = 1 | W = 4 | W1 / W4 | Device busy | IO stall |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| tessera-715-sweep-1 (proof) | 4 | scratch | 87.8 s | 190.2 s | 0.46× | — | — |
| tessera-715-diag-1 (proof, W = 4 first) | 4 | scratch | 159.5 s | 211.1 s | 0.76× | — | — |
| tessera-715-r2-par-c4, pass 0 | 4 | scratch | 235.6 s | 153.0 s | 1.54× | 98 % / 98 % | 16 / 21 % |
| tessera-715-r2-par-c4, pass 1 | 4 | scratch | 98.2 s | 78.9 s | 1.24× | 84 % / 92 % | 16 / 23 % |
| tessera-715-r2-par-c8 | 8 | scratch | 75.5 s | 66.5 s | 1.14× | 79 % / 91 % | 25 / 14 % |
| tessera-715-r2-par-shm | 4 | `/dev/shm` | 44.4 s | **16.0 s** | **2.78×** | none | 0 % |

**Per-stage CPU (seconds, per run).** Generator alone: 5.1 s (`tessera-715-r2-par-gen`, `--generate-only`).
Main thread (generator + stage write): 8.8–11.1 s, so the stage write is ~4–6 s. Stage read + encode: 39.7–42.1
s on one worker at W = 1, 9.4–11.3 s on each of four at W = 4: the same work, split. Committer (journal, fsync,
rename, seal): 0.7–1.1 s.

**Where the time goes on scratch.** CPU is idle (cgroup CPU stall 0.1–0.9 %). The committer spends 74–96 % of
its samples in D state, the main thread 18–39 %, the workers 5–25 %. The device is 79–98 % busy and recorded
7.1–22.5 GiB written per run, more than a run writes (≤ ~10.5 GB: staged input, staged blocks, output). The
probe (`tessera-715-r2-probe`) shows why: both scratch volumes are emptyDirs on `vdf`, the node's 1 TB kubelet
disk that every pod's emptyDirs share. Every virtio disk reports `rotational=1`, so the flag doesn't tell
media apart. `/` and `/tmp` are overlay mounts, and `ksb run --help` and `ksb caps` offer no other volume type:
the only non-HDD volume a job can get is `/dev/shm`, which is RAM and counts toward `--mem`.

**Run-to-run variance on scratch** at 268 M rows, five runs per W in four jobs (one of them at 8 CPUs):
W = 1 75.5–235.6 s (3.1×), W = 4 66.5–211.1 s (3.2×). Identical passes of one job differ 2.4× (235.6 / 98.2 s).

**Answer.** The saturated resource is the shared scratch device. Removing CPU contention (`--cpu 8`) doesn't
help: 1.14×. Removing the disk (tmpfs) gives 2.78×, with W = 4 then CPU-bound on 4 cores (5 busy threads, CPU
stall 15.6 %). So the slowdown is the setup. The writer has no serialization point that stops W = 4 from
scaling when its I/O is fast. On this disk its committer, which fsyncs every commit, waits on the device. Whether
a dedicated HDD would also saturate wasn't measured: ksb has no such volume.

## Jobs

| Job | Purpose | Exit |
| --- | --- | ---: |
| tessera-715-r2-probe | block devices behind the volumes | 0 |
| tessera-715-r2-heap-w1 | heaptrack W = 1; reports over `ksb out`'s 1 MiB, so cancelled | cancelled |
| tessera-715-r2-heap-w4 | heaptrack W = 4, peak summaries (reports too large to fetch) | 0 |
| tessera-715-r2-heap2-w1, -w4 | peak summaries (stack parse bug: empty frames) | 0, 0 |
| tessera-715-r2-heapfmt | heaptrack collapsed-format probe, 1 leaf | 0 |
| tessera-715-r2-heap3-w1, -w4 | peak by owner crate, 64 vs 256 leaves | 0, 0 |
| tessera-715-r2-heap4-w1 | site attribution, 64 vs 128 (site pattern too broad) | 0 |
| tessera-715-r2-heap5-w1 | site attribution, 64 vs 128 leaves | 0 |
| tessera-715-r2-par-c4 | stages, CPU, I/O, W = 1 and 4, twice | 0 |
| tessera-715-r2-par-c8 | control (i): 8 CPUs | 0 |
| tessera-715-r2-par-shm | control (ii): session and output on tmpfs | 0 |
| tessera-715-r2-par-gen | generator alone | 0 |

## What this decides, for the architect

- Criterion 2 at W = 1 measures a real per-leaf term the §8 bound omits. Its necessary part (~3.1 KB here) is
  the manifest's per-leaf entry; most of the rest is the `Value` representation and allocator retention.
  Choosing between an explicit `c_leaf · n` term and a writer change that holds less per leaf is outside this
  round.
- At W = 4, τ = 5 % sits below the measured run-to-run variance of the live encode peak (up to +14.4 %).
- Criterion 6 can't be measured on ksb's shared scratch. On tmpfs the writer scales 2.78× at W = 4.
