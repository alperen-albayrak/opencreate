# Roadmap

Six phases, each independently shippable (§12). Strikethrough = done.

## ~~Phase 1 — Engine bring-up~~ ✅
Window + Vulkan clear → textured test chunk → fly camera. MoltenVK
pitfalls solved here (see [conventions.md](conventions.md) for the ones
that bit).

## ~~Phase 2 — World prototype~~ ✅
Chunk pipeline, terrain with biomes/rivers/trees/caves, walking/collision,
place/break, save/load, day cycle, lighting, greedy meshing, debug HUD.
The first "it's a game" build.

## Phase 3 — Survival core (current)
Done: client/server split, stats, inventory, crafting, game modes,
passive creatures, multi-noise worldgen, villages
(two-phase placement: region-hashed centers, per-chunk hash-placed
houses with lit interiors), food & eating (apples from leaves, data-driven
`food:` values).
Remaining:
- ~~**Inventory screen**~~ ✅ — E opens a panel with 36 server-authoritative
  per-slot stacks (a configurable 9-slot hotbar + 27 main), a 3×3 crafting
  grid with a result slot, and a cursor for moving/splitting/merging stacks.
  Items move via `InventoryClick`; the server resyncs the whole inventory.
  Per-player inventories over the wire join the multiplayer protocol work.
- **Ocean creatures** — fish with water movement AI
- ~~**Sound**~~ ✅ — fully synthesized at startup (zero audio assets in
  the repo): per-surface footsteps with speed-following cadence, dig
  crunch, place thud, eat, splash, menu clicks, and looping wind/
  underwater ambience that fades with altitude and submersion. Master
  volume slider in settings. Spatialized remote-entity sounds join
  multiplayer.
- Local skin + texture pack selection (overlay stack). *Started:* blocky
  six-part player body with walk swing, visible in the F5 third-person
  views (back/front, wall-aware camera); color-set skins in
  data/skins.ron. Image skins + a settings picker arrive with texture
  packs.

## Graphics roadmap — deferred-PBR rendering (runs alongside phases 3–5)

> **The full forward design now lives in [world-building/rendering.md](world-building/rendering.md)** —
> a deferred-PBR architecture targeting parity with modern voxel renderers, part of the broader
> [world-building/](world-building/README.md) design set (matter model,
> temperature, atmosphere, ecology, time). The notes below record what has
> **shipped** so far against the original forward-renderer plan.

Researched against modern voxel renderers (deferred-PBR upgrades and the
shaderpack playbook); adapted to our forward Vulkan renderer. Five
stages, each independently shippable, every effect behind a graphics
setting with a cheap tier so the M1/60 fps budget holds.

**A. HDR foundation + Graphics settings tab** *(prerequisite)*
Offscreen HDR target (+ sampleable depth) and a final tonemap pass
(ACES + dither); UI renders at native resolution afterwards. Unlocks:
**resolution scale** (render at 0.5–2.0×, UI stays sharp), **max FPS
cap**, and a tabbed settings screen — *Game* / *Graphics* (resolution
scale, fps cap, render distance, plus every toggle below as it lands).

**B. Water v2** *(top priority)*
Own translucent pass reading the opaque color + depth snapshot:
scrolling procedural wave normals (world-space, seam-free), Schlick
fresnel, sky reflection + high-exponent sun glint, screen-space
refraction with the depth guard, per-channel Beer–Lambert depth
absorption (shallow turquoise → deep blue), soft shorelines via depth
fade. Water setting: low (flat) / normal / high; ultra adds SSR later.
*Shipped so far:* texel-quantized ripple living only in the sun glint,
Beer–Lambert absorption, crisp waterline, Voronoi-web caustics in the
style of the official sprite sheet (procedural, our own), and an
underwater camera mode — dense blue fog, blue sky dome, clouds off.

**C. Sky & atmosphere**
One reusable `sky(direction)` function feeding the sky pass, water
reflections and fog. **Celestials**: sun disc with glow and sunset
band; **moon with phases**, opposite the sun; procedural star field
that fades in at dusk; **real-world constellations** — embed a small
bright-star catalog (RA/Dec of the ~300 brightest stars) projected on
the rotating sky dome, so Orion, the Big Dipper and Cassiopeia appear
where they belong. **Fog**: exponential height + distance fog colored
by `sky()` (terrain dissolves into atmosphere; permanently hides chunk
pop-in). **Clouds**: scrolling blocky noise layer with an on/off
toggle; 2.5D raymarched slab as a later "volumetric" tier.
*Shipped so far:* sun disc + glow, horizon fog, blocky cloud layer
(depth-prepass so only outer silhouettes read as edges), directional
dusk (the anti-sun horizon darkens first), moon with 8 phases and a
halo, procedural star field plus a real bright-star catalog (Orion,
Big Dipper, Cassiopeia, Southern Cross... ~46 stars from RA/Dec)
rotating with the day, **aerial perspective** (distant terrain dissolves
into the sky colour in its view direction — warm toward a low sun, cool
away), and **physical airmass sun-reddening** (the disc reddens and dims
through Rayleigh extinction as it sets, ∝1/λ⁴ — no hand tint). The sky
dome itself stays the artist gradient (a uniform clear-sky look a
from-scratch single-scatter dome fought); a full analytic atmosphere
(Preetham/Hosek) is a possible later upgrade. *Later polish:* per-direction fog color,
per-cloud dusk tinting, sidereal drift; cloud shadows (modern voxel renderers have them,
players ask for a toggle — if ever added, OFF-able from day one;
cheap analytically since the cloud pattern is a pure function). **Far terrain LOD** *(shipped, v2 — blocky)*:
a colored ring generated from the seed on a worker thread (256-block
tiles), built the way Minecraft's LOD mods (Voxy, Distant Horizons —
approach studied, no code) keep distance blocky: 4-block cells render
as flat-topped columns at quantized heights with vertical stair-step
walls (tops bright, sides 0.72), run-length merged; seas flatten to a
sky-leaning fresnel sheet so the ring continues the real water. Drawn
after the chunks (depth keeps detail on top) and discarded inside the
loaded square; fog saturates near the ring's edge (~970 blocks). Far
terrain toggle in graphics settings. *Later:* multiple rings with
halving resolution, far trees, server-driven tiles for MP.

