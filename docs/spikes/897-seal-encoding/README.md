# Spike #897 prototypes: seal encoding bake-off (A vs A′ vs B)

Evidence for [`../897-seal-encoding.md`](../897-seal-encoding.md) and the proposed
[ADR-0066](../../adr/0066-seal-encoding.md). **Spike code, not product code**: a standalone crate outside the
tessera workspace (its own empty `[workspace]` table), plus Python and JavaScript readers. Nothing here is
built or tested by the repository's flake checks.

## What each option is in the prototype

| Option | Stored manifest | Seal | Verifier (`src/options.rs`) |
|---|---|---|---|
| **A** | `manifest.json`, any JSON spelling, carries `manifest_hash` | `digest(JCS(parsed − manifest_hash))` | tessera-core at PR #894's head (`ea2e47a`, alpha.6 + I-JSON integers), pinned as a git dependency |
| **A′** | `manifest.json` = exactly `JCS(manifest)`, no `manifest_hash` key | `digest(stored bytes)`, in a `manifest.seal` member | strict parse → refuse bytes ≠ their own JCS → compare seal → tessera-core's typed layer |
| **B** | `manifest.cbor` = profile CBOR (`src/cbor.rs`), no `manifest_hash` key | `digest(stored bytes)`, in a `manifest.seal` member | own strict decoder → compare seal → tessera-core's typed layer through a JSON bridge |
| **B0** | as B, without numeric reduction | as B | kept only to show what the reduction rule buys (S7, JS round trips) |

The typed layer (declared-form check, unique block names, `id` and `content_hash`) is tessera-core's own for
every option, so the comparison isolates the encoding and the place of the seal.

## Run everything

From the repository root, inside the dev shell (it supplies cargo, uv and node):

```sh
export NIX_CONFIG="experimental-features = nix-command flakes"
nix develop -c bash -c 'cd docs/spikes/897-seal-encoding && ./run.sh'
```

`run.sh` regenerates every file in `results/` and exits non-zero if any step fails. It writes scratch
vectors and the two demo products to `out/` (ignored by git). Individual steps:

| Command | Writes | What it shows |
|---|---|---|
| `cargo test --release` | `results/unit-tests.txt` | the profile encoder/decoder at the 64-bit edges, preferred floats, numeric reduction, and every refused form |
| `cargo run --release -- corpus` | `results/corpus.md` | the 13 tracked `.tsra` under each option: verify, A′ seal = A seal = golden, B round trip, sizes, `id` slice |
| `cargo run --release -- attacks` | `results/attacks.md` | the attack replay (19 attacks × 13 products × 3 options), with verbatim verdicts for `recon_int16` |
| `cargo run --release -- numbers` | `results/numbers.txt` | what `serde_jcs` writes for integers beyond 2^53 |
| `cargo run --release -- libprobe` | `results/libprobe.md` | whether `minicbor` 2.3.1 and `cbor4ii` 1.2.5 refuse non-deterministic CBOR on their own |
| `cargo run --release -- vectors out/vectors` | `results/vectors.txt` | byte vectors for the Python and JS readers; the Rust strict decoder's verdict on 11 bad inputs |
| `cargo run --release -- products out/products` | `results/products.txt` | the A′ and B demo products used for the legibility test |
| `cargo run --release -- floats out/vectors/floats.tsv` | (input to the next two) | 200,000 doubles with their JCS text and profile CBOR bytes |
| `uv run py/agree.py out/vectors` | `results/agree-python.txt` | Python (`json`, `jcs` 0.2.1, `cbor2` 6.3.0, `blake3`) on the same bytes |
| `uv run py/premortem.py out/vectors` | `results/premortem-python.txt` | what third-party writers emit without the reference library |
| `uv run py/floats.py …` and `node js/floats.mjs …` | `results/floats.txt` | float formatting agreement across Rust, Python and JS |
| `node js/agree.mjs out/vectors` | `results/agree-js.txt` | node 24 (`JSON.parse`, `canonicalize` 5.1.0, `cbor-x` 1.6.9, `@noble/hashes`) on the same bytes |

`./footprint.sh` writes `results/footprint.txt`: how many tracked files touch `manifest_hash`, `canonical::` and
`manifest.json` (the cost section's numbers).

`results/legibility.md` is the one hand-recorded result: two fresh agents, each given one demo product and five
fixed questions (see that file for the brief and the verbatim reports).

The JavaScript dependencies are pinned in `js/package-lock.json`; `run.sh` installs them with `npm ci
--ignore-scripts`, so `cbor-x` runs its pure-JS decoder (the optional native `cbor-extract` addon is not built).
The Python dependencies are pinned in each script's inline metadata (PEP 723).

## Build directory

The crate builds into whatever `CARGO_TARGET_DIR` is set (on the shared seat: the slot dir named in the task
brief). It compiles tessera-core from git at the pinned commit, so it never touches the workspace's own build.
