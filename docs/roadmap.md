# Roadmap

Six phases, each independently shippable (§12). Strikethrough = done.

## ~~Phase 1 — Engine bring-up~~ ✅
Window + Vulkan clear → textured test chunk → fly camera. MoltenVK
pitfalls solved here (see [conventions.md](conventions.md) for the ones
that bit).

## ~~Phase 2 — World prototype~~ ✅
Chunk pipeline, terrain with biomes/rivers/trees/caves, walking/collision,
place/break, save/load, day cycle, lighting, greedy meshing, debug HUD.
The first "it's a game" build.

## Phase 3 — Survival core (current)
Done: client/server split, stats, inventory, crafting, game modes,
passive creatures, multi-noise worldgen, villages
(two-phase placement: region-hashed centers, per-chunk hash-placed
houses with lit interiors), food & eating (apples from leaves, data-driven
`food:` values).
Remaining:
- **Inventory screen** — drag-and-drop grid UI + real crafting grid
- **Ocean creatures** — fish with water movement AI
- Local skin + texture pack selection (overlay stack)

## Graphics roadmap — "vibrant visuals" (runs alongside phases 3–5)

Researched against modern voxel renderers (deferred-PBR upgrades and the
shaderpack playbook); adapted to our forward Vulkan renderer. Five
stages, each independently shippable, every effect behind a graphics
setting with a cheap tier so the M1/60 fps budget holds.

**A. HDR foundation + Graphics settings tab** *(prerequisite)*
Offscreen HDR target (+ sampleable depth) and a final tonemap pass
(ACES + dither); UI renders at native resolution afterwards. Unlocks:
**resolution scale** (render at 0.5–2.0×, UI stays sharp), **max FPS
cap**, and a tabbed settings screen — *Game* / *Graphics* (resolution
scale, fps cap, render distance, plus every toggle below as it lands).

**B. Water v2** *(top priority)*
Own translucent pass reading the opaque color + depth snapshot:
scrolling procedural wave normals (world-space, seam-free), Schlick
fresnel, sky reflection + high-exponent sun glint, screen-space
refraction with the depth guard, per-channel Beer–Lambert depth
absorption (shallow turquoise → deep blue), soft shorelines via depth
fade. Water setting: low (flat) / normal / high; ultra adds SSR later.

**C. Sky & atmosphere**
One reusable `sky(direction)` function feeding the sky pass, water
reflections and fog. **Celestials**: sun disc with glow and sunset
band; **moon with phases**, opposite the sun; procedural star field
that fades in at dusk; **real-world constellations** — embed a small
bright-star catalog (RA/Dec of the ~300 brightest stars) projected on
the rotating sky dome, so Orion, the Big Dipper and Cassiopeia appear
where they belong. **Fog**: exponential height + distance fog colored
by `sky()` (terrain dissolves into atmosphere; permanently hides chunk
pop-in). **Clouds**: scrolling blocky noise layer with an on/off
toggle; 2.5D raymarched slab as a later "volumetric" tier.

**D. Lighting**
Per-vertex ambient occlusion (corner darkening; AO joins the greedy
merge key, diagonal flip against anisotropy) — the look that makes
blocks read as solid. Then cascaded sun shadows: 3×2048 cascades, PCF,
texel snapping against shimmer, normal-offset bias tuned for blocky
geometry, twilight fade, multiplied by voxel skylight so caves never
leak. Shadow setting: off / normal / high.

**E. Post & polish**
Bloom (downsample chain — the sun halo), auto-exposure (dark caves,
dazzling exits), AA options (off / FXAA / TAA + sharpen). Ultra tier:
SSR water, volumetric clouds, god rays, SSAO, per-biome color grading.
PBR texture channels (normal/roughness/emissive per texture) join with
texture packs in §7.5.

## Phase 4 — Multiplayer
`postcard` serialization + QUIC (`quinn`) behind the existing `Transport`
trait; dedicated headless server binary; prediction/reconciliation
hardening; interest management for many players; skin/pack distribution
via asset sync. The protocol boundary already exists, so this phase is
mostly transport + polish.

## Phase 5 — Depth & modding
LOD far terrain, GPU occlusion culling, combat/hostile mobs, more
structures, audio; the `./mods/` loader ships content-mod support (data +
assets merged into the registry) and the WASM behavior-mod API (§7.6).

## Phase 6 — Physics & machines
`rapier3d` + Sable-style voxel octrees → physics assembler → airships;
then block networks (rotational power, electric). Ordered after
multiplayer because moving grids must be designed against replication.

## Engine debts (no phase, do when they hurt)
- Palette-compressed sections (storage is flat u16 per voxel today)
- Pooled vertex buffers + multi-draw-indirect (per-chunk binds today)
- Region files replacing the folder store (same trait)
