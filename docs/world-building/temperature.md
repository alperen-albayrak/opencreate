# Temperature & the Deep Core

**Design, not yet built.** A temperature field over the world that makes deep
digging hot and hazardous, lets blocks glow by incandescence, and drives phase
changes — without a full per-voxel simulation. The trick is that almost all of
it is a **pure function**, with sparse dynamic state only where something is
actually being heated.

## The three-tier effective temperature

Effective temperature decomposes into three tiers, most of it free:

1. **Static base — a pure function (zero storage, never saved):**
   ```
   T_base(pos) = clamp(surface_temp + geothermal_gradient·depth, …, core_temp)
                 + biome/altitude modifiers
   ```
   A pure function of position + `EnvDef.thermal`, like the skylight-shaft rule
   and worldgen climate noise. Queryable anywhere instantly; recomputed like
   terrain. Gives deep-core heat, the geothermal glow gradient, baseline player
   heat, and fluid/gas equilibrium temperature.
2. **Dynamic source delta — sparse, bounded, source-driven:** fire/lava/heated
   elements add a *local* delta via a bounded flood-fill from active sources
   within the §6.6 active area. **Reuses the block-light BFS** — same machinery
   and cost class as lamp light. Most of the world has no source, so temperature
   is just the base function.
3. **Per-block stored temperature — only on actively-heated blocks** (the
   reserved `temperature` block-state), cooling toward local ambient by Newton's
   law. Sparse: only cells meaningfully above ambient carry state (near-ambient
   snaps to none). **Persisted like a block edit, and frozen offline — no
   elapsed-time catch-up** (see [time](time.md)): a block heated to 200° reloads
   at 200° and only cools as you keep playing.

```
effective T = base(pos) + source_delta(pos) [+ stored if heated]
```

A function call plus a sparse lookup. Consumers are cheap one-shot queries
(player hazard 1 sample/tick; buoyancy equilibrium; phase change event-driven
when T crosses a melt/boil point) — never a global scan.

## The thermal trait

Thermal is a **shared trait across all three registries** (see
[matter model](matter-model.md)), not just solid blocks. The static material
props:

| Field | Meaning |
|---|---|
| `heat_capacity` | thermal mass — how much energy to change temperature |
| `conductivity` | how fast heat flows to neighbours |
| `resistivity` | electric-heating resistance (mostly solids) |
| `melting_point` / `boiling_point` / `ignitable` | phase-change / combustion thresholds |

The temperature field samples whatever matter sits at a cell. Consequences that
*unify* other features: **lava glows because it is a hot fluid** (same blackbody
pipeline); **hot-air buoyancy *is* temperature** (a hot gas is less dense → rises,
so `density` is a function of T — see [atmosphere](atmosphere.md)); conduction
flows between any adjacent matter; and **phase transitions** move matter between
registries (lava→obsidian, ice→water→steam — see [matter model](matter-model.md)).

## Heat transfer: simulate gradients, not blocks

