//! The recipe book (C key): lists every recipe with availability; number
//! keys craft. A drag-and-drop crafting grid arrives with the inventory
//! screen; until then this quick-craft panel is the §7 recipe pipeline
//! end-to-end.

use oc_assets::Registry;
use oc_renderer::{UiQuad, UiText};

pub struct CraftLine {
    pub recipe: usize,
    pub label: String,
    pub craftable: bool,
}

/// One line per recipe: "[1] 1 log -> 4 planks" plus availability.
pub fn lines(registry: &Registry, count_of: impl Fn(oc_assets::ItemId) -> u32) -> Vec<CraftLine> {
    (0..registry.recipe_count())
        .filter_map(|index| {
            let view = registry.recipe_view(index)?;
            let ingredients = view
                .ingredients
                .iter()
                .map(|(item, n)| format!("{} {}", n, registry.item(*item).name))
                .collect::<Vec<_>>()
                .join(" + ");
            let craftable = registry.craftable(index, &count_of);
            Some(CraftLine {
                recipe: index,
                label: format!(
                    "[{}] {} - {} {}{}",
                    index + 1,
                    ingredients,
                    view.result.1,
                    registry.item(view.result.0).name,
                    if craftable { "" } else { "  (missing)" }
                ),
                craftable,
            })
        })
        .collect()
}

/// Panel geometry + text for the open recipe book.
pub fn panel(lines: &[CraftLine], width: f32) -> (Vec<UiQuad>, Vec<UiText>) {
    const SCALE: f32 = 2.0;
    const LINE_H: f32 = 8.0 * SCALE + 6.0;
    const PAD: f32 = 16.0;
    let panel_w = 560.0;
    let panel_h = (lines.len() as f32 + 1.5) * LINE_H + PAD * 2.0;
    let x = (width - panel_w) / 2.0;
    let y = 120.0;

    let quads = vec![UiQuad {
        x,
        y,
        w: panel_w,
        h: panel_h,
        color: [0.05, 0.05, 0.08, 0.85],
    }];
    let mut texts = vec![UiText {
        text: "crafting  [c] close".into(),
        x: x + PAD,
        y: y + PAD,
        scale: SCALE,
    }];
    for (row, line) in lines.iter().enumerate() {
        texts.push(UiText {
            text: line.label.clone(),
            x: x + PAD,
            y: y + PAD + (row as f32 + 1.5) * LINE_H,
            scale: SCALE,
        });
    }
    (quads, texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_show_availability() {
        let registry = Registry::load_default().unwrap();
        let log = registry.find("oc:log").unwrap();
        // One log: the planks recipe is craftable, everything else missing.
        let lines = lines(&registry, |item| if item == log { 1 } else { 0 });
        assert_eq!(lines.len(), registry.recipe_count());
        let planks_line = lines.iter().find(|l| l.label.contains("Planks")).unwrap();
        assert!(planks_line.craftable);
        assert!(!planks_line.label.contains("missing"));
        let stick_line = lines.iter().find(|l| l.label.contains("Stick")).unwrap();
        assert!(!stick_line.craftable);
        assert!(stick_line.label.contains("missing"));
    }

    #[test]
    fn panel_fits_all_lines() {
        let registry = Registry::load_default().unwrap();
        let lines = lines(&registry, |_| 0);
        let (quads, texts) = panel(&lines, 2560.0);
        assert_eq!(quads.len(), 1);
        assert_eq!(texts.len(), lines.len() + 1, "title + one line each");
        let bg = quads[0];
        for t in &texts {
            assert!(t.x >= bg.x && t.y >= bg.y && t.y < bg.y + bg.h, "text inside panel");
        }
    }
}
