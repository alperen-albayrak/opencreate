//! Block textures: procedural defaults, overridable by PNG files in a
//! resource pack (the §7.5 overlay stack). A pack drops
//! `data/textures/block/<name>.png` to replace a layer; missing or invalid
//! files fall back to the procedural default (the proven audio.rs / skins.ron
//! pattern).

use std::path::Path;

pub const TEXTURE_SIZE: u32 = 16;
pub const LAYER_COUNT: u32 = 25;
/// Mip levels for the block array: 16→8→4→2→1 = `floor(log2(16)) + 1`.
pub const MIP_LEVELS: u32 = 5;

/// Block texture array layer names, in array order (must match
/// `build_block_textures` and `mesh::layers`). A pack overrides a layer with
/// `data/textures/block/<name>.png`.
pub const LAYER_NAMES: [&str; LAYER_COUNT as usize] = [
    "grass_top", "dirt", "stone", "grass_side", "sand", "water", "log_side",
    "log_top", "leaves", "lamp", "snow", "planks", "bedrock", "lava",
    "obsidian", "basalt", "ice",
    // Tranche 1: metals, gem, ores, cobblestone, granite (layers 17..24).
    "iron_block", "copper_block", "gold_block", "diamond_block",
    "coal_ore", "iron_ore", "cobblestone", "granite",
];

/// Intrinsic emissive (blackbody-glow) temperature in °C per block-texture
/// layer, built from the registry: a fluid block (lava) stamps its texture
/// layers with the fluid's own temperature, so the geometry pass glows lava at
/// its real ~1200 °C (orange) instead of the ambient rock temperature (dull
/// red). 0 = no intrinsic glow → the surface uses the ambient height curve.
pub static EMISSIVE_TEMPS: std::sync::LazyLock<[f32; LAYER_COUNT as usize]> =
    std::sync::LazyLock::new(|| {
        let mut temps = [0.0f32; LAYER_COUNT as usize];
        for i in 0u16.. {
            let id = oc_world::BlockId(i);
            let Some(def) = oc_world::registry::def(id) else { break };
            if let Some(t) = oc_world::fluid_registry::for_block(id).and_then(|f| f.temperature) {
                for face in 0..6 {
                    let layer = def.textures.layer(face) as usize;
                    if layer < temps.len() {
                        temps[layer] = temps[layer].max(t);
                    }
                }
            }
        }
        temps
    });

/// Surface `(roughness, metalness)` per block-texture layer for specular PBR,
/// built from the registry exactly like [`EMISSIVE_TEMPS`]: a block stamps its
/// `roughness`/`metalness` onto every texture layer it uses. When two blocks
/// share a layer the shiniest (lowest) roughness and any metal flag win — the
/// visually dominant material. The matte default (roughness 1, metal 0) leaves
/// a layer's specular off.
pub static MATERIALS: std::sync::LazyLock<[(f32, f32); LAYER_COUNT as usize]> =
    std::sync::LazyLock::new(|| {
        let mut mats = [(1.0f32, 0.0f32); LAYER_COUNT as usize];
        for i in 0u16.. {
            let id = oc_world::BlockId(i);
            let Some(def) = oc_world::registry::def(id) else { break };
            for face in 0..6 {
                let layer = def.textures.layer(face) as usize;
                if layer < mats.len() {
                    mats[layer].0 = mats[layer].0.min(def.roughness);
                    mats[layer].1 = mats[layer].1.max(def.metalness);
                }
            }
        }
        mats
    });

/// Packs a surface `(roughness, metalness)` into the single free G-buffer
/// channel `GB1.w` (8-bit UNORM, so 256 codes). The top bit of the code (≥128)
/// is the metal flag; the low 7 bits are `roughness × 127`. Decoded in
/// `pbr.wgsl` as `code = round(w*255); metal = code >= 128;
/// roughness = (code & 127) / 127` — an integer split so a matte dielectric
/// (roughness 1, code 127) never aliases onto a shiny metal (code 128).
pub fn pack_material(roughness: f32, metalness: f32) -> f32 {
    let metal_bit = if metalness >= 0.5 { 128.0 } else { 0.0 };
    let code = metal_bit + (roughness.clamp(0.0, 1.0) * 127.0).round();
    code / 255.0
}

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
                    9 if x == 0 || y == 0 || x == size - 1 || y == size - 1 => {
                        shade([142, 105, 55], n, 12)
                    }
                    9 => shade([255, 222, 150], n, 12),
                    // Bedrock: dark, heavily mottled rock — visually distinct from
                    // stone so the impassable floor reads as different.
                    12 => shade([62, 60, 66], n, 30),
                    // Lava: molten orange-yellow with a darker crust where the
                    // noise dips (every 5th cell), giving a cracked surface.
                    13 if n % 5 == 0 => shade([122, 36, 8], n, 18),
                    13 => shade([226, 104, 26], n, 44),
                    // Obsidian: near-black volcanic glass, smooth, with the
                    // occasional brighter purple glint.
                    14 if n % 7 == 0 => shade([60, 48, 84], n, 10),
                    14 => shade([22, 18, 34], n, 8),
                    // Basalt: dark grey, finely mottled volcanic rock.
                    15 => shade([52, 50, 56], n, 20),
                    // Ice: pale blue, smooth, with faint brighter cracks.
                    16 if n % 6 == 0 => shade([205, 230, 255], n, 6),
                    16 => shade([165, 205, 240], n, 10),
                    // Iron block: light steel grey, faint metallic mottle.
                    17 => shade([198, 198, 205], n, 10),
                    // Copper block: warm reddish-orange metal.
                    18 => shade([190, 116, 70], n, 16),
                    // Gold block: bright warm yellow metal.
                    19 => shade([224, 184, 72], n, 12),
                    // Diamond block: pale cyan crystal with brighter facets.
                    20 if n % 5 == 0 => shade([205, 245, 250], n, 8),
                    20 => shade([130, 210, 225], n, 14),
                    // Coal ore: stone matrix with scattered near-black flecks.
                    21 if n % 4 == 0 => shade([34, 32, 34], n, 8),
                    21 => shade([125, 125, 125], n, 18),
                    // Iron ore: stone matrix with tan-orange ore flecks.
                    22 if n % 5 == 0 => shade([178, 146, 104], n, 14),
                    22 => shade([125, 125, 125], n, 18),
                    // Cobblestone: heavily mottled broken grey stone.
                    23 => shade([120, 120, 125], n, 32),
                    // Granite: pink-grey igneous with lighter crystal flecks.
                    24 if n % 6 == 0 => shade([198, 168, 152], n, 12),
                    24 => shade([150, 110, 102], n, 20),
                    // Unknown layer → magenta (matches the registry's missing tint).
                    _ => shade([255, 0, 255], n, 0),
                };
                pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }
    pixels
}

