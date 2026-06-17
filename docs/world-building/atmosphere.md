# Atmosphere & Gas Composition

**Design, not yet built.** Air is a **mixture, not one gas**. This page covers
the gas registry, the multi-component composition field, partial-pressure
breathing, and terraforming. It draws on Stationeers (true ideal-gas mixtures),
Oxygen Not Included (cheap density-sorted spatial composition), and real
Earth/Mars atmospheric data.

## `GasDef` — a component, not "air"

Each `GasDef` is **one component** (O₂, CO₂, N₂, H₂O vapour, helium, steam,
methane, toxic, …):

| Field | Meaning |
|---|---|
| `density` | molar mass → buoyancy + vertical layering (CO₂ sinks, H₂ rises) |
| `breathable` | does it sustain the player? (O₂ yes; CO₂/toxic no) |
| `color` / `fog_color` | look when it dominates a volume |
| `light_emission` | glowing gases |
| `flammable` / `toxic` | optional flags |
| thermal trait | shared with the other registries (see [matter model](matter-model.md)) |

**Air is a mixture of these components**, tracked as a composition field.

## The composition field

A **coarse multi-component field** per chunk/section: **moles per component +
shared temperature**. It is physically grounded on the **ideal gas law**
(Stationeers' model):

```
P  = n_total · R · T / V
Pᵢ = P · (nᵢ / n_total)     # partial pressure of component i
```

A small fixed species set (O₂ · N₂ · CO₂ · H₂O-vapour · fuel · toxic) keeps it
SoA/SIMD-friendly. It **reuses the heatmap / cellular-automaton machinery** (see
[dynamic environment](dynamic-environment.md)): diffuses chunk-to-chunk with flux
∝ partial-pressure gradient, is frozen offline, and relaxes by closed form toward
the dimension default when unloaded (see [time](time.md)). Vertical layering
comes from component `density` (ONI's density-sort — CO₂ pools low, H₂ collects
high).

### Two-tier cost (the perf lever)

- **Fully-open outdoor air = one well-mixed reservoir** — the global-atmosphere
  pure function (free; see the [matter model](matter-model.md) contexts).
- **The detailed per-chunk mole field runs only inside / near sealed or built
  structures**, where composition actually matters.
- **Active-set / dirty-flag:** settled chunks stop computing and re-wake on a
  neighbour change; diffusion sub-steps every N ticks, not per frame.

## The gas cycle (matter conservation)

Living things and fire are sources/sinks, so air composition is a closed loop:

- **Plants** photosynthesize: **−CO₂ +O₂** (need CO₂ + light + water — see
  [ecology](ecology.md)).
- **Animals / players / fire** respire / combust: **−O₂ +CO₂**.
- **Volcanoes** outgas: **+CO₂** (see [dynamic environment](dynamic-environment.md)).

Everything runs on **1:1 mole stoichiometry** —
`6 CO₂ + 6 H₂O + light → glucose + 6 O₂`, and respiration/combustion is the exact
reverse. Recipes only *move* moles between components; nothing is created or
lost. **Fire** ignites when fuel + ppO₂ + autoignition-temperature all hold →
consumes O₂ + fuel, emits CO₂ + heat (raising T → raising P, a feedback).

## Breathing — partial pressure, not percent

Player breathability is **derived from composition** and gated on **O₂ partial
pressure**, not its fraction — so thin air at altitude (low total pressure)
suffocates even at Earth's O₂ *fraction*:

| ppO₂ | Effect |
|---|---|
| ≥ ~16–21 kPa | safe |
| ~16 → 8 kPa | impaired (hypoxia warnings) |
| < ~5 kPa | suffocation |

**CO₂ / toxic is a *separate* damage channel** — harm above its own ppCO₂
threshold, independent of O₂ (not mere displacement). **Gear modifies the
effective supply:** an O₂ tank / rebreather supplies O₂; **gills extract O₂ from
a fluid's `oxygen_content`** (see [fluids](fluids.md)); a sealed suit seals out
toxic/vacuum. This plugs into the existing `stats.rs` oxygen + exertion systems,
mirroring the [heat hazard](temperature.md).

## Sealed volumes & terraforming

A sealed volume (the contained-quantity context) holds its own mixture:

- A **sealed CO₂ greenhouse** grows plants indoors by pumping up ppCO₂.
- An **O₂-exporter terraforms** a CO₂ world. Real Mars is ≈ 95% CO₂ but only
  **~610 Pa total** — so terraforming-by-photosynthesis raises ppO₂ over played
  time, but the deeper blockers are **too little total pressure** and **nitrogen
  limitation**, not lack of carbon. A richer goal than a single O₂ bar.

## Per-dimension defaults

`EnvDef.atmosphere_composition` is each world's **default mixture + total
pressure** that the open-air reservoir relaxes toward:

| World | Composition |
|---|---|
| Earth / overworld | ≈ 78% N₂ / 21% O₂ at 101 kPa |
| Mars | ≈ 95% CO₂ at 0.6 kPa |
| Airless moon | ≈ vacuum (0 kPa) |

This is kept **separate** from `EnvDef.atmosphere` (Rayleigh/Mie sky-scattering
params — see [rendering](rendering.md)): two different things.

## See also

- [matter-model.md](matter-model.md) — `GasDef`, the runtime contexts, the shared thermal trait.
- [ecology.md](ecology.md) — plants as CO₂/O₂ sources and the CO₂ growth axis.
- [fluids.md](fluids.md) — `oxygen_content` and gill extraction.
