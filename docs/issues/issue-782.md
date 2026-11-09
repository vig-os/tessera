---
type: issue
state: open
created: 2026-11-08T12:02:30Z
updated: 2026-11-08T12:02:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/782
comments: 0
labels: feature, effort:large, area:core, area:integrity, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:20.455Z
---

# [Issue 782]: [feat(provenance): typed multi-step pipeline records, per-row provenance within a product, and references to external inputs tessera does not hold](https://github.com/vig-os/tessera/issues/782)

## Motivation

Every scitadel-hub corpus product (epic #778) is the output of a **multi-step pipeline**, and the owner requires **full provenance of every step**. For example:

`acquire (route, URL, artefact blake3) → parse (JATS | publisher HTML | Grobid TEI | PDF extractor @ version, config) → table/figure extraction (tool @ version, model weights hash) → chunking (tokenizer, size) → embedding (model id + weights hash, dtype, GPU job id, host) → index build`

Two things are missing.

1. **Typed per-step records.** Who, what, which weights, which inputs, which job and host, for each step, not one scalar producer.
2. **Per-row (per-work) provenance inside one product.** A shard of about 10⁵–10⁶ works mixes sources: some works were parsed from JATS, some from Grobid TEI, some from a PDF extractor. Some were re-extracted with a newer tool than their neighbours. The product-level `producer` can't say which rows came from which pipeline. The input artefacts (PDF/XML blake3 hashes) number in the millions per shard, so they can't be listed as `Source` edges.

## What already exists (verified at `dev` `26159de`)

- **`Producer`** (`tessera/crates/tessera-core/src/provenance.rs:46`) is scalar: `tool`, `version`, `git_commit`, `git_repo`, `dirty`. One per product (ADR-0058 §1).
- **`Generation`** (`provenance.rs:184`) is an opaque `config` map or a `config_ref` to a Blob. Its keys are "never inspected by the engine" (ADR-0058 §2). Model weights hash, GPU job id and host fit there only as untyped free text.
- **`Source` edges** (`provenance.rs:15`; `manifest.rs:60`) are typed by role with a pinned `content_hash`. ADR-0048 settles the shape for many inputs: one merkle-rooted reference, with per-file digests in `aux/`, outside the seal.
- **ADR-0060 (Accepted 2026-10-18, not built; #564)** adds a sealed `actors[]` slot for agentic chains (implementer → reviewer → approver) with a W3C PROV mapping. It models *who acted*, not a typed per-step *tool/model/inputs/outputs* record. Its PROV mapping is the right export target for this issue too.
- **No per-row provenance anywhere.** Grep for row-level, provenance column or lineage column finds nothing.
- **References to inputs tessera doesn't hold.** The hub deletes some original inputs after ingest (open-access PDFs it can fetch again by URL) and keeps others in a cold bucket **outside** tessera. ADR-0033 assumes originals are kept as immutable products. Today:
  - `tessera provenance` (`tessera/crates/tessera-cli/src/provenance.rs:247-280`) classifies an edge whose reference isn't `blake3:<64 hex>` (`looks_like_product_id`, `:233-241`) as `ExternalLeaf`, which is **not** a gap (`is_gap`, `:274-279`). So a URL reference passes `--require-complete`.
  - But a raw artefact referenced by its **blake3 digest** as the reference is taken for a product id and reported as `Missing`. That **fails** `--require-complete`.
  - `Source.content_hash` is defined as the parent product's `manifest_hash` (`provenance.rs:19-21`, SPEC §8). Putting a raw file's blake3 there overloads it.
  - There is no typed place for URL, fetch date, media type or holding status (`held | cold:<location> | discarded`), and nothing re-checks an external leaf's bytes when they *are* fetchable.
  - `--require-complete` is a `tessera provenance` flag; `tessera verify` has no such mode.

## The gap

1. There is no typed, sealed **pipeline** record: an ordered list of steps, each with tool and version, config digest, model and weights digest, inputs (by merkle root), compute context (host, job id, accelerator) and timestamps.
2. There is no way for rows in one product to point at **which** pipeline variant produced them.
3. There is no scalable way to commit to millions of per-row input artefact hashes that a reader can check per row.
4. There is no first-class **external, unheld artefact** reference: digest, URL, fetch date and holding status, kept distinct from a product edge, and checkable on demand.

## Proposal

1. **A sealed `pipeline` slot** (ADR-0058 extension, alongside ADR-0060 `actors`): `steps: [{ id, tool{name, version, git_commit}, config_digest, model{id, weights_blake3}?, inputs: [{role, merkle_root}], compute{host, job_id, accelerator}?, started, ended }]`.
   - Typed keys the engine can filter and compare. Free-form extras stay in `Generation.config`.
   - Maps to PROV `Activity`/`used`/`wasGeneratedBy` through ADR-0060's mapping.
   - **Sensitivity:** `host` and `job_id` should be optional and maskable (ADR-0040 tiers).
2. **A per-row provenance column.** A declared `prov_id` column (a small integer) references a **provenance table block** in the same product. One row per distinct pipeline variant: step-list digest, source kind (`jats|html|tei|pdf-extract`) and extractor versions. Rows of `works`/`elements` point at their variant. It costs about a byte per row after dictionary encoding.
3. **Per-row input hashes:** a declared `artefact_blake3` column on the row (or the `works` row). The product's `Source` edge commits to the merkle root over that column (ADR-0048 shape), so a reader can check one work's input hash against the sealed root without listing millions of edges.
4. **External artefact references:** a typed edge or record `ExternalArtefact { blake3, size, media_type, urls[], fetched_at, holding: held | cold{location} | discarded, licence_class? }`.
   - It is always an **expected leaf**. It never counts as a `--require-complete` gap, whatever its digest looks like, because its kind is declared rather than guessed from the reference shape.
   - Per row, it is the declared `artefact_blake3` column plus a small `artefacts` table block (or `aux/` member, ADR-0042) carrying URLs, fetch dates and holding status, committed through the merkle root as in item 3.
   - `tessera provenance --fetch-check` (opt-in, networked) re-fetches a sample or all held/cold/URL artefacts and compares blake3, reporting `match | changed-upstream | unreachable` without changing completeness.
   - This amends ADR-0033's assumption that originals are always immutable products: an original may be **referenced, not held**.
5. **Inheritance through derivation:** a derived product (embeddings) records its own step and points at the parent's seal. `tessera provenance <product> [--row K]` walks the DAG (as #452 did for `config_ref`) and prints the full chain for one work: artefact → parse → extract → chunk → embed.

## Acceptance

- [ ] A product with a 3-step `pipeline` slot seals and verifies. Tampering with any step field breaks the seal.
- [ ] A shard with 3 provenance variants: a `prov_id` filter returns exactly the rows from one extractor, and the column costs ≤ ~1 B/row (measured).
- [ ] `tessera provenance --row <work_id>` prints the end-to-end chain across a base shard and its embedding product.
- [ ] A per-row `artefact_blake3` is checked against the sealed merkle root.
- [ ] A PROV export includes the pipeline steps (via ADR-0060's mapping).
- [ ] A product whose inputs are `discarded` and `cold` external artefacts passes `tessera provenance --require-complete`, whether or not the reference is digest-shaped. `--fetch-check` reports a changed-upstream artefact without failing completeness.

## Related

- Epic #778. ADR-0058 (producer/generation), ADR-0060 / #564 (actors, PROV), ADR-0048 (edge shape, `aux/` digests), ADR-0040 (sensitivity), #452 (provenance verb), #780 (`prov_id` is a declared reference).