/// Builds the block texture array, overlaying any per-layer PNG overrides from
/// `data/textures/block/` on top of the procedural defaults. Each override must
/// be `TEXTURE_SIZE`² RGBA; anything else logs a warning and keeps the default.
pub fn load_block_textures() -> Vec<u8> {
    let mut pixels = build_block_textures();
    let layer_bytes = (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize;
    for (layer, name) in LAYER_NAMES.iter().enumerate() {
        if let Some(rgba) = load_override(&format!("data/textures/block/{name}.png")) {
            let start = layer * layer_bytes;
            pixels[start..start + layer_bytes].copy_from_slice(&rgba);
        }
    }
    pixels
}

/// Loads a 16×16 RGBA PNG override if present and valid, else `None`.
fn load_override(path: &str) -> Option<Vec<u8>> {
    if !Path::new(path).exists() {
        return None;
    }
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            if rgba.width() != TEXTURE_SIZE || rgba.height() != TEXTURE_SIZE {
                tracing::warn!(
                    path,
                    width = rgba.width(),
                    height = rgba.height(),
                    "block texture override is not {TEXTURE_SIZE}² — ignoring"
                );
                return None;
            }
            Some(rgba.into_raw())
        }
        Err(e) => {
            tracing::warn!(path, error = %e, "failed to decode block texture override");
            None
        }
    }
}

fn shade(base: [u8; 3], noise: u32, amplitude: i32) -> [u8; 3] {
    let delta = (noise % (2 * amplitude as u32 + 1)) as i32 - amplitude;
    base.map(|c| (c as i32 + delta).clamp(0, 255) as u8)
}

/// Representative color of a block for UI swatches (hotbar icons until
/// the asset pipeline brings real item icons). sRGB 0..1 with alpha.
/// The color is data now (`BlockDef.color`); unknown blocks read magenta.
pub fn block_swatch(block: oc_world::BlockId) -> [f32; 4] {
    let (r, g, b) = oc_world::registry::def(block).map_or((255, 0, 255), |d| d.color);
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
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
        let bedrock = avg(12);
        assert!(bedrock.iter().all(|&c| c < 110), "bedrock not dark: {bedrock:?}");
        let lava = avg(13);
        assert!(
            lava[0] > 150 && lava[0] > lava[1] && lava[1] > lava[2],
            "lava not molten-orange: {lava:?}"
        );
    }

    #[test]
    fn png_override_loads_and_rejects_wrong_size() {
        let dir = std::env::temp_dir().join(format!("oc-tex-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A correct 16×16 RGBA PNG decodes to the layer's raw bytes.
        let ok = dir.join("ok.png");
        image::RgbaImage::from_pixel(TEXTURE_SIZE, TEXTURE_SIZE, image::Rgba([10, 20, 30, 255]))
            .save(&ok)
            .unwrap();
        let bytes = load_override(ok.to_str().unwrap()).expect("16x16 png loads");
        assert_eq!(bytes.len(), (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize);
        assert_eq!(&bytes[0..4], &[10, 20, 30, 255]);

        // A wrong-sized PNG is rejected (falls back to procedural).
        let bad = dir.join("bad.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255])).save(&bad).unwrap();
        assert!(load_override(bad.to_str().unwrap()).is_none());

        // A missing file is simply no override.
        assert!(load_override(dir.join("missing.png").to_str().unwrap()).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn array_size_is_stable_with_no_overrides() {
        // With no pack present (test CWD has no data/textures/block/), the
        // overlay loader equals the procedural baseline, same total size.
        let array = load_block_textures();
        assert_eq!(
            array.len(),
            (TEXTURE_SIZE * TEXTURE_SIZE * 4 * LAYER_COUNT) as usize
        );
    }
}
