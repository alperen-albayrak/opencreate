//! The inventory screen (E/C): a character pane on the left (a paper-doll
//! in the player's skin colors that watches the cursor, with armor slots
//! awaiting the armor system), and on the right a 3×3 crafting grid with a
//! result slot, the main storage grid, and the hotbar row. Items move
//! slot-to-slot through a cursor stack picked up and put down by clicking.
//!
//! Storage and the crafting grid are server-authoritative; this screen is
//! pure presentation plus hit-testing. Slot indices map to the protocol:
//! storage 0..9 is the hotbar row, 9..36 the main grid.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiQuad, UiText, block_swatch};

use crate::avatar::Skin;

/// One slot: an item with a count, or empty.
type Slot = Option<(ItemId, u32)>;

// Logical units, multiplied by the effective UI scale.
const SLOT: f32 = 34.0;
const GAP: f32 = 4.0;
const PAD: f32 = 10.0;
/// Character pane width (left side).
const DOLL_W: f32 = 175.0;
/// Gap between the crafting grid and its result slot (room for the arrow).
const OUT_GAP: f32 = 32.0;
/// Vertical gap below the crafting grid before the main storage grid.
const SECTION_GAP: f32 = 18.0;
/// Vertical gap between the main grid and the hotbar row.
const ROW_GAP: f32 = 12.0;

const PANEL_W: f32 = PAD * 3.0 + DOLL_W + (9.0 * (SLOT + GAP) - GAP);
const PANEL_H: f32 = 350.0;

/// What the cursor is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Storage slot 0..36 (0..9 is the hotbar row).
    Storage(usize),
    /// Crafting-grid slot 0..9.
    Craft(usize),
    /// The crafting result slot.
    Output,
    None,
}

fn panel_origin(width: f32, height: f32, ui: f32) -> (f32, f32) {
    (((width - PANEL_W * ui) / 2.0), ((height - PANEL_H * ui) / 2.0))
}

/// Left edge of the right-hand content column.
fn col_x0(width: f32, height: f32, ui: f32) -> f32 {
    panel_origin(width, height, ui).0 + (PAD * 2.0 + DOLL_W) * ui
}

/// Top of the content column (below the title row).
fn content_top(height: f32, py: f32, ui: f32) -> f32 {
    let _ = height;
    py + 28.0 * ui
}

fn step(ui: f32) -> f32 {
    (SLOT + GAP) * ui
}

fn craft_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let x0 = col_x0(w, h, ui);
    let top = content_top(h, py, ui);
    (x0 + (i % 3) as f32 * step(ui), top + (i / 3) as f32 * step(ui), SLOT * ui, SLOT * ui)
}

fn output_rect(w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let x0 = col_x0(w, h, ui);
    let top = content_top(h, py, ui);
    (x0 + 3.0 * step(ui) + OUT_GAP * ui, top + step(ui), SLOT * ui, SLOT * ui)
}

fn main_top(h: f32, py: f32, ui: f32) -> f32 {
    content_top(h, py, ui) + 3.0 * step(ui) + SECTION_GAP * ui
}

/// Main storage slot rect for storage index 9..36 (`i` = index - 9).
fn main_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let x0 = col_x0(w, h, ui);
    let top = main_top(h, py, ui);
    (x0 + (i % 9) as f32 * step(ui), top + (i / 9) as f32 * step(ui), SLOT * ui, SLOT * ui)
}

fn hotbar_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let x0 = col_x0(w, h, ui);
    let top = main_top(h, py, ui) + 3.0 * step(ui) + ROW_GAP * ui;
    (x0 + i as f32 * step(ui), top, SLOT * ui, SLOT * ui)
}

fn inside(pos: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    pos.0 >= rect.0 && pos.0 < rect.0 + rect.2 && pos.1 >= rect.1 && pos.1 < rect.1 + rect.3
}

