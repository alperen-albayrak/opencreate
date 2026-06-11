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

/// Climate band a column belongs to, from the temperature noise channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Desert,
    Grassland,
    Snowy,
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
        let base = (hills * 18.0 + swell * 40.0 + 4.0).floor() as i32;

        // Rivers (§5.3, MC 1.18 style): where a noise channel crosses zero,
        // the terrain depresses to a sea-level valley; the standard water
        // fill then makes it a river.
        let r = fbm(self.seed ^ 0x41E5_0000_0000_0007, x as f64 / 384.0, z as f64 / 384.0, 2);
        const RIVER_HALF_WIDTH: f64 = 0.045;
        if base > SEA_LEVEL - 3 && r.abs() < RIVER_HALF_WIDTH {
            // 1 at the centerline, 0 at the banks; smooth the bank slope.
            let t = fade(1.0 - r.abs() / RIVER_HALF_WIDTH);
            let bed = (SEA_LEVEL - 3) as f64;
            return (base as f64 + (bed - base as f64) * t).floor() as i32;
        }
        base
    }

    /// Climate at a column (pure function of the seed).
    pub fn biome(&self, x: i32, z: i32) -> Biome {
        let t = fbm(self.seed ^ 0x7E3A_0000_0000_0009, x as f64 / 640.0, z as f64 / 640.0, 2);
        if t > 0.32 {
            Biome::Desert
        } else if t < -0.32 {
            Biome::Snowy
        } else {
            Biome::Grassland
        }
    }

    /// Block for world-space `y` in a column with the given surface/biome.
    pub fn block_at_biome(&self, surface: i32, biome: Biome, y: i32) -> BlockId {
        if y > surface {
            if y <= SEA_LEVEL { blocks::WATER } else { blocks::AIR }
        } else if y >= surface - 3 {
            match biome {
                Biome::Desert => blocks::SAND,
                _ if surface <= BEACH_TOP => blocks::SAND,
                Biome::Snowy if y == surface => blocks::SNOW,
                Biome::Grassland if y == surface => blocks::GRASS,
                _ => blocks::DIRT,
            }
        } else {
            blocks::STONE
        }
    }

    /// Grassland-profile blocks; kept for biome-agnostic call sites/tests.
    pub fn block_at(&self, surface: i32, y: i32) -> BlockId {
        self.block_at_biome(surface, Biome::Grassland, y)
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
            if surface <= BEACH_TOP {
                continue;
            }
            match self.biome(x, z) {
                Biome::Desert => continue,
                // Sparse trees in the snow.
                Biome::Snowy if (h >> (40 + i)) & 3 != 0 => continue,
                _ => origins.push(IVec3::new(x, surface + 1, z)),
            }
        }
        origins
    }

    /// True where 3D noise carves a cave out of solid terrain ("cheese"
    /// caves, §5.2). Pure function of (seed, position, surface).
    pub fn is_cave(&self, pos: BlockPos, surface: i32) -> bool {
        // Keep the world floor and shorelines intact: no holes in the
        // bottom sections, none puncturing beach/ocean floors into the sea.
        if pos.y <= (BOTTOM_SECTION_Y + 1) * SECTION_SIZE {
            return false;
        }
        if surface <= BEACH_TOP && pos.y > surface - 6 {
            return false;
        }

        let depth = surface - pos.y;
        // Entrances exist but are rare: the carve threshold relaxes with
        // depth (8+ blocks down caves open up properly).
        let threshold = if depth < 8 { 0.52 } else { 0.34 };

        // Vertically squashed noise -> caverns wider than they are tall.
        let d = fbm3(
            self.seed ^ 0xCAFE_0000_0000_0003,
            pos.x as f64 / 36.0,
            pos.y as f64 / 22.0,
            pos.z as f64 / 36.0,
            3,
        );
        d > threshold
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

/// 3D fractional Brownian motion over value noise, roughly in [-1, 1].
fn fbm3(seed: u64, x: f64, y: f64, z: f64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for octave in 0..octaves {
        let octave_seed = seed.wrapping_add((octave as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        sum += value_noise_3d(octave_seed, x * frequency, y * frequency, z * frequency) * amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum
}

/// Trilinear value noise in [-1, 1] with smoothstep fade.
fn value_noise_3d(seed: u64, x: f64, y: f64, z: f64) -> f64 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (fade(x - x0), fade(y - y0), fade(z - z0));
    let (xi, yi, zi) = (x0 as i32, y0 as i32, z0 as i32);

    let at = |dx: i32, dy: i32, dz: i32| lattice3(seed, xi + dx, yi + dy, zi + dz);
    let bottom = lerp(
        lerp(at(0, 0, 0), at(1, 0, 0), fx),
        lerp(at(0, 0, 1), at(1, 0, 1), fx),
        fz,
    );
    let top = lerp(
        lerp(at(0, 1, 0), at(1, 1, 0), fx),
        lerp(at(0, 1, 1), at(1, 1, 1), fx),
        fz,
    );
    lerp(bottom, top, fy)
}

/// Deterministic 3D lattice value in [-1, 1].
fn lattice3(seed: u64, x: i32, y: i32, z: i32) -> f64 {
    let mixed = seed ^ (y as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    (lattice_bits(mixed, x, z) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
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
    fn all_biomes_occur_and_are_deterministic() {
        let g = TerrainGenerator::new(42);
        let mut seen = [false; 3];
        for x in (-4096..4096).step_by(97) {
            for z in (-4096..4096).step_by(101) {
                assert_eq!(g.biome(x, z), g.biome(x, z));
                seen[match g.biome(x, z) {
                    Biome::Desert => 0,
                    Biome::Grassland => 1,
                    Biome::Snowy => 2,
                }] = true;
            }
        }
        assert_eq!(seen, [true; 3], "all biomes should appear within 8km");
    }

    #[test]
    fn biome_surface_blocks() {
        let g = TerrainGenerator::new(7);
        assert_eq!(g.block_at_biome(20, Biome::Desert, 20), blocks::SAND);
        assert_eq!(g.block_at_biome(20, Biome::Desert, 18), blocks::SAND);
        assert_eq!(g.block_at_biome(20, Biome::Snowy, 20), blocks::SNOW);
        assert_eq!(g.block_at_biome(20, Biome::Snowy, 19), blocks::DIRT);
        assert_eq!(g.block_at_biome(20, Biome::Grassland, 20), blocks::GRASS);
        // Beaches override every biome's cap near the water line.
        assert_eq!(g.block_at_biome(1, Biome::Snowy, 1), blocks::SAND);
    }

    #[test]
    fn rivers_carve_to_sea_level_through_hills() {
        let g = TerrainGenerator::new(42);
        // Find genuine river centers: columns lying lower than their broad
        // surroundings, at/below the river bed level.
        let mut rivers = 0;
        for x in (-3000..3000).step_by(11) {
            for z in (-3000..3000).step_by(13) {
                let h = g.surface_height(x, z);
                if h == SEA_LEVEL - 3 {
                    let near = g.surface_height(x + 96, z + 96);
                    if near > SEA_LEVEL + 4 {
                        rivers += 1;
                    }
                }
            }
        }
        assert!(rivers > 20, "expected river valleys through high terrain, found {rivers}");
    }

    #[test]
    fn caves_carve_a_sane_fraction_of_deep_stone() {
        let g = TerrainGenerator::new(42);
        let mut carved = 0;
        let mut total = 0;
        for x in (-200..200).step_by(7) {
            for z in (-200..200).step_by(7) {
                let surface = g.surface_height(x, z);
                for y in (-40..surface - 10).step_by(5) {
                    total += 1;
                    if g.is_cave(IVec3::new(x, y, z), surface) {
                        carved += 1;
                    }
                }
            }
        }
        let percent = carved * 100 / total;
        assert!(
            (2..=30).contains(&percent),
            "deep cave volume should be a modest fraction: {percent}% ({carved}/{total})"
        );
    }

    #[test]
    fn caves_never_breach_ocean_floors_or_world_bottom() {
        let g = TerrainGenerator::new(42);
        for x in -100..100 {
            for z in (-100..100).step_by(3) {
                let surface = g.surface_height(x, z);
                if surface <= 1 {
                    for y in (surface - 5)..=surface {
                        assert!(
                            !g.is_cave(IVec3::new(x, y, z), surface),
                            "cave punctured the sea floor at ({x},{y},{z})"
                        );
                    }
                }
                assert!(!g.is_cave(IVec3::new(x, -48, z), surface), "hole in world floor");
            }
        }
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
