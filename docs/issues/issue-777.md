---
type: issue
state: open
created: 2026-11-08T11:42:39Z
updated: 2026-11-09T03:32:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/777
comments: 3
labels: feature, effort:medium, area:io
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:22.781Z
---

# [Issue 777]: [feat(io): remote range-read open, verify and projection of .tsra over HTTP/S3 (split; HF helper → 783)](https://github.com/vig-os/tessera/issues/777)

## Motivation

The scitadel-hub corpus (epic #778) will publish tessera products on Hugging Face: about 8M patents with full text and 768-d embeddings, and topic subsets of OpenAlex. **Owner decision (2026-11-08): publish `.tsra` only, with no Parquet copy.** Vortex gives better column access than Parquet: O(1) random take, filter pushdown and column projection. A second format would also double storage and split provenance.

What Parquet would have bought is HF's dataset viewer and `datasets.load_dataset()`. This issue replaces that with a tessera-native path: a consumer should be able to **find, verify and column-project a `.tsra` straight from an HF dataset repo, without downloading the whole file**.

## What already exists

- **An object-store range reader** (ADR-0034 / #225) reads blocks by byte range. HF serves repo files at `https://huggingface.co/datasets/<org>/<repo>/resolve/<rev>/<path>` and supports HTTP `Range` requests (to be verified: redirects to the CDN/xet backend, and range behaviour on large LFS files).
- **`tessera export` emits FAIR metadata records** (`ro-crate | datacite`, `tessera/crates/tessera-cli/src/main.rs:697-704, 1882-1893`). They are the natural source for a dataset card.
- **The streaming provider** (#654 / #666 / #668) is the read side that remote projection would reuse.

## The gap

- No `.tsra` can be opened over plain HTTPS from an HF repo URL.
- There is no generator for an HF dataset card.
- There is no documented publishing workflow.

## Proposal

1. **Remote open over HTTPS:** `tessera inspect|verify|read|sql hf://datasets/<org>/<repo>[@rev]/<path>.tsra`, or a plain `https://…/resolve/…` URL, through the ADR-0034 reader (`object_store` HTTP store or equivalent).
   - Fetch the manifest and the needed blocks by range.
   - `verify --remote` checks the manifest seal and the digests of the blocks it touched.
   - Pin by revision (commit sha), so a published product is immutable by URL as well as by `content_hash`.
2. **A card generator:** `tessera export <file> --hf-card` emits a `README.md` with YAML front matter built from the manifest's DataCite fields: license, citation, `content_hash`/`manifest_hash`, block and column listing, vector columns with dim and dtype, and how to read it with tessera (CLI, Rust, Python). Because the viewer can't render `.tsra`, the card carries a small inline preview table, rendered by `tessera read --limit`, instead.
3. **Docs:** "Publishing a tessera product to Hugging Face". Upload with `hf upload`, pin a revision, make the repo layout one product per file or a Collection, and read remotely.
4. **Deferred and out of scope:** a Parquet projection for the HF viewer and `load_dataset()`. Revisit only if consumers ask for it.

## Acceptance

- [ ] A sealed product in an HF dataset repo can be inspected, verified (`--remote`) and column-projected with bounded transfer. Measured: bytes fetched are much smaller than the file size for a single-column read.
- [ ] Vector columns (#768) read remotely as fixed-size arrays.
- [ ] `--hf-card` produces a card that HF accepts (front matter validates) with a preview table.
- [ ] A docs page and a worked example against a public test repo.

## Related

- Epic: #778 (parent).
- ADR-0034 / #225 (object-store range reader), #654 / #666 / #668 (streaming), #768 (vectors), #601 / #602 (DataCite actors), ADR-0054.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 11:49 AM_

Rescoped by owner decision: publish `.tsra` only, with no Parquet export (Vortex column access beats Parquet, and one format keeps one provenance spine). The issue now covers opening, verifying and column-projecting a `.tsra` straight from an HF dataset repo via HTTP range reads, plus a dataset-card generator. A Parquet projection for the HF viewer and `load_dataset()` is deferred.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 9, 2026 at 03:23 AM_

**Reframe from scitadel-hub (vig-os/scitadel ADR-008, decision 19, 2026-11-08):** scitadel-hub will **not** publish its corpus on Hugging Face; the corpus is walled and served by the hub. Its warm and cold copies sit on S3-compatible stores (versitygw, then Hetzner Object Storage). So scitadel-hub no longer drives this issue.

What still matters from here: **remote range-read open and verify of `.tsra` from any HTTP/S3 object store**, which is what the hub's warm and cold tiers need. HF-specific parts (dataset card, HF repo layout) become optional, for other tessera users who do want to publish. Suggest splitting this issue into (a) generic remote range-read open and verify over HTTP/S3, which scitadel-hub needs, and (b) an HF publication helper, which is optional and lower priority.

---

# [Comment #3]() by [gerchowl]()

_Posted on November 9, 2026 at 03:32 AM_

**Owner ruling, 2026-11-09: split.** This issue keeps the generic half: remote range-read open + verify (+ projection) of a `.tsra` over HTTP/S3 (versitygw, Hetzner), building on ADR-0034/#225 and ADR-0062. It is wave 2 of #778. The optional Hugging Face card/publication helper moves to its own backlog issue (linked below).

