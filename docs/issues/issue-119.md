---
type: issue
state: closed
created: 2026-02-25T20:34:35Z
updated: 2026-09-28T17:53:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/119
comments: 1
labels: feature, effort:medium, area:core
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:09.993Z
---

# [Issue 119]: [[FEATURE] fd5.ingest.metadata — RO-Crate and DataCite metadata import](https://github.com/vig-os/tessera/issues/119)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/metadata.py` — helpers that read existing metadata files (RO-Crate JSON-LD, DataCite YAML, or other structured metadata) and use them to enrich fd5 file creation with study info, creators, licenses, and provenance.

This is the **inverse** of `fd5.rocrate` and `fd5.datacite` exports. Instead of *generating* metadata from fd5 files, we *consume* external metadata to populate fd5 files during ingest.

## Use cases

1. **Lab already has an RO-Crate** — import `ro-crate-metadata.json` to populate `study/`, creators, license during fd5 file creation
2. **Dataset has a DataCite record** — import `datacite.yml` to populate study metadata
3. **External metadata file** — generic JSON/YAML metadata that maps to fd5 study/subject/provenance groups

## Scope

### Metadata sources supported

| Source | Format | Maps to |
|--------|--------|---------|
| RO-Crate | `ro-crate-metadata.json` (JSON-LD) | `study/` (license, name, creators), provenance hints |
| DataCite | `datacite.yml` (YAML) | `study/` (creators, dates, subjects) |
| Generic | JSON or YAML with key-value pairs | `study/`, `metadata/`, root attrs |

### Key functionality

1. **RO-Crate import** — parse `@graph`, extract `Dataset` entity for license/name/authors, extract `Person` entities for creators
2. **DataCite import** — parse YAML, extract creators, dates, subjects, title
3. **Merge into builder** — provide a dict compatible with `builder.write_study()` and `builder.write_metadata()`
4. **Conflict resolution** — if the user provides metadata AND an external file, user-provided values take precedence

### No additional heavy dependencies

Uses `json` (stdlib) and `pyyaml` (already a project dependency).

## Proposed API

```python
def load_rocrate_metadata(rocrate_path: Path) -> dict:
    """Extract fd5-compatible study metadata from an RO-Crate JSON-LD file.
    
    Returns a dict with keys: study_type, license, name, description, creators.
    """

def load_datacite_metadata(datacite_path: Path) -> dict:
    """Extract fd5-compatible study metadata from a DataCite YAML file.
    
    Returns a dict with keys: study_type, license, name, description, creators, dates.
    """

def load_metadata(path: Path) -> dict:
    """Auto-detect metadata format and extract fd5-compatible metadata.
    
    Supports: ro-crate-metadata.json, datacite.yml, generic JSON/YAML.
    """
```

These are used by loaders:

```python
# In a loader:
meta = load_rocrate_metadata(rocrate_path)
with create(output_dir, product="recon", ...) as builder:
    builder.write_study(**meta)
```

## Acceptance criteria

- [ ] `load_rocrate_metadata()` extracts license, name, creators from RO-Crate JSON-LD
- [ ] `load_datacite_metadata()` extracts creators, dates, subjects from DataCite YAML
- [ ] `load_metadata()` auto-detects format by filename
- [ ] Returned dicts are directly usable with `builder.write_study()`
- [ ] Missing fields in source metadata → absent keys (no errors)
- [ ] Tests with synthetic RO-Crate and DataCite files
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)
Independent of format-specific loaders — can run in parallel.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Tessera has RO-Crate + DataCite EXPORT (tessera export ro-crate|datacite, fixed by #267 → 6b4b55d). Import direction is not on the roadmap; refile if needed.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

