#!/usr/bin/env bash
# Gate B — the feature-snapshot determinism gate (ADR-0057 §5).
#
# Gate A (Phase 1) tells you the day the goldens moved. Gate B tells you the day the *risk* was
# introduced, on the PR that introduced it: it snapshots the resolved feature set of every crate on
# the **seal path** and fails when one changes.
#
# Why this matters concretely (ADR-0057 §5, "the hazard the probe actually found"): the optional,
# ingest-unrelated `sql` feature already turns on `arrow-array/chrono-tz` — ADR-0056 hazard H1
# (tzdb as a compiled-in vs. filesystem dependency), rated *high × fatal*, arriving through a
# feature instead of through a host. The same class of hazard is worse on the Vortex side: the
# workspace pins `vortex-btrblocks = { features = ["pco"] }` to *exclude* ALP, which was not
# byte-deterministic (#380/#384). Feature unification is monotonic — it can only add — so any
# future dependency that transitively enables `vortex-btrblocks/alp` silently re-encodes every
# float column in every table.
#
# A changed snapshot is NOT automatically wrong. It is a **deliberate corpus event**: the PR must
# either show that no golden moved (and update the snapshot with a stated reason) or carry the
# corpus regeneration alongside.
#
# Usage:
#   scripts/feature-snapshots.sh <OUT_DIR>
#
# Regenerate the committed baseline (run from the repo root, inside `nix develop`):
#   scripts/feature-snapshots.sh tessera/tests/feature-snapshots
#
# The `feature-snapshots` flake check runs this into a temp dir and `diff`s it against the
# committed baseline — the hermetic equivalent of `git diff --exit-code`.
set -euo pipefail

out_dir="${1:-}"
if [ -z "${out_dir}" ]; then
  echo "usage: $0 <OUT_DIR>" >&2
  exit 2
fi

# The seal path: everything that can move sealed bytes. Hand-maintained — ADR-0057 names deriving
# this from the dependency graph as an open gap. Adding a codec crate to the write path without
# adding it here leaves a blind spot.
#
#   blake3                            content_hash / manifest_hash
#   zarrs pco zstd                    the array block backend + its codecs
#   vortex-btrblocks vortex-file      the table block backend (compressor + writer)
#   vortex-array vortex-buffer        the table block in-memory representation
#   arrow-array arrow-buffer arrow-schema
#                                     the shared arrow tree the ADR-0056 table lane canonicalises
#                                     through (and where `sql` flips `chrono-tz` today)
#
# NB: ADR-0057 §5 spells the pcodec entry `pcodec` — that is the *project* name; the crate on
# crates.io (and in `Cargo.lock`) is `pco`, so that is what `cargo tree -i` can be given. Same
# codec, and it is the one the workspace pins `vortex-btrblocks/pco` to reach.
CRATES=(
  blake3
  zarrs
  pco
  zstd
  vortex-btrblocks
  vortex-file
  vortex-array
  vortex-buffer
  arrow-array
  arrow-buffer
  arrow-schema
)

# Determinism of the invocation itself:
#   LC_ALL=C           `sort` collation must not depend on the developer's locale.
#   --charset ascii    the tree glyphs must not depend on a UTF-8 terminal.
#   --locked           resolve against the committed Cargo.lock, never re-resolve.
#   --offline          no network: the hermetic nix check has none, and a registry fetch could
#                      silently change what is being snapshotted.
#   --all-features     ADR-0057 §5 — snapshot the *widest* graph, including `static-hdf5` (which
#                      the PR clippy gate no longer builds) and `sql`, so a drift anywhere in the
#                      feature space is visible even though no single build enables all of it.
#   -e features        feature edges, not just package edges — the whole point of Gate B.
#   -i <crate>         invert: who pulls this crate in, and with which features.
#   --target all       cargo tree otherwise resolves cfg() against the HOST triple, which would make
#                      the snapshot host-dependent — and CI runs `nix flake check` on x86_64-linux
#                      AND aarch64-linux, so a target-gated dependency appearing on one and not the
#                      other would fail the gate on one arch for no determinism reason. All 11
#                      snapshots are byte-identical under host / aarch64 / all today; this keeps
#                      them that way by construction rather than by luck.
export LC_ALL=C

# The cargo workspace root. Normally `<repo>/tessera`, derived from this script's own location so a
# developer can run it from anywhere. The `feature-snapshots` flake check sets `TESSERA_WORKSPACE`
# instead, because there the script lives in the nix store while the sources are unpacked in $PWD.
workspace_dir="${TESSERA_WORKSPACE:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../tessera" && pwd)}"

mkdir -p "${out_dir}"
out_dir="$(cd -- "${out_dir}" && pwd)"

# Two normalisations, so the snapshot records the *dependency graph* and nothing else. Both are
# lossless for Gate B's purpose — neither can hide a feature change:
#
#  1. `cargo tree` prints path-dependencies as `tessera-io v… (/abs/path/to/crates/tessera-io)`.
#     That absolute path differs between a developer's checkout, a git worktree, and the nix
#     sandbox's `/build/source`, which would make the gate fail for everyone but its author.
#
#  2. Workspace members share one `version.workspace` (`0.1.0-alpha.1` today), which release-plz
#     bumps on every release. A software version bump cannot change an encoding — it is not a
#     corpus event — so pinning it to `vWORKSPACE` keeps release PRs out of Gate B's face. The
#     versions that *can* move bytes are the third-party ones, and those stay verbatim.
normalise() {
  sed -E \
    -e 's# \(/[^)]*\)##g' \
    -e 's#(tessera-(core|io|cli|ingest|py|wasm)) v[0-9][^ ]*#\1 vWORKSPACE#g'
}

cd "${workspace_dir}"
for c in "${CRATES[@]}"; do
  cargo tree --quiet --locked --offline --all-features --charset ascii --target all \
    -e features -i "${c}" \
    | normalise \
    | sort > "${out_dir}/${c}.txt"
done

echo "wrote ${#CRATES[@]} feature snapshots to ${out_dir}"
