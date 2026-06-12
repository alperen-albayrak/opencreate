//! The inventory screen (E), laid out after the Minecraft/Hytale
//! references: a character pane on the left (paper-doll in your skin
//! colors that watches the cursor, with armor slots awaiting the armor
//! system), a clickable crafting list top-right, a fixed slot grid below
//! it, and the rebindable hotbar row at the bottom. Drag a block from the
//! grid onto a hotbar slot to bind it; the mouse is released while open.
//!
//! Inventory storage stays a server-authoritative item->count map; this
//! screen is pure presentation plus the local hotbar binding.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiQuad, UiText, block_swatch};

use crate::avatar::Skin;
use crate::craft_menu::CraftLine;

// Logical units, multiplied by the effective UI scale.
const PANEL_W: f32 = 580.0;
const PANEL_H: f32 = 340.0;
const PAD: f32 = 10.0;
/// Character pane width (left side).
const DOLL_W: f32 = 175.0;
/// Inventory grid: fixed Minecraft-style 9 columns.
const GRID_COLS: usize = 9;
const GRID_ROWS: usize = 3;
const SLOT: f32 = 36.0;
const GAP: f32 = 4.0;
const RECIPE_H: f32 = 13.0;
/// Rows reserved for the crafting list above the grid.
const CRAFT_H: f32 = 88.0;

/// One displayed stack.
pub struct Stack {
    pub item: ItemId,
    pub count: u32,
    pub name: String,
}

/// What the cursor is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Stack(usize),
    HotbarSlot(usize),
    Recipe(usize),
    None,
}

fn panel_origin(width: f32, height: f32, ui: f32) -> (f32, f32) {
    (((width - PANEL_W * ui) / 2.0), ((height - PANEL_H * ui) / 2.0))
}

/// The right-side content column (crafting + grid + hotbar).
fn content_x(width: f32, height: f32, ui: f32) -> f32 {
    panel_origin(width, height, ui).0 + (DOLL_W + PAD * 2.0) * ui
}

fn grid_slot_rect(index: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(width, height, ui);
    let x0 = content_x(width, height, ui);
    let (col, row) = (index % GRID_COLS, index / GRID_COLS);
    (
        x0 + col as f32 * (SLOT + GAP) * ui,
        py + (28.0 + CRAFT_H + 14.0) * ui + row as f32 * (SLOT + GAP) * ui,
        SLOT * ui,
        SLOT * ui,
    )
}

fn hotbar_slot_rect(slot: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(width, height, ui);
    let x0 = content_x(width, height, ui);
    (
        x0 + slot as f32 * (SLOT + GAP) * ui,
        py + (PANEL_H - PAD) * ui - SLOT * ui,
        SLOT * ui,
        SLOT * ui,
    )
}

fn recipe_rect(row: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(width, height, ui);
    let x0 = content_x(width, height, ui);
    (
        x0,
        py + 28.0 * ui + row as f32 * RECIPE_H * ui,
        (PANEL_W - DOLL_W - PAD * 3.0) * ui,
        RECIPE_H * ui,
    )
}

fn inside(pos: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    pos.0 >= rect.0 && pos.0 < rect.0 + rect.2 && pos.1 >= rect.1 && pos.1 < rect.1 + rect.3
}

/// Hit test in framebuffer pixels.
pub fn hit(
    pos: (f32, f32),
    width: f32,
    height: f32,
    ui: f32,
    stacks: usize,
    recipes: usize,
) -> Hit {
    for slot in 0..9 {
        if inside(pos, hotbar_slot_rect(slot, width, height, ui)) {
            return Hit::HotbarSlot(slot);
        }
    }
    for index in 0..stacks.min(GRID_COLS * GRID_ROWS) {
        if inside(pos, grid_slot_rect(index, width, height, ui)) {
            return Hit::Stack(index);
        }
    }
    for row in 0..recipes {
        if inside(pos, recipe_rect(row, width, height, ui)) {
            return Hit::Recipe(row);
        }
    }
    Hit::None
}

/// Item swatch: the block's color, or a fallback for pure items.
fn item_swatch(registry: &Registry, item: ItemId) -> [f32; 4] {
    if let Some(block) = registry.block_for_item(item) {
        return block_swatch(block);
    }
    match registry.item(item).name.as_str() {
        "Apple" => [0.80, 0.18, 0.16, 1.0],
        "Stick" => [0.48, 0.34, 0.18, 1.0],
        _ => [0.6, 0.6, 0.62, 1.0],
    }
}

