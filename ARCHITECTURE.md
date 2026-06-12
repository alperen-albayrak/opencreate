# OpenCreate — Architecture Design Plan

## Context

A from-scratch, AGPL-3.0 licensed voxel survival game in Rust with its **own engine** on **Vulkan**. This document is the architecture-level design — the stable foundation everything else builds on. No code yet; this is the discussion artifact.

**Hard requirements**
- Very tall build range — well beyond the genre-typical ~384: target **~5000+ blocks of sky headroom** (skyscrapers with LOD) while staying inside the M1/16 GB budget; X-Z world size ±30M+ blocks
- Smooth on 16 GB RAM + RX 9060 XT-class GPU; stretch: 60 fps @ 32-chunk render distance on M1 MacBook Air
- World generation: biomes, villages, rivers, lakes, trees, oceans with creatures
- Survival systems: health, hunger, stamina, oxygen (diving); classic 3×3 grid crafting
- Fully offline-capable, and multiplayer-capable with dedicated servers; 30 TPS server tick
- Customization: player skin files, texture/resource packs
- Modding like NeoForge's UX: drop a mod in `./mods/`, later a Modrinth-style open store
- Extensible toward Create Aeronautics-style physics contraptions (airships) and Create-style power networks — not in baseline, but architecture must not preclude them
- Stable, extensible base — designed to be built upon for years

