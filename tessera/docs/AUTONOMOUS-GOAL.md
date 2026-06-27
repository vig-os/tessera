# Autonomous goal — full proven parity

This is the standing directive for an autonomous landing run. Paste the fenced block below as the
agent's goal (optionally prefixed with `/goal`). It starts at the empirical-overhead spike (#221) and
loops — measure → flip → implement → **independently verify** → repeat — terminating **only** on proven
full parity (every ADR implemented + Accepted, every FEATURE-MATRIX row real and test-backed, every gate
green, confirmed by a fresh-context audit) or a true external blocker.

The load-bearing part is the **anti-hallucination discipline**: every landing is re-verified by a
fresh-context subagent that re-derives "done" from the actual artifacts (tests that exist and pass,
`file:line`, gate output, ADR↔matrix↔code agreement) — a self-reported "done + green" is treated as an
unverified claim until an independent agent confirms it.

```
GOAL — Drive Tessera to FULL PROVEN PARITY, autonomously, never stopping until proven done.

Repo: vig-os/tessera · branch spike/tessera-core · worktree ~/worktrees/tessera-core.
Orient first (every cycle if context is fresh): docs/adr/README.md (ADR index 0020–0032),
tessera/docs/FEATURE-MATRIX.md, tessera/docs/SPEC.md, tessera/docs/ROADMAP.md, the open issues
(#203, #214–#221), and the memory file. The architecture is settled (ADRs 0026–0032); what
remains is measuring, implementing, and PROVING.

DEFINITION OF DONE (the only success-stop — ALL must hold, verified by a fresh agent, not by you):
  • every ADR 0020–0032 is Accepted with an as-built citation (0027 → Superseded by 0028; 0020's
    content_hash clause flipped when 0028's MMR ships), every supersession seam reconciled.
  • every FEATURE-MATRIX row is ✓ and backed by a NAMED, passing test + a green gate.
  • full coverage: unit + integration + conformance + doctest + trycmd; the mdBook how-to builds green.
  • `nix flake check` green; all guardrails gates green (incl. adr-matrix, derived-docs, doc-tests).
  • a FINAL fresh-context audit subagent confirms parity with ZERO hallucinations/gaps.

THE LOOP (repeat until DONE; this is "full circle"):
  1. START at #221: run the empirical-overhead spike SYNTHETIC-FIRST (committable/CI-safe) —
     measure A sparse dense-vs-COO crossover, B chunk-index/Merkle leaf granularity, C t_c temporal
     depth, (D) pyramid/projection. Write thresholds → SPIKE-RESULTS.md + back into the ADRs.
  2. Flip the validated ADRs Proposed→Accepted (reconcile seams with their stated triggers).
  3. Implement the next backlog item in dependency order: MMR content_hash + golden-corpus regen →
     chunk-index/pyramid/sidecars/fused pass → new schemas (dynamic_pet/diffusion_mri/multicontrast_mri)
     + ROI + trait-sets → spatial referencing (0030) → sparse COO (0031) → referencing descriptor (0032)
     → streaming-compaction completion (0026). Match SPEC; keep determinism + lossless invariants.
  4. Test it to the DONE bar: unit + integration + conformance; add a doctest and a trycmd/snapbox
     walkthrough (scaffold trycmd NOW — the CLI is real); the example becomes how-to.
  5. ANTI-HALLUCINATION REVIEW (mandatory, every landing): spawn a FRESH-CONTEXT subagent
     (code-reviewer or Explore, zero shared context) to INDEPENDENTLY verify the claim against the
     ACTUAL artifacts — does the named test exist and pass, does file:line match, does the gate output
     say what you claim, do ADR↔matrix↔code agree, is anything overstated/stubbed/`todo!`? Treat your
     own "done" as an UNVERIFIED claim until the reviewer re-derives it. If it finds drift, fabrication,
     or an overstatement: fix, then re-review before advancing. Never let a self-assessment stand alone.
  6. Commit (ONLY after a background `nix flake check` exits 0 — never commit+check in one command;
     `git -c commit.gpgsign=false commit --no-verify`), push, update FEATURE-MATRIX (tick the row with
     its proving test+gate — the adr-matrix gate enforces Accepted↔matrix), flip ADR status, close/
     comment the issue, update memory. Then loop to the next gap.

PROCESS (non-negotiable):
  • stay on spike/tessera-core; do NOT merge to main.
  • real DUPLET PET/CT (/mnt/HDD …DUPLET-Patients/) is MANUAL-bench-only, NEVER committed (no PHI).
  • DRY/SOLID, composition-over-inheritance, store-don't-compute, nature-not-rank, feature-by-presence,
    SSoT-derived, deterministic — the six invariants; new code must honour them.
  • boil the ocean: ship the complete thing (tests + docs), not a workaround or a "table it for later".

STOP ONLY FOR:
  • external blockers needing user creds/data: #209 cosign signing / OCI / WORM, WASM toolchain,
    real-PHI datasets — surface them in one line and KEEP WORKING on everything else.
  • a genuine architectural fork where new evidence CONFLICTS with a committed ADR — surface with the
    evidence and a recommendation.
  • DONE, as defined above and confirmed by the final fresh-context audit.
  Do not stop for fatigue, length, or "good enough". If work remains and nothing above is tripped,
  continue.

LEGIBILITY: self-pace; one-line status at each milestone; end EVERY turn with the
`※ recap: <state>. Next: <step>.` sentinel.
```

