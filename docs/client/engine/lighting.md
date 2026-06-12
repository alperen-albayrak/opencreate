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
  sideways/down with opacity costs (air 1, water 3, leaves and solids
  block). Caves are dark, overhangs shade smoothly, water dims with depth.
- **Block light**: emissive blocks (lamp = 15) seed the same BFS without
  the shaft rule.
- Both packed per face into the vertex light byte (`sky:4 | block:4`).

## Shading (chunk.wgsl)

`brightness = max(sky_level × (ambient + sun_diffuse), block_level × 0.95)`

The sun vector and ambient come from push constants, driven by the
day/night cycle (`oc-client/src/sky.rs`: 10-minute days, warm dusk band,
moonlit nights — sun direction is pre-scaled by daylight so night kills
the diffuse term). Lamps therefore glow constantly while sky light fades
with the sun.

## Edits

Re-meshing an edited column recomputes its light field; the 8 neighbor
columns re-mesh asynchronously since an edit's light reaches up to 15
blocks. Persistent light storage becomes necessary only when the §6.6
active-area simulation needs per-tick light queries (mob spawning, crops).
