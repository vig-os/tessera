---
type: issue
state: open
created: 2026-10-16T23:39:59Z
updated: 2026-10-16T23:39:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/580
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:41.702Z
---

# [Issue 580]: [feat(schema): first-class trees/graphs, polylines/tube tracks and meshes/surfaces (anatomist use case)](https://github.com/vig-os/tessera/issues/580)

### Context

anatomist (a Bayesian canine-anatomy pipeline over CT/PET, same owner) needs three geometric/topological object kinds that are not currently first-class in tessera. Each is useful far beyond anatomist (vessel/airway tracking in human CT, cell lineage trees, mesh registration), so the right home is tessera-side primitives, not downstream project code.

### The three objects anatomist needs

**(1) Hierarchical trees / typed graphs** — Higra watershed hierarchies produce 3–6 trees per dog (area, volume, dynamics on gradient scales for lung/soft-tissue/bone). Each tree is a parent-array `parent[i]` plus per-node attributes (`altitude`, HU stats, volume, position-in-frame-A), with a `vertex_map` from image basins → leaf id. Multiple trees per dog, plus persistence diagrams (birth/death pairs), image grammars/DAGs (body→cavity→organ→lobe), and a template structure tree (template v1).

**(2) Polyline / tube tracks** — centreline tracks for aorta, caudal vena cava, trachea, spinal cord, vessel/bronchial trees. Per step: `(x,y,z, tangent3, curvature3, radius, cov6)` plus a smoothed-posterior band from an RTS smoother. Branches at bifurcations (multiple-hypothesis). Reference truth (part of phase-0 expert centrelines) and system output both live here.

**(3) Meshes / surfaces** — organ outer surfaces for the GIS-style topological vector model in ARCHITECTURE §4 ("touching organs share one boundary surface, no gaps or overlaps"), Reeb-graph skeletons for the articulated frame, factor-graph pose graphs.

### Current tessera workaround and why it hurts

All three can be encoded as table(s) + optional array, but:
- No codified schema → every consumer re-invents conventions, no cross-tool interop.
- `recursive_hierarchy` (ADR-0043) is about tessera's **internal** `{hash, stats, referencing}` recursion, not a user-facing graph primitive.
- No way for a `transform` or `roi` product to point into "node 1734 of tree `dynamics@scale=bone`".

### Proposal

Three additive product schemas, each a thin convention over existing array+table blocks:

- **`tree`** — blocks: `nodes` (table: `id, parent_id, altitude, size, …user attrs`), optional `leaves` (array: label volume of leaf-id per voxel). Reference formats to align with on export: NetworkX node-link JSON, GraphML/GEXF, HDF5 (as Higra writes). Expose a `read_tree(name) → networkx.DiGraph` adapter in `tessera-py`.
- **`persistence_diagram`** — one table of `(birth, death, dim, feature_id)` plus a `(filtration, metric)` field. Reference: GUDHI/Dionysus CSV/TXT; the standard column layout.
- **`polyline`** — table of `(track_id, step, x, y, z, radius, tangent_xyz?, cov6?)` or arrays per track. Reference formats: MRtrix `.tck`, DIPY `.trk`, VTK polydata, glTF `GL_LINE_STRIP`. Export CLI `tessera export tck/trk/vtk <polyline.tsra>`.
- **`mesh`** — tables `vertices (x,y,z, normal?, uv?)` + `faces (v0,v1,v2)` + optional `attrs`. Reference formats: GIFTI, PLY, STL, glTF, VTK polydata; export CLI equivalents.

A `node_ref = {product_id, tree_name, node_id}` referential primitive would let an ROI/transform/finding point into a tree.

### Reference-format match

tessera **lags** here today; the asks map 1:1 to published formats so a tessera version earns its keep only by adding the seal/provenance/MMR/pyramid on top.

