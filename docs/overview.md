# Overview

OpenCreate is a from-scratch, AGPL-3.0, Minecraft-class voxel game written
in Rust with its **own engine on Vulkan** (`ash`, via MoltenVK on macOS — no
Bevy, no wgpu). It is built to be a stable, extensible open-source base for
years of development.

## The three long-term differentiators

1. **Very tall worlds** — default Y range −512..+5120 (sea level at Y = 0),
   affordable because columns store sections sparsely: sky that nobody has
   built in costs nothing.
2. **Client–server from day one** — even offline singleplayer runs a real
   authoritative server on its own thread, talking over the same protocol
   multiplayer will use. Multiplayer is a transport swap, not a rewrite.
3. **Create-style extensibility** — the data model reserves first-class
   seams for physics contraptions (airships à la Create Aeronautics) and
   block power networks, and all content is data-driven so mods build the
   same way the base game does.

## Hard requirements (from §0)

- X-Z world size ≥ ±30M blocks; signed `i32` coords centered on 0,0,0
- Smooth on 16 GB + RX 9060 XT; stretch: 60 fps @ 32-chunk view on M1 Air
- Worldgen: biomes, villages, rivers, caves, oceans with creatures —
  deterministic from a 64-bit seed
- Survival: health/hunger/stamina/oxygen, 3×3 crafting
- 30 TPS authoritative server; offline-capable and multiplayer-capable
- Minecraft-compatible skins, resource-pack overlays
- `./mods/` drop-in modding: content mods are pure data, behavior mods are
  sandboxed WASM (phase 5)

## What it looks like today

A playable survival prototype: procedurally generated islands with biomes,
rivers, caves and trees; flood-fill lighting with a day/night cycle;
gather-craft-build survival with stats, fall damage and swimming; passive
creatures wandering the grass; four data-driven game modes; autosaving
worlds. See [status.md](status.md).
