# Survival Simulation

Stats live in a `Stats` component on the player entity and tick at 30 TPS
via the pure function `oc_server::stats::tick` (fully unit-tested). All
values range 0..=10.

## The rules (per second)

| Stat | Rule |
|---|---|
| Oxygen | −1.0 while the eye block is water (10 s of air); +4.0 in air. At 0: −1.0 health/s drowning damage |
| Stamina | −1.4 while sprinting (~7 s of sprint); +1.8 at rest. The client blocks sprinting at 0 |
| Hunger | −10/1200 (a full belly lasts ~20 min); ×4 while sprinting. At 0: −0.5 health/s starvation |
| Health | +0.5/s regeneration while hunger ≥ 7 (and health > 0); capped at 10 |

**Death** (health ≤ 0): stats reset to full, the player teleports to the
world spawn, and the client gets `Respawn { position }`.

## Fall damage

`oc_server::falling::FallTracker` watches the player's reported Y each
tick: downward motion accumulates; when it stops, falls beyond **3 blocks**
deal `0.7 × (blocks − 3)` damage. Flying and water contact at any point
during the fall exempt it (a splash clears the accumulator). This is
client-report-based until phase 4's server-side movement reconciliation.

## Inputs

The client reports `sprinting` (fast + moving + grounded mode) and
`flying` in `PlayerState`; the server derives submersion by sampling the
world at eye height (feet + 1.62).

## Mode gating

`tick_stats` returns immediately for modes without `has_stats`
(creative, spectator) — no drain, no damage, no broadcasts.

## Broadcasting

Stats are sent at most every 8 ticks and only when a value moved by ≥0.05
(quantized comparison), so the channel stays quiet at steady state.
