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
have to reach into the container format to read it. It is exactly the serialized manifest, so two
things are worth knowing before you parse it:

- **`producer` is polymorphic.** An object (`{tool, version, git_commit?, git_repo?, dirty?}`) on
  anything sealed since ADR-0058, and a bare string (`"tessera/0.0.0"`) on older products, which
  round-trip unchanged. Accept both. Optional fields (`generation`, `study`, `schema`, the optional
  producer keys) are omitted when unset rather than emitted as `null`.
- **Its stability is the format's, not the CLI's.** The dump carries `tessera_version`, the spec
  version; fields are added compatibly (that is how `producer` and `generation` arrived), while a
  rename or removal is a major break, and a reader refuses a manifest whose major exceeds its own.
  Key your parser off `tessera_version`, not off the tessera binary's version.

## Reading the recipe, and walking the chain

`inspect` prints `generation.config_ref` as a digest. `inspect --resolve-config` dereferences it to the
config the product was actually generated with, so the recipe reads end to end rather than ending at a
hash you have to chase by hand. A small text config is printed inline; a large or non-UTF-8 one is named
along with the `tessera extract` command that streams it — and which of the two happens is decided from
the Blob spec's recorded `size`, so nothing is buffered to find out. A `config_ref` that names no carried
block is a **sealed reference that does not hold**, and is reported as a typed error; plain `inspect` on
the same file keeps working, because the product is otherwise valid.

`sources[]` answers *which parent* for one product. `tessera provenance` answers the chain question:
what identity did this inherit, from where, and what recipe made each hop. It walks `derived_from` back
toward the roots and prints the producer, the recipe and the shared identity fields per hop.

Parents live in other files, so resolution is explicit and **local**: the product's own directory first,
then an explicit `--collection`, then each `--search` directory. There is no registry resolver — a read
verb should not reach the network, so a parent that exists only in a registry reads as unresolved until
you fetch it.

What the walk reports per hop, and whether it counts against `--require-complete`:

| Outcome | Gap? | Meaning |
| --- | --- | --- |
| `pinned` | no | the parent on disk is the exact version the edge committed to |
| `different version` | yes | the right lineage, another version — see below |
| `unresolved` | yes | a product-shaped reference nothing was found for |
| `CORRUPT` | yes | the parent is on disk but its manifest does not verify |
| `no pinned version` | yes | names a parent but pins no version, so nothing to prove against |
| `external leaf` | no | a vendor path, filename or SOP UID — where a chain *should* end |
| `version not present` | no | a `snapshot_of` breadcrumb whose version is absent |

The last two are deliberately not gaps. A real chain ends at vendor files, and `publish` deliberately
drops history, so counting either would leave every product permanently "incomplete" and make the flag
worthless.

**Corruption is not opt-in.** A candidate file that *is* a Tessera product but whose manifest fails
verification always exits nonzero, with or without `--require-complete`, and is reported **on the edge
that needed it** — the hop reads `CORRUPT` and names the file, rather than calling the parent absent. The
two states invite opposite responses: "I could not find that parent" sends you looking for another copy,
while "what I found does not verify" ends the search and starts an incident. A file that is not a Tessera
product at all is ignored in silence, so pointing `--search` at a real directory stays practical.

Naming the file means reading the identity it *claims*, which is why `tessera-io` exposes
`read_manifest_unverified`. What it returns is untrusted by construction — a claim by something that has
already failed verification — and is used only to point at the file, never to satisfy the edge.

A version pointer earns its exemption from **three** checks, not one: the reference equals the pinned
hash, the role is the one `publish` writes, and — once it resolves — the parent is in the walked product's
own lineage. Shape alone is forgeable, and skipping both descent and completeness is too much to hand to
any edge that can be crafted to look the part.

A gap is **rendered, not raised**: the exit stays 0, because a chain you cannot resolve from one
directory is the normal case and a verb that failed on it would be useless in a pipeline. Pass
`--require-complete` for the gate, and `--json` for a machine-readable walk whose `outcome` tokens are
stable. A genuine cycle is a malformed DAG and always exits nonzero.

The distinction the taxonomy exists to protect is the second row. A metadata correction on a parent is
routine, and reporting it as an integrity failure would cry corruption over an edit — after which
operators learn to wave away the one error that must never be waved away. So chain verification raises
`ProvenanceVersionSkew`, never `Integrity`; real corruption is caught by payload verification
(`tessera verify`), a separate mechanism that reads the actual bytes.

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/provenance-walk.trycmd}}

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
