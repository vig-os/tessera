#!/usr/bin/env bash
# Spike #897: regenerate every result in results/ from the tracked .tsra files.
# Run from this directory inside the repo dev shell (`nix develop` at the repo root), which supplies
# cargo, uv and node. Exit status is non-zero if any step fails.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p results out
# test order and wall time vary between runs; sort and drop the time so the file is reproducible
cargo test --release 2>&1 | grep -E '^test |^test result' | sed 's/; finished in .*//' | sort > results/unit-tests.txt
cargo run --release -q -- corpus > results/corpus.md
cargo run --release -q -- attacks > results/attacks.md
cargo run --release -q -- numbers > results/numbers.txt
cargo run --release -q -- libprobe > results/libprobe.md
cargo run --release -q -- vectors out/vectors > results/vectors.txt
cargo run --release -q -- products out/products > results/products.txt
cargo run --release -q -- floats out/vectors/floats.tsv > /dev/null
uv run -q py/agree.py out/vectors > results/agree-python.txt
uv run -q py/premortem.py out/vectors > results/premortem-python.txt
(cd js && npm ci --no-audit --no-fund --ignore-scripts > /dev/null)
node js/agree.mjs out/vectors > results/agree-js.txt
{ uv run -q py/floats.py out/vectors/floats.tsv; node js/floats.mjs out/vectors/floats.tsv; } > results/floats.txt
./footprint.sh > results/footprint.txt
echo "results/ regenerated"
