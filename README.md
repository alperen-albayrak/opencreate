# OpenCreate

An open-source, Minecraft-class voxel game written in Rust with its own Vulkan engine.

**Status:** playable survival prototype (roadmap phase 3 of 6 in progress). The original design is in [ARCHITECTURE.md](ARCHITECTURE.md); living documentation — how everything works, decisions, roadmap, gotchas — lives in [docs/](docs/README.md).

## Playing the prototype

```sh
cargo run
```

Procedural terrain streams in around you: biomes (grassland, desert, snow), rivers, oceans, beaches, trees, caves, flood-fill lighting with a day/night cycle. Worlds autosave to `./saves/` and resume on relaunch (only edited terrain is stored; the rest regenerates from the seed).

| Input | Action |
|---|---|
| Click window / Esc | Capture / release the mouse |
| W A S D + mouse | Move and look |
| Space | Jump, swim up (or rise while flying) |
| Left Shift | Descend while flying |
| Left Ctrl | Sprint |
| F | Toggle fly/walk |
| Left / right click | Break / place block |
| 1 2 3 4 | Select stone / dirt / grass / lamp |
| F3 | Toggle the debug HUD |

## Goals

- Own engine on Vulkan (`ash`; MoltenVK on macOS) — targets 60 fps at 32-chunk render distance, including on Apple Silicon
- Huge worlds: ±30M+ blocks horizontally, ~5600 blocks of vertical build range (sea level at Y 0) via sparse storage
- Survival gameplay: biomes, villages, rivers, caves, big mountains, crafting, health/hunger/stamina/oxygen
- Client–server from day one: fully offline singleplayer and Minecraft-style multiplayer share one protocol
- Multiple dimensions per world, data-driven content (RON), texture packs, Minecraft-compatible player skins
- Modding first: drop-in `./mods/` folder (data + sandboxed WASM), designed for an open ContentDB/Modrinth-style ecosystem
- Extensible toward physics contraptions (airships à la Create Aeronautics) and block power networks

## License

[AGPL-3.0](LICENSE). Mods interact only through data files and the WASM API boundary, so mods are independent works and may use any license.
