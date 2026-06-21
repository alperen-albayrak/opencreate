//! Flood-fill lighting (ARCHITECTURE.md §3): classic voxel sky light +
//! block light, 4 bits each.
//!
//! Milestone-2 design: light is a **pure function of the blocks**, computed
//! over a 3×3-column region when a column is meshed and baked into mesh
//! vertices. Light range is 15 and the region gives 16 blocks of margin, so
//! values inside the center column are exact. Nothing is stored in the
//! world; edits relight automatically when their sections remesh. Persistent
//! light storage arrives with the §6.6 active-area simulation, where light
//! must be queryable per tick (mob spawning, crops).

use std::collections::VecDeque;

use glam::IVec3;
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE};

use crate::BlockId;

/// Width of the computed region in blocks: center column plus a 16-block
/// skirt on each side, enough for the full 15-block light range.
const WIDTH: i32 = 3 * SECTION_SIZE;

pub const MAX_LIGHT: u8 = 15;

/// Computed light levels for a 48×H×48 region around one chunk column.
pub struct LightField {
    /// Minimum corner of the region in world space.
    base: BlockPos,
    height: i32,
    sky: Vec<u8>,
    block_r: Vec<u8>,
    block_g: Vec<u8>,
    block_b: Vec<u8>,
    blocks: Vec<BlockId>,
}

impl LightField {
    /// Packed light at a world position: `sky << 12 | r << 8 | g << 4 | b`
    /// (each nibble 0..=15). Above the region is full sky; below is darkness.
    pub fn get(&self, pos: BlockPos) -> u16 {
        match self.index(pos) {
            Some(i) => {
                (self.sky[i] as u16) << 12
                    | (self.block_r[i] as u16) << 8
                    | (self.block_g[i] as u16) << 4
                    | self.block_b[i] as u16
            }
            None if pos.y >= self.base.y + self.height => (MAX_LIGHT as u16) << 12,
            None => 0,
        }
    }

    fn index(&self, pos: BlockPos) -> Option<usize> {
        let rel = pos - self.base;
        let inside = rel.cmpge(IVec3::ZERO).all()
            && rel.x < WIDTH
            && rel.z < WIDTH
            && rel.y < self.height;
        inside.then(|| ((rel.y * WIDTH + rel.z) * WIDTH + rel.x) as usize)
    }

