//! Menu screens: layout, hit-testing and rendering for the data-driven
//! title/pause menus (`data/menus.ron`) and the dynamic world-selection
//! screen. Pure layout math — testable without a window.

use oc_assets::{MenuDef, Registry};
use oc_renderer::{UiQuad, UiText};

// Logical units: multiplied by the effective UI scale (DPI x setting).
pub const BUTTON_W: f32 = 260.0;
pub const BUTTON_H: f32 = 28.0;
const GAP: f32 = 7.0;
const LABEL_SCALE: f32 = 1.5;
/// The font's glyph advance (font.rs): 6 px per character at scale 1.
const GLYPH_W: f32 = 6.0;
const GLYPH_H: f32 = 7.0;

pub struct Button {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    /// Action id this button fires: a menus.ron action (`oc:resume`) or a
    /// screen-internal one (`play:worldname`).
    pub action: String,
    /// Drawn brighter (focused text fields use this).
    pub highlighted: bool,
}

impl Button {
    pub fn contains(&self, mouse: (f32, f32)) -> bool {
        mouse.0 >= self.x
            && mouse.0 <= self.x + self.w
            && mouse.1 >= self.y
            && mouse.1 <= self.y + self.h
    }
}

/// A laid-out menu for one frame: hit-test with [`hit`], render with
/// [`quads`]/[`texts`].
pub struct MenuView {
    pub title: String,
    title_scale: f32,
    pub buttons: Vec<Button>,
    /// Translucent dim over the world (pause) instead of opaque sky (title).
    dim_background: bool,
    /// Effective UI scale the view was laid out with.
    ui: f32,
}

impl MenuView {
    /// Lays out a data-driven menu (menus.ron) for a `w`×`h` framebuffer
    /// at the given UI scale.
    pub fn from_def(def: &MenuDef, registry: &Registry, w: f32, h: f32, ui: f32, dim: bool) -> Self {
        let x = (w - BUTTON_W * ui) / 2.0;
        let mut y = h * 0.38;
        let buttons = def
            .entries
            .iter()
            .map(|entry| {
                let button = Button {
                    x,
                    y,
                    w: BUTTON_W * ui,
                    h: BUTTON_H * ui,
                    label: registry.text(&entry.label).to_owned(),
                    action: entry.action.clone(),
                    highlighted: false,
                };
                y += (BUTTON_H + GAP) * ui;
                button
            })
            .collect();
        Self {
            title: registry.text(&def.title).to_owned(),
            title_scale: (if dim { 2.5 } else { 4.5 }) * ui,
            buttons,
            dim_background: dim,
            ui,
        }
    }

    /// The action under the mouse, if any.
    pub fn hit(&self, mouse: (f32, f32)) -> Option<&str> {
        self.buttons
            .iter()
            .find(|b| b.contains(mouse))
            .map(|b| b.action.as_str())
    }

    pub fn quads(&self, w: f32, h: f32, mouse: (f32, f32)) -> Vec<UiQuad> {
        let mut quads = Vec::new();
        if self.dim_background {
            quads.push(UiQuad { x: 0.0, y: 0.0, w, h, color: [0.02, 0.02, 0.04, 0.65] });
        }
        for b in &self.buttons {
            let hovered = b.contains(mouse);
            let color = if hovered {
                [0.34, 0.36, 0.42, 0.95]
            } else if b.highlighted {
                [0.24, 0.26, 0.32, 0.92]
            } else {
                [0.15, 0.16, 0.20, 0.88]
            };
            quads.push(UiQuad { x: b.x, y: b.y, w: b.w, h: b.h, color });
        }
        quads
    }

    pub fn texts(&self, w: f32, h: f32) -> Vec<UiText> {
        let mut texts = vec![centered(
            self.title.clone(),
            w / 2.0,
            h * 0.18,
            self.title_scale,
        )];
        let _ = h;
        for b in &self.buttons {
            texts.push(centered(
                b.label.clone(),
                b.x + b.w / 2.0,
                b.y + b.h / 2.0,
                LABEL_SCALE * self.ui,
            ));
        }
        texts
    }
}

/// A text run centered on (cx, cy).
fn centered(text: String, cx: f32, cy: f32, scale: f32) -> UiText {
    let width = text.len() as f32 * GLYPH_W * scale;
    UiText {
        x: cx - width / 2.0,
        y: cy - GLYPH_H * scale / 2.0,
        text,
        scale,
    }
}

// --- world selection --------------------------------------------------------

