# Modding

The strategy (§7.6): **the base game is built the way mods will be
built.** Everything gameplay-shaped is data in `data/` today, loaded
through the `oc-assets` registry; the phase-5 loader merges the same
formats from `./mods/`, so a large share of mods need zero code.

## What is data right now

| File | Defines |
|---|---|
| `data/items.ron` | Items: namespaced id, display name, optional block-state id they place |
| `data/recipes.ron` | Shaped + shapeless crafting recipes |
| `data/gamemodes.ron` | Game modes as capability-flag bundles |
| `data/creatures.ron` | Creature kinds: collision size, tint, speed |

The repo files are embedded into the binary as built-in defaults
(`Registry::load_default`); `Registry::load_from_dir` reads a directory —
the seam the mod loader will use.

## Identity rules (the Minecraft lesson, §7.6)

- **Namespaced string ids everywhere** (`oc:stone`, `mymod:copper`).
  String ids are the stable identity: saves persist them, mods reference
  them.
- **Numeric ids are per-load handles** (registry indices) used on the wire
  and in hot paths. They are never persisted. Multiplayer mod sync (phase
  5 handshake) exchanges the string→numeric mapping at join.
- Registries load once at startup and freeze; duplicate ids and dangling
  references are load **errors**, not warnings.

## The phase-5 plan

`.ocmod` packages (zip: `mod.ron` manifest + `data/` + `assets/` +
optional `code.wasm`):
1. **Content mods** — data + assets only; merged into the registries.
   Most "new blocks/items/recipes/biomes/creatures" mods are this tier.
2. **Behavior mods** — sandboxed WASM (wasmtime) registering systems,
   block behaviors and event handlers through a versioned host API,
   declared read/write sets so handlers parallelize (the Luanti
   single-threaded-Lua lesson).
3. Engine hooks (shaders, post-processing) stay data-driven, not
   sandboxed code.

Mods interact only through data files and the WASM API boundary — no
linking — so mods are independent works and may use any license despite
the game being AGPL.

## Still hardcoded (will become data)

Block definitions themselves (`oc_world::blocks` ids + solidity/opacity/
light properties), biome parameters, worldgen settings, key bindings.
Each migrates to the registry as its data format stabilizes.
