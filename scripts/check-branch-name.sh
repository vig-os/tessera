#!/usr/bin/env bash
# Branch-name gate — the LOCAL half of the rule CI enforces (#499).
#
# This exists because the two halves had diverged. The previous local hook was a
# `no-commit-to-branch --pattern` that accepted `<type>/<slug>` with NO issue number, while CI's
# `Validate branch name` step requires `<type>/<issue>-<summary>` for every type but `chore`. A branch
# could therefore pass every local gate and fail only after a ~45-minute CI leg — the "passes locally /
# fails in CI" drift this repo's tooling argues against everywhere else.
#
# The clause list below mirrors `.github/workflows/ci.yml`'s `ALLOWED` exactly, clause for clause. If
# you change one, change both — and prefer changing CI first, since that file is devkit-managed and a
# local-only relaxation would be silently reverted on the next scaffold upgrade.
#
# The TYPE LIST is not duplicated: it is read from `.vig-os`'s `DEVKIT_BRANCH_TYPES`, the same key CI's
# resolve-toolchain action reads, so the two gates cannot drift on which types exist.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
manifest="${repo_root}/.vig-os"

types=""
if [ -f "${manifest}" ]; then
  types="$(sed -n 's/^DEVKIT_BRANCH_TYPES=//p' "${manifest}" | tr -d '[:space:]' | head -1)"
fi
# devkit's stock set, used when the key is absent or deliberately empty (it resolves to this default
# at scaffold time, so an empty key means "the default", not "no types").
[ -n "${types}" ] || types="feature,bugfix,hotfix,release,docs,test,refactor"
alternation="${types//,/|}"

branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
# Detached HEAD, a rebase in progress, or a fresh repo with no commits: there is no branch name to
# judge, and failing here would block legitimate mid-rebase commits.
if [ -z "${branch}" ] || [ "${branch}" = "HEAD" ]; then
  exit 0
fi

allowed="^(main|dev)\$"
allowed+="|^chore/[a-z0-9]+(-[a-z0-9]+)*\$"
allowed+="|^(${alternation})/[0-9]+-[a-z0-9]+(-[a-z0-9]+)*\$"
allowed+="|^worktree/[0-9]+\$"
allowed+="|^renovate/.+\$"
allowed+="|^dependabot/.+\$"
allowed+="|^release/[0-9]+\\.[0-9]+\\.[0-9]+\$"

if printf '%s' "${branch}" | grep -qE "${allowed}"; then
  exit 0
fi

cat >&2 <<EOF
Branch name '${branch}' does not follow the convention.

Accepted (identical to the CI gate in .github/workflows/ci.yml):
  <type>/<issue>-<summary>   e.g. fix/356-encode-trace-race   types: ${types}
  chore/<summary>            for deliberately issue-less maintenance
  release/X.Y.Z              release trains
  worktree/<issue>           autonomous worktree pipeline
  main, dev                  long-lived branches
  renovate/*, dependabot/*   bot namespaces

The issue number is REQUIRED for every type except chore. That is CI's rule, and this hook now
mirrors it so you find out here instead of after a 45-minute CI leg (#499).

Rename with:  git branch -m <new-name>
Note: renaming a branch that already has an open PR CLOSES that PR, so rename before pushing.
EOF
exit 1