    /// The region's minimum corner, height, and cached block snapshot. The
    /// snapshot is laid out `((y*WIDTH + z)*WIDTH + x)` over a `WIDTH`-wide
    /// (3-column) region — the same layout the heat field uses — so a parallel
    /// [`crate::heat`] field can reuse this scan instead of re-sampling the
    /// whole column (the expensive part of a deep-world mesh job).
    pub fn base(&self) -> BlockPos {
        self.base
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn blocks(&self) -> &[BlockId] {
        &self.blocks
    }
}

/// How sky light is seeded into a freshly built field.
enum SkySeed<'a> {
    /// No skylight enters — a deep window with rock above it (only block light
    /// floods, so a deep edit needn't span up to the surface).
    None,
    /// Derive each column's heightmap (highest non-air block) from the window's
    /// own blocks — the classic full-column walk, exact when the window reaches
    /// the open sky.
    DeriveFromWindow,
    /// Seed from an external per-(x,z) heightmap: the world Y of the highest
    /// sky-blocking block (`i32::MIN` for a column open to the void). This lets a
    /// vertical *band* be lit without loading the column above it — the server
    /// ships the heightmap so the client need only hold the band.
    Heights(&'a dyn Fn(i32, i32) -> i32),
}

/// Computes light for the 3×3 columns centered on `center`. `sample` is
/// queried once per voxel between `min_y` (inclusive) and `max_y`
/// (exclusive). `sky_open` is whether `max_y` is the open sky (derive the
/// heightmap from the window and seed skylight) or a window ceiling deep
/// underground (no skylight enters).
pub fn compute_light(
    sample: impl Fn(BlockPos) -> BlockId,
    center: ChunkPos,
    min_y: i32,
    max_y: i32,
    sky_open: bool,
) -> LightField {
    let seed = if sky_open { SkySeed::DeriveFromWindow } else { SkySeed::None };
    compute_inner(sample, center, min_y, max_y, seed)
}

/// Computes light for a vertical *band* whose sky is seeded from a server-sent
/// heightmap rather than the window's own top — so a band can be lit correctly
/// without holding the column above it. `heights(x, z)` gives the world Y of the
/// highest sky-blocking block at a world column (`i32::MIN` = open to the void).
pub fn compute_light_banded(
    sample: impl Fn(BlockPos) -> BlockId,
    center: ChunkPos,
    min_y: i32,
    max_y: i32,
    heights: impl Fn(i32, i32) -> i32,
) -> LightField {
    compute_inner(sample, center, min_y, max_y, SkySeed::Heights(&heights))
}

fn compute_inner(
    sample: impl Fn(BlockPos) -> BlockId,
    center: ChunkPos,
    min_y: i32,
    max_y: i32,
    seed: SkySeed<'_>,
) -> LightField {
    let base = IVec3::new(
        (center.x - 1) * SECTION_SIZE,
        min_y,
        (center.z - 1) * SECTION_SIZE,
    );
    let height = (max_y - min_y).max(1);
    let volume = (WIDTH * WIDTH * height) as usize;

    let mut field = LightField {
        base,
        height,
        sky: vec![0; volume],
        block_r: vec![0; volume],
        block_g: vec![0; volume],
        block_b: vec![0; volume],
        blocks: Vec::with_capacity(volume),
    };
    for y in 0..height {
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                field.blocks.push(sample(base + IVec3::new(x, y, z)));
            }
        }
    }

    let mut queue: VecDeque<(usize, u8)> = VecDeque::new();

    // Sky seeding: a cell sees the open sky when it sits above its column's
    // highest sky-blocking block — the **heightmap**. Cells above it take full
    // sky; from there down the level attenuates per block (water dims a level
    // each; an opaque block stops it, the vertical-shaft rule keeps full-strength
    // sky through clear air). The heightmap source is either derived from the
    // window (full-column walk) or supplied by the server (band lighting).
    let w = WIDTH as usize;
    match seed {
        SkySeed::None => {}
        SkySeed::DeriveFromWindow => {
            // Highest non-air block per window column, as a world Y.
            let mut highest = vec![i32::MIN; w * w];
            for z in 0..w {
                for x in 0..w {
                    for y in (0..height).rev() {
                        if field.blocks[(y as usize * w + z) * w + x] != crate::blocks::AIR {
                            highest[z * w + x] = min_y + y;
                            break;
                        }
                    }
                }
            }
            seed_sky(&mut field, &mut queue, min_y, height, &|wx, wz| {
                highest[((wz - base.z) as usize) * w + (wx - base.x) as usize]
            });
        }
        SkySeed::Heights(h) => seed_sky(&mut field, &mut queue, min_y, height, h),
    }
    bfs(&mut field.sky, &field.blocks, &mut queue, height, true);

    // Block light: emissive blocks (lamps, lava) seed each channel at its tinted
    // level — hue from the block's emissive color, reach from its emission. Three
    // independent channel floods then propagate it (sources may be sparse or
    // dense). Geothermal incandescence is deliberately NOT seeded here: the hot
    // deep glows via the per-vertex emissive term in the geometry pass — smooth
    // and continuous in depth — not a quantized 4-bit block-light flood, which
    // would step at each integer level boundary and draw visible horizontal
    // banding across the deep rock.
    for (i, block) in field.blocks.iter().copied().enumerate() {
        let [r, g, b] = block.light_color();
        if r > 0 {
            field.block_r[i] = r;
        }
        if g > 0 {
            field.block_g[i] = g;
        }
        if b > 0 {
            field.block_b[i] = b;
        }
    }
    flood_channel(&mut field.block_r, &field.blocks, height);
    flood_channel(&mut field.block_g, &field.blocks, height);
    flood_channel(&mut field.block_b, &field.blocks, height);

    field
}

