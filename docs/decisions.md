# Decisions

The chosen paths, why they were chosen, and what they replaced. Decisions
marked **locked** shouldn't be re-litigated without strong cause.

## Locked

| Decision | Choice | Why |
|---|---|---|
| Engine | Own engine on raw Vulkan (`ash`), MoltenVK on macOS | Full control, no framework churn; Vulkan 1.2 ∩ MoltenVK features only |
| Architecture | Client–server always, even offline (§1) | Retrofitting multiplayer kills voxel projects; offline = in-proc channel, zero latency |
| Tick rate | 30 TPS fixed, server thread | Smoother than MC's 20, still cheap; sim can never hitch rendering |
| Coordinates | Signed `i32`, centered 0,0,0, sea level Y=0 | Altitude reads like elevation; ±2.1B address space |
| Y range | Per-dimension, default −512..+5120 | Sparse columns make tall sky free until built in |
| Sections | 16³, sparse columns | Meshing/lighting/network granularity; ecosystem intuition transfers |
| Entity positions | `f64` absolute (+ frame id later) | f64 resolves to nanometers at 64M blocks; frames enable airships |
| Rendering positions | Camera-relative `f32` (floating origin) | Precision never degrades far from spawn; baked in from the first triangle |
| ECS | `bevy_ecs` standalone (0.16) | Archetypal like Flecs (Hytale's choice), mature; adopted with survival stats |
| Content format | RON files in `data/`, embedded as defaults | Readable, serde-native; base game built the way mods will be |
| Identity | Namespaced string ids (`oc:stone`) stable; numeric ids per-load | The painful Minecraft lesson; saves persist strings, wire uses u16 |
| Serialization (saves) | Hand-rolled versioned binary + zstd | No serde overhead in the hot path; format-version field from day 1 |
| Noise | Hand-rolled value-noise fBm | Keeps the §13 noise-crate decision open; deterministic, no deps |
| Modding runtime | WASM via wasmtime (phase 5) | Rust has no stable ABI; sandbox + one binary per mod |
| Physics (phase 6) | `rapier3d` + custom voxel octree mid-phase | Validated by reading Sable's source — that's exactly what ships |
| Persistence boundary | `WorldStore` trait | Luanti lesson: backends swap (folder now, region/PostgreSQL later) |

## Decided along the way (as-built)

- **Lighting is a pure function**, computed per mesh job over the 3×3-column
  snapshot rather than stored: light range 15 < the 16-block margin, so
  center-column values are exact. Persistent light storage waits for the
  §6.6 active-area simulation (mob spawning needs queryable light).
- **Game modes are registry data**, not an enum — five engine capability
  flags composed by `data/gamemodes.ron`; mods add modes as data.
- **Full entity snapshots** (15 Hz) instead of deltas — robust and cheap at
  current entity counts; deltas when counts grow.
- **Crafting by recipe index** over the wire (shared registry), with the
  grid UI deferred to the inventory screen.
- **Only dirty columns are saved**; pristine terrain regenerates from the
  seed. Saves stay tiny; worldgen changes only disturb unedited terrain.
- **Rejection-by-echo**: the server answers every block edit with the
  authoritative `BlockChanged`; an echo matching the prediction is a no-op,
  a differing one rolls the client back. No separate ack/nack machinery.

## Still open (§13)

- Noise implementation benchmark (`noise` crate vs hand-rolled SIMD) —
  revisit when worldgen cost matters
- Default X-Z playable border (address space is ±2.1B; pick the fence)
- Name/branding ("opencreate" assumed from the directory)
- Region-file layout details (32×32 columns per file planned)
