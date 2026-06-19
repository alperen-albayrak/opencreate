# The Deep World — Vertical Layers, Lava & the Hellish Deep

**Design, not yet built.** How the overworld is structured *downward*: a tall
column of rock that grows hot and hazardous with depth, opens into lava, and
ends at an impassable floor. It resolves the realism-vs-spectacle tension in
the heat model ([temperature.md](temperature.md)) cleanly: the **rock** uses a
gentle, near-realistic geothermal gradient, while the **glow and lethal heat
come from real lava (~1200 °C)** — not from faking a steep gradient. This is
the "use nature's values" principle applied to depth.

## The vertical profile (overworld)

Sea level is `y = 0`; the survivable band tops out at **50 °C** (human
physiology — see [survival](../gameplay/survival.md) and the heat hazard).
The geothermal gradient is tuned to **0.18 °C/block** so that band is reached
~200 blocks down — the rock above is freely mineable.

| Depth (below sea level) | Layer | Feel |
|---|---|---|
| 0 … ~200 | Ordinary rock / caves | Safe. Warms gently with depth (14 °C → ~50 °C). |
| ~200 | **Heat-hazard onset** | Ambient hits 50 °C — unprotected players start taking heat damage. |
| ~200 … ~250 | Hot rock | Increasingly dangerous; survivable only briefly without insulation. |
| ~250 … ~300 | **Lava + stone transition** | Lava pockets/veins appear among the stone; intense local heat + glow. |
| ~300 … ~350 | **Lava sea** | Mostly/entirely lava (~1200 °C). Glowing, effectively instant death without protection. |
| ~350 … floor | **Bedrock** | An **unbreakable** floor — survival players can never dig (or fall) below it. |

Numbers are targets, tunable per dimension via `EnvDef.thermal` +
worldgen — a volcanic or young planet shifts the lava up; an airless/cold
moon has no lava deep at all.

## Why the heat comes from lava, not the gradient

Real rock heats ~0.025 °C per metre — 64 m down, Earth is barely 16 °C. A
"deep rock glows" effect within a few hundred blocks would require a wildly
unrealistic gradient. Instead:

- **Rock**: a gentle gradient (0.18 °C/block) gives a believable "it gets
  warmer as you go down," reaching the human-danger threshold (50 °C) at a
  depth worth gating (~200). This is the static tier-1 base
  ([temperature.md](temperature.md)).
- **Lava**: a genuine ~1200 °C heat source (basaltic lava's real temperature).
  Its heat radiates as the **tier-2 source delta** (the bounded flood-fill),
  and it **glows** by the blackbody model (it's well past the Draper point).
  So the dramatic glow and the lethal deep are *physically honest* — they're
  what real molten rock does.

This also means the heat features are **dormant in a shallow world** and only
come alive once the world is deep enough to hold lava — which is correct.

## The hellish deep (future)

Before the pure lava sea, the design reserves a **hellish layer** — a
nether/Terraria-underworld-style region of hostile caverns, its own
biome/blocks/creatures, sitting above the molten floor. It gives the descent a
destination and a difficulty ramp (hostile cavern → lava → wall) rather than
just "rock until lava." Reserved as a later dimension/biome; the layering here
leaves room for it.

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

1. **Content** (data, low risk): `oc:bedrock` (unbreakable — `hardness: -1`)
   and `oc:lava` (a hot, glowing `FluidDef`: high `light_emission`, blackbody
   `emissive`, ~1200 °C, lethal; a tier-2 heat source). See
   [matter-model.md](matter-model.md) / [fluids.md](fluids.md).
2. **Deepen the world** (the keystone, higher risk): extend the vertical range
   (`BOTTOM_SECTION_Y`) from −64 to roughly −360 so the layers fit. Touches
   worldgen, column save size, light range, and performance — do it carefully.
3. **Deep geology** in worldgen: the bands above (rock → hot rock → lava+stone
   → lava sea → bedrock), layered on the existing strata model
   ([geology.md](geology.md)).
4. **Activate the heat**: lava as a tier-2 source lights up the existing glow
   ([rendering.md](rendering.md)) and the player heat hazard
   ([survival](../gameplay/survival.md)) where they belong. This also unblocks
   **phase transitions** (lava + water → obsidian/basalt; ice ↔ water ↔ steam),
   which need `oc:obsidian`/`oc:basalt`/`oc:ice` content too.

Cross-refs: [temperature.md](temperature.md) (the three-tier heat model),
[matter-model.md](matter-model.md) (cross-registry phase transitions),
[geology.md](geology.md) (strata), [fluids.md](fluids.md) (lava as a fluid).
