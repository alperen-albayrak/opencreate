//! The block hotbar: creative palette of placeable blocks until survival
//! inventories (phase 3) replace it.

use oc_renderer::{UiQuad, UiText, block_swatch};
use oc_world::{BlockId, blocks};

/// Placeable palette, in slot order (keys 1..=9).
pub const ITEMS: [BlockId; 9] = [
    blocks::STONE,
    blocks::DIRT,
    blocks::GRASS,
    blocks::SAND,
    blocks::LOG,
    blocks::PLANKS,
    blocks::LEAVES,
    blocks::LAMP,
    blocks::SNOW,
];

pub fn block_name(block: BlockId) -> &'static str {
    match block {
        blocks::STONE => "stone",
        blocks::DIRT => "dirt",
        blocks::GRASS => "grass",
        blocks::SAND => "sand",
        blocks::LOG => "log",
        blocks::LEAVES => "leaves",
        blocks::LAMP => "lamp",
        blocks::SNOW => "snow",
        blocks::PLANKS => "planks",
        _ => "block",
    }
}

pub struct Hotbar {
    pub selected: usize,
    /// Accumulated scroll, consumed in whole steps.
    scroll: f64,
}

impl Hotbar {
    pub fn new() -> Self {
        Self { selected: 0, scroll: 0.0 }
    }

    pub fn block(&self) -> BlockId {
        ITEMS[self.selected]
    }

    /// Selects slot `n` (0-based) if it exists.
    pub fn select(&mut self, n: usize) {
        if n < ITEMS.len() {
            self.selected = n;
        }
    }

    /// Feeds mouse-wheel motion; whole steps cycle the selection.
    pub fn scroll(&mut self, delta: f64) {
        self.scroll += delta;
        while self.scroll >= 1.0 {
            self.scroll -= 1.0;
            self.selected = (self.selected + ITEMS.len() - 1) % ITEMS.len();
        }
        while self.scroll <= -1.0 {
            self.scroll += 1.0;
            self.selected = (self.selected + 1) % ITEMS.len();
        }
    }

    /// Lays the hotbar out for a framebuffer of `width`×`height` pixels
    /// at the given UI scale. `counts[i]` is how many of slot i's block
    /// the player carries; empty slots render dimmed.
    pub fn quads(
        &self,
        width: f32,
        height: f32,
        ui: f32,
        counts: &[u32; ITEMS.len()],
    ) -> Vec<UiQuad> {
        let (slot, gap, inset) = (SLOT * ui, GAP * ui, INSET * ui);
        let (x0, y) = Self::origin(width, height, ui);
        let mut quads = Vec::with_capacity(ITEMS.len() * 2 + 1);
        for (i, &block) in ITEMS.iter().enumerate() {
            let x = x0 + i as f32 * (slot + gap);
            if i == self.selected {
                // Selection ring: a slightly larger bright quad behind.
                quads.push(UiQuad {
                    x: x - 1.5 * ui,
                    y: y - 1.5 * ui,
                    w: slot + 3.0 * ui,
                    h: slot + 3.0 * ui,
                    color: [1.0, 1.0, 1.0, 0.9],
                });
            }
            quads.push(UiQuad {
                x,
                y,
                w: slot,
                h: slot,
                color: [0.08, 0.08, 0.1, 0.75],
            });
            let mut swatch = block_swatch(block);
            swatch[3] = if counts[i] > 0 { 1.0 } else { 0.25 };
            quads.push(UiQuad {
                x: x + inset,
                y: y + inset,
                w: slot - 2.0 * inset,
                h: slot - 2.0 * inset,
                color: swatch,
            });
        }
        quads
    }