/// The character pane: armor placeholders and a paper-doll whose head
/// (and eyes) track the cursor, like the Minecraft/Hytale previews.
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

    // Armor slots: a dimmed column awaiting the armor system.
    for (row, label) in ["H", "C", "L", "B"].iter().enumerate() {
        let x = px + PAD * ui;
        let y = py + (34.0 + row as f32 * 42.0) * ui;
        quads.push(UiQuad { x, y, w: 30.0 * ui, h: 30.0 * ui, color: [0.10, 0.10, 0.13, 0.9] });
        texts.push(UiText {
            text: (*label).into(),
            x: x + 12.0 * ui,
            y: y + 11.0 * ui,
            scale: ui * 0.9,
        });
    }

    // The doll, centered in the remaining pane width.
    let cx = px + (PAD + 30.0 + (DOLL_W - 30.0 - PAD) / 2.0) * ui;
    let top = py + 44.0 * ui;
    let (head, torso_w, torso_h, arm_w, leg_h) =
        (44.0 * ui, 52.0 * ui, 62.0 * ui, 18.0 * ui, 66.0 * ui);

    // Head leans a touch toward the cursor; the eyes lean further.
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
    // Eyes: two dark pixels that follow harder.
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
    // Torso and arms.
    let torso_y = top + head + 2.0 * ui;
    quads.push(UiQuad {
        x: cx - torso_w / 2.0,
        y: torso_y,
        w: torso_w,
        h: torso_h,
        color: skin.torso,
    });
    for side in [-1.0f32, 1.0] {
        quads.push(UiQuad {
            x: cx + side * (torso_w / 2.0 + arm_w / 2.0) - arm_w / 2.0,
            y: torso_y,
            w: arm_w,
            h: torso_h,
            color: skin.arms,
        });
    }
    // Legs.
    for side in [-1.0f32, 1.0] {
        quads.push(UiQuad {
            x: cx + side * torso_w / 4.0 - torso_w / 4.0 + side.max(0.0) * 0.0,
            y: torso_y + torso_h + 2.0 * ui,
            w: torso_w / 2.0 - 2.0 * ui,
            h: leg_h,
            color: skin.legs,
        });
    }
}

