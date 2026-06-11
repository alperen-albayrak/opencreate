//! Voxel raycasting (Amanatides & Woo DDA) for block targeting.

use glam::{DVec3, IVec3};
use oc_core::BlockPos;

use crate::World;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayHit {
    /// The solid block that was hit.
    pub block: BlockPos,
    /// Unit normal of the face entered, pointing out of the block.
    /// Zero when the ray origin is already inside a solid block.
    pub normal: IVec3,
}

/// Walks the voxel grid from `origin` along `dir` (need not be normalized)
/// and returns the first solid block within `max_dist` (world units).
pub fn raycast(world: &World, origin: DVec3, dir: DVec3, max_dist: f64) -> Option<RayHit> {
    let dir = dir.normalize_or_zero();
    if dir == DVec3::ZERO {
        return None;
    }

    let mut voxel = origin.floor().as_ivec3();
    if !world.block(voxel).is_air() {
        return Some(RayHit { block: voxel, normal: IVec3::ZERO });
    }

    let step = IVec3::new(
        if dir.x > 0.0 { 1 } else { -1 },
        if dir.y > 0.0 { 1 } else { -1 },
        if dir.z > 0.0 { 1 } else { -1 },
    );
    // Distance along the ray between successive grid planes per axis.
    let t_delta = DVec3::new(
        if dir.x != 0.0 { (1.0 / dir.x).abs() } else { f64::INFINITY },
        if dir.y != 0.0 { (1.0 / dir.y).abs() } else { f64::INFINITY },
        if dir.z != 0.0 { (1.0 / dir.z).abs() } else { f64::INFINITY },
    );
    // Distance along the ray to the first grid plane per axis.
    let next_plane = |o: f64, v: i32, s: i32| {
        if s > 0 { (v + 1) as f64 - o } else { o - v as f64 }
    };
    let mut t_max = DVec3::new(
        next_plane(origin.x, voxel.x, step.x) * t_delta.x,
        next_plane(origin.y, voxel.y, step.y) * t_delta.y,
        next_plane(origin.z, voxel.z, step.z) * t_delta.z,
    );

    loop {
        // Advance along whichever axis crosses its next plane first.
        let axis = if t_max.x < t_max.y {
            if t_max.x < t_max.z { 0 } else { 2 }
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        if t_max[axis] > max_dist {
            return None;
        }
        voxel[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        if !world.block(voxel).is_air() {
            let mut normal = IVec3::ZERO;
            normal[axis] = -step[axis];
            return Some(RayHit { block: voxel, normal });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;
    use oc_core::ChunkPos;

    fn world_with_block(pos: BlockPos) -> World {
        let mut world = World::new(1);
        world.generate_column(ChunkPos::new(pos.x >> 4, pos.z >> 4));
        world.set_block(pos, blocks::STONE);
        world
    }

    #[test]
    fn hits_block_straight_ahead() {
        let target = BlockPos::new(8, 200, 8);
        let world = world_with_block(target);
        let hit = raycast(
            &world,
            DVec3::new(8.5, 200.5, 4.0),
            DVec3::Z,
            10.0,
        )
        .expect("should hit");
        assert_eq!(hit.block, target);
        assert_eq!(hit.normal, IVec3::new(0, 0, -1));
    }

    #[test]
    fn hits_top_face_from_above() {
        let target = BlockPos::new(8, 200, 8);
        let world = world_with_block(target);
        let hit = raycast(
            &world,
            DVec3::new(8.5, 205.0, 8.5),
            DVec3::NEG_Y,
            10.0,
        )
        .expect("should hit");
        assert_eq!(hit.block, target);
        assert_eq!(hit.normal, IVec3::Y);
    }

    #[test]
    fn respects_max_distance() {
        let target = BlockPos::new(8, 200, 8);
        let world = world_with_block(target);
        let from = DVec3::new(8.5, 200.5, 0.0);
        assert!(raycast(&world, from, DVec3::Z, 3.0).is_none());
        assert!(raycast(&world, from, DVec3::Z, 9.0).is_some());
    }

    #[test]
    fn diagonal_ray_lands_on_target() {
        let target = BlockPos::new(10, 201, 10);
        let world = world_with_block(target);
        let from = DVec3::new(5.5, 203.5, 5.5);
        let to_center = target.as_dvec3() + DVec3::splat(0.5) - from;
        let hit = raycast(&world, from, to_center, 20.0).expect("should hit");
        assert_eq!(hit.block, target);
    }
}
