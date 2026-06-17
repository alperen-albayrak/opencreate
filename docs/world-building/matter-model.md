# The Matter Model

**Design, mostly not yet built.** Blocks today are hardcoded in
`oc-world/src/blocks.rs` (ids 0–10, properties as methods). This page describes
the registry-driven model that replaces them, unifying the three states of
matter so the same field means the same thing everywhere — and so a substance
can move *between* states (lava → stone) without special-casing.

## Three states, three registries

| State | Registry | Stored in | Examples |
|---|---|---|---|
| Solid | `BlockDef` | `data/blocks.ron` | stone, planks, ore, ice, glass, lamp |
| Liquid | `FluidDef` | `data/fluids.ron` | water, lava, oil, milk, mud, blood |
| Gas | `GasDef` | (composition field) | O₂, CO₂, N₂, steam, helium, methane |

Solids are voxels. Liquids and gases are **volumetric simulated quantities**,
not voxels-only (a tank of water or a balloon of hot air is an *amount*, not a
grid of blocks). See [Runtime contexts](#runtime-contexts) below and the
[fluids](fluids.md) / [atmosphere](atmosphere.md) pages.

A fourth registry, **`EnvDef`** (`data/dimensions/*.ron`), is per dimension /
planet: `gravity`, `atmosphere_composition` (default gas mixture + total
pressure), `atmosphere` (Rayleigh/Mie sky params — see [rendering](rendering.md)),
celestial bodies, and an optional `thermal` profile (see [temperature](temperature.md)).
*Reserved design goal (Factorio Space Age):* planets should be **mechanically
distinct** — differing in available resources and mechanics, not just gravity /
atmosphere / sky reskins — so `EnvDef` must stay expressive enough to gate
content per world.

## The block registry (the keystone)

`BlockDef` is the Phase-0 blocker that unblocks everything else — without it,
features force `match block { … }` hacks. The schema is grouped by concern, all
`#[serde(default)]`, so a block declares only what differs from the default:

- **Render/material** — per-face texture ids, `normal`/`MER(S)` ids,
  `roughness`, `metalness`, `emissive` (HDR RGB), `subsurface`, `render_layer`,
  `map_color`.
- **Light** — cast light comes from `emissive`; attenuation is the shared
  `extinction`. The baked light field is 3-channel RGB.
- **Gameplay (schema now, behavior later)** — `hardness`, `blast_resistance`,
  `harvest_tool` + `harvest_tier`, `drops`, `sound_set`, `flammability`,
  `replaceable`.
- **Physics/sim** — `friction`, `gravity` (falling), `collision_shape` ≠
  `outline_shape`, `random_tick`, plus the Create-physics traits
  (`weight_class`, `floating`, `bouncy`, `sticky`, `slippery`, `fragile`, glue).
- **Thermal** — `heat_capacity`, `conductivity`, `melting_point`,
  `resistivity`, `ignitable` (see [temperature](temperature.md)).
- **States** — a `BlockState` (facing, waterlogged, age/growth, `temperature`):
  the numeric id encodes variants.

### Break-time model *(gameplay — reserved)*

The `hardness` / `harvest_*` fields feed a standard Minecraft-style break-time
formula, **stored as data now** and consumed when the survival dig system is
built:

```
break_time = hardness × (1.5 if correct tool else 5) / tool_speed
```

Tool-tier speeds: hand 1, wood 2, stone 4, iron 6, diamond 8, **gold 12**,
netherite 9. `hardness = −1` ⇒ **unbreakable**; **instant break** when the tool's
damage-per-tick exceeds `hardness × 30`; `requires_correct_tool` gates whether
the block drops at all. Tiers (`harvest_tier`) run wood → stone → iron → diamond →
netherite.

### Save migration (not a hack)

Columns store raw `u16` ids today (`format_version: 1`, hardcoded 0–10). A
registry makes numeric ids per-load, so the persistence (`oc-world/src/store.rs`)
bumps to **`format_version: 2`** with a **per-world block palette** (a
string↔numeric table in the level header). Stored cells use per-world ids
remapped on load via the **stable string ids** (`oc:air`, `oc:planks`, …), so
reorders/mods never corrupt saves. v1→v2 is lossless via a built-in legacy map.

## Shared trait fragments

Registries *opt into* fragments rather than duplicating fields. A fragment
applies only to the states where it makes physical sense:

| Fragment | Fields | Solids | Liquids | Gases |
|---|---|---|---|---|
| optical-surface | `roughness`, `metalness`, `normal`/height | ✓ | surfaces only | ✗ |
| volumetric-medium | `extinction`, `opacity`, `fog_color`/`fog_distance`, `ior` | transparent only | ✓ | ✓ |
| subsurface (SSS) | `subsurface` | thin (leaves) | ✗ | ✗ |
| emissive + thermal | `emissive`, `temperature`, `heat_capacity`, `conductivity`, melt/boil/ignite | ✓ | ✓ | ✓ |
| respiration | `breathability`, `oxygen_content` | ✗ | ✓ | ✓ |
| mass | continuous `density` (`weight_class` derived) | ✓ | ✓ | ✓ |
| mechanical / mining | friction, sticky, hardness, drops, states, redstone | ✓ | ✗ | ✗ |

## Canonical fields & merges

Light is **per-channel RGB end-to-end** — the unifying principle. A lit surface
= `albedo_rgb × incoming_light_rgb`, so a blue object under red light reads dark.
Several fields that used to be separate are merged because they are really one
thing:

- **`emissive`** (HDR RGB, blackbody-drivable) is the **single source** for a
  matter's glow *and* its cast light: hue = light color, brightness = reach.
  `luminance`/`light_color` are **derived**, never stored.
- **`extinction`** (per-channel RGB) is the **one** attenuation field — merges a
  solid's old `light_filter` and a medium's `absorption`. Feeds both the RGB
  light flood-fill and volumetric rendering. Water R:G:B ≈ **30:3:1**.
- **`opacity`/alpha is the source of truth; `render_layer` is derived**
  (opaque / cutout / translucent), overridable for special cases.
- **`density`** is continuous for all matter; the Create-style discrete
  **`weight_class`** is a *derived display bucket*, not stored.
- One name each: **`ambient_floor`**, **`fog_color`** + **`fog_distance`**,
  **`ior`**, **`harvest_tool`** + **`harvest_tier`**, **`collision_shape`** +
  **`outline_shape`**.
- Kept **separate** (different things): `breathability` (breathe directly) vs
  `oxygen_content` (gear-extractable); `friction` (solid surface) vs `viscosity`
  (fluid drag); EnvDef `atmosphere_composition` (the gas mixture) vs `atmosphere`
  (sky scattering params).

## Runtime contexts

A `FluidDef`/`GasDef` is the *substance*; at runtime it exists in one of three
forms, each stored, simulated, and rendered differently:

| Context | States | Storage | Sim | Render |
|---|---|---|---|---|
| Voxel cell | solids, liquids | id + fill in the grid | block tick / liquid-flow queue (§6.6) | solid / water pass |
| Contained quantity | liquids, gases | an amount + id in a tank/envelope | fill/transfer/leak + (gas) buoyancy | container UI / balloon shader |
| Global atmosphere | gases | none — `EnvDef.atmosphere_composition` | altitude-pressure pure function | sky/fog tint |

**The rule every query uses:** *effective matter at a point = the voxel fluid/
gas if present, else the global atmosphere.* It drives breathing, buoyancy, and
the temperature `effective T`. Costs: voxel = bounded queue, contained = one
number, global = a free pure function. Open-air drifting gas **clouds** (toxic/
steam) are the only per-cell-expensive case → reserved, approximated as a region.

## Cross-registry phase transitions

The payoff of unifying the registries: a **`phase_transition`** field on the
shared thermal trait names a matter's melt/freeze/boil product as a
**registry-ref into *any* registry**.

- `lava` (FluidDef) →[freeze]→ obsidian/basalt (BlockDef)
- `ice` (BlockDef) ↔ `water` (FluidDef) ↔ `steam` (GasDef) — the *same field*
  across all three states.

That cross-registry pointer **is** "matter moving between registries," and it is
why the three registries share one thermal trait. The swap is **event-driven**
(fired only when a cell's effective T crosses the point near a reason — lava
meets water/air, a heat source removed), never a global scan; a **latent-heat
plateau** makes it linger at the threshold. The **quench-rate nuance** falls out
for free and is real physics: fast-quenched lava (contacts water) freezes to a
glass → **obsidian**; slow-cooled lava crystallizes to **stone/basalt** — exactly
Minecraft's lava+water → obsidian-vs-cobblestone. See [temperature](temperature.md).

## See also

- [temperature.md](temperature.md) — the thermal trait and effective-temperature model.
- [fluids.md](fluids.md) / [atmosphere.md](atmosphere.md) — the liquid and gas registries.
- [../../ARCHITECTURE.md](../../ARCHITECTURE.md) — the §6.5 block-network and §6.6 active-area seams these reserve hooks for.