**D. Lighting**
Per-vertex ambient occlusion (corner darkening; AO joins the greedy
merge key, diagonal flip against anisotropy) — the look that makes
blocks read as solid. *Shipped:* classic side1/side2/corner AO baked
per vertex (2 bits in word 0), merges only along AO-constant axes,
brighter-diagonal split. *Shipped (deferred path):* cascaded sun shadows —
3×2048 texel-snapped cascades, PCF comparison sampling, grazing-scaled
normal-offset bias, twilight + low-sun fade, blocky (default) / soft-PCF
styles, a sky-tinted ambient fill (shadows read cool-blue, never black, and
vanish in caves), and a settings toggle. The earlier "never convinced" was
**three real bugs, not the approach** (found via research + adversarial
debugging): the depth-only caster bound the 12-byte packed vertex at an 8-byte
stride (scrambled caster geometry — the phantom triangular acne); the
orthographic depth axis was inverted, so the occluder lost the `LESS` depth test
and nothing ever cast; and the cascade was picked by view-space depth, so
wide-angle screen-edge pixels fell outside the near cascade's box and dropped
their shadow. All fixed, with a regression test on occluder clip-z ordering.
*Shipped (deferred path):* **Cook–Torrance/GGX specular + a cheap sky-reflection
IBL** — roughness/metalness packed into the one free G-buffer channel (`GB1.w`,
an 8-bit metal-bit + roughness code), so ice/obsidian catch a sun glint and a
sky-blue sheen while matte blocks stay unchanged. Now **per-texel**: a linear
**MER** map (metalness/roughness) and a **normal map** (RGB + heightfield in
alpha), procedurally derived from each texture's grain (overridable by
`_mer.png`/`_n.png`/`_h.png` packs) — the geometry pass perturbs the normal into
`GB1.xy` and does **parallax occlusion mapping** for real surface relief and
depth (metallic ore flecks, recessed cobble mortar), all in the existing
G-buffer. *Shipped:* **dynamic point lights** — emissive blocks (torches, lava,
lamps) cast smooth coloured light + GGX specular, derived client-side from the
loaded sections and added over the baked block-light flood-fill (which supplies
the wall-respecting taxicab *diamond* shape); the dynamic glow smooths it. Per-
texel emissive/subsurface (needs a 4th target), per-light shadows, and froxel
clustering remain later steps.

**E. Post & polish**
Bloom (downsample chain — the sun halo), auto-exposure (dark caves,
dazzling exits), AA options (off / FXAA / TAA + sharpen). *Shipped:*
dual-Kawase bloom pyramid (half-res, up to 6 levels, soft-knee
threshold at HDR 1.0, +0.35 mix before ACES); auto-exposure (16x16
log-luminance grid, CPU readback two frames later, geometric mean,
eased at 1.8/s, clamped 0.55-2.4). Ultra tier:
volumetric clouds, SSAO, per-biome color grading. *Shipped:*
SSR water — the opaque color snapshots between passes and top water
faces march the depth buffer (16 geometric steps, fresnel-gated,
screen-edge fade, sky fallback); Water reflections toggle in settings.
*Shipped:* **volumetric god-rays + ground mist** — a raymarched fullscreen pass
after the deferred lighting resolve (depth sampled, additive blend): per-view-ray
single scattering sampled against the sun cascades — Rayleigh (broad blue haze,
all view directions) + Mie/Henyey-Greenstein (forward shafts toward the sun),
driven by per-dimension scattering coefficients, with a height-density mist ramp
and Beer–Lambert transmittance; caves stay dark (the cascades occlude the air);
Volumetric fog toggle in settings. PBR texture channels (normal/roughness/emissive
per texture) join with texture packs in §7.5.

## Phase 4 — Multiplayer
`postcard` serialization + QUIC (`quinn`) behind the existing `Transport`
trait; dedicated headless server binary; prediction/reconciliation
hardening; interest management for many players; skin/pack distribution
via asset sync. The protocol boundary already exists, so this phase is
mostly transport + polish.

## Phase 5 — Depth & modding
LOD far terrain, GPU occlusion culling, combat/hostile mobs, more
structures, audio; the `./mods/` loader ships content-mod support (data +
assets merged into the registry) and the WASM behavior-mod API (§7.6).

## Phase 6 — Physics & machines
`rapier3d` + Sable-style voxel octrees → physics assembler → airships;
then block networks (rotational power, electric). Ordered after
multiplayer because moving grids must be designed against replication.

## Engine debts (no phase, do when they hurt)
- Palette-compressed sections (storage is flat u16 per voxel today)
- Pooled vertex buffers + multi-draw-indirect (per-chunk binds today)
- Region files replacing the folder store (same trait)
