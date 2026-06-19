//! The inventory screen (E/C).
//!
//! Survival shows one panel: a paper-doll on the left, a 3×3 crafting grid
//! with a result slot, the main storage grid, and the hotbar row. Items move
//! slot-to-slot through a cursor stack picked up and put down by clicking.
//!
//! Creative is **tabbed**: category tabs (left) and a Search tab (top-right)
//! show the infinite item **palette** above the hotbar; an Inventory tab
//! (bottom-right) opens the survival layout plus a **trash** slot. Picking a
//! palette item puts a stack on the cursor.
//!
//! Storage and the crafting grid are server-authoritative; this screen is
//! pure presentation plus hit-testing. Storage 0..9 is the hotbar row, 9..36
//! the main grid.

use oc_assets::{ItemId, Registry};
use oc_renderer::{UiQuad, UiText, block_swatch};

use crate::avatar::Skin;

/// One slot: an item with a count, or empty.
type Slot = Option<(ItemId, u32)>;

// Logical units, multiplied by the effective UI scale.
const SLOT: f32 = 34.0;
const GAP: f32 = 4.0;
const PAD: f32 = 10.0;
/// Character pane width (left side, survival / Inventory tab).
const DOLL_W: f32 = 175.0;
/// Gap between the crafting grid and its result slot (room for the arrow).
const OUT_GAP: f32 = 32.0;
/// Vertical gap below the crafting grid before the main storage grid.
const SECTION_GAP: f32 = 18.0;
/// Vertical gap between the main grid and the hotbar row.
const ROW_GAP: f32 = 12.0;
/// Tab button size.
const TAB_W: f32 = 50.0;
const TAB_H: f32 = 18.0;

const PANEL_W: f32 = PAD * 3.0 + DOLL_W + (9.0 * (SLOT + GAP) - GAP);
const PANEL_H: f32 = 350.0;

/// Which creative tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreativeTab {
    /// A category tab (index into `Creative::categories`).
    Category(usize),
    /// The search tab (top-right).
    Search,
    /// The survival-like inventory tab (bottom-right).
    Inventory,
}

/// Creative-screen state, supplied by the session. `None` = survival.
pub struct Creative<'a> {
    /// Category tab labels, in display order.
    pub categories: &'a [String],
    pub active: CreativeTab,
    /// Current search text (shown on the Search tab).
    pub search: &'a str,
    /// Items shown on the active palette/Search tab (already filtered).
    pub palette: &'a [ItemId],
    /// First visible palette row.
    pub scroll: usize,
}

/// What the cursor is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Storage slot 0..36 (0..9 is the hotbar row).
    Storage(usize),
    /// Crafting-grid slot 0..9.
    Craft(usize),
    /// The crafting result slot.
    Output,
    /// A creative-palette item (per-load id).
    Palette(u16),
    /// The creative trash slot.
    Trash,
    /// A tab click.
    Tab(CreativeTab),
    None,
}

fn panel_origin(width: f32, height: f32, ui: f32) -> (f32, f32) {
    (((width - PANEL_W * ui) / 2.0), ((height - PANEL_H * ui) / 2.0))
}

/// Left edge of the right-hand content column (survival / Inventory tab).
fn col_x0(width: f32, height: f32, ui: f32) -> f32 {
    panel_origin(width, height, ui).0 + (PAD * 2.0 + DOLL_W) * ui
}

fn content_top(py: f32, ui: f32) -> f32 {
    py + 28.0 * ui
}

fn step(ui: f32) -> f32 {
    (SLOT + GAP) * ui
}

fn craft_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let (x0, top) = (col_x0(w, h, ui), content_top(py, ui));
    (x0 + (i % 3) as f32 * step(ui), top + (i / 3) as f32 * step(ui), SLOT * ui, SLOT * ui)
}

fn output_rect(w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let (x0, top) = (col_x0(w, h, ui), content_top(py, ui));
    (x0 + 3.0 * step(ui) + OUT_GAP * ui, top + step(ui), SLOT * ui, SLOT * ui)
}

