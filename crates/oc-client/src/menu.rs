//! Menu screens: layout, hit-testing and rendering for the data-driven
//! title/pause menus (`data/menus.ron`) and the dynamic world-selection
//! screen. Pure layout math — testable without a window.

use oc_assets::{MenuDef, Registry};
use oc_renderer::{UiQuad, UiText};

pub const BUTTON_W: f32 = 520.0;
pub const BUTTON_H: f32 = 56.0;
const GAP: f32 = 14.0;
const LABEL_SCALE: f32 = 3.0;
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
    fn contains(&self, mouse: (f32, f32)) -> bool {
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
}

impl MenuView {
    /// Lays out a data-driven menu (menus.ron) for a `w`×`h` framebuffer.
    pub fn from_def(def: &MenuDef, registry: &Registry, w: f32, h: f32, dim: bool) -> Self {
        let x = (w - BUTTON_W) / 2.0;
        let mut y = h * 0.38;
        let buttons = def
            .entries
            .iter()
            .map(|entry| {
                let button = Button {
                    x,
                    y,
                    w: BUTTON_W,
                    h: BUTTON_H,
                    label: registry.text(&entry.label).to_owned(),
                    action: entry.action.clone(),
                    highlighted: false,
                };
                y += BUTTON_H + GAP;
                button
            })
            .collect();
        Self {
            title: registry.text(&def.title).to_owned(),
            title_scale: if dim { 5.0 } else { 9.0 },
            buttons,
            dim_background: dim,
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
                LABEL_SCALE,
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
}

impl Column {
    fn new(w: f32, h: f32, top: f32) -> Self {
        Self { buttons: Vec::new(), x: (w - BUTTON_W) / 2.0, y: h * top }
    }

    fn row(&mut self, label: String, action: String, highlighted: bool) {
        self.buttons.push(Button {
            x: self.x,
            y: self.y,
            w: BUTTON_W,
            h: BUTTON_H,
            label,
            action,
            highlighted,
        });
        self.y += BUTTON_H + GAP;
    }

    fn space(&mut self) {
        self.y += GAP * 2.0;
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
    pub fn view(&self, registry: &Registry, w: f32, h: f32) -> MenuView {
        let mut column = Column::new(w, h, 0.30);
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
            title_scale: 6.0,
            buttons: column.buttons,
            dim_background: false,
        }
    }

    /// Splits a world-row click into play vs delete: clicking the right
    /// fifth of the row (the delete tag) arms/fires deletion.
    pub fn world_click(&mut self, world: &str, mouse_x: f32, w: f32) -> WorldAction {
        let delete_zone = mouse_x > (w - BUTTON_W) / 2.0 + BUTTON_W * 0.72;
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

    pub fn view(&self, registry: &Registry, w: f32, h: f32) -> MenuView {
        let mut column = Column::new(w, h, 0.32);
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
            title_scale: 6.0,
            buttons: column.buttons,
            dim_background: false,
        }
    }
}

/// The in-game mode picker (reached from the pause menu): one row per
/// registered game mode, so modded modes appear automatically. Without
/// cheat permission the list is replaced by an explanation.
pub fn modes_view(registry: &Registry, current: u16, cheats: bool, w: f32, h: f32) -> MenuView {
    let mut column = Column::new(w, h, 0.34);
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
        title_scale: 5.0,
        buttons: column.buttons,
        dim_background: true,
    }
}

/// Seed from user input: blank → fallback, digits → the number itself,
/// anything else → a splitmix-style hash (MC-style string seeds).
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
        let view = MenuView::from_def(def, &registry, 1280.0, 720.0, true);
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
        let view = MenuView::from_def(def, &registry, 1280.0, 720.0, false);
        assert!(view.buttons.iter().any(|b| b.action == "oc:open_worlds"));
        assert!(view.buttons.iter().any(|b| b.action == "oc:quit_app"));
    }

    #[test]
    fn worlds_screen_lists_and_deletes_with_confirmation() {
        let registry = registry();
        let mut screen = WorldsScreen::new(vec!["alpha".into(), "beta".into()]);
        let view = screen.view(&registry, 1280.0, 720.0);
        assert!(view.buttons.iter().any(|b| b.action == "world:alpha"));
        assert!(view.buttons.iter().any(|b| b.action == "create_screen"));

        // Delete needs an arming click then a confirming click.
        let delete_x = 1280.0 / 2.0 + BUTTON_W * 0.4;
        assert_eq!(screen.world_click("alpha", delete_x, 1280.0), WorldAction::ArmDelete);
        assert_eq!(screen.world_click("alpha", delete_x, 1280.0), WorldAction::Delete);
        // A click on the left side just plays.
        assert_eq!(screen.world_click("beta", 1280.0 / 2.0 - 200.0, 1280.0), WorldAction::Play);
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

        let view = screen.view(&registry, 1280.0, 720.0);
        assert!(view.buttons.iter().any(|b| b.action == "create"));
        assert!(view.buttons.iter().any(|b| b.action == "cycle_create_mode"));

        // Cheats default off and toggle.
        assert!(!screen.cheats);
        assert!(view.buttons.iter().any(|b| b.action == "toggle_create_cheats"));
        screen.cheats = !screen.cheats;
        let view = screen.view(&registry, 1280.0, 720.0);
        let row = view.buttons.iter().find(|b| b.action == "toggle_create_cheats").unwrap();
        assert!(row.label.contains("On"), "toggled label: {}", row.label);
    }

    #[test]
    fn modes_view_lists_every_registered_mode() {
        let registry = registry();
        let view = modes_view(&registry, 1, true, 1280.0, 720.0);
        // One row per mode plus Back.
        assert_eq!(view.buttons.len(), registry.mode_count() + 1);
        assert!(view.buttons.iter().any(|b| b.action == "mode:0"));
        // The current mode is marked.
        let current = view.buttons.iter().find(|b| b.action == "mode:1").unwrap();
        assert!(current.label.contains("[x]"), "current mode marked: {}", current.label);

        // Without cheats no mode is selectable, only Back.
        let locked = modes_view(&registry, 1, false, 1280.0, 720.0);
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
}