/// Hit test in framebuffer pixels.
pub fn hit(pos: (f32, f32), width: f32, height: f32, ui: f32) -> Hit {
    for slot in 0..9 {
        if inside(pos, hotbar_rect(slot, width, height, ui)) {
            return Hit::Storage(slot);
        }
    }
    for i in 0..27 {
        if inside(pos, main_rect(i, width, height, ui)) {
            return Hit::Storage(9 + i);
        }
    }
    for i in 0..9 {
        if inside(pos, craft_rect(i, width, height, ui)) {
            return Hit::Craft(i);
        }
    }
    if inside(pos, output_rect(width, height, ui)) {
        return Hit::Output;
    }
    Hit::None
}

/// Item swatch: the block's color, or a fallback for pure items. Shared
/// with the HUD hotbar.
pub fn item_swatch(registry: &Registry, item: ItemId) -> [f32; 4] {
    if let Some(block) = registry.block_for_item(item) {
        return block_swatch(block);
    }
    match registry.item(item).name.as_str() {
        "Apple" => [0.80, 0.18, 0.16, 1.0],
        "Stick" => [0.48, 0.34, 0.18, 1.0],
        _ => [0.6, 0.6, 0.62, 1.0],
    }
}

fn slot_quad(quads: &mut Vec<UiQuad>, rect: (f32, f32, f32, f32), lit: bool) {
    let (x, y, w, h) = rect;
    quads.push(UiQuad {
        x,
        y,
        w,
        h,
        color: if lit { [0.25, 0.25, 0.3, 0.9] } else { [0.13, 0.13, 0.17, 0.9] },
    });
}

/// Draws a stack's swatch + count inside a slot rect.
fn draw_stack(
    quads: &mut Vec<UiQuad>,
    texts: &mut Vec<UiText>,
    rect: (f32, f32, f32, f32),
    registry: &Registry,
    stack: Slot,
    ui: f32,
) {
    let Some((item, count)) = stack else {
        return;
    };
    let (x, y, w, h) = rect;
    let inset = 4.0 * ui;
    quads.push(UiQuad {
        x: x + inset,
        y: y + inset,
        w: w - 2.0 * inset,
        h: h - 2.0 * inset,
        color: item_swatch(registry, item),
    });
    if count > 1 {
        let label = count.to_string();
        texts.push(UiText {
            text: label.clone(),
            x: x + w - label.len() as f32 * 6.0 * ui - 2.0 * ui,
            y: y + h - 9.0 * ui,
            scale: ui,
        });
    }
}

/// The character pane: armor placeholders and a paper-doll whose head and
/// eyes track the cursor.
fn doll(
    quads: &mut Vec<UiQuad>,
    texts: &mut Vec<UiText>,
    skin: &Skin,
    mouse: (f32, f32),
    width: f32,
    height: f32,
    ui: f32,
) {
    let (px, py) = panel_origin(width, height, ui);

    for (row, label) in ["H", "C", "L", "B"].iter().enumerate() {
        let x = px + PAD * ui;
        let y = py + (34.0 + row as f32 * 42.0) * ui;
        quads.push(UiQuad { x, y, w: 30.0 * ui, h: 30.0 * ui, color: [0.10, 0.10, 0.13, 0.9] });
        texts.push(UiText { text: (*label).into(), x: x + 12.0 * ui, y: y + 11.0 * ui, scale: ui * 0.9 });
    }

    let cx = px + (PAD + 30.0 + (DOLL_W - 30.0 - PAD) / 2.0) * ui;
    let top = py + 44.0 * ui;
    let (head, torso_w, torso_h, arm_w, leg_h) =
        (44.0 * ui, 52.0 * ui, 62.0 * ui, 18.0 * ui, 66.0 * ui);

    let head_cx = cx;
    let head_cy = top + head / 2.0;
    let dx = (mouse.0 - head_cx).clamp(-600.0, 600.0) / 600.0;
    let dy = (mouse.1 - head_cy).clamp(-600.0, 600.0) / 600.0;
    let (lean_x, lean_y) = (dx * 4.0 * ui, dy * 3.0 * ui);

    quads.push(UiQuad {
        x: cx - head / 2.0 + lean_x,
        y: top + lean_y,
        w: head,
        h: head,
        color: skin.head,
    });
    let eye = 5.0 * ui;
    for side in [-1.0f32, 1.0] {
        quads.push(UiQuad {
            x: cx + side * 10.0 * ui - eye / 2.0 + lean_x + dx * 3.0 * ui,
            y: top + 16.0 * ui + lean_y + dy * 3.0 * ui,
            w: eye,
            h: eye * 1.2,
            color: [0.12, 0.10, 0.10, 1.0],
        });
    }
    let torso_y = top + head + 2.0 * ui;
    quads.push(UiQuad { x: cx - torso_w / 2.0, y: torso_y, w: torso_w, h: torso_h, color: skin.torso });
    for side in [-1.0f32, 1.0] {
        quads.push(UiQuad {
            x: cx + side * (torso_w / 2.0 + arm_w / 2.0) - arm_w / 2.0,
            y: torso_y,
            w: arm_w,
            h: torso_h,
            color: skin.arms,
        });
    }
    for side in [-1.0f32, 1.0] {
        quads.push(UiQuad {
            x: cx + side * torso_w / 4.0 - torso_w / 4.0,
            y: torso_y + torso_h + 2.0 * ui,
            w: torso_w / 2.0 - 2.0 * ui,
            h: leg_h,
            color: skin.legs,
        });
    }
}