/// Trash slot (creative Inventory tab): just right of the result slot.
fn trash_rect(w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let out = output_rect(w, h, ui);
    (out.0 + (SLOT + 10.0) * ui, out.1, SLOT * ui, SLOT * ui)
}

fn main_top(py: f32, ui: f32) -> f32 {
    content_top(py, ui) + 3.0 * step(ui) + SECTION_GAP * ui
}

/// Main storage slot rect for storage index 9..36 (`i` = index - 9).
fn main_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let (x0, top) = (col_x0(w, h, ui), main_top(py, ui));
    (x0 + (i % 9) as f32 * step(ui), top + (i / 9) as f32 * step(ui), SLOT * ui, SLOT * ui)
}

fn hotbar_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let (x0, top) = (col_x0(w, h, ui), main_top(py, ui) + 3.0 * step(ui) + ROW_GAP * ui);
    (x0 + i as f32 * step(ui), top, SLOT * ui, SLOT * ui)
}

// --- Creative palette geometry (full-width, centered) ---

const GRID_W: f32 = 9.0 * (SLOT + GAP) - GAP;

fn palette_grid_x0(w: f32, h: f32, ui: f32) -> f32 {
    let (px, _) = panel_origin(w, h, ui);
    px + (PANEL_W - GRID_W) / 2.0 * ui
}

fn palette_top(w: f32, h: f32, ui: f32, search: bool) -> f32 {
    let (_, py) = panel_origin(w, h, ui);
    py + (28.0 + if search { 16.0 } else { 0.0 }) * ui
}

/// Bottom of the panel, where the centered palette-view hotbar sits.
fn palette_hotbar_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (_, py) = panel_origin(w, h, ui);
    let x0 = palette_grid_x0(w, h, ui);
    let top = py + (PANEL_H - PAD - SLOT) * ui;
    (x0 + i as f32 * step(ui), top, SLOT * ui, SLOT * ui)
}

fn palette_cell_rect(k: usize, w: f32, h: f32, ui: f32, search: bool) -> (f32, f32, f32, f32) {
    let x0 = palette_grid_x0(w, h, ui);
    let top = palette_top(w, h, ui, search);
    (x0 + (k % 9) as f32 * step(ui), top + (k / 9) as f32 * step(ui), SLOT * ui, SLOT * ui)
}

/// How many palette rows fit between the grid top and the hotbar.
fn palette_visible_rows(w: f32, h: f32, ui: f32, search: bool) -> usize {
    let (_, py) = panel_origin(w, h, ui);
    let top = palette_top(w, h, ui, search);
    let hot_top = py + (PANEL_H - PAD - SLOT) * ui;
    let avail = hot_top - 8.0 * ui - top;
    (avail / step(ui)).floor().max(1.0) as usize
}

fn cat_tab_rect(i: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(w, h, ui);
    (px + (PAD + i as f32 * (TAB_W + 3.0)) * ui, py + 4.0 * ui, TAB_W * ui, TAB_H * ui)
}

fn search_tab_rect(w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(w, h, ui);
    (px + (PANEL_W - PAD - TAB_W) * ui, py + 4.0 * ui, TAB_W * ui, TAB_H * ui)
}

fn inv_tab_rect(w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
    let (px, py) = panel_origin(w, h, ui);
    (px + (PANEL_W - PAD - TAB_W) * ui, py + (PANEL_H - PAD - TAB_H) * ui, TAB_W * ui, TAB_H * ui)
}

fn inside(pos: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    pos.0 >= rect.0 && pos.0 < rect.0 + rect.2 && pos.1 >= rect.1 && pos.1 < rect.1 + rect.3
}

/// Survival-layout hit test (also the creative Inventory tab; `with_trash`).
fn survival_hit(pos: (f32, f32), w: f32, h: f32, ui: f32, with_trash: bool) -> Hit {
    for slot in 0..9 {
        if inside(pos, hotbar_rect(slot, w, h, ui)) {
            return Hit::Storage(slot);
        }
    }
    for i in 0..27 {
        if inside(pos, main_rect(i, w, h, ui)) {
            return Hit::Storage(9 + i);
        }
    }
    for i in 0..9 {
        if inside(pos, craft_rect(i, w, h, ui)) {
            return Hit::Craft(i);
        }
    }
    if inside(pos, output_rect(w, h, ui)) {
        return Hit::Output;
    }
    if with_trash && inside(pos, trash_rect(w, h, ui)) {
        return Hit::Trash;
    }
    Hit::None
}

