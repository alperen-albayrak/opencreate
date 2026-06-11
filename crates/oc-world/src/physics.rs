//! AABB-vs-voxel character collision — the baseline behind the §6.5 physics
//! module boundary (`rapier3d` + octrees replace the internals in phase 6).
//!
//! Movement resolves axis by axis (Y first, then X, then Z) against solid
//! voxels, the classic Minecraft scheme: simple, stable, and tunneling-free
//! at survival movement speeds.

use glam::DVec3;
use oc_core::BlockPos;

use crate::World;

/// Gap left between the box and a surface it collides with, so faces never
/// sit exactly coplanar (which makes float comparisons flip-flop).
const SKIN: f64 = 1e-4;

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    /// Box standing on `feet` (bottom-center).
    pub fn standing(feet: DVec3, half_width: f64, height: f64) -> Self {
        Self {
            min: DVec3::new(feet.x - half_width, feet.y, feet.z - half_width),
            max: DVec3::new(feet.x + half_width, feet.y + height, feet.z + half_width),
        }
    }

    pub fn translated(self, d: DVec3) -> Self {
        Self { min: self.min + d, max: self.max + d }
    }

    pub fn intersects_block(&self, pos: BlockPos) -> bool {
        let bmin = pos.as_dvec3();
        let bmax = bmin + DVec3::ONE;
        self.min.cmplt(bmax).all() && self.max.cmpgt(bmin).all()
    }
}

/// Outcome of a collision-resolved move.
#[derive(Debug, Clone, Copy)]
pub struct MoveResult {
    /// Movement actually applied (≤ requested on each axis).
    pub delta: DVec3,
    /// True when the requested downward motion was stopped by ground.
    pub on_ground: bool,
    /// Per-axis: was movement clamped by a collision?
    pub hit: [bool; 3],
}

/// Moves `aabb` by `delta` through `world`, clamping against solid voxels.
pub fn move_aabb(world: &World, aabb: Aabb, delta: DVec3) -> MoveResult {
    let mut cur = aabb;
    let mut applied = DVec3::ZERO;
    let mut hit = [false; 3];

    // Y first so floors/ceilings resolve before walls grab sideways motion.
    for axis in [1usize, 0, 2] {
        let wanted = delta[axis];
        let allowed = sweep_axis(world, &cur, axis, wanted);
        let mut step = DVec3::ZERO;
        step[axis] = allowed;
        cur = cur.translated(step);
        applied[axis] = allowed;
        hit[axis] = allowed != wanted;
    }

    MoveResult {
        delta: applied,
        on_ground: delta.y < 0.0 && hit[1],
        hit,
    }
}

/// Largest movement along `axis` (signed, |result| ≤ |d|) before the box
/// touches a solid voxel.
fn sweep_axis(world: &World, aabb: &Aabb, axis: usize, d: f64) -> f64 {
    if d == 0.0 {
        return 0.0;
    }
    let (a1, a2) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    // Voxel range the box covers on the two fixed axes, shrunk by the skin
    // so resting flush against a neighboring wall doesn't count as overlap.
    let lo1 = (aabb.min[a1] + SKIN).floor() as i32;
    let hi1 = (aabb.max[a1] - SKIN).floor() as i32;
    let lo2 = (aabb.min[a2] + SKIN).floor() as i32;
    let hi2 = (aabb.max[a2] - SKIN).floor() as i32;

    let solid_layer = |v: i32| {
        for c1 in lo1..=hi1 {
            for c2 in lo2..=hi2 {
                let mut pos = BlockPos::ZERO;
                pos[axis] = v;
                pos[a1] = c1;
                pos[a2] = c2;
                if !world.block(pos).is_air() {
                    return true;
                }
            }
        }
        false
    };

    if d > 0.0 {
        let leading = aabb.max[axis];
        let first = (leading + SKIN).floor() as i32;
        let last = (leading + d).floor() as i32;
        for v in first..=last {
            if solid_layer(v) {
                return (v as f64 - leading - SKIN).max(0.0);
            }
        }
        d
    } else {
        let leading = aabb.min[axis];
        let first = (leading - SKIN).floor() as i32;
        let last = (leading + d).floor() as i32;
        for v in (last..=first).rev() {
            if solid_layer(v) {
                return ((v + 1) as f64 - leading + SKIN).min(0.0);
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;
    use oc_core::ChunkPos;

    /// World with a generated column plus a flat stone platform at y=200,
    /// far above any generated terrain (max ~80), so tests are layout-exact.
    fn platform_world() -> World {
        let mut world = World::new(1);
        world.generate_column(ChunkPos::new(0, 0));
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(BlockPos::new(x, 200, z), blocks::STONE);
            }
        }
        world
    }

    fn player_box(feet: DVec3) -> Aabb {
        Aabb::standing(feet, 0.3, 1.8)
    }

    #[test]
    fn falls_and_lands_on_platform() {
        let world = platform_world();
        let aabb = player_box(DVec3::new(8.0, 205.0, 8.0));
        let result = move_aabb(&world, aabb, DVec3::new(0.0, -10.0, 0.0));
        assert!(result.on_ground);
        // Platform top is y=201; feet stop just above it.
        assert!((result.delta.y - (201.0 - 205.0 + SKIN)).abs() < 1e-9, "{:?}", result.delta);
    }

    #[test]
    fn walks_freely_on_open_platform() {
        let world = platform_world();
        let aabb = player_box(DVec3::new(4.0, 201.0 + SKIN, 4.0));
        let result = move_aabb(&world, aabb, DVec3::new(2.0, 0.0, 1.5));
        assert!(!result.hit[0] && !result.hit[2], "{result:?}");
        assert_eq!(result.delta.x, 2.0);
        assert_eq!(result.delta.z, 1.5);
    }

    #[test]
    fn wall_blocks_horizontal_motion() {
        let mut world = platform_world();
        // Wall at x=10 spanning the player's height.
        for y in 201..204 {
            for z in 0..16 {
                world.set_block(BlockPos::new(10, y, z), blocks::STONE);
            }
        }
        let aabb = player_box(DVec3::new(8.0, 201.0 + SKIN, 8.0));
        let result = move_aabb(&world, aabb, DVec3::new(5.0, 0.0, 0.0));
        assert!(result.hit[0]);
        // Box face (x = 8.3) stops at the wall face (x = 10) minus skin.
        assert!((result.delta.x - (10.0 - 8.3 - SKIN)).abs() < 1e-9, "{:?}", result.delta);
    }

    #[test]
    fn ceiling_stops_upward_motion() {
        let mut world = platform_world();
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(BlockPos::new(x, 204, z), blocks::STONE);
            }
        }
        let aabb = player_box(DVec3::new(8.0, 201.0 + SKIN, 8.0));
        let result = move_aabb(&world, aabb, DVec3::new(0.0, 5.0, 0.0));
        assert!(result.hit[1]);
        assert!(!result.on_ground);
        // Head (feet + 1.8) stops at y=204 minus skin.
        let expected = 204.0 - (201.0 + SKIN + 1.8) - SKIN;
        assert!((result.delta.y - expected).abs() < 1e-9, "{:?}", result.delta);
    }
}
