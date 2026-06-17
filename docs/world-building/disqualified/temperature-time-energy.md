# Disqualified — Temperature, Time & Energy

Approaches considered and rejected for [temperature](../temperature.md) and
[time](../time.md). Shape: **Considered** → **Why we moved on** → **Instead**.
Entries marked **correction** are mistakes we actually made in the design and
fixed — the most important ones not to repeat.

## Full per-voxel temperature field

- **Considered** — a temperature value simulated on every voxel in the world.
- **Why we moved on** — enormous storage and CPU, and **almost entirely
  redundant**: away from a heat source, every cell just equals the base function.
- **Instead** — the **three-tier model**: pure base function + sparse source
  delta + stored temperature *only* on actively-heated blocks. See
  [../temperature.md](../temperature.md).

## Tick-replay catch-up ("simulate the missed time")  *(correction)*

- **Considered** — on load, replay the simulation ticks skipped while the world
  was closed (the Luanti approach).
- **Why we moved on** — expensive and only approximate, and **unbounded** for long
  gaps (quit for a year → replay a year?).
- **Instead** — **offline freeze** (`world_age` doesn't advance while closed) plus
  **closed-form forward-evaluation** `f(world_age)` on load. Exact and O(1). See
  [../time.md](../time.md).

## "Resume cooling exactly where it left off" for the coarse climate  *(correction)*

- **Considered** — freeze the coarse climate offset and, on return, resume from
  the saved value.
- **Why we moved on** — the **ice-world scenario** broke it: camp by a volcano,
  leave for centuries of `world_age`, return — the volcano is long extinct and the
  land *must* have re-frozen, but naive resume keeps it warm.
- **Instead** — **closed-form relaxation toward the *current* equilibrium**, using
  a per-cell `world_age` timestamp: `offset(now) = eq(now) + (offset_saved −
  eq(now))·exp(−Δworld_age/τ)`. *(Per-block **stored** heat does still freeze —
  that's a different, edit-like layer.)* See [../time.md](../time.md).

## Infinite / perpetual energy sources

- **Considered** — lava (or "heat blocks") as an endless energy supply.
- **Why we moved on** — breaks **conservation of energy**; it's perpetual motion.
- **Instead** — heat sources are finite **batteries** (lava gives up its heat and
  **solidifies**; fuel depletes). Only **externally-harvested reservoirs** — the
  star (solar), wind, the planet's geothermal **core** — are effectively endless.
  See [../temperature.md](../temperature.md).

## Wall-clock world aging

- **Considered** — age the world against **real elapsed time** / the real
  calendar.
- **Why we moved on** — punishes players for stepping away, and is inconsistent
  between single-player and a multiplayer server.
- **Instead** — **played-time-only `world_age`**, frozen offline. See
  [../time.md](../time.md).

## Full pre-simulated world history everywhere

- **Considered** — a Dwarf-Fortress-style narrative history (civilizations,
  ruins, events) simulated across the **whole** world.
- **Why we moved on** — we **can't pre-simulate ungenerated space**, and we never
  tick-replay it; a history for chunks no one has visited has no place to live.
- **Instead** — pre-simulate only the **initial worldgen footprint**; everything
  beyond is **procedurally aged** by `f(seed, pos, world_age)` (looks weathered,
  isn't a recorded narrative). A bounded feature, **reserved**. See
  [../time.md](../time.md).