/// Builds the whole panel. `drag` renders at the cursor; `mouse` drives
/// hover highlights and the doll's gaze.
#[allow(clippy::too_many_arguments)]
pub fn panel(
    registry: &Registry,
    stacks: &[Stack],
    recipes: &[CraftLine],
    hotbar_items: &[oc_world::BlockId; 9],
    selected: usize,
    drag: Option<ItemId>,
    skin: &Skin,
    mouse: (f32, f32),
    width: f32,
    height: f32,
    ui: f32,
) -> (Vec<UiQuad>, Vec<UiText>) {
    let (px, py) = panel_origin(width, height, ui);
    let x0 = content_x(width, height, ui);
    let mut quads = vec![
        UiQuad { x: px, y: py, w: PANEL_W * ui, h: PANEL_H * ui, color: [0.05, 0.05, 0.08, 0.93] },
        // Character pane backdrop.
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
        UiText { text: "CRAFTING".into(), x: x0, y: py + 16.0 * ui, scale: ui * 0.9 },
        UiText {
            text: "DRAG A BLOCK ONTO THE HOTBAR".into(),
            x: x0,
            y: py + (PANEL_H - PAD) * ui - (SLOT + 14.0) * ui,
            scale: ui * 0.85,
        },
    ];

    doll(&mut quads, &mut texts, skin, mouse, width, height, ui);

    let hover = hit(mouse, width, height, ui, stacks.len(), recipes.len());

    // The recipe list.
    for (row, line) in recipes.iter().enumerate() {
        let (x, y, w, h) = recipe_rect(row, width, height, ui);
        if hover == Hit::Recipe(row) && line.craftable {
            quads.push(UiQuad { x, y, w, h, color: [0.2, 0.3, 0.2, 0.9] });
        }
        let mut text = line.label.clone();
        // The number-key prefix is meaningless here.
        if let Some(rest) = text.split_once(' ').map(|(_, rest)| rest.to_owned()) {
            text = rest;
        }
        texts.push(UiText {
            text,
            x: x + 2.0 * ui,
            y: y + 2.0 * ui,
            scale: ui * if line.craftable { 1.0 } else { 0.85 },
        });
    }

    // The fixed slot grid: filled stacks first, empty squares after.
    for index in 0..GRID_COLS * GRID_ROWS {
        let (x, y, w, h) = grid_slot_rect(index, width, height, ui);
        let filled = index < stacks.len();
        let lit = filled && hover == Hit::Stack(index);
        quads.push(UiQuad {
            x,
            y,
            w,
            h,
            color: if lit {
                [0.25, 0.25, 0.3, 0.9]
            } else if filled {
                [0.13, 0.13, 0.17, 0.9]
            } else {
                [0.09, 0.09, 0.12, 0.85]
            },
        });
        if let Some(stack) = stacks.get(index) {
            let inset = 5.0 * ui;
            quads.push(UiQuad {
                x: x + inset,
                y: y + inset,
                w: w - 2.0 * inset,
                h: h - 2.0 * inset,
                color: item_swatch(registry, stack.item),
            });
            let label = stack.count.to_string();
            texts.push(UiText {
                text: label.clone(),
                x: x + w - label.len() as f32 * 6.0 * ui - 2.0 * ui,
                y: y + h - 9.0 * ui,
                scale: ui,
            });
        }
    }

    // The hotbar binding row, numbered like the keys.
    for (slot, &block) in hotbar_items.iter().enumerate() {
        let (x, y, w, h) = hotbar_slot_rect(slot, width, height, ui);
        let lit = hover == Hit::HotbarSlot(slot) || slot == selected;
        quads.push(UiQuad {
            x,
            y,
            w,
            h,
            color: if lit { [0.3, 0.3, 0.36, 0.95] } else { [0.13, 0.13, 0.17, 0.9] },
        });
        let inset = 5.0 * ui;
        let mut swatch = block_swatch(block);
        swatch[3] = 0.95;
        quads.push(UiQuad {
            x: x + inset,
            y: y + inset,
            w: w - 2.0 * inset,
            h: h - 2.0 * inset,
            color: swatch,
        });
        texts.push(UiText {
            text: (slot + 1).to_string(),
            x: x + 2.0 * ui,
            y: y + 2.0 * ui,
            scale: ui * 0.8,
        });
    }

    // The dragged block rides the cursor.
    if let Some(item) = drag {
        let size = SLOT * 0.7 * ui;
        quads.push(UiQuad {
            x: mouse.0 - size / 2.0,
            y: mouse.1 - size / 2.0,
            w: size,
            h: size,
            color: item_swatch(registry, item),
        });
    }

    (quads, texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_match_geometry() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let rect = grid_slot_rect(0, w, h, ui);
        assert_eq!(hit((rect.0 + 2.0, rect.1 + 2.0), w, h, ui, 3, 2), Hit::Stack(0));
        let hb = hotbar_slot_rect(4, w, h, ui);
        assert_eq!(hit((hb.0 + 1.0, hb.1 + 1.0), w, h, ui, 3, 2), Hit::HotbarSlot(4));
        let rc = recipe_rect(1, w, h, ui);
        assert_eq!(hit((rc.0 + 1.0, rc.1 + 1.0), w, h, ui, 3, 2), Hit::Recipe(1));
        assert_eq!(hit((1.0, 1.0), w, h, ui, 3, 2), Hit::None);
        // Empty grid squares are not stacks.
        let empty = grid_slot_rect(20, w, h, ui);
        assert_eq!(hit((empty.0 + 2.0, empty.1 + 2.0), w, h, ui, 3, 2), Hit::None);
    }

    #[test]
    fn grid_and_recipes_do_not_overlap() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let last_recipe = recipe_rect(4, w, h, ui);
        let first_slot = grid_slot_rect(0, w, h, ui);
        assert!(
            last_recipe.1 + last_recipe.3 <= first_slot.1,
            "recipes {last_recipe:?} run into the grid {first_slot:?}"
        );
        let last_row = grid_slot_rect(GRID_COLS * GRID_ROWS - 1, w, h, ui);
        let hotbar = hotbar_slot_rect(0, w, h, ui);
        assert!(last_row.1 + last_row.3 <= hotbar.1, "grid runs into the hotbar");
    }

    #[test]
    fn panel_renders_stacks_and_recipes() {
        let registry = Registry::load_default().unwrap();
        let apple = registry.find("oc:apple").unwrap();
        let stacks = vec![Stack { item: apple, count: 3, name: "Apple".into() }];
        let recipes = crate::craft_menu::lines(&registry, |_| 5);
        let items = crate::hotbar::ITEMS;
        let skin = crate::avatar::load_skin();
        let (quads, texts) = panel(
            &registry, &stacks, &recipes, &items, 0, None, &skin, (0.0, 0.0), 1600.0, 1000.0, 1.0,
        );
        // Background + doll + 27 grid squares + 9 hotbar slots at least.
        assert!(quads.len() > 40);
        assert!(texts.len() >= 4 + recipes.len());
    }
}
