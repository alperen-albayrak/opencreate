# World Model

## Coordinates (§3)

- Block coordinates are **signed `i32` on all three axes**, world centered
  on 0,0,0, **sea level at Y = 0**. Default build range −512..+5120.
- `SectionPos` addresses 16³ sections (`block >> 4`, arithmetic shift so
  negatives floor correctly); `ChunkPos` addresses 16×16 vertical columns.
- Entity/player positions are **`f64`** (`DVec3`). Rendering converts to
  **camera-relative `f32`** per draw — the GPU never sees absolute world
  coordinates (floating origin; see [conventions](../conventions.md)).

## Storage (as built)

`oc_world::World` holds:
- `HashMap<SectionPos, Arc<Section>>` — only sections containing at least
  one non-air block exist; absence inside a generated column means air.
  `Arc` lets mesh jobs snapshot sections; edits go through `Arc::make_mut`
  (copy-on-write).
- `HashMap<ChunkPos, ColumnSpan>` — the inclusive vertical section range a
  generated column covers (grows if a player builds above it).
- A dirty set of edited columns for persistence.

A `Section` is currently a flat `Box<[BlockId; 4096]>` (u16 per voxel),
indexed `(y*16 + z)*16 + x`. Palette compression (§3's uniform/detailed
states) replaces the backing storage later without changing the API.

## Blocks

Hardcoded ids in `oc_world::blocks` until the data-driven block registry:
air 0, stone 1, dirt 2, grass 3, sand 4, water 5, log 6, leaves 7, lamp 8,
snow 9, planks 10. Properties live as methods on `BlockId`:

- `is_solid()` — collides, stops raycasts (everything but air and water)
- `is_opaque()` — hides adjacent faces in meshing (same set today)
- `light_opacity()` — light pass-through cost: air 1, water 1 (one per
  block, Java-1.13 rule), solids block
- `light_emission()` — lamp 15, everything else 0

## Light model

Classic voxel **sky light + block light, 4 bits each**, computed by BFS
flood fill (`oc_world::light`): column scan seeds sky light (level-15
travels down through air unattenuated), emissive blocks seed block light,
both propagate with per-block opacity costs. Light is **not stored** — it
is recomputed per mesh job over a 3×3-column snapshot, which is exact
because the max light range (15) is less than the 16-block margin. See
[client/engine/lighting.md](../client/engine/lighting.md).

## Reserved seams (not yet active)

Dimensions and grids: chunk storage will be keyed `(DimensionId, GridId,
ChunkPos)` so airship-style moving grids (§6.5) bolt on without a rewrite.
Entity positions gain a `FrameId`. Nothing in the current code assumes
"the one global grid" structurally, but the keys aren't threaded through
yet — they arrive with phase 6 groundwork.
