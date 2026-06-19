# Phase 0 Foundation — Implementation Status

**Branch:** `feat/phase0-foundation` (off `main`). **In progress, not merged.**
Status snapshot — what's built vs. pending in the Phase-0 "foundations +
heat/environment" effort. The design lives in
[world-building/](world-building/README.md) and the approved plan; this page
records where the *implementation* stands.

The effort is eight gated stages (A–H); each must `cargo build` +
`cargo test --workspace` green and be in-game verified before the next.

## Stage status

| Stage | What | Status |
|---|---|---|
| **A** | Data-driven block registry + save v1→v2 (per-world block palette) | ✅ done |
| **B** | `oc-core::physical` constants/models (Beer–Lambert, Fresnel, blackbody…) | ✅ done |
| **C** | Per-frame Scene/Environment UBO | ✅ done |
| **D** | Texture pipeline (PNG overlay + mipmaps) | ✅ done |
| **E** | Deferred G-buffer + PBR lighting pass + RGB light bake (+ shadows wired) | ✅ done |
| **F** | `FluidDef`/`GasDef`/`EnvDef` registries — worlds as data, per-world dimensions | ✅ done |
| **G** | Three-tier heat field + blackbody glow + phase transitions + heat hazard | ⏳ **partial** |
| **H** | Coarse 16³ heat+moisture grid + volcanoes + `world_age` | ⛔ not started |

A–F are committed, green, and in-game verified. Highlights: the opaque world
renders **deferred** (G-buffer → fullscreen PBR lighting → forward overlay);
**whole worlds are data** (gravity, sky, atmosphere, fluids per `EnvDef`), with
a second **moon** dimension proving per-world selection at runtime.

## Stage G — what's done, what's blocked

**Done & committed:**
- **G1** — tier-1 static base temperature `T_base(pos)` from `EnvDef.thermal`
  (`oc-world/src/temperature.rs`); deep = hot toward the core, altitude cools,
  airless body = uniformly cold.
- **G2** — blackbody glow (deferred lighting pass, past the Draper point) +
  geothermal **cast light** (hot cells seed the block-light flood-fill) +
  per-world **`ambient_floor`** (nothing renders pure black; overworld 0.045,
  moon 0.02).
- **G6** — player **heat hazard** in `stats.rs` (damage outside a survivable
  band; creative exempt). The band is **nature's** (≈50 °C hot / −60 °C cold) —
  physical thresholds are not gameplay-tuned.
- **Gradient** retuned to a near-realistic **0.18 °C/block** (50 °C survivable
  band reached ~200 blocks down), replacing a fake 480×-steep placeholder.

**Consequence:** with the realistic gradient, the current **shallow (−64)**
world's deep is only ~26 °C — so the glow and heat hazard are **correctly
dormant** here. They come alive from **real lava** in the deep world.

**Pending (blocked on content):**
- **G3** — tier-2 *source* heat (bounded flood-fill from lava/fire). No heat
  sources exist yet (only water as a fluid).
- **G4** — tier-3 *stored* per-block temperature + its sparse per-section save
  layer + Newton cooling.
- **G5** — *phase transitions* (lava↔obsidian/basalt, ice↔water↔steam). The
  `phase_transition` refs are reserved but `oc:lava`/`oc:ice`/`oc:obsidian`/
  `oc:basalt` don't exist as content.

## The key coupling: finishing G ≈ building the deep world

G3 and G5 can't be built without heat-source + transition **content** (lava,
ice, obsidian). That content is exactly the **deep-world build** described in
[world-building/deep-world.md](world-building/deep-world.md): a deeper world
with a lava sea and a bedrock floor, where heat and glow come from real lava.
And **Stage H's volcanoes are lava heat sources too** — so H also wants that
content. So the remaining work converges on one path.

## Remaining work, in dependency order

1. **Deep-world content + worldgen** ([deep-world.md](world-building/deep-world.md)):
   `oc:bedrock` (unbreakable) + `oc:lava` (hot ~1200 °C glowing fluid, a tier-2
   heat source) as data → deepen the world (`BOTTOM_SECTION_Y`, −64 → ~−360) →
   deep geology zones (rock → hot rock → lava+stone → lava sea → bedrock). This
   **delivers G3 + G5** and activates the dormant glow/hazard from lava.
2. **G4** — stored per-block temperature + save layer + Newton cooling.
3. **Stage H** — coarse 16³ climate grid + volcanoes (lava sources) +
   global `world_age`.

**Reserved (later):** a hellish layer above the lava sea; a colour-coded
temperature-status HUD (cold/normal/warm/hot/extreme; later a real-temp readout
of the player or looked-at object via gear).

## Risk note

The riskiest remaining change is **deepening the world's vertical range** — it
touches worldgen, column save size, light range, and performance, the way the
deferred-renderer rewrite (Stage E) touched the whole opaque path. It wants
focused, careful work, not a tail-end rush.
