---
type: issue
state: closed
created: 2026-02-25T20:25:03Z
updated: 2026-09-28T17:53:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/112
comments: 4
labels: feature, effort:small, area:core
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:12.136Z
---

# [Issue 112]: [[FEATURE] fd5.ingest.raw — raw/numpy array loader](https://github.com/vig-os/tessera/issues/112)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/raw.py` — a loader that wraps raw numpy arrays (or binary files) into sealed fd5 files. This is the simplest loader and serves as:

1. The reference implementation of the `Loader` protocol
2. A practical tool for users who already have data in numpy/binary form
3. The fallback when no format-specific loader is needed

## Scope

- Accept numpy arrays directly (in-memory)
- Accept raw binary files with user-specified dtype/shape
- Support any product type registered in the schema registry
- Delegate all product-specific writing to the product schema's `write()` method

### No additional dependencies

This loader uses only `numpy` (already a core dependency).

## Proposed API

```python
def ingest_array(
    data: dict[str, Any],
    output_dir: Path,
    *,
    product: str,
    name: str,
    description: str,
    timestamp: str | None = None,
    metadata: dict | None = None,
    study_metadata: dict | None = None,
    sources: list[dict] | None = None,
) -> Path:
    """Wrap a data dict into a sealed fd5 file.
    
    The data dict is passed directly to the product schema's write() method.
    """

def ingest_binary(
    binary_path: Path,
    output_dir: Path,
    *,
    dtype: str,
    shape: tuple[int, ...],
    product: str,
    name: str,
    description: str,
    **kwargs,
) -> Path:
    """Read a raw binary file, reshape, and produce a sealed fd5 file."""
```

## Acceptance criteria

- [ ] Implements `Loader` protocol from `fd5.ingest._base`
- [ ] `ingest_array()` produces valid fd5 files for any registered product type
- [ ] `ingest_binary()` reads raw binary with specified dtype/shape
- [ ] Provenance records source binary file SHA-256 (for `ingest_binary`)
- [ ] Tests cover recon and at least one other product type
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)
---

# [Comment #1]() by [gerchowl]()

_Posted on February 25, 2026 at 08:39 PM_

## Design

Issue: #112 — `fd5.ingest.raw` — raw/numpy array loader

### Context

Since #109 (`fd5.ingest._base`) is not yet implemented, this PR will create the minimal `ingest` package: `__init__.py`, `_base.py` (Loader protocol + helpers), and `raw.py`.

### Architecture

**Module structure:**
```
src/fd5/ingest/
├── __init__.py      # re-exports: Loader, ingest_array, ingest_binary, hash_source_files
├── _base.py         # Loader protocol, hash_source_files() helper
└── raw.py           # ingest_array(), ingest_binary(), RawLoader class
```