/// Hit test in framebuffer pixels. `creative` = `None` for survival.
pub fn hit(pos: (f32, f32), width: f32, height: f32, ui: f32, creative: Option<&Creative>) -> Hit {
    let Some(c) = creative else {
        return survival_hit(pos, width, height, ui, false);
    };
    // Tabs are clickable from every creative tab.
    for i in 0..c.categories.len() {
        if inside(pos, cat_tab_rect(i, width, height, ui)) {
            return Hit::Tab(CreativeTab::Category(i));
        }
    }
    if inside(pos, search_tab_rect(width, height, ui)) {
        return Hit::Tab(CreativeTab::Search);
    }
    if inside(pos, inv_tab_rect(width, height, ui)) {
        return Hit::Tab(CreativeTab::Inventory);
    }
    if c.active == CreativeTab::Inventory {
        return survival_hit(pos, width, height, ui, true);
    }
    // Palette / Search tab: items, then the hotbar row.
    let search = c.active == CreativeTab::Search;
    let rows = palette_visible_rows(width, height, ui, search);
    for k in 0..rows * 9 {
        if inside(pos, palette_cell_rect(k, width, height, ui, search)) {
            return match c.palette.get(c.scroll * 9 + k) {
                Some(item) => Hit::Palette(item.0),
                None => Hit::None,
            };
        }
    }
    for i in 0..9 {
        if inside(pos, palette_hotbar_rect(i, width, height, ui)) {
            return Hit::Storage(i);
        }
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

/// Draws a stack's swatch + count inside a slot rect. `count` shown when > 1.
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
    // Pane backdrop.
    quads.push(UiQuad {
        x: px + PAD * ui / 2.0,
        y: py + 24.0 * ui,
        w: (DOLL_W + PAD) * ui,
        h: (PANEL_H - 32.0) * ui,
        color: [0.08, 0.09, 0.12, 0.9],
    });

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

    let head_cy = top + head / 2.0;
    let dx = (mouse.0 - cx).clamp(-600.0, 600.0) / 600.0;
    let dy = (mouse.1 - head_cy).clamp(-600.0, 600.0) / 600.0;
    let (lean_x, lean_y) = (dx * 4.0 * ui, dy * 3.0 * ui);

    quads.push(UiQuad { x: cx - head / 2.0 + lean_x, y: top + lean_y, w: head, h: head, color: skin.head });
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

/// The survival layout: doll, crafting grid + result, main grid, hotbar, and
/// (creative Inventory tab) a trash slot. Shared by survival and creative.
#[allow(clippy::too_many_arguments)]
fn draw_inventory_view(
    quads: &mut Vec<UiQuad>,
    texts: &mut Vec<UiText>,
    registry: &Registry,
    slots: &[Slot; 36],
    craft: &[Slot; 9],
    craft_result: Option<(ItemId, u8)>,
    selected: usize,
    skin: &Skin,
    mouse: (f32, f32),
    hover: Hit,
    w: f32,
    h: f32,
    ui: f32,
    show_trash: bool,
) {
    doll(quads, texts, skin, mouse, w, h, ui);

    // Crafting grid + arrow + result.
    for i in 0..9 {
        let rect = craft_rect(i, w, h, ui);
        slot_quad(quads, rect, hover == Hit::Craft(i));
        draw_stack(quads, texts, rect, registry, craft[i], ui);
    }
    let out = output_rect(w, h, ui);
    texts.push(UiText { text: ">".into(), x: out.0 - 18.0 * ui, y: out.1 + out.3 / 2.0 - 4.0 * ui, scale: ui * 1.2 });
    slot_quad(quads, out, hover == Hit::Output);
    draw_stack(quads, texts, out, registry, craft_result.map(|(it, n)| (it, n as u32)), ui);

    if show_trash {
        let t = trash_rect(w, h, ui);
        quads.push(UiQuad {
            x: t.0,
            y: t.1,
            w: t.2,
            h: t.3,
            color: if hover == Hit::Trash { [0.4, 0.15, 0.15, 0.95] } else { [0.22, 0.10, 0.10, 0.9] },
        });
        texts.push(UiText { text: "X".into(), x: t.0 + t.2 / 2.0 - 3.0 * ui, y: t.1 + t.3 / 2.0 - 4.0 * ui, scale: ui });
    }

    // Main grid (storage 9..36).
    for i in 0..27 {
        let rect = main_rect(i, w, h, ui);
        slot_quad(quads, rect, hover == Hit::Storage(9 + i));
        draw_stack(quads, texts, rect, registry, slots[9 + i], ui);
    }
    // Hotbar (storage 0..9); selected stays lit.
    for i in 0..9 {
        let rect = hotbar_rect(i, w, h, ui);
        slot_quad(quads, rect, hover == Hit::Storage(i) || i == selected);
        draw_stack(quads, texts, rect, registry, slots[i], ui);
    }
}

fn tab_button(quads: &mut Vec<UiQuad>, texts: &mut Vec<UiText>, rect: (f32, f32, f32, f32), label: &str, active: bool, ui: f32) {
    quads.push(UiQuad {
        x: rect.0,
        y: rect.1,
        w: rect.2,
        h: rect.3,
        color: if active { [0.3, 0.3, 0.36, 0.95] } else { [0.12, 0.12, 0.16, 0.9] },
    });
    let short: String = label.chars().take(6).collect();
    texts.push(UiText { text: short, x: rect.0 + 4.0 * ui, y: rect.1 + 5.0 * ui, scale: ui * 0.8 });
}

fn draw_tabs(quads: &mut Vec<UiQuad>, texts: &mut Vec<UiText>, c: &Creative, w: f32, h: f32, ui: f32) {
    for (i, label) in c.categories.iter().enumerate() {
        tab_button(quads, texts, cat_tab_rect(i, w, h, ui), label, c.active == CreativeTab::Category(i), ui);
    }
    tab_button(quads, texts, search_tab_rect(w, h, ui), "FIND", c.active == CreativeTab::Search, ui);
    tab_button(quads, texts, inv_tab_rect(w, h, ui), "INV", c.active == CreativeTab::Inventory, ui);
}

/// The palette + hotbar view (category / Search tab).
fn draw_palette(
    quads: &mut Vec<UiQuad>,
    texts: &mut Vec<UiText>,
    registry: &Registry,
    slots: &[Slot; 36],
    c: &Creative,
    selected: usize,
    hover: Hit,
    w: f32,
    h: f32,
    ui: f32,
) {
    let search = c.active == CreativeTab::Search;
    if search {
        let x0 = palette_grid_x0(w, h, ui);
        let (_, py) = panel_origin(w, h, ui);
        texts.push(UiText { text: format!("SEARCH: {}_", c.search), x: x0, y: py + 28.0 * ui, scale: ui * 0.9 });
    }
    let rows = palette_visible_rows(w, h, ui, search);
    for k in 0..rows * 9 {
        let rect = palette_cell_rect(k, w, h, ui, search);
        let item = c.palette.get(c.scroll * 9 + k).copied();
        slot_quad(quads, rect, item.is_some_and(|it| hover == Hit::Palette(it.0)));
        if let Some(it) = item {
            let inset = 4.0 * ui;
            quads.push(UiQuad {
                x: rect.0 + inset,
                y: rect.1 + inset,
                w: rect.2 - 2.0 * inset,
                h: rect.3 - 2.0 * inset,
                color: item_swatch(registry, it),
            });
        }
    }

    // Scrollbar when the palette overflows.
    let total_rows = c.palette.len().div_ceil(9).max(1);
    if total_rows > rows {
        let x0 = palette_grid_x0(w, h, ui);
        let top = palette_top(w, h, ui, search);
        let track_h = rows as f32 * step(ui);
        let track_x = x0 + GRID_W * ui + 4.0 * ui;
        quads.push(UiQuad { x: track_x, y: top, w: 5.0 * ui, h: track_h, color: [0.1, 0.1, 0.13, 0.8] });
        let thumb_h = (rows as f32 / total_rows as f32 * track_h).max(8.0 * ui);
        let max_scroll = (total_rows - rows) as f32;
        let t = if max_scroll > 0.0 { (c.scroll as f32 / max_scroll).min(1.0) } else { 0.0 };
        quads.push(UiQuad { x: track_x, y: top + t * (track_h - thumb_h), w: 5.0 * ui, h: thumb_h, color: [0.4, 0.4, 0.46, 0.9] });
    }

    // The hotbar row (storage 0..9), centered under the palette.
    for i in 0..9 {
        let rect = palette_hotbar_rect(i, w, h, ui);
        slot_quad(quads, rect, hover == Hit::Storage(i) || i == selected);
        draw_stack(quads, texts, rect, registry, slots[i], ui);
    }
}

/// The item under the cursor for a given hover hit, if that slot holds one.
/// Tabs, the trash, empty slots, and `None` yield nothing (no tooltip).
fn hovered_item(
    hover: Hit,
    slots: &[Slot; 36],
    craft: &[Slot; 9],
    result: Option<(ItemId, u8)>,
) -> Option<ItemId> {
    match hover {
        Hit::Storage(i) => slots.get(i).copied().flatten().map(|(it, _)| it),
        Hit::Craft(i) => craft.get(i).copied().flatten().map(|(it, _)| it),
        Hit::Output => result.map(|(it, _)| it),
        Hit::Palette(id) => Some(ItemId(id)),
        Hit::Trash | Hit::Tab(_) | Hit::None => None,
    }
}

/// Draws a name tooltip near the cursor: a bordered dark box with the
/// already-localized `name`. The bitmap font is uppercase-only, so it reads
/// in caps like the rest of the UI. Positioned below-right of the cursor and
/// clamped so the box stays fully on-screen.
fn tooltip(
    quads: &mut Vec<UiQuad>,
    texts: &mut Vec<UiText>,
    name: &str,
    mouse: (f32, f32),
    w: f32,
    h: f32,
    ui: f32,
) {
    if name.is_empty() {
        return;
    }
    let pad = 5.0 * ui;
    let char_w = 6.0 * ui; // font cell advance (CELL_W)
    let text_w = name.chars().count() as f32 * char_w;
    let (box_w, box_h) = (text_w + 2.0 * pad, 7.0 * ui + 2.0 * pad);
    let mut x = mouse.0 + 14.0 * ui;
    let mut y = mouse.1 + 14.0 * ui;
    if x + box_w > w {
        x = (mouse.0 - box_w - 10.0 * ui).max(0.0);
    }
    if y + box_h > h {
        y = (h - box_h).max(0.0);
    }
    // Border then fill (a later quad draws over an earlier one); the renderer
    // always paints text above quads, so the label stays legible on top.
    let b = ui;
    quads.push(UiQuad { x: x - b, y: y - b, w: box_w + 2.0 * b, h: box_h + 2.0 * b, color: [0.35, 0.35, 0.42, 0.95] });
    quads.push(UiQuad { x, y, w: box_w, h: box_h, color: [0.05, 0.05, 0.08, 0.97] });
    texts.push(UiText { text: name.to_string(), x: x + pad, y: y + pad, scale: ui });
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
    creative: Option<&Creative>,
) -> (Vec<UiQuad>, Vec<UiText>) {
    let (px, py) = panel_origin(width, height, ui);
    let mut quads =
        vec![UiQuad { x: px, y: py, w: PANEL_W * ui, h: PANEL_H * ui, color: [0.05, 0.05, 0.08, 0.93] }];
    let mut texts = Vec::new();
    let hover = hit(mouse, width, height, ui, creative);

    match creative {
        None => {
            texts.push(UiText { text: "INVENTORY  [E] CLOSE".into(), x: px + PAD * ui, y: py + PAD * ui, scale: ui });
            draw_inventory_view(
                &mut quads, &mut texts, registry, slots, craft, craft_result, selected, skin, mouse,
                hover, width, height, ui, false,
            );
        }
        Some(c) => {
            draw_tabs(&mut quads, &mut texts, c, width, height, ui);
            if c.active == CreativeTab::Inventory {
                draw_inventory_view(
                    &mut quads, &mut texts, registry, slots, craft, craft_result, selected, skin,
                    mouse, hover, width, height, ui, true,
                );
            } else {
                draw_palette(&mut quads, &mut texts, registry, slots, c, selected, hover, width, height, ui);
            }
        }
    }

    // The cursor stack rides the mouse (every view).
    if let Some((item, count)) = cursor {
        let size = SLOT * 0.8 * ui;
        let (cx, cy) = (mouse.0 - size / 2.0, mouse.1 - size / 2.0);
        quads.push(UiQuad { x: cx, y: cy, w: size, h: size, color: item_swatch(registry, item) });
        if count > 1 {
            let label = count.to_string();
            texts.push(UiText {
                text: label.clone(),
                x: cx + size - label.len() as f32 * 6.0 * ui - 1.0 * ui,
                y: cy + size - 9.0 * ui,
                scale: ui,
            });
        }
    }

    // Hover tooltip: the localized name of the item under the cursor, shown
    // only when the cursor isn't carrying a stack (so it never hides the
    // dragged item). Added last so its box sits above the rest of the panel.
    if cursor.is_none()
        && let Some(item) = hovered_item(hover, slots, craft, craft_result)
    {
        tooltip(&mut quads, &mut texts, registry.item_name(item), mouse, width, height, ui);
    }

    (quads, texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats() -> Vec<String> {
        vec!["building".into(), "natural".into()]
    }

    #[test]
    fn survival_hits_map_to_the_right_slots() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let hb = hotbar_rect(0, w, h, ui);
        assert_eq!(hit((hb.0 + 2.0, hb.1 + 2.0), w, h, ui, None), Hit::Storage(0));
        let m = main_rect(0, w, h, ui);
        assert_eq!(hit((m.0 + 2.0, m.1 + 2.0), w, h, ui, None), Hit::Storage(9));
        let last = main_rect(26, w, h, ui);
        assert_eq!(hit((last.0 + 2.0, last.1 + 2.0), w, h, ui, None), Hit::Storage(35));
        let c = craft_rect(4, w, h, ui);
        assert_eq!(hit((c.0 + 2.0, c.1 + 2.0), w, h, ui, None), Hit::Craft(4));
        let o = output_rect(w, h, ui);
        assert_eq!(hit((o.0 + 2.0, o.1 + 2.0), w, h, ui, None), Hit::Output);
        assert_eq!(hit((1.0, 1.0), w, h, ui, None), Hit::None);
        // No trash slot in survival.
        let t = trash_rect(w, h, ui);
        assert_ne!(hit((t.0 + 2.0, t.1 + 2.0), w, h, ui, None), Hit::Trash);
    }

    #[test]
    fn survival_regions_do_not_overlap() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let last_craft = craft_rect(8, w, h, ui);
        let first_main = main_rect(0, w, h, ui);
        assert!(last_craft.1 + last_craft.3 <= first_main.1, "craft into main");
        let last_main = main_rect(26, w, h, ui);
        let hotbar = hotbar_rect(0, w, h, ui);
        assert!(last_main.1 + last_main.3 <= hotbar.1, "main into hotbar");
    }

    #[test]
    fn creative_tabs_palette_and_hotbar_hit() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let registry = Registry::load_default().unwrap();
        let palette = registry.items_in_category("building");
        assert!(!palette.is_empty());
        let c = Creative { categories: &cats(), active: CreativeTab::Category(0), search: "", palette: &palette, scroll: 0 };

        // Category / Search / Inventory tabs.
        let t0 = cat_tab_rect(0, w, h, ui);
        assert_eq!(hit((t0.0 + 2.0, t0.1 + 2.0), w, h, ui, Some(&c)), Hit::Tab(CreativeTab::Category(0)));
        let ts = search_tab_rect(w, h, ui);
        assert_eq!(hit((ts.0 + 2.0, ts.1 + 2.0), w, h, ui, Some(&c)), Hit::Tab(CreativeTab::Search));
        let ti = inv_tab_rect(w, h, ui);
        assert_eq!(hit((ti.0 + 2.0, ti.1 + 2.0), w, h, ui, Some(&c)), Hit::Tab(CreativeTab::Inventory));

        // First palette cell yields the first item; the hotbar still works.
        let cell = palette_cell_rect(0, w, h, ui, false);
        assert_eq!(hit((cell.0 + 2.0, cell.1 + 2.0), w, h, ui, Some(&c)), Hit::Palette(palette[0].0));
        let pb = palette_hotbar_rect(3, w, h, ui);
        assert_eq!(hit((pb.0 + 2.0, pb.1 + 2.0), w, h, ui, Some(&c)), Hit::Storage(3));
    }

    #[test]
    fn creative_inventory_tab_shows_trash() {
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let palette: Vec<ItemId> = Vec::new();
        let c = Creative { categories: &cats(), active: CreativeTab::Inventory, search: "", palette: &palette, scroll: 0 };
        let t = trash_rect(w, h, ui);
        assert_eq!(hit((t.0 + 2.0, t.1 + 2.0), w, h, ui, Some(&c)), Hit::Trash);
        // And the main grid is hittable on this tab.
        let m = main_rect(0, w, h, ui);
        assert_eq!(hit((m.0 + 2.0, m.1 + 2.0), w, h, ui, Some(&c)), Hit::Storage(9));
    }

    #[test]
    fn panel_renders_survival_and_creative() {
        let registry = Registry::load_default().unwrap();
        let stone = registry.find("oc:stone").unwrap();
        let mut slots: [Slot; 36] = [None; 36];
        slots[0] = Some((stone, 64));
        let craft: [Slot; 9] = [None; 9];
        let skin = crate::avatar::load_skin();

        let (q, t) = panel(&registry, &slots, &craft, Some((stone, 5)), None, 0, &skin, (0.0, 0.0), 1600.0, 1000.0, 1.0, None);
        assert!(q.len() > 50);
        assert!(t.iter().any(|x| x.text == "64"));

        let palette = registry.items_in_category("building");
        let c = Creative { categories: &cats(), active: CreativeTab::Category(0), search: "", palette: &palette, scroll: 0 };
        let (qc, tc) = panel(&registry, &slots, &craft, None, None, 0, &skin, (0.0, 0.0), 1600.0, 1000.0, 1.0, Some(&c));
        assert!(qc.len() > 20);
        assert!(tc.iter().any(|x| x.text == "FIND"), "tab labels render");
    }

    #[test]
    fn hovering_a_filled_slot_shows_its_localized_name() {
        let registry = Registry::load_default().unwrap();
        let stone = registry.find("oc:stone").unwrap();
        let mut slots: [Slot; 36] = [None; 36];
        slots[0] = Some((stone, 64));
        let craft: [Slot; 9] = [None; 9];
        let skin = crate::avatar::load_skin();
        let (w, h, ui) = (1600.0, 1000.0, 1.0);
        let hb = hotbar_rect(0, w, h, ui);
        let mouse = (hb.0 + 2.0, hb.1 + 2.0);
        let name = registry.item_name(stone);

        // Empty cursor over a filled slot: the localized name shows as a tooltip.
        let (_q, t) = panel(&registry, &slots, &craft, None, None, 0, &skin, mouse, w, h, ui, None);
        assert!(t.iter().any(|x| x.text == name), "tooltip shows {name:?}");

        // Carrying a stack: no tooltip (it would hide the dragged item).
        let (_q2, t2) =
            panel(&registry, &slots, &craft, Some((stone, 1)), None, 0, &skin, mouse, w, h, ui, None);
        assert!(!t2.iter().any(|x| x.text == name), "no tooltip while dragging");

        // Over empty space: no tooltip.
        let (_q3, t3) =
            panel(&registry, &slots, &craft, None, None, 0, &skin, (1.0, 1.0), w, h, ui, None);
        assert!(!t3.iter().any(|x| x.text == name), "no tooltip over empty space");
    }
}
