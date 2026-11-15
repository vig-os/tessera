# shellcheck shell=bash
# Release-tag SemVer helpers, sourced (never executed) by scripts/last-release-tag.sh (#606) and
# scripts/promotion-plan.sh (#854), so both read "a release tag" and "which tag is higher" the same way.

# A `v`-prefixed SemVer core with an optional pre-release; build metadata is not used by our tags.
# shellcheck disable=SC2034 # used by the scripts that source this file
TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

# SemVer sort of bare versions on stdin, ascending. GNU `sort -V` follows Debian ordering, where
# `~` sorts before everything, even the end of the string; mapping the pre-release `-` to `~` makes
# 0.1.0~alpha.1 < 0.1.0, which is SemVer's rule. Numeric runs compare numerically (alpha.2 < alpha.10).
# Exact for our `<word>.<number>` pre-releases (alpha.N, beta.N, rc.N); NOT for SemVer's mixed rule
# that numeric identifiers sort below alphanumeric ones (sort -V puts alpha.beta before alpha.1).
semver_sort() {
  sed 's/-/~/' | sort -V | sed 's/~/-/'
}

# Whether version $1 <= version $2 in SemVer order.
semver_le() {
  [ "$(printf '%s\n%s\n' "$1" "$2" | semver_sort | tail -n 1)" = "$2" ]
}
