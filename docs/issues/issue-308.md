---
type: issue
state: open
created: 2026-07-02T15:11:53Z
updated: 2026-09-28T18:00:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/308
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:50.577Z
---

# [Issue 308]: [Spike: Phase-3b GUI tech — egui vs Tauri vs webstack (gated on volren-gpu wasm feasibility)](https://github.com/vig-os/tessera/issues/308)

Decide the rich viewer / 3-D tier (Phase 3b, EPIC #295 T2/T3) delivery tech. Framed in `docs/spikes/tsra-explorer.md` § Phase 3 + the UX spike.

**Reframe — two axes, not a 3-way race:**
- **UI toolkit:** Rust immediate-mode (**egui/eframe**) vs web (HTML/TS, used by both Tauri and webstack).
- **Delivery:** native desktop (egui-native or **Tauri**) vs browser (**webstack**).
- Tauri + webstack **share one web frontend** (Tauri = web-UI-as-desktop). **egui** is the Rust-native alt → native + web(wasm) from one source, with **volren + plotters + tessera-io embedding directly** (precedent: rerun.io = egui+wgpu multimodal viewer, native+web).
- Two coherent strategies to bench: **(1) Rust-native-led** (egui desktop + optional egui-wasm web, + webstack for reach) vs **(2) web-UI-led** (Vite+TS → webstack + Tauri over `tsra serve`).

**Pivotal gate — spike FIRST:** does **`volren-gpu` cross-compile to wasm→WebGPU**? Yes → client-side interactive 3-D in egui-web/webstack; No → web tiers must **server-render** (PNG over serve), tilting interactive 3-D toward native. This one result reshapes the rest.

**Benchmark criteria (thin vertical slice each: read one .tsra → render one CT/PET volume via volren + one plotters chart):**
1. Interactive render — ray-march/MIP fps + slice-scrub latency on a 512³ volume (native vs webview vs browser)
2. volren-gpu wasm feasibility (the gate) — compiles? runs? perf vs native
3. Time-to-first-pixel + bundle/binary size (egui-wasm blob vs Tauri small binary vs native)
4. Dev velocity for fusion/MPR/linked-brushing UI (immediate-mode vs web ecosystem)
5. Offline / PHI-local (all trivial except webstack) + WebGPU-in-webview variability (WebKitGTK/Linux weakest)
6. Reach / shareability (URL, no-install = webstack only)
7. One-codebase reuse (egui native+web; web-UI reuses webstack↔Tauri)

**Deferred / not a footgun:** Phase-3; the substrate (view-model #1a + `tsra serve` + vendored volren, ADR-0050) keeps all three viable *provided none of their assumptions leak into 1a/serve/volren*. Pull the volren-wasm probe earlier if we want the answer sooner.

Refs: #286