/// One editable line of a form.
pub struct TextField {
    pub value: String,
    pub focused: bool,
    max_len: usize,
}

impl TextField {
    fn new(max_len: usize) -> Self {
        Self { value: String::new(), focused: false, max_len }
    }

    pub fn type_char(&mut self, c: char) {
        if self.focused && self.value.len() < self.max_len && (c.is_ascii_graphic() || c == ' ') {
            self.value.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.focused {
            self.value.pop();
        }
    }
}

/// Builds a column-of-rows screen (shared by the dynamic screens).
struct Column {
    buttons: Vec<Button>,
    x: f32,
    y: f32,
    ui: f32,
}

impl Column {
    fn new(w: f32, h: f32, top: f32, ui: f32) -> Self {
        Self { buttons: Vec::new(), x: (w - BUTTON_W * ui) / 2.0, y: h * top, ui }
    }

    fn row(&mut self, label: String, action: String, highlighted: bool) {
        self.buttons.push(Button {
            x: self.x,
            y: self.y,
            w: BUTTON_W * self.ui,
            h: BUTTON_H * self.ui,
            label,
            action,
            highlighted,
        });
        self.y += (BUTTON_H + GAP) * self.ui;
    }

    fn space(&mut self) {
        self.y += GAP * 2.0 * self.ui;
    }
}

/// Renders a text field row: value (with caret while focused) or its
/// language-keyed placeholder.
fn field_label(field: &TextField, registry: &Registry, placeholder: &str) -> String {
    if field.value.is_empty() {
        registry.text(placeholder).to_owned()
    } else {
        format!("{}{}", field.value, if field.focused { "_" } else { "" })
    }
}

/// The world-selection screen: existing worlds (delete with click-again
/// confirmation) and the way into world creation.
pub struct WorldsScreen {
    pub worlds: Vec<String>,
    pub pending_delete: Option<String>,
}

impl WorldsScreen {
    pub fn new(worlds: Vec<String>) -> Self {
        Self { worlds, pending_delete: None }
    }

    /// Lays the screen out as a [`MenuView`] (every row is a button).
    pub fn view(&self, registry: &Registry, w: f32, h: f32, ui: f32) -> MenuView {
        let mut column = Column::new(w, h, 0.30, ui);
        if self.worlds.is_empty() {
            column.row(registry.text("worlds.empty").to_owned(), String::new(), false);
        }
        for world in &self.worlds {
            let label = if self.pending_delete.as_deref() == Some(world) {
                format!("{world}  [{}]", registry.text("worlds.delete_confirm"))
            } else {
                format!("{world}  [{} >]", registry.text("worlds.delete"))
            };
            column.row(label, format!("world:{world}"), false);
        }
        column.space();
        column.row(registry.text("worlds.new").to_owned(), "create_screen".into(), false);
        column.row(registry.text("menu.back").to_owned(), "back".into(), false);

        MenuView {
            title: registry.text("menu.worlds").to_owned(),
            title_scale: 3.0 * ui,
            buttons: column.buttons,
            dim_background: false,
            ui,
        }
    }

