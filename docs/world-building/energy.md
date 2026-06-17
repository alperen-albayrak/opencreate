# Energy

**A design principle that shapes the world now, plus a reserved power network
(phase 6).** Conservation of energy is a project-wide pillar: it is what makes
heat, power, and machines a *real constraint* instead of cosmetic flavour. This
page is the general statement; [temperature](temperature.md) is its thermal
half, and [dynamic-environment](dynamic-environment.md) shows it driving
volcanoes.

## No free energy

There are **no perpetual or infinite-output blocks**. Every joule is **harvested,
stored, converted, or spent — never created**. A heat source isn't a magic
emitter; it is something giving up energy it got from somewhere. This single rule
is why a power setup has to be *designed* rather than just placed.

## Reservoirs vs batteries

The central distinction. Both obey conservation — the difference is whether the
store runs out on a human timescale.

| Kind | Examples | Behaviour |
|---|---|---|
| **Reservoir** (effectively infinite) | the planet's geothermal **core**, the **star** (solar), **wind** | an external flow you *tap*; capturing it never depletes it on play timescales (you still only capture a finite flux per tick) |
| **Battery** (finite stored) | **lava** lakes, **heated blocks**, **fuel** (coal/charcoal), charged accumulators | a fixed amount of stored energy that **drains as it's used** and must be refilled or replaced |

The consequence that does the gameplay work: **geothermal is sustainable because
a deep tap cools local rock that re-conducts from the vast deeper core** (a
reservoir, like the sun) — while a **surface lava lake is a quick finite
battery** that cools and solidifies. Same heat, different source class → different
sustainability. Emergent gating: endless power lives **deep, behind the heat
hazard**.

## Forms & the conversion chain

Energy moves between forms, and **every conversion is lossy** (no perpetual
motion — some always leaks to ambient as heat):

- **Thermal** — heat/temperature ([temperature](temperature.md)). Harvested from
  the core, lava, or fuel; drives blackbody glow, phase changes, and the heat
  hazard.
- **Kinetic / rotational** — Create-style mechanical power through shafts and
  cogs (**stress × speed**). Sources: water wheel, windmill, steam engine.
- **Electric** — the wired network. Resistive heating **`P = I²R`** turns electric
  → thermal, a real incandescent element (see the lamp section of
  [temperature](temperature.md)).
- **Chemical** — fuel **combustion** (`−O₂ +CO₂ + heat`, see
  [atmosphere](atmosphere.md)); and food → the player's exertion budget (the
  reserved nutrition model in [ecology](ecology.md)).
- **Radiant** — sunlight captured by solar.

The universal pattern is **harvest → convert → transmit → consume**: sources tap
an external flow or release a store, the network moves it, consumers spend it.

## The power network (reserved — §6.5, phase 6)

The block network that ties these forms together is **reserved, not built**:
rotational power graphs (stress/speed through shafts/cogs, Create-style) and an
**electric network** (`P = I²R` heating elements, where `R` = a block's
`resistivity` × geometry and `I` = network current). Sources convert/harvest,
consumers spend, the network moves energy. The hooks already exist on `BlockDef`
(`resistivity`, redstone/network metadata — see [matter-model](matter-model.md));
until the network lands, energy is expressed only through the **thermal** model.

## Worked example: the endless heater

A **solar-panel farm wired to an electric heater runs forever** — because the sun
(`EnvDef`'s star) is a **reservoir**, not a battery. Contrast the *same heater*
fed differently:

- **Charcoal-burning element** → stops when the fuel runs out (a **chemical
  battery**).
- **Lava-warmed plate** → goes cold as the lava gives up its heat and **crusts to
  stone/obsidian** (a **thermal battery**).
- **Deep geothermal tap → generator → element** → sustainable, because it draws
  from the core **reservoir**.

Same device, different source class, different outcome — and that distinction *is*
the constraint the player plays against.

## Bookkeeping (how conservation is enforced)

- No source emits more than it harvests or has stored. Discharge is **emergent
  from material data**, not a scripted timer: cooling follows Newton's law
  `dT/dt ∝ −(T − ambient) / (heat_capacity · mass)`, with a **latent-heat
  plateau** at phase changes.
- Conversions shed energy to ambient, so efficiency is always < 100% — **no closed
  loop gains energy**.

## Matter is conserved too

Energy's twin pillar: matter doesn't appear from nothing either. **Ore deposits
are finite and deplete** ([geology](geology.md)); recycling, decay, and the
**soil-nutrient cycle** ([ecology](ecology.md)) return matter. The two
conservation laws run in parallel and give the world its long-term economy.

## Validation

- **Factorio** — solar + accumulators (reservoir + battery) and a real power
  network balancing production against consumption.
- **Create** — rotational power as stress × speed through a kinetic graph.
- **Real thermodynamics** — energy conservation with lossy conversion; no
  perpetual motion.

## See also

- [temperature.md](temperature.md) — thermal energy, reservoir vs battery in heat terms, the worked volcanic-vent example.
- [dynamic-environment.md](dynamic-environment.md) — a volcano as a large finite heat battery fed by the deep reservoir.
- [atmosphere.md](atmosphere.md) — combustion (chemical → thermal), O₂ as the limiter.
- [../../ARCHITECTURE.md](../../ARCHITECTURE.md) — §6.5, the reserved power/block network.
