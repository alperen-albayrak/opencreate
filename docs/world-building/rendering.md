# Rendering: the Deferred-PBR Roadmap

The plan to reach **feature parity or better with modern voxel renderers** with a
modular, physically-grounded renderer. This page **supersedes and expands** the
"Graphics roadmap — deferred-PBR rendering" section in [../roadmap.md](../roadmap.md):
the roadmap there records what has *shipped* on the current forward renderer;
this page is the forward **design** — the deferred-PBR architecture and the
staged path to it. Most of it is **not built yet**; shipped pieces are marked.

**Performance target: the M5 16GB Air (quality/correctness first).** The M1 is a
dedicated *later* performance phase — old hardware does **not** constrain the
architecture. Each feature still ships behind a setting so a future low tier can
scale it down without redesign.

## Architecture — deferred (G-buffer) PBR, done tile-friendly

The opaque world renders its **material attributes into a G-buffer**; a
**lighting pass** then computes per-pixel PBR for the sun/moon + ambient +
(later) many dynamic lights. This is the deferred-PBR path and unlocks its payoff:
per-pixel material lighting, cheap many-light scaling, and SSAO/SSR straight
from the G-buffer. Engineered tile-friendly (the right way to do deferred, and
what makes the eventual M1 pass cheap):

- **Thin, packed G-buffer** (no fat targets, no stored world position):
  - `GB0` RGBA8 — albedo.rgb + AO/flags
  - `GB1` — **octahedral-encoded normal** + roughness + metalness
  - **emissive** folded straight into the HDR target (no extra attachment)
  - **world position reconstructed from depth** (already sampleable)
- **Tile-memory subpasses** — the G-buffer is a Vulkan subpass with **input
  attachments**, so on a tiled GPU (Apple/MoltenVK → Metal tile shading) it
  lives in on-chip tile memory and is **never written to main memory**.
- **Hybrid lighting** — the existing **per-vertex baked sky/block flood-fill
  stays** and feeds the lighting pass as the ambient/indirect term; we keep that
  near-free GI-ish light *and* gain deferred direct lighting.
- **Transparency stays forward** (water, cutout leaves, sky, clouds), composited
  after the lighting resolve — exactly as modern voxel renderers do.

## Lighting — the baked-vs-dynamic contract

Split light by how often it changes: **bake what's static, compute what's
dynamic.** See [../client/engine/lighting.md](../client/engine/lighting.md) for
today's flood-fill.

- **Baked** (per-vertex field, re-baked only on edits):
  - **`sky_visibility`** — *scalar* 0–15: how much open sky reaches a cell.
    Store **visibility, not a final color** (sky color is time-varying).
  - **`block_light`** — **RGB**: colored flood-fill seeded by emissive blocks.
- **Dynamic** (per-pixel, each frame):
  - **sky tint** = `sky_visibility × sky_color(time)` → **day/night is a uniform
    change, no re-bake** (the key trick).
  - **sun + moon** directional `× N·L × shadow`, **point lights** (clustered),
    and the **`ambient_floor`**.
- **Compose:** `L = albedo·(AO·(ambient_floor + sky_vis·sky_color + block_light)
  + sun + moon + Σpoints) + emissive`. AO occludes only the indirect terms;
  `emissive` is added (self-glow → bloom).

Light is **per-channel RGB end-to-end**: lit color = `albedo_rgb ×
incoming_light_rgb`, so a blue object under red light reads dark. A monochrome
bake survives as the cheap **low tier**. See [matter model](matter-model.md) for
the `emissive`/`extinction` fields this consumes.

## Physical constants (the grounding)

`physical.rs` will be the single source of these, so the whole world stays
calibrated:

| Constant | Value | Drives |
|---|---|---|
| Water absorption | R:G:B ≈ **30:3:1** | underwater color by depth (see [fluids](fluids.md)) |
| Water IOR | **1.333** → Fresnel F0 = 0.02 | water reflectance |
| Snell window | **48.6°** | "up transparent / down mirror" underwater |
| Rayleigh β | ∝ **1/λ⁴** | blue sky, red sunset |
| Mie phase | Henyey–Greenstein **g ≈ 0.76** | sun glow, fog shafts |
| Exposure key | **0.18** | photographic middle grey |
| Draper point | ≈ **798 K** | blackbody emissive onset (see [temperature](temperature.md)) |

## Natural phenomena (modelled from nature, ≥ modern voxel renderers)

Sun: **airmass** horizon-reddening (optical path ∝ 1/cos θ) + limb darkening.
Moon: phase-correct lit fraction, **earthshine**, and a **Purkinje** night shift
(scotopic blue-shift) modern voxel renderers don't fully do. Stars: **apparent magnitude →
brightness** + **B–V index → blackbody color** (hot = blue, cool = red). Sky:
Rayleigh + Mie with **aerial perspective**. Foliage: leaf/grass **subsurface
scattering** (backlit leaves glow). Fluids: per-fluid **Beer–Lambert** (see
[fluids](fluids.md)). Emissive: **blackbody color from temperature** for
lava/fire/heated metal (see [temperature](temperature.md)).

## Feature roadmap (staged by re-port cost)

Ship the architecture-independent wins first, do the deferred rewrite **once**,
then build every lighting/material feature a single time on deferred.

