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
