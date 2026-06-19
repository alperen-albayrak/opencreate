//! Tier-2 source heat (docs/world-building/temperature.md): a bounded flood-fill
//! of the temperature **delta** a heat source (lava now; fire/heated blocks
//! later) raises the surrounding cells above the geothermal base.
//!
//! Modelled on the light flood-fill ([`crate::light`]): a **pure function of the
//! blocks** over the same 48×H×48 region, with no stored state. The client
//! recomputes it when a column meshes (to bake the glow); the server recomputes
//! it to sample the player's ambient temperature. Because it is deterministic
//! over the blocks both sides already have, it needs no syncing — only tier-3
//! stored heat (genuine dynamic state) is server-authoritative and synced.
//!
//! The delta decays multiplicatively per block, attenuated by the neighbour's
//! `conductivity`: open air carries heat (radiation/convection), stone conducts
//! it, and insulators (wool/wood/snow/leaves) shield it. Combined with a base
//! distance falloff it bounds itself within ~12 blocks — the same cost class as
//! lamp light. `effective T = base(pos) + delta(pos)`.

use std::collections::VecDeque;

use glam::IVec3;
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE};

use crate::env_registry::EnvDef;
use crate::BlockId;

/// Width of the computed region in blocks: the centre column plus a 16-block
/// skirt on each side — enough margin for the bounded heat spread (≈12 blocks),
/// matching [`crate::light`].
const WIDTH: i32 = 3 * SECTION_SIZE;

/// Deltas below this (°C above the base) are dropped — what bounds the flood
/// (and below which the glow/hazard wouldn't notice anyway).
const MIN_DELTA: f32 = 4.0;

/// Per-block distance falloff: even through a perfect conductor the delta fades
/// (0.6/block ⇒ a 1200 °C source is below `MIN_DELTA` by ~12 blocks).
const STEP_DECAY: f32 = 0.6;

/// Conductivity (W/m·K) treated as a full conductor — stone. Solids normalise
/// against this; more conductive matter is clamped to 1.0.
const K_REFERENCE: f32 = 2.5;

/// Source-heat delta field for a 48×H×48 region around one chunk column.
pub struct HeatField {
    /// Minimum corner of the region in world space.
    base: BlockPos,
    height: i32,
    /// Per-cell temperature delta (°C) above the geothermal base.
    delta: Vec<f32>,
}

impl HeatField {
    /// Source-heat delta (°C above the geothermal base) at a world position;
    /// 0 outside the region or where no source reaches.
    pub fn delta(&self, pos: BlockPos) -> f32 {
        match self.index(pos) {
            Some(i) => self.delta[i],
            None => 0.0,
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

/// The temperature (°C) a block radiates as a heat source, if it is one. Lava
/// (a hot fluid with an intrinsic `temperature`) is the source today; fire and
/// player-heated blocks join later. Ordinary matter is not a source.
pub fn source_temp(block: BlockId) -> Option<f32> {
    crate::fluid_registry::for_block(block).and_then(|f| f.temperature)
}

/// Fraction of a cell's delta that crosses into `block` per block travelled
/// (combined with [`STEP_DECAY`]). Open air carries heat freely
/// (radiation/convection); a solid conducts it in proportion to its
/// conductivity (normalised to stone), so insulators shield.
fn transmit(block: BlockId) -> f32 {
    if block == crate::blocks::AIR {
        return 1.0;
    }
    (crate::registry::props(block).conductivity / K_REFERENCE).clamp(0.05, 1.0)
}

/// Computes the tier-2 source-heat delta for the 3×3 columns centred on
/// `center`. `sample` is queried once per voxel in `[min_y, max_y)`; `env`
/// supplies the geothermal base the seed delta is measured above.
pub fn compute_heat(
    sample: impl Fn(BlockPos) -> BlockId,
    env: &EnvDef,
    center: ChunkPos,
    min_y: i32,
    max_y: i32,
) -> HeatField {
    let base = IVec3::new(
        (center.x - 1) * SECTION_SIZE,
        min_y,
        (center.z - 1) * SECTION_SIZE,
    );
    let height = (max_y - min_y).max(1);
    let volume = (WIDTH * WIDTH * height) as usize;

    let mut blocks = Vec::with_capacity(volume);
    for y in 0..height {
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                blocks.push(sample(base + IVec3::new(x, y, z)));
            }
        }
    }

    compute_heat_in(&blocks, base, height, env)
}

/// Computes the source-heat delta over a **pre-sampled** block snapshot (laid
/// out `((y*WIDTH + z)*WIDTH + x)`, `WIDTH` wide — the same layout a
/// [`crate::light::LightField`] holds). Lets a caller reuse the snapshot the
/// light field already built for the same region instead of re-sampling the
/// whole column, which is what made a deep-world mesh job's heat pass cost
/// 100+ ms. Seeding + flood only — the expensive scan is shared.
pub fn compute_heat_in(blocks: &[BlockId], base: BlockPos, height: i32, env: &EnvDef) -> HeatField {
    let volume = (WIDTH * WIDTH * height) as usize;
    debug_assert_eq!(blocks.len(), volume, "heat snapshot must be WIDTH×WIDTH×height");
    let mut field = HeatField { base, height, delta: vec![0.0; volume] };

    // Seed: each source cell starts at its temperature above the local base.
    let w = WIDTH as usize;
    let layer = w * w;
    let mut queue: VecDeque<(usize, f32)> = VecDeque::new();
    for (i, &block) in blocks.iter().enumerate() {
        let Some(src) = source_temp(block) else { continue };
        let pos = base + IVec3::new((i % w) as i32, (i / layer) as i32, ((i / w) % w) as i32);
        let d = src - crate::temperature::base(pos, env);
        if d > MIN_DELTA && d > field.delta[i] {
            field.delta[i] = d;
            queue.push_back((i, d));
        }
    }

    // Flood: spread the delta, decaying per block and attenuated by what it
    // enters. Insulators choke it; air and stone carry it ~12 blocks.
    while let Some((i, level)) = queue.pop_front() {
        if level < field.delta[i] - 1e-3 {
            continue; // a hotter path already superseded this one
        }
        let x = i % w;
        let z = (i / w) % w;
        let y = i / layer;
        let neighbors = [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (z > 0).then(|| i - w),
            (z + 1 < w).then(|| i + w),
            (y > 0).then(|| i - layer),
            (y + 1 < height as usize).then(|| i + layer),
        ];
        for ni in neighbors.into_iter().flatten() {
            let next = level * STEP_DECAY * transmit(blocks[ni]);
            if next > MIN_DELTA && next > field.delta[ni] + 1e-3 {
                field.delta[ni] = next;
                queue.push_back((ni, next));
            }
        }
    }

    field
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;
    use crate::env_registry;
    use std::collections::HashMap;

    /// All-air world with `extra` blocks placed in it.
    fn world_with(extra: &[(BlockPos, BlockId)]) -> impl Fn(BlockPos) -> BlockId + '_ {
        let map: HashMap<BlockPos, BlockId> = extra.iter().copied().collect();
        move |pos| map.get(&pos).copied().unwrap_or(blocks::AIR)
    }

