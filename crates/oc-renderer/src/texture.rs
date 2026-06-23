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
                    // Cobblestone: a cell pattern of domed stones + dark mortar
                    // (structured so its normal map reads as real relief).
                    23 => cobble(x, y),
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

/// Per-texel surface material (the "MER" array): R = metalness, G = roughness,
/// B/A reserved (per-texel emissive/subsurface land in a later step). Built from
/// the per-block [`MATERIALS`] table, but *per-texel* so a block's detail can
/// vary within one face — e.g. the iron-bearing flecks of iron ore read as raw
/// metal while the stone matrix stays matte. Sampled in `chunk_gbuffer.wgsl` and
/// packed into `GB1.w` (the same 8-bit code `pbr.wgsl` decodes), superseding the
/// per-layer Scene-UBO material lookup. Linear data — upload as UNORM, not SRGB.
pub fn build_block_mer() -> Vec<u8> {
    let size = TEXTURE_SIZE as usize;
    let mut pixels = Vec::with_capacity(size * size * 4 * LAYER_COUNT as usize);
    for layer in 0..LAYER_COUNT {
        let (base_rough, base_metal) = MATERIALS[layer as usize];
        for y in 0..size {
            for x in 0..size {
                let n = hash_noise(x as u32, y as u32, layer);
                let (rough, metal) = match layer {
                    // Iron ore (layer 22): the iron flecks — the same noise mask
                    // build_block_textures uses for the tan specks — read as raw
                    // metal; the surrounding stone matrix stays matte dielectric.
                    22 if n % 5 == 0 => (0.45_f32, 1.0_f32),
                    _ => (base_rough, base_metal),
                };
                pixels.push((metal.clamp(0.0, 1.0) * 255.0).round() as u8);
                pixels.push((rough.clamp(0.0, 1.0) * 255.0).round() as u8);
                pixels.push(0);
                pixels.push(0);
            }
        }
    }
    pixels
}

/// The MER array with any per-layer `_mer.png` pack overrides applied. (The
/// override overlay lands in a later sub-step; for now this is the procedural
/// build.)
pub fn load_block_mer() -> Vec<u8> {
    build_block_mer()
}

