//! The block hotbar: creative palette of placeable blocks until survival
//! inventories (phase 3) replace it.

use oc_renderer::{UiQuad, block_swatch};
use oc_world::{BlockId, blocks};

/// Placeable palette, in slot order (keys 1..=8).
pub const ITEMS: [BlockId; 8] = [
    blocks::STONE,
    blocks::DIRT,
    blocks::GRASS,
    blocks::SAND,
    blocks::LOG,
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

    /// Lays the hotbar out for a framebuffer of `width`×`height` pixels.
    pub fn quads(&self, width: f32, height: f32) -> Vec<UiQuad> {
        const SLOT: f32 = 64.0;
        const GAP: f32 = 6.0;
        const INSET: f32 = 8.0;
        const MARGIN_BOTTOM: f32 = 24.0;

        let total = ITEMS.len() as f32 * SLOT + (ITEMS.len() as f32 - 1.0) * GAP;
        let x0 = (width - total) / 2.0;
        let y = height - MARGIN_BOTTOM - SLOT;

        let mut quads = Vec::with_capacity(ITEMS.len() * 2 + 1);
        for (i, &block) in ITEMS.iter().enumerate() {
            let x = x0 + i as f32 * (SLOT + GAP);
            if i == self.selected {
                // Selection ring: a slightly larger bright quad behind.
                quads.push(UiQuad {
                    x: x - 3.0,
                    y: y - 3.0,
                    w: SLOT + 6.0,
                    h: SLOT + 6.0,
                    color: [1.0, 1.0, 1.0, 0.9],
                });
            }
            quads.push(UiQuad {
                x,
                y,
                w: SLOT,
                h: SLOT,
                color: [0.08, 0.08, 0.1, 0.75],
            });
            let mut swatch = block_swatch(block);
            swatch[3] = 1.0;
            quads.push(UiQuad {
                x: x + INSET,
                y: y + INSET,
                w: SLOT - 2.0 * INSET,
                h: SLOT - 2.0 * INSET,
                color: swatch,
            });
        }
        quads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_via_keys_and_scroll() {
        let mut hotbar = Hotbar::new();
        assert_eq!(hotbar.block(), blocks::STONE);
        hotbar.select(6);
        assert_eq!(hotbar.block(), blocks::LAMP);
        hotbar.select(99); // out of range: ignored
        assert_eq!(hotbar.selected, 6);

        hotbar.scroll(-1.0); // scroll down: next slot
        assert_eq!(hotbar.selected, 7);
        hotbar.scroll(-1.0); // wraps
        assert_eq!(hotbar.selected, 0);
        hotbar.scroll(1.0); // scroll up: previous, wraps back
        assert_eq!(hotbar.selected, 7);
        // Sub-step scrolling accumulates without switching.
        hotbar.scroll(-0.4);
        assert_eq!(hotbar.selected, 7);
        hotbar.scroll(-0.7);
        assert_eq!(hotbar.selected, 0);
    }

    #[test]
    fn layout_is_centered_and_on_screen() {
        let hotbar = Hotbar::new();
        let (w, h) = (2560.0, 1600.0);
        let quads = hotbar.quads(w, h);
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
    fn every_item_has_a_name() {
        for block in ITEMS {
            assert_ne!(block_name(block), "block");
        }
    }
}
