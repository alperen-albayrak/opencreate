# Conventions & Things To Remember

The hard-won knowledge. Read this before touching the renderer or worldgen.

## Gotchas that actually bit us

- **naga's SPIR-V backend defaults to `ADJUST_COORDINATE_SPACE`** — a
  hidden wgpu-style Y flip injected into vertex shaders. Combined with the
  explicit Vulkan Y flip in the projection matrix this rendered the world
  upside down. It is disabled in `oc-renderer/build.rs`; the projection
  matrix owns the one Y flip. Never re-enable it.
- **Determine Vulkan winding empirically.** Framebuffer-space orientation
  sign conventions tripped us twice analytically. The working state:
  `FrontFace::COUNTER_CLOCKWISE` + `CullMode::BACK` for chunk and entity
  pipelines. If geometry vanishes after touching vertex order, suspect
  winding first and test with `CullMode::NONE`.
- **`gen` is a reserved keyword in edition 2024** — the worldgen module is
  named `terrain`, not `gen`.
- **Seed 20260611 (the default) has open ocean at the origin.** The game
  searches outward for dry land to spawn; tests must anchor on found grass
  (see `oc-server/src/creatures.rs::test_world`) instead of assuming land
  at 0,0. The "all-sand world" incident was a player standing on the
  seabed inside backface-culled water — check the camera's position before
  assuming the renderer broke.
- **GPU buffers are freed via a retire list**, `FRAMES_IN_FLIGHT` frames
  after replacement — never destroy a buffer the GPU may still read, and
  never add a `device_wait_idle` per upload.
- **Sections are `Arc<Section>` with copy-on-write** (`Arc::make_mut`) so
  mesh jobs hold snapshots safely while edits land.
- **Mesh jobs need the full 3×3 column neighborhood** before meshing a
  column (border face culling + exact lighting). The streamer only meshes
  once all neighbors exist; unloading must remove GPU meshes by the
  column's actual sections, not just bookkeeping.
- **Worldgen edits change the world for existing seeds.** Only player
  edits persist; expect terrain discontinuities at save boundaries after
  generator changes during development.

## The rules we keep

- **Camera-relative rendering**: the GPU never sees absolute world
  coordinates in f32. World-space translation happens in f64 on the CPU
  per draw. Breaking this silently ruins precision far from spawn.
- **Light range must stay ≤ 15** while lighting is computed per mesh job:
  exactness depends on range < the 16-block snapshot margin.
- **Determinism**: worldgen, tree placement, cave carving and creature
  AI randomness are pure functions of (seed, position/tick) via splitmix-
  style hashes. No RNG state, nothing to persist, multiplayer-consistent.
- **Server authority**: clients predict, the server's echo confirms or
  rolls back. Every new gameplay action follows the same shape.
- **Every slice ships tested**: implement → headless tests → live
  screenshot verification where rendering is involved → commit → push.
  Visual bugs get isolated with debug experiments (force colors/sizes,
  frame-diff two screenshots — static artifacts aren't entities).

## Dev workflow notes

- Check `pgrep -x opencreate` before launching/killing game instances —
  the project owner play-tests frequently.
- Screenshots for verification: run the game in the background, sleep,
  `screencapture -x`, kill; crop with PIL to inspect details.
- The frame budget gate: steady-state 60 fps on the M1 through MoltenVK.
  Watch the HUD or the 5 s perf log line after renderer changes.
- Block ids are hardcoded in `oc_world::blocks` (0=air … 10=planks) until
  the data-driven block registry exists; `data/items.ron` references them
  by number. Keep the two in sync when adding blocks.
