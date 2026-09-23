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
