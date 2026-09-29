---
type: issue
state: open
created: 2026-07-02T14:14:20Z
updated: 2026-09-28T17:59:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/296
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:54.992Z
---

# [Issue 296]: [T3: multi-product fan-out orchestration on Kubernetes (Jobs/queue/KEDA/Argo + resource mgmt)](https://github.com/vig-os/tessera/issues/296)

**Tier 3** of the compute topology (EPIC #295). Process many `.tsra` products in parallel — ingest a hospital dump, reconstruct N studies, extract PHI-safe features, derive pyramids/products. **Object-parallel map: 1 worker = 1 product**, each running the tessera binary + embedded DataFusion; stateless + retriable; bounded memory (ADR-0026) → dense packing.

**Orchestration ladder:**
- fixed one-shot batch → **K8s Indexed Job** (K8s is the scheduler; oversubscribe via `completions`, bound `parallelism` in-flight)
- streaming arrival + autoscale → **queue (SQS / Redis / RabbitMQ) + KEDA** (scale-to-zero on queue depth, at-least-once + retry)
- multi-step DAG (ingest→recon→features→sign→register + fan-in) → **Argo Workflows** (per-step resources)

**Resource management:** per-job `requests`/`limits` (CPU compressible→throttle, memory incompressible→OOM); node pools + `nodeAffinity`/**taints+tolerations** to reserve GPU/expensive nodes; `nvidia.com/gpu` extended resources; Karpenter/cluster-autoscaler right-sizes instance types from pending-pod requests; ResourceQuota/LimitRange + PriorityClass/preemption. **One right-sized Job (or Argo step) per resource profile** — not one uniform Indexed Job.

Output: N sealed products + N PHI-safe summary rows (fan-in feeds T4, #). Refs: #286
