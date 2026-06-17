# Disqualified — Matter Model & Fields

Field designs considered and rejected for the [matter model](../matter-model.md).
Shape: **Considered** → **Why we moved on** → **Instead**. The recurring lesson:
**one source of truth per physical quantity**; derive the rest.

## Separate `light_filter` and `absorption`

- **Considered** — a solid's `light_filter` (how it dims propagated light) and a
  medium's `absorption` (how it dims light through a volume) as two fields.
- **Why we moved on** — they are the **same physics**: light dying as it passes
  through matter. Two fields means two places to tune and two ways to disagree.
- **Instead** — one **per-channel `extinction`**, feeding both the RGB light
  flood-fill and volumetric rendering (water R:G:B ≈ 30:3:1). See
  [../matter-model.md](../matter-model.md).

## Separate `luminance` and `light_color`

- **Considered** — store a glow brightness (`luminance`) and a cast-light color
  (`light_color`) as independent fields.
- **Why we moved on** — they describe the same emission and **drift out of sync**;
  redundant with the emissive value.
- **Instead** — derive both from **`emissive`** (HDR RGB): hue = light color,
  brightness = reach. Store `emissive`; never store the derivations.

## Stored `weight_class`

- **Considered** — a discrete Create-style weight bucket (weightless / 0.25 / 0.5
  / 1 / 2 / super-heavy) stored per block.
- **Why we moved on** — a block is **unit volume**, so its mass *is* its
  `density`; a separately stored bucket can contradict the continuous value.
- **Instead** — continuous **`density`** for all matter; `weight_class` is a
  **derived display bucket**, not a stored field.

## `render_layer` as the source of truth

- **Considered** — author each block's render layer (opaque / cutout /
  translucent) directly.
- **Why we moved on** — the layer is really a **function of opacity/alpha**;
  authoring both invites a block whose declared layer mismatches its alpha.
- **Instead** — **`opacity`/alpha is the source of truth; `render_layer` is
  derived** (overridable for genuine special cases).

## Raw block ids in saves  *(correction — current format is v1)*

- **Considered / current** — store raw `u16` block ids in columns
  (`format_version: 1`, hardcoded 0 = air … 10 = planks).
- **Why we moved on** — a **data-driven registry makes numeric ids per-load**, so
  raw ids would corrupt the moment blocks are reordered or a mod adds one.
- **Instead** — **`format_version: 2`** with a **per-world string↔numeric
  palette**; stored cells remap on load via stable string ids (`oc:air`, …).
  v1→v2 is a **lossless** built-in migration. Lives in `oc-world/src/store.rs`.
  See [../matter-model.md](../matter-model.md).
