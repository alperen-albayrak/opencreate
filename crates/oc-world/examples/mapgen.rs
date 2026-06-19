//! Offline worldgen visualizer (dev tool): renders a top-down biome map
//! with hillshading and an underground cross-section as PPM images, so
//! terrain changes can be eyeballed without launching the game.
//!
//!     cargo run -p oc-world --release --example mapgen [seed]
//!
//! Writes map.ppm and section.ppm into the working directory.

use std::fs::File;
use std::io::{BufWriter, Write};

use oc_world::blocks;
use oc_world::terrain::{Biome, SEA_LEVEL, TerrainGenerator};

fn main() {
    let seed = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20260612);
    let generator = TerrainGenerator::new(seed);
    top_down(&generator, "map.ppm", 1024, 4);
    cross_section(&generator, "section.ppm");
    village_view(&generator, seed, "village.ppm");
    println!("wrote map.ppm (4096x4096 blocks), section.ppm and village.ppm for seed {seed}");
}

/// Top-down render of real generated blocks around the first village
/// found that has at least 3 houses (4 px per block).
fn village_view(generator: &TerrainGenerator, seed: u64, path: &str) {
    let mut found = None;
    'regions: for r in 0i32..40 {
        for rx in -r..=r {
            for rz in -r..=r {
                if rx.abs() != r && rz.abs() != r {
                    continue;
                }
                if let Some(center) = generator.village_center(rx, rz) {
                    let houses: usize = (-2..=2)
                        .flat_map(|dx| (-2..=2).map(move |dz| (dx, dz)))
                        .map(|(dx, dz)| {
                            generator
                                .house_origins(oc_core::ChunkPos::new(center.x + dx, center.z + dz))
                                .len()
                        })
                        .sum();
                    if houses >= 3 {
                        found = Some(center);
                        break 'regions;
                    }
                }
            }
        }
    }
    let Some(center) = found else {
        println!("no village with 3+ houses found; skipping {path}");
        return;
    };
    let mut world = oc_world::World::new(seed);
    for dx in -3..=3 {
        for dz in -3..=3 {
            world.generate_column(oc_core::ChunkPos::new(center.x + dx, center.z + dz));
        }
    }
    const BLOCKS: i32 = 96;
    const PX: usize = 4;
    let size = (BLOCKS as usize) * PX;
    let base_x = center.x * 16 + 8 - BLOCKS / 2;
    let base_z = center.z * 16 + 8 - BLOCKS / 2;
    let mut out = BufWriter::new(File::create(path).unwrap());
    writeln!(out, "P6\n{size} {size}\n255").unwrap();
    for pz in 0..size {
        for px in 0..size {
            let x = base_x + (px / PX) as i32;
            let z = base_z + (pz / PX) as i32;
            // Topmost non-air block.
            let mut color = [0u8; 3];
            for y in (-16..120).rev() {
                let b = world.block(glam::IVec3::new(x, y, z));
                if !b.is_air() {
                    color = if b == blocks::WATER {
                        [40, 90, 170]
                    } else if b == blocks::GRASS {
                        [100, 160, 70]
                    } else if b == blocks::SAND {
                        [216, 200, 150]
                    } else if b == blocks::PLANKS {
                        [196, 148, 84]
                    } else if b == blocks::LOG {
                        [96, 66, 40]
                    } else if b == blocks::LEAVES {
                        [40, 96, 36]
                    } else if b == blocks::SNOW {
                        [240, 244, 250]
                    } else if b == blocks::DIRT {
                        [120, 85, 58]
                    } else {
                        [110, 110, 110]
                    };
                    break;
                }
            }
            out.write_all(&color).unwrap();
        }
    }
}

