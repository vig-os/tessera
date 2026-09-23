---
type: pull_request
state: closed
branch: dependabot/github_actions/dev/actions-minor-patch-aa2a37f0ca → dev
created: 2026-02-24T19:13:30Z
updated: 2026-03-02T06:25:29Z
author: dependabot[bot]
author_url: https://github.com/dependabot[bot]
url: https://github.com/vig-os/tessera/pull/2
comments: 1
labels: dependencies, github_actions
assignees: none
milestone: none
projects: none
relationship: none
synced: 2026-08-04T10:45:18.903Z
---

# [PR 2](https://github.com/vig-os/tessera/pull/2) ci(deps): bump the actions-minor-patch group with 2 updates

Bumps the actions-minor-patch group with 2 updates: [actions/dependency-review-action](https://github.com/actions/dependency-review-action) and [github/codeql-action](https://github.com/github/codeql-action).

Updates `actions/dependency-review-action` from 4.8.2 to 4.8.3
<details>
<summary>Release notes</summary>
<p><em>Sourced from <a href="https://github.com/actions/dependency-review-action/releases">actions/dependency-review-action's releases</a>.</em></p>
<blockquote>
<h2>4.8.3</h2>
<h2>Dependency Review Action v4.8.3</h2>
<p>This is a bugfix release that updates a number of upstream dependencies and includes a fix for the earlier feature that detected oversized summaries and upload them as artifacts, which could occasionally crash the action.</p>
<p>We have also updated the release process to use a long-lived <code>v4</code> <strong>branch</strong> for the action, instead of a force-pushed tag, which aligns better with git branching strategies; the change should be transparent to end users.</p>
<h2>What's Changed</h2>
<ul>
<li>GitHub Actions can't push to our protected main by <a href="https://github.com/dangoor"><code>@​dangoor</code></a> in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1017">actions/dependency-review-action#1017</a></li>
<li>Bump actions/stale from 9.1.0 to 10.1.0 by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/995">actions/dependency-review-action#995</a></li>
<li>Bump github/codeql-action from 3 to 4 by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1003">actions/dependency-review-action#1003</a></li>
<li>Bump actions/setup-node from 4 to 6 by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1005">actions/dependency-review-action#1005</a></li>
<li>Upgrade glob to address a vulnerability by <a href="https://github.com/brrygrdn"><code>@​brrygrdn</code></a> in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1024">actions/dependency-review-action#1024</a></li>
<li>Bump js-yaml by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1020">actions/dependency-review-action#1020</a></li>
<li>Addressing vulnerabilities by <a href="https://github.com/Ahmed3lmallah"><code>@​Ahmed3lmallah</code></a> in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1036">actions/dependency-review-action#1036</a></li>
<li>Bump fast-xml-parser from 5.3.3 to 5.3.5 by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1050">actions/dependency-review-action#1050</a></li>
<li>Bump fast-xml-parser from 5.3.5 to 5.3.6 by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1053">actions/dependency-review-action#1053</a></li>
<li>Properly truncate long summaries and catch errors by <a href="https://github.com/juxtin"><code>@​juxtin</code></a> in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1052">actions/dependency-review-action#1052</a></li>
<li>Bump spdx-expression-parse from 3.0.1 to 4.0.0 in the spdx-licenses group across 1 directory by <a href="https://github.com/dependabot"><code>@​dependabot</code></a>[bot] in <a href="https://redirect.github.com/actions/dependency-review-action/pull/931">actions/dependency-review-action#931</a></li>
<li>Changes for Release 4.8.3 by <a href="https://github.com/ahpook"><code>@​ahpook</code></a> in <a href="https://redirect.github.com/actions/dependency-review-action/pull/1054">actions/dependency-review-action#1054</a></li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://github.com/actions/dependency-review-action/compare/v4.8.2..v4.8.3">https://github.com/actions/dependency-review-action/compare/v4.8.2..v4.8.3</a></p>
</blockquote>
</details>
<details>
<summary>Commits</summary>
<ul>
<li><a href="https://github.com/actions/dependency-review-action/commit/05fe4576374b728f0c523d6a13d64c25081e0803"><code>05fe457</code></a> Merge pull request <a href="https://redirect.github.com/actions/dependency-review-action/issues/1054">#1054</a> from actions/ahpook/release-4.8.3</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/3a8496cb71ebae2e228d1c4a47974cdc724cf07d"><code>3a8496c</code></a> Update generated package files for v4.8.3</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/0f22a0159293e2496eef4ce36c3b7b3b31081f7d"><code>0f22a01</code></a> Update CONTRIBUTING for new release process</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/58be34364db3f04dc3de8db0417b5d18451a4fdf"><code>58be343</code></a> Updating package versions for 4.8.3</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/9284e0c621cb66311d82087d9ea1f539e40da6eb"><code>9284e0c</code></a> Merge pull request <a href="https://redirect.github.com/actions/dependency-review-action/issues/931">#931</a> from actions/dependabot/npm_and_yarn/spdx-licenses-20...</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/8b766562f01731bcb0f65222324f2152d142a19a"><code>8b76656</code></a> Bump spdx-expression-parse in the spdx-licenses group across 1 directory</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/43f5f029f51af9c859564cae942f58ea63a22100"><code>43f5f02</code></a> Merge pull request <a href="https://redirect.github.com/actions/dependency-review-action/issues/1052">#1052</a> from actions/juxtin/fix-long-summaries</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/f0033fc4d6972851b5170177d58a8da79811a797"><code>f0033fc</code></a> Merge pull request <a href="https://redirect.github.com/actions/dependency-review-action/issues/1053">#1053</a> from actions/dependabot/npm_and_yarn/fast-xml-parser...</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/b379e2e05ffa2e429ca97047d4c2738a0039425e"><code>b379e2e</code></a> Bump fast-xml-parser from 5.3.5 to 5.3.6</li>
<li><a href="https://github.com/actions/dependency-review-action/commit/2e1cf54a500fb2037239e92489ed0bad323c8c68"><code>2e1cf54</code></a> Properly truncate long summaries and catch errors</li>
<li>Additional commits viewable in <a href="https://github.com/actions/dependency-review-action/compare/3c4e3dcb1aa7874d2c16be7d79418e9b7efd6261...05fe4576374b728f0c523d6a13d64c25081e0803">compare view</a></li>
</ul>
</details>
<br />

Updates `github/codeql-action` from 4.32.2 to 4.32.4
<details>
<summary>Release notes</summary>
<p><em>Sourced from <a href="https://github.com/github/codeql-action/releases">github/codeql-action's releases</a>.</em></p>
<blockquote>
<h2>v4.32.4</h2>
<ul>
<li>Update default CodeQL bundle version to <a href="https://github.com/github/codeql-action/releases/tag/codeql-bundle-v2.24.2">2.24.2</a>. <a href="https://redirect.github.com/github/codeql-action/pull/3493">#3493</a></li>
<li>Added an experimental change which improves how certificates are generated for the authentication proxy that is used by the CodeQL Action in Default Setup when <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries are configured</a>. This is expected to generate more widely compatible certificates and should have no impact on analyses which are working correctly already. We expect to roll this change out to everyone in February. <a href="https://redirect.github.com/github/codeql-action/pull/3473">#3473</a></li>
<li>When the CodeQL Action is run <a href="https://docs.github.com/en/code-security/how-tos/scan-code-for-vulnerabilities/troubleshooting/troubleshooting-analysis-errors/logs-not-detailed-enough#creating-codeql-debugging-artifacts-for-codeql-default-setup">with debugging enabled in Default Setup</a> and <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries are configured</a>, the &quot;Setup proxy for registries&quot; step will output additional diagnostic information that can be used for troubleshooting. <a href="https://redirect.github.com/github/codeql-action/pull/3486">#3486</a></li>
<li>Added a setting which allows the CodeQL Action to enable network debugging for Java programs. This will help GitHub staff support customers with troubleshooting issues in GitHub-managed CodeQL workflows, such as Default Setup. This setting can only be enabled by GitHub staff. <a href="https://redirect.github.com/github/codeql-action/pull/3485">#3485</a></li>
<li>Added a setting which enables GitHub-managed workflows, such as Default Setup, to use a <a href="https://github.com/dsp-testing/codeql-cli-nightlies">nightly CodeQL CLI release</a> instead of the latest, stable release that is used by default. This will help GitHub staff support customers whose analyses for a given repository or organization require early access to a change in an upcoming CodeQL CLI release. This setting can only be enabled by GitHub staff. <a href="https://redirect.github.com/github/codeql-action/pull/3484">#3484</a></li>
</ul>
<h2>v4.32.3</h2>
<ul>
<li>Added experimental support for testing connections to <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries</a>. This feature is not currently enabled for any analysis. In the future, it may be enabled by default for Default Setup. <a href="https://redirect.github.com/github/codeql-action/pull/3466">#3466</a></li>
</ul>
</blockquote>
</details>
<details>
<summary>Changelog</summary>
<p><em>Sourced from <a href="https://github.com/github/codeql-action/blob/main/CHANGELOG.md">github/codeql-action's changelog</a>.</em></p>
<blockquote>
<h1>CodeQL Action Changelog</h1>
<p>See the <a href="https://github.com/github/codeql-action/releases">releases page</a> for the relevant changes to the CodeQL CLI and language packs.</p>
<h2>[UNRELEASED]</h2>
<p>No user facing changes.</p>
<h2>4.32.4 - 20 Feb 2026</h2>
<ul>
<li>Update default CodeQL bundle version to <a href="https://github.com/github/codeql-action/releases/tag/codeql-bundle-v2.24.2">2.24.2</a>. <a href="https://redirect.github.com/github/codeql-action/pull/3493">#3493</a></li>
<li>Added an experimental change which improves how certificates are generated for the authentication proxy that is used by the CodeQL Action in Default Setup when <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries are configured</a>. This is expected to generate more widely compatible certificates and should have no impact on analyses which are working correctly already. We expect to roll this change out to everyone in February. <a href="https://redirect.github.com/github/codeql-action/pull/3473">#3473</a></li>
<li>When the CodeQL Action is run <a href="https://docs.github.com/en/code-security/how-tos/scan-code-for-vulnerabilities/troubleshooting/troubleshooting-analysis-errors/logs-not-detailed-enough#creating-codeql-debugging-artifacts-for-codeql-default-setup">with debugging enabled in Default Setup</a> and <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries are configured</a>, the &quot;Setup proxy for registries&quot; step will output additional diagnostic information that can be used for troubleshooting. <a href="https://redirect.github.com/github/codeql-action/pull/3486">#3486</a></li>
<li>Added a setting which allows the CodeQL Action to enable network debugging for Java programs. This will help GitHub staff support customers with troubleshooting issues in GitHub-managed CodeQL workflows, such as Default Setup. This setting can only be enabled by GitHub staff. <a href="https://redirect.github.com/github/codeql-action/pull/3485">#3485</a></li>
<li>Added a setting which enables GitHub-managed workflows, such as Default Setup, to use a <a href="https://github.com/dsp-testing/codeql-cli-nightlies">nightly CodeQL CLI release</a> instead of the latest, stable release that is used by default. This will help GitHub staff support customers whose analyses for a given repository or organization require early access to a change in an upcoming CodeQL CLI release. This setting can only be enabled by GitHub staff. <a href="https://redirect.github.com/github/codeql-action/pull/3484">#3484</a></li>
</ul>
<h2>4.32.3 - 13 Feb 2026</h2>
<ul>
<li>Added experimental support for testing connections to <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registries</a>. This feature is not currently enabled for any analysis. In the future, it may be enabled by default for Default Setup. <a href="https://redirect.github.com/github/codeql-action/pull/3466">#3466</a></li>
</ul>
<h2>4.32.2 - 05 Feb 2026</h2>
<ul>
<li>Update default CodeQL bundle version to <a href="https://github.com/github/codeql-action/releases/tag/codeql-bundle-v2.24.1">2.24.1</a>. <a href="https://redirect.github.com/github/codeql-action/pull/3460">#3460</a></li>
</ul>
<h2>4.32.1 - 02 Feb 2026</h2>
<ul>
<li>A warning is now shown in Default Setup workflow logs if a <a href="https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/manage-usage-and-access/giving-org-access-private-registries">private package registry is configured</a> using a GitHub Personal Access Token (PAT), but no username is configured. <a href="https://redirect.github.com/github/codeql-action/pull/3422">#3422</a></li>
<li>Fixed a bug which caused the CodeQL Action to fail when repository properties cannot successfully be retrieved. <a href="https://redirect.github.com/github/codeql-action/pull/3421">#3421</a></li>
</ul>
<h2>4.32.0 - 26 Jan 2026</h2>
<ul>
<li>Update default CodeQL bundle version to <a href="https://github.com/github/codeql-action/releases/tag/codeql-bundle-v2.24.0">2.24.0</a>. <a href="https://redirect.github.com/github/codeql-action/pull/3425">#3425</a></li>
</ul>
<h2>4.31.11 - 23 Jan 2026</h2>
<ul>
<li>When running a Default Setup workflow with <a href="https://docs.github.com/en/actions/how-tos/monitor-workflows/enable-debug-logging">Actions debugging enabled</a>, the CodeQL Action will now use more unique names when uploading logs from the Dependabot authentication proxy as workflow artifacts. This ensures that the artifact names do not clash between multiple jobs in a build matrix. <a href="https://redirect.github.com/github/codeql-action/pull/3409">#3409</a></li>
<li>Improved error handling throughout the CodeQL Action. <a href="https://redirect.github.com/github/codeql-action/pull/3415">#3415</a></li>
<li>Added experimental support for automatically excluding <a href="https://docs.github.com/en/repositories/working-with-files/managing-files/customizing-how-changed-files-appear-on-github">generated files</a> from the analysis. This feature is not currently enabled for any analysis. In the future, it may be enabled by default for some GitHub-managed analyses. <a href="https://redirect.github.com/github/codeql-action/pull/3318">#3318</a></li>
<li>The changelog extracts that are included with releases of the CodeQL Action are now shorter to avoid duplicated information from appearing in Dependabot PRs. <a href="https://redirect.github.com/github/codeql-action/pull/3403">#3403</a></li>
</ul>
<h2>4.31.10 - 12 Jan 2026</h2>
<ul>
<li>Update default CodeQL bundle version to 2.23.9. <a href="https://redirect.github.com/github/codeql-action/pull/3393">#3393</a></li>
</ul>
<h2>4.31.9 - 16 Dec 2025</h2>
<p>No user facing changes.</p>
<h2>4.31.8 - 11 Dec 2025</h2>
<!-- raw HTML omitted -->
</blockquote>
<p>... (truncated)</p>
</details>
<details>
<summary>Commits</summary>
<ul>
<li><a href="https://github.com/github/codeql-action/commit/89a39a4e59826350b863aa6b6252a07ad50cf83e"><code>89a39a4</code></a> Merge pull request <a href="https://redirect.github.com/github/codeql-action/issues/3494">#3494</a> from github/update-v4.32.4-39ba80c47</li>
<li><a href="https://github.com/github/codeql-action/commit/e5d84c885c00d506f7816d26a298534dbbffac6d"><code>e5d84c8</code></a> Apply remaining review suggestions</li>
<li><a href="https://github.com/github/codeql-action/commit/0c202097b5de484e2a3725d4467f9cb7e3107881"><code>0c20209</code></a> Apply suggestions from code review</li>
<li><a href="https://github.com/github/codeql-action/commit/314172e5a1e1691ba4ad232b3d0230ceaf3d9239"><code>314172e</code></a> Fix typo</li>
<li><a href="https://github.com/github/codeql-action/commit/cdda72d36b93310932b0afe1784acd0209d190dd"><code>cdda72d</code></a> Add changelog entries</li>
<li><a href="https://github.com/github/codeql-action/commit/cfda84cc5509282e2adc1570c3cf29c3167ae87f"><code>cfda84c</code></a> Update changelog for v4.32.4</li>
<li><a href="https://github.com/github/codeql-action/commit/39ba80c47550c834104c0f222b502461ac312c29"><code>39ba80c</code></a> Merge pull request <a href="https://redirect.github.com/github/codeql-action/issues/3493">#3493</a> from github/update-bundle/codeql-bundle-v2.24.2</li>
<li><a href="https://github.com/github/codeql-action/commit/00150dad957fc9c1cba52bdab82e458ae5c09fe5"><code>00150da</code></a> Add changelog note</li>
<li><a href="https://github.com/github/codeql-action/commit/d97dce6561ae3dd4e4db9bfa95479f7572bd7566"><code>d97dce6</code></a> Update default bundle to codeql-bundle-v2.24.2</li>
<li><a href="https://github.com/github/codeql-action/commit/50fdbb9ec845c41d6d3509d794e3a28af7032c59"><code>50fdbb9</code></a> Merge pull request <a href="https://redirect.github.com/github/codeql-action/issues/3492">#3492</a> from github/henrymercer/new-repository-properties-ff</li>
<li>Additional commits viewable in <a href="https://github.com/github/codeql-action/compare/45cbd0c69e560cd9e7cd7f8c32362050c9b7ded2...89a39a4e59826350b863aa6b6252a07ad50cf83e">compare view</a></li>
</ul>
</details>
<br />


Dependabot will resolve any conflicts with this PR as long as you don't alter it yourself. You can also trigger a rebase manually by commenting `@dependabot rebase`.

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
- `@dependabot ignore <dependency name> major version` will close this group update PR and stop Dependabot creating any more for the specific dependency's major version (unless you unignore this specific dependency's major version or upgrade to it yourself)
- `@dependabot ignore <dependency name> minor version` will close this group update PR and stop Dependabot creating any more for the specific dependency's minor version (unless you unignore this specific dependency's minor version or upgrade to it yourself)
- `@dependabot ignore <dependency name>` will close this group update PR and stop Dependabot creating any more for the specific dependency (unless you unignore this specific dependency or upgrade to it yourself)
- `@dependabot unignore <dependency name>` will remove all of the ignore conditions of the specified dependency
- `@dependabot unignore <dependency name> <ignore condition>` will remove the ignore condition of the specified dependency and ignore conditions


</details>


---
---

## Comments (1)

### [Comment #1](https://github.com/vig-os/tessera/pull/2#issuecomment-3982399574) by [@dependabot[bot]](https://github.com/apps/dependabot)

_Posted on March 2, 2026 at 06:25 AM_

Looks like these dependencies are updatable in another way, so this is no longer needed.

---
---

## Commits

### Commit 1: [5e92291](https://github.com/vig-os/tessera/commit/5e9229102d07408cdab8345bbcf58e1e8366dfee) by [dependabot[bot]](https://github.com/apps/dependabot) on February 24, 2026 at 07:13 PM
ci(deps): bump the actions-minor-patch group with 2 updates, 8 files modified (.github/workflows/ci.yml, .github/workflows/codeql.yml, .github/workflows/scorecard.yml)
