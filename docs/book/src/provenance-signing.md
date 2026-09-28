# Provenance, signing & trust

Integrity proves a product is *unaltered*. Provenance and signing prove *where it came from* and *who
attests to it*.

## Provenance

Every product carries a `sources[]` DAG — typed-role edges (`ingested_from`, `derived_from`,
`reference`, …), each pinned to the parent's `content_hash`. Because each edge commits to the parent's
seal, chain verification walks from a product back to its root, checking every hop
(`provenance::verify_chain`; cycles are rejected). A derived product (a pyramid, a registration, a
recon) always names its parents this way, so the lineage is a verifiable graph, not a string.

De-identification is a first-class transform: `ingest dicom --deidentify` applies PS3.15, and
`--crypto-shred` carries the *original* identity as an `age`-encrypted `aux/` envelope inside the one
file — recoverable by a key holder, and unrecoverable (shredded) by destroying the key. The sealed edge
stays a single merkle-rooted reference, never a list of patient-bearing paths (ADR-0048).

## Who made it, and how (ADR-0058)

The `sources[]` DAG answers *which parent*. Two further sealed records answer *who built this* and
*how*:

- **`producer`** — the generating tool's identity (`tool`, `version`, and optionally `git_repo`,
  `git_commit`, `dirty`). This is the DAQ / sorter / recon / sim that produced the semantic content,
  not the packager. An external generator records its own identity through the write API or the
  ingest spec's `[product.producer]`.
- **`generation`** — the *recipe*: either an inline `config` bag, or a `config_ref` digest pointing
  at a vendor config file carried verbatim as a Blob block in the same `.tsra`. The bag is
  deliberately **non-opinionated** — the keys belong to the generator, and Tessera never interprets
  or validates them, so an instrument it has never seen is self-describing with zero format change.

Both are inside `manifest_hash`. You cannot alter the recorded recipe without changing the product's
id, which is what makes the record worth trusting — the same reason signatures sign the envelope and
not just the payload. (Wall-clock `ingested_at` and host stay *outside* the seal, in unsealed `aux/`,
so re-ingesting the same bytes stays byte-identical — ADR-0042.)

`ingest --spec` records both declaratively, and `inspect` prints them back:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/provenance.trycmd}}

For scripting, `inspect --json` dumps the whole sealed manifest — provenance included — so you never
have to reach into the container format to read it.

## Signing & trust

A signature is an **ed25519 detached signature over `manifest_hash`** — which transitively attests every
block digest and all metadata. It rides as an additive `aux/signatures/<key_id>.sig.json` member (or a
detached sidecar), so signing never mutates identity. Verification re-hashes the payload *and* checks the
signature, and — being pure-Rust — runs in the WASM core with zero C dependencies.

This walkthrough is the four-persona story — an instrument generates a key, signs a study, a custodian
adds the signer to a trust store, and verification succeeds (and rejects a wrong key):

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/persona_signing.trycmd}}

The signer identity can be an **ORCID** (attribution) or a trust-store handle (trust); ssh-ed25519 and
sigstore-keyless are additive backends. The one thing Tessera can't provide is the *production trust
root* — that's your PKI / HSM / CA.

## FAIR discovery

`tessera export` emits an **RO-Crate 1.1** JSON-LD record and a schema-complete **DataCite** record from
the manifest (for a DOI mint / data-portal landing page). The deposit to a live InvenioRDM instance is a
downstream integration, but the records themselves are produced from the sealed product.
