# World Generation

Everything is a **pure, deterministic function of (seed, position)** —
unit-testable, multiplayer-consistent, and nothing needs persisting.
Implementation: `oc-world/src/terrain.rs` (a dedicated `oc-worldgen`
crate splits out when this grows). Randomness comes from splitmix-style
integer hashes; noise is hand-rolled value noise with fBm (2D for the
heightmap, 3D for caves), keeping the §13 noise-crate decision open.

## The column pipeline (`generate_column_data`)

For each 16×16 column (pure; runs on rayon workers):

1. **Heightmap** — two fBm scales: rolling hills (1/64 freq, ±18 blocks)
   plus a continental swell (1/512, ±40), offset +4.
2. **Rivers** — where a third noise channel (1/384) crosses zero
   (|n| < 0.045), the surface depresses smoothly to 3 blocks below sea
   level; the water fill then makes it a river. (Known cosmetic issue:
   1-wide uncarved slivers at the threshold edge — fix tracked.)
3. **Biome** — a temperature channel (1/640) splits desert (t > 0.32),
   snowy (t < −0.32), grassland (between).
4. **Block profile** per (x,z): top block grass/snow/sand by biome (sand
   everywhere within the beach band, surface ≤ 1), then 3 filler blocks
   (dirt or sand), stone below. Open terrain at y ≤ 0 fills with water.
5. **Caves** — vertically squashed 3D fBm (1/36 xz, 1/22 y) carves air
   where the value beats a threshold: 0.52 within 8 blocks of the surface
   (rare mouths), 0.34 deeper (proper caverns). Never carves the bottom
   section band or within 6 blocks of beach/ocean floors (no sea drains).
6. **Trees** — each column owns 0–2 hash-placed tree origins on grass;
   generation scans the 3×3 neighborhood's origins and writes any tree
   blocks (trunk 4–6 logs, blob canopy) that fall inside this column —
   the standard cross-chunk feature solution, columns stay independent.

The column's section span is sized by its max height (trees included);
generation bottoms out at section −4 (block −64) until deep worlds matter.

## Spawning the player

`find_spawn` ring-searches outward from the origin (8-block steps) for the
first surface above the beach band (`h > 1`) — the default seed has open
ocean at 0,0.

## Planned (§5)

Multi-noise biome table (continentalness/erosion/peaks/temp/humidity) with
data-driven biome defs, 3D density terrain for overhangs and big mountains
(+800..+1500 peaks), ores by depth, villages via two-phase placement, and
the active-area block tick (crops, fire, liquids) per §6.6.
