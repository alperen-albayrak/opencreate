# Lighting

Design choice: **light is a pure function of the blocks**, computed when a
column is meshed — nothing stored, nothing to invalidate. Implementation:
`oc-world/src/light.rs` (engine-agnostic), consumed by mesh jobs.

## Why it's exact

`compute_light` BFS-floods a 48×48×H region (the meshed column plus a
16-block skirt on each side). Light attenuates at least 1 per block and
maxes at 15, so nothing outside the skirt can influence the center column:
values there are **exactly** what a global solver would produce. This
argument breaks if light range ever exceeds 16 — see
[conventions](../../conventions.md).

## Model

- **Sky light**: per-column scan from the open sky; level 15 passes down
  through air unattenuated (the vertical shaft rule), then BFS spreads
  sideways/down with opacity costs (air 1, water 1 — one per block,
  Java-1.13 style — leaves and solids
  block). Caves are dark, overhangs shade smoothly, water dims with depth.
- **Block light**: emissive blocks (lamp = 15) seed the same BFS without
  the shaft rule.
- Both packed per face into the vertex light byte (`sky:4 | block:4`).

## Shading (chunk.wgsl)

Each light term is scaled by the corner's ambient-occlusion multiplier
`ao_mul = 0.66 + 0.34·ao/3` (ao 0..3, from word 0), and the sky light is
split into ambient and sun-diffuse so a shadow term can darken just the
diffuse half:

`shade = ao_mul × max(sky × (ambient + (1−ambient)·diffuse·visibility), block × 0.95)`

`visibility` is the sun-shadow term — 1 today, since cascaded shadows are
shelved ([roadmap.md](../../roadmap.md)). The result is written to the HDR
target, then bloomed, auto-exposed and ACES-tonemapped.

The sun vector and ambient come from push constants, driven by the
day/night cycle (`oc-client/src/sky.rs`: 30-minute days, length owned by
`oc_server::DAY_LENGTH_SECS`; warm dusk band,
moonlit nights — sun direction is pre-scaled by daylight so night kills
the diffuse term). Lamps therefore glow constantly while sky light fades
with the sun.

## Edits

Re-meshing an edited column recomputes its light field; the 8 neighbor
columns re-mesh asynchronously since an edit's light reaches up to 15
blocks. Persistent light storage becomes necessary only when the §6.6
active-area simulation needs per-tick light queries (mob spawning, crops).