fn biome_color(biome: Biome) -> [f64; 3] {
    match biome {
        Biome::DeepOcean => [10.0, 40.0, 96.0],
        Biome::Ocean => [24.0, 72.0, 144.0],
        Biome::River => [56.0, 116.0, 190.0],
        Biome::Beach => [216.0, 200.0, 150.0],
        Biome::StonyShore => [128.0, 128.0, 128.0],
        Biome::Plains => [112.0, 172.0, 80.0],
        Biome::Forest => [56.0, 128.0, 52.0],
        Biome::Taiga => [72.0, 140.0, 108.0],
        Biome::SnowyPlains => [234.0, 240.0, 246.0],
        Biome::SnowyTaiga => [198.0, 214.0, 204.0],
        Biome::Desert => [226.0, 206.0, 130.0],
        Biome::StonyPeaks => [152.0, 152.0, 152.0],
        Biome::SnowyPeaks => [250.0, 250.0, 255.0],
    }
}

/// Top-down biome map, hillshaded from the west, water darkened by depth.
fn top_down(generator: &TerrainGenerator, path: &str, pixels: usize, scale: i32) {
    let mut out = BufWriter::new(File::create(path).unwrap());
    writeln!(out, "P6\n{pixels} {pixels}\n255").unwrap();
    let half = (pixels as i32 / 2) * scale;
    for pz in 0..pixels {
        for px in 0..pixels {
            let x = px as i32 * scale - half;
            let z = pz as i32 * scale - half;
            let h = generator.surface_height(x, z);
            let biome = generator.biome(x, z);
            let mut color = biome_color(biome);
            if h < SEA_LEVEL && matches!(biome, Biome::DeepOcean | Biome::Ocean | Biome::River) {
                // Deeper water is darker.
                let depth = ((-h) as f64 / 45.0).clamp(0.0, 1.0);
                for c in &mut color {
                    *c *= 1.0 - 0.5 * depth;
                }
            } else {
                // Hillshade from the slope toward the west neighbor.
                let west = generator.surface_height(x - scale, z);
                let slope = ((h - west) as f64 / scale as f64).clamp(-2.0, 2.0);
                let shade = 1.0 + slope * 0.18;
                // And a gentle altitude lift so plateaus read.
                let lift = 1.0 + (h.max(0) as f64 / 160.0) * 0.25;
                for c in &mut color {
                    *c *= shade * lift;
                }
            }
            let rgb: Vec<u8> = color.iter().map(|c| c.clamp(0.0, 255.0) as u8).collect();
            out.write_all(&rgb).unwrap();
        }
    }
}

/// Vertical slice along X at z = 64, top to the bedrock floor: terrain layers,
/// caves, the hellish caverns and the lava lake.
fn cross_section(generator: &TerrainGenerator, path: &str) {
    const Z: i32 = 64;
    const X_RANGE: i32 = 1024;
    const Y_TOP: i32 = 200;
    const Y_BOTTOM: i32 = -800;
    let width = (2 * X_RANGE) as usize;
    let height = (Y_TOP - Y_BOTTOM) as usize;
    let mut out = BufWriter::new(File::create(path).unwrap());
    writeln!(out, "P6\n{width} {height}\n255").unwrap();
    for row in 0..height {
        let y = Y_TOP - 1 - row as i32;
        for col in 0..width {
            let x = col as i32 - X_RANGE;
            let info = generator.column(x, Z);
            let pos = glam::IVec3::new(x, y, Z);
            let base = generator.block_in_column(&info, y);
            let block = generator.carve(pos, info.surface, base);
            // A solid block carved to air reads as a (dark) cave/hellish cavern.
            let carved = base.is_solid() && block.is_air();
            let color: [u8; 3] = if carved {
                [20, 12, 8] // cave / hellish cavern air
            } else if block == blocks::WATER {
                [40, 90, 170]
            } else if block == blocks::LAVA {
                [230, 110, 30] // lava lake
            } else if block == blocks::BEDROCK {
                [60, 58, 64] // bedrock floor
            } else if block == blocks::GRASS {
                [100, 160, 70]
            } else if block == blocks::DIRT {
                [120, 85, 58]
            } else if block == blocks::SAND {
                [216, 200, 150]
            } else if block == blocks::SNOW {
                [240, 244, 250]
            } else if block == blocks::STONE {
                [110, 110, 110]
            } else if block.is_air() {
                if y > info.surface { [185, 215, 240] } else { [20, 12, 8] }
            } else {
                [200, 0, 200]
            };
            out.write_all(&color).unwrap();
        }
    }
}
