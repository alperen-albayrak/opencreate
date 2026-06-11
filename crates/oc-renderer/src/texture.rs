//! Procedural placeholder block textures, until the asset pipeline (§7.5)
//! provides real ones.

pub const TEXTURE_SIZE: u32 = 16;
pub const LAYER_COUNT: u32 = 4;

/// RGBA pixels for the block texture array. Layer order must match
/// `mesh::layers`: grass top, dirt, stone, grass side.
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
                    _ if y < 3 => shade([106, 170, 64], n, 24),
                    _ => shade([134, 96, 67], n, 20),
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

/// Small deterministic integer hash (xorshift-style) for texture grain.
fn hash_noise(x: u32, y: u32, layer: u32) -> u32 {
    let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ layer.wrapping_mul(2246822519);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^ (h >> 16)
}
