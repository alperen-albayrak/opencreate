# The Engine (oc-renderer)

Raw Vulkan via `ash`, constrained to **Vulkan 1.2 ∩ MoltenVK** so macOS
works through Vulkan-on-Metal. `winit` for windowing, `gpu-allocator` for
memory, `glam` for math, `naga` compiles WGSL→SPIR-V at build time. No
frameworks. The renderer never sees game logic: it consumes meshes,
transforms, draw lists and UI primitives (§4).

## Frame structure

One render pass (color + depth), recorded fresh each frame into one of
`FRAMES_IN_FLIGHT = 2` command buffers:

1. **Chunks** — one draw per visible section mesh (frustum-culled CPU-side
   with Gribb–Hartmann planes in camera-relative space); push constants
   carry the camera-relative MVP and the sun vector.
2. **Entities** — tinted cuboids, one push-constant draw each.
3. **Block outline** — line-list cube around the targeted block.
4. **UI** — alpha-blended screen-space quads + bitmap text, no depth.

Presentation uses per-image semaphores; swapchain recreation handles
resize/out-of-date. GPU buffers replaced mid-flight go on a **retire
list** and are freed once their last possible frame has provably
completed — never `device_wait_idle` per upload.

## The cardinal rule

**Camera-relative rendering**: every world-space translation happens in
f64 on the CPU; the GPU only ever sees positions relative to the camera.
This is what keeps precision intact ±30M blocks from spawn and it must
hold for every new pipeline.

## Coordinate-space landmines

Two hard-won facts (details in [conventions](../../conventions.md)):
naga's `ADJUST_COORDINATE_SPACE` is disabled — the projection matrix owns
the single Vulkan Y flip; and winding/culling conventions are verified
empirically (current: CCW front, back culling).

## Sub-pages

- [meshing.md](meshing.md) — greedy mesher and the packed vertex format
- [lighting.md](lighting.md) — the flood-fill light field
- [ui.md](ui.md) — font atlas, HUD, hotbar rendering

## Perf state

Steady 60 fps on the M1 at view radius 12: ~830 chunks drawn of ~2400
resident (≈65% frustum-culled). Next wins when needed: pooled vertex
buffers + multi-draw-indirect, palette-aware meshing, GPU occlusion (§4).
