---
type: issue
state: closed
created: 2026-08-07T16:45:38Z
updated: 2026-09-29T02:32:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/373
comments: 0
labels: priority:low, effort:small, area:core
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:44.056Z
---

# [Issue 373]: [chore(verify): route the verify worker cap through the resource-cap resolver (#368)](https://github.com/vig-os/tessera/issues/373)

Now that both #367 (parallel verify) and #368 (resource-cap resolver) are on dev, close the loop the two PRs flagged: `Cmd::Verify` currently uses `WriteConfig::for_system().worker_count()` for its parallel L2 probe. Swap that one call for `resource::resolve_write_config(None, None)?.worker_count()` so `tessera verify` honors `TESSERA_WORKERS` / `.tessera/config.toml` like ingest does. Optionally add `--workers` to the verify subcommand for the flag tier. Trivial + a test that an env cap changes the resolved worker count. Refs: #367, #368
