//! The hotbar: the bottom row of the inventory (storage slots 0..9). Any
//! item can be bound to any slot by dragging it there in the inventory
//! screen; keys 1..=9 and the mouse wheel pick the active slot. Creative
//! carries no gathered items, so it falls back to a fixed block palette.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiQuad, UiText};
use oc_world::{BlockId, blocks};

use crate::inventory_screen::item_swatch;

/// Creative's fixed palette, in slot order (keys 1..=9).
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

/// One displayed hotbar slot: an item with a count, or empty.
pub type Slot = Option<(ItemId, u32)>;

pub struct Hotbar {
    pub selected: usize,
    /// Accumulated scroll, consumed in whole steps.
    scroll: f64,
}

impl Hotbar {
    pub fn new() -> Self {
        Self { selected: 0, scroll: 0.0 }
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
        let len = ITEMS.len();
        while self.scroll >= 1.0 {
            self.scroll -= 1.0;
            self.selected = (self.selected + len - 1) % len;
        }
        while self.scroll <= -1.0 {
            self.scroll += 1.0;
            self.selected = (self.selected + 1) % len;
        }
    }

    /// Lays the hotbar out for a framebuffer of `width`×`height` pixels at
    /// the given UI scale. `slots[i]` is the item bound to slot i (or none);
    /// `show_counts` is false in creative (the palette is infinite).
    pub fn quads(
        &self,
        width: f32,
        height: f32,
        ui: f32,
        registry: &Registry,
        slots: &[Slot; 9],
        _show_counts: bool,
    ) -> Vec<UiQuad> {
        let (slot, gap, inset) = (SLOT * ui, GAP * ui, INSET * ui);
        let (x0, y) = Self::origin(width, height, ui);
        let mut quads = Vec::with_capacity(ITEMS.len() * 2 + 1);
        for (i, stack) in slots.iter().enumerate() {
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
            quads.push(UiQuad { x, y, w: slot, h: slot, color: [0.08, 0.08, 0.1, 0.75] });
            if let Some((item, _)) = stack {
                quads.push(UiQuad {
                    x: x + inset,
                    y: y + inset,
                    w: slot - 2.0 * inset,
                    h: slot - 2.0 * inset,
                    color: item_swatch(registry, *item),
                });
            }
        }
        quads
    }

    /// Count labels for filled slots, bottom-right corners (skipped when
    /// `show_counts` is false, i.e. the infinite creative palette).
    pub fn count_labels(
        &self,
        width: f32,
        height: f32,
        ui: f32,
        slots: &[Slot; 9],
        show_counts: bool,
    ) -> Vec<UiText> {
        if !show_counts {
            return Vec::new();
        }
        let (slot, gap) = (SLOT * ui, GAP * ui);
        let (x0, y) = Self::origin(width, height, ui);
        let scale = ui;
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, stack)| stack.map(|(_, n)| (i, n)))
            .filter(|(_, n)| *n > 1)
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

    fn palette(registry: &Registry) -> [Slot; 9] {
        std::array::from_fn(|i| registry.item_for_block(ITEMS[i]).map(|item| (item, 1)))
    }

    #[test]
    fn selection_via_keys_and_scroll() {
        let mut hotbar = Hotbar::new();
        assert_eq!(hotbar.selected, 0);
        hotbar.select(7);
        assert_eq!(hotbar.selected, 7);
        hotbar.select(99); // out of range: ignored
        assert_eq!(hotbar.selected, 7);

        let last = ITEMS.len() - 1;
        hotbar.select(0);
        hotbar.scroll(-1.0); // scroll down: next slot
        assert_eq!(hotbar.selected, 1);
        hotbar.scroll(1.0); // scroll up: previous
        assert_eq!(hotbar.selected, 0);
        hotbar.scroll(1.0); // wraps to the end
        assert_eq!(hotbar.selected, last);
        // Sub-step scrolling accumulates without switching.
        hotbar.select(0);
        hotbar.scroll(-0.4);
        assert_eq!(hotbar.selected, 0);
        hotbar.scroll(-0.7);
        assert_eq!(hotbar.selected, 1);
    }

    #[test]
    fn layout_is_centered_and_on_screen() {
        let registry = Registry::load_default().unwrap();
        let hotbar = Hotbar::new();
        let (w, h) = (2560.0, 1600.0);
        let quads = hotbar.quads(w, h, 2.0, &registry, &palette(&registry), false);
        // 9 slots x (bg + swatch) + 1 selection ring.
        assert_eq!(quads.len(), ITEMS.len() * 2 + 1);
        for q in &quads {
            assert!(q.x >= 0.0 && q.x + q.w <= w, "quad off-screen: {q:?}");
            assert!(q.y >= 0.0 && q.y + q.h <= h, "quad off-screen: {q:?}");
        }
        // Symmetric horizontal centering.
        let left = quads.iter().map(|q| q.x).fold(f32::MAX, f32::min);
        let right = quads.iter().map(|q| q.x + q.w).fold(0.0, f32::max);
        assert!((left - (w - right)).abs() < 8.0, "not centered: {left} vs {}", w - right);
    }

    #[test]
    fn count_labels_only_for_stacks_above_one() {
        let registry = Registry::load_default().unwrap();
        let hotbar = Hotbar::new();
        let stone = registry.item_for_block(blocks::STONE).unwrap();
        let dirt = registry.item_for_block(blocks::DIRT).unwrap();
        let mut slots: [Slot; 9] = [None; 9];
        slots[0] = Some((stone, 64));
        slots[1] = Some((dirt, 1)); // a single item: no label
        slots[3] = Some((stone, 7));
        let labels = hotbar.count_labels(2560.0, 1600.0, 2.0, &slots, true);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].text, "64");
        assert_eq!(labels[1].text, "7");
        // Creative (infinite) shows no counts.
        assert!(hotbar.count_labels(2560.0, 1600.0, 2.0, &slots, false).is_empty());
    }

    #[test]
    fn every_palette_block_has_a_name() {
        for block in ITEMS {
            assert_ne!(block_name(block), "block");
        }
    }

    #[test]
    fn stat_bars_reflect_values() {
        let full = stat_bars(2560.0, 1600.0, 2.0, 10.0, 10.0, 10.0, 10.0);
        assert_eq!(full.len(), 6); // full oxygen hides its bar
        let low = stat_bars(2560.0, 1600.0, 2.0, 5.0, 10.0, 10.0, 3.0);
        assert_eq!(low.len(), 8);
        let dead = stat_bars(2560.0, 1600.0, 2.0, 0.0, 10.0, 10.0, 10.0);
        assert_eq!(dead.len(), 5);
        let half = stat_bars(2560.0, 1600.0, 2.0, 5.0, 10.0, 10.0, 10.0);
        assert!((half[1].w / full[1].w - 0.5).abs() < 0.01);
    }
}
