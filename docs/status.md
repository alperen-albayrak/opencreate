# Status

*Last updated: 2026-06-15.*

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
- **Worldgen v3, multi-noise**: five warped climate
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
- Survival inventory: gather on break, consume on place (server-validated,
  client-predicted). The **E/C inventory screen** holds 36 real per-slot
  stacks (a configurable 9-slot hotbar + 27 main), a 3×3 crafting grid with
  a result slot, a cursor for moving/splitting/merging stacks, and a
  watching paper-doll; the server is authoritative (full resync per change)
- **Creative inventory**: a tabbed all-items palette (category + Search tabs)
  with infinite stacks, a trash slot on the Inventory tab, and a real
  configurable hotbar/inventory filled from the palette; placing never
  decreases counts
- Food & eating: apples drop from leaves (1-in-3, position-hashed),
  G eats (+3 hunger, server-validated); any item with a `food:` value
  in items.ron is edible
- Four game modes (survival/creative/adventure/spectator) as data
- Passive creatures: cows & sheep as data-driven quadruped models
  (server wander AI + interpolated client rendering)
- **Menus**: title screen, world select/create/delete (name + seed +
  starting mode + cheats), pause menu with a game-mode picker and a
  cheats toggle; menus and all UI text are data (`menus.ron`,
  `lang/en.ron`) so mods can extend both. Pausing freezes the
  singleplayer simulation (`SetPaused`; multiplayer servers will ignore
  it) and autosaves
- **Cheats/permissions**: mode changes require the world's
  cheats flag (singleplayer) — one mechanism with multiplayer ops/admins
  (phase 4); see [game-modes.md](gameplay/game-modes.md)
- **Settings** (`settings.ron`): render distance, FOV, mouse sensitivity
  and UI size as sliders (value shown right, live apply); all UI is
  DPI-aware (display scale × UI-size setting) for 4K monitors/TVs.
  Worlds start behind an async loading screen
- **Graphics stage A (HDR foundation)**: the world renders into an
  offscreen HDR target (B10G11R11) resolved by an ACES tonemap pass;
  resolution scale (0.5-2.0, UI stays native) and a max-FPS cap live in
  the new Graphics settings tab — see the graphics roadmap
- **Graphics stage B (water v2)**: water meshes split from solids and
  drawn in their own blended pass with animated wave normals, fresnel,
  sky reflection, sun glint, and — from the sampled opaque depth —
  Beer-Lambert absorption (shallow turquoise to deep blue), soft
  shorelines and in-shader occlusion
- **Graphics stage C (sky & atmosphere)**: one `sky()` function feeds the
  sky pass, water and fog — sun disc + glow, height/distance fog (hides
  chunk pop-in), blocky cloud layer; **celestials**: moon with 8 phases +
  halo, a procedural star field, and a real bright-star catalog (Orion, the
  Big Dipper, Cassiopeia…) rotating with the day; directional dusk
- **Far-terrain LOD**: a seed-generated blocky ring beyond the loaded
  chunks (Voxy / Distant-Horizons style), drawn after the chunks and
  discarded inside the loaded square; fog saturates near its edge. Toggle
  in graphics settings
- **Graphics stage D (lighting)**: per-vertex ambient occlusion baked into
  the mesh (corner darkening). Cascaded sun shadows were built but
  **shelved** — forced off, the pass kept dormant, no settings entry
- **Graphics stage E (post & polish)**: dual-Kawase bloom pyramid,
  auto-exposure (the eye adapts to caves and bright exits), and SSR water
  reflections (settings toggle)
- **Sound**: fully synthesized at startup — per-surface footsteps, dig/
  place, eat, splash, menu clicks, looping wind/underwater ambience; master
  volume slider; `data/sounds/*.wav` overrides the synthesis (sound packs)
- **Underwater view**: Java-1.13 one-per-block water light plus an
  eye-adjusting blue fog; the sky dome goes blue and clouds turn off while
  submerged

## Known issues

- Worldgen changes between commits regenerate unedited terrain differently
  (only player edits persist — by design during development); saves made
  before worldgen v3 will not match their regenerated surroundings

## Not started yet

Image skins + a texture-pack picker (color-set skins and a blocky player
avatar already exist), the `./mods/` loader, a dedicated server binary +
QUIC, palette compression, GPU occlusion culling, and physics grids. See
[roadmap.md](roadmap.md).
