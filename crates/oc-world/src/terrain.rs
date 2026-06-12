//! Terrain generation (ARCHITECTURE.md §5): multi-noise worldgen, the
//! architecture modern voxel games converged on:
//!
//! - Five 2D climate channels — continentalness, erosion, weirdness,
//!   temperature, humidity — sampled through a domain warp.
//! - "Peaks & valleys" folded out of weirdness: `PV = 1 − |3|w| − 2|`.
//!   Rivers are the zero-crossing band of weirdness (PV ≈ −1), so they
//!   form long connected lines for free; peaks sit at |w| ≈ ⅔.
//! - Terrain height from nested splines: a continentalness spline carries
//!   ocean floors and scales an inland (erosion × PV) spline table, the
//!   same shape as vanilla's offset spline. Jaggedness noise roughens
//!   low-erosion peaks; a detail channel (damped by erosion and near
//!   rivers) adds local relief.
//! - Biomes from the climate tuple plus altitude zoning; surface rules
//!   (steep slopes expose stone, snow caps, beach sand) repaint the top.
//! - Caves: "cheese" caverns from squashed 3D noise plus "spaghetti"
//!   tunnels along the intersection of two noise zero-surfaces.
//!
//! All of it is hand-rolled value noise over splitmix lattice hashes, so
//! worldgen stays a pure deterministic function of the 64-bit seed.

use glam::IVec3;
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE};

use crate::{BlockId, blocks};

/// Lowest section a generated column reaches. Terrain below this is left
/// ungenerated until caves/depth arrive; meshes are never seen from below in
/// normal play.
pub const BOTTOM_SECTION_Y: i32 = -4;

/// Water fills open terrain up to this Y (ARCHITECTURE.md §3: sea level 0).
pub const SEA_LEVEL: i32 = 0;

/// Dry surfaces at or below this height near coasts/rivers are sand.
const BEACH_TOP: i32 = SEA_LEVEL + 1;

/// No trees grow above this surface height (alpine treeline).
const TREELINE: i32 = 78;

// Channel seeds (xor'd into the world seed so channels decorrelate).
const SEED_SHIFT_A: u64 = 0x5817_F7AA_0000_0001;
const SEED_SHIFT_B: u64 = 0x5817_F7BB_0000_0002;
const SEED_CONTINENTS: u64 = 0xC047_14E4_0000_0003;
const SEED_EROSION: u64 = 0xE205_104A_0000_0004;
const SEED_WEIRDNESS: u64 = 0x3E1A_D4E5_0000_0005;
const SEED_TEMPERATURE: u64 = 0x7E3A_0000_0000_0009;
const SEED_HUMIDITY: u64 = 0x4A11_D170_0000_0006;
const SEED_JAGGED: u64 = 0x1A66_ED00_0000_0007;
const SEED_DETAIL: u64 = 0xDE7A_11ED_0000_0008;
const SEED_CHEESE: u64 = 0xCAFE_0000_0000_0003;
const SEED_SPAGHETTI_A: u64 = 0x59A6_8E77_1000_000A;
const SEED_SPAGHETTI_B: u64 = 0x59A6_8E77_2000_000B;
const SEED_TREES: u64 = 0x7EE5_0000_0000_0001;
const SEED_TREE_SHAPE: u64 = 0x7EE5_0000_0000_0002;
const SEED_VILLAGE: u64 = 0x111A_6E00_0000_000C;
const SEED_HOUSE: u64 = 0x115E_0000_0000_000D;

/// Villages are placed per square region of this many chunks.
pub const VILLAGE_REGION: i32 = 12;

/// Heightmap terrain, pure function of (seed, x, z).
#[derive(Clone, Copy)]
pub struct TerrainGenerator {
    seed: u64,
}

/// The climate tuple a column's terrain and biome derive from.
#[derive(Debug, Clone, Copy)]
pub struct Climate {
    pub temperature: f64,
    pub humidity: f64,
    pub continentalness: f64,
    pub erosion: f64,
    pub weirdness: f64,
    /// Peaks & valleys: −1 in valleys (river lines), +1 on ridges.
    pub pv: f64,
}

/// What a column is, resolved once per (x, z).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnInfo {
    pub surface: i32,
    pub biome: Biome,
    /// True on cliff-grade slopes; the surface rule exposes stone there.
    pub steep: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Biome {
    DeepOcean,
    Ocean,
    Beach,
    StonyShore,
    River,
    Plains,
    Forest,
    Taiga,
    SnowyPlains,
    SnowyTaiga,
    Desert,
    StonyPeaks,
    SnowyPeaks,
}

impl Biome {
    /// Biomes whose dry surface is grass (creature spawning, trees).
    pub fn has_grass_surface(self) -> bool {
        matches!(
            self,
            Biome::Plains | Biome::Forest | Biome::Taiga | Biome::River
        )
    }
}

