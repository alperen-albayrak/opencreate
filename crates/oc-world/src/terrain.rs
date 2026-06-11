//! Terrain generation (ARCHITECTURE.md §5).
//!
//! Milestone-2 start: a deterministic value-noise fBm heightmap with a
//! grass/dirt/stone surface. Hand-rolled so the noise-crate decision (§13)
//! stays open; the lattice hash keeps worldgen reproducible from the seed.

use glam::IVec3;
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE};

use crate::{BlockId, blocks};

/// Lowest section a generated column reaches. Terrain below this is left
/// ungenerated until caves/depth arrive; meshes are never seen from below in
/// normal play.
pub const BOTTOM_SECTION_Y: i32 = -4;

/// Water fills open terrain up to this Y (ARCHITECTURE.md §3: sea level 0).
pub const SEA_LEVEL: i32 = 0;

/// Surfaces at or below this height are sand (beaches and sea floor).
const BEACH_TOP: i32 = SEA_LEVEL + 1;

/// Heightmap terrain, pure function of (seed, x, z).
#[derive(Clone, Copy)]
pub struct TerrainGenerator {
    seed: u64,
}

impl TerrainGenerator {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// Y of the topmost solid block in this column (sea level is Y = 0).
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        // Two scales: rolling hills plus a broader continental swell.
        let hills = fbm(self.seed, x as f64 / 64.0, z as f64 / 64.0, 4);
        let swell = fbm(self.seed ^ 0x5EED_C0FF_EE00_0001, x as f64 / 512.0, z as f64 / 512.0, 2);
        (hills * 18.0 + swell * 40.0 + 4.0).floor() as i32
    }

    /// Block for world-space `y` in a column whose surface is at `surface`.
    pub fn block_at(&self, surface: i32, y: i32) -> BlockId {
        if y > surface {
            if y <= SEA_LEVEL { blocks::WATER } else { blocks::AIR }
        } else if y >= surface - 3 {
            if surface <= BEACH_TOP {
                blocks::SAND
            } else if y == surface {
                blocks::GRASS
            } else {
                blocks::DIRT
            }
        } else {
            blocks::STONE
        }
    }

    /// Deterministic tree trunk-base positions whose canopies may intersect
    /// `chunk`'s column or its neighbors. Trees only grow on grass (above
    /// the beach band).
    pub fn tree_origins(&self, chunk: ChunkPos) -> Vec<BlockPos> {
        let mut origins = Vec::new();
        let h = lattice_bits(self.seed ^ 0x7EE5_0000_0000_0001, chunk.x, chunk.z);
        let count = (h % 3) as usize; // 0..=2 trees per column
        for i in 0..count {
            let bits = h >> (8 + i * 16);
            let dx = (bits & 15) as i32;
            let dz = ((bits >> 4) & 15) as i32;
            let (x, z) = (chunk.x * SECTION_SIZE + dx, chunk.z * SECTION_SIZE + dz);
            let surface = self.surface_height(x, z);
            if surface > BEACH_TOP {
                origins.push(IVec3::new(x, surface + 1, z));
            }
        }
        origins
    }

    /// The blocks of one tree (trunk + canopy) rooted at `origin`.
    pub fn tree_blocks(&self, origin: BlockPos) -> Vec<(BlockPos, BlockId)> {
        let h = lattice_bits(self.seed ^ 0x7EE5_0000_0000_0002, origin.x, origin.z);
        let trunk = 4 + (h % 3) as i32; // 4..=6
        let mut out = Vec::new();
        for dy in 0..trunk {
            out.push((origin + IVec3::new(0, dy, 0), blocks::LOG));
        }
        // Canopy: two 5x5 layers, then a 3x3, then a plus on top.
        for (dy, radius) in [(trunk - 2, 2i32), (trunk - 1, 2), (trunk, 1)] {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx == 0 && dz == 0 && dy < trunk {
                        continue; // trunk occupies the center
                    }
                    // Trim corners of the wide layers for a rounder shape.
                    if radius == 2 && dx.abs() == 2 && dz.abs() == 2 && (h >> (dx + 2 * dz + 10)) & 1 == 0 {
                        continue;
                    }
                    out.push((origin + IVec3::new(dx, dy, dz), blocks::LEAVES));
                }
            }
        }
        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            out.push((origin + IVec3::new(dx, trunk + 1, dz), blocks::LEAVES));
        }
        out
    }
}

