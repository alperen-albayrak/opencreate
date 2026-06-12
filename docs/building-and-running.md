# Building & Running

## Prerequisites

- Rust (edition 2024 toolchain)
- macOS: MoltenVK (e.g. `brew install molten-vk` / the Vulkan SDK); the
  loader is found via the system path with a fallback to
  `/opt/homebrew/lib/libvulkan.dylib`
- Linux/Windows: a Vulkan 1.2 driver

Shaders are WGSL, compiled to SPIR-V at build time by `naga` in
`oc-renderer/build.rs` — no external shader toolchain needed.

```sh
cargo build            # build everything
cargo test --workspace # all tests are headless (no GPU needed)
cargo run              # play
```

`RUST_LOG=info cargo run` prints the perf log (fps, worst frame, chunk
counts) every 5 s, plus server lifecycle messages.

## Playing

| Input | Action |
|---|---|
| Click window / Esc | Capture / release the mouse |
| W A S D + mouse | Move and look |
| Space | Jump; swim up; rise while flying |
| Left Shift | Descend while flying |
| Left Ctrl | Sprint (drains stamina when walking) |
| F | Toggle fly/walk (creative & spectator only) |
| G | Cycle game mode (survival → creative → adventure → spectator) |
| Left / right click | Break / place the targeted block |
| 1–9 / mouse wheel | Select hotbar slot |
| C | Open/close the crafting recipe book (digits craft while open) |
| F3 | Toggle the debug HUD |

New worlds start in survival on the nearest dry land to the origin. You
start with nothing: punch terrain to gather blocks, then build/craft.

## Saves

`./saves/world/` relative to the working directory:
- `columns/c.X.Z.ocz` — zstd-compressed edited columns (only player-edited
  terrain is stored; everything else regenerates from the seed)
- `level.txt` — seed, time of day, player position/look, game mode

The server autosaves every 30 s and on window close. Delete the folder for
a fresh world. The default seed is fixed in `oc-client` (`WORLD_SEED`)
until a world-selection UI exists.

## Dev profile note

`profile.dev` builds this workspace at `opt-level = 1` with dependencies at
`opt-level = 3` — meshing and worldgen are unusable at `-O0`.
