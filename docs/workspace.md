# Workspace Map

```
opencreate/
├── crates/
│   ├── oc-core       # shared vocabulary: coords, constants (16³ sections, 30 TPS)
│   ├── oc-world      # world storage, terrain gen, lighting, physics, raycast, persistence
│   ├── oc-assets     # data-driven content registry: items, recipes, game modes, creatures
│   ├── oc-protocol   # client⇄server messages + Transport trait + in-proc channel
│   ├── oc-server     # authoritative simulation: 30 TPS tick, ECS, stats, creatures, saves
│   ├── oc-renderer   # the engine: Vulkan via ash, meshing, pipelines, UI, font
│   └── oc-client     # game client: window/input, prediction, streaming, hotbar, HUD
├── bins/opencreate   # the game (client + embedded server)
├── data/             # RON content: items, recipes, gamemodes, creatures (embedded as defaults)
└── docs/             # you are here
```

Planned crates (per §2, not yet created): `oc-worldgen` (when generation
outgrows `oc-world/terrain.rs`), `oc-net` (QUIC, phase 4), `oc-mods`
(phase 5), `bins/opencreate-server` (phase 4).

## Dependency rules (the boundaries that keep it stable)

- **`oc-core` depends on nothing** in the workspace; everything depends on it.
- **`oc-renderer` never sees game logic.** It consumes meshes, transforms,
  draw lists and UI primitives. It depends on `oc-world` only for
  `BlockId`/`Section` vocabulary used by the mesher.
- **`oc-server` never links Vulkan/winit.** It must compile headless on a
  VPS. (Check: `cargo build -p oc-server` pulls no graphics crates.)
- **`oc-protocol` is the only way client and server talk** — even in the
  same process. No back-channel shortcuts.
- **`oc-assets` owns identity**: string ids are stable, numeric ids are
  per-load handles. Anything crossing the wire or the save file uses the
  appropriate one (see [decisions.md](decisions.md)).

## Crate cheat-sheet

| Crate | Key modules |
|---|---|
| oc-core | `coords` (block→section/chunk math, negative-safe) |
| oc-world | `world` (sparse storage + column gen), `terrain` (noise/biomes/rivers/caves/trees), `light` (flood-fill), `physics` (swept AABB), `raycast` (DDA), `section`, `store` (persistence) |
| oc-assets | `lib` (Registry: items, recipes + matching, modes, creatures) |
| oc-protocol | `lib` (messages, `Transport`, `in_proc_channel`) |
| oc-server | `lib` (tick loop, subscriptions, edits, persistence), `stats`, `falling`, `creatures` |
| oc-renderer | `lib` (frame orchestration), `context`, `swapchain`, `depth`, `chunk_renderer`, `mesh` (greedy), `texture`, `outline`, `entity`, `ui`, `font` |
| oc-client | `lib` (app/frame loop/edits), `streaming`, `player`, `camera`, `sky`, `hotbar`, `craft_menu`, `entities` |
