# Disqualified — Rendering

Approaches considered and rejected for the [renderer](../rendering.md). Shape:
**Considered** → **Why we moved on** → **Instead**.

## Pure-forward rendering for the opaque path

- **Considered** — keep the existing forward renderer for the whole opaque world,
  just adding PBR and more lights to it.
- **Why we moved on** — forward can't cheaply scale to many dynamic lights (every
  light re-shades every fragment), and SSAO / SSR / screen-space effects are
  awkward without material attributes available in screen space.
- **Instead** — a **deferred (G-buffer) PBR** path for opaque geometry; clustered
  many-light, SSAO and SSR fall out of the G-buffer. *Transparency (water,
  cutout leaves, sky) stays **forward** — that is correct in a deferred renderer
  too, not a concession.* See [../rendering.md](../rendering.md).

## Fat G-buffer

- **Considered** — store world position and full material attributes in wide
  render targets for the lighting pass to read.
- **Why we moved on** — fat targets waste bandwidth and **defeat tile-memory
  subpasses** on tiled GPUs (MoltenVK→Metal), which is exactly what keeps
  deferred cheap.
- **Instead** — a **thin, packed G-buffer** (octahedral-encoded normal, packed
  roughness/metalness, emissive folded into HDR) and **reconstruct world position
  from depth**, which we already keep sampleable.

## Baked final sky color in the light field

- **Considered** — bake the sky's *contribution as a color* into the per-vertex
  light field.
- **Why we moved on** — sky color is **time-varying** (day → dusk → night), so a
  baked color would force a full re-bake every time tick — the expensive thing we
  are trying to avoid.
- **Instead** — bake **sky *visibility*** (a scalar 0–15) and multiply by
  `sky_color(time)` in the lighting pass. Day/night becomes a **uniform change,
  no re-bake**.

## Monochrome-only light

- **Considered** — a single 0–15 light scalar (vanilla-style) as *the* lighting
  model.
- **Why we moved on** — it can't represent **colored light** (a red lamp casting
  red, a blue object reading dark under red light), which is central to the
  intended look.
- **Instead** — **per-channel RGB light end-to-end**. Monochrome survives only as
  the cheap **Low tier** downgrade, never the design. See [../rendering.md](../rendering.md).

## Cascaded shadow maps  *(not this, not now)*

- **Considered / built** — 3×2048 texel-snapped cascaded shadow maps, with
  comparison sampling, per-cascade bias, cross-fade, and twilight fade.
- **Why we moved on** — they read **smudgy at distance** with **seams while
  moving**; the look never convinced in playtests. The code sits dormant, forced
  off.
- **Instead (planned)** — **voxel-aware shadows**: per-block sun visibility baked
  like AO, or ray-stepped voxel shadows. This is **not a permanent veto** — it's
  "not shadow maps, not yet." See [../../roadmap.md](../../roadmap.md).