/// Deterministic 64 hash bits for a 2D lattice point.
fn lattice_bits(seed: u64, x: i32, z: i32) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Fractional Brownian motion over value noise, roughly in [-1, 1].
fn fbm(seed: u64, x: f64, z: f64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for octave in 0..octaves {
        // Decorrelate octaves so lattice artifacts don't line up.
        let octave_seed = seed.wrapping_add((octave as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        sum += value_noise(octave_seed, x * frequency, z * frequency) * amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum
}

/// Bilinear value noise in [-1, 1] with smoothstep fade.
fn value_noise(seed: u64, x: f64, z: f64) -> f64 {
    let x0 = x.floor();
    let z0 = z.floor();
    let fx = fade(x - x0);
    let fz = fade(z - z0);
    let (xi, zi) = (x0 as i32, z0 as i32);

    let v00 = lattice(seed, xi, zi);
    let v10 = lattice(seed, xi + 1, zi);
    let v01 = lattice(seed, xi, zi + 1);
    let v11 = lattice(seed, xi + 1, zi + 1);

    lerp(lerp(v00, v10, fx), lerp(v01, v11, fx), fz)
}

fn fade(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Deterministic lattice value in [-1, 1] (splitmix64 finalizer).
fn lattice(seed: u64, x: i32, z: i32) -> f64 {
    // 53 high bits -> [0, 1) -> [-1, 1].
    (lattice_bits(seed, x, z) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_are_deterministic() {
        let a = TerrainGenerator::new(42);
        let b = TerrainGenerator::new(42);
        for (x, z) in [(0, 0), (1000, -1000), (-31, 17), (i32::MAX / 2, i32::MIN / 2)] {
            assert_eq!(a.surface_height(x, z), b.surface_height(x, z));
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = TerrainGenerator::new(1);
        let b = TerrainGenerator::new(2);
        let same = (0..64).filter(|&i| a.surface_height(i * 100, 0) == b.surface_height(i * 100, 0)).count();
        assert!(same < 16, "seeds 1 and 2 produced near-identical terrain");
    }

    #[test]
    fn heights_stay_in_sane_band() {
        let g = TerrainGenerator::new(7);
        for x in (-2048..2048).step_by(61) {
            for z in (-2048..2048).step_by(67) {
                let h = g.surface_height(x, z);
                assert!((-80..=80).contains(&h), "height {h} at ({x},{z}) out of band");
            }
        }
    }

    #[test]
    fn column_profile_is_grass_dirt_stone() {
        let g = TerrainGenerator::new(7);
        let surface = 10;
        assert_eq!(g.block_at(surface, 11), blocks::AIR);
        assert_eq!(g.block_at(surface, 10), blocks::GRASS);
        assert_eq!(g.block_at(surface, 9), blocks::DIRT);
        assert_eq!(g.block_at(surface, 7), blocks::DIRT);
        assert_eq!(g.block_at(surface, 6), blocks::STONE);
        assert_eq!(g.block_at(surface, -500), blocks::STONE);
    }

    #[test]
    fn underwater_profile_is_sand_then_water() {
        let g = TerrainGenerator::new(7);
        let surface = -5;
        assert_eq!(g.block_at(surface, -5), blocks::SAND);
        assert_eq!(g.block_at(surface, -7), blocks::SAND);
        assert_eq!(g.block_at(surface, -9), blocks::STONE);
        assert_eq!(g.block_at(surface, -4), blocks::WATER);
        assert_eq!(g.block_at(surface, SEA_LEVEL), blocks::WATER);
        assert_eq!(g.block_at(surface, SEA_LEVEL + 1), blocks::AIR);
        // Beach band: dry sand just above the water line.
        assert_eq!(g.block_at(1, 1), blocks::SAND);
        assert_eq!(g.block_at(2, 2), blocks::GRASS);
    }

    #[test]
    fn trees_are_deterministic_and_grounded() {
        let g = TerrainGenerator::new(42);
        let mut found = 0;
        for cx in -8..8 {
            for cz in -8..8 {
                let chunk = oc_core::ChunkPos::new(cx, cz);
                let a = g.tree_origins(chunk);
                let b = g.tree_origins(chunk);
                assert_eq!(a, b, "tree origins must be deterministic");
                for origin in a {
                    found += 1;
                    // Rooted one above a grass surface, never on the beach.
                    let h = g.surface_height(origin.x, origin.z);
                    assert_eq!(origin.y, h + 1);
                    assert!(h > 1, "tree on beach/underwater at {origin}");
                    let blocks_list = g.tree_blocks(origin);
                    assert!(blocks_list.len() > 10, "tree too small");
                    assert!(blocks_list.iter().any(|(_, b)| *b == blocks::LOG));
                    assert!(blocks_list.iter().any(|(_, b)| *b == blocks::LEAVES));
                }
            }
        }
        assert!(found > 10, "expected a healthy number of trees, got {found}");
    }
}
