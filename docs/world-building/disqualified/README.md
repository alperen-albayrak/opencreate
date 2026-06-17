# Disqualified — ideas we moved on from

A record of approaches we **considered and deliberately rejected** while
designing the [world model](../README.md). Each entry follows one shape:

> **Considered** — the idea. **Why we moved on** — the problem it ran into.
> **Instead** — what we chose (with a link to the accepted page).

This is the mirror image of [../../decisions.md](../../decisions.md) (chosen
paths): it exists so a future contributor — or a future us — doesn't burn time
re-proposing a dead end or quietly reintroducing one we already corrected.
Nothing here is implemented; these are design decisions, not code.

Some entries are *not* permanent vetoes — they're "not this, not now," and say
so. A few marked **(correction)** are mistakes we actually made in the design and
fixed; those are the most important not to repeat.

## Pages

| Page | Rejected ideas it records |
|---|---|
| [rendering.md](rendering.md) | Pure-forward rendering, fat G-buffer, baked sky color, monochrome-only light, shadow maps |
| [matter-and-fields.md](matter-and-fields.md) | Separate `light_filter`/`absorption`, separate `luminance`/`light_color`, stored `weight_class`, `render_layer` as truth, raw block ids in saves |
| [temperature-time-energy.md](temperature-time-energy.md) | Per-voxel temperature field, tick-replay catch-up, "resume cooling on return", infinite energy sources, wall-clock aging, full pre-simulated history |
| [ecology-and-atmosphere.md](ecology-and-atmosphere.md) | Gaussian niche curves, arithmetic-mean suitability, biome names as primitives, a pollution axis, a scalar "breathable" value, single-gas air models |

## The shortlist (one line each)

- **Forward+ rendering** → deferred PBR ([rendering](rendering.md)).
- **Monochrome light as the model** → RGB light, mono only as a Low tier.
- **`light_filter` + `absorption` as two fields** → one `extinction`.
- **Separate `luminance`/`light_color`** → derived from `emissive`.
- **Stored `weight_class`** → derived from continuous `density`.
- **`render_layer` as truth** → derived from `opacity`/alpha.
- **Full per-voxel temperature field** → three-tier sparse model.
- **Tick-replay catch-up / "resume cooling"** → offline freeze + closed-form forward-eval *(correction)*.
- **Infinite/perpetual energy, lava as infinite** → conservation; finite batteries.
- **Wall-clock world aging** → played-time-only `world_age` freeze.
- **Full pre-simulated world history** → only the initial footprint; the rest is `f(seed, world_age)`.
- **Gaussian niche curves** → trapezoids; **arithmetic mean** → geometric mean.
- **A separate pollution axis** → folded into the gas-composition field.
- **A scalar "breathable" air value / single-gas air** → multi-component composition with partial pressures.
