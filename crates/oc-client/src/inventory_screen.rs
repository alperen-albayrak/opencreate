//! The inventory screen (E): every carried stack in a grid, a clickable
//! recipe list, and a rebindable hotbar — drag a block from the grid onto
//! a hotbar slot to bind it there. The mouse is released while open.
//!
//! Inventory storage stays a server-authoritative item->count map; this
//! screen is pure presentation plus the local hotbar binding.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiQuad, UiText, block_swatch};

use crate::craft_menu::CraftLine;

// Logical units, multiplied by the effective UI scale.
const PANEL_W: f32 = 545.0;
const PANEL_H: f32 = 300.0;
const PAD: f32 = 10.0;
const SLOT: f32 = 34.0;
const GAP: f32 = 4.0;
const GRID_COLS: usize = 7;
const RECIPES_X: f32 = 285.0;
const RECIPE_H: f32 = 13.0;

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

fn grid_slot_rect(index: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(width, height, ui);
    let (col, row) = (index % GRID_COLS, index / GRID_COLS);
    (
        px + PAD * ui + col as f32 * (SLOT + GAP) * ui,
        py + 32.0 * ui + row as f32 * (SLOT + GAP) * ui,
        SLOT * ui,
        SLOT * ui,
    )
}

fn hotbar_slot_rect(slot: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(width, height, ui);
    (
        px + PAD * ui + slot as f32 * (SLOT * 0.82 + GAP) * ui,
        py + (PANEL_H - PAD) * ui - SLOT * 0.82 * ui,
        SLOT * 0.82 * ui,
        SLOT * 0.82 * ui,
    )
}

fn recipe_rect(row: usize, width: f32, height: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(width, height, ui);
    (
        px + RECIPES_X * ui,
        py + 32.0 * ui + row as f32 * RECIPE_H * ui,
        (PANEL_W - RECIPES_X - PAD) * ui,
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
    for index in 0..stacks {
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

/// Builds the whole panel. `drag` renders at the cursor; `mouse` drives
/// hover highlights.
#[allow(clippy::too_many_arguments)]
pub fn panel(
    registry: &Registry,
    stacks: &[Stack],
    recipes: &[CraftLine],
    hotbar_items: &[oc_world::BlockId; 9],
    selected: usize,
    drag: Option<ItemId>,
    mouse: (f32, f32),
    width: f32,
    height: f32,
    ui: f32,
) -> (Vec<UiQuad>, Vec<UiText>) {
    let (px, py) = panel_origin(width, height, ui);
    let mut quads = vec![UiQuad {
        x: px,
        y: py,
        w: PANEL_W * ui,
        h: PANEL_H * ui,
        color: [0.05, 0.05, 0.08, 0.92],
    }];
    let mut texts = vec![
        UiText {
            text: "INVENTORY  [E] CLOSE".into(),
            x: px + PAD * ui,
            y: py + PAD * ui,
            scale: ui,
        },
        UiText {
            text: "DRAG A BLOCK ONTO THE HOTBAR".into(),
            x: px + PAD * ui,
            y: py + (PANEL_H - PAD) * ui - (SLOT * 0.82 + 12.0) * ui,
            scale: ui * 0.9,
        },
        UiText {
            text: "CRAFT: CLICK A RECIPE".into(),
            x: px + RECIPES_X * ui,
            y: py + PAD * ui + 10.0 * ui,
            scale: ui * 0.9,
        },
    ];

    let hover = hit(mouse, width, height, ui, stacks.len(), recipes.len());

    // The stack grid.
    for (index, stack) in stacks.iter().enumerate() {
        let (x, y, w, h) = grid_slot_rect(index, width, height, ui);
        let lit = hover == Hit::Stack(index);
        quads.push(UiQuad {
            x,
            y,
            w,
            h,
            color: if lit { [0.25, 0.25, 0.3, 0.9] } else { [0.12, 0.12, 0.15, 0.9] },
        });
        let inset = 4.0 * ui;
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

    // The hotbar binding row.
    for (slot, &block) in hotbar_items.iter().enumerate() {
        let (x, y, w, h) = hotbar_slot_rect(slot, width, height, ui);
        let lit = hover == Hit::HotbarSlot(slot) || slot == selected;
        quads.push(UiQuad {
            x,
            y,
            w,
            h,
            color: if lit { [0.3, 0.3, 0.36, 0.95] } else { [0.12, 0.12, 0.15, 0.9] },
        });
        let inset = 4.0 * ui;
        let mut swatch = block_swatch(block);
        swatch[3] = 0.95;
        quads.push(UiQuad {
            x: x + inset,
            y: y + inset,
            w: w - 2.0 * inset,
            h: h - 2.0 * inset,
            color: swatch,
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
        let inside_pos = (rect.0 + 2.0, rect.1 + 2.0);
        assert_eq!(hit(inside_pos, w, h, ui, 3, 2), Hit::Stack(0));
        let hb = hotbar_slot_rect(4, w, h, ui);
        assert_eq!(hit((hb.0 + 1.0, hb.1 + 1.0), w, h, ui, 3, 2), Hit::HotbarSlot(4));
        let rc = recipe_rect(1, w, h, ui);
        assert_eq!(hit((rc.0 + 1.0, rc.1 + 1.0), w, h, ui, 3, 2), Hit::Recipe(1));
        assert_eq!(hit((1.0, 1.0), w, h, ui, 3, 2), Hit::None);
    }

    #[test]
    fn panel_renders_stacks_and_recipes() {
        let registry = Registry::load_default().unwrap();
        let apple = registry.find("oc:apple").unwrap();
        let stacks = vec![Stack { item: apple, count: 3, name: "Apple".into() }];
        let recipes = crate::craft_menu::lines(&registry, |_| 5);
        let items = crate::hotbar::ITEMS;
        let (quads, texts) =
            panel(&registry, &stacks, &recipes, &items, 0, None, (0.0, 0.0), 1600.0, 1000.0, 1.0);
        assert!(quads.len() > 10);
        // Title + hints + one count label + a text per recipe.
        assert!(texts.len() >= 4 + recipes.len());
    }
}
