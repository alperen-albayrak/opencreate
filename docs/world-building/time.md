# How Time Works — `world_age`, Offline Freeze & Relativity

**Design, not yet built.** This page is the locked model for how time flows in a
world, what happens to a world while you are away, and what happens across
dimensions that run at different rates. It underpins [volcanoes and the dynamic
environment](dynamic-environment.md), [seasons](ecology.md), and the offline
behaviour of [temperature](temperature.md). The guiding constraint is the same
one [world generation](../server/world-generation.md) already lives by:
**prefer pure, deterministic functions over stored simulation history.**

## `world_age`: one clock, cumulative *played* time

There is a single global counter, `world_age`, saved with the level. It measures
**cumulative time the world has actually run** — not wall-clock time, not the
real calendar. It advances **only while the world is being simulated**:

- **Singleplayer** — advances while you play; **frozen** on save/quit and while
  the game is paused. This matches the existing `SetPaused` rule (see
  [the server tick](../server/README.md)): the SP simulation freezes when paused.
- **Multiplayer** — advances while the server is up (the server ignores client
  pause), and is frozen only when the server itself is down.

Quit for a week or walk away for an hour and `world_age` does not move. There is
no hidden "real time" that the world ages against.

## The hard rule: no catch-up — but forward-evaluation is free

Two things that sound similar are treated very differently:

- **Banned: tick-replay catch-up.** We never replay skipped ticks. A world that
  was closed does not "fast-forward" thousands of simulation steps on load. This
  supersedes the Luanti-style *simulate-missed-time* approach for our SP case.
- **Allowed and cheap: closed-form forward-evaluation `f(world_age)`.** On load
  we may *evaluate* deterministic state at the current `world_age` in O(1) —
  worldgen aging and coarse-climate relaxation (below). No loop over elapsed
  time; just plug `world_age` into a function.

Because `world_age` is frozen offline, **an offline gap has `Δ = 0`**, so a saved
world **resumes exactly as it was saved**. Any forward-aging you observe reflects
`world_age` that *actually advanced* — time you spent playing, possibly in
another dimension (see [relativity](#per-dimension-relativity-time-shift)).

## Rendered, loaded, and not-yet-generated

Time affects three kinds of space differently:

| Where | What happens over `world_age` |
|---|---|
| **Active area** (rendered / near a player) | Full per-block simulation runs: block ticks, fluids, [temperature](temperature.md) cooling. This is the only place the expensive sim lives. |
| **Loaded but unrendered** | The cheap **coarse chunk-climate** layer may live-simulate a wider in-memory region while the world runs, so unrendered chunks still evolve (see [dynamic environment](dynamic-environment.md)). The per-block sim is frozen here. |
| **Not yet generated** | Nothing is simulated. When the region is first generated, its aged state is computed as `f(seed, pos, world_age)` — already correct for the current age. |

The not-yet-generated case is the elegant one. A region that has **never been
visited** has no "true simulated path" that we could have missed — so evaluating
`f(seed, pos, world_age)` *is* the truth, not an approximation. A volcano born
with a 5-year life, in a region first generated when 1 year has been played, is
evaluated at age 1 → **4 years of life left**, automatically, whether or not you
ever go there.

## Three aging layers

State is partitioned by *how it ages on load*:

| Layer | What | How it ages |
|---|---|---|
| **(a) Worldgen-aged** | volcanoes, climate baseline drift, erosion-as-function | always-current `f(world_age)` — **no timestamp stored** |
| **(b) Coarse-climate offset** | heat / moisture added by sources or the player | **forward relaxation** toward current equilibrium, per-cell timestamp |
| **(c) Player edits / structures** | placed blocks, built bases | **persist** verbatim; do not thermodynamically decay |

**Layer (b)** is the only place a per-cell timestamp lives. Each coarse cell
records the `world_age` at which it was last touched; on load it relaxes toward
its present equilibrium in closed form:

```
offset(now) = eq(now) + (offset_saved − eq(now)) · exp(−Δworld_age / τ)
```

It is **not frozen** — over a long gap (`Δworld_age ≫ τ`) it relaxes *fully* to
equilibrium. One evaluation per cell on load; no replay.

**Layer (c)** persists exactly — your base does not rot — but the *environment
around it* ages via (b). Surface blocks (snow, ice, grass) that follow the
climate **materialize on visit** to match (b), through the dynamic-biome
block-tick system.

## Per-dimension relativity (time shift)

Different worlds can run at different rates. Each dimension has its own age:

```
dimension_age = rate · world_age
```

where `rate` is tied to the dimension's `gravity` in its `EnvDef` — a
black-hole-adjacent planet has a small `rate` and **runs slow**. Worldgen for a
dimension always reads *that dimension's* age.

The payoff: spend an hour on a slow, high-gravity world while your **home
planet's** clock races ahead. When you return home, its volcanoes, erosion, and
climate baseline have aged the right number of "years" — computed
deterministically on generate/load via the same `f(dimension_age)`. No
background simulation of the world you left; it is reconstructed, exact, on
arrival. This is fully consistent with the offline-freeze rule: only
`world_age` that elapsed (here, while you were on the slow world) feeds the
forward-evaluation.

## Worked example: the volcano on the ice world

A −18 °C ice world. You camp beside a warm, active [volcano](dynamic-environment.md),
then travel to a black-hole dimension and return after **~300 years** of
`world_age` have accumulated.

1. **The volcano (layer a):** `f(world_age − birth_age)` → its 5-year life ended
   ~295 years ago. It generates as **extinct** — cold rock, no heat input.
2. **The coarse climate (layer b):** with the volcano gone, the local
   equilibrium is the ambient **−18 °C**. Since `Δworld_age ≫ τ`, the saved warm
   offset relaxes **fully** to −18 °C — computed in closed form on load, not
   simulated.
3. **The surface:** materializes on visit to match (b) → rows of **snow and
   ice** where there had been bare warm ground.
4. **Your camp (layer c):** **persists**, exactly as you left it — now **buried
   in snow.**

The result reads as a real history — *"the volcano died and the land re-froze
over centuries"* — but it cost one function evaluation per layer, with no
tick-replay and no special-casing.

## Calendar & seasons

The in-world calendar (year / month / day) and the seasonal cycle advance with
`world_age` and are likewise **frozen offline**. They drive plant flowering,
fruiting, and dormancy — see [ecology](ecology.md).

## Generated history (future work, with a hard limit)

A Dwarf-Fortress-style *narrative* history — pre-`world_age` events,
civilizations, ruins, eroded landmarks — is a reserved future feature, with one
unavoidable limit: a simulated history can be pre-generated **only for the
initial worldgen footprint**. Chunks generated lazily on first visit **cannot
share** that simulated history — by design we never tick-replay ungenerated
space. They instead receive the deterministic forward-eval `f(seed, pos,
world_age)`, which *looks* aged (eroded, weathered) but is **not a recorded
narrative**. So: pre-generate a bounded history region; everything beyond it is
procedurally-aged, not historically-simulated.

## Why this shape

It keeps saves tiny (mostly functions, one timestamp per touched coarse cell),
makes multiplayer and singleplayer agree (everything is `f(seed, pos,
world_age)`), and never spends CPU simulating space no one is in — while still
letting a world you return to feel like time genuinely passed. It is the
[world-generation](../server/world-generation.md) philosophy — *pure functions
of `(seed, position)`* — extended with one more argument: `world_age`. See
[../../ARCHITECTURE.md](../../ARCHITECTURE.md) for the engine context.
