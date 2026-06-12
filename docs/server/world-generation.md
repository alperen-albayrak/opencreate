# World Generation

Everything is a **pure, deterministic function of (seed, position)** —
unit-testable, multiplayer-consistent, and nothing needs persisting.
Implementation: `oc-world/src/terrain.rs` (a dedicated `oc-worldgen`
crate splits out when this grows). Randomness comes from splitmix-style
integer hashes; noise is hand-rolled value noise with fBm (2D for climate
and relief, 3D for caves), keeping the §13 noise-crate decision open.

The design follows **Minecraft 1.18+'s multi-noise architecture** (researched
from the vanilla density-function data): climate noises pick both the terrain
shape (through splines) and the biome (through a parameter table), so biomes
and terrain always agree and biome borders never produce seams.

## Climate channels (`TerrainGenerator::climate`)

Five 2D noise channels per column, all sampled through a shared domain warp
(±26 blocks, wavelength 128) so coastlines and biome borders wander together:

| Channel | Wavelength | Octaves | Drives |
|---|---|---|---|
| Continentalness | 1400 | 6 | ocean ↔ inland, coastlines, land-height scaling |
| Erosion | 1000 | 4 | flat ↔ mountainous, detail amplitude |
| Weirdness | 440 | 3 | rivers + ridges via the PV fold |
| Temperature | 2200 | 3 | biome bands (snowy → desert) |
| Humidity | 800 | 3 | biome bands (plains → forest/taiga) |

**Peaks & valleys** is folded out of weirdness exactly as in vanilla:
`PV = 1 − |3|w| − 2|`. Valleys (PV ≈ −1) sit on the zero-crossing band of
weirdness — a level-set of a smooth field, which is why **rivers are long
connected winding lines for free** (same trick spaghetti caves use in 3D).
Peaks sit at |w| ≈ ⅔.

## Terrain shaping (nested splines)

`surface_height` evaluates vanilla-style nested splines (piecewise
smoothstep, zero slope at knots — monotone, no overshoot):

1. **Inland height** = spline over erosion of splines over PV
   (`LAND_TABLE`, 6×6 knots): low erosion → mountain country (PV crest
   +160), high erosion → flats (+2..6). Valley columns (PV −1) go a few
   blocks *negative* at most erosion levels, so **rivers carve
   themselves**; the lowest-erosion row stays positive (no rivers through
   extreme mountains, as in vanilla).
2. **Continentalness spline** carries fixed ocean knots (deep ocean −42,
   ocean shelf −20..−12) and scales the inland value up from the coast
   (0.25× at the shore → 1.15× far inland) — coastal cliffs appear by
   themselves wherever the inland value is mountainous.
3. **Jaggedness** — high-frequency noise (wavelength 28, mostly additive:
   negative lobes quartered) ramped in only on inland, low-erosion ridge
   tops; up to ~±42 blocks of rocky relief on summits.
4. **Detail** — local relief (wavelength 56), amplitude grown by low
   erosion and altitude, **damped to zero near valley centerlines** so
   rivers stay connected.

## Biomes (13)

`biome_for` resolves the climate tuple + surface height:
ocean bands by continentalness (DeepOcean < −0.45 < Ocean < −0.19);
River where PV < −0.85 and the surface is underwater; Beach/StonyShore on
low dry coast land (stony where erosion < −0.375); altitude zoning with
hash-dithered lines (> ~72 StonyPeaks — SnowyPeaks if cold, > ~92 always
SnowyPeaks); then a temperature × humidity "middle biomes" table:
SnowyPlains/SnowyTaiga (cold), Plains/Taiga, Plains/Forest (temperate),
Desert (hot).

## Surface rules (`block_in_column`)

Vanilla-style repaint of the top of the column: submerged floors are sand
in the shallows (surface ≥ −10) and bare stone deeper; **steep slopes
(≥ 8 blocks across the 2-block central difference) expose stone whatever
the biome**; deserts/beaches cap with sand, snowy biomes with snow, peaks
with stone/snow-over-stone, the grass family with grass + 3 dirt. Open
terrain at y ≤ 0 fills with water.

## Caves (two systems, MC 1.18 style)

- **Cheese caverns** — vertically squashed 3D fBm (1/36 xz, 1/22 y) carves
  where the value beats a threshold: 0.52 within 8 blocks of the surface
  (rare mouths), 0.34 deeper (proper caverns).
- **Spaghetti tunnels** — the neighborhood of the intersection of two
  independent 3D noise zero-surfaces (1/110 xz, 1/52 y):
  `max(|s1|, |s2|) < t` with t widening 0.05 → 0.078 over depth 5..24.
  Two thin sheets crossing make winding 1-D tunnels that connect the
  caverns over long distances.

Neither system carves the bottom section band or within 6 blocks of
beach/ocean/river floors (no sea drains).

## Trees

Each chunk hashes out 4 candidate slots; each rolls against a biome
density (Forest 48/64, Taiga 36/64, SnowyTaiga 20/64, Plains 3/64) and
grows only on non-steep grass below the treeline (y 78). Generation scans
the 3×3 neighborhood's origins and writes any tree blocks that fall inside
this column — the standard cross-chunk feature solution.

The column's section span is sized by its max height (trees included);
generation bottoms out at section −4 (block −64) until deep worlds matter.

## Visualizing

`cargo run -p oc-world --release --example mapgen [seed]` renders
`map.ppm` (top-down 4096² biome/hillshade map) and `section.ppm`
(underground cross-section with caves) — the fastest way to eyeball a
worldgen change without launching the game.

## Spawning the player

`find_spawn` ring-searches outward from the origin (8-block steps, up to
2048 blocks) for the first surface above the beach band (`h > 1`).

## Planned (§5)

Data-driven biome defs (RON, mod-extensible like everything else), 3D
density terrain for overhangs, aquifers (local water tables in caves),
ores by depth, badlands-style strata, villages via two-phase placement,
and the active-area block tick (crops, fire, liquids) per §6.6.