The discipline that keeps tiers 2–3 cheap is Minecraft's own
([block update](https://minecraft.wiki/w/Block_update)): **never scan the world —
do work only where something changed, and only while it stays out of equilibrium.**
A cell does heat work only when there is a real *gradient*; once it matches its
surroundings it drops out of the simulation and costs nothing.

- **Equilibrium is free.** Tier-1 `base(pos)` *is* the equilibrium field. Two
  adjacent deep rocks both sit at `base` → ~0 gradient → nothing to transfer, so
  nothing is computed. The quiescent majority of the world (deep mining away from
  any source) costs only the pure base function. Gradients exist in just three
  places: next to an active source (tier 2), around a recent **block edit**, and
  at a **phase boundary**.
- **The trigger is the block-update hook** — Minecraft's neighbour-notification,
  reused. `set_block` already fires on every edit; that is where an affected cell
  is added to the heat **active-set** and its six neighbours are woken — "notify
  nearby blocks to re-check," never a global pass.
- **The active-set lifecycle.** A cell enters on a source change, a block edit, or
  a gradient from an active neighbour; it relaxes by discrete Newton/Fourier
  conduction `ΔTᵢ = Σⱼ kᵢⱼ·(Tⱼ − Tᵢ)·dt / (heat_capacityᵢ · massᵢ)` — per-pair
  `conductivity` k, so **insulators shield**; it **leaves** once `|T − ambient| < ε`
  (snap to none). Stepped every N ticks over the active-set only, **frozen offline**
  (see [time](time.md)), saved sparsely. Sources are finite, so they cannot create
  unbounded work.

**Latent-heat plateau (the boiling pot).** When a cell reaches a phase point its
incoming heat stops raising `T` and instead fills a **latent-heat accumulator**
(the enthalpy of the transition); `T` is **pinned** at the phase point until the
accumulator fills, then the matter converts (a cross-registry
[phase transition](matter-model.md)) and `T` moves again. So a pot of water over a
fire **holds at 100 °C until the last of it has boiled** to steam — real
thermodynamics, event-driven, no extra machinery.

**A placed cold block equilibrates to ambient — and glows on the way.** Drop a
~24 °C block into the ~900 °C deep: the edit puts it in the active-set out of
equilibrium with `base(pos)`, so it Newton-relaxes *upward*, **crossing the Draper
point (525 °C) and glowing** dull-red → orange as it warms, then drops out once it
reaches ambient. The deep is a **reservoir** — it does not measurably cool by
heating one block; the surrounding field *is* the source, so individual neighbours
need no special flag. The mirror case is a lava block (a finite **battery**) moved
somewhere cool: it gives up its heat and solidifies. A block placed where the
ambient already matches it has ~0 gradient and never ticks — so only matter moved
across a temperature difference costs anything.

## Conservation of energy

There are **no free or infinite sources** (a project-wide principle — see
[energy](energy.md) for the general statement). The key distinction is
**reservoir vs battery**:

- **Reservoirs** (effectively infinite): the planet's geothermal **core**, the
  **star** (solar), and **wind**. A geothermal→electric loop is sustainable
  because a deep tap cools local rock that re-conducts from the vast deeper heat.
- **Batteries** (finite stored heat): **lava** and heated blocks. Lava is a hot
  fluid that **cools and solidifies** as it gives up heat; fuel burners deplete.

Cooling is emergent from material data:
```
dT/dt ∝ −(T − ambient) / (heat_capacity · mass)
```
so **lava (huge `heat_capacity` × volume) drains slowly** while a thin metal bar
cools fast. Phase transitions add a **latent-heat plateau**. Emergent gating:
sustainable geothermal lives **deep, behind the heat hazard**; a surface lava
lake is a quick *finite* battery.

## Blackbody glow

Any matter's `emissive = blackbody(local_temperature)` past the **Draper point
≈ 798 K (525 °C)**, so **deep rock glows dull-red → orange before any lava
appears** (and lava glows by the same rule). The static geothermal glow is a
pure function of depth → **baked into `emissive` at mesh time** (free, like baked
light); only dynamic heated blocks need a sparse emissive update. See
[rendering](rendering.md) for the blackbody → bloom pipeline.

Cross-validated by TerraFirmaCraft's heat-color ladder (`Heat.java`), a
ready-made `temperature → color` table:

| Tier | Range |
|---|---|
| FAINT_RED | 480–580 °C (≈ 750–850 K, straddles the Draper point) |
| DARK_RED | 580–730 °C |
| BRIGHT_RED | 730–930 °C |
| ORANGE | 930–1100 °C |
| YELLOW | 1100–1300 °C |
| YELLOW_WHITE → WHITE | 1300–1500 °C |
| BRILLIANT_WHITE | 1500–1600 °C |

## Worked example: heat around a volcanic vent

Putting the three tiers, conservation, glow, and phase change together at a
magma vent (illustrative, game-tuned numbers):

- **Tier 1 — base (free).** Deep by the vent the tuned geothermal base already
  reads ~550 °C — past the Draper point — so the **raw rock glows FAINT_RED on
  its own, before any lava**. This gradient is a pure function of depth, baked
  into `emissive` at mesh time.
- **Tier 2 — source delta (sparse).** The lava itself is a hot fluid at ~1100 °C
  (ORANGE). A bounded flood-fill — the same BFS as lamp light — spreads a
  falling heat delta into the surrounding rock and cavern air: nearby cells climb
  a few hundred degrees, the air warms, and the player's [heat hazard](#player-heat-hazard)
  starts ticking. No global field, just a local bubble around the source.
- **Tier 3 — stored (only where heated).** Drop an iron block at the lava's edge:
  it becomes an actively-heated block carrying a `temperature` block-state, rising
  toward local ambient (÷ `heat_capacity`), glowing red → yellow → a lamp. Walk
  away and it reloads at that temperature — **frozen offline, no catch-up** (see
  [time](time.md)) — cooling only as you keep playing.
- **Conservation — the vent is finite.** The lava lake is a *battery*: huge
  `heat_capacity` × volume, so it drains **slowly** (`dT/dt ∝ −(T−ambient) /
  (heat_capacity·mass)`) — but it drains. As it gives up heat it crosses its
  freezing point and **crusts over**: fast-quenched at a water edge → **obsidian**,
  slow-cooled → **basalt** (a cross-registry [phase transition](matter-model.md));
  the latent-heat plateau holds it at the freezing temperature while it solidifies.
  The vent stays hot only as long as the deeper reservoir keeps feeding it — i.e.
  the volcano's finite lifespan in [dynamic-environment](dynamic-environment.md).

## Player-heated blocks become lamps

Heat a metal block by **fire/forge** (steel red ~800 K → yellow-white ~1500 K)
or by **electric resistive heating**: `P = I²R`, where **R** = the block's
`resistivity` × geometry and **I** = current from the reserved §6.5 power
network. Abstract units, real ratios — heating elements are high-R materials
(nichrome/tungsten ≫ copper) tuned to reach the Draper point at a sensible power
level. Temperature rises with input (÷ `heat_capacity`) and cools by Newton's
law; past the Draper point the block's `emissive` glows **and its cast light is
derived from temperature** → it literally becomes a lamp. Needs dynamic block
temperature + the power network; both reserved now, neither implemented.

## Player heat hazard

Mirrors the breathing model (see [atmosphere](atmosphere.md)): a survival stat that
takes damage outside a survivable band, **insulation gear** widening tolerance
(same `environment + gear_modifier` shape as `breathability`). Plugs into
`stats.rs` like oxygen, and reads `effective T`, so it picks up source heat, the
dynamic [heatmap](dynamic-environment.md) (volcano warmth), and any local cooling
for free.

The player takes damage from the **effective temperature at their position even in
open air** — but the *rate* scales with how well the surrounding medium delivers
heat (real heat-flux physics, nature's conductivity ratios):

```
heat_dps = max(0, T_eff − T_safe) × medium_conductivity × (1 − gear_insulation)
```

| Medium | conductivity (W/m·K) | feel |
|---|---|---|
| air | ~0.025 | hot air hurts *slowly* — you can dash through a hot pocket (sauna) |
| water | ~0.6 | ~25× air — hot water is quickly dangerous |
| rock | ~2–3 | touching hot rock is worse |
| lava | ~1–2 | submersion is effectively instant death |

The medium is the "effective matter at a point" rule (voxel fluid if present, else
the gas/air — see [matter model](matter-model.md)). Two emergent consequences:

- **Coolant pockets.** Water is a high-`heat_capacity`, finite **heat-sink**: poured
  into a hot cave it conducts heat out, reaches 100 °C and **boils away on the latent
  plateau**, opening a *temporary* survivable pocket sized by how much you haul down
  (near 900 °C it flashes fast). Standing in the cooled air / steam → lower `T_eff`
  → less damage, until it is spent and the reservoir reheats.
- **Built warmth.** A furnace/fire warms its immediate blocks strongly (tier-2) and
  nudges its 16³ cell's average via the coarse [heatmap](dynamic-environment.md) — a
  single furnace barely moves a whole chunk, a sustained fire or volcano moves a
  region; the warmth decays when the finite fuel runs out.

## Deep-core content

Lava lakes, obsidian / cold-lava-stone caves, and hellish creatures are all
`BlockDef`/`FluidDef`/creature-registry **content** — no new systems. The hot
deep is **optional per planet**: an Earth-like world sets `EnvDef.thermal`
(`surface_temp`, `geothermal_gradient` ~0.025 K/m scaled to block depth,
`core_temp`); a **big airless moon omits `thermal`** → uniformly cold, no
deep-hot zone.

## See also

- [matter-model.md](matter-model.md) — the thermal trait and phase transitions.
- [time.md](time.md) — why stored heat freezes offline with no catch-up.
- [dynamic-environment.md](dynamic-environment.md) — the coarse heatmap and volcanoes.
