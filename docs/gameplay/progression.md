# Progression & Recipes *(reserved — future gameplay)*

**Not built, and out of scope for the current world-building work** — recorded
here so the TerraFirmaCraft (TFC) research survives for the eventual survival /
machines phases. Today crafting is a single 3×3 grid
([inventory-and-crafting.md](inventory-and-crafting.md)); the takeaway below is
that a real progression system needs **pluggable recipe *types*** and
**tier metadata on metals/ores/tools**, not just one grid.

## The architectural takeaway

TFC's entire tech tree is **custom data-driven recipe types** plus standard
shaped/shapeless recipes for assembly. So our recipe system should support
**multiple recipe types and multi-step processes**, each its own data schema,
resolved by a registry — not a hardcoded grid. Metals, ores, and tools carry
**tier metadata** that gates what the next step can make.

## Recipe types (TFC)

| Type | Shape |
|---|---|
| Knapping | a pattern grid over rock / clay / fire-clay / leather / goat-horn — the earliest tool gate (stone tools → ceramic molds) |
| Heating | `{ ingredient → result_item/result_fluid at temperature }` — melt/transform past a threshold |
| Casting | `{ mold + fluid → result, break_chance }` — pour molten metal into fired molds |
| Alloy | mix molten metals by component min/max % ranges |
| Anvil | `{ ingredient, min tier, 1–3 forge rules, step sequence }` — the forging minigame |
| Welding | join two pieces, gated by `min_weld_tier` |
| Bloomery | `{ fluid input + charcoal catalyst → bloom, duration }` — early iron |
| Quern / Loom / Scraping / Barrel | grinding, weaving, hide-scraping, sealed/instant fluid processing |

**Anvil forge steps** move a pointer toward a target: HIT_LIGHT −3, HIT_MEDIUM
−6, HIT_HARD −9, DRAW −15, PUNCH +2, BEND +7, UPSET +13, SHRINK +16. The
**heat-color cue** (when metal is workable) is the same blackbody ladder used for
[temperature glow](../world-building/temperature.md): FAINT_RED ~480 °C →
BRILLIANT_WHITE ~1600 °C.

## Metals — melt points & tool tiers (TFC)

Melt temperatures: copper 1080, bronze 950, wrought iron 1535, steel 1540, black
steel 1485 °C (each metal also carries a specific-heat-capacity).

| Metal | Tier | Durability | Speed | Damage |
|---|---|---|---|---|
| Copper | 1 | 600 | 5.25 | 3.25 |
| Bronze (+ bismuth/black) | 2 | 1200–1460 | 7.3 | 4.0 |
| Wrought Iron | 3 | 2200 | 8.0 | 4.75 |
| Steel | 4 | 3300 | 9.5 | 5.75 |
| Black Steel | 5 | 4200 | 11.0 | 7.0 |
| Blue / Red Steel | 6 | 6500 | 12.0 | 9.0 |

## The loop

Rock-knapping → pottery (clay knapping → fired molds) → **copper** (cast/heat) →
**bronze** (alloy) → **iron** (bloomery) → **steel** → black/blue/red steel —
each tier required to forge the next-tier anvil, which gates the next tier's
recipes. Ore placement that feeds this lives in
[world-building/geology.md](../world-building/geology.md) (the placement is
world-building; the smelting/tiers above are the gameplay layered on top).

## See also

- [inventory-and-crafting.md](inventory-and-crafting.md) — the crafting that exists today.
- [../world-building/geology.md](../world-building/geology.md) — ore strata & veins.
- [../world-building/temperature.md](../world-building/temperature.md) — the heat/blackbody model the forge cues reuse.
