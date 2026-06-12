//! Data-driven content loading (ARCHITECTURE.md §7): items and recipes from
//! RON files. The base game is built the way mods will be built — the same
//! formats load from `./mods/` in phase 5.
//!
//! The repo's `data/` files are embedded as the built-in defaults, so the
//! game runs from any working directory; `load_from_dir` reads overrides.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use oc_world::BlockId;
use serde::Deserialize;

const DEFAULT_ITEMS: &str = include_str!("../../../data/items.ron");
const DEFAULT_RECIPES: &str = include_str!("../../../data/recipes.ron");
const DEFAULT_GAMEMODES: &str = include_str!("../../../data/gamemodes.ron");
const DEFAULT_CREATURES: &str = include_str!("../../../data/creatures.ron");
const DEFAULT_MENUS: &str = include_str!("../../../data/menus.ron");
const DEFAULT_LANG_EN: &str = include_str!("../../../data/lang/en.ron");

/// Runtime item handle (index into the registry). String ids (`oc:stone`)
/// are the stable identity; numeric ids are per-load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u16);

#[derive(Debug, Deserialize)]
pub struct ItemDef {
    /// Namespaced stable id, e.g. `oc:planks`.
    pub id: String,
    pub name: String,
    /// Block-state id this item places, if it's a block item.
    pub block: Option<u16>,
    /// Hunger points (0..=10 scale) restored when eaten; 0 = not food.
    #[serde(default)]
    pub food: u32,
}

#[derive(Debug, Deserialize)]
enum RecipeDef {
    Shaped {
        /// Rows of key characters; space = empty. Up to 3×3.
        pattern: Vec<String>,
        keys: HashMap<char, String>,
        result: (String, u8),
    },
    Shapeless {
        ingredients: Vec<String>,
        result: (String, u8),
    },
}

/// A compiled recipe: normalized for O(1)-ish matching.
enum Recipe {
    /// Normalized pattern (top-left trimmed): dimensions + cells.
    Shaped {
        width: usize,
        height: usize,
        cells: Vec<Option<ItemId>>,
        result: (ItemId, u8),
    },
    /// Sorted ingredient list.
    Shapeless {
        ingredients: Vec<ItemId>,
        result: (ItemId, u8),
    },
}

/// Runtime game-mode handle (index into the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeId(pub u16);

/// A game mode: a named bundle of engine capability flags. Mods add modes
/// by shipping more of these (§7.6); the flags themselves are engine
/// vocabulary.
#[derive(Debug, Clone, Deserialize)]
pub struct GameModeDef {
    /// Namespaced stable id, e.g. `oc:survival`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub can_edit_blocks: bool,
    #[serde(default)]
    pub uses_inventory: bool,
    #[serde(default)]
    pub has_stats: bool,
    #[serde(default)]
    pub can_fly: bool,
    #[serde(default)]
    pub noclip: bool,
}

/// Runtime creature-kind handle (index into the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CreatureKindId(pub u16);

/// A creature kind, data-driven like blocks/items (§5.6).
#[derive(Debug, Clone, Deserialize)]
pub struct CreatureDef {
    /// Namespaced stable id, e.g. `oc:critter`.
    pub id: String,
    pub name: String,
    /// Collision box (width, height) in blocks.
    pub size: (f32, f32),
    /// Primary body tint (sRGB).
    pub color: (u8, u8, u8),
    /// Secondary tint — face/legs for quadrupeds (defaults to `color`).
    #[serde(default)]
    pub accent: Option<(u8, u8, u8)>,
    /// Client model: "box" (single cuboid) or "quadruped" (body, head,
    /// four legs — cows, sheep).
    #[serde(default = "default_creature_model")]
    pub model: String,
    /// Walk speed, blocks per second.
    pub speed: f32,
}

fn default_creature_model() -> String {
    "box".into()
}

/// A recipe's shopping list, independent of arrangement.
#[derive(Debug, Clone)]
pub struct RecipeView {
    pub index: usize,
    pub result: (ItemId, u8),
    pub ingredients: Vec<(ItemId, u32)>,
}

/// One button in a data-driven menu. Mods (phase 5) merge entries by id,
/// so they can add buttons or replace vanilla ones.
#[derive(Debug, Clone, Deserialize)]
pub struct MenuEntryDef {
    /// Namespaced stable id, e.g. `oc:resume`.
    pub id: String,
    /// Language key (resolved through [`Registry::text`]), never literal text.
    pub label: String,
    /// Named action the client interprets, e.g. `oc:quit_world`.
    pub action: String,
}

