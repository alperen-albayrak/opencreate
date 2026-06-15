# Survival Rules

The authoritative numbers (all server-enforced; see
[server/simulation.md](../server/simulation.md) for implementation).

## Stats (0..=10, bars above the hotbar)

- **Health** (red) — damaged by drowning, starvation and falls;
  regenerates +0.5/s while hunger ≥ 7. At 0 you die: full stats, teleport
  to world spawn.
- **Hunger** (orange) — a full belly lasts ~20 minutes; sprinting burns
  it 4× faster. Empty: −0.5 health/s.

## Food

Breaking leaves has a 1-in-3 (position-hashed) chance of dropping an
**apple** alongside the leaves; **G eats one** (+3 hunger, capped at 10
— a full belly refuses food). The HUD shows your apple count above the
stat bars whenever you carry any. Food is data: any item with a `food:`
value in `items.ron` is edible, so mods add foods by adding items.
- **Stamina** (green) — ~7 s of sprint; refills in ~6 s of rest. At 0,
  sprinting stops until it recovers.
- **Oxygen** (blue, shown only underwater) — 10 s of air; refills in
  ~2.5 s at the surface. Empty: −1 health/s.

## Falls

Drops beyond 3 blocks hurt: 0.7 damage per extra block. Landing in water
(even a splash mid-fall) or flying cancels it.

## Water

Swimming is buoyant-drag: you sink slowly (terminal 3.5 b/s), Space swims
up (4.5 b/s), movement is ×0.55, and you can hop out at the surface.
Watch the blue bar.

## The economy

Breaking a block puts its item in your inventory (1:1; no tools or drop
tables yet — leaves drop leaves, plus the occasional apple). Placing
consumes one. The hotbar dims
what you don't have and prints counts for what you do. The server
validates everything; a desynced client gets snapped back by the echo.
