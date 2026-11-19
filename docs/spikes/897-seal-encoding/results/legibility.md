# Legibility test (2026-11-17)

Two fresh agents, run in parallel, each given one file and no documentation (A′:
`out/products/aprime.tsra`, B: `out/products/b.tsra`, both copied to `product.tsra` in an empty
directory). Tools allowed: unzip, jq, python3 (stdlib), file, xxd, od, date; packages only via `uv run --with`
or `pip install --user`, which the agent had to record. The five questions were fixed and identical. The
brief told them not to read the repository and not to search the web.

Ground truth: `recon` / `recon-int16` / `2024-01-01T00:00:00Z`; one block `volume`, `array`, shape
`[64, 64, 64]`, `int16`; `daq_start_ns` = `1730000000123456789` = `2024-10-27T03:33:20.123456789Z`; required
field `modality`; the seal matches.

| | A′ (`manifest.json`, JCS) | B (`manifest.cbor`, deterministic CBOR) |
|---|---|---|
| Correct answers | 5/5 | 5/5 |
| Elapsed (agent's own `date +%s.%N`) | 36.1 s | 35.0 s |
| Shell commands | 8 | 8 |
| Packages installed | `blake3` (question 5 only) | `cbor2` (to read anything at all) and `blake3` |
| How the manifest was read | `unzip -p … manifest.json \| jq` | `cbor2`, after reading `aux/README` |

## A′ report, verbatim

> - ANSWERS:
>   1. Product type `recon` (a reconstructed image volume, per the embedded schema's description); name
>      `recon-int16`; timestamp `2024-01-01T00:00:00Z`.
>   2. One block: name `volume`, kind `array`, shape `[64, 64, 64]` on axes `[z, y, x]`, dtype `int16` (codec
>      `pcodec`, unit `HU`, chunks `[64,64,64]`).
>   3. `daq_start_ns` = `1730000000123456789` (stored as a JSON string, so exact) = 2024-10-27
>      03:33:20.123456789 UTC.
>   4. Required schema fields: only `modality`.
>   5. Yes — BLAKE3 of the 3825-byte `manifest.json` =
>      `6e029e5020940b5f9237bdea6dd18e7e2a80ffc43634857da942985aa4e61e07`, matching `manifest.seal` exactly.
> - START/END: start 1794947477.404112638, end 1794947513.536084713, elapsed 36.131972075 s.
> - COMMANDS: 8 shell commands.
> - ERRORS: `python3 -c "hashlib.new('blake3')"` printed `no builtin blake3: unsupported hash type blake3`
>   (expected probe). All other commands succeeded.
> - FRICTION: The archive is a plain zip with a self-describing `mimetype` (`application/vnd.tessera`),
>   `manifest.json`, a `manifest.seal` sidecar, and one `blocks/volume` payload — jq handles everything. The
>   only hurdle was BLAKE3: stdlib hashlib lacks it, so I installed the `blake3` package via
>   `uv run --with blake3` to verify the seal.
> - CONFIDENCE: High that `daq_start_ns` is exact. The raw JSON stores it as a string `"1730000000123456789"`
>   (not a bare JSON number), so it bypasses any IEEE-754 double rounding; the schema also declares the field
>   as `int64`, and 1730000000123456789 exceeds 2^53 so a float decode would have lost precision — the string
>   encoding is clearly deliberate and the value is byte-for-byte what the producer wrote.

## B report, verbatim

> ANSWERS:
>
> 1. Product type: `recon` (a reconstructed image volume); name: `recon-int16`; timestamp:
>    `2024-01-01T00:00:00Z`.
> 2. Blocks: one block — name `volume`, kind `array`, shape `[64, 64, 64]` (axes z,y,x), dtype `int16`.
> 3. `daq_start_ns` = `1730000000123456789` → UTC `2024-10-27T03:33:20.123456789Z`.
> 4. Required schema fields: `modality` (only).
> 5. Yes — computed BLAKE3 of `manifest.cbor` =
>    `blake3:276427d6044dc1ef5dd1553bbf5df808988bfda20fef51807aa5d3adcd8e14e8`, equal to `manifest.seal`.
>
> START/END: `1794947483.760169925` / `1794947518.740271857`; elapsed ≈ `34.98` seconds.
>
> COMMANDS: 8 shell commands.
>
> ERRORS: one — `python3 -c "..."` printed `<string>:6: DeprecationWarning:
> datetime.datetime.utcfromtimestamp() is deprecated and scheduled for removal in a future version. Use
> timezone-aware objects to represent datetimes in UTC: datetime.datetime.fromtimestamp(timestamp,
> datetime.UTC).` (warning only; computation succeeded).
>
> FRICTION: Minimal. The file is a ZIP with a `mimetype` stub (`application/vnd.tessera`), a deterministic
> CBOR `manifest.cbor`, a BLAKE3 `manifest.seal`, and a plain-text `aux/README` that names the manifest format.
> I installed two Python packages via `uv run --with ...`: `cbor2` (to decode the manifest) and `blake3` (to
> verify the seal).
>
> CONFIDENCE: High — the CBOR decoder returned a native Python `int` (`1730000000123456789`) for
> `daq_start_ns`, Python ints are arbitrary precision, and the schema declares the field as `int64`, so no
> float rounding was in play; the ns remainder (`123456789`) was separated by integer division before
> formatting.

## Reading

- Both formats were legible to an agent **with network access to a package index**. The difference is where
  the dependency sits: A′ needs one only to check the seal (BLAKE3, the same in every option); B needs one to
  read a single field. Offline (inferred from what each agent installed, not run), A′ still answers questions
  1–4 with `jq` and B none without a hand-written CBOR decoder.
- The B agent found the decoder through the unsealed `aux/README` pointer. A zip archive comment cannot carry
  that pointer: SPEC §6 requires an empty comment (a comment can hide a second archive), so an `aux/` member
  is the only place, and it is unauthenticated (ADR-0042).
- n = 1 per option, one capable agent; this measures "can it be done and what does it need", not speed.
- The B file the agent read was encoded without numeric reduction (profile B0, seal `blake3:276427d6…`), before
  the spike added that rule; `run.sh` now writes the reduced encoding. None of the five answers touches a float.
- An agent in the same position cannot decode CBOR at all without a package, so the B number depends on
  network access; the A′ number does not (BLAKE3 aside, which every option shares).
