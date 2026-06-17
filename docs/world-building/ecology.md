# Ecology & Climate: Niche-Based Biomes

**Design, not yet built.** This is the reserved data model for flora, fauna, and
climate — Phase 5+ depth. The principle is settled and validated against
[TerraFirmaCraft](https://terrafirmacraft.github.io/) (TFC), which hands us
real-tuned starting numbers; the implementation comes after the
[matter model](matter-model.md) and worldgen foundations land. Items tagged
**(gameplay — reserved)** are deliberately out of scope for the world-building
pass and exist only so the registry doesn't preclude them.

## Biome = a derived label

We don't author biomes as primitives. The world is described by continuous
**environmental axes** — temperature, humidity/hydration, light, CO₂, air
pressure, soil — and a single **suitability model** decides what grows and what
spawns there. A "biome" is just the label you read off the dominant result. One
model governs both plants and animals, so vegetation and wildlife always agree
with the climate instead of being painted on independently.

TFC is empirical proof this works: every crop, fruit tree, bush, and animal in
it is defined by **temperature + rainfall ranges** (trapezoid niches), plants
are **seasonal**, ores sit in **depth + rock strata** (see [geology](geology.md)),
and animals spawn by **climate niche**. The locked design knobs are: **trapezoid**
tolerance curves, **geometric-mean** suitability, **environment-modifying
succession**, and **fauna niche-driven** spawning.

## The climate field

All climate is a **deterministic pure function** of position + calendar +
[`world_age`](time.md), plus the coarse dynamic offset from the
[dynamic environment](dynamic-environment.md). It is cheap to evaluate anywhere
and frozen offline. Effective **temperature is a SUM of contributions** — TFC's
exact model, whose constants we adopt as seeds:

| Contribution | Effect |
|---|---|
| Latitude | Triangle wave, ±18 °C swing, "poles" ~20 km apart |
| Seasonal | Per-month modifier, sinusoid over the year (scaled by latitude) |
| Daily | ±4 °C noise, hottest at noon |
| Elevation | −0.16 °C/block above sea level, capped ~−18 °C |
| Depth | Below ground lerps toward ~15 °C, then the geothermal gradient takes over toward `core_temp` (see [temperature](temperature.md)) |
| Dynamic offset | The coarse heat/moisture field from volcanoes, water, sources |

**Longitude/coast → rainfall ~0–500 mm**, which becomes the **hydration** axis
(groundwater = base + rainfall, on a 0–100 scale). Temperature and hydration
together govern growth. **Köppen thresholds** (≈ −17 °C … >21 °C; rainfall
<75 / <150 mm for desert/steppe) give clean biome-zone cutoffs for free.

This layers on top of the existing multi-noise worldgen
([world-generation.md](../server/world-generation.md)) rather than replacing it —
the static climate noise stays; the niche model and the dynamic offset are added
on top.

## Calendar & seasons

TFC's numbers: a **year = 12 months × ~8 days ≈ 96 days**, a **day = 24 h**, and
**4 seasons** (3 months each). Seasonal temperature swing is just the per-month
modifier **scaled by latitude** — none at the equator, strong at the poles; the
southern hemisphere inverts. Plants flower, fruit, or go dormant by season.

Seasons are **per-dimension** via `EnvDef`: axial tilt sets the seasonal
strength, and year/day length are configurable — a no-tilt planet has no seasons
at all. The calendar advances with [`world_age`](time.md) and is frozen offline.

## `PlantDef` niche

Each plant declares a **trapezoid `{min, opt_lo, opt_hi, max}` per axis** —
flat-topped tolerance curves that are 1.0 in the optimal band and ramp to 0 at
the edges. Axes:

- **temperature**, **hydration**
- **light** — per-RGB-channel, so an alien blue-light plant is just data
- **CO₂**
- **air pressure**
- **soil** — substrate block-tag + `fertility`

Plus a **season window** for maturity/fruiting, a **growth form** (in-place
staged / vertical-spread like kelp / multiblock tree via the existing tree
generator), and an **env-modification** field (see succession below).

**Photosynthesis rate = growth rate**, and it scales with **both light and CO₂**
— each a trapezoid that **saturates to a plateau** (real botany: the rate climbs
with light or CO₂ until something else limits it). So **CO₂ enrichment boosts
growth**, exactly as greenhouse CO₂ injection does in the real world (link
[atmosphere](atmosphere.md)). Overall growth is the **geometric mean** of the
per-axis suitabilities — one weak axis throttles the whole plant, so a crop in
the cold *or* the dark *or* starved of CO₂ stalls.

The data format is TFC's `ClimateRange`: `{ min/max_temperature,
min/max_hydration, *_wiggle_range }` — the min/max plus the wiggle range **is**
the trapezoid. Seed numbers straight from TFC:

| Plant | Temperature | Notes |
|---|---|---|
| Wheat | −7 … 22 °C | hydration 15–85 |
| Rice | 8 … 31 °C | hydration ≥35, waterlogged |
| Orange | 8 … 41 °C | warm/tropical |
| Cranberry | — | grows underwater |

Light < 12 halts growth regardless of the rest (the same gate vanilla crops use).

### Environment-modifying succession

Plants **change their surroundings** — shade, humidity, soil — which shifts the
niche for the next species. A pioneer that adds soil fertility or casts shade
lets a later, shade-loving plant move in. This makes succession emergent from
the same niche model instead of being scripted.

## `CreatureDef` niche

Animals use the **same temperature + rainfall trapezoids** (polar bear ≤10 °C;
lion ≥16 °C; crocodile >15 °C…) → **niche-driven spawning**. The TFC spawn-data
format is `{ min/max_temperature, min/max_groundwater, min/max_forest, chance,
months }`.

**Husbandry** (familiarity/taming, gender, infant/adult/old stages, milk/wool/egg
schedules) is **(gameplay — reserved)**, but the TFC numbers are recorded here so
the research survives. Familiarity caps at **0.35** for most animals;
`READY_TO_MATE = 0.30`; products need familiarity **> 0.15**; feeding adds **+0.06**
and it decays **0.02/day** below the no-decay threshold. Offspring inherit ~50 % of
the parents' average familiarity (90 % if both ≥ 0.9). Aging / breeding / products
(days; cooldowns in hours):

| Species | Adult | Gestation/hatch | Offspring | Product |
|---|---|---|---|---|
| Cow | 192 d | 58 d | 2 | milk 24 h |
| Sheep | 56 d | 32 d | 2 | wool 168 h |
| Goat | 96 d | 32 d | 2 | milk 72 h |
| Yak | 180 d | 64 d | 1 | milk 24 h |
| Alpaca | 98 d | 36 d | 2 | milk/wool 120 h |
| Pig | 80 d | 19 d | 10 | — |
| Chicken | 24 d | egg hatch 8 d | — | egg 30 h |
| Duck / Quail | 32 / 22 d | hatch 8 d | — | egg 32 / 28 h |

Milk is 1 bucket (1000 mB); wool is 1 (2 above 0.99 familiarity).

## Super-plant (heat-eater)

A `PlantDef` with a **hot optimum** plus **endothermic growth that drains the
coarse-climate heat** — a literal heat sink — and an edible yield. It doubles as
a **terraforming tool** (cool a volcanic region into livability) and stays
conservation-consistent: the heat it removes is real heat taken out of the
[dynamic environment](dynamic-environment.md)'s coarse field, not free cooling.

## Soil-nutrient cycle & nutrition *(gameplay — reserved)*

Crops consume a soil nutrient (TFC: **N/P/K**, needing rotation/fertilizer), so
the `fertility` axis becomes a **matter-conservation loop** — dead plants and
compost return fertility, which feeds growth — the matter analog of the
[energy-conservation](temperature.md) pillar. Food carries **nutrition groups**
(fruit/veg/protein/grain/dairy) that raise or lower max health, and thirst ties
to the [fluid model](fluids.md) (saltwater doesn't hydrate). All reserved depth.

## Biome-alignment axis *(reserved)*

An optional extra environment axis — **alignment** — tagging regions as
benign↔savage (Dwarf Fortress) or good↔evil (Terraria), independent of the
climate axes. It would bias spawns and flora (savage zones get tougher fauna;
"evil" zones get hostile variants) and spread via the same coarse-chunk CA as
[climate](dynamic-environment.md). Purely reserved — the niche model doesn't need
it, but the environment vector should leave room for it.

## Feasibility

Every climate field is a deterministic pure function (cheap); a niche is a
handful of trapezoids; worldgen places climax flora + niche fauna; growth and
succession run on sparse random ticks; the biome label is derived. Nothing here
needs a heavy simulation — the cost is bounded by the active area. The point of
writing it down now is to **reserve the data model** (`PlantDef` / `CreatureDef`
niches, soil tags, season/calendar) so the registry and worldgen never preclude
it.

## See also

- [atmosphere.md](atmosphere.md) — the CO₂/O₂ field plants breathe and modify
- [time.md](time.md) — how seasons and `world_age` advance and freeze
- [dynamic-environment.md](dynamic-environment.md) — dynamic biomes that shift with heat and moisture
- [geology.md](geology.md) — rock strata and the soil substrate
- [temperature.md](temperature.md) — the geothermal contribution and the heat-sink super-plant
