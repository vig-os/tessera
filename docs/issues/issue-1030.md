---
type: issue
state: open
created: 2026-11-27T09:57:02Z
updated: 2026-11-27T09:57:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1030
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:52.584Z
---

# [Issue 1030]: [Reserve builder CPUs for opt-in wall-clock tests in ksb-test](https://github.com/vig-os/tessera/issues/1030)

The ksb-test wrapper runs tests inside a Nix builder, while ksb run reserves CPUs only for the reporting app. Consequently --cpu and --exclusive-cpus on that job do not reserve the CPUs executing tests. Add a supported path to execute opt-in wall-clock tests in a reserved builder slice with a fixed CPU allocation and explicit test environment. Required for the TESSERA_PERF_RATCHET=1 workflow in #703; ordinary CI must keep these tests opted out. See also #961.