impl TerrainGenerator {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The climate tuple at a column. All channels are sampled through a
    /// shared domain warp so coastlines and biome borders wander together.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let (fx, fz) = (x as f64, z as f64);
        let wx = fx + 26.0 * fbm(self.seed ^ SEED_SHIFT_A, fx / 128.0, fz / 128.0, 2);
        let wz = fz + 26.0 * fbm(self.seed ^ SEED_SHIFT_B, fx / 128.0, fz / 128.0, 2);

        let continentalness =
            (fbm(self.seed ^ SEED_CONTINENTS, wx / 1400.0, wz / 1400.0, 6) * 2.4).clamp(-1.2, 1.0);
        let erosion =
            (fbm(self.seed ^ SEED_EROSION, wx / 1000.0, wz / 1000.0, 4) * 2.6).clamp(-1.0, 1.0);
        let weirdness =
            (fbm(self.seed ^ SEED_WEIRDNESS, wx / 440.0, wz / 440.0, 3) * 2.6).clamp(-1.2, 1.2);
        let temperature =
            (fbm(self.seed ^ SEED_TEMPERATURE, wx / 2200.0, wz / 2200.0, 3) * 2.4).clamp(-1.0, 1.0);
        let humidity =
            (fbm(self.seed ^ SEED_HUMIDITY, wx / 800.0, wz / 800.0, 3) * 2.4).clamp(-1.0, 1.0);

        // The peaks&valleys fold: valleys where weirdness crosses zero, peaks at
        // |w| = 2/3. Rivers fall out of the PV ≈ −1 band.
        let pv = (1.0 - (3.0 * weirdness.abs() - 2.0).abs()).clamp(-1.0, 1.0);

