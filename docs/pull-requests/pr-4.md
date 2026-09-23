---
type: pull_request
state: open
branch: dependabot/github_actions/dev/actions/upload-artifact-6.0.0 → dev
created: 2026-02-24T19:13:42Z
updated: 2026-03-30T09:20:27Z
author: dependabot[bot]
author_url: https://github.com/dependabot[bot]
url: https://github.com/vig-os/tessera/pull/4
comments: 0
labels: dependencies, github_actions
assignees: none
milestone: none
projects: none
relationship: none
synced: 2026-08-04T10:44:58.631Z
---

# [PR 4](https://github.com/vig-os/tessera/pull/4) ci(deps): bump actions/upload-artifact from 4.6.2 to 6.0.0

Bumps [actions/upload-artifact](https://github.com/actions/upload-artifact) from 4.6.2 to 6.0.0.
<details>
<summary>Release notes</summary>
<p><em>Sourced from <a href="https://github.com/actions/upload-artifact/releases">actions/upload-artifact's releases</a>.</em></p>
<blockquote>
<h2>v6.0.0</h2>
<h2>v6 - What's new</h2>
<blockquote>
<p>[!IMPORTANT]
actions/upload-artifact@v6 now runs on Node.js 24 (<code>runs.using: node24</code>) and requires a minimum Actions Runner version of 2.327.1. If you are using self-hosted runners, ensure they are updated before upgrading.</p>
</blockquote>
<h3>Node.js 24</h3>
<p>This release updates the runtime to Node.js 24. v5 had preliminary support for Node.js 24, however this action was by default still running on Node.js 20. Now this action by default will run on Node.js 24.</p>
<h2>What's Changed</h2>
<ul>
<li>Upload Artifact Node 24 support by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/719">actions/upload-artifact#719</a></li>
<li>fix: update <code>@​actions/artifact</code> for Node.js 24 punycode deprecation by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/744">actions/upload-artifact#744</a></li>
<li>prepare release v6.0.0 for Node.js 24 support by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/745">actions/upload-artifact#745</a></li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://github.com/actions/upload-artifact/compare/v5.0.0...v6.0.0">https://github.com/actions/upload-artifact/compare/v5.0.0...v6.0.0</a></p>
<h2>v5.0.0</h2>
<h2>What's Changed</h2>
<p><strong>BREAKING CHANGE:</strong> this update supports Node <code>v24.x</code>. This is not a breaking change per-se but we're treating it as such.</p>
<ul>
<li>Update README.md by <a href="https://github.com/GhadimiR"><code>@​GhadimiR</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/681">actions/upload-artifact#681</a></li>
<li>Update README.md by <a href="https://github.com/nebuk89"><code>@​nebuk89</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/712">actions/upload-artifact#712</a></li>
<li>Readme: spell out the first use of GHES by <a href="https://github.com/danwkennedy"><code>@​danwkennedy</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/727">actions/upload-artifact#727</a></li>
<li>Update GHES guidance to include reference to Node 20 version by <a href="https://github.com/patrikpolyak"><code>@​patrikpolyak</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/725">actions/upload-artifact#725</a></li>
<li>Bump <code>@actions/artifact</code> to <code>v4.0.0</code></li>
<li>Prepare <code>v5.0.0</code> by <a href="https://github.com/danwkennedy"><code>@​danwkennedy</code></a> in <a href="https://redirect.github.com/actions/upload-artifact/pull/734">actions/upload-artifact#734</a></li>
</ul>
<h2>New Contributors</h2>
<ul>
<li><a href="https://github.com/GhadimiR"><code>@​GhadimiR</code></a> made their first contribution in <a href="https://redirect.github.com/actions/upload-artifact/pull/681">actions/upload-artifact#681</a></li>
<li><a href="https://github.com/nebuk89"><code>@​nebuk89</code></a> made their first contribution in <a href="https://redirect.github.com/actions/upload-artifact/pull/712">actions/upload-artifact#712</a></li>
<li><a href="https://github.com/danwkennedy"><code>@​danwkennedy</code></a> made their first contribution in <a href="https://redirect.github.com/actions/upload-artifact/pull/727">actions/upload-artifact#727</a></li>
<li><a href="https://github.com/patrikpolyak"><code>@​patrikpolyak</code></a> made their first contribution in <a href="https://redirect.github.com/actions/upload-artifact/pull/725">actions/upload-artifact#725</a></li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://github.com/actions/upload-artifact/compare/v4...v5.0.0">https://github.com/actions/upload-artifact/compare/v4...v5.0.0</a></p>
</blockquote>
</details>
<details>
<summary>Commits</summary>
<ul>
<li><a href="https://github.com/actions/upload-artifact/commit/b7c566a772e6b6bfb58ed0dc250532a479d7789f"><code>b7c566a</code></a> Merge pull request <a href="https://redirect.github.com/actions/upload-artifact/issues/745">#745</a> from actions/upload-artifact-v6-release</li>
<li><a href="https://github.com/actions/upload-artifact/commit/e516bc8500aaf3d07d591fcd4ae6ab5f9c391d5b"><code>e516bc8</code></a> docs: correct description of Node.js 24 support in README</li>
<li><a href="https://github.com/actions/upload-artifact/commit/ddc45ed9bca9b38dbd643978d88e3981cdc91415"><code>ddc45ed</code></a> docs: update README to correct action name for Node.js 24 support</li>
<li><a href="https://github.com/actions/upload-artifact/commit/615b319bd27bb32c3d64dca6b6ed6974d5fbe653"><code>615b319</code></a> chore: release v6.0.0 for Node.js 24 support</li>
<li><a href="https://github.com/actions/upload-artifact/commit/017748b48f8610ca8e6af1222f4a618e84a9c703"><code>017748b</code></a> Merge pull request <a href="https://redirect.github.com/actions/upload-artifact/issues/744">#744</a> from actions/fix-storage-blob</li>
<li><a href="https://github.com/actions/upload-artifact/commit/38d4c7997f5510fcc41fc4aae2a6b97becdbe7fc"><code>38d4c79</code></a> chore: rebuild dist</li>
<li><a href="https://github.com/actions/upload-artifact/commit/7d27270e0cfd253e666c44abac0711308d2d042f"><code>7d27270</code></a> chore: add missing license cache files for <code>@​actions/core</code>, <code>@​actions/io</code>, and mi...</li>
<li><a href="https://github.com/actions/upload-artifact/commit/5f643d3c9475505ccaf26d686ffbfb71a8387261"><code>5f643d3</code></a> chore: update license files for <code>@​actions/artifact</code><a href="https://github.com/5"><code>@​5</code></a>.0.1 dependencies</li>
<li><a href="https://github.com/actions/upload-artifact/commit/1df1684032c88614064493e1a0478fcb3583e1d0"><code>1df1684</code></a> chore: update package-lock.json with <code>@​actions/artifact</code><a href="https://github.com/5"><code>@​5</code></a>.0.1</li>
<li><a href="https://github.com/actions/upload-artifact/commit/b5b1a918401ee270935b6b1d857ae66c85f3be6f"><code>b5b1a91</code></a> fix: update <code>@​actions/artifact</code> to ^5.0.0 for Node.js 24 punycode fix</li>
<li>Additional commits viewable in <a href="https://github.com/actions/upload-artifact/compare/ea165f8d65b6e75b540449e92b4886f43607fa02...b7c566a772e6b6bfb58ed0dc250532a479d7789f">compare view</a></li>
</ul>
</details>
<br />


[![Dependabot compatibility score](https://dependabot-badges.githubapp.com/badges/compatibility_score?dependency-name=actions/upload-artifact&package-manager=github_actions&previous-version=4.6.2&new-version=6.0.0)](https://docs.github.com/en/github/managing-security-vulnerabilities/about-dependabot-security-updates#about-compatibility-scores)

You can trigger a rebase of this PR by commenting `@dependabot rebase`.

[//]: # (dependabot-automerge-start)
[//]: # (dependabot-automerge-end)

---

<details>
<summary>Dependabot commands and options</summary>
<br />

You can trigger Dependabot actions by commenting on this PR:
- `@dependabot rebase` will rebase this PR
- `@dependabot recreate` will recreate this PR, overwriting any edits that have been made to it
- `@dependabot show <dependency name> ignore conditions` will show all of the ignore conditions of the specified dependency
- `@dependabot ignore this major version` will close this PR and stop Dependabot creating any more for this major version (unless you reopen the PR or upgrade to it yourself)
- `@dependabot ignore this minor version` will close this PR and stop Dependabot creating any more for this minor version (unless you reopen the PR or upgrade to it yourself)
- `@dependabot ignore this dependency` will close this PR and stop Dependabot creating any more for this dependency (unless you reopen the PR or upgrade to it yourself)


</details>

> **Note**
> Automatic rebases have been disabled on this pull request as it has been open for over 30 days.

