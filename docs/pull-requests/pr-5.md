---
type: pull_request
state: open
branch: dependabot/github_actions/dev/actions/cache-5.0.3 → dev
created: 2026-02-24T19:13:47Z
updated: 2026-03-30T09:20:28Z
author: dependabot[bot]
author_url: https://github.com/dependabot[bot]
url: https://github.com/vig-os/tessera/pull/5
comments: 0
labels: dependencies, github_actions
assignees: none
milestone: none
projects: none
relationship: none
synced: 2026-08-04T10:44:57.524Z
---

# [PR 5](https://github.com/vig-os/tessera/pull/5) ci(deps): bump actions/cache from 4.3.0 to 5.0.3

Bumps [actions/cache](https://github.com/actions/cache) from 4.3.0 to 5.0.3.
<details>
<summary>Release notes</summary>
<p><em>Sourced from <a href="https://github.com/actions/cache/releases">actions/cache's releases</a>.</em></p>
<blockquote>
<h2>v5.0.3</h2>
<h2>What's Changed</h2>
<ul>
<li>Bump <code>@actions/cache</code> to v5.0.5 (Resolves: <a href="https://github.com/actions/cache/security/dependabot/33">https://github.com/actions/cache/security/dependabot/33</a>)</li>
<li>Bump <code>@actions/core</code> to v2.0.3</li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://github.com/actions/cache/compare/v5...v5.0.3">https://github.com/actions/cache/compare/v5...v5.0.3</a></p>
<h2>v.5.0.2</h2>
<h1>v5.0.2</h1>
<h2>What's Changed</h2>
<p>When creating cache entries, 429s returned from the cache service will not be retried.</p>
<h2>v5.0.1</h2>
<blockquote>
<p>[!IMPORTANT]
<strong><code>actions/cache@v5</code> runs on the Node.js 24 runtime and requires a minimum Actions Runner version of <code>2.327.1</code>.</strong></p>
<p>If you are using self-hosted runners, ensure they are updated before upgrading.</p>
</blockquote>
<hr />
<h1>v5.0.1</h1>
<h2>What's Changed</h2>
<ul>
<li>fix: update <code>@​actions/cache</code> for Node.js 24 punycode deprecation by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/cache/pull/1685">actions/cache#1685</a></li>
<li>prepare release v5.0.1 by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/cache/pull/1686">actions/cache#1686</a></li>
</ul>
<h1>v5.0.0</h1>
<h2>What's Changed</h2>
<ul>
<li>Upgrade to use node24 by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/cache/pull/1630">actions/cache#1630</a></li>
<li>Prepare v5.0.0 release by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/cache/pull/1684">actions/cache#1684</a></li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://github.com/actions/cache/compare/v5...v5.0.1">https://github.com/actions/cache/compare/v5...v5.0.1</a></p>
<h2>v5.0.0</h2>
<blockquote>
<p>[!IMPORTANT]
<strong><code>actions/cache@v5</code> runs on the Node.js 24 runtime and requires a minimum Actions Runner version of <code>2.327.1</code>.</strong></p>
<p>If you are using self-hosted runners, ensure they are updated before upgrading.</p>
</blockquote>
<hr />
<h2>What's Changed</h2>
<ul>
<li>Upgrade to use node24 by <a href="https://github.com/salmanmkc"><code>@​salmanmkc</code></a> in <a href="https://redirect.github.com/actions/cache/pull/1630">actions/cache#1630</a></li>
</ul>
<!-- raw HTML omitted -->
</blockquote>
<p>... (truncated)</p>
</details>
<details>
<summary>Changelog</summary>
<p><em>Sourced from <a href="https://github.com/actions/cache/blob/main/RELEASES.md">actions/cache's changelog</a>.</em></p>
<blockquote>
<h1>Releases</h1>
<h2>How to prepare a release</h2>
<blockquote>
<p>[!NOTE]<br />
Relevant for maintainers with write access only.</p>
</blockquote>
<ol>
<li>Switch to a new branch from <code>main</code>.</li>
<li>Run <code>npm test</code> to ensure all tests are passing.</li>
<li>Update the version in <a href="https://github.com/actions/cache/blob/main/package.json"><code>https://github.com/actions/cache/blob/main/package.json</code></a>.</li>
<li>Run <code>npm run build</code> to update the compiled files.</li>
<li>Update this <a href="https://github.com/actions/cache/blob/main/RELEASES.md"><code>https://github.com/actions/cache/blob/main/RELEASES.md</code></a> with the new version and changes in the <code>## Changelog</code> section.</li>
<li>Run <code>licensed cache</code> to update the license report.</li>
<li>Run <code>licensed status</code> and resolve any warnings by updating the <a href="https://github.com/actions/cache/blob/main/.licensed.yml"><code>https://github.com/actions/cache/blob/main/.licensed.yml</code></a> file with the exceptions.</li>
<li>Commit your changes and push your branch upstream.</li>
<li>Open a pull request against <code>main</code> and get it reviewed and merged.</li>
<li>Draft a new release <a href="https://github.com/actions/cache/releases">https://github.com/actions/cache/releases</a> use the same version number used in <code>package.json</code>
<ol>
<li>Create a new tag with the version number.</li>
<li>Auto generate release notes and update them to match the changes you made in <code>RELEASES.md</code>.</li>
<li>Toggle the set as the latest release option.</li>
<li>Publish the release.</li>
</ol>
</li>
<li>Navigate to <a href="https://github.com/actions/cache/actions/workflows/release-new-action-version.yml">https://github.com/actions/cache/actions/workflows/release-new-action-version.yml</a>
<ol>
<li>There should be a workflow run queued with the same version number.</li>
<li>Approve the run to publish the new version and update the major tags for this action.</li>
</ol>
</li>
</ol>
<h2>Changelog</h2>
<h3>5.0.3</h3>
<ul>
<li>Bump <code>@actions/cache</code> to v5.0.5 (Resolves: <a href="https://github.com/actions/cache/security/dependabot/33">https://github.com/actions/cache/security/dependabot/33</a>)</li>
<li>Bump <code>@actions/core</code> to v2.0.3</li>
</ul>
<h3>5.0.2</h3>
<ul>
<li>Bump <code>@actions/cache</code> to v5.0.3 <a href="https://redirect.github.com/actions/cache/pull/1692">#1692</a></li>
</ul>
<h3>5.0.1</h3>
<ul>
<li>Update <code>@azure/storage-blob</code> to <code>^12.29.1</code> via <code>@actions/cache@5.0.1</code> <a href="https://redirect.github.com/actions/cache/pull/1685">#1685</a></li>
</ul>
<h3>5.0.0</h3>
<blockquote>
<p>[!IMPORTANT]
<code>actions/cache@v5</code> runs on the Node.js 24 runtime and requires a minimum Actions Runner version of <code>2.327.1</code>.
If you are using self-hosted runners, ensure they are updated before upgrading.</p>
</blockquote>
<h3>4.3.0</h3>
<ul>
<li>Bump <code>@actions/cache</code> to <a href="https://redirect.github.com/actions/toolkit/pull/2132">v4.1.0</a></li>
</ul>
<!-- raw HTML omitted -->
</blockquote>
<p>... (truncated)</p>
</details>
<details>
<summary>Commits</summary>
<ul>
<li><a href="https://github.com/actions/cache/commit/cdf6c1fa76f9f475f3d7449005a359c84ca0f306"><code>cdf6c1f</code></a> Merge pull request <a href="https://redirect.github.com/actions/cache/issues/1695">#1695</a> from actions/Link-/prepare-5.0.3</li>
<li><a href="https://github.com/actions/cache/commit/a1bee22673bee4afb9ce4e0a1dc3da1c44060b7d"><code>a1bee22</code></a> Add review for the <code>@​actions/http-client</code> license</li>
<li><a href="https://github.com/actions/cache/commit/46957638dc5c5ff0c34c0143f443c07d3a7c769f"><code>4695763</code></a> Add licensed output</li>
<li><a href="https://github.com/actions/cache/commit/dc73bb9f7bf74a733c05ccd2edfd1f2ac9e5f502"><code>dc73bb9</code></a> Upgrade dependencies and address security warnings</li>
<li><a href="https://github.com/actions/cache/commit/345d5c2f761565bace4b6da356737147e9041e3a"><code>345d5c2</code></a> Add 5.0.3 builds</li>
<li><a href="https://github.com/actions/cache/commit/8b402f58fbc84540c8b491a91e594a4576fec3d7"><code>8b402f5</code></a> Merge pull request <a href="https://redirect.github.com/actions/cache/issues/1692">#1692</a> from GhadimiR/main</li>
<li><a href="https://github.com/actions/cache/commit/304ab5a0701ee61908ccb4b5822347949a2e2002"><code>304ab5a</code></a> license for httpclient</li>
<li><a href="https://github.com/actions/cache/commit/609fc19e67cd310e97eb36af42355843ffcb35be"><code>609fc19</code></a> Update licensed record for cache</li>
<li><a href="https://github.com/actions/cache/commit/b22231e43df11a67538c05e88835f1fa097599c5"><code>b22231e</code></a> Build</li>
<li><a href="https://github.com/actions/cache/commit/93150cdfb36a9d84d4e8628c8870bec84aedcf8a"><code>93150cd</code></a> Add PR link to releases</li>
<li>Additional commits viewable in <a href="https://github.com/actions/cache/compare/0057852bfaa89a56745cba8c7296529d2fc39830...cdf6c1fa76f9f475f3d7449005a359c84ca0f306">compare view</a></li>
</ul>
</details>
<br />


[![Dependabot compatibility score](https://dependabot-badges.githubapp.com/badges/compatibility_score?dependency-name=actions/cache&package-manager=github_actions&previous-version=4.3.0&new-version=5.0.3)](https://docs.github.com/en/github/managing-security-vulnerabilities/about-dependabot-security-updates#about-compatibility-scores)

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