/// Seeds skylight from a per-(x,z) heightmap: cells above the heightmap take
/// full sky and queue; from the heightmap down the level attenuates per block.
/// A column whose heightmap sits at/above the window top gets no sky (it is
/// entirely underground); a column open to the void (`i32::MIN`) is full sky.
fn seed_sky(
    field: &mut LightField,
    queue: &mut VecDeque<(usize, u8)>,
    min_y: i32,
    height: i32,
    heights: &dyn Fn(i32, i32) -> i32,
) {
    let w = WIDTH as usize;
    for z in 0..w {
        for x in 0..w {
            let world_x = field.base.x + x as i32;
            let world_z = field.base.z + z as i32;
            let h = heights(world_x, world_z);
            if h >= min_y + height {
                continue; // whole window column is at/under the surface: no sky
            }
            let mut level = MAX_LIGHT;
            for y in (0..height).rev() {
                let i = (y as usize * w + z) * w + x;
                if min_y + y > h {
                    field.sky[i] = MAX_LIGHT; // open sky above the heightmap
                    queue.push_back((i, MAX_LIGHT));
                    continue;
                }
                match field.blocks[i].light_opacity() {
                    None => break,
                    Some(cost) => {
                        let air = field.blocks[i] == crate::blocks::AIR;
                        if !(air && level == MAX_LIGHT) {
                            level = level.saturating_sub(cost);
                        }
                        field.sky[i] = level;
                        if level > 1 {
                            queue.push_back((i, level));
                        }
                        if level == 0 {
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Seeds a block-light channel from its already-placed source levels and
/// floods it through transparent blocks (the same attenuation as the sky BFS,
/// without the vertical-shaft rule).
fn flood_channel(light: &mut [u8], blocks: &[BlockId], height: i32) {
    let mut queue: VecDeque<(usize, u8)> = VecDeque::new();
    for (i, &level) in light.iter().enumerate() {
        if level > 1 {
            queue.push_back((i, level));
        }
    }
    bfs(light, blocks, &mut queue, height, false);
}

/// Propagates queued light through transparent blocks, attenuating by each
/// block's opacity. `sky_rule`: level-15 light travels down unattenuated.
fn bfs(
    light: &mut [u8],
    blocks: &[BlockId],
    queue: &mut VecDeque<(usize, u8)>,
    height: i32,
    sky_rule: bool,
) {
    let w = WIDTH as usize;
    let layer = w * w;
    while let Some((i, level)) = queue.pop_front() {
        if light[i] != level {
            continue; // superseded by a brighter path
        }
        let x = i % w;
        let z = (i / w) % w;
        let y = i / layer;
        let neighbors = [
            (x > 0).then(|| (i - 1, false)),
            (x + 1 < w).then(|| (i + 1, false)),
            (z > 0).then(|| (i - w, false)),
            (z + 1 < w).then(|| (i + w, false)),
            (y > 0).then(|| (i - layer, true)),
            (y + 1 < height as usize).then(|| (i + layer, false)),
        ];
        for (ni, downward) in neighbors.into_iter().flatten() {
            let Some(cost) = blocks[ni].light_opacity() else {
                continue;
            };
            let air = blocks[ni] == crate::blocks::AIR;
            let next = if sky_rule && downward && level == MAX_LIGHT && air {
                MAX_LIGHT
            } else {
                level.saturating_sub(cost)
            };
            if next > light[ni] {
                light[ni] = next;
                if next > 1 {
                    queue.push_back((ni, next));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;
    use std::collections::HashMap;

    /// Flat stone floor at y=0 with optional extra blocks.
    fn world_with(extra: &[(BlockPos, BlockId)]) -> impl Fn(BlockPos) -> BlockId + '_ {
        let map: HashMap<BlockPos, BlockId> = extra.iter().copied().collect();
        move |pos| {
            if let Some(&b) = map.get(&pos) {
                b
            } else if pos.y <= 0 {
                blocks::STONE
            } else {
                blocks::AIR
            }
        }
    }

    fn field(extra: &[(BlockPos, BlockId)]) -> LightField {
        compute_light(world_with(extra), ChunkPos::new(0, 0), -16, 48, true)
    }

    fn sky(f: &LightField, pos: BlockPos) -> u8 {
        (f.get(pos) >> 12) as u8
    }

    /// Block-light brightness = the brightest channel (= the emission reach;
    /// the lamp's warm tint dims green/blue but red tracks the old value).
    fn blk(f: &LightField, pos: BlockPos) -> u8 {
        let l = f.get(pos);
        (((l >> 8) & 15).max((l >> 4) & 15).max(l & 15)) as u8
    }

    #[test]
    fn open_air_has_full_sky_light() {
        let f = field(&[]);
        assert_eq!(sky(&f, IVec3::new(8, 1, 8)), 15);
        assert_eq!(sky(&f, IVec3::new(8, 30, 8)), 15);
        // Above the region: still full sky.
        assert_eq!(sky(&f, IVec3::new(8, 1000, 8)), 15);
        // Inside the floor: opaque, no light.
        assert_eq!(sky(&f, IVec3::new(8, 0, 8)), 0);
    }

    #[test]
    fn light_creeps_under_an_overhang() {
        // Roof at y=4 over x in 4..=12, open to the west of x=4.
        let roof: Vec<(BlockPos, BlockId)> = (4..=12)
            .flat_map(|x| (0..=16).map(move |z| (IVec3::new(x, 4, z), blocks::STONE)))
            .collect();
        let f = field(&roof);
        // Under the open edge: nearly full; deeper under: progressively dimmer.
        let at = |x| sky(&f, IVec3::new(x, 2, 8));
        assert_eq!(at(3), 15, "open sky beside the roof");
        assert!(at(5) >= 12, "just under the edge: {}", at(5));
        assert!(at(8) < at(5), "deeper is darker");
        // Mid-roof at (8, 8): nearest opening is 5 blocks away -> 15 - 5.
        assert_eq!(at(8), 10, "mid-roof light should drop with distance");
    }

    #[test]
    fn water_dims_sky_light_with_depth() {
        // Water pool from y=1..=5 at one spot (column of water).
        let pool: Vec<(BlockPos, BlockId)> = (1..=5)
            .flat_map(|y| {
                (4..=12).flat_map(move |x| (4..=12).map(move |z| (IVec3::new(x, y, z), blocks::WATER)))
            })
            .collect();
        let f = field(&pool);
        let top = sky(&f, IVec3::new(8, 5, 8));
        let bottom = sky(&f, IVec3::new(8, 1, 8));
        assert!(top < 15, "water surface attenuates: {top}");
        assert!(bottom < top, "deeper water is darker: {bottom} vs {top}");
    }

    #[test]
    fn lamp_emits_block_light_gradient() {
        let f = field(&[(IVec3::new(8, 3, 8), blocks::LAMP)]);
        assert_eq!(blk(&f, IVec3::new(8, 3, 8)), 15);
        assert_eq!(blk(&f, IVec3::new(9, 3, 8)), 14);
        assert_eq!(blk(&f, IVec3::new(12, 3, 8)), 11);
        // Manhattan distance decay reaches zero past 15 blocks.
        assert_eq!(blk(&f, IVec3::new(8 + 16, 3, 8)), 0);
        // Sky light is unaffected by the lamp.
        assert_eq!(sky(&f, IVec3::new(9, 3, 8)), 15);
    }

    /// The banded path, fed the heightmap the full-column path would derive,
    /// must produce identical sky light — proof the server-sent heightmap can
    /// replace the top-down window walk without changing the result.
    #[test]
    fn banded_with_derived_heightmap_matches_full_column() {
        // Terrain: stone floor at y<=0, a hill of stone to y=6 over a patch, a
        // lamp, and a water pool — a mix of opaque, emissive, and translucent.
        let mut extra: Vec<(BlockPos, BlockId)> = Vec::new();
        for x in 6..=10 {
            for z in 6..=10 {
                for y in 1..=6 {
                    extra.push((IVec3::new(x, y, z), blocks::STONE));
                }
            }
        }
        for y in 1..=3 {
            extra.push((IVec3::new(2, y, 2), blocks::WATER));
        }
        let sample = world_with(&extra);

        let full = compute_light(&sample, ChunkPos::new(0, 0), -16, 48, true);
        // Highest non-air block per world column (what the window would derive).
        let heights = |wx: i32, wz: i32| -> i32 {
            for y in (-16..48).rev() {
                if sample(IVec3::new(wx, y, wz)) != blocks::AIR {
                    return y;
                }
            }
            i32::MIN
        };
        let banded = compute_light_banded(&sample, ChunkPos::new(0, 0), -16, 48, heights);

        for y in -16..48 {
            for z in 0..16 {
                for x in 0..16 {
                    let p = IVec3::new(x, y, z);
                    assert_eq!(
                        full.get(p) >> 12,
                        banded.get(p) >> 12,
                        "sky differs at {p}"
                    );
                }
            }
        }
    }

    /// A band that starts below the surface, fed a heightmap that sits above the
    /// window, gets no skylight — the deep-band-is-dark case the streamer relies
    /// on (sky comes only from columns whose heightmap is within/below the band).
    #[test]
    fn banded_below_a_high_heightmap_is_dark() {
        let sample = world_with(&[]); // open flat world, floor at y<=0
        // Pretend the surface is far above this band (y=200) everywhere.
        let banded =
            compute_light_banded(&sample, ChunkPos::new(0, 0), -64, -16, |_, _| 200);
        for y in -64..-16 {
            assert_eq!(sky(&banded, IVec3::new(8, y, 8)), 0, "deep band has no sky at y={y}");
        }
    }

    #[test]
    fn sealed_cave_is_dark_until_lit() {
        // Box: floor y=0 (world), walls and roof sealing 6..10 on x/z at y 1..4.
        let mut blocks_list = Vec::new();
        for x in 5..=11 {
            for z in 5..=11 {
                blocks_list.push((IVec3::new(x, 5, z), blocks::STONE)); // roof
                for y in 1..=4 {
                    let edge = x == 5 || x == 11 || z == 5 || z == 11;
                    if edge {
                        blocks_list.push((IVec3::new(x, y, z), blocks::STONE));
                    }
                }
            }
        }
        let dark = field(&blocks_list);
        assert_eq!(sky(&dark, IVec3::new(8, 2, 8)), 0, "sealed cave has no sky light");
        assert_eq!(blk(&dark, IVec3::new(8, 2, 8)), 0);

        blocks_list.push((IVec3::new(8, 1, 8), blocks::LAMP));
        let lit = field(&blocks_list);
        assert!(blk(&lit, IVec3::new(8, 2, 8)) >= 13, "lamp lights the cave");
    }
}