    /// Count labels for non-empty slots, bottom-right corners.
    pub fn count_labels(
        &self,
        width: f32,
        height: f32,
        ui: f32,
        counts: &[u32; ITEMS.len()],
    ) -> Vec<UiText> {
        let (slot, gap) = (SLOT * ui, GAP * ui);
        let (x0, y) = Self::origin(width, height, ui);
        let scale = ui;
        counts
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| {
                let text = n.to_string();
                let w = text.len() as f32 * 6.0 * scale;
                UiText {
                    text,
                    x: x0 + i as f32 * (slot + gap) + slot - w - 2.0 * ui,
                    y: y + slot - 7.0 * scale - 2.0 * ui,
                    scale,
                }
            })
            .collect()
    }

    fn origin(width: f32, height: f32, ui: f32) -> (f32, f32) {
        let (slot, gap) = (SLOT * ui, GAP * ui);
        let total = ITEMS.len() as f32 * slot + (ITEMS.len() as f32 - 1.0) * gap;
        ((width - total) / 2.0, height - MARGIN_BOTTOM * ui - slot)
    }
}

// Logical units: multiplied by the effective UI scale (DPI x setting).
const SLOT: f32 = 32.0;
const GAP: f32 = 3.0;
const INSET: f32 = 4.0;
const MARGIN_BOTTOM: f32 = 12.0;