/// Per-texel tangent-space normal map array (RGB = normal·0.5+0.5; flat =
/// (128,128,255) = no relief) with the **heightfield in the alpha channel**.
/// Derived from each layer's albedo luminance via wrapped central differences
/// (seam-free across merged quads), so a texture's visible grain lights with
/// matching relief under the moving sun (RGB), and parallax occlusion mapping
/// can march the height (A) for true view-dependent depth. Combined with a
/// per-face tangent frame in `chunk_gbuffer.wgsl`. Linear — upload as UNORM.
pub fn build_block_normals() -> Vec<u8> {
    let color = build_block_textures();
    let size = TEXTURE_SIZE as usize;
    let layer_bytes = size * size * 4;
    let mut out = Vec::with_capacity(layer_bytes * LAYER_COUNT as usize);
    // Bump depth: relief strength. Modest so blocks read as textured, not lumpy.
    const STRENGTH: f32 = 1.5;
    for layer in 0..LAYER_COUNT as usize {
        let base = layer * layer_bytes;
        let height = |x: i32, y: i32| -> f32 {
            let xx = x.rem_euclid(size as i32) as usize;
            let yy = y.rem_euclid(size as i32) as usize;
            let p = base + (yy * size + xx) * 4;
            (0.299 * color[p] as f32 + 0.587 * color[p + 1] as f32 + 0.114 * color[p + 2] as f32)
                / 255.0
        };
        for y in 0..size as i32 {
            for x in 0..size as i32 {
                // Central difference → tangent-space gradient → normal.
                let dx = height(x + 1, y) - height(x - 1, y);
                let dy = height(x, y + 1) - height(x, y - 1);
                let (nx, ny, nz) = (-dx * STRENGTH, -dy * STRENGTH, 1.0);
                let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
                out.push(((nx * inv * 0.5 + 0.5) * 255.0).round() as u8);
                out.push(((ny * inv * 0.5 + 0.5) * 255.0).round() as u8);
                out.push(((nz * inv * 0.5 + 0.5) * 255.0).round() as u8);
                out.push((height(x, y).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    out
}

/// The normal array with any per-layer `_n.png`/`_h.png` pack overrides applied.
/// (The override overlay lands in a later sub-step; for now, procedural.)
pub fn load_block_normals() -> Vec<u8> {
    build_block_normals()
}

fn shade(base: [u8; 3], noise: u32, amplitude: i32) -> [u8; 3] {
    let delta = (noise % (2 * amplitude as u32 + 1)) as i32 - amplitude;
    base.map(|c| (c as i32 + delta).clamp(0, 255) as u8)
}

/// A seamless cobblestone cell pattern: rounded stones that brighten (dome up)
/// toward each stone's centre, separated by darker mortar gaps. The luminance
/// reads as height — stones bulge, mortar recesses — so the normal map derives
/// real cobble relief. Tiles across merged quads: the seed grid wraps modulo
/// `GRID`, while seed positions stay unwrapped so wrapped neighbours sit at the
/// correct offset.
fn cobble(x: usize, y: usize) -> [u8; 3] {
    const GRID: i32 = 4; // stones per axis across the 16px tile
    let cell = TEXTURE_SIZE as f32 / GRID as f32;
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let (gx, gy) = ((px / cell).floor() as i32, (py / cell).floor() as i32);
    let (mut f1, mut f2, mut tint) = (1e9_f32, 1e9_f32, 0u32);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (gx + dx, gy + dy);
            let h = hash_noise(cx.rem_euclid(GRID) as u32, cy.rem_euclid(GRID) as u32, 777);
            let jx = (h & 0xff) as f32 / 255.0;
            let jy = ((h >> 8) & 0xff) as f32 / 255.0;
            let sx = (cx as f32 + 0.2 + 0.6 * jx) * cell;
            let sy = (cy as f32 + 0.2 + 0.6 * jy) * cell;
            let d = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt();
            if d < f1 {
                (f2, f1, tint) = (f1, d, h);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    // Mortar where two cells meet (small F2-F1); otherwise a domed stone.
    if f2 - f1 < 0.9 {
        return [66, 66, 70];
    }
    let dome = 1.0 - (f1 / (cell * 0.8)).min(1.0);
    let per_stone = (tint % 21) as f32 - 10.0;
    let v = (118.0 + dome * 30.0 + per_stone).clamp(40.0, 210.0);
    [v as u8, v as u8, (v + 3.0).min(255.0) as u8]
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
    fn mer_array_encodes_metal_and_roughness() {
        let mer = build_block_mer();
        let size = (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize;
        assert_eq!(mer.len(), size * LAYER_COUNT as usize);
        // Iron block (layer 17) is uniformly metal: R (metalness) = 255 everywhere.
        let iron_block = &mer[17 * size..18 * size];
        assert!(iron_block.chunks(4).all(|px| px[0] == 255), "iron block not all metal");
        // Iron ore (layer 22) is *per-texel*: metallic flecks + matte matrix.
        let iron_ore = &mer[22 * size..23 * size];
        let metal = iron_ore.chunks(4).filter(|px| px[0] >= 128).count();
        let matte = iron_ore.chunks(4).filter(|px| px[0] < 128).count();
        assert!(metal > 0 && matte > 0, "iron ore not per-texel: {metal} metal / {matte} matte");
        // Ice (layer 16) is a smooth dielectric: no metal, low roughness (G).
        let ice = &mer[16 * size..17 * size];
        assert!(ice.chunks(4).all(|px| px[0] == 0), "ice should not be metal");
        assert!((ice[1] as f32 / 255.0) < 0.3, "ice should be smooth");
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