/// A data-driven menu screen (`data/menus.ron`).
#[derive(Debug, Clone, Deserialize)]
pub struct MenuDef {
    pub id: String,
    /// Language key for the heading.
    pub title: String,
    pub entries: Vec<MenuEntryDef>,
}

pub struct Registry {
    items: Vec<ItemDef>,
    by_string_id: HashMap<String, ItemId>,
    by_block: HashMap<u16, ItemId>,
    recipes: Vec<Recipe>,
    modes: Vec<GameModeDef>,
    mode_by_id: HashMap<String, ModeId>,
    creatures: Vec<CreatureDef>,
    creature_by_id: HashMap<String, CreatureKindId>,
    menus: Vec<MenuDef>,
    /// UI strings by language key (the active language's table).
    texts: HashMap<String, String>,
}

impl Registry {
    /// Loads the embedded base-game content.
    pub fn load_default() -> Result<Self> {
        let mut registry =
            Self::parse(DEFAULT_ITEMS, DEFAULT_RECIPES, DEFAULT_GAMEMODES, DEFAULT_CREATURES)?;
        registry.load_menus(DEFAULT_MENUS)?;
        registry.load_lang(DEFAULT_LANG_EN)?;
        Ok(registry)
    }

    /// Parses `menus.ron`, replacing the menu set.
    pub fn load_menus(&mut self, menus_ron: &str) -> Result<()> {
        self.menus = ron::from_str(menus_ron).context("parsing menus")?;
        Ok(())
    }

    /// Parses a language file, replacing the active string table.
    pub fn load_lang(&mut self, lang_ron: &str) -> Result<()> {
        self.texts = ron::from_str(lang_ron).context("parsing lang")?;
        Ok(())
    }

    /// The menu with this id, if defined.
    pub fn menu(&self, id: &str) -> Option<&MenuDef> {
        self.menus.iter().find(|menu| menu.id == id)
    }

