//! Procedural placeholder block textures, until the asset pipeline (§7.5)
//! provides real ones.

pub const TEXTURE_SIZE: u32 = 16;
pub const LAYER_COUNT: u32 = 12;

/// RGBA pixels for the block texture array. Layer order must match
/// `mesh::layers`: grass top, dirt, stone, grass side, sand, water,
/// log side, log top, leaves, lamp, snow, planks.
pub fn build_block_textures() -> Vec<u8> {
    let size = TEXTURE_SIZE as usize;
    let mut pixels = Vec::with_capacity(size * size * 4 * LAYER_COUNT as usize);
    for layer in 0..LAYER_COUNT {
        for y in 0..size {
            for x in 0..size {
                let n = hash_noise(x as u32, y as u32, layer);
                let rgb = match layer {
                    0 => shade([106, 170, 64], n, 24),
                    1 => shade([134, 96, 67], n, 20),
                    2 => shade([125, 125, 125], n, 18),
                    // Grass side: dirt with a grass strip on the top edge.
                    3 if y < 3 => shade([106, 170, 64], n, 24),
                    3 => shade([134, 96, 67], n, 20),
                    4 => shade([219, 209, 160], n, 14),
                    5 => shade([54, 106, 224], n, 10),
                    // Log side: vertical bark streaks.
                    6 => shade([104, 82, 50], hash_noise(x as u32, 0, layer), 22),
                    7 => shade([151, 122, 73], n, 16),
                    8 => shade([58, 134, 52], n, 30),
                    10 => shade([238, 242, 248], n, 8),
                    // Planks: horizontal board stripes.
                    11 if y % 4 == 0 => shade([142, 110, 68], n, 8),
                    11 => shade([172, 136, 84], hash_noise(0, y as u32 / 4, layer), 14),
                    // Lamp: bright warm glow with a darker rim.
                    _ if x == 0 || y == 0 || x == size - 1 || y == size - 1 => {
                        shade([142, 105, 55], n, 12)
                    }
                    _ => shade([255, 222, 150], n, 12),
                };
                pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }
    pixels
}

fn shade(base: [u8; 3], noise: u32, amplitude: i32) -> [u8; 3] {
    let delta = (noise % (2 * amplitude as u32 + 1)) as i32 - amplitude;
    base.map(|c| (c as i32 + delta).clamp(0, 255) as u8)
}

/// Representative color of a block for UI swatches (hotbar icons until
/// the asset pipeline brings real item icons). sRGB 0..1 with alpha.
pub fn block_swatch(block: oc_world::BlockId) -> [f32; 4] {
    use oc_world::blocks;
    let rgb: [u32; 3] = match block {
        blocks::GRASS => [106, 170, 64],
        blocks::DIRT => [134, 96, 67],
        blocks::STONE => [125, 125, 125],
        blocks::SAND => [219, 209, 160],
        blocks::WATER => [54, 106, 224],
        blocks::LOG => [104, 82, 50],
        blocks::LEAVES => [58, 134, 52],
        blocks::LAMP => [255, 222, 150],
        blocks::SNOW => [238, 242, 248],
        blocks::PLANKS => [172, 136, 84],
        _ => [255, 0, 255],
    };
    [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0, 1.0]
}

/// Small deterministic integer hash (xorshift-style) for texture grain.
fn hash_noise(x: u32, y: u32, layer: u32) -> u32 {
    let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ layer.wrapping_mul(2246822519);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^ (h >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_palettes_are_distinct() {
        let pixels = build_block_textures();
        let size = (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize;
        assert_eq!(pixels.len(), size * LAYER_COUNT as usize);
        let avg = |layer: usize| -> [u32; 3] {
            let mut sum = [0u32; 3];
            for px in pixels[layer * size..(layer + 1) * size].chunks(4) {
                for c in 0..3 {
                    sum[c] += px[c] as u32;
                }
            }
            sum.map(|s| s / (TEXTURE_SIZE * TEXTURE_SIZE))
        };
        let grass = avg(0);
        assert!(grass[1] > grass[0] && grass[1] > grass[2], "grass top not green: {grass:?}");
        let sand = avg(4);
        assert!(sand[0] > 180 && sand[2] < 180, "sand not beige: {sand:?}");
        let water = avg(5);
        assert!(water[2] > 150 && water[2] > water[0], "water not blue: {water:?}");
        let leaves = avg(8);
        assert!(leaves[1] > leaves[0], "leaves not green: {leaves:?}");
    }
}
