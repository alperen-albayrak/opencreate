# Status

*Last updated: 2026-06-12 (worldgen v3).*

**Roadmap position:** phase 1 (engine bring-up) and phase 2 (world
prototype) are complete; phase 3 (survival core) is well underway. The
workspace holds 7 crates, ~90 tests, all green; the M1 dev machine runs
60 fps at a 12-column (192-block) view distance.

## Done

### Phase 1 — Engine bring-up
Vulkan 1.2 via ash + MoltenVK; swapchain, depth, camera-relative f64
rendering (floating origin); textured chunk; fly camera.

### Phase 2 — World prototype
- Async chunk pipeline: rayon generation/meshing jobs, budgeted uploads
- Greedy meshing (coverage-equivalence tested), CPU frustum culling
- Flood-fill lighting (sky + block, baked per-vertex), lamps, day/night
- Walking/swimming/collision, block place/break with target outline
- Save/load behind `WorldStore` (zstd, atomic, dirty-columns-only),
  30 s autosave, level metadata
- Debug HUD (own bitmap font + UI pipeline), perf counters

### Phase 3 — Survival core (in progress)
- **The §1 client/server split is live**: `oc-protocol` + `oc-server`
  (30 TPS authoritative thread) + client as a predicted mirror
- **Worldgen v3, MC 1.18-style multi-noise**: five warped climate
  channels, peaks&valleys fold, nested-spline terrain shaper (deep
  oceans → coasts → plains → jagged peaks), self-carving connected
  rivers, 13 biomes with altitude zoning and steep-slope surface rules,
  cheese + spaghetti caves, biome-driven tree density, offline `mapgen`
  visualizer — see [world-generation.md](server/world-generation.md)
- **Villages**: two-phase placement (region-hashed centers on flat
  friendly land, per-chunk hash-placed houses), plank houses with log
  corners, doorways and lamp-lit interiors carved authoritatively
  through terrain
- Survival stats on `bevy_ecs` (drowning, stamina, hunger, regen, respawn),
  fall damage, stat bars
- Items/recipes/game-modes/creatures all data-driven (RON registry)
- Survival inventory (gather on break, consume on place, server-validated
  with prediction rollback), crafting via the C-key recipe book
- Food & eating: apples drop from leaves (1-in-3, position-hashed),
  E eats (+3 hunger, server-validated); any item with a `food:` value
  in items.ron is edible
- Four game modes (survival/creative/adventure/spectator) as data
- Passive creatures: server wander AI + interpolated client rendering
- **Menus**: title screen, world select/create/delete (name + seed +
  starting mode + cheats), pause menu with a game-mode picker and a
  cheats toggle; menus and all UI text are data (`menus.ron`,
  `lang/en.ron`) so mods can extend both. Pausing freezes the
  singleplayer simulation (`SetPaused`; multiplayer servers will ignore
  it) and autosaves
- **Cheats/permissions, MC-style**: mode changes require the world's
  cheats flag (singleplayer) — one mechanism with multiplayer ops/admins
  (phase 4); see [game-modes.md](gameplay/game-modes.md)

## Known issues

- Worldgen changes between commits regenerate unedited terrain differently
  (only player edits persist — by design during development); saves made
  before worldgen v3 will not match their regenerated surroundings

## Not started yet

Drag-grid inventory screen, `./mods/` loader,
texture packs/skins, dedicated server binary + QUIC, palette compression,
LOD, GPU occlusion culling, physics grids. See [roadmap.md](roadmap.md).
