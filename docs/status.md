# Status

*Last updated: 2026-06-12 (commit `6e5786c`).*

**Roadmap position:** phase 1 (engine bring-up) and phase 2 (world
prototype) are complete; phase 3 (survival core) is well underway. The
workspace holds 7 crates, ~86 tests, all green; the M1 dev machine runs
60 fps at a 12-column (192-block) view distance.

## Done

### Phase 1 — Engine bring-up
Vulkan 1.2 via ash + MoltenVK; swapchain, depth, camera-relative f64
rendering (floating origin); textured chunk; fly camera.

### Phase 2 — World prototype
- Async chunk pipeline: rayon generation/meshing jobs, budgeted uploads
- Greedy meshing (coverage-equivalence tested), CPU frustum culling
- Worldgen: value-noise heightmap, three biomes, sea-level rivers, cheese
  caves, trees, beaches, oceans
- Flood-fill lighting (sky + block, baked per-vertex), lamps, day/night
- Walking/swimming/collision, block place/break with target outline
- Save/load behind `WorldStore` (zstd, atomic, dirty-columns-only),
  30 s autosave, level metadata
- Debug HUD (own bitmap font + UI pipeline), perf counters

### Phase 3 — Survival core (in progress)
- **The §1 client/server split is live**: `oc-protocol` + `oc-server`
  (30 TPS authoritative thread) + client as a predicted mirror
- Survival stats on `bevy_ecs` (drowning, stamina, hunger, regen, respawn),
  fall damage, stat bars
- Items/recipes/game-modes/creatures all data-driven (RON registry)
- Survival inventory (gather on break, consume on place, server-validated
  with prediction rollback), crafting via the C-key recipe book
- Four game modes (survival/creative/adventure/spectator) as data
- Passive creatures: server wander AI + interpolated client rendering

## Known issues

- Cosmetic: 1-block-wide sand "needles" at river-carve threshold edges on
  beaches (worldgen smoothing fix tracked separately)
- Worldgen changes between commits regenerate unedited terrain differently
  (only player edits persist — by design during development)

## Not started yet

Villages, food/eating, drag-grid inventory screen, `./mods/` loader,
texture packs/skins, dedicated server binary + QUIC, palette compression,
LOD, GPU occlusion culling, physics grids. See [roadmap.md](roadmap.md).