**`_base.py`** — Minimal foundation (subset of #109):
- `Loader` protocol (runtime_checkable) with `supported_product_types` property and `ingest()` method
- `hash_source_files(paths)` → list of dicts with `path`, `sha256`, `size_bytes` for provenance records

**`raw.py`** — Two public functions + a `RawLoader` class:
- `ingest_array(data, output_dir, *, product, name, description, ...)` → Path: wraps a data dict into a sealed fd5 file using `fd5.create()` context manager. Delegates product-specific writing to the schema's `write()` method.
- `ingest_binary(binary_path, output_dir, *, dtype, shape, product, name, description, ...)` → Path: reads a raw binary file via `numpy.fromfile`, reshapes, builds a data dict with a `volume` key, then delegates to `ingest_array`. Records source binary SHA-256 in provenance.
- `RawLoader` class: implements the `Loader` protocol, wrapping `ingest_array`.

### Data Flow

1. User calls `ingest_array(data_dict, out_dir, product="recon", ...)` or `ingest_binary(path, out_dir, dtype="float32", shape=(64,64,64), product="recon", ...)`
2. For `ingest_binary`: read binary → reshape → compute SHA-256 → build provenance record → call `ingest_array`
3. `ingest_array` uses `fd5.create()` context manager (existing API) to:
   - Set root attrs (product, name, description, timestamp)
   - Call `builder.write_product(data)` (delegates to schema `write()`)
   - Optionally write metadata, sources, provenance
   - Seal the file (schema embedding, hashing, rename)
4. Return the final sealed file path

### Key Decisions

1. **Reuse `fd5.create()`** rather than duplicating file creation logic — keeps the loader thin.
2. **`ingest_binary` builds the data dict itself** using the key `"volume"` for the reshaped array. Additional data dict keys can be passed via `**kwargs` for product schemas that need more (e.g. `affine`, `dimension_order`).
3. **Timestamp defaults to `datetime.now(UTC).isoformat()`** when not provided.
4. **Provenance for `ingest_binary`** records `original_files` with SHA-256 and size, plus ingest tool/version metadata.
5. **`RawLoader.supported_product_types`** returns all registered product types from the registry, since raw arrays can be used with any product.

### Error Handling

- Unknown product type → `ValueError` from `fd5.registry.get_schema()` (fail fast)
- Binary file not found → `FileNotFoundError`
- dtype/shape mismatch with file size → `ValueError` with clear message
- Schema `write()` failures → propagate naturally through `fd5.create()` cleanup

### Testing Strategy

- Unit tests for `hash_source_files` (happy path, empty, non-existent file)
- Unit tests for `ingest_array` with recon product (produces valid sealed fd5)
- Unit tests for `ingest_binary` (reads binary, records SHA-256 provenance)
- Unit test for `RawLoader` protocol conformance
- Edge cases: empty array, missing required fields, wrong dtype/shape for binary
- Tests for sinogram product type to satisfy "at least one other product type" criterion


---

# [Comment #2]() by [gerchowl]()

_Posted on February 25, 2026 at 08:39 PM_

## Implementation Plan

Issue: #112
Branch: feature/112-ingest-raw

### Tasks

- [x] Task 1: Create `src/fd5/ingest/__init__.py` and `src/fd5/ingest/_base.py` with `Loader` protocol and `hash_source_files()` helper — `src/fd5/ingest/__init__.py`, `src/fd5/ingest/_base.py` — verify: `uv run python -c "from fd5.ingest._base import Loader, hash_source_files"`
- [x] Task 2: Write tests for `hash_source_files()` — `tests/test_ingest_base.py` — verify: `just test-one ingest_base`
- [x] Task 3: Write tests for `ingest_array()` with recon product — `tests/test_ingest_raw.py` — verify: `just test-one ingest_raw` (expect failures)
- [x] Task 4: Implement `ingest_array()` in `src/fd5/ingest/raw.py` — `src/fd5/ingest/raw.py` — verify: `just test-one ingest_raw`
- [x] Task 5: Write tests for `ingest_binary()` — `tests/test_ingest_raw.py` — verify: `just test-one ingest_raw` (expect failures for binary tests)
- [x] Task 6: Implement `ingest_binary()` in `src/fd5/ingest/raw.py` — `src/fd5/ingest/raw.py` — verify: `just test-one ingest_raw`
- [x] Task 7: Write tests for `RawLoader` protocol conformance + sinogram product type — `tests/test_ingest_raw.py` — verify: `just test-one ingest_raw`
- [x] Task 8: Implement `RawLoader` class and update `__init__.py` re-exports — `src/fd5/ingest/raw.py`, `src/fd5/ingest/__init__.py` — verify: `just test-one ingest_raw`
- [x] Task 9: Update CHANGELOG.md — `CHANGELOG.md` — verify: visual inspection

---

# [Comment #3]() by [gerchowl]()

_Posted on February 25, 2026 at 08:52 PM_

## Autonomous Run Complete

- Design: posted
- Plan: posted (9 tasks)
- Execute: all tasks done
- Verify: all checks pass (997 tests, lint clean, precommit clean on changed files)
- PR: https://github.com/vig-os/fd5/pull/122
- CI: pending (cross-fork PR requires maintainer approval to trigger CI)


---

# [Comment #4]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Superseded by ADR-0056 'ingest array' ladder (#386) — see docs/adr/0056-generic-ingest-normalise-vs-preserve.md.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

