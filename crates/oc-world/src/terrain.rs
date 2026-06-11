//! Terrain generation (ARCHITECTURE.md §5).
//!
//! Milestone-2 start: a deterministic value-noise fBm heightmap with a
//! grass/dirt/stone surface. Hand-rolled so the noise-crate decision (§13)
//! stays open; the lattice hash keeps worldgen reproducible from the seed.

use crate::{BlockId, blocks};

/// Lowest section a generated column reaches. Terrain below this is left
/// ungenerated until caves/depth arrive; meshes are never seen from below in
/// normal play.
pub const BOTTOM_SECTION_Y: i32 = -4;

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
            blocks::AIR
        } else if y == surface {
            blocks::GRASS
        } else if y >= surface - 3 {
            blocks::DIRT
        } else {
            blocks::STONE
        }
    }
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
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    // 53 high bits -> [0, 1) -> [-1, 1].
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
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
}
