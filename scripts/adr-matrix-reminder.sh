#!/usr/bin/env bash
# Advisory pre-push nudge: an ADR changed in this push. Non-blocking (always exit 0) — the *hard*
# enforcement (every Accepted ADR cited in FEATURE-MATRIX) lives in `nix flake check`
# (guardrails-gates → check-adr-matrix.sh), which `--no-verify` can't bypass. This is just a human
# reminder for the judgment case: a *Proposed* ADR you may want to surface as an ○ row.
echo "ℹ  ADR(s) changed in this push."
echo "   • Accepted → must be cited in tessera/docs/FEATURE-MATRIX.md (enforced by nix flake check)."
echo "   • Proposed → roadmap; consider an ○ 'planned' row if it should be visible in the baseline."
exit 0