        Climate { temperature, humidity, continentalness, erosion, weirdness, pv }
    }

    /// Y of the topmost solid block in this column (sea level is Y = 0).
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let climate = self.climate(x, z);
        self.height_for(&climate, x, z)
    }

    /// Everything downstream needs about a column, computed in one pass.
    pub fn column(&self, x: i32, z: i32) -> ColumnInfo {
        let climate = self.climate(x, z);
        let surface = self.height_for(&climate, x, z);
        // Cliff detection from the central height difference; the surface
        // rule strips soil off anything steeper than ~4 blocks per block.
        let dx = (self.surface_height(x + 1, z) - self.surface_height(x - 1, z)).abs();
        let dz = (self.surface_height(x, z + 1) - self.surface_height(x, z - 1)).abs();
        let steep = dx.max(dz) >= 8;
        let biome = self.biome_for(&climate, surface, x, z);
        ColumnInfo { surface, biome, steep }
    }

    /// Climate band of a column (pure function of the seed).
    pub fn biome(&self, x: i32, z: i32) -> Biome {
        let climate = self.climate(x, z);
        let surface = self.height_for(&climate, x, z);
        self.biome_for(&climate, surface, x, z)
    }

    fn height_for(&self, climate: &Climate, x: i32, z: i32) -> i32 {
        let land = land_height(climate.erosion, climate.pv);
        let base = continent_height(climate.continentalness, land);

        let mut height = base;

        // Jaggedness: rocky relief on inland, low-erosion ridge tops only
        // (vanilla's jaggedness spline), mostly additive (negative lobes
        // are quartered) so summits spike rather than crater.
        let jag = ramp(climate.continentalness, 0.0, 0.3)
            * ramp(-climate.erosion, 0.35, 0.75)
            * ramp(climate.pv, 0.4, 0.85);
        if jag > 0.0 {
            let j = fbm(self.seed ^ SEED_JAGGED, x as f64 / 28.0, z as f64 / 28.0, 3) * 2.0;
            let j = if j > 0.0 { j } else { j * 0.25 };
            height += jag * 42.0 * j;
        }

        // Local relief: stronger at low erosion, damped to nothing near
        // valley centerlines so rivers stay connected water lines.
        let amp = (2.0 + 9.0 * ramp(-climate.erosion, -0.4, 1.0) + base.max(0.0) * 0.04)
            * ramp(climate.pv + 1.0, 0.05, 0.30);
        let detail = fbm(self.seed ^ SEED_DETAIL, x as f64 / 56.0, z as f64 / 56.0, 3) * 1.6;
        height += amp * detail;

        height.floor() as i32
    }

    fn biome_for(&self, climate: &Climate, surface: i32, x: i32, z: i32) -> Biome {
        if climate.continentalness < -0.45 {
            return Biome::DeepOcean;
        }
        if climate.continentalness < -0.19 {
            return Biome::Ocean;
        }
        // Rivers: the valley band of the weirdness fold, where the offset
        // spline actually carved below the waterline.
        if climate.pv < -0.85 && surface < SEA_LEVEL {
            return Biome::River;
        }
        // Shores: low dry land near the coast. Low erosion means the land
        // came down steeply — stony shore instead of a sand beach.
        if surface <= BEACH_TOP + 2 && climate.continentalness < 0.05 {
            return if climate.erosion < -0.375 { Biome::StonyShore } else { Biome::Beach };
        }
        // Altitude zoning, dithered so the lines aren't contours.
        let jitter = (lattice_bits(self.seed ^ SEED_DETAIL, x, z) & 7) as i32;
        if surface > 92 + jitter {
            return Biome::SnowyPeaks;
        }
        if surface > 72 + jitter {
            return if climate.temperature < -0.2 { Biome::SnowyPeaks } else { Biome::StonyPeaks };
        }
        // The "middle biomes" table: temperature bands × humidity.
        match climate.temperature {
            t if t < -0.45 => {
                if climate.humidity < 0.1 {
                    Biome::SnowyPlains
                } else {
                    Biome::SnowyTaiga
                }
            }
            t if t < -0.15 => {
                if climate.humidity < -0.1 {
                    Biome::Plains
                } else {
                    Biome::Taiga
                }
            }
            t if t < 0.25 => {
                if climate.humidity < 0.05 {
                    Biome::Plains
                } else {
                    Biome::Forest
                }
            }
            t if t < 0.55 => {
                if climate.humidity < -0.15 {
                    Biome::Plains
                } else {
                    Biome::Forest
                }
            }
            _ => Biome::Desert,
        }
    }

    /// Block for world-space `y` in a resolved column (the surface rules).
    pub fn block_in_column(&self, info: &ColumnInfo, y: i32) -> BlockId {
        let surface = info.surface;
        if y > surface {
            return if y <= SEA_LEVEL { blocks::WATER } else { blocks::AIR };
        }
        let depth = surface - y;
        // Submerged floors: sand in the shallows, bare stone deeper.
        if surface < SEA_LEVEL {
            return if depth < 3 && surface >= -10 { blocks::SAND } else { blocks::STONE };
        }
        if depth >= 4 {
            return blocks::STONE;
        }
        match info.biome {
            Biome::Beach | Biome::Desert => blocks::SAND,
            Biome::StonyShore | Biome::StonyPeaks => blocks::STONE,
            _ if info.steep => blocks::STONE,
            Biome::SnowyPeaks => {
                if depth == 0 {
                    blocks::SNOW
                } else {
                    blocks::STONE
                }
            }
            Biome::SnowyPlains | Biome::SnowyTaiga => {
                if depth == 0 {
                    blocks::SNOW
                } else {
                    blocks::DIRT
                }
            }
            // Grassland family (incl. dry river banks).
            _ => {
                if depth == 0 {
                    blocks::GRASS
                } else {
                    blocks::DIRT
                }
            }
        }
    }

    /// Deterministic tree trunk-base positions whose canopies may intersect
    /// `chunk`'s column or its neighbors. Density is biome-driven.
    pub fn tree_origins(&self, chunk: ChunkPos) -> Vec<BlockPos> {
        let h = lattice_bits(self.seed ^ SEED_TREES, chunk.x, chunk.z);
        let mut origins = Vec::new();
        for i in 0..4usize {
            let bits = h >> (i * 14);
            let dx = (bits & 15) as i32;
            let dz = ((bits >> 4) & 15) as i32;
            let roll = ((bits >> 8) & 63) as u32;
            let (x, z) = (chunk.x * SECTION_SIZE + dx, chunk.z * SECTION_SIZE + dz);
            let info = self.column(x, z);
            // Chance out of 64 that this candidate slot grows a tree.
            let chance = match info.biome {
                Biome::Forest => 48,
                Biome::Taiga => 36,
                Biome::SnowyTaiga => 20,
                Biome::Plains | Biome::SnowyPlains => 3,
                _ => 0,
            };
            if roll < chance && info.surface > BEACH_TOP && info.surface < TREELINE && !info.steep
            {
                origins.push(IVec3::new(x, info.surface + 1, z));
            }
        }
        origins
    }

    /// Two-phase village placement, phase 1: does this region (a
    /// `VILLAGE_REGION`² chunk square) hold a village, and where is its
    /// center chunk? Villages settle flat, friendly land only.
    pub fn village_center(&self, region_x: i32, region_z: i32) -> Option<ChunkPos> {
        let bits = lattice_bits(self.seed ^ SEED_VILLAGE, region_x, region_z);
        if bits % 3 != 0 {
            return None; // ~1 in 3 regions
        }
        let cx = region_x * VILLAGE_REGION + ((bits >> 8) % VILLAGE_REGION as u64) as i32;
        let cz = region_z * VILLAGE_REGION + ((bits >> 16) % VILLAGE_REGION as u64) as i32;
        let info = self.column(cx * SECTION_SIZE + 8, cz * SECTION_SIZE + 8);
        let friendly = matches!(info.biome, Biome::Plains | Biome::Desert);
        if !friendly || info.steep || info.surface <= BEACH_TOP || info.surface > 48 {
            return None;
        }
        Some(ChunkPos::new(cx, cz))
    }

    /// Two-phase village placement, phase 2: the house anchored in this
    /// chunk (floor-center block, one above the surface), if any. Houses
    /// cluster within 2 chunks of the village center and only stand on
    /// ground that is flat across their footprint.
    pub fn house_origins(&self, chunk: ChunkPos) -> Vec<BlockPos> {
        let region = (chunk.x.div_euclid(VILLAGE_REGION), chunk.z.div_euclid(VILLAGE_REGION));
        let Some(center) = self.village_center(region.0, region.1) else {
            return Vec::new();
        };
        if (chunk.x - center.x).abs() > 2 || (chunk.z - center.z).abs() > 2 {
            return Vec::new();
        }
        let bits = lattice_bits(self.seed ^ SEED_HOUSE, chunk.x, chunk.z);
        if bits % 8 >= 5 {
            return Vec::new(); // ~5 of 8 in-range chunks build a house
        }
        // Keep the origin 4..=11 inside the chunk: houses from adjacent
        // chunks can never overlap (min center spacing 9 > footprint 7).
        let ox = 4 + ((bits >> 8) % 8) as i32;
        let oz = 4 + ((bits >> 16) % 8) as i32;
        let (x, z) = (chunk.x * SECTION_SIZE + ox, chunk.z * SECTION_SIZE + oz);
        let anchor = self.column(x, z);
        if !matches!(anchor.biome, Biome::Plains | Biome::Desert | Biome::Forest)
            || anchor.surface <= BEACH_TOP
            || anchor.surface > 48
        {
            return Vec::new();
        }
        // Flat-enough check across the footprint corners.
        for (dx, dz) in [(-3, -3), (3, -3), (-3, 3), (3, 3)] {
            let h = self.surface_height(x + dx, z + dz);
            if (h - anchor.surface).abs() > 2 {
                return Vec::new();
            }
        }
        vec![IVec3::new(x, anchor.surface + 1, z)]
    }

    /// The blocks of one house. Unlike trees these are authoritative:
    /// AIR entries carve the interior out of any terrain bump.
    pub fn house_blocks(&self, origin: BlockPos) -> Vec<(BlockPos, BlockId)> {
        let mut out = Vec::new();
        for dx in -3i32..=3 {
            for dz in -3i32..=3 {
                let edge = dx.abs() == 3 || dz.abs() == 3;
                // Floor slab, with a short foundation skirt under the edges
                // so sloped ground doesn't leave gaps.
                out.push((origin + IVec3::new(dx, -1, dz), blocks::PLANKS));
                if edge {
                    for dy in -3..-1 {
                        out.push((origin + IVec3::new(dx, dy, dz), blocks::PLANKS));
                    }
                }
                for dy in 0..3 {
                    let pos = origin + IVec3::new(dx, dy, dz);
                    if edge {
                        let corner = dx.abs() == 3 && dz.abs() == 3;
                        // Doorway: a 1×2 gap in the south wall center.
                        let door = dz == 3 && dx == 0 && dy < 2;
                        let block = if corner {
                            blocks::LOG
                        } else if door {
                            BlockId::AIR
                        } else {
                            blocks::PLANKS
                        };
                        out.push((pos, block));
                    } else {
                        // Interior space, carved even through terrain.
                        out.push((pos, BlockId::AIR));
                    }
                }
                // Flat roof.
                out.push((origin + IVec3::new(dx, 3, dz), blocks::PLANKS));
            }
        }
        // A lamp on the floor in the back corner keeps the inside lit.
        out.push((origin + IVec3::new(2, 0, -2), blocks::LAMP));
        out
    }

    /// True where noise carves a cave out of solid terrain. Two systems,
    /// "cheese" caverns (squashed 3D noise over a threshold)
    /// and "spaghetti" tunnels (the neighborhood of the intersection of two
    /// noise zero-surfaces — thin sheets crossing make winding 1D tunnels).
    pub fn is_cave(&self, pos: BlockPos, surface: i32) -> bool {
        // Keep the world floor and shorelines intact: no holes in the
        // bottom sections, none puncturing beach/ocean/river floors.
        if pos.y <= (BOTTOM_SECTION_Y + 1) * SECTION_SIZE {
            return false;
        }
        if surface <= BEACH_TOP && pos.y > surface - 6 {
            return false;
        }
        let depth = surface - pos.y;

        // Cheese caverns: entrances exist but are rare (the threshold
        // relaxes with depth); vertically squashed -> wide caverns.
        let threshold = if depth < 8 { 0.52 } else { 0.34 };
        let d = fbm3(
            self.seed ^ SEED_CHEESE,
            pos.x as f64 / 36.0,
            pos.y as f64 / 22.0,
            pos.z as f64 / 36.0,
            3,
        );
        if d > threshold {
            return true;
        }

        // Spaghetti tunnels, widening with depth, never surface-piercing.
        if depth >= 5 {
            let t = 0.05 + 0.028 * ramp(depth as f64, 5.0, 24.0);
            let s1 = fbm3(
                self.seed ^ SEED_SPAGHETTI_A,
                pos.x as f64 / 110.0,
                pos.y as f64 / 52.0,
                pos.z as f64 / 110.0,
                2,
            );
            if s1.abs() >= t {
                return false; // cheap early-out before the second channel
            }
            let s2 = fbm3(
                self.seed ^ SEED_SPAGHETTI_B,
                pos.x as f64 / 110.0,
                pos.y as f64 / 52.0,
                pos.z as f64 / 110.0,
                2,
            );
            return s2.abs() < t;
        }
        false
    }

    /// The blocks of one tree (trunk + canopy) rooted at `origin`.
    pub fn tree_blocks(&self, origin: BlockPos) -> Vec<(BlockPos, BlockId)> {
        let h = lattice_bits(self.seed ^ SEED_TREE_SHAPE, origin.x, origin.z);
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

// --- terrain shaper splines ------------------------------------------------

/// Erosion spline knots (low = mountainous, high = flat).
const E_KNOTS: [f64; 6] = [-1.0, -0.5, -0.2, 0.05, 0.45, 1.0];
/// Peaks & valleys knots (−1 = valley/river, +1 = ridge crest).
const PV_KNOTS: [f64; 6] = [-1.0, -0.85, -0.2, 0.2, 0.7, 1.0];

/// Inland surface height (blocks above sea level) by erosion × PV.
/// Rows follow `E_KNOTS`, columns follow `PV_KNOTS`. Valley columns go
/// negative so rivers carve themselves; the low-erosion row is mountain
/// country (plus jaggedness on top of it).
const LAND_TABLE: [[f64; 6]; 6] = [
    [12.0, 16.0, 40.0, 70.0, 120.0, 160.0], // e −1.0: extreme mountains (no rivers)
    [-6.0, -2.0, 22.0, 36.0, 60.0, 80.0],   // e −0.5: gorges through high hills
    [-5.0, -1.0, 12.0, 20.0, 34.0, 44.0],   // e −0.2: rugged hills
    [-4.0, 0.0, 7.0, 11.0, 17.0, 23.0],     // e  0.05: rolling lowlands
    [-3.0, -1.0, 4.0, 6.0, 9.0, 12.0],      // e  0.45: plains
    [-2.0, -1.0, 2.0, 3.0, 4.0, 6.0],       // e  1.0: flats / wide shallow rivers
];

/// Inland height for an (erosion, PV) pair: a PV spline at each erosion
/// knot, then a spline across erosion — vanilla's nested-spline shape.
fn land_height(erosion: f64, pv: f64) -> f64 {
    let mut by_erosion = [0.0; 6];
    for (row, values) in LAND_TABLE.iter().enumerate() {
        by_erosion[row] = spline(&PV_KNOTS, values, pv);
    }
    spline(&E_KNOTS, &by_erosion, erosion)
}

/// The continentalness spline: fixed ocean-floor knots, then land knots
/// that scale the inland height up from the coast (0.25× at the shore
/// ramping past 1× far inland — coastal cliffs appear by themselves where
/// the inland value is mountainous).
fn continent_height(continentalness: f64, land: f64) -> f64 {
    let xs = [-1.05, -0.55, -0.42, -0.19, -0.10, 0.03, 0.30, 1.00];
    let ys = [
        -42.0,
        -38.0,
        -20.0,
        -12.0,
        0.25 * land,
        0.55 * land,
        land,
        1.15 * land,
    ];
    spline(&xs, &ys, continentalness)
}

/// Piecewise smoothstep spline through (xs, ys) knots; clamps outside.
/// Zero slope at every knot keeps it monotone between knots (no
/// Catmull-Rom overshoot), which is what terrain plateaus want.
fn spline(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    for i in 0..xs.len() - 1 {
        if x <= xs[i + 1] {
            let t = fade((x - xs[i]) / (xs[i + 1] - xs[i]));
            return lerp(ys[i], ys[i + 1], t);
        }
    }
    *ys.last().unwrap()
}

/// Smoothstepped 0..1 ramp of `v` across [a, b].
fn ramp(v: f64, a: f64, b: f64) -> f64 {
    fade(((v - a) / (b - a)).clamp(0.0, 1.0))
}

// --- noise primitives -------------------------------------------------------

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
    use std::collections::HashMap;

    #[test]
    fn pv_fold_matches_the_mc_formula() {
        let g = TerrainGenerator::new(1);
        // Valleys at w = 0, peaks at |w| = 2/3, mid at |w| = 1/3 and 1.
        // We can't pick w directly, so check the formula on raw values.
        let pv = |w: f64| (1.0 - (3.0 * w.abs() - 2.0).abs()).clamp(-1.0, 1.0);
        assert_eq!(pv(0.0), -1.0);
        assert!((pv(2.0 / 3.0) - 1.0).abs() < 1e-12);
        assert!(pv(1.0 / 3.0).abs() < 1e-12);
        assert!(pv(1.0).abs() < 1e-12);
        // And that the sampled channel agrees with its own weirdness.
        for (x, z) in [(0, 0), (5000, -3000), (-12345, 999)] {
            let c = g.climate(x, z);
            assert!((c.pv - pv(c.weirdness)).abs() < 1e-12);
        }
    }

    #[test]
    fn heights_and_columns_are_deterministic() {
        let a = TerrainGenerator::new(42);
        let b = TerrainGenerator::new(42);
        for (x, z) in [(0, 0), (1000, -1000), (-31, 17), (i32::MAX / 2, i32::MIN / 2)] {
            assert_eq!(a.surface_height(x, z), b.surface_height(x, z));
            assert_eq!(a.column(x, z), b.column(x, z));
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
        for x in (-8192..8192).step_by(127) {
            for z in (-8192..8192).step_by(131) {
                let h = g.surface_height(x, z);
                assert!((-96..=300).contains(&h), "height {h} at ({x},{z}) out of band");
            }
        }
    }

    #[test]
    fn oceans_are_deep_where_continentalness_is_low() {
        let g = TerrainGenerator::new(42);
        let mut deep = 0;
        for x in (-8192..8192).step_by(97) {
            for z in (-8192..8192).step_by(101) {
                let c = g.climate(x, z);
                if c.continentalness < -0.55 {
                    deep += 1;
                    let h = g.surface_height(x, z);
                    assert!(h < -15, "deep ocean floor too shallow: {h} at ({x},{z})");
                }
            }
        }
        assert!(deep > 50, "expected real deep-ocean area, found {deep} samples");
    }

    #[test]
    fn the_full_biome_range_occurs() {
        let g = TerrainGenerator::new(42);
        let mut seen: HashMap<Biome, usize> = HashMap::new();
        for x in (-12288..12288).step_by(89) {
            for z in (-12288..12288).step_by(97) {
                *seen.entry(g.biome(x, z)).or_default() += 1;
            }
        }
        for required in [
            Biome::DeepOcean,
            Biome::Ocean,
            Biome::Beach,
            Biome::River,
            Biome::Plains,
            Biome::Forest,
            Biome::Desert,
        ] {
            assert!(seen.contains_key(&required), "missing {required:?}; saw {seen:?}");
        }
        let cold = seen.contains_key(&Biome::SnowyPlains) || seen.contains_key(&Biome::SnowyTaiga);
        assert!(cold, "no snowy biomes; saw {seen:?}");
        let peaks = seen.contains_key(&Biome::SnowyPeaks) || seen.contains_key(&Biome::StonyPeaks);
        assert!(peaks, "no peak biomes; saw {seen:?}");
    }

    #[test]
    fn rivers_are_connected_water_lines_inland() {
        let g = TerrainGenerator::new(42);
        let mut rivers = 0;
        let mut dry_valleys = 0;
        for x in (-6000..6000).step_by(13) {
            for z in (-6000..6000).step_by(17) {
                let c = g.climate(x, z);
                if c.continentalness > 0.05 && c.pv < -0.9 && c.erosion > -0.3 {
                    if g.surface_height(x, z) < SEA_LEVEL {
                        rivers += 1;
                    } else {
                        dry_valleys += 1;
                    }
                }
            }
        }
        assert!(rivers > 100, "expected inland river water, found {rivers}");
        // Valley centerlines should mostly be wet where erosion is normal.
        assert!(
            rivers > dry_valleys,
            "valley floors mostly dry: {rivers} wet vs {dry_valleys} dry"
        );
    }

    #[test]
    fn peaks_rise_high_where_the_shaper_says_so() {
        let g = TerrainGenerator::new(42);
        let mut tallest = i32::MIN;
        for x in (-12288..12288).step_by(53) {
            for z in (-12288..12288).step_by(59) {
                tallest = tallest.max(g.surface_height(x, z));
            }
        }
        assert!(tallest > 100, "no real mountains found; tallest {tallest}");
    }

    #[test]
    fn surface_rules_per_biome() {
        let g = TerrainGenerator::new(7);
        let info = |surface, biome, steep| ColumnInfo { surface, biome, steep };

        // Grassland family: grass, dirt, stone.
        let plains = info(10, Biome::Plains, false);
        assert_eq!(g.block_in_column(&plains, 11), blocks::AIR);
        assert_eq!(g.block_in_column(&plains, 10), blocks::GRASS);
        assert_eq!(g.block_in_column(&plains, 8), blocks::DIRT);
        assert_eq!(g.block_in_column(&plains, 6), blocks::STONE);

        // Desert and beach: sand cap.
        assert_eq!(g.block_in_column(&info(20, Biome::Desert, false), 20), blocks::SAND);
        assert_eq!(g.block_in_column(&info(2, Biome::Beach, false), 2), blocks::SAND);

        // Snow caps and snowy flats.
        assert_eq!(g.block_in_column(&info(120, Biome::SnowyPeaks, false), 120), blocks::SNOW);
        assert_eq!(g.block_in_column(&info(120, Biome::SnowyPeaks, false), 119), blocks::STONE);
        assert_eq!(g.block_in_column(&info(12, Biome::SnowyPlains, false), 12), blocks::SNOW);
        assert_eq!(g.block_in_column(&info(12, Biome::SnowyPlains, false), 11), blocks::DIRT);

        // Cliffs expose stone whatever the biome.
        assert_eq!(g.block_in_column(&info(30, Biome::Forest, true), 30), blocks::STONE);
        // Stony biomes are stone-topped.
        assert_eq!(g.block_in_column(&info(80, Biome::StonyPeaks, false), 80), blocks::STONE);
        assert_eq!(g.block_in_column(&info(1, Biome::StonyShore, false), 1), blocks::STONE);

        // Underwater: sandy shallows, stone deeps; water above.
        let shallows = info(-4, Biome::Ocean, false);
        assert_eq!(g.block_in_column(&shallows, -4), blocks::SAND);
        assert_eq!(g.block_in_column(&shallows, -8), blocks::STONE);
        assert_eq!(g.block_in_column(&shallows, -2), blocks::WATER);
        assert_eq!(g.block_in_column(&shallows, SEA_LEVEL), blocks::WATER);
        assert_eq!(g.block_in_column(&shallows, SEA_LEVEL + 1), blocks::AIR);
        let abyss = info(-35, Biome::DeepOcean, false);
        assert_eq!(g.block_in_column(&abyss, -35), blocks::STONE);
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
    fn spaghetti_tunnels_exist_below_the_entrance_band() {
        let g = TerrainGenerator::new(42);
        // Count cave cells that the cheese channel alone would NOT carve:
        // those are spaghetti tunnels.
        let mut spaghetti = 0;
        for x in (-400..400).step_by(3) {
            for z in (-400..400).step_by(5) {
                let surface = g.surface_height(x, z);
                let y = surface - 30;
                if y <= (BOTTOM_SECTION_Y + 1) * SECTION_SIZE {
                    continue;
                }
                let cheese = fbm3(
                    g.seed ^ SEED_CHEESE,
                    x as f64 / 36.0,
                    y as f64 / 22.0,
                    z as f64 / 36.0,
                    3,
                ) > 0.34;
                if g.is_cave(IVec3::new(x, y, z), surface) && !cheese {
                    spaghetti += 1;
                }
            }
        }
        assert!(spaghetti > 50, "expected spaghetti tunnels, found {spaghetti} cells");
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
    fn trees_are_deterministic_grounded_and_biome_gated() {
        let g = TerrainGenerator::new(42);
        let mut found = 0;
        for cx in -24..24 {
            for cz in -24..24 {
                let chunk = oc_core::ChunkPos::new(cx, cz);
                let a = g.tree_origins(chunk);
                let b = g.tree_origins(chunk);
                assert_eq!(a, b, "tree origins must be deterministic");
                for origin in a {
                    found += 1;
                    let info = g.column(origin.x, origin.z);
                    assert_eq!(origin.y, info.surface + 1);
                    assert!(info.surface > BEACH_TOP, "tree on beach/underwater at {origin}");
                    assert!(
                        matches!(
                            info.biome,
                            Biome::Forest
                                | Biome::Taiga
                                | Biome::SnowyTaiga
                                | Biome::Plains
                                | Biome::SnowyPlains
                        ),
                        "tree in {:?} at {origin}",
                        info.biome
                    );
                    let blocks_list = g.tree_blocks(origin);
                    assert!(blocks_list.len() > 10, "tree too small");
                    assert!(blocks_list.iter().any(|(_, b)| *b == blocks::LOG));
                    assert!(blocks_list.iter().any(|(_, b)| *b == blocks::LEAVES));
                }
            }
        }
        assert!(found > 10, "expected a healthy number of trees, got {found}");
    }

    #[test]
    fn villages_exist_and_are_deterministic() {
        let g = TerrainGenerator::new(42);
        let mut villages = 0;
        for rx in -40..40 {
            for rz in -40..40 {
                assert_eq!(g.village_center(rx, rz), g.village_center(rx, rz));
                if let Some(center) = g.village_center(rx, rz) {
                    villages += 1;
                    // Centers sit inside their own region on friendly land.
                    assert_eq!(center.x.div_euclid(VILLAGE_REGION), rx);
                    assert_eq!(center.z.div_euclid(VILLAGE_REGION), rz);
                    let info = g.column(center.x * 16 + 8, center.z * 16 + 8);
                    assert!(matches!(info.biome, Biome::Plains | Biome::Desert));
                }
            }
        }
        // 6400 regions × (1/3 roll) × biome/flatness gate: plenty survive.
        assert!(villages > 40, "expected villages across the map, found {villages}");
    }

    #[test]
    fn houses_cluster_near_their_village_and_never_overlap() {
        let g = TerrainGenerator::new(42);
        let mut houses_total = 0;
        let mut villages_with_houses = 0;
        'regions: for rx in -40..40 {
            for rz in -40..40 {
                let Some(center) = g.village_center(rx, rz) else { continue };
                let mut origins = Vec::new();
                for dcx in -4..=4 {
                    for dcz in -4..=4 {
                        let chunk = oc_core::ChunkPos::new(center.x + dcx, center.z + dcz);
                        for origin in g.house_origins(chunk) {
                            // Phase 2 only builds within 2 chunks of center.
                            assert!(dcx.abs() <= 2 && dcz.abs() <= 2, "stray house at {origin}");
                            origins.push(origin);
                        }
                    }
                }
                for (i, a) in origins.iter().enumerate() {
                    for b in &origins[i + 1..] {
                        let gap = (a.x - b.x).abs().max((a.z - b.z).abs());
                        assert!(gap >= 8, "houses overlap: {a} vs {b}");
                    }
                }
                houses_total += origins.len();
                if !origins.is_empty() {
                    villages_with_houses += 1;
                }
                if houses_total > 60 && villages_with_houses > 10 {
                    break 'regions;
                }
            }
        }
        assert!(villages_with_houses > 10, "most villages should have houses");
        assert!(houses_total > 60, "expected a healthy housing stock, got {houses_total}");
    }

    #[test]
    fn house_blocks_form_a_lit_walled_room() {
        let g = TerrainGenerator::new(42);
        let origin = IVec3::new(100, 20, 100);
        let list = g.house_blocks(origin);
        // Last write wins, matching the overlay map's insert order.
        let get = |d: IVec3| {
            list.iter().rev().find(|(p, _)| *p == origin + d).map(|(_, b)| *b)
        };
        assert_eq!(get(IVec3::new(0, 0, 3)), Some(BlockId::AIR), "doorway bottom");
        assert_eq!(get(IVec3::new(0, 1, 3)), Some(BlockId::AIR), "doorway top");
        assert_eq!(get(IVec3::new(3, 0, 3)), Some(blocks::LOG), "corner post");
        assert_eq!(get(IVec3::new(1, 0, 3)), Some(blocks::PLANKS), "wall");
        assert_eq!(get(IVec3::new(0, 3, 0)), Some(blocks::PLANKS), "roof");
        assert_eq!(get(IVec3::new(0, -1, 0)), Some(blocks::PLANKS), "floor");
        assert_eq!(get(IVec3::new(0, 0, 0)), Some(BlockId::AIR), "interior");
        assert_eq!(get(IVec3::new(2, 0, -2)), Some(blocks::LAMP), "lamp");
    }

    #[test]
    fn forests_are_denser_than_plains() {
        let g = TerrainGenerator::new(42);
        let mut forest_trees = 0;
        let mut forest_chunks = 0;
        let mut plains_trees = 0;
        let mut plains_chunks = 0;
        for cx in -64..64 {
            for cz in -64..64 {
                let chunk = oc_core::ChunkPos::new(cx, cz);
                let center = g.biome(cx * 16 + 8, cz * 16 + 8);
                match center {
                    Biome::Forest => {
                        forest_chunks += 1;
                        forest_trees += g.tree_origins(chunk).len();
                    }
                    Biome::Plains => {
                        plains_chunks += 1;
                        plains_trees += g.tree_origins(chunk).len();
                    }
                    _ => {}
                }
            }
        }
        assert!(forest_chunks > 20 && plains_chunks > 20, "not enough sample chunks");
        let forest_density = forest_trees as f64 / forest_chunks as f64;
        let plains_density = plains_trees as f64 / plains_chunks as f64;
        assert!(
            forest_density > plains_density * 3.0,
            "forest {forest_density:.2} trees/chunk vs plains {plains_density:.2}"
        );
    }
}
