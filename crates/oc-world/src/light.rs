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
}

/// Computes light for the 3×3 columns centered on `center`. `sample` is
/// queried once per voxel between `min_y` (inclusive) and `max_y`
/// (exclusive, the open sky above the tallest content).
pub fn compute_light(
    sample: impl Fn(BlockPos) -> BlockId,
    center: ChunkPos,
    min_y: i32,
    max_y: i32,
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

    // Sky seeding: walk each column down from the open sky. Level-15 light
    // passes through air without attenuation (the vertical shaft rule).
    let layer = (WIDTH * WIDTH) as usize;
    for z in 0..WIDTH {
        for x in 0..WIDTH {
            let mut level = MAX_LIGHT;
            for y in (0..height).rev() {
                let i = (y * WIDTH + z) * WIDTH + x;
                let i = i as usize;
                match field.blocks[i].light_opacity() {
                    None => break,
                    Some(cost) => {
                        // The free vertical shaft is air-only: water still
                        // dims a level per block on the way down.
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
    bfs(&mut field.sky, &field.blocks, &mut queue, height, true);

    // Block light: emissive blocks seed each channel at its tinted level (hue
    // from the block's emissive color, reach from its emission). Plus
    // geothermal blackbody light — any cell hot enough to glow (past the
    // Draper point, a pure function of depth) casts warm light, so the hot
    // deep is dimly ember-lit rather than pitch black. Three independent
    // channel floods then propagate it (sources may be sparse or dense).
    let env = crate::env_registry::active();
    let w = WIDTH as usize;
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

        let pos = base + IVec3::new((i % w) as i32, (i / layer) as i32, ((i / w) % w) as i32);
        let temp = crate::temperature::base(pos, env);
        if temp > crate::temperature::DRAPER_C {
            let level = (((temp - crate::temperature::DRAPER_C) / 40.0) as i32)
                .clamp(0, MAX_LIGHT as i32) as u8;
            field.block_r[i] = field.block_r[i].max(level);
            field.block_g[i] = field.block_g[i].max((level as f32 * 0.45) as u8);
            field.block_b[i] = field.block_b[i].max((level as f32 * 0.12) as u8);
        }
    }
    flood_channel(&mut field.block_r, &field.blocks, height);
    flood_channel(&mut field.block_g, &field.blocks, height);
    flood_channel(&mut field.block_b, &field.blocks, height);

    field
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
        compute_light(world_with(extra), ChunkPos::new(0, 0), -16, 48)
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
