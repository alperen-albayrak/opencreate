# Dynamic Environment: Volcanoes, the Chunk Heatmap & Dynamic Biomes

Forward **design** — **not built yet**. A living environment where heat sources
warp the climate around them, biomes drift over time, and volcanoes are born,
rage, and die. The whole system is **CPU/sim + RAM bound, not GPU** — a 5070 Ti
is irrelevant; 32 GB is ample — because it runs on a **coarse grid**, never
per-voxel. The time rules that make it cheap and exact live in [time](time.md);
this page is the heat, the volcanoes, and the biomes that ride on them.

Proven techniques it borrows: Terraria's Corruption/Hallow spread (a block-tick
cellular automaton — "biome" past a tile threshold, non-infectable blocks stop
it), voxel climate sims (temperature/humidity fields → biome), and Factorio's
per-chunk pollution CA (the same machinery).

## The coarse heatmap

A **per-section (16³) 3D grid**, sparse — one **heat** (+ **moisture**) value per
section, layered on top of the pure-function temperature base (tier 1 in
[temperature](temperature.md)). Only non-ambient sections are stored, like sparse
section storage, so the empty 99% of the world costs nothing.

- **3D buys height levels.** Heat diffuses vertically as well as horizontally, so
  deep is hot and high is cold, and **lava pumped in at a given Y warms that
  band** specifically.
- **Diffusion is a cellular automaton** running section↔section every N ticks —
  orders of magnitude fewer cells than a per-block field, and it parallelizes
  trivially.
- **The dynamic offset adds to the static climate**, it doesn't replace it:
  `effective_climate = static_noise(pos) + coarse_offset(section)`.

## Simulated wider than rendered

A bounded **climate set** — radius larger than the render distance, and
**anchored to active sources** like volcanoes — live-simulates the cheap coarse
grid *while the world runs*, so **unrendered chunks still evolve**. When you
finally walk into one of those chunks, its per-block surface
(`ice → water → grass → dry → desert`) **materializes lazily on visit** as the
blocks reconcile to their section's climate via random ticks. The coarse grid is
saved (it's tiny); offline the whole thing is **frozen** — this is live
simulation, never retroactive catch-up (see [time](time.md)).

## Volcanoes

A volcano is a **worldgen feature carrying a large *finite* heat/lava
reservoir** — a big **battery**, in the conservation language of
[temperature](temperature.md), not an infinite source. It **anchors a climate
region**: pumping lava raises that region's coarse heat **slowly**, because the
coarse field has high thermal inertia.

It **fades over a randomized 5–300 "year" lifespan** (real-volcano flavour) as a
**deterministic O(1) aging function** — there is no per-tick volcano sim:

```
activity = f(world_age − birth_age, lifespan)
```

Because it's a closed-form function of age, **a 5-year volcano and a 300-year
volcano cost exactly the same** to evaluate. The 5–300 "years" are compressed
through a tunable **geo-year ≈ a few play-hours**, keeping roughly the real-world
~1:60 short:long ratio so both fast and slow volcanoes feel distinct in a play
session.

### Worked example — a volcano's heat over its life

A volcano is born at `birth_age = 0` with `lifespan = 50` geo-years on a cold
plain (regional baseline ≈ 4 °C):

| `world_age` | `activity = f(age, 50)` | What the world looks like |
|---|---|---|
| 0 yr | rising | Eruption begins; reservoir full; lava starts pumping. |
| 10 yr | ~peak (≈40 yr "left") | Coarse heat around/above the vent has climbed with inertia; the warmed band has **melted nearby ice and greened the slopes**; deep sections glow (blackbody, see [temperature](temperature.md)). |
| 35 yr | declining | Reservoir depleting; lava output drops; the heat field stops climbing and begins to sag. |
| 50 yr | 0 (extinct) | Lava stops. With the source gone the coarse offset **relaxes back toward the 4 °C baseline** (closed-form, per [time](time.md)); the slopes cool, the greenery dies back, frost returns. |

The key move: nobody simulated 50 years of ticks. At any moment — including the
instant a never-visited chunk first generates — the volcano's state is just
`f(world_age − birth_age, lifespan)`, exact and free.

### Worked example — leave and come back centuries later

Camp beside a *young, warm* volcano on a **−18 °C ice world**, then travel to a
[slow, high-gravity dimension](time.md) and return after **~300 yr** of
`world_age`:

- **Volcano** (deterministic, layer a): `f(age)` → long **extinct** (it died
  ~295 yr ago, well past its lifespan).
- **Coarse climate** (layer b): source gone → equilibrium = ambient −18 °C, and
  `Δworld_age ≫ τ`, so the offset **relaxes fully to −18 °C** in one closed-form
  step on load.
- **Surface** materializes on visit → **rows of snow and ice**.
- **Your camp** (player edits, layer c) **persists — buried in snow.**

"The volcano died and the land re-froze over centuries" — produced entirely by
closed-form evaluation, with no replay and no special-casing. The layer model
(a/b/c) and the relativity that makes 1 hour away cost 300 years here both live
in [time](time.md).

## Dynamic biomes

Biomes are not a fixed worldgen stamp — they're a **derived label** over the
effective climate (see [ecology](ecology.md)):

```
effective_climate = static_noise(pos) + coarse_dynamic_offset(section)
```

evaluated over **both a heat and a moisture channel**. The existing multi-noise
climate→biome lookup in [world generation](../server/world-generation.md)
consumes it unchanged, so **biomes shift** as the offsets move, via the same
Terraria/MC-style climate-driven block-tick conversion.

**Moisture is symmetric to heat:**

- Add water to a desert → coarse moisture rises → it **greens**.
- High heat **evaporates** moisture → a hot desert **stays dry** unless
  re-watered.

So a heat↔moisture feedback emerges for free, and the same `effective T` that
drives the biome also feeds the **player heat hazard** (see
[temperature](temperature.md)) at no extra cost.

## Limits (design choices, not hardware)

- **Coarse**, not per-voxel.
- **Regional**, not a global climate model.
- **Gradual** (random-tick paced), not instant.
- 3D heat is **coarse-grid diffusion**, not a full PDE solver.
- **Offline = frozen.**

## Reserved idea — a time-accelerator machine

A *powered, in-region* device that locally multiplies the sim rate (crops,
smelting, weathering run at `N × dt`), with an energy cost ∝ multiplier × volume.
This is **distinct from the per-dimension relativity** in [time](time.md) (which
is free and global per dimension). Far-future, post-§6.6, **no committed design**
— noted only so the architecture doesn't preclude it.