    /// Splits a world-row click into play vs delete: clicking the right
    /// fifth of the row (the delete tag) arms/fires deletion.
    pub fn world_click(&mut self, world: &str, mouse_x: f32, w: f32, ui: f32) -> WorldAction {
        let delete_zone = mouse_x > (w - BUTTON_W * ui) / 2.0 + BUTTON_W * ui * 0.72;
        if delete_zone {
            if self.pending_delete.as_deref() == Some(world) {
                self.pending_delete = None;
                WorldAction::Delete
            } else {
                self.pending_delete = Some(world.to_owned());
                WorldAction::ArmDelete
            }
        } else {
            WorldAction::Play
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorldAction {
    Play,
    ArmDelete,
    Delete,
}

/// The create-world screen: name, seed, and world options (game mode,
/// cheats — more options join this form as they exist).
pub struct CreateScreen {
    pub name: TextField,
    pub seed: TextField,
    /// Registry index of the starting game mode.
    pub mode: usize,
    /// Allow cheats (changing game mode etc.) in this world.
    pub cheats: bool,
}

impl CreateScreen {
    pub fn new() -> Self {
        let mut name = TextField::new(24);
        name.focused = true; // start typing immediately
        Self { name, seed: TextField::new(24), mode: 0, cheats: false }
    }

    /// Routes typed characters to whichever field is focused.
    pub fn type_char(&mut self, c: char) {
        self.name.type_char(c);
        self.seed.type_char(c);
    }

    pub fn backspace(&mut self) {
        self.name.backspace();
        self.seed.backspace();
    }

    pub fn focus(&mut self, field: &str) {
        self.name.focused = field == "name";
        self.seed.focused = field == "seed";
    }

    /// The selected mode's string id (what the server is told).
    pub fn mode_id(&self, registry: &Registry) -> String {
        registry.mode(oc_assets::ModeId(self.mode as u16)).id.clone()
    }

    pub fn cycle_mode(&mut self, registry: &Registry) {
        self.mode = (self.mode + 1) % registry.mode_count();
    }

    pub fn view(&self, registry: &Registry, w: f32, h: f32, ui: f32) -> MenuView {
        let mut column = Column::new(w, h, 0.32, ui);
        column.row(field_label(&self.name, registry, "worlds.name"), "focus:name".into(), self.name.focused);
        column.row(field_label(&self.seed, registry, "worlds.seed"), "focus:seed".into(), self.seed.focused);
        let mode = registry.mode(oc_assets::ModeId(self.mode as u16));
        column.row(
            format!("{}: {} >", registry.text("create.mode"), mode.name),
            "cycle_create_mode".into(),
            false,
        );
        let cheats = registry.text(if self.cheats { "menu.on" } else { "menu.off" });
        column.row(
            format!("{}: {} >", registry.text("menu.cheats"), cheats),
            "toggle_create_cheats".into(),
            false,
        );
        column.space();
        column.row(registry.text("worlds.create").to_owned(), "create".into(), false);
        column.row(registry.text("menu.back").to_owned(), "back_worlds".into(), false);

        MenuView {
            title: registry.text("worlds.new").to_owned(),
            title_scale: 3.0 * ui,
            buttons: column.buttons,
            dim_background: false,
            ui,
        }
    }
}

/// The in-game mode picker (reached from the pause menu): one row per
/// registered game mode, so modded modes appear automatically. Without
/// cheat permission the list is replaced by an explanation.
pub fn modes_view(registry: &Registry, current: u16, cheats: bool, w: f32, h: f32, ui: f32) -> MenuView {
    let mut column = Column::new(w, h, 0.34, ui);
    if cheats {
        for index in 0..registry.mode_count() {
            let mode = registry.mode(oc_assets::ModeId(index as u16));
            let marker = if index as u16 == current { " [x]" } else { "" };
            column.row(format!("{}{marker}", mode.name), format!("mode:{index}"), false);
        }
    } else {
        column.row(registry.text("menu.cheats_required").to_owned(), String::new(), false);
    }
    column.space();
    column.row(registry.text("menu.back").to_owned(), "back_pause".into(), false);
    MenuView {
        title: registry.text("menu.select_mode").to_owned(),
        title_scale: 2.5 * ui,
        buttons: column.buttons,
        dim_background: true,
        ui,
    }
}

// --- settings ---------------------------------------------------------------

/// One slider: a labeled range with a step, value shown on the right.
pub struct Slider {
    /// Which setting this drives (`render_distance`, `fov`, ...).
    pub id: &'static str,
    /// Language key for the label.
    label: &'static str,
    /// Which settings tab shows this slider.
    tab: usize,
    min: f32,
    max: f32,
    step: f32,
    pub value: f32,
}

impl Slider {
    fn set_fraction(&mut self, t: f32) {
        let raw = self.min + t.clamp(0.0, 1.0) * (self.max - self.min);
        self.value = ((raw / self.step).round() * self.step).clamp(self.min, self.max);
    }

    fn fraction(&self) -> f32 {
        ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    fn display(&self) -> String {
        if self.min == 0.0 && self.max == 1.0 && self.step == 1.0 {
            // A toggle in slider's clothing.
            if self.value > 0.5 { "On".into() } else { "Off".into() }
        } else if self.step >= 1.0 {
            format!("{}", self.value.round() as i64)
        } else {
            format!("{:.2}", self.value)
        }
    }
}

// Settings layout, logical units.
const ROW_W: f32 = 340.0;
const ROW_H: f32 = 24.0;
const ROW_GAP: f32 = 10.0;
const LABEL_W: f32 = 150.0;
const SLIDER_W: f32 = 130.0;

/// The settings screen: sliders + Back. Pure geometry, testable.
pub const SETTINGS_TABS: [&str; 3] =
    ["settings.tab_game", "settings.tab_graphics", "settings.tab_effects"];

pub struct SettingsScreen {
    pub sliders: Vec<Slider>,
    /// Active tab index into [`SETTINGS_TABS`].
    pub tab: usize,
    /// Return to the pause menu (true) or the title screen (false).
    pub back_to_pause: bool,
}

impl SettingsScreen {
    pub fn from_settings(settings: &crate::settings::Settings, back_to_pause: bool) -> Self {
        use crate::settings::*;
        let slider = |id, label, tab, (min, max): (f32, f32), step, value| Slider {
            id,
            label,
            tab,
            min,
            max,
            step,
            value,
        };
        Self {
            sliders: vec![
                // Game tab.
                slider(
                    "sensitivity",
                    "settings.sensitivity",
                    0,
                    SENSITIVITY_RANGE,
                    0.05,
                    settings.mouse_sensitivity,
                ),
                slider("ui_scale", "settings.ui_scale", 0, UI_SCALE_RANGE, 0.05, settings.ui_scale),
                // Graphics tab.
                slider(
                    "render_distance",
                    "settings.render_distance",
                    1,
                    RENDER_DISTANCE_RANGE,
                    1.0,
                    settings.render_distance as f32,
                ),
                slider(
                    "render_distance_vertical",
                    "settings.render_distance_vertical",
                    1,
                    VERTICAL_RENDER_DISTANCE_RANGE,
                    1.0,
                    settings.render_distance_vertical as f32,
                ),
                slider("fov", "settings.fov", 1, FOV_RANGE, 1.0, settings.fov),
                slider(
                    "resolution_scale",
                    "settings.resolution_scale",
                    1,
                    RESOLUTION_SCALE_RANGE,
                    0.05,
                    settings.resolution_scale,
                ),
                slider("max_fps", "settings.max_fps", 1, MAX_FPS_RANGE, 10.0, settings.max_fps as f32),
                slider(
                    "clouds",
                    "settings.clouds",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.clouds { 1.0 } else { 0.0 },
                ),
                slider(
                    "water_reflections",
                    "settings.water_reflections",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.water_reflections { 1.0 } else { 0.0 },
                ),
                slider("volume", "settings.volume", 0, (0.0, 1.0), 0.05, settings.volume),
                slider(
                    "far_terrain",
                    "settings.far_terrain",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.far_terrain { 1.0 } else { 0.0 },
                ),
                slider(
                    "shadows",
                    "settings.shadows",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.shadows { 1.0 } else { 0.0 },
                ),
                slider(
                    "shadow_style",
                    "settings.shadow_style",
                    2,
                    (0.0, 1.0),
                    1.0,
                    settings.shadow_style as f32,
                ),
                slider(
                    "volumetric_fog",
                    "settings.volumetric_fog",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.volumetric_fog { 1.0 } else { 0.0 },
                ),
                slider(
                    "foliage_sss",
                    "settings.foliage_sss",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.foliage_sss { 1.0 } else { 0.0 },
                ),
                slider(
                    "taa",
                    "settings.taa",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.taa { 1.0 } else { 0.0 },
                ),
                slider(
                    "color_grade",
                    "settings.color_grade",
                    2,
                    (0.0, 1.0),
                    1.0,
                    if settings.color_grade { 1.0 } else { 0.0 },
                ),
            ],
            tab: 0,
            back_to_pause,
        }
    }

    /// Indices of the sliders on the active tab, in display order.
    fn visible(&self) -> Vec<usize> {
        (0..self.sliders.len()).filter(|&i| self.sliders[i].tab == self.tab).collect()
    }

    /// The tab-switch buttons along the top.
    pub fn tab_buttons(&self, registry: &Registry, w: f32, h: f32, ui: f32) -> Vec<Button> {
        let total = SETTINGS_TABS.len() as f32;
        let tab_w = (BUTTON_W - GAP * (total - 1.0)) / total * ui;
        let x0 = (w - BUTTON_W * ui) / 2.0;
        let y = h * 0.32 - (BUTTON_H + GAP * 2.0) * ui;
        SETTINGS_TABS
            .iter()
            .enumerate()
            .map(|(index, key)| Button {
                x: x0 + index as f32 * (tab_w + GAP * ui),
                y,
                w: tab_w,
                h: BUTTON_H * ui,
                label: registry.text(key).to_owned(),
                action: format!("tab:{index}"),
                highlighted: index == self.tab,
            })
            .collect()
    }

    /// Writes the slider values back into a settings struct.
    pub fn apply(&self, settings: &mut crate::settings::Settings) {
        for slider in &self.sliders {
            match slider.id {
                "render_distance" => settings.render_distance = slider.value.round() as i32,
                "render_distance_vertical" => {
                    settings.render_distance_vertical = slider.value.round() as i32
                }
                "fov" => settings.fov = slider.value,
                "sensitivity" => settings.mouse_sensitivity = slider.value,
                "ui_scale" => settings.ui_scale = slider.value,
                "resolution_scale" => settings.resolution_scale = slider.value,
                "max_fps" => settings.max_fps = slider.value.round() as i32,
                "clouds" => settings.clouds = slider.value > 0.5,
                "water_reflections" => settings.water_reflections = slider.value > 0.5,
                "far_terrain" => settings.far_terrain = slider.value > 0.5,
                "shadows" => settings.shadows = slider.value > 0.5,
                "shadow_style" => settings.shadow_style = slider.value.round() as u32,
                "volumetric_fog" => settings.volumetric_fog = slider.value > 0.5,
                "foliage_sss" => settings.foliage_sss = slider.value > 0.5,
                "taa" => settings.taa = slider.value > 0.5,
                "color_grade" => settings.color_grade = slider.value > 0.5,
                "volume" => settings.volume = slider.value,
                _ => {}
            }
        }
        *settings = settings.clamped();
    }

    fn row_origin(&self, w: f32, h: f32, ui: f32) -> (f32, f32) {
        ((w - ROW_W * ui) / 2.0, h * 0.32)
    }

    /// The slider bar rectangle for visible row `row`.
    fn bar_rect(&self, row: usize, w: f32, h: f32, ui: f32) -> (f32, f32, f32, f32) {
        let (x0, y0) = self.row_origin(w, h, ui);
        let y = y0 + row as f32 * (ROW_H + ROW_GAP) * ui;
        (x0 + LABEL_W * ui, y + (ROW_H - 6.0) / 2.0 * ui, SLIDER_W * ui, 6.0 * ui)
    }

    /// Which slider a press at `mouse` grabs (returns the slider's index
    /// into `sliders`, not the row).
    pub fn slider_at(&self, mouse: (f32, f32), w: f32, h: f32, ui: f32) -> Option<usize> {
        for (row, &index) in self.visible().iter().enumerate() {
            let (bx, by, bw, bh) = self.bar_rect(row, w, h, ui);
            let pad = ui * 8.0;
            if mouse.0 >= bx - pad
                && mouse.0 <= bx + bw + pad
                && mouse.1 >= by - (ROW_H / 2.0) * ui
                && mouse.1 <= by + bh + (ROW_H / 2.0) * ui
            {
                return Some(index);
            }
        }
        None
    }

    /// Sets slider `index` (a `sliders` index) from a mouse x position.
    pub fn drag(&mut self, index: usize, mouse_x: f32, w: f32, h: f32, ui: f32) {
        let Some(row) = self.visible().iter().position(|&i| i == index) else {
            return;
        };
        let (bx, _, bw, _) = self.bar_rect(row, w, h, ui);
        let t = (mouse_x - bx) / bw;
        self.sliders[index].set_fraction(t);
    }

    pub fn back_button(&self, registry: &Registry, w: f32, h: f32, ui: f32) -> Button {
        let (_, y0) = self.row_origin(w, h, ui);
        Button {
            x: (w - BUTTON_W * ui) / 2.0,
            y: y0 + (self.visible().len() as f32 + 0.8) * (ROW_H + ROW_GAP) * ui,
            w: BUTTON_W * ui,
            h: BUTTON_H * ui,
            label: registry.text("menu.back").to_owned(),
            action: "settings_back".into(),
            highlighted: false,
        }
    }

    pub fn quads(
        &self,
        registry: &Registry,
        w: f32,
        h: f32,
        ui: f32,
        mouse: (f32, f32),
    ) -> Vec<UiQuad> {
        let mut quads = Vec::new();
        if self.back_to_pause {
            quads.push(UiQuad { x: 0.0, y: 0.0, w, h, color: [0.02, 0.02, 0.04, 0.65] });
        }
        for (row, &index) in self.visible().iter().enumerate() {
            let (bx, by, bw, bh) = self.bar_rect(row, w, h, ui);
            quads.push(UiQuad {
                x: bx,
                y: by,
                w: bw,
                h: bh,
                color: [0.05, 0.05, 0.06, 0.8],
            });
            let t = self.sliders[index].fraction();
            quads.push(UiQuad {
                x: bx + 1.0 * ui,
                y: by + 1.0 * ui,
                w: (bw - 2.0 * ui) * t,
                h: bh - 2.0 * ui,
                color: [0.45, 0.62, 0.45, 0.95],
            });
            // Knob, brighter when grabbed-able.
            let knob_x = bx + (bw - 3.0 * ui) * t;
            let hot = self.slider_at(mouse, w, h, ui) == Some(index);
            quads.push(UiQuad {
                x: knob_x,
                y: by - 2.0 * ui,
                w: 3.0 * ui,
                h: bh + 4.0 * ui,
                color: if hot { [1.0, 1.0, 1.0, 1.0] } else { [0.8, 0.8, 0.85, 0.95] },
            });
        }
        for tab in self.tab_buttons(registry, w, h, ui) {
            let hovered = tab.contains(mouse);
            quads.push(UiQuad {
                x: tab.x,
                y: tab.y,
                w: tab.w,
                h: tab.h,
                color: if tab.highlighted {
                    [0.30, 0.34, 0.44, 0.95]
                } else if hovered {
                    [0.34, 0.36, 0.42, 0.95]
                } else {
                    [0.15, 0.16, 0.20, 0.88]
                },
            });
        }
        let back = self.back_button(registry, w, h, ui);
        let hovered = back.contains(mouse);
        quads.push(UiQuad {
            x: back.x,
            y: back.y,
            w: back.w,
            h: back.h,
            color: if hovered { [0.34, 0.36, 0.42, 0.95] } else { [0.15, 0.16, 0.20, 0.88] },
        });
        quads
    }

    /// The action under the mouse among tab/back buttons, if any.
    pub fn button_hit(&self, registry: &Registry, mouse: (f32, f32), w: f32, h: f32, ui: f32) -> Option<String> {
        for tab in self.tab_buttons(registry, w, h, ui) {
            if tab.contains(mouse) {
                return Some(tab.action);
            }
        }
        let back = self.back_button(registry, w, h, ui);
        back.contains(mouse).then(|| back.action)
    }

    pub fn texts(&self, registry: &Registry, w: f32, h: f32, ui: f32) -> Vec<UiText> {
        let (x0, _) = self.row_origin(w, h, ui);
        let mut texts = vec![centered(
            registry.text("menu.settings").to_owned(),
            w / 2.0,
            h * 0.18,
            3.0 * ui,
        )];
        for (row, &index) in self.visible().iter().enumerate() {
            let slider = &self.sliders[index];
            let (bx, by, bw, bh) = self.bar_rect(row, w, h, ui);
            let text_y = by + bh / 2.0 - GLYPH_H * 1.25 * ui / 2.0;
            texts.push(UiText {
                text: registry.text(slider.label).to_owned(),
                x: x0,
                y: text_y,
                scale: 1.25 * ui,
            });
            // The value, printed to the right of the bar.
            texts.push(UiText {
                text: slider.display(),
                x: bx + bw + 8.0 * ui,
                y: text_y,
                scale: 1.25 * ui,
            });
        }
        for tab in self.tab_buttons(registry, w, h, ui) {
            texts.push(centered(
                tab.label.clone(),
                tab.x + tab.w / 2.0,
                tab.y + tab.h / 2.0,
                1.25 * ui,
            ));
        }
        let back = self.back_button(registry, w, h, ui);
        texts.push(centered(
            back.label.clone(),
            back.x + back.w / 2.0,
            back.y + back.h / 2.0,
            LABEL_SCALE * ui,
        ));
        texts
    }
}

/// Seed from user input: blank → fallback, digits → the number itself,
/// anything else → a splitmix-style hash (string seeds work too).
pub fn parse_seed(input: &str, fallback: u64) -> u64 {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return fallback;
    }
    if let Ok(n) = trimmed.parse::<u64>() {
        return n;
    }
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    for byte in trimmed.bytes() {
        h ^= byte as u64;
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    }
    h ^ (h >> 31)
}

/// Filesystem-safe world name; falls back to "world" if nothing survives.
pub fn sanitize_name(input: &str) -> String {
    let cleaned: String = input
        .trim()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() { "world".to_owned() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::load_default().unwrap()
    }

    #[test]
    fn menus_lay_out_and_hit_test() {
        let registry = registry();
        let def = registry.menu("oc:pause").expect("pause menu defined");
        let view = MenuView::from_def(def, &registry, 1280.0, 720.0, 1.0, true);
        assert_eq!(view.buttons.len(), def.entries.len());
        assert_eq!(view.title, "Paused", "lang key resolved");

        // Clicking the first button center fires its action.
        let first = &view.buttons[0];
        let center = (first.x + first.w / 2.0, first.y + first.h / 2.0);
        assert_eq!(view.hit(center), Some("oc:resume"));
        // Clicking outside hits nothing.
        assert_eq!(view.hit((5.0, 5.0)), None);

        // All buttons are on-screen and non-overlapping.
        for pair in view.buttons.windows(2) {
            assert!(pair[1].y >= pair[0].y + pair[0].h, "buttons overlap");
        }
        for b in &view.buttons {
            assert!(b.x >= 0.0 && b.x + b.w <= 1280.0 && b.y + b.h <= 720.0);
        }
    }

    #[test]
    fn title_menu_resolves_from_data() {
        let registry = registry();
        let def = registry.menu("oc:title").expect("title menu defined");
        let view = MenuView::from_def(def, &registry, 1280.0, 720.0, 1.0, false);
        assert!(view.buttons.iter().any(|b| b.action == "oc:open_worlds"));
        assert!(view.buttons.iter().any(|b| b.action == "oc:quit_app"));
    }

    #[test]
    fn worlds_screen_lists_and_deletes_with_confirmation() {
        let registry = registry();
        let mut screen = WorldsScreen::new(vec!["alpha".into(), "beta".into()]);
        let view = screen.view(&registry, 1280.0, 720.0, 1.0);
        assert!(view.buttons.iter().any(|b| b.action == "world:alpha"));
        assert!(view.buttons.iter().any(|b| b.action == "create_screen"));

        // Delete needs an arming click then a confirming click.
        let delete_x = 1280.0 / 2.0 + BUTTON_W * 0.4;
        assert_eq!(screen.world_click("alpha", delete_x, 1280.0, 1.0), WorldAction::ArmDelete);
        assert_eq!(screen.world_click("alpha", delete_x, 1280.0, 1.0), WorldAction::Delete);
        // A click on the left side just plays.
        assert_eq!(
            screen.world_click("beta", 1280.0 / 2.0 - 200.0, 1280.0, 1.0),
            WorldAction::Play
        );
    }

    #[test]
    fn create_screen_types_and_cycles_modes() {
        let registry = registry();
        let mut screen = CreateScreen::new();
        // The name field starts focused.
        for c in "My World!".chars() {
            screen.type_char(c);
        }
        assert_eq!(screen.name.value, "My World!");
        screen.backspace();
        assert_eq!(screen.name.value, "My World");
        assert_eq!(screen.seed.value, "", "unfocused field ignores typing");
        screen.focus("seed");
        screen.type_char('7');
        assert_eq!(screen.seed.value, "7");

        // Mode selection cycles through the whole registry and wraps.
        assert_eq!(screen.mode_id(&registry), "oc:survival");
        for _ in 0..registry.mode_count() {
            screen.cycle_mode(&registry);
        }
        assert_eq!(screen.mode_id(&registry), "oc:survival");
        screen.cycle_mode(&registry);
        assert_ne!(screen.mode_id(&registry), "oc:survival");

        let view = screen.view(&registry, 1280.0, 720.0, 1.0);
        assert!(view.buttons.iter().any(|b| b.action == "create"));
        assert!(view.buttons.iter().any(|b| b.action == "cycle_create_mode"));

        // Cheats default off and toggle.
        assert!(!screen.cheats);
        assert!(view.buttons.iter().any(|b| b.action == "toggle_create_cheats"));
        screen.cheats = !screen.cheats;
        let view = screen.view(&registry, 1280.0, 720.0, 1.0);
        let row = view.buttons.iter().find(|b| b.action == "toggle_create_cheats").unwrap();
        assert!(row.label.contains("On"), "toggled label: {}", row.label);
    }

    #[test]
    fn modes_view_lists_every_registered_mode() {
        let registry = registry();
        let view = modes_view(&registry, 1, true, 1280.0, 720.0, 1.0);
        // One row per mode plus Back.
        assert_eq!(view.buttons.len(), registry.mode_count() + 1);
        assert!(view.buttons.iter().any(|b| b.action == "mode:0"));
        // The current mode is marked.
        let current = view.buttons.iter().find(|b| b.action == "mode:1").unwrap();
        assert!(current.label.contains("[x]"), "current mode marked: {}", current.label);

        // Without cheats no mode is selectable, only Back.
        let locked = modes_view(&registry, 1, false, 1280.0, 720.0, 1.0);
        assert!(locked.buttons.iter().all(|b| !b.action.starts_with("mode:")));
        assert!(locked.buttons.iter().any(|b| b.action == "back_pause"));
    }

    #[test]
    fn seeds_parse_numeric_string_and_blank() {
        assert_eq!(parse_seed("", 42), 42);
        assert_eq!(parse_seed("  ", 42), 42);
        assert_eq!(parse_seed("12345", 0), 12345);
        let a = parse_seed("glacier", 0);
        let b = parse_seed("glacier", 99);
        assert_eq!(a, b, "string seeds ignore the fallback");
        assert_ne!(a, parse_seed("glacie", 0), "different strings differ");
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("My Cool World"), "My-Cool-World");
        assert_eq!(sanitize_name("  ../evil  "), "evil");
        assert_eq!(sanitize_name("!!!"), "world");
    }

    #[test]
    fn settings_sliders_drag_and_apply() {
        use crate::settings::Settings;
        let registry = registry();
        let mut screen = SettingsScreen::from_settings(&Settings::default(), false);
        assert_eq!(screen.sliders.len(), 17);
        // The clouds toggle reads On/Off and round-trips.
        let clouds = screen.sliders.iter().position(|s| s.id == "clouds").unwrap();
        assert_eq!(screen.sliders[clouds].display(), "On");
        // The sun-shadows toggle defaults On.
        let shadows = screen.sliders.iter().position(|s| s.id == "shadows").unwrap();
        assert_eq!(screen.sliders[shadows].display(), "On");

        let (w, h, ui) = (1280.0, 720.0, 1.0);
        // FOV lives on the Graphics tab (row 2 there, sliders[4]: render distance,
        // vertical render distance, then FOV).
        screen.tab = 1;
        let fov = 4;
        let (bx, by, bw, bh) = screen.bar_rect(2, w, h, ui);
        let grabbed = screen.slider_at((bx + bw / 2.0, by + bh / 2.0), w, h, ui);
        assert_eq!(grabbed, Some(fov));
        screen.drag(fov, bx + bw + 50.0, w, h, ui);
        assert_eq!(screen.sliders[fov].value, 110.0, "clamped to max");
        screen.drag(fov, bx - 50.0, w, h, ui);
        assert_eq!(screen.sliders[fov].value, 50.0, "clamped to min");
        screen.drag(fov, bx + bw / 2.0, w, h, ui);
        assert_eq!(screen.sliders[fov].value, 80.0, "midpoint, step-rounded");

        // Sliders on the inactive tab can't be grabbed or dragged.
        screen.tab = 0;
        assert_ne!(screen.slider_at((bx + bw / 2.0, by + bh / 2.0), w, h, ui), Some(fov));
        let before = screen.sliders[fov].value;
        screen.drag(fov, bx, w, h, ui);
        assert_eq!(screen.sliders[fov].value, before, "hidden slider ignores drags");
        screen.tab = 1;

        // Step rounding on the UI scale slider (0.05 steps, Game tab).
        screen.tab = 0;
        let (bx, _, bw, _) = screen.bar_rect(1, w, h, ui);
        screen.drag(1, bx + bw * 0.4321, w, h, ui);
        let v = screen.sliders[1].value;
        assert!((v / 0.05 - (v / 0.05).round()).abs() < 1e-4, "stepped: {v}");

        // Values write back, clamped.
        let mut settings = Settings::default();
        screen.apply(&mut settings);
        assert_eq!(settings.fov, 80.0);
        assert_eq!(settings.ui_scale, v);

        // The value is rendered to the right of its bar (Graphics tab).
        screen.tab = 1;
        let texts = screen.texts(&registry, w, h, ui);
        assert!(texts.iter().any(|t| t.text == "80" && t.x > bx + bw));
        // Tab buttons exist and switch.
        assert!(screen.button_hit(&registry, (5.0, 5.0), w, h, ui).is_none());
        let tabs = screen.tab_buttons(&registry, w, h, ui);
        assert_eq!(tabs.len(), 3);
        assert!(tabs[1].highlighted, "active tab marked");
        // Geometry scales linearly with ui.
        let (bx2, _, bw2, _) = screen.bar_rect(1, w, h, 2.0);
        assert!((bw2 - bw * 2.0).abs() < 1e-4);
        let _ = bx2;
    }
}
