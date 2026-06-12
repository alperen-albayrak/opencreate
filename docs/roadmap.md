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
passive creatures, MC 1.18-style multi-noise worldgen, villages
(two-phase placement: region-hashed centers, per-chunk hash-placed
houses with lit interiors), food & eating (apples from leaves, data-driven
`food:` values).
Remaining:
- **Inventory screen** — drag-and-drop grid UI + real crafting grid
- **Ocean creatures** — fish with water movement AI
- Local skin + texture pack selection (overlay stack)

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
- Per-vertex ambient occlusion; smooth lighting