**Lessons adopted from research**
- **Hytale**: server-authoritative ECS as the core runtime model; singleplayer = local in-process server; data-driven assets (JSON, hot-reloadable); "build the game within the game" — base content uses the same data/plugin paths mods will use.
- **Luanti (ex-Minetest)** — 15+ years of proof for our two biggest bets: it has *always* run client–server even in singleplayer (validates §1), uses 16³ map blocks (validates §3), and its **ContentDB** (3000+ mods, in-game browser, one-click install) is the working template for our Modrinth-style store. Its cautionary lessons we design against: gameplay scripting runs on a **single thread** (the #1 server scaling complaint — our ECS systems and worldgen are parallel from day one, and the mod API is designed not to force serialization); its Irrlicht-era renderer lags modern hardware (we build GPU-driven Vulkan); and it became an "engine first, no flagship game," which stunted its audience — **we ship a game first**; the platform falls out of the data-driven architecture.
- **Modern voxel rendering** (Sodium, Vercidium, Exile, Aokana): binary greedy meshing, heavily packed vertex formats, pooled GPU memory with multi-draw-indirect, GPU-driven culling, LOD for far terrain.

---

## 1. Top-level architecture: authoritative server + thin-ish client

The single most important decision. The game is **client–server from day one**, even offline:

```
┌────────────────────────── one process (singleplayer) ──────────────────────────┐
│                                                                                │
│  ┌─────────────┐   oc-protocol messages    ┌──────────────────────────┐        │
│  │  oc-client   │ ◄──── in-proc channel ───►│        oc-server         │        │
│  │ render/input │                           │ world sim · ECS · saves  │        │
│  └─────────────┘                            └──────────────────────────┘        │
└────────────────────────────────────────────────────────────────────────────────┘
            ▲  same protocol, different transport  ▼
        QUIC/TCP socket ◄────────────────► dedicated headless server binary
```

- **Offline play** = client + server in one process, connected by an in-memory channel. Zero network stack, zero latency, no compromise.
- **Multiplayer** = the exact same protocol over QUIC. The dedicated server is the same `oc-server` crate compiled headless (no Vulkan dependency — important for cheap Linux hosting, and AGPL makes server source availability a feature, not a burden).
- **Server is authoritative** for all game state (blocks, entities, health/hunger, inventory). Client predicts movement and block edits locally, server reconciles. This kills the retrofit pain that has ended many voxel projects, and prevents cheating later.

**Tick model**: server simulates at a fixed **30 TPS** (user choice; smoother than the genre-typical 20, still cheap — sim runs on its own thread and can never hitch rendering); client renders at uncapped/vsync fps with interpolation between server states. Fixed tick = deterministic-ish gameplay, simple save semantics, easy to reason about.

## 2. Cargo workspace layout

```
opencreate/
├── crates/
│   ├── oc-core       # shared vocabulary: ids, registries, coords, math, config
│   ├── oc-world      # chunk/palette storage, block state, lighting data
│   ├── oc-worldgen   # noise stack, biomes, carvers, features, structures
│   ├── oc-protocol   # client⇄server message types + serialization (transport-agnostic)
│   ├── oc-server     # authoritative sim: ECS, ticking, persistence, chunk service
│   ├── oc-net        # QUIC transport (quinn); in-proc channel transport
│   ├── oc-renderer   # the engine: Vulkan (ash), meshing, frame graph, camera
│   ├── oc-client     # game client: input, prediction, UI/HUD, audio (later)
│   ├── oc-assets     # data-driven content loading: blocks, items, recipes, biomes
│   └── oc-mods       # mod discovery/manifest/load-order + WASM host (grows in phase 5)
├── bins/
│   ├── opencreate    # the game (client + embedded server)
│   └── opencreate-server  # headless dedicated server
├── data/             # RON/JSON content: blocks/, items/, recipes/, biomes/, structures/
└── mods/             # drop-in mod folder (created at runtime next to the executable/save dir)
```

Boundary rules that keep it stable:
- `oc-renderer` never sees game logic; it consumes "here are chunk meshes + entity transforms".
- `oc-server` never links Vulkan/winit. Compiles on a VPS with no GPU.
- `oc-protocol` is the only way client and server talk — even in-process. This is the contract that makes multiplayer "already done" architecturally.
- `oc-core` is dependency-light and everything depends on it; nothing in it depends on the rest.

## 3. World model

### Coordinates (centered at 0,0,0 — user decision)
- **The world is centered on the origin: block coords are signed `i32` on all three axes** (±2.1 billion address space). Spawn is near 0,0; the playable X-Z border is per-dimension config (default ±32M, raisable any time since the address space is already there).
- Vertical: signed Y with **sea level at Y = 0** (a nice property of centered coords — altitude reads like real-world elevation; compare the usual −64..320 with sea level at 62). Per-dimension `min_y..max_y`, **default −512 .. +5120**: modest depth below sea level (the default generator does *not* dig 500-block oceans — ocean floors sit around −30..−60, deep oceans maybe −100, caves bottoming near −300), and a huge sky for megabuilds. A Subnautica-style mod dimension can configure −4000..+200 instead; the engine doesn't care (see "why tall is cheap" below).
- **Entity positions: `f64` absolute, in a reference frame** — `(FrameId, DVec3)`. Frame 0 = the dimension's static grid (mainstream voxel games use plain absolute doubles too; even at 64M blocks f64 resolves to ~7.5 *nanometers*, so absolute f64 is genuinely fine — no chunk-relative scheme needed on the server). The `FrameId` exists for one reason: an entity standing on a **moving physics grid** (airship — see §6.5) is positioned *in that grid's local frame* and inherits its motion. Frame-relative from day one costs nothing and makes contraptions possible later.
- Rendering: **camera-relative `f32`** (floating origin) — the GPU only ever sees positions relative to the camera, so precision never degrades far from spawn. This must be baked in from the first triangle; retrofitting is miserable. (Chunk-relative encoding is also used on the wire to keep entity packets small — an encoding detail, not a data-model one.)

### Dimensions (overworld / nether / end style — user requirement)
A world is **a set of dimensions**; each dimension owns its grids, entities, time/sky settings, and worldgen config. Architecture:
- **Data-driven**: dimensions are defined in `data/dimensions/*.ron` — id, Y range (`min_y..max_y`, e.g. overworld −512..+5120, a nether-like −128..+128 with a ceiling, a Subnautica-like −4000..+200), sky/ambient parameters, which generator config to use, spawn rules. Mods can therefore add whole dimensions as content (§7.6).
- **Addressing**: chunk storage is keyed `(DimensionId, GridId, ChunkPos)`; an entity's frame resolves through its dimension. Only the player's current dimension is loaded around them; the server simulates a dimension only while someone (or something marked persistent) is in it.
- **Persistence**: one subfolder per dimension inside the world save (`world/dim/overworld/r.*.ocr`, …), same region format everywhere.
- **Protocol**: a `ChangeDimension` message tears down/re-streams chunks — same machinery as a long-range teleport, so it's nearly free once chunk streaming exists.
- **Baseline ships the overworld only**; portals + a second dimension are content for the depth phase. The IDs and save layout exist from day one because retrofitting dimension keys into every system later is exactly the kind of rewrite this plan exists to avoid.

### Grids (the Create Aeronautics seam)
Each dimension is **a set of voxel grids**: one huge static grid (`GridId 0`) plus, later, N small dynamic grids with rigid-body transforms (airships, vehicles). All chunk storage is keyed by `(DimensionId, GridId, ChunkPos)`. Baseline only ever creates grid 0 per dimension — but because *nothing in the codebase assumes "the one global grid"*, physics contraptions later are an addition, not a rewrite. See §6.5.

### Chunks — sparse columns (why a 5000+ block sky is affordable)
- **16×16×16 sections**, grouped into **16×16 columns with *sparse* vertical storage**: a column holds a sorted map of only its *non-trivial* sections, not a fixed array spanning the Y range. The dimension's Y range is just address space.
- Three section states, and this is the whole trick:
  - **absent** — all air (or unexplored sky): costs *zero* bytes. 90%+ of a tall world is this.
  - **uniform** — one block state everywhere (deep stone, ocean interior): costs ~16 bytes (palette of 1, no bit array). Uniform-light sections (pitch-dark underground, full-bright sky) likewise omit light arrays.
  - **detailed** — palette + packed bit array (1–8 bits/voxel typical, ~0.5–8 KB) + light only where light actually varies.
  So raising the sky from 1024 to 5120 costs **nothing until someone builds there** — and then only for the sections they touch. Skylight shafts through empty space are computed per-column analytically (everything above the heightmap = light 15), not stored.
- Why 16³ and not 32³: meshing/lighting/network granularity stays small, sparse skipping is finer-grained, and the entire modding/tooling ecosystem intuition transfers.
- **Block states**: a global registry (string id → numeric id, data-driven from `data/blocks/`) + per-block properties (orientation, waterlogged, growth stage) encoded as state ids.

### Lighting
- Classic voxel **block light + sky light, 4 bits each per voxel**, computed by flood-fill (BFS) on the server/meshing side, baked into chunk mesh vertices. Simple, proven, fast, and it's what makes caves/underwater feel right. Fancy GI can layer on top years later without changing the data model.

### Memory math (validates the hardware targets)
- Realistic loaded column with default worldgen (surface band ≈ −300..+1500 of *potential* content, mostly uniform/absent outside ~20–40 detailed sections near the surface): ~150–300 KB — **independent of the dimension's Y range** thanks to sparse columns.
- 32-chunk render distance = 65×65 ≈ 4.2K columns ≈ **0.7–1.3 GB** world data + ~0.5–1 GB meshes/GPU buffers + game/ECS overhead → comfortably inside 16 GB on the M1 MacBook (and Apple Silicon shares RAM/VRAM, so no double copy).
- Player-built megastructures (a 5000-block skyscraper) add memory proportional to *what was built*; the render side is bounded by the same per-frame mesh/upload budgets, and far/tall geometry falls into the LOD system (§4.6) like distant terrain does.

## 4. The engine (oc-renderer): Vulkan via ash

- **`ash`** (raw Vulkan bindings) — this is "our own engine," not an abstraction layer. **Vulkan 1.2 baseline** with only features MoltenVK supports, so macOS works through **MoltenVK** (Vulkan-on-Metal). Concretely: no geometry shaders, conservative descriptor-indexing use, handle `VK_KHR_portability_enumeration`/`portability_subset` at instance/device creation. Windows/Linux get native Vulkan.
- `winit` for windows/input, `gpu-allocator` for VRAM, `glam` for math. That's the whole engine dependency story.

### Voxel rendering pipeline (the part that hits 60 fps)
1. **Binary greedy meshing** per 16³ section (bitmask-based face culling + greedy quad merging — the current state of the art, microseconds per section).
2. **Packed vertices**: position-in-section + face + texture id + light packed into ~8 bytes/vertex. A 2D **texture array** (not atlas — no bleeding, trivial mipmaps) for block faces.
3. **One big pooled vertex buffer** per pass + `vkCmdDrawIndexedIndirect`: thousands of chunk sections drawn in a handful of draw calls.
4. Culling: **CPU frustum culling at section granularity** for milestone 1 (cheap, sufficient); GPU occlusion culling (Hi-Z compute) as a later upgrade — slot for it exists in the frame graph from day one.
5. Passes: opaque → cutout (leaves/grass) → water/transparent (sorted back-to-front per section), simple ordered alpha for milestone 1.
6. **LOD beyond render distance** (Distant-Horizons-style merged far terrain) is a later-phase feature (roadmap phase 5); the chunk pipeline is designed so far columns can be downsampled without new data formats.
7. Day/night cycle: sky-light scaling + sun-direction ambient in the shader — cheap, ships in milestone 1.

### Async chunk pipeline
`rayon`-pooled stages: **generate → light → mesh → GPU upload**, prioritized by distance-to-player, with a per-frame upload budget (e.g. ≤4 MB/frame) so loading never hitches the frame. Eviction = save + drop beyond render distance + margin.

## 5. World generation (oc-worldgen)

Deterministic from a 64-bit seed; pure function of (seed, position) so it's unit-testable and multiplayer-consistent.

1. **Multi-noise biome system**: 5 noise channels — continentalness, erosion, peaks/valleys, temperature, humidity — looked up against a biome table defined in `data/biomes/`. Launch biome set: plains, forest, desert, mountains, snowy, ocean, beach, river.
2. **Terrain**: heightmap from the noise stack, plus **3D density noise** for overhangs and sheer cliffs, and **cave carvers** (3D noise "cheese + spaghetti" caves, plus large cavern biome-like zones at depth). With sea level at Y0, the default generator uses roughly −300..+1500: caves and deep oceans in the negative band (kept shallow by design — no 500-block default oceans), and the peaks/valleys channel pushing **genuinely big mountains** to +800..+1500 — a headline feature, tuned aggressively. Everything above is open sky for builders (up to +5120), and mod dimensions can re-budget the bands freely.
3. **Rivers & lakes**: river = its own noise channel carving below the water table where it crosses terrain (the modern multi-noise approach; robust); lakes = local depressions filled to the water table. Oceans come free from low continentalness.
4. **Features** (per-chunk decorations, ordered, deterministic): trees (per-biome shapes), grass/flowers, ores by depth bands, kelp/seagrass in oceans.
5. **Structures — villages**: the classic cross-chunk problem, solved with the standard **two-phase approach**: phase 1 picks structure origins + bounding boxes from seed alone (no chunk data needed); phase 2, any chunk that intersects a bounding box materializes its slice. Villages = **jigsaw-lite**: hand-built piece templates (saved as voxel snippets) connected via sockets — roads, houses, farms, wells — placed on terrain flat-enough heuristics. This same machinery later gives dungeons, ruins, etc.
6. **Creatures/spawning**: biome-driven spawn tables in data (`data/biomes/*.ron` lists spawnable creatures + weights); ocean creatures (fish etc.) are just entities with water-movement AI, spawned by the same rules.

## 6. Game simulation: ECS on the server

- **`bevy_ecs` used standalone** (no Bevy engine, just the ECS library): archetypal storage like Flecs (Hytale's choice), mature scheduler, change detection, used in production by many non-Bevy projects. We own the engine; we don't need to own the ECS. (Lean alternative if we want fewer deps: `hecs` + hand-rolled scheduling — my call is bevy_ecs, discuss below.)
- Everything dynamic is an entity: players, creatures, dropped items, later projectiles/vehicles.
- **Survival stats as components + systems**, all server-side:
  - `Health` — damage events (fall, drowning, later combat), regeneration tied to hunger.
  - `Hunger` — depletes with activity (sprinting/jumping cost more), eating restores; starvation damages.
  - `Stamina` — drains on sprint/swim-burst, regenerates at rest; gates sprinting.
  - `Oxygen` — depletes while head is in water voxels, refills at surface; drowning damage at zero. (Voxel-aware: the system just samples the block at eye position — this is why stats live next to the world on the server.)
- Client receives stat updates via protocol and renders HUD; it never computes them.

## 6.5 Physics grids & block networks (Create Aeronautics-style extensibility)

Not in the baseline — but the baseline is shaped so these bolt on cleanly. Research findings on how the mods actually work:

- **Create Aeronautics / Create: Simulated**: a *Physics Assembler* converts glue-connected blocks into a **"Physics Contraption" (sublevel)** — an independent block grid freed from the world grid, simulated by **Sable**, a custom rigid-body physics engine built for block contraptions. Blocks stay fully interactive while the structure moves (propellers, hot-air lift, levitation rocks → airships/planes).
- **Valkyrien Skies**: same concept — ship blocks live in a far-away **"shipyard"** region of the normal world grid and are *projected* into the world with a position+rotation transform; a custom physics engine handles rigid-body collision between ships and terrain. Blocks "believe they're grid-aligned" so all vanilla mechanics keep working. An *assembly finder* scans connected blocks to form a ship.

### How Sable actually does it (from reading its source — local clones at `~/_e/dev/sable`, `~/_e/dev/Simulated-Project`)

⚠️ **License**: Sable is Polyform Shield 1.0.0 and Simulated's assets are All-Rights-Reserved — *not* AGPL-compatible. We learn the architecture; we never copy code.

Sable's physics core is **Rust** (called from Java over JNI — a bridge we won't need):

1. **Solver = forked `rapier3d`** with `simd` + `parallel` features and CCD enabled. They did *not* write a rigid-body solver; they wrote voxel-aware collision detection that feeds Rapier. This resolves our open question: **`rapier3d` it is.**
2. **Solidity octrees per grid** (crate `marten`): every chunk region of both the world and each contraption maintains a flat-array, cache-coherent octree of solid voxels (node = branch index / leaf block-id / empty), plus a **separate octree for liquids**. Built/updated incrementally as blocks change.
3. **Mid-phase = octree-vs-octree traversal**: walk the contraption's octree top-down; each node's bounding sphere is transformed by the body's pose and tested against the other grid's octrees at matching power-of-two granularity; leaves emit `(static_block, dynamic_block)` overlap pairs. Levels with ≥256 nodes fan out across `rayon`. Pairs become box–box contacts in Rapier using **per-blockstate collision data** registered up front (friction, restitution, volume, fluid flag, list of collision AABBs, optional contact callbacks into game code).
4. **Buoyancy & drag reuse the same machinery** against the *liquid* octree: each overlapping voxel contributes an upward force ∝ (AABB overlap volume × the block's displacement volume) and a drag force ∝ velocity-at-point × overlap volume, with 8-point sub-sampling for small bodies. This is why their airships *feel* good in water and air — it's volumetric, not a single buoyancy point.
5. **Force-group API between game and solver**: gameplay features queue named force groups (e.g. `BALLOON_LIFT`) carrying linear+angular impulses and recorded point-forces (which also powers their force-visualizer debug tool). The hot-air balloon (Aeronautics, game side) detects the enclosed envelope via a layered flood-fill graph, tracks lifting-gas amounts/temperature per gas type fed by heater blocks, smooths lift over time (destroying half the balloon doesn't snap the force), and applies the result as one force group at the gas centroid. **The split — solver knows voxels and forces, game knows balloons and gases — is exactly the mod-API seam we want**, so mods can add new force sources without touching physics internals.

**What this means for our architecture — five seams we cut now, fill later:**

1. **Grids are first-class** (§3): chunk storage keyed by `(GridId, ChunkPos)`. A contraption = a new small grid + a `RigidBody` component (transform, velocity, angular velocity, mass/inertia computed from its blocks). VS's "shipyard hack" exists because its host game *couldn't* do this; we can do it natively.
2. **Entity frames** (§3): entity positions are `(FrameId, DVec3)`, so a player standing on an airship is parented to the airship's frame and moves with it for free.
3. **Renderer draws grids, not "the world"**: every chunk mesh is drawn with its grid's model transform (identity for grid 0). Camera-relative rendering already requires a per-draw transform, so moving grids cost the renderer *nothing* architecturally.
4. **Physics layer slot**: baseline ships simple AABB-vs-voxel character collision behind a `physics` module boundary. In the airships phase, `rapier3d` plugs in behind that boundary as the solver, and we build the Sable-proven voxel layer on top: per-grid solidity octrees (+liquid octrees) maintained incrementally on block change, octree-vs-octree pair finding as the mid-phase, per-blockstate collision data from the block registry, volumetric buoyancy/drag, and a force-group API for gameplay/mods. No JNI bridge needed — we're Rust end-to-end, which removes Sable's biggest tax.
5. **Block access is grid-local**: block behaviors and neighbor queries go through a grid-scoped accessor (never raw global coords), so a furnace or door works identically on the ground or mid-flight.

**Block networks (Create's kinetic/"electric" systems)**: Create's rotation/stress and wire-style power are *graphs over connected blocks*, updated incrementally on placement/removal. We reserve a `BlockNetwork` subsystem concept in `oc-world`: a registry of network types (later: rotational power, electric, fluid pipes), each maintaining connected-component graphs per grid with event-driven updates (the VS lesson: recalculate on change, never poll). Baseline includes the *hooks* (block place/remove events carry network metadata from block data files) but implements no network types.

## 6.6 Block ticking, liquids & active areas (from Luanti's source — local clone at `~/_e/dev/luanti`)

Luanti (LGPL-2.1, 15 years in production) has a complete, battle-tested answer for the subsystem every survival feature depends on. Our adaptation:

- **Active area**: only chunks within a radius of players (smaller than render distance) are *simulated*. Everything below runs only there; the rest of the loaded world is inert. This is what keeps a 30 TPS tick affordable at huge render distances.
- **Random ticks** (crops grow, grass spreads, fire): each active section gets N random voxel samples per tick; blocks declare a `random_tick` behavior in their data file. Luanti's ABM trigger model is worth copying for mods: triggers declared as *data* (which block contents, required/forbidden neighbors, interval, 1-in-N chance) so the engine can scan efficiently and mods never poll. Its "simple catch-up" trick — simulating missed triggers when a chunk reactivates so crops don't freeze while you're away — comes along too.
- **Scheduled block timers** (furnace finishes in 8s, sapling grows at T+x): per-chunk timer lists, **serialized with the chunk** so they survive unload/reload, with elapsed-time catch-up on activation.
- **Block update events**: placing/removing a block notifies neighbors (doors, torches falling, water starting to flow) — the same event stream the §6.5 block networks and WASM mods subscribe to.
- **Liquids**: a dedicated **liquid update queue** (Luanti: `transforming_liquid` + a `ReflowScan` pass that rebuilds pending flows when a chunk loads), budgeted per tick so giant floods can't stall the server. Baseline ships classic finite-spread flow for water; the queue design is what matters.

**Two more Luanti lessons adopted**: (1) its mapgen is organized as registries of biomes/ores/decorations/schematics — same shape as our data-driven worldgen, good validation; (2) its map persistence sits behind a database interface with SQLite/PostgreSQL/LevelDB backends — so our §9 region files go behind a small `WorldStore` trait, letting big dedicated servers swap in PostgreSQL later without touching world code.

## 7. Crafting & items (data-driven)

- **3×3 shaped + shapeless recipes**, declared in `data/recipes/*.ron` — the familiar datapack mental model:
  ```ron
  ( type: "shaped", pattern: ["P P", " S ", " S "], keys: { "P": "oc:planks", "S": "oc:stick" }, result: ("oc:pickaxe_wood", 1) )
  ```
- Recipe matching = normalize grid → hash lookup (shaped) / multiset lookup (shapeless). Trivial and fast.
- **Items registry** mirrors the block registry; inventory = component (hotbar 9 + main 27, MC layout). Crafting executes **on the server** (client sends "craft request", server validates ingredients — multiplayer-safe by construction).
- Because blocks/items/recipes/biomes are all data files loaded through `oc-assets` with **hot reload in dev**, the base game is built the way mods will be built (the Hytale principle) — modding later is an unlock, not a rewrite.

## 7.5 Texture packs & player skins

- **Resource packs**: the base game's own textures/sounds/models load as just another pack — `oc-assets` resolves assets through an **ordered overlay stack** (user packs override base, the classic resource-pack model). A pack is a folder/zip mirroring the asset tree (`textures/block/stone.png` overrides base). Switching packs rebuilds the block texture array at runtime; hot reload already exists in dev, so live pack switching is nearly free. Pack format is versioned from day 1.
- **Player skins**: industry-standard **64×64 PNG skin files** (the common UV layout, so existing skins and skin editors just work). Locally the player picks a file; on join the client uploads the skin (or its hash), the server caches and distributes it to clients in range via the protocol's asset-sync path — the same mechanism that later syncs server resource packs/mods to joining players (the Hytale model). Skins are client-visual only; the server never trusts them for anything but bytes + size limits.

## 7.6 Modding architecture (NeoForge-like UX, data + WASM underneath)

**User experience goal**: drop a mod file into `./mods/`, launch, it works — and later, one-click install from a Modrinth-style open store. The architecture supports both from day one even though the code-mod runtime ships in phase 5.

**Why not literally NeoForge's mechanism**: NeoForge can rewrite the game because Java runs on a VM with runtime class loading. Rust is native code with no stable ABI — loading native mod `.dll`s is unsafe, platform-fragmented, and a malware vector. The Rust-native answer (and Hytale's choice) is **sandboxed WASM**, which gives mods one binary for all platforms, memory-safety, capability-based permissions, and a versioned API. Mods feel like NeoForge mods; they're built like Hytale plugins.

**Mod package format** — `.ocmod` (a zip):
```
mymod.ocmod
├── mod.ron          # manifest: id, name, version, authors, dependencies (semver), api_version
├── data/            # blocks, items, recipes, biomes, structures — same format as base game
├── assets/          # textures, sounds, models — same overlay rules as resource packs (§7.5)
└── code.wasm        # optional — only for behavior mods (new machines, AI, world hooks)
```
Three tiers of mod, increasing power:
1. **Content mods** = data + assets only, no code. Because the base game is itself data-driven (§7), a huge share of typical content mods (new blocks, items, recipes, biomes, structures, creatures-with-existing-AI) need **zero code** — these work from the *first survival release*, since the loader just merges mod data into the same registries the base game uses.
2. **Behavior mods** = + WASM module: registers ECS systems, block behaviors, network types (§6.5 hooks), event handlers (block place/break, entity tick, player join…) through a versioned host API (`wasmtime` + WIT component model). Server-side first; client-side UI/script sandbox later, per Hytale.
3. **Engine mods** (shaders/renderer): not sandboxed-moddable; instead the renderer exposes data-driven hooks (shader includes, post-process chain) — Hytale's node-shader idea is the eventual shape.

**Architectural commitments made now (cheap) so this works later:**
- **Namespaced string ids everywhere** (`oc:stone`, `mymod:copper_pipe`) — already the registry design (§3).
- **Per-world id mapping persisted in the save** (string→numeric table in the level header): saves survive mods being added/removed/updated, numeric ids are never hardcoded. This is the painful lesson of the genre; it costs one table if done now.
- **Registry freeze point**: mods load → registries freeze → world loads. No mid-session registration; hot reload in dev only.
- **Protocol carries the mod handshake**: at join, server sends its mod list + registry mapping + missing packs via asset sync (§7.5) — clients auto-download content mods Hytale-style; behavior mods that need a client half are declared in the manifest.
- **Dependency resolution**: loader topo-sorts mods by manifest dependencies (semver ranges), refuses conflicting ids with clear errors.
- **Store-readiness**: the manifest (stable id + semver + dependency ranges + api_version) is exactly what a Modrinth-like index needs; "the store" is then just a client that downloads `.ocmod` files into `mods/` — no special game support beyond the manifest. Modrinth itself has an open API; we could publish there before having our own store. The end-state UX to copy is **Luanti's ContentDB**: an in-game browser with one-click install/update/dependency resolution, backed by an open, self-hostable index.
- **Mod API threading**: the Luanti lesson — its single-threaded Lua caps every large server. Our WASM event handlers are declared with what they read/write (like ECS system params), so independent handlers can run in parallel with core systems instead of serializing the tick.
- **License note**: mods interact only through data files and the WASM API boundary (no linking), so mods are independent works — modders can license mods however they want despite the game being AGPL. State this explicitly in the repo to keep the ecosystem unafraid.

## 8. Networking (oc-protocol + oc-net)

- `oc-protocol`: typed messages serialized with `postcard` (compact, no-std-friendly, schema-evolvable enough). Core message families:
  - **ChunkData** (palette sections, zstd-compressed), **BlockUpdate** (single + batch)
  - **PlayerInput** (movement intent, look, actions) — client→server
  - **EntityState** (position/velocity snapshots, interpolated client-side)
  - **Stats/Inventory/Craft** transactions, **Chat/System**
- Transports behind one trait: **in-proc channel** (offline, milestone 1) and **QUIC via `quinn`** (multiplayer phase — streams for bulk chunk data, datagrams for entity snapshots; TLS built-in).
- **Interest management**: each player gets chunks/entities within their view radius only. Client prediction for own movement; server reconciliation. (The standard model for the genre.)

## 9. Persistence

- **Region files behind a `WorldStore` trait** (the Luanti lesson — its SQLite/PostgreSQL/LevelDB backends all sit behind one interface): default backend is one file per 32×32 columns, zstd-compressed palette sections, plus a per-world `level` header (seed, time, settings) and per-player files. Append-friendly, corruption-isolated, proven shape (MCA-like but our own format, versioned from day 1 with a format-version field). Big dedicated servers can get a PostgreSQL backend later without touching world code.
- Entities/stats serialize via the same component serialization used by the protocol.
- Saves are atomic (write-new-then-rename) — survival worlds must not corrupt on crash.

## 10. Key dependencies (lean by design)

| Crate | Why |
|---|---|
| `ash`, `gpu-allocator`, `winit` | Vulkan, VRAM, windowing — the engine core |
| `glam` | SIMD math |
| `bevy_ecs` (standalone) | server simulation runtime |
| `rayon` | gen/light/mesh thread pools |
| `noise` or hand-rolled simplex | worldgen noise (decide by benchmark; hand-rolled likely) |
| `postcard` + `serde` | protocol + save serialization |
| `zstd` | chunk/save compression |
| `quinn` | QUIC transport (multiplayer phase) |
| `ron` | data-driven content files |
| `wasmtime` (phase 5) | sandboxed WASM mod runtime |
| `rapier3d` (phase 6) | rigid-body solver under our voxel collision layer (Sable-validated) |
| `tracing`, `anyhow`/`thiserror` | logs/errors |

No engine frameworks, no Bevy-the-engine, no wgpu. ~12 direct deps for a whole game is genuinely lean.

## 11. Performance budgets (how we keep ourselves honest)

- **Frame budget** (M1, 60 fps = 16.6 ms): render ≤ 8 ms, meshing/uploads amortized off-thread with per-frame caps, sim is server-side (separate thread) so it can't hitch rendering.
- **Built-in HUD**: frame time graph, draw calls, loaded columns, RAM/VRAM counters from day one — perf targets are tested continuously, not at the end.
- **Stress tests as CI-able binaries**: "teleport 10k blocks and measure chunk catch-up", "worldgen 1k columns/sec benchmark", determinism tests (same seed ⇒ byte-identical chunks), palette + save roundtrip property tests.

## 12. Roadmap (phases, each independently shippable)

1. **Engine bring-up**: window + Vulkan clear → textured test chunk → camera fly-around. (MoltenVK pitfalls solved here.)
2. **World prototype**: chunk pipeline, terrain gen with biomes/rivers/trees/caves, walking/collision, block place/break, save/load, day cycle. *(First "it's a game" build.)*
3. **Survival core**: hotbar/inventory, 3×3 crafting, health/hunger/stamina/oxygen, simple creatures (ocean fish + land passive), villages, local skin + texture pack selection (overlay stack ships here since base assets are already a pack).
4. **Multiplayer**: QUIC transport, prediction/reconciliation polish, dedicated server binary, interest management hardening, skin/pack distribution via asset sync.
5. **Depth & modding**: LOD far terrain, GPU occlusion culling, combat/mobs, more structures, audio; mod loader ships content-mod support (data+assets from `./mods/`) and the WASM behavior-mod API (§7.6). (Content-mod loading may land earlier — it's nearly free once `oc-assets` exists.)
6. **Physics & machines** (Create-inspired): rigid-body physics grids → assembler + airships/balloons (§6.5), then block networks (rotational power, electric). Ordered after multiplayer because moving grids must be designed against the replication model.

## 13. Open decision points (to discuss)

1. **ECS**: `bevy_ecs` standalone (my pick — most capable, Flecs-like) vs `hecs` (smaller, more "own everything").
2. **Noise**: `noise` crate vs hand-rolled SIMD simplex (perf-critical; can defer to a benchmark in phase 2).
3. **Default X-Z playable border**: address space is 0..4.29B; pick the default fence (0..64M, or larger).
4. **Name/branding**: "opencreate" assumed from the directory name (fits the Create inspiration nicely).

**Decided so far**: signed `i32` coordinates centered on 0,0,0 with sea level at Y=0; per-dimension Y ranges with **default −512..+5120** backed by sparse column storage (tall sky is free until built in); 30 TPS; multi-dimension worlds in the data model from day one; client–server always; ash+MoltenVK; WASM modding with `./mods/` drop-in; **RON** for content files; **`rapier3d` + Sable-style voxel octree layer** for phase-6 physics (validated by reading Sable's source — it ships exactly this: stock-ish Rapier fork + custom octree mid-phase).

## Verification (for when implementation starts)

- Each roadmap step ends in something runnable; phase 1 exit = textured chunk at 60 fps on the M1 dev machine through MoltenVK.
- Headless tests: worldgen determinism, palette roundtrip, save/load roundtrip, recipe matching, lighting flood-fill correctness.
- Perf gates: chunk-catchup stress test + frame-time HUD checked against budgets at every phase.
