#!/usr/bin/env bash
# Enforce the skill-directory naming convention.
#
# The hook referenced this path but the script was never committed, so any edit under
# .cursor/skills/ failed the commit with "No such file or directory".
#
# Convention, derived from the directories that exist today: lowercase alphanumeric segments
# separated by `-` or `_` (e.g. `ci_check`, `pr_post-merge`, `worktree_solve-and-pr`,
# `solve-and-pr`). No uppercase, no spaces, no leading/trailing separator.
#
# Usage: check-skill-names.sh <skills-dir>
set -euo pipefail

dir="${1:?usage: check-skill-names.sh <skills-dir>}"
[ -d "$dir" ] || exit 0

status=0
for path in "$dir"/*/; do
  [ -d "$path" ] || continue
  name="$(basename "$path")"
  if ! [[ "$name" =~ ^[a-z0-9]+([-_][a-z0-9]+)*$ ]]; then
    echo "$dir/$name: invalid skill directory name" >&2
    echo "    expected lowercase alphanumeric segments separated by '-' or '_'" >&2
    status=1
  fi
done

exit "$status"
