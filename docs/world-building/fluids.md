# Fluids

**Partly shipped, mostly designed.** Underwater per-channel Beer–Lambert
absorption and the eye-adjusting underwater camera are **already in the game**
(`oc-client/src/sky.rs` `underwater()`, the forward water pass, and the "Water
v2" work in [../roadmap.md](../roadmap.md)). The **`FluidDef` registry** that
generalizes all of it to data-driven fluid *types* is the planned part.

## `FluidDef` — water is just the first entry

`data/fluids.ron` makes fluids **content, not code**: oil, olive oil, milk,
lava, mud, and blood are data. Per fluid:

| Field | Drives |
|---|---|
| `color`, `opacity` | look, transparency |
| `extinction` (per-channel RGB) | Beer–Lambert absorption (merged with `light_filter` — see [matter model](matter-model.md)) |
| `emissive` | lava glows (blackbody — see [temperature](temperature.md)) |
| `density` | buoyancy / float |
| `viscosity` | flow speed |
| `fog_color`, `fog_distance` | submerged fog |
| `ior` | Fresnel + refraction |
| `breathability` | ~0 → drowning |
| `oxygen_content` | gear/gills extract O₂ (see [atmosphere](atmosphere.md)) |
| surface style | foam, sheen, wave model |

The Step-2 absorption, the water pass, fog, and buoyancy all **read `FluidDef`** —
no "water" hardcoding anywhere.

## Physical constants

Pure-water absorption `a(λ)` is strongly wavelength-dependent — red dies first,
blue lingers:

| Channel | a(λ) | 1/e depth |
|---|---|---|
| Red | ≈ 0.45 | ≈ 2 m |
| Green | ≈ 0.05 | ≈ 20 m |
| Blue | ≈ 0.014 | ≈ 70 m |

→ **R:G:B ≈ 30:3:1** (blue survives, red dies). Water **IOR 1.333** →
**Fresnel F0 = 0.02**; the **Snell window critical angle is 48.6°** (a ≈97° cone
looking up — outside it the surface is a mirror, the "up transparent / down
reflective" look). Other fluids vary: lava emits (blackbody), milk is opaque,
oil has a low-roughness sheen.

## What's shipped today

In `oc-client/src/sky.rs` and the forward water pass:

- **Per-channel Beer–Lambert** that darkens with depth, red faster than blue, so
  deep dives go dark and blue instead of staying a bright sheet.
- An **eye-adjusting underwater fog distance** (dense on the dive, clearing as
  the eyes adjust — between Java's 90 and Bedrock's 60 blocks).
- A blue **fog dome**, clouds off, and Voronoi-web **caustics**.

The generalization — moving these constants into `data/fluids.ron` so any fluid
renders correctly — is the planned `FluidDef` work.

## Runtime representation

A fluid is a *substance*; how it is stored depends on context (see the
[matter model](matter-model.md) contexts table):

- **Voxel cell** — an ocean is water voxels; the sim is the reserved §6.6
  liquid-flow queue. Reserved here: **finite-spread flow + waterlogging**, with
  the Dwarf-Fortress-style **7-level fluid depth + pressure** as the flow model.
- **Contained quantity** — tank water, a fuel reservoir: an *amount* + id, not
  voxelized.

## Surfaces are optical too

The optical-surface trait applies to **liquid surfaces**, not just solids:
molten metal, mercury, and an oil sheen carry `roughness`/`metalness`/`normal`
and light like any PBR surface (see [rendering](rendering.md) Step 7).

## Phase transitions

Fluids tie into the other registries through temperature: lava → obsidian/stone,
ice → water → steam. The mechanism (a `phase_transition` registry-ref, event-
driven, with the quench-rate nuance) lives in the
[matter model](matter-model.md) and [temperature](temperature.md) pages.

## See also

- [matter-model.md](matter-model.md) — `FluidDef`, `extinction`, runtime contexts.
- [temperature.md](temperature.md) — lava as a finite heat battery; blackbody glow.
- [../../ARCHITECTURE.md](../../ARCHITECTURE.md) — the §6.6 active-area liquid-flow seam.
