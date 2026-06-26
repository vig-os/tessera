#!/usr/bin/env bash
# guardrails gate: every **Accepted** ADR (per docs/adr/README.md) must be cited in FEATURE-MATRIX.md.
#
# The FEATURE-MATRIX is hand-maintained (no generator), so `derived-docs` can't catch it drifting.
# This keeps the one-page baseline honest for *decided/shipped* work without being noisy: it keys on
# the ADR **status** (Accepted), not on raw edits, so Proposed ADRs (roadmap) and typo fixes never trip
# it. Run from the repo root. Exempt as-built *decision* ADRs (not feature rows) via $ADR_MATRIX_EXEMPT.
set -euo pipefail

README="${1:-docs/adr/README.md}"
MATRIX="${2:-tessera/docs/FEATURE-MATRIX.md}"
EXEMPT="${ADR_MATRIX_EXEMPT:-0002 0003}" # concurrency / schema-id — recorded decisions, not features

missing=""
while IFS= read -r line; do
  # README index rows look like:  | [0024](0024-....md) | description | **Accepted** |
  num="$(printf '%s' "$line" | grep -oE '\[0[0-9]{3}\]' | head -1 | tr -dc '0-9')" || true
  [ -n "$num" ] || continue
  printf '%s' "$line" | grep -qi 'Accepted' || continue # only Accepted ADRs are required
  case " $EXEMPT " in *" $num "*) continue ;; esac      # skip exempt decision ADRs
  grep -q "ADR-$num" "$MATRIX" || missing="$missing $num"
done <"$README"

if [ -n "$missing" ]; then
  echo "✗ ADR↔matrix: Accepted ADR(s) with no FEATURE-MATRIX citation:$missing" >&2
  echo "  → add a row citing ADR-00NN, or add the number to ADR_MATRIX_EXEMPT (non-feature decision)." >&2
  exit 1
fi
echo "✓ ADR↔matrix: every Accepted ADR is cited in FEATURE-MATRIX"
