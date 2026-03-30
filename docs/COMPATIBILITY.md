# fd5 Compatibility Policy

This document defines the versioning, compatibility, and deprecation guarantees for the fd5 format and its tooling.

## Format version

fd5 uses semantic versioning for the format itself. The format version is tracked independently of the Python package version.

- **Current format: v0.x** (pre-stable). Breaking changes are possible between minor versions. Users should pin their fd5 package version and test migrations before upgrading.
- **Target: v1.0** (stable). Once the format reaches v1.0, the guarantees in this document become binding commitments. The criteria for v1.0 are: conformance test suite passing across Python and at least one other language, and at least six months of production use without schema-level changes.

## Schema versioning

Each product schema has its own version, stored as the `_schema_version` integer attribute on the file root. Schema versions follow semantic versioning principles applied to HDF5 structure:

- **Patch** (1.0.x): Bug fixes in schema description text or documentation. No structural changes to groups, datasets, or attributes. Files are byte-identical before and after a patch-level schema update.
- **Minor** (1.x.0): Additive changes only -- new optional groups, datasets, or attributes. Old readers can safely ignore unknown fields. No migration is required; existing files remain valid.
- **Major** (x.0.0): Breaking changes -- removed or renamed fields, changed dtypes, restructured groups. Requires migration to produce a new file conforming to the updated schema. The fd5 migration framework handles this (see [Migration Guide](tutorials/migration-guide.md)).

Within polymorphic sub-groups (e.g., `metadata/`), the `_type` and `_version` attributes handle evolution independently of the top-level schema version. A new algorithm or method uses a new `_type` value with its own attributes. No schema change is required, and no re-ingest of existing data is needed.

## Reader tolerance rules

These rules define how fd5 readers must behave when encountering files written by newer or older versions of the format.

- **Readers MUST ignore unknown attributes and groups.** This is the foundation of forward compatibility. A v1 reader encountering a v2 file with additional groups reads what it knows and skips the rest.
- **Readers MUST NOT require optional fields.** Only fields marked `required` in the product's JSON Schema are expected to be present. Missing optional fields must not cause errors.
- **Readers SHOULD warn on unknown `_schema_version` but still attempt to read.** If a reader encounters a version newer than it supports, it logs a warning and reads what it can. It may refuse to open the file only if it cannot guarantee correct interpretation of the data.
- **Writers MUST embed the schema they wrote against.** The `_schema` attribute on the file root contains the full JSON Schema document used during creation. This makes every file self-describing.

## Writer guarantees

These invariants hold for every fd5 file produced by a conforming writer:

- **Sealed files MUST pass `fd5.verify()` immediately after creation.** The `content_hash` stored in the file must match the Merkle tree recomputed from the file's data and attributes.
- **`content_hash` MUST be deterministic.** Given the same data, metadata, and attributes, the content hash is always the same. The hash is a `sha256:`-prefixed hex digest of the Merkle root computed bottom-up from all datasets and attributes (excluding the `content_hash` attribute itself).
- **`id` MUST be deterministic.** Given the same `id_inputs` (the set of attributes listed in `id_inputs`), the `id` hash is always the same. The `id` is computed as `sha256(sorted key-value pairs joined with null bytes)`.
- **Files MUST be readable by any HDF5 tool.** fd5 files are standard HDF5. They can be opened with `h5dump`, HDFView, MATLAB `h5read`, R `rhdf5`, or any other HDF5-compatible tool. No custom file format, compression codec, or plugin is required beyond standard HDF5.

## Deprecation policy (for v1.0+)

Once fd5 reaches v1.0, these deprecation rules apply:

- **Deprecated features are announced in the CHANGELOG** with a clear deprecation timeline and the version in which they will be removed.
- **Deprecated features remain functional for at least 2 minor versions.** For example, a feature deprecated in v1.2 will not be removed until v1.4 at the earliest.
- **Migration tools are provided before removing deprecated features.** Users will always have a path from the old behavior to the new one before the old behavior is removed.
- **Breaking changes only occur in major version bumps.** No attribute removal, dataset renaming, or dtype change happens in a minor or patch release.

## Cross-language parity

fd5 is designed to be readable and writable from multiple languages. The following rules ensure interoperability:

- **All language implementations MUST pass the conformance test suite.** The test suite verifies file structure, attribute types, hash computation, and schema embedding.
- **Files created by any language MUST be readable by all others.** A file created by the Python SDK must be readable by the Rust or C/C++ implementation, and vice versa.
- **Hash computation MUST produce identical results across languages.** The Merkle tree algorithm, attribute serialization order, and byte representations are specified precisely to ensure this. Key details:
  - Attributes are hashed in sorted key order.
  - String attributes use UTF-8 encoding.
  - Numeric attributes use their NumPy `tobytes()` representation (native byte order of the stored array).
  - Dataset data is hashed as contiguous row-major bytes (`C` order).

## Version contract summary

From the [white paper](../white-paper.md):

> - New versions only ADD attributes and groups
> - Existing attributes and groups are never removed or renamed
> - A reader for version N can always read version N-1
> - A reader encountering a version newer than it understands logs a warning and reads what it can
> - The `_type`/`_version` mechanism handles evolution within polymorphic sub-groups independently of the top-level schema version

These rules, combined with the immutable-file design (schema changes require migration to a new file), ensure that fd5 files remain trustworthy for long-term data storage.
