# Disqualified — Ecology & Atmosphere

Approaches considered and rejected for [ecology](../ecology.md) and
[atmosphere](../atmosphere.md). Shape: **Considered** → **Why we moved on** →
**Instead**.

## Gaussian niche tolerance curves

- **Considered** — model each environmental tolerance (temperature, hydration, …)
  as a **bell curve** with a single peak.
- **Why we moved on** — real biological tolerances have a **flat optimal
  plateau**, not one sharp peak; a Gaussian is also harder to author and tune, and
  doesn't match TFC's data shape.
- **Instead** — a **trapezoid** `{min, opt_lo, opt_hi, max}` (zero outside, flat
  1.0 across the optimum, linear ramps between), which is exactly TFC's min/max +
  wiggle range. See [../ecology.md](../ecology.md).

## Arithmetic-mean (or Liebig-minimum) suitability

- **Considered** — combine the per-axis suitabilities by **averaging** them, or by
  taking the strict **minimum** (Liebig's law of the minimum).
- **Why we moved on** — the arithmetic mean is **too forgiving** (a plant with one
  zero axis still scores well); the hard minimum is **too sharp** (discontinuous,
  ignores the second-worst axis).
- **Instead** — the **geometric mean**: any single near-zero axis throttles the
  whole organism, but *smoothly*. See [../ecology.md](../ecology.md).

## Biome names as authored primitives

- **Considered** — hand-author named biomes ("Forest", "Desert") that decide what
  grows and spawns.
- **Why we moved on** — authored biomes can't stay consistent with a **dynamic
  climate**, and they force flora and fauna to be painted on **independently** of
  the conditions.
- **Instead** — a **niche/suitability model** over continuous climate axes; the
  **biome is a derived label** read back from the result. See
  [../ecology.md](../ecology.md).

## A separate pollution / contamination axis

- **Considered** — a Factorio-style **pollution** field added to the environment
  vector (its own spread + niche effects).
- **Why we moved on** — environmental harm is **already what breathing measures**;
  a second cellular-automaton field duplicates the gas field. *(Project direction:
  "pollution is not needed; breathable is enough.")*
- **Instead** — folded into the **gas-composition field** as toxic / CO₂
  components, spread by the same CA. See [../atmosphere.md](../atmosphere.md).

## A scalar "breathable" air value

- **Considered** — a single flat 0–15 `breathability` number for air.
- **Why we moved on** — a scalar can't express **thin air at altitude**, **sealed
  CO₂ greenhouses**, or **terraforming** a CO₂ world toward breathable.
- **Instead** — breathability **derived from gas composition** (O₂ **partial
  pressure**, safe ≥ ~16 kPa) with a **separate** CO₂/toxic damage channel. See
  [../atmosphere.md](../atmosphere.md).

## Single-gas air

- **Considered** — model the atmosphere as **one** gas.
- **Why we moved on** — can't represent CO₂-rich worlds, O₂ enrichment, or the
  fact that **plants need CO₂** while players need O₂.
- **Instead** — air is a **multi-component composition** (a mixture of `GasDef`
  components). See [../atmosphere.md](../atmosphere.md).

## Single-gas-per-tile / binary-sealed / full per-voxel gas models

- **Considered** — ONI's **one-gas-per-cell** field; Space Engineers' **binary**
  airtight-or-not flag; or a full **per-voxel multi-gas** simulation.
- **Why we moved on** — one-gas-per-tile **can't hold a mixture**; the binary flag
  **loses composition** entirely; per-voxel multi-gas is **far too expensive**.
- **Instead** — a **multi-component mole mixture per coarse cell** (`PV=nRT`,
  Stationeers-style): open air is **one well-mixed reservoir**, and the detailed
  per-chunk field runs **only near sealed structures**. *(ONI's cheap
  density-sort layering — CO₂ sinks, H₂ rises — is kept.)* See
  [../atmosphere.md](../atmosphere.md).
