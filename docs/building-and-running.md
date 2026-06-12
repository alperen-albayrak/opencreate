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

The game opens on a **title screen**: Singleplayer leads to the world
list (each world is a folder under `./saves/`), where you can open,
delete (click the delete tag twice), or **create** a world — the create
screen takes a name, a seed (numeric or any string, blank = random), the
starting game mode, and a **cheats** toggle (off by default; with cheats
off the game mode can't be changed in play, but the world owner can flip
cheats from the pause menu). Menus, labels and languages are data:
`data/menus.ron` + `data/lang/*.ron`.

**Settings** (title or pause menu) holds sliders for render distance
(4–24 chunks), field of view (50–110°), mouse sensitivity, and **UI
size** — drag or click the bar, the value prints on the right. They
apply live and persist to `./settings.ron` (per install, not per world).
All UI is laid out in DPI-aware units: the effective scale is the
display's scale factor × your UI-size setting, so 4K monitors and 4K TVs
are both readable and tunable. Worlds open through a loading screen
(server startup runs off the main thread).

| Input | Action |
|---|---|
| Esc | Pause menu (freezes the singleplayer simulation; multiplayer servers will keep running). In menus: back |
| W A S D + mouse | Move and look |
| Space | Jump; swim up; rise while flying |
| Left Shift | Descend while flying |
| Left Ctrl | Sprint (drains stamina when walking) |
| F | Toggle fly/walk (creative & spectator only) |
| Left / right click | Break / place the targeted block |
| 1–9 / mouse wheel | Select hotbar slot |
| C | Open/close the crafting recipe book (digits craft while open) |
| E | Eat an apple (survival; apples drop from leaves) |
| F3 | Toggle the debug HUD |

New worlds spawn you on the nearest dry land to the origin. In survival
you start with nothing: punch terrain to gather blocks, then build/craft.
Pausing autosaves, Minecraft-style; quitting to title runs a final save.

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