/// Survival stat bars drawn above the hotbar: health, hunger, stamina,
/// and (only while not full) oxygen. Values are 0..=10.
pub fn stat_bars(
    width: f32,
    height: f32,
    ui: f32,
    health: f32,
    hunger: f32,
    stamina: f32,
    oxygen: f32,
) -> Vec<UiQuad> {
    let bar_w = 110.0 * ui;
    let bar_h = 6.0 * ui;
    let gap = 3.0 * ui;
    let above_hotbar = 55.0 * ui;

    let mut quads = Vec::new();
    let mut bar = |index: i32, value: f32, color: [f32; 4]| {
        let x = width / 2.0 - bar_w - gap / 2.0 + (index % 2) as f32 * (bar_w + gap);
        let y = height - above_hotbar - (index / 2) as f32 * (bar_h + gap);
        quads.push(UiQuad { x, y, w: bar_w, h: bar_h, color: [0.05, 0.05, 0.06, 0.7] });
        let fill = (value / 10.0).clamp(0.0, 1.0);
        if fill > 0.0 {
            quads.push(UiQuad {
                x: x + 1.0 * ui,
                y: y + 1.0 * ui,
                w: (bar_w - 2.0 * ui) * fill,
                h: bar_h - 2.0 * ui,
                color,
            });
        }
    };
    bar(0, health, [0.86, 0.18, 0.18, 0.95]);
    bar(1, hunger, [0.83, 0.55, 0.2, 0.95]);
    bar(2, stamina, [0.3, 0.78, 0.32, 0.95]);
    if oxygen < 9.95 {
        bar(3, oxygen, [0.25, 0.5, 0.95, 0.95]);
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_via_keys_and_scroll() {
        let mut hotbar = Hotbar::new();
        assert_eq!(hotbar.block(), blocks::STONE);
        hotbar.select(7);
        assert_eq!(hotbar.block(), blocks::LAMP);
        hotbar.select(99); // out of range: ignored
        assert_eq!(hotbar.selected, 7);

        let last = ITEMS.len() - 1;
        hotbar.scroll(-1.0); // scroll down: next slot
        assert_eq!(hotbar.selected, last);
        hotbar.scroll(-1.0); // wraps
        assert_eq!(hotbar.selected, 0);
        hotbar.scroll(1.0); // scroll up: previous, wraps back
        assert_eq!(hotbar.selected, last);
        // Sub-step scrolling accumulates without switching.
        hotbar.scroll(-0.4);
        assert_eq!(hotbar.selected, last);
        hotbar.scroll(-0.7);
        assert_eq!(hotbar.selected, 0);
    }

    #[test]
    fn layout_is_centered_and_on_screen() {
        let hotbar = Hotbar::new();
        let (w, h) = (2560.0, 1600.0);
        let quads = hotbar.quads(w, h, 2.0, &[1; ITEMS.len()]);
        // 8 slots x (bg + swatch) + 1 selection ring.
        assert_eq!(quads.len(), ITEMS.len() * 2 + 1);
        for q in &quads {
            assert!(q.x >= 0.0 && q.x + q.w <= w, "quad off-screen: {q:?}");
            assert!(q.y >= 0.0 && q.y + q.h <= h, "quad off-screen: {q:?}");
        }
        // Symmetric horizontal centering: as much space on the left of the
        // first slot as right of the last (ring excluded).
        let xs: Vec<f32> = quads.iter().map(|q| q.x).collect();
        let left = xs.iter().cloned().fold(f32::MAX, f32::min);
        let right = quads.iter().map(|q| q.x + q.w).fold(0.0, f32::max);
        assert!((left - (w - right)).abs() < 8.0, "not centered: {left} vs {}", w - right);
    }

    #[test]
    fn stat_bars_reflect_values() {
        // Full oxygen hides its bar: 3 bars x (bg + fill).
        let full = stat_bars(2560.0, 1600.0, 2.0, 10.0, 10.0, 10.0, 10.0);
        assert_eq!(full.len(), 6);
        // Low oxygen shows a fourth bar.
        let low = stat_bars(2560.0, 1600.0, 2.0, 5.0, 10.0, 10.0, 3.0);
        assert_eq!(low.len(), 8);
        // Zero health: background only, no fill quad.
        let dead = stat_bars(2560.0, 1600.0, 2.0, 0.0, 10.0, 10.0, 10.0);
        assert_eq!(dead.len(), 5);
        // Fill width scales with the value.
        let half = stat_bars(2560.0, 1600.0, 2.0, 5.0, 10.0, 10.0, 10.0);
        let full_fill = full[1].w;
        let half_fill = half[1].w;
        assert!((half_fill / full_fill - 0.5).abs() < 0.01);
    }

    #[test]
    fn count_labels_only_for_owned_items() {
        let hotbar = Hotbar::new();
        let mut counts = [0; ITEMS.len()];
        counts[0] = 64;
        counts[3] = 7;
        let labels = hotbar.count_labels(2560.0, 1600.0, 2.0, &counts);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].text, "64");
        assert_eq!(labels[1].text, "7");
        assert!(labels[1].x > labels[0].x, "labels follow slot order");
    }

    #[test]
    fn every_item_has_a_name() {
        for block in ITEMS {
            assert_ne!(block_name(block), "block");
        }
    }

    #[test]
    fn hud_layout_scales_with_the_ui_setting() {
        // The in-game HUD (hotbar slots, stat bars) follows the effective
        // UI scale: doubling it doubles every element's size.
        let hotbar = Hotbar::new();
        let (w, h) = (3840.0, 2160.0);
        let at_1 = hotbar.quads(w, h, 1.0, &[1; ITEMS.len()]);
        let at_2 = hotbar.quads(w, h, 2.0, &[1; ITEMS.len()]);
        // Slot background quads (skip the selection ring at index 0).
        assert!((at_2[1].w - at_1[1].w * 2.0).abs() < 1e-3, "slot width scales");
        assert!((at_2[1].h - at_1[1].h * 2.0).abs() < 1e-3, "slot height scales");

        let bars_1 = stat_bars(w, h, 1.0, 5.0, 5.0, 5.0, 10.0);
        let bars_2 = stat_bars(w, h, 2.0, 5.0, 5.0, 5.0, 10.0);
        assert!((bars_2[0].w - bars_1[0].w * 2.0).abs() < 1e-3, "bar width scales");
        // Both stay centered and on-screen at the bigger scale.
        for q in at_2.iter().chain(bars_2.iter()) {
            assert!(q.x >= 0.0 && q.x + q.w <= w && q.y + q.h <= h, "off-screen: {q:?}");
        }
    }
}
