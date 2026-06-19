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
  airless body = uniformly cold. The overworld curve is a long gentle descent to
  the 50 °C onset at −512, then a steep ramp into the molten layer (see
  [deep-world](world-building/deep-world.md)).
- **G2** — blackbody glow past the Draper point (525 °C), **baked per-vertex** at
  mesh time (`chunk_gbuffer.wgsl`) — deep rock glows dull-red → orange before any
  lava — plus per-world **`ambient_floor`** (nothing renders pure black). (The
  earlier geothermal *cast light* was removed: its 4-bit quantization banded the
  glow; the smooth per-vertex emissive replaced it.)
- **G3.1** — tier-2 **source heat**: a bounded, conductivity-attenuated flood-fill
  from lava (`oc-world/src/heat.rs`, modelled on the light BFS), baked into the
  vertex glow so rock near lava glows hotter and insulators shield. A pure
  function of the blocks (deterministic, no sync); the client reuses the light
  field's block snapshot to avoid a second column scan.
- **G6** — player **heat hazard** in `stats.rs` (`thermal_damage_rate`) — two
  physical paths summed (**convection** through the medium + **conduction** through
  the blocks you touch), scaled by nature's conductivity ratios. The band is
  **nature's** (≈50 °C hot / −60 °C cold); creative exempt. Verified in-game: a
  deep survival spawn dies from heat in ~0.6 s, the surface is safe. (Also fixed:
  a teleport/respawn no longer counts as a fall.)

**Now live (the deep world is built):** with the deep overworld — lava lake at
−656, bedrock floor at −752 — the glow, source heat, and hazard are **active**:
the descent ramps cool caves → glowing molten layer → lava, and the deep is
lethal. A shallow or airless world keeps them correctly dormant.

**Pending:**
- **G3.2** — tier-3 *stored* per-block temperature (a placed block heats up and
  glows over seconds), its sparse per-section save layer, server-authoritative
  sync to clients, and Newton cooling (frozen offline, no catch-up).
- **G5** — *phase transitions* (lava↔obsidian/basalt, ice↔water↔steam) + the
  latent-heat plateau + water cooling. Needs `oc:obsidian`/`oc:basalt`/`oc:ice`
  content (lava/water already exist as placeable items).

## The key coupling: the deep world is built

The deep-world build ([world-building/deep-world.md](world-building/deep-world.md))
landed the lava sea + bedrock floor the heat features need, so **G3.1** (source
heat) and **G6** (hazard) are now live. **G5** still needs its transition
**content** (`oc:obsidian`/`oc:basalt`/`oc:ice`); and **Stage H's volcanoes** are
lava heat sources too, so H wants the same content path — the remaining work
still converges there.

## Remaining work, in dependency order

1. **G3.2** — tier-3 *stored* per-block temperature: server-authoritative state,
   a sparse per-section save layer (lossless format bump), sync to clients, and
   Newton cooling (frozen offline). Delivers "a placed block heats up and glows."
2. **G5** — *phase transitions* + content: `oc:obsidian`/`oc:basalt`/`oc:ice`,
   lava+water→obsidian/basalt, ice↔water↔steam, the latent-heat plateau, and
   water as a finite coolant (the temporary survivable pocket).
3. **Stage H** — coarse 16³ climate grid + volcanoes (lava sources) +
   global `world_age`.

**Reserved (later):** a hellish layer above the lava sea (distinct content); a
colour-coded temperature-status HUD (cold/normal/warm/hot/extreme; later a
real-temp readout of the player or looked-at object via gear); insulation gear.

## Risk note

The deep-world deepening (the keystone risk — worldgen, column save size, light
range, performance) is **done and measured fine**. The riskiest remaining change
is now **G3.2's stored-heat persistence + sync**: a save-format bump (must stay
lossless) and a new server→client channel for live per-block temperature. It
wants focused, careful work, not a tail-end rush.