- **Stage 0 — ship now (forward, zero re-port):**
  - **Step 1 ✅ shipped** — additive colored lighting + base-ambient floor.
  - **Step 2** — fluid/underwater **Beer–Lambert (30:3:1)** + Snell window 48.6°.
  - **Step 3** — exposure/tonemap polish (center-weighted metering, key → 0.18).
- **Stage 1 — foundations ✅ shipped:** the data-driven **block registry** (the
  keystone — see [matter model](matter-model.md)) → `physical.rs` → Scene UBO →
  the **deferred G-buffer scaffold + RGB light-field bake** → the data registries
  (`FluidDef`/`GasDef`/`EnvDef`). The big rewrite, done once.
- **Stage 2 — lighting, built once on deferred:** RGB-light consumption ✅;
  directional **sun/moon + shadows ✅ shipped** (cascades fixed + restyled — see
  the shadow bug story in [../roadmap.md](../roadmap.md) §D); **atmosphere &
  volumetrics ✅ shipped** (per-pixel `sky_vis` cave fog + raymarched Rayleigh+Mie
  god-rays / ground mist, per-dimension); stars by magnitude + spectral color
  (later).
- **Stage 3 — materials (NEXT):** **Cook–Torrance + Fresnel–Schlick** PBR
  (roughness+metal packed into the free `GB1.w`); per-texel **normal + MER(S)**
  (deferred); leaf/grass SSS; **many clustered dynamic lights**.
- **Stage 4 — beyond-parity + perf:** IBL reflections, per-biome color grading,
  TAAU, foliage wind; then the **M1 performance phase** (MDI/pooled draws, LOD,
  GPU culling, tier downgrades) — *after* quality lands, never constraining it.

## Research notes — shadows & atmosphere (shipped)

Findings from researching modern voxel deferred renderers + standard real-time
graphics references, recorded so the decisions aren't re-litigated.

**Atmosphere & volumetrics.** Modern voxel deferred renderers ship *true*
volumetric fog + light shafts as a documented, configurable system — it is **not**
exclusive to ray-traced pipelines: a per-medium (air/water) volume with explicit
**scattering/absorption coefficients** and a **height-density** falloff, plus an
analytic sky model with named **Rayleigh + Mie** terms using the **Henyey-Greenstein**
phase (forward-biased, g≈0.75 air). So our raymarched god-ray pass is faithful in
spirit. We drive it off **real Rayleigh + Mie single-scattering**, not a hand-tuned
isotropic floor:
- **Rayleigh** — phase `3/(16π)·(1+cos²θ)` (near-isotropic) with a blue-biased
  coefficient → the **broad haze visible in every view direction** (the principled
  replacement for a constant "floor"; the colour is the coefficient, not a tint).
- **Mie** — Henyey-Greenstein forward lobe → the **sun-aligned god-ray shafts**.
- **Beer–Lambert** transmittance along the ray; per-dimension coefficients scaled
  to block (not km) distances by a `fog_density` knob; height ramp pools mist low.
- Caves stay dark for free — the sun shadow cascades occlude the air.

**Shadows (cascaded, deferred).** Standard CSM is the right approach; the voxel
twist is **shell-only greedy meshes** (a flat top is one up-facing quad with no
underside), so front-face culling of casters has nothing to record → caster cull
is **NONE**, and acne is beaten by **grazing-scaled normal-offset bias** + a
**low-sun fade**, not by depth bias alone (which diverges at grazing angles). The
"broken shadows" that shelved the feature were **three concrete bugs**, not the
technique: (1) the depth-only caster bound the 12-byte packed vertex at an 8-byte
stride (scrambled caster geometry); (2) the orthographic depth axis was inverted,
so the nearest occluder lost the `LESS` depth test and nothing cast; (3) the
cascade was selected by view-space depth, dropping wide-angle screen-edge pixels
that sit outside the near cascade's box — fixed by selecting the first cascade
whose box actually contains the point. Both edge styles shipped: soft PCF
(default) vs blocky (1/16-block-snapped) edges, and a sky-tinted ambient fill so
shadowed surfaces read cool-blue and never pitch black.

## Quality tiers (presets over toggles)

Tiers are **presets that set bundles of individual settings** — each feature
reads its own toggle, so any combination stays valid ("High but shadows off").
Targets: **Low → M1** · **Medium → mid** · **High → M5 16GB Air** (primary) ·
**Ultra → RTX 5070 Ti / 32 GB** (ceiling). They scale resolution/AA, render
distance, light propagation (mono → RGB), shadows, sky/atmosphere, water, PBR
materials, AO (baked → SSAO/GTAO), and dynamic-light count.

**Always-on, every tier** (cheap, not perf-sensitive): auto-exposure, bloom,
`ambient_floor`, base absorption, the brightness slider.

## Already shipped (current forward renderer)

From [../roadmap.md](../roadmap.md): HDR target + sampleable depth + **ACES
tonemap**, **dual-Kawase bloom**, **auto-exposure**, **SSR water** + per-channel
**Beer–Lambert** absorption + caustics + underwater camera, **far-terrain LOD**
ring, **moon phases** + a real **bright-star catalog**, per-vertex **ambient
occlusion**, and Step 1's additive colored lighting + ambient floor.

**Shelved:** cascaded sun shadows — built (3×2048 texel-snapped cascades) but
the look never convinced (smudgy at distance, seams while moving); the code sits
dormant awaiting a crisper **voxel-aware** redesign, to be revived in Stage 2.
