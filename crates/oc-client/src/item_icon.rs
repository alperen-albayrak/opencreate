//! Item icons for the hotbar and inventory: Minecraft-style isometric cubes
//! for block items (flat-shaded from per-block top/side colors) and small
//! pixel-art for non-block items (apple, stick). Everything is built from
//! [`oc_renderer::UiPoly`] — solid-color filled quads — because the UI
//! pipeline samples no texture; the 3D read comes entirely from per-face
//! shading.
//!
//! Designs are authored per item id (the [`icon_for`] table, produced by the
//! icon-design pass). Any item without an entry falls back to a single-colour
//! cube from its block swatch, or a flat square for a pure item, so new
//! content still gets a reasonable icon.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiPoly, block_swatch};

// Minecraft-like face shading: bright top, dimmer left, dimmest right. Three
// tones from one base colour give the cube its 3-D form without any texture.
const TOP_SHADE: f32 = 1.0;
const LEFT_SHADE: f32 = 0.82;
const RIGHT_SHADE: f32 = 0.6;

/// A flat pixel-art rectangle, normalised to the slot (0..1, origin top-left).
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
}

/// How an item is drawn.
enum Icon {
    /// Isometric cube from a top-face and a side-face base colour (RGB 0..255).
    Cube { top: [u8; 3], side: [u8; 3] },
    /// Flat pixel-art: rects drawn back-to-front.
    Art(&'static [Rect]),
}

// A single thin wooden stick on a lower-left → upper-right diagonal, faked as a
// staircase of square segments with a highlight strip, a shadow strip, and
// darker cut ends.
const STICK: &[Rect] = &[
    Rect { x: 0.18, y: 0.66, w: 0.16, h: 0.16, color: [0.47, 0.33, 0.18, 1.0] },
    Rect { x: 0.30, y: 0.55, w: 0.16, h: 0.16, color: [0.47, 0.33, 0.18, 1.0] },
    Rect { x: 0.42, y: 0.44, w: 0.16, h: 0.16, color: [0.47, 0.33, 0.18, 1.0] },
    Rect { x: 0.54, y: 0.33, w: 0.16, h: 0.16, color: [0.47, 0.33, 0.18, 1.0] },
    Rect { x: 0.66, y: 0.22, w: 0.16, h: 0.16, color: [0.47, 0.33, 0.18, 1.0] },
    Rect { x: 0.20, y: 0.62, w: 0.50, h: 0.06, color: [0.60, 0.45, 0.27, 1.0] },
    Rect { x: 0.32, y: 0.70, w: 0.50, h: 0.05, color: [0.31, 0.21, 0.11, 1.0] },
    Rect { x: 0.18, y: 0.70, w: 0.12, h: 0.12, color: [0.36, 0.25, 0.13, 1.0] },
    Rect { x: 0.70, y: 0.18, w: 0.12, h: 0.12, color: [0.36, 0.25, 0.13, 1.0] },
];

// A round red apple: a brown stem and a green leaf behind, then the red body
// built from stacked bands (widest in the middle), then an upper-left sheen.
const APPLE: &[Rect] = &[
    Rect { x: 0.46, y: 0.12, w: 0.06, h: 0.20, color: [0.43, 0.27, 0.14, 1.0] },
    Rect { x: 0.52, y: 0.14, w: 0.16, h: 0.10, color: [0.31, 0.67, 0.24, 1.0] },
    Rect { x: 0.62, y: 0.10, w: 0.10, h: 0.08, color: [0.31, 0.67, 0.24, 1.0] },
    Rect { x: 0.34, y: 0.30, w: 0.32, h: 0.10, color: [0.784, 0.176, 0.157, 1.0] },
    Rect { x: 0.26, y: 0.38, w: 0.48, h: 0.12, color: [0.784, 0.176, 0.157, 1.0] },
    Rect { x: 0.20, y: 0.48, w: 0.60, h: 0.20, color: [0.784, 0.176, 0.157, 1.0] },
    Rect { x: 0.24, y: 0.66, w: 0.52, h: 0.12, color: [0.784, 0.176, 0.157, 1.0] },
    Rect { x: 0.32, y: 0.76, w: 0.36, h: 0.10, color: [0.784, 0.176, 0.157, 1.0] },
    Rect { x: 0.28, y: 0.44, w: 0.14, h: 0.16, color: [0.96, 0.43, 0.39, 1.0] },
];

const FALLBACK: &[Rect] =
    &[Rect { x: 0.18, y: 0.18, w: 0.64, h: 0.64, color: [0.6, 0.6, 0.62, 1.0] }];

/// The authored icon for an item id, or `None` to use the swatch fallback.
fn icon_for(id: &str) -> Option<Icon> {
    Some(match id {
        "oc:stone" => Icon::Cube { top: [128, 128, 128], side: [128, 128, 128] },
        "oc:dirt" => Icon::Cube { top: [134, 96, 67], side: [134, 96, 67] },
        "oc:grass" => Icon::Cube { top: [106, 170, 64], side: [134, 96, 67] },
        "oc:sand" => Icon::Cube { top: [219, 209, 160], side: [219, 209, 160] },
        "oc:log" => Icon::Cube { top: [165, 135, 95], side: [104, 82, 50] },
        "oc:leaves" => Icon::Cube { top: [58, 134, 52], side: [58, 134, 52] },
        "oc:lamp" => Icon::Cube { top: [255, 222, 150], side: [255, 222, 150] },
        "oc:snow" => Icon::Cube { top: [238, 242, 248], side: [238, 242, 248] },
        "oc:planks" => Icon::Cube { top: [172, 136, 84], side: [172, 136, 84] },
        "oc:stick" => Icon::Art(STICK),
        "oc:apple" => Icon::Art(APPLE),
        _ => return None,
    })
}

fn shade(rgb: [u8; 3], k: f32) -> [f32; 4] {
    [rgb[0] as f32 / 255.0 * k, rgb[1] as f32 / 255.0 * k, rgb[2] as f32 / 255.0 * k, 1.0]
}

/// Appends the icon for `item`, filling the slot `rect` = (x, y, w, h) in
/// framebuffer pixels, to `out`.
pub fn draw(registry: &Registry, item: ItemId, rect: (f32, f32, f32, f32), out: &mut Vec<UiPoly>) {
    let icon = icon_for(&registry.item(item).id).unwrap_or_else(|| {
        // Fallback: a block item becomes a flat-shaded cube from its swatch;
        // a pure item a neutral square.
        match registry.block_for_item(item) {
            Some(block) => {
                let c = block_swatch(block);
                let rgb = [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8];
                Icon::Cube { top: rgb, side: rgb }
            }
            None => Icon::Art(FALLBACK),
        }
    });
    match icon {
        Icon::Cube { top, side } => cube(
            rect,
            shade(top, TOP_SHADE),
            shade(side, LEFT_SHADE),
            shade(side, RIGHT_SHADE),
            out,
        ),
        Icon::Art(rects) => art(rect, rects, out),
    }
}

/// Three shaded faces of an isometric cube inscribed in the (square) slot.
fn cube(
    rect: (f32, f32, f32, f32),
    top: [f32; 4],
    left: [f32; 4],
    right: [f32; 4],
    out: &mut Vec<UiPoly>,
) {
    let (x, y, w, h) = rect;
    let s = w.min(h);
    let ox = x + (w - s) / 2.0;
    let oy = y + (h - s) / 2.0;
    let p = s * 0.12; // margin
    let dr = (s - 2.0 * p) * 0.25; // top-face half-height
    let cx = ox + s / 2.0;
    let t = [cx, oy + p]; // top apex
    let l = [ox + p, oy + p + dr]; // top-face left
    let r = [ox + s - p, oy + p + dr]; // top-face right
    let m = [cx, oy + p + 2.0 * dr]; // top-face bottom / front top
    let lb = [ox + p, oy + s - p - dr]; // left-face bottom
    let rb = [ox + s - p, oy + s - p - dr]; // right-face bottom
    let fb = [cx, oy + s - p]; // front-bottom apex
    out.push(UiPoly { corners: [t, r, l, m], color: top });
    out.push(UiPoly { corners: [l, m, lb, fb], color: left });
    out.push(UiPoly { corners: [m, r, fb, rb], color: right });
}

/// Axis-aligned pixel-art rects mapped into the slot.
fn art(rect: (f32, f32, f32, f32), rects: &[Rect], out: &mut Vec<UiPoly>) {
    let (x, y, w, h) = rect;
    for q in rects {
        let (rx, ry) = (x + q.x * w, y + q.y * h);
        let (rw, rh) = (q.w * w, q.h * h);
        out.push(UiPoly {
            corners: [[rx, ry], [rx + rw, ry], [rx, ry + rh], [rx + rw, ry + rh]],
            color: q.color,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_items_are_three_face_cubes() {
        let reg = Registry::load_default().unwrap();
        let stone = reg.find("oc:stone").unwrap();
        let mut out = Vec::new();
        draw(&reg, stone, (0.0, 0.0, 32.0, 32.0), &mut out);
        assert_eq!(out.len(), 3, "a cube is three faces");
        // Top face is brighter than the right face (the shading read).
        assert!(out[0].color[0] > out[2].color[0]);
    }

    #[test]
    fn art_items_emit_their_rects_and_stay_in_the_slot() {
        let reg = Registry::load_default().unwrap();
        let apple = reg.find("oc:apple").unwrap();
        let mut out = Vec::new();
        draw(&reg, apple, (10.0, 10.0, 32.0, 32.0), &mut out);
        assert_eq!(out.len(), APPLE.len());
        for poly in &out {
            for [cx, cy] in poly.corners {
                assert!((10.0..=42.0).contains(&cx) && (10.0..=42.0).contains(&cy));
            }
        }
    }

    #[test]
    fn unknown_block_item_falls_back_to_a_cube() {
        let reg = Registry::load_default().unwrap();
        // bedrock has no authored icon but is a placeable block.
        if let Some(bedrock) = reg.find("oc:bedrock") {
            let mut out = Vec::new();
            draw(&reg, bedrock, (0.0, 0.0, 32.0, 32.0), &mut out);
            assert_eq!(out.len(), 3, "fallback cube");
        }
    }
}
