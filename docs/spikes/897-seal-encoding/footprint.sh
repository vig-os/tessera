#!/usr/bin/env bash
# Spike #897: how much of the tree touches the seal and JCS (the "Cost" section's footprint numbers).
# Run from this directory; prints to stdout. Counts are over tracked files only.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
src() { git ls-files 'tessera/crates/*.rs' 'tessera/corpus/*.py'; }
count() { # pattern: occurrences, matching lines, files, crates
  local pat=$1
  printf '%-22s occurrences %4d · lines %4d · files %3d · in: %s\n' "$pat" \
    "$(src | xargs grep -ohE "$pat" | wc -l)" \
    "$(src | xargs grep -hE "$pat" | wc -l)" \
    "$(src | xargs grep -lE "$pat" | wc -l)" \
    "$(src | xargs grep -lE "$pat" | awk -F/ '{print $3}' | sort -u | paste -sd ' ')"
}
count 'manifest_hash'
count 'canonical::'
count 'manifest\.json'
printf 'test snapshots (.trycmd, .stdout, .json, .toml under tests/) holding a blake3: value: %d files\n' \
  "$(git ls-files 'tessera/crates/*/tests/*' | grep -vE '\.(rs|tsra)$' | xargs grep -l 'blake3:' | wc -l)"
printf 'ADRs mentioning manifest_hash or JCS: %d; SPEC lines: %d\n' \
  "$(grep -lE 'manifest_hash|JCS' docs/adr/0*.md | wc -l)" "$(grep -cE 'manifest_hash|JCS' tessera/docs/SPEC.md)"
printf 'tracked .tsra: %d; distinct manifest.json contents: %d\n' "$(git ls-files '*.tsra' | wc -l)" \
  "$(git ls-files '*.tsra' | while read -r f; do unzip -p "$f" manifest.json | md5sum; done | sort -u | wc -l)"