/// Builds the whole panel from the authoritative inventory mirror.
#[allow(clippy::too_many_arguments)]
pub fn panel(
    registry: &Registry,
    slots: &[Slot; 36],
    craft: &[Slot; 9],
    cursor: Slot,
    craft_result: Option<(ItemId, u8)>,
    selected: usize,
    skin: &Skin,
    mouse: (f32, f32),
    width: f32,
    height: f32,
    ui: f32,
) -> (Vec<UiQuad>, Vec<UiText>) {
    let (px, py) = panel_origin(width, height, ui);
    let x0 = col_x0(width, height, ui);
    let mut quads = vec![
        UiQuad { x: px, y: py, w: PANEL_W * ui, h: PANEL_H * ui, color: [0.05, 0.05, 0.08, 0.93] },
        UiQuad {
            x: px + PAD * ui / 2.0,
            y: py + 24.0 * ui,
            w: (DOLL_W + PAD) * ui,
            h: (PANEL_H - 32.0) * ui,
            color: [0.08, 0.09, 0.12, 0.9],
        },
    ];
    let mut texts = vec![
        UiText { text: "INVENTORY  [E] CLOSE".into(), x: px + PAD * ui, y: py + PAD * ui, scale: ui },
        UiText { text: "CRAFTING".into(), x: x0, y: py + 16.0 * ui, scale: ui * 0.85 },
    ];

    doll(&mut quads, &mut texts, skin, mouse, width, height, ui);

    let hover = hit(mouse, width, height, ui);

    // Crafting grid.
    for i in 0..9 {
        let rect = craft_rect(i, width, height, ui);
        slot_quad(&mut quads, rect, hover == Hit::Craft(i));
        draw_stack(&mut quads, &mut texts, rect, registry, craft[i], ui);
    }
    // Arrow + result slot.
    let out = output_rect(width, height, ui);
    texts.push(UiText {
        text: ">".into(),
        x: out.0 - 18.0 * ui,
        y: out.1 + out.3 / 2.0 - 4.0 * ui,
        scale: ui * 1.2,
    });
    slot_quad(&mut quads, out, hover == Hit::Output);
    draw_stack(
        &mut quads,
        &mut texts,
        out,
        registry,
        craft_result.map(|(item, n)| (item, n as u32)),
        ui,
    );

    // Main storage grid (storage slots 9..36).
    for i in 0..27 {
        let rect = main_rect(i, width, height, ui);
        slot_quad(&mut quads, rect, hover == Hit::Storage(9 + i));
        draw_stack(&mut quads, &mut texts, rect, registry, slots[9 + i], ui);
    }

    // Hotbar row (storage slots 0..9); the selected slot stays lit.
    for i in 0..9 {
        let rect = hotbar_rect(i, width, height, ui);
        slot_quad(&mut quads, rect, hover == Hit::Storage(i) || i == selected);
        draw_stack(&mut quads, &mut texts, rect, registry, slots[i], ui);
    }

    // The cursor stack rides the mouse.
    if let Some((item, count)) = cursor {
        let size = SLOT * 0.8 * ui;
        let rect = (mouse.0 - size / 2.0, mouse.1 - size / 2.0, size, size);
        quads.push(UiQuad { x: rect.0, y: rect.1, w: size, h: size, color: item_swatch(registry, item) });
        if count > 1 {
            let label = count.to_string();
            texts.push(UiText {
                text: label.clone(),
                x: rect.0 + size - label.len() as f32 * 6.0 * ui - 1.0 * ui,
                y: rect.1 + size - 9.0 * ui,
                scale: ui,
            });
        }
    }

    (quads, texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_map_to_the_right_slots() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        // Hotbar slot 0 is storage 0.
        let hb = hotbar_rect(0, w, h, ui);
        assert_eq!(hit((hb.0 + 2.0, hb.1 + 2.0), w, h, ui), Hit::Storage(0));
        // First main slot is storage 9.
        let m = main_rect(0, w, h, ui);
        assert_eq!(hit((m.0 + 2.0, m.1 + 2.0), w, h, ui), Hit::Storage(9));
        // Last main slot is storage 35.
        let last = main_rect(26, w, h, ui);
        assert_eq!(hit((last.0 + 2.0, last.1 + 2.0), w, h, ui), Hit::Storage(35));
        // Craft grid and output.
        let c = craft_rect(4, w, h, ui);
        assert_eq!(hit((c.0 + 2.0, c.1 + 2.0), w, h, ui), Hit::Craft(4));
        let o = output_rect(w, h, ui);
        assert_eq!(hit((o.0 + 2.0, o.1 + 2.0), w, h, ui), Hit::Output);
        // Empty space.
        assert_eq!(hit((1.0, 1.0), w, h, ui), Hit::None);
    }

    #[test]
    fn regions_do_not_overlap() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let last_craft = craft_rect(8, w, h, ui);
        let first_main = main_rect(0, w, h, ui);
        assert!(last_craft.1 + last_craft.3 <= first_main.1, "craft grid runs into main grid");
        let last_main = main_rect(26, w, h, ui);
        let hotbar = hotbar_rect(0, w, h, ui);
        assert!(last_main.1 + last_main.3 <= hotbar.1, "main grid runs into the hotbar");
    }

    #[test]
    fn panel_renders_slots_craft_and_cursor() {
        let registry = Registry::load_default().unwrap();
        let stone = registry.find("oc:stone").unwrap();
        let log = registry.find("oc:log").unwrap();
        let mut slots: [Slot; 36] = [None; 36];
        slots[0] = Some((stone, 64));
        slots[10] = Some((log, 3));
        let mut craft: [Slot; 9] = [None; 9];
        craft[0] = Some((log, 1));
        let skin = crate::avatar::load_skin();
        let (quads, texts) = panel(
            &registry,
            &slots,
            &craft,
            Some((stone, 5)),
            None,
            0,
            &skin,
            (0.0, 0.0),
            1600.0,
            1000.0,
            1.0,
        );
        // Background + doll + 9 craft + output + 27 main + 9 hotbar slots, plus
        // swatches and the cursor: comfortably many quads.
        assert!(quads.len() > 50);
        // At least the titles plus stack counts (64, 3, 5) render text.
        assert!(texts.iter().any(|t| t.text == "64"));
        assert!(texts.iter().any(|t| t.text == "5"));
    }
}
