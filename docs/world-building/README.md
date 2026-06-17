# World-Building Design

Forward **design** for OpenCreate's physically-grounded world model. Unlike the
rest of [docs/](../README.md) — which describes the game **as built** — these
pages are mostly **not yet implemented**: they are the locked roadmap for
terrain, climate, ecology, the three states of matter, temperature, the dynamic
environment, and the "Vibrant Visuals"-parity renderer. Each page marks what is
shipped vs. designed. The complete, unabridged source is the approved plan; this
folder reorganizes it by subject so it can actually be read.

## The grounding idea

A natural world driven by **real physical constants**, not per-case fudges.
Every effect is a function of a real-world model; `physical.rs` is the single
source of constants (Beer–Lambert, Rayleigh/Mie, Fresnel, blackbody) so the
whole world stays calibrated and consistent. Five principles run through every
page here:

- **Physics first** — looks and behavior derive from measured constants.
- **Conservation of energy** — no free/infinite sources. Lava is a finite
  *battery* that cools and solidifies; the only *endless* supply is harvesting an
  external natural source (the star, wind, or the planet's geothermal core).
- **Conservation of matter** — ore deposits are finite and deplete; recycling,
  decay, and the soil-nutrient cycle return matter. No free resources.
- **`world_age` freezes offline** — no tick-replay catch-up; deterministic
  closed-form forward-evaluation `f(world_age)` is allowed and cheap.
- **Data-driven & modular** — substances, planets, and looks live in `data/`
  (registries), so fluids, gases, and whole worlds are *content, not code*; each
  subject is its own module behind a quality-tier setting.

## Three states of matter → three registries

| State | Registry | Examples |
|---|---|---|
| Solid | `BlockDef` (`data/blocks.ron`) | stone, planks, ore, ice, glass |
| Liquid | `FluidDef` (`data/fluids.ron`) | water, lava, oil, milk, mud, blood |
| Gas | `GasDef` (`data/gases.ron`) | O₂, CO₂, N₂, steam, helium, methane |

A fourth registry, **`EnvDef`** (`data/dimensions/*.ron`), describes each
dimension/planet: gravity, the default atmosphere composition, celestial bodies,
and an optional thermal (geothermal) profile. The three matter registries are
**unified by shared trait fragments** (optical surface, volumetric medium,
emissive+thermal, respiration, mass) so the same field means the same thing
across all of them — and so a substance can move *between* registries when it
freezes/melts/boils (lava → stone, ice → water → steam).

## Pages

| Page | What's in it |
|---|---|
| [matter-model.md](matter-model.md) | The three registries, shared trait fragments, runtime contexts, canonical field names, and cross-registry phase transitions |
| [temperature.md](temperature.md) | The three-tier temperature model, the geothermal deep core, blackbody glow, the heat hazard, and the energy-conservation rule |
| [energy.md](energy.md) | Conservation of energy: reservoirs vs. batteries, energy forms & the conversion chain, the reserved power network |
| [atmosphere.md](atmosphere.md) | Air as a multi-component gas mixture: O₂/CO₂/N₂, `PV=nRT`, partial-pressure breathing, terraforming, sealed volumes |
| [fluids.md](fluids.md) | `FluidDef`: per-channel absorption, buoyancy, viscosity, breathability/oxygen |
| [ecology.md](ecology.md) | Climate field, calendar/seasons, plant & creature niche curves, the soil-nutrient cycle, super-plants |
| [time.md](time.md) | How time works: `world_age`, offline freeze, closed-form forward-eval, per-dimension relativity (time shift), and rendered vs. unrendered vs. ungenerated chunks |
| [dynamic-environment.md](dynamic-environment.md) | Volcanoes, the coarse chunk heatmap, dynamic biomes, generated history (all driven by the [time](time.md) model) |
| [geology.md](geology.md) | Layered rock strata and data-driven ore veins |
| [rendering.md](rendering.md) | The deferred-PBR "Vibrant Visuals"-parity graphics roadmap |
| [disqualified/](disqualified/README.md) | Ideas and approaches we **considered and moved on from**, each with why — so we don't repeat the dead ends |

## See also

- [../ARCHITECTURE.md](../../ARCHITECTURE.md) — the original approved engine design (referenced as "§N").
- [../roadmap.md](../roadmap.md) — the six development phases and shipped graphics work.
- [../server/world-generation.md](../server/world-generation.md) — the terrain pipeline these systems build on.
