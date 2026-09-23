#!/usr/bin/env bash
# Enforce that every GitHub Action is pinned to a full 40-character commit SHA.
#
# Replaces `uv run check-action-pins`, whose Python entry point no longer exists in this repo
# (the fd5-era package was removed at graduation; there is no pyproject.toml or uv.lock). The
# hook therefore failed on every workflow edit.
#
# A tag or branch ref (`uses: actions/checkout@v4`) is mutable: whoever controls the tag can
# change what runs in CI after review. A SHA is not. Local `uses: ./...` and reusable-workflow
# refs within this repo are exempt.
set -euo pipefail

shopt -s nullglob
files=(.github/workflows/*.yml .github/workflows/*.yaml .github/actions/*/action.yml .github/actions/*/action.yaml)
[ "${#files[@]}" -eq 0 ] && exit 0

status=0
for f in "${files[@]}"; do
  while IFS= read -r line; do
    lineno="${line%%:*}"
    content="${line#*:}"

    # The value after `uses:`, minus any trailing comment.
    ref="$(printf '%s' "$content" | sed -E 's/.*uses:[[:space:]]*//; s/[[:space:]]*#.*$//' | xargs)"
    [ -z "$ref" ] && continue

    # Local composite actions / reusable workflows in this repo.
    case "$ref" in
      ./*|.github/*) continue ;;
    esac

    # Docker refs are pinned by digest, not SHA.
    case "$ref" in
      docker://*)
        if [[ "$ref" == *"@sha256:"* ]]; then continue; fi
        echo "$f:$lineno: docker action must be pinned by digest (@sha256:...) — got '$ref'" >&2
        status=1
        continue
        ;;
    esac

    if ! [[ "$ref" =~ @[0-9a-f]{40}$ ]]; then
      echo "$f:$lineno: action must be pinned to a full 40-char commit SHA — got '$ref'" >&2
      echo "    fix: gh api repos/<owner>/<repo>/commits/<tag> --jq .sha" >&2
      status=1
    fi
  done < <(grep -nE '^\s*(-\s*)?uses:' "$f" || true)
done

exit "$status"
