# The Deep World — Vertical Layers, Lava & the Hellish Deep

**Built (terrain, thermal curve, glow); heat hazard + tier-2 source heat
pending (G3/G6).** How the overworld is structured *downward*: a tall column of
rock that grows hot and hazardous with depth, opens into lava, and ends at an
impassable floor. It applies the "use nature's values" principle to depth — a
gentle, realistic gradient through the safe zone, **steepening into the molten
layer** near the lava (rock approaching a magma body really is near-molten), with
the **lava itself the hottest (~1200 °C)**.

## The vertical profile (overworld)

Sea level is `y = 0`; the survivable band tops out at **50 °C** (human
physiology — see [survival](../gameplay/survival.md) and the heat hazard). The
build range is `[−1024, +1024]`; **−768 … −1024 is reserved** for future
expansion. The curve is a long gentle cool descent to the 50 °C onset at −512,
then a steep ramp into the molten layer. Values are the active overworld
[`EnvDef.thermal`](../../data/dimensions/overworld.ron) profile:

| World Y | Layer | Feel |
|---|---|---|
| 0 … −512 | Ordinary rock & caves | Safe. Warms gently (24 → 50 °C, ~0.05 °C/block). Big deep caverns open from −352 but stay cool. |
| −512 | **Heat-hazard onset** | Ambient hits 50 °C — unprotected players start taking heat damage. |
| −512 … −560 | Hot rock | Steep ramp into the molten layer (50 → 525 °C); survivable only briefly without insulation. |
| −560 | **Draper point** | 525 °C — rock begins to glow dull-red by incandescence. |
| −560 … −656 | **Glowing hellish band** | 525 → ~1000 °C; rock glows dull-red → orange, brightening toward the lava. |
| −656 … −752 | **Lava lake** | Big lava-filled caverns (~1200 °C). Glowing, effectively instant death without protection. |
| −752 … floor | **Bedrock** | An **unbreakable** floor — survival players can never dig (or fall) below it. |

Numbers are tunable per dimension via `EnvDef.thermal` + worldgen — a volcanic or
young planet shifts the lava up; an airless/cold moon has no lava deep at all.

## Why the heat comes from lava, not the gradient

Real rock heats ~0.025 °C per metre — 64 m down, Earth is barely 16 °C. A
uniform gradient steep enough to glow within reach would be absurd. The honest
resolution is that the gradient is **not uniform**: it is gentle through the
crust and steepens sharply as you approach the molten layer — which is what
actually happens near a magma chamber.

- **Crust (0 … −512)**: a gentle ~0.05 °C/block rise to the 50 °C danger
  threshold — most of the depth is freely mineable. Static tier-1 base
  ([temperature.md](temperature.md)).
- **Molten layer (−512 … −656)**: the gradient steepens hard (rock nearing a
  magma body is genuinely near-molten), carrying the rock past the Draper point
  so it **glows dull-red → orange** by the blackbody model, brightening into the
  lava. Still tier-1 base — a per-vertex emissive, smooth and continuous.
- **Lava (~1200 °C)**: basaltic lava's real temperature — the hottest thing
  down there, lethal, glowing, and (once built) a **tier-2 source** radiating
  heat into the surrounding rock and cavern air.

So the dramatic glow and the lethal deep are *physically honest* — a realistic
crust gradient that steepens into a real molten layer, capped by real lava.

This also means the heat features are **dormant in a shallow world** and only
come alive once the world is deep enough to hold lava — which is correct.

## The hellish deep

The **big hostile caverns** above the lava (the air band from −352, carved by the
deep-cavern noise) are **built** — and from −560 their walls glow, so the descent
already has a difficulty ramp (cool caves → glowing molten layer → lava → wall)
rather than just "rock until lava." What's still **reserved** is the distinct
*content* — a nether/Terraria-underworld-style biome with its own blocks and
hostile creatures filling that layer. The layering leaves room for it.

## Temperature-status HUD (reserved)

A consumer of the effective-temperature field: a small status indicator —
**cold · normal · warm · hot · extreme** — each tier a distinct colour, so the
colour alone reads as danger. A later **gear/extension** upgrade can turn it
into a real readout: the **player's own** ambient temperature, and/or the
temperature of the **block/entity being looked at** (two separate values).
Because the tiers are colour-coded, showing a raw number still carries the
same at-a-glance meaning. Build with the HUD/thermometer work.

## Implementation notes

The build, in dependency order (see also the Phase-0 plan):

1. **Content** ✅ done: `oc:bedrock` (unbreakable — `hardness: -1`) and `oc:lava`
   (a hot, glowing `FluidDef`: high `light_emission`, blackbody `emissive`,
   ~1200 °C, lethal). See [matter-model.md](matter-model.md) / [fluids.md](fluids.md).
2. **Deepen the world** ✅ done (the keystone): the vertical range now runs to the
   `[−1024, +1024]` build limits; the bottom generated section (`BOTTOM_SECTION_Y`)
   sits at the bedrock floor (~−752/−768). Touched worldgen, column save size,
   light range, and performance — measured fine (deep gen stays under the ~5-min
   world-creation budget).
3. **Deep geology** ✅ done in worldgen: the bands (cool rock & caves → big hellish
   caverns from −352 → glowing molten layer → lava lake at −656 → bedrock floor),
   layered on the existing strata model ([geology.md](geology.md)) via
   `EnvDef.layers`.
4. **Activate the heat** — *partly done / current work.* The thermal curve + the
   per-vertex blackbody **glow are live**; remaining: the **tier-2 source delta**
   (lava radiating heat through rock, insulators shielding — G3) and the **player
   heat hazard** (G6, [survival](../gameplay/survival.md)). This also unblocks
   **phase transitions** (lava + water → obsidian/basalt; ice ↔ water ↔ steam),
   which need `oc:obsidian`/`oc:basalt`/`oc:ice` content too.

Cross-refs: [temperature.md](temperature.md) (the three-tier heat model),
[matter-model.md](matter-model.md) (cross-registry phase transitions),
[geology.md](geology.md) (strata), [fluids.md](fluids.md) (lava as a fluid).