## How to use
- Kick off a run: paste the fenced block (optionally `/goal <block>`).
- Resume after a compaction or a new session: the block tells the agent to re-orient from the ADR
  index, FEATURE-MATRIX, SPEC, ROADMAP, the open issues, and memory — so it is self-bootstrapping.
- The run is **append-only on `spike/tessera-core`** and never merges to `main`; external-blocked items
  (#209 signing/OCI/WORM, WASM, real-PHI data) are surfaced, not forced.

## ⚠ Known structural blocker (discovered during the run — needs a user decision)
The DONE definition above is **internally inconsistent** and, as written, **cannot be satisfied by
autonomous work alone**. Criterion (2) requires *every* FEATURE-MATRIX row `✓`, but several rows are
exactly the **external blockers** the "STOP ONLY FOR" clause says to surface-and-skip:
- **signing** (#209 — needs cosign keys), **WORM** (object-lock storage), **OCI artifact** (a registry),
  **WASM bindings** (vortex/hdf5 don't target wasm32).

So the conjunction "every row ✓ **and** keep-working-not-blocking" can't both hold. **This is a fork only
the user can resolve** (surfaced repeatedly in the run; recorded in memory):
- **(a)** provide the externals (creds/registry/WORM/WASM toolchain) → those rows become buildable; or
- **(b)** rescope DONE criterion (2) to *every **autonomously-achievable** row* (mark signing/WORM/OCI/
  WASM "out of scope, pending creds") → the run can then converge.

Until (a) or (b), an honest run **cannot report all five criteria met**, no matter how much it builds.
The buildable remainder (ADR-0028 §5 fused pass · 0030 §3 export + §5 deformable pipeline · 0029 trait/
mixin sets · 0032 unified descriptor · large vendor ingest) continues toward the achievable subset. This
note changes **no** criteria — it only records the conflict at its source so it isn't re-discovered.

## ✅ RESOLUTION (2026-06-27, user chose HYBRID of (a)+(b))
The user re-scoped after auditing my "external-blocked" framing as too conservative — most of it is
nix-CI-mockable. **Decisions:** signing = scheme-agnostic envelope over `manifest_hash` with **ed25519 +
ssh-ed25519** backends + **ORCID** as `signer_identity` + **age/sops** key-at-rest (in-test keypairs;
only a *production trust identity* is external). WASM = **`tessera-core`→wasm32** (zero C deps — spine/
verify/proofs/referencing) + **Arrow-JS as the RS↔TS boundary** for columnar data. WORM/OCI = build the
**mechanism + nix-CI service mocks** (MinIO object-lock · zot registry); only *production* bucket/registry
external. ADR-0026 = **MC-synthetic listmode + shrink the ring below the file** (no PHI needed for
bounded-memory + determinism); only *cross-arch x86==ARM CI* external. Plus: **structured `tracing`
write-path observability** (guardrails logging/trace style — SSoT on `append_block`, then encoders).
**The irreducible externals shrank to four:** a production signing identity · a production WORM bucket ·
a production registry · an ARM CI runner. Everything functional is now autonomously buildable + nix-tested.
Program tracked as tasks #26–30; first landing `52133c7` (write-path tracing).

## ✅ TERMINAL AUTONOMOUS STATE (2026-06-27, ~100 increments) — criteria 1+2 at their autonomous max
The hybrid program shipped. **7 FEATURE-MATRIX rows flipped ✓ this run** (signing · pyo3+WASM bindings ·
WORM · OCI · metadata-first durable header · RO-Crate/DataCite · Ingest NIfTI+raw), each via
verify-before-build + audit-before-flip (the nix-service-mock pattern — a real `distribution` registry on
loopback — was established for OCI and is reusable for MinIO). **Precise residual, all five criteria:**
- **(1) ADRs:** of 0020–0032, **only ADR-0026 is non-terminal** (all others Accepted; 0027 Superseded;
  0021 doesn't exist). 0026's Accept gate is the determinism-critical external set: **cross-arch x86==ARM
  determinism re-validation + real >RAM listmode + golden regen**. Its §3 streaming-reader *mechanism* is
  buildable (transpose_2p/3p prepped) but the **flip** needs the ARM runner.
- **(2) Matrix:** 3 rows remain, **all externally/tradeoff-gated** — DICOM-JPEG (needs the C++ codecs
  charls/gdcm the flake deliberately disabled for hermeticity) · cross-arch determinism (ARM runner) ·
  GE-HDF5 7GB stream (= 0026 + cross-arch).
- **(5) final audit:** gated on (1)+(2).
**So criteria 1, 2, 5 now converge on the SAME irreducible externals:** an **ARM CI runner** (the big one —
unblocks 0026 + the cross-arch row + the GE-HDF5 stream flip), the **deliberately-disabled C++ DICOM
codecs** (a hermeticity tradeoff to reverse, for DICOM-JPEG), and **real >RAM/PHI data** (0026 scale
validation). Criteria 3+4 met throughout. **The autonomous build has reached its proven ceiling** — every
ADR clause and matrix row that does NOT need one of those is done. Converging the five-criteria DONE now
needs the user to provide the ARM runner / re-enable the C++ codecs / supply >RAM data (or rescope those
rows out). This is not a stall; it is completion of the autonomously-achievable scope.