    /// Resolves a language key; unknown keys show as themselves, so a
    /// missing translation is visible but never a crash.
    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        self.texts.get(key).map_or(key, String::as_str)
    }

    /// Loads the content files from a directory.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .with_context(|| format!("reading {}", dir.join(name).display()))
        };
        Self::parse(
            &read("items.ron")?,
            &read("recipes.ron")?,
            &read("gamemodes.ron")?,
            &read("creatures.ron")?,
        )
    }

    fn parse(
        items_ron: &str,
        recipes_ron: &str,
        gamemodes_ron: &str,
        creatures_ron: &str,
    ) -> Result<Self> {
        let items: Vec<ItemDef> = ron::from_str(items_ron).context("parsing items")?;
        let mut by_string_id = HashMap::new();
        let mut by_block = HashMap::new();
        for (index, item) in items.iter().enumerate() {
            if by_string_id.insert(item.id.clone(), ItemId(index as u16)).is_some() {
                bail!("duplicate item id {:?}", item.id);
            }
            if let Some(block) = item.block {
                by_block.insert(block, ItemId(index as u16));
            }
        }

        let defs: Vec<RecipeDef> = ron::from_str(recipes_ron).context("parsing recipes")?;
        let lookup = |id: &str| -> Result<ItemId> {
            by_string_id
                .get(id)
                .copied()
                .with_context(|| format!("recipe references unknown item {id:?}"))
        };
        let mut recipes = Vec::with_capacity(defs.len());
        for def in defs {
            recipes.push(match def {
                RecipeDef::Shaped { pattern, keys, result } => {
                    if pattern.is_empty() || pattern.len() > 3 {
                        bail!("shaped pattern must be 1-3 rows");
                    }
                    let width = pattern.iter().map(|r| r.chars().count()).max().unwrap();
                    if width == 0 || width > 3 {
                        bail!("shaped pattern must be 1-3 columns");
                    }
                    let mut cells = Vec::with_capacity(width * pattern.len());
                    for row in &pattern {
                        let chars: Vec<char> = row.chars().collect();
                        for x in 0..width {
                            match chars.get(x).copied().unwrap_or(' ') {
                                ' ' => cells.push(None),
                                key => {
                                    let id = keys.get(&key).with_context(|| {
                                        format!("pattern key {key:?} missing from keys")
                                    })?;
                                    cells.push(Some(lookup(id)?));
                                }
                            }
                        }
                    }
                    Recipe::Shaped {
                        width,
                        height: pattern.len(),
                        cells,
                        result: (lookup(&result.0)?, result.1),
                    }
                }
                RecipeDef::Shapeless { ingredients, result } => {
                    if ingredients.is_empty() || ingredients.len() > 9 {
                        bail!("shapeless recipes take 1-9 ingredients");
                    }
                    let mut ids = ingredients
                        .iter()
                        .map(|id| lookup(id))
                        .collect::<Result<Vec<_>>>()?;
                    ids.sort();
                    Recipe::Shapeless { ingredients: ids, result: (lookup(&result.0)?, result.1) }
                }
            });
        }

        let modes: Vec<GameModeDef> =
            ron::from_str(gamemodes_ron).context("parsing game modes")?;
        if modes.is_empty() {
            bail!("at least one game mode is required");
        }
        let mut mode_by_id = HashMap::new();
        for (index, mode) in modes.iter().enumerate() {
            if mode_by_id.insert(mode.id.clone(), ModeId(index as u16)).is_some() {
                bail!("duplicate game mode id {:?}", mode.id);
            }
        }

        let creatures: Vec<CreatureDef> =
            ron::from_str(creatures_ron).context("parsing creatures")?;
        let mut creature_by_id = HashMap::new();
        for (index, def) in creatures.iter().enumerate() {
            if creature_by_id
                .insert(def.id.clone(), CreatureKindId(index as u16))
                .is_some()
            {
                bail!("duplicate creature id {:?}", def.id);
            }
        }

        Ok(Self {
            items,
            by_string_id,
            by_block,
            recipes,
            modes,
            mode_by_id,
            creatures,
            creature_by_id,
            menus: Vec::new(),
            texts: HashMap::new(),
        })
    }

    pub fn creature(&self, id: CreatureKindId) -> &CreatureDef {
        &self.creatures[(id.0 as usize).min(self.creatures.len().saturating_sub(1))]
    }

    pub fn creature_count(&self) -> usize {
        self.creatures.len()
    }

    pub fn find_creature(&self, string_id: &str) -> Option<CreatureKindId> {
        self.creature_by_id.get(string_id).copied()
    }

    pub fn mode(&self, id: ModeId) -> &GameModeDef {
        &self.modes[(id.0 as usize).min(self.modes.len() - 1)]
    }

    pub fn mode_count(&self) -> usize {
        self.modes.len()
    }

    pub fn find_mode(&self, string_id: &str) -> Option<ModeId> {
        self.mode_by_id.get(string_id).copied()
    }

    /// The world default (survival if present, else the first defined).
    pub fn default_mode(&self) -> ModeId {
        self.find_mode("oc:survival").unwrap_or(ModeId(0))
    }

    /// The next mode in registry order, wrapping (for the cycle key).
    pub fn next_mode(&self, id: ModeId) -> ModeId {
        ModeId(((id.0 as usize + 1) % self.modes.len()) as u16)
    }

    pub fn item(&self, id: ItemId) -> &ItemDef {
        &self.items[id.0 as usize]
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn find(&self, string_id: &str) -> Option<ItemId> {
        self.by_string_id.get(string_id).copied()
    }

    /// The item that drops when this block is broken.
    pub fn item_for_block(&self, block: BlockId) -> Option<ItemId> {
        self.by_block.get(&block.0).copied()
    }

    /// The block an item places, if any.
    pub fn block_for_item(&self, id: ItemId) -> Option<BlockId> {
        self.item(id).block.map(BlockId)
    }

    pub fn recipe_count(&self) -> usize {
        self.recipes.len()
    }

    /// Result and aggregated ingredient counts of a recipe, for recipe-book
    /// UIs and server-side validation.
    pub fn recipe_view(&self, index: usize) -> Option<RecipeView> {
        let recipe = self.recipes.get(index)?;
        let (result, raw): ((ItemId, u8), Vec<ItemId>) = match recipe {
            Recipe::Shaped { cells, result, .. } => {
                (*result, cells.iter().flatten().copied().collect())
            }
            Recipe::Shapeless { ingredients, result } => (*result, ingredients.clone()),
        };
        let mut ingredients: Vec<(ItemId, u32)> = Vec::new();
        for item in raw {
            match ingredients.iter_mut().find(|(i, _)| *i == item) {
                Some((_, n)) => *n += 1,
                None => ingredients.push((item, 1)),
            }
        }
        ingredients.sort();
        Some(RecipeView { index, result, ingredients })
    }

    /// Whether `have` (item -> count) covers a recipe's ingredients.
    pub fn craftable(&self, index: usize, have: impl Fn(ItemId) -> u32) -> bool {
        self.recipe_view(index)
            .is_some_and(|view| view.ingredients.iter().all(|(item, n)| have(*item) >= *n))
    }

    /// Matches a 3×3 crafting grid (row-major, None = empty slot) against
    /// every recipe. Shaped patterns match at any offset; shapeless recipes
    /// ignore arrangement.
    pub fn match_recipe(&self, grid: &[Option<ItemId>; 9]) -> Option<(ItemId, u8)> {
        // Normalize: bounding box of the filled cells.
        let filled: Vec<usize> = (0..9).filter(|&i| grid[i].is_some()).collect();
        if filled.is_empty() {
            return None;
        }
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (3usize, 3usize, 0usize, 0usize);
        for &i in &filled {
            let (x, y) = (i % 3, i / 3);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let (w, h) = (max_x - min_x + 1, max_y - min_y + 1);

        let mut multiset: Vec<ItemId> = filled.iter().map(|&i| grid[i].unwrap()).collect();
        multiset.sort();

        for recipe in &self.recipes {
            match recipe {
                Recipe::Shaped { width, height, cells, result } => {
                    if *width != w || *height != h {
                        continue;
                    }
                    let matches = (0..h * w).all(|i| {
                        let (x, y) = (i % w, i / w);
                        grid[(min_y + y) * 3 + (min_x + x)] == cells[i]
                    });
                    if matches {
                        return Some(*result);
                    }
                }
                Recipe::Shapeless { ingredients, result } => {
                    if *ingredients == multiset {
                        return Some(*result);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::load_default().expect("default content parses")
    }

    fn grid(slots: &[(usize, &str)], reg: &Registry) -> [Option<ItemId>; 9] {
        let mut grid = [None; 9];
        for (slot, id) in slots {
            grid[*slot] = Some(reg.find(id).unwrap_or_else(|| panic!("unknown {id}")));
        }
        grid
    }

    #[test]
    fn default_content_loads() {
        let reg = registry();
        assert!(reg.item_count() >= 10);
        let planks = reg.find("oc:planks").expect("planks exist");
        assert_eq!(reg.item(planks).name, "Planks");
        // Block items map both ways.
        let stone = reg.find("oc:stone").unwrap();
        let block = reg.block_for_item(stone).unwrap();
        assert_eq!(reg.item_for_block(block), Some(stone));
        // Sticks place nothing.
        assert_eq!(reg.block_for_item(reg.find("oc:stick").unwrap()), None);
    }

    #[test]
    fn shapeless_matches_any_slot() {
        let reg = registry();
        for slot in [0, 4, 8] {
            let result = reg.match_recipe(&grid(&[(slot, "oc:log")], &reg));
            let (item, count) = result.expect("log -> planks");
            assert_eq!(reg.item(item).id, "oc:planks");
            assert_eq!(count, 4);
        }
    }

    #[test]
    fn shaped_matches_at_any_offset_but_not_rearranged() {
        let reg = registry();
        // Two planks stacked vertically -> sticks, anywhere in the grid.
        for (top, bottom) in [(0, 3), (1, 4), (4, 7), (5, 8)] {
            let result =
                reg.match_recipe(&grid(&[(top, "oc:planks"), (bottom, "oc:planks")], &reg));
            let (item, count) = result.expect("planks column -> sticks");
            assert_eq!(reg.item(item).id, "oc:stick");
            assert_eq!(count, 4);
        }
        // Horizontal arrangement is a different shape: no match.
        assert!(
            reg.match_recipe(&grid(&[(3, "oc:planks"), (4, "oc:planks")], &reg))
                .is_none()
        );
    }

    #[test]
    fn two_by_two_planks_make_a_lamp() {
        let reg = registry();
        let result = reg.match_recipe(&grid(
            &[(4, "oc:planks"), (5, "oc:planks"), (7, "oc:planks"), (8, "oc:planks")],
            &reg,
        ));
        assert_eq!(reg.item(result.unwrap().0).id, "oc:lamp");
    }

    #[test]
    fn wrong_or_empty_grids_match_nothing() {
        let reg = registry();
        assert!(reg.match_recipe(&[None; 9]).is_none());
        assert!(reg.match_recipe(&grid(&[(0, "oc:stone")], &reg)).is_none());
        // Right shape, wrong items.
        assert!(
            reg.match_recipe(&grid(&[(0, "oc:stone"), (3, "oc:stone")], &reg))
                .is_none()
        );
    }

    #[test]
    fn recipe_views_aggregate_ingredients() {
        let reg = registry();
        let views: Vec<RecipeView> =
            (0..reg.recipe_count()).map(|i| reg.recipe_view(i).unwrap()).collect();
        // The 2x2 planks -> lamp recipe aggregates to 4 planks.
        let lamp = reg.find("oc:lamp").unwrap();
        let planks = reg.find("oc:planks").unwrap();
        let view = views.iter().find(|v| v.result.0 == lamp).expect("lamp recipe");
        assert_eq!(view.ingredients, vec![(planks, 4)]);

        // Craftable checks against counts.
        assert!(reg.craftable(view.index, |i| if i == planks { 4 } else { 0 }));
        assert!(!reg.craftable(view.index, |i| if i == planks { 3 } else { 0 }));
        assert!(reg.recipe_view(999).is_none());
    }

    #[test]
    fn duplicate_item_ids_are_rejected() {
        let items = r#"[(id: "oc:x", name: "X", block: None), (id: "oc:x", name: "Y", block: None)]"#;
        assert!(Registry::parse(items, "[]", MODES, "[]").is_err());
    }

    #[test]
    fn recipes_with_unknown_items_are_rejected() {
        let items = r#"[(id: "oc:x", name: "X", block: None)]"#;
        let recipes = r#"[Shapeless(ingredients: ["oc:missing"], result: ("oc:x", 1))]"#;
        assert!(Registry::parse(items, recipes, MODES, "[]").is_err());
    }

    const MODES: &str = r#"[(id: "oc:survival", name: "Survival", can_edit_blocks: true, uses_inventory: true, has_stats: true)]"#;

    #[test]
    fn standard_modes_cover_the_classic_set() {
        let reg = registry();
        assert_eq!(reg.mode_count(), 4);
        let survival = reg.mode(reg.find_mode("oc:survival").unwrap());
        assert!(survival.can_edit_blocks && survival.uses_inventory && survival.has_stats);
        assert!(!survival.can_fly && !survival.noclip);
        let creative = reg.mode(reg.find_mode("oc:creative").unwrap());
        assert!(creative.can_edit_blocks && creative.can_fly && !creative.uses_inventory);
        let spectator = reg.mode(reg.find_mode("oc:spectator").unwrap());
        assert!(spectator.noclip && spectator.can_fly && !spectator.can_edit_blocks);
        assert_eq!(reg.default_mode(), reg.find_mode("oc:survival").unwrap());
        // The cycle wraps over every registered mode.
        let mut m = reg.default_mode();
        for _ in 0..reg.mode_count() {
            m = reg.next_mode(m);
        }
        assert_eq!(m, reg.default_mode());
    }

    #[test]
    fn creatures_load_with_stable_ids() {
        let reg = registry();
        assert!(reg.creature_count() >= 2);
        let cow = reg.creature(reg.find_creature("oc:cow").unwrap());
        assert_eq!(cow.name, "Cow");
        assert_eq!(cow.model, "quadruped");
        assert!(cow.size.0 > 0.0 && cow.speed > 0.0);
        assert!(reg.find_creature("oc:missing").is_none());
    }

    #[test]
    fn mods_can_define_new_modes() {
        // A mod-style mode: free building but grounded and mortal.
        let modes = r#"[
            (id: "oc:survival", name: "Survival", can_edit_blocks: true, uses_inventory: true, has_stats: true),
            (id: "mymod:builder", name: "Builder", can_edit_blocks: true, has_stats: true),
        ]"#;
        let items = r#"[(id: "oc:x", name: "X", block: None)]"#;
        let reg = Registry::parse(items, "[]", modes, "[]").unwrap();
        assert_eq!(reg.mode_count(), 2);
        let builder = reg.mode(reg.find_mode("mymod:builder").unwrap());
        assert!(builder.can_edit_blocks && builder.has_stats);
        assert!(!builder.uses_inventory && !builder.can_fly, "defaults are off");

        // Duplicates and empty mode lists are load errors.
        assert!(Registry::parse(items, "[]", "[]", "[]").is_err());
        let dup = r#"[(id: "oc:a", name: "A"), (id: "oc:a", name: "B")]"#;
        assert!(Registry::parse(items, "[]", dup, "[]").is_err());
    }
}
