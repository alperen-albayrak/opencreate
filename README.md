# OpenCreate

An open-source, Minecraft-class voxel game written in Rust with its own Vulkan engine.

**Status:** architecture/design phase — no code yet. The full design is in [ARCHITECTURE.md](ARCHITECTURE.md).

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
