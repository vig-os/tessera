---
type: issue
state: open
created: 2026-07-02T14:14:20Z
updated: 2026-09-28T17:59:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/295
comments: 1
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:55.353Z
---

# [Issue 295]: [EPIC: compute & infrastructure topology for tessera across scales (T0 acquisition → T5 federation)](https://github.com/vig-os/tessera/issues/295)

Umbrella for the deployment/scale architecture designed in `docs/spikes/tsra-explorer.md` (§ Compute & infrastructure topology). The sealed `.tsra` product is the spine; each scale tier is a thin consumer/producer; **compute stays SSOT (`tessera-io`, once) — only orchestration + engine change**.

**Tiers:** T0 acquisition/edge (ingest→seal, exists) · T1 storage/dist (S3 + OCI + CoW + WORM, exists) · T2 doctor station (embedded, local-first / cluster-optional — #286) · **T3 fan-out** · **T4 aggregated analysis** · **T5 federation**.

**Invariants:** SSOT compute + Arrow waist · sealed product = source of truth, downstream = rebuildable projection · access-locality decides placement (object-local→sealed/fan-out; cross-object→aggregate) · data-shape routing (arrays→OME-Zarr; tables→Arrow/Parquet/Iceberg→lakehouse) · bounded-mem (ADR-0026) enables both edge ingest + dense K8s packing · fan-out(map) vs shuffle on one K8s substrate.

Sub-issues below. Refs: #286
---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 02:15 PM_

Sub-issues (compute topology across scales):
- **#296** — T3 multi-product fan-out on K8s (Jobs/Indexed + queue(SQS/Redis/RabbitMQ)+KEDA / Argo; per-job resource mgmt, taints/GPU, Karpenter)
- **#297** — T4 cross-object aggregated analysis + query-engine ladder (DataFusion embedded → DuckDB → lakehouse; object-table levels; projection ELT)
- **#298** — Ballista(+Vortex) distributed-shuffle spike (Arrow-native scale-out on the same K8s; Spice.ai reference)
- **#299** — T5 unbundled data platform / catalog-index integrations (InvenioRDM · vector · relational cohort search)

Design: `docs/spikes/tsra-explorer.md` § Compute & infrastructure topology across scales.