    fn field(extra: &[(BlockPos, BlockId)]) -> HeatField {
        compute_heat(
            world_with(extra),
            env_registry::overworld(),
            ChunkPos::new(0, 0),
            0,
            48,
        )
    }

    fn lava() -> BlockId {
        crate::registry::find_block("oc:lava").expect("oc:lava exists")
    }

    fn wood() -> BlockId {
        crate::registry::find_block("oc:planks").expect("oc:planks exists")
    }

    #[test]
    fn lava_radiates_a_falling_delta() {
        let f = field(&[(IVec3::new(8, 8, 8), lava())]);
        let at = |d: i32| f.delta(IVec3::new(8 + d, 8, 8));
        assert!(at(0) > 100.0, "the lava cell is very hot: {}", at(0));
        assert!(at(1) > 0.0 && at(1) < at(0), "one block out is cooler: {}", at(1));
        assert!(at(3) < at(1), "falls off with distance: {}", at(3));
        // Bounded: far enough out the delta drops to nothing.
        assert_eq!(at(15), 0.0, "heat does not reach 15 blocks: {}", at(15));
    }

    #[test]
    fn an_insulating_wall_shields() {
        // Lava at x=8, a wood wall at x=10, sample beyond it at x=12.
        let wall: Vec<(BlockPos, BlockId)> = (6..=10)
            .flat_map(|y| (6..=10).map(move |z| (IVec3::new(10, y, z), wood())))
            .collect();
        let mut blocks = vec![(IVec3::new(8, 8, 8), lava())];
        blocks.extend(wall);
        let f = field(&blocks);
        let before_wall = f.delta(IVec3::new(9, 8, 8));
        let behind_wall = f.delta(IVec3::new(12, 8, 8));
        assert!(before_wall > 0.0, "rock between lava and wall is heated");
        // Open air at the same distance for comparison (no wall).
        let open = field(&[(IVec3::new(8, 8, 8), lava())]).delta(IVec3::new(12, 8, 8));
        assert!(
            behind_wall < open * 0.5,
            "the insulating wall shields: behind={behind_wall} vs open={open}"
        );
    }

    #[test]
    fn no_source_means_no_delta() {
        let f = field(&[(IVec3::new(8, 8, 8), blocks::STONE)]);
        assert_eq!(f.delta(IVec3::new(8, 8, 8)), 0.0);
        assert_eq!(f.delta(IVec3::new(9, 8, 8)), 0.0);
    }
}
