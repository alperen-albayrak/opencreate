# Geology: Rock Strata & Ores

**Status: design, not built.** Today's worldgen
([../server/world-generation.md](../server/world-generation.md)) produces
terrain, biomes, caves, trees, and villages as pure functions of `(seed,
position)`, but the subsurface is a single undifferentiated stone. This page
describes the planned **layered rock + ore** model, validated against
TerraFirmaCraft (TFC) and using its concrete data shapes as the starting point.

**Scope.** Ore *placement* is in-scope world-building. The *use* of ores —
smelting, alloying, tool/armor tiers — is gameplay progression and is
**reserved** (the registry and worldgen must merely not preclude it).

## Rock strata

The subsurface is built from **stacked rock layers** descending from the
surface. Each layer's thickness is multi-octave noise scaled to roughly
**43–63 blocks**, so boundaries undulate rather than sitting at fixed Y. To find
the rock at a position, subtract layer thicknesses from the depth below the
adjusted surface until the position falls inside a layer.

Vertical ordering, top → bottom:

| Band | Rock type | Examples |
|---|---|---|
| Sedimentary | deposited, near-surface | shale, limestone, dolomite, chalk, chert |
| Metamorphic | heat/pressure-altered | slate, phyllite, schist, gneiss, marble, quartzite |
| Igneous intrusive | slow-cooled, deep | granite, diorite, gabbro |
| Igneous extrusive | erupted, caps the surface | rhyolite, andesite, dacite, basalt |

A **region type** picks the top stratum, so geology correlates with surface
form:

- **Ocean** → igneous extrusive floor.
- **Land** → extrusive or sedimentary top.
- **Volcanic** → extrusive (often doubled) — the rock home of
  [volcanoes and volcanic climate regions](dynamic-environment.md).
- **Uplift** → sedimentary or an uplift mix that **exposes deeper rock** at the
  surface (the analogue of mountain-core outcrops).

Dikes and kimberlite pipes (vertical intrusions that bring deep material up) are
**reserved** as later feature content, not part of the first pass.

## Ore veins (data-driven)

Ores are **content, not code**: each vein is a config entry (RON, mirroring the
existing `data/` registries), so adding an ore or retuning a deposit is a data
edit. The shape follows TFC's vein format:

```ron
(
    type: cluster_vein,          // or disc_vein (adds a height), kaolin_disc_vein
    rarity: 24,                  // 1-in-N chunks carry this vein
    density: 0.25,               // per-block chance to replace inside the vein
    min_y: 40, max_y: 130,
    size: 20,
    blocks: [
        ( replace: ["rock/extrusive"],
          with: [
            ( weight: 70, block: "ore/poor_copper" ),
            ( weight: 25, block: "ore/normal_copper" ),
            ( weight:  5, block: "ore/rich_copper" ),
          ] ),
    ],
    indicator: ( rarity: 14, depth: 35, blocks: [( block: "ore/small_copper" )] ),
)
```

Key fields: **`rarity`** (1-in-N chunks), **`density`** (per-block replace
chance), the **Y band**, and a **weighted grade table** — every ore comes in
**poor / normal / rich** grades chosen by weight, plus a surface **indicator**
scatter ("small" ores) that hints at the vein below.

Representative placement (ores live in specific rock types and depth bands;
deeper and rarer trends richer):

| Ore | Rock types | Y band | Rarity |
|---|---|---|---|
| Native copper | extrusive (rhyolite/basalt/andesite) | 40–130 | 1/24 |
| Cassiterite (tin) | intrusive (granite/diorite/gabbro) | 80–180 | 1/5 |
| Hematite (iron) | extrusive | 10–90 | 1/45 |
| Malachite (copper) | marble/limestone/chalk/dolomite | −30–70 | 1/45 |
| Bituminous coal | sedimentary (disc vein) | −35..−12 | 1/210 |

Because ores are gated to rock types and depths, the [strata](#rock-strata)
layout *is* the prospecting game: you read the surface rock and dig where the
right band should be.

## Conservation of matter

Ore deposits are **finite and deplete** — there are no free resources, the
matter analogue of the [energy-conservation rule](temperature.md). Recycling,
decay, and the soil-nutrient cycle (see [ecology.md](ecology.md)) return matter
to circulation rather than spawning it. A mined-out vein stays mined out.

## What this requires

The data-driven [block registry](matter-model.md) and worldgen must not
preclude **rock-type strata** or **per-rock ore blocks** (e.g.
`ore/normal_copper/rhyolite` as distinct from the granite variant). The existing
worldgen already lists *ores by depth* and *badlands-style strata* in its
[Planned section](../server/world-generation.md#planned-5); this page is the
fuller design those notes point at. See [ARCHITECTURE.md](../../ARCHITECTURE.md)
§5 for the deterministic-worldgen and two-phase-placement rules this builds on.
