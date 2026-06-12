//! Hand-authored 5×7 bitmap font for the debug HUD, until the asset
//! pipeline (§7.5) brings real fonts. Text is uppercased before lookup.

/// Glyph cell size in the atlas (5×7 pixels + 1px spacing).
pub const CELL_W: u32 = 6;
pub const CELL_H: u32 = 8;

/// Characters in atlas order. Anything else renders as space.
pub const CHARSET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.:-+/,%[] ";

/// One quad's worth of layout: pixel rect + atlas texel rect.
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub fn atlas_width() -> u32 {
    CHARSET.len() as u32 * CELL_W
}

pub fn atlas_height() -> u32 {
    CELL_H
}

/// R8 pixels for the font atlas (255 = glyph, 0 = background).
pub fn build_atlas() -> Vec<u8> {
    let width = atlas_width() as usize;
    let mut pixels = vec![0u8; width * CELL_H as usize];
    for (index, ch) in CHARSET.chars().enumerate() {
        let rows = glyph(ch);
        for (gy, row) in rows.iter().enumerate() {
            for (gx, cell) in row.bytes().enumerate() {
                if cell == b'#' {
                    pixels[gy * width + index * CELL_W as usize + gx] = 255;
                }
            }
        }
    }
    pixels
}

/// Lays out `text` (top-left origin, pixels) into glyph quads. `\n` starts a
/// new line. `scale` multiplies the 6×8 cell.
pub fn layout(text: &str, origin_x: f32, origin_y: f32, scale: f32) -> Vec<GlyphQuad> {
    let atlas_w = atlas_width() as f32;
    let mut quads = Vec::with_capacity(text.len());
    let (mut x, mut y) = (origin_x, origin_y);
    for ch in text.to_ascii_uppercase().chars() {
        if ch == '\n' {
            x = origin_x;
            y += CELL_H as f32 * scale;
            continue;
        }
        let index = CHARSET.find(ch).unwrap_or(CHARSET.len() - 1);
        if ch != ' ' {
            let u0 = (index as u32 * CELL_W) as f32 / atlas_w;
            quads.push(GlyphQuad {
                x,
                y,
                w: 5.0 * scale,
                h: 7.0 * scale,
                u0,
                v0: 0.0,
                u1: u0 + 5.0 / atlas_w,
                v1: 7.0 / CELL_H as f32,
            });
        }
        x += CELL_W as f32 * scale;
    }
    quads
}

/// 5×7 pixel rows per glyph ('#' = set).
fn glyph(ch: char) -> [&'static str; 7] {
    match ch {
        'A' => [" ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
        'B' => ["#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### "],
        'C' => [" ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### "],
        'D' => ["#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### "],
        'E' => ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####"],
        'F' => ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#    "],
        'G' => [" ### ", "#   #", "#    ", "# ###", "#   #", "#   #", " ### "],
        'H' => ["#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
        'I' => [" ### ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
        'J' => ["  ###", "   # ", "   # ", "   # ", "   # ", "#  # ", " ##  "],
        'K' => ["#   #", "#  # ", "# #  ", "##   ", "# #  ", "#  # ", "#   #"],
        'L' => ["#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####"],
        'M' => ["#   #", "## ##", "# # #", "# # #", "#   #", "#   #", "#   #"],
        'N' => ["#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #"],
        'O' => [" ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
        'P' => ["#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    "],
        'Q' => [" ### ", "#   #", "#   #", "#   #", "# # #", "#  # ", " ## #"],
        'R' => ["#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #"],
        'S' => [" ####", "#    ", "#    ", " ### ", "    #", "    #", "#### "],
        'T' => ["#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  "],
        'U' => ["#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
        'V' => ["#   #", "#   #", "#   #", "#   #", "#   #", " # # ", "  #  "],
        'W' => ["#   #", "#   #", "#   #", "# # #", "# # #", "## ##", "#   #"],
        'X' => ["#   #", "#   #", " # # ", "  #  ", " # # ", "#   #", "#   #"],
        'Y' => ["#   #", "#   #", " # # ", "  #  ", "  #  ", "  #  ", "  #  "],
        'Z' => ["#####", "    #", "   # ", "  #  ", " #   ", "#    ", "#####"],
        '0' => [" ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### "],
        '1' => ["  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
        '2' => [" ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####"],
        '3' => [" ### ", "#   #", "    #", "  ## ", "    #", "#   #", " ### "],
        '4' => ["   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # "],
        '5' => ["#####", "#    ", "#### ", "    #", "    #", "#   #", " ### "],
        '6' => ["  ## ", " #   ", "#    ", "#### ", "#   #", "#   #", " ### "],
        '7' => ["#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   "],
        '8' => [" ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### "],
        '9' => [" ### ", "#   #", "#   #", " ####", "    #", "   # ", " ##  "],
        '.' => ["     ", "     ", "     ", "     ", "     ", "  ## ", "  ## "],
        ':' => ["     ", "  ## ", "  ## ", "     ", "  ## ", "  ## ", "     "],
        '-' => ["     ", "     ", "     ", " ### ", "     ", "     ", "     "],
        '+' => ["     ", "  #  ", "  #  ", "#####", "  #  ", "  #  ", "     "],
        '/' => ["    #", "    #", "   # ", "  #  ", " #   ", "#    ", "#    "],
        ',' => ["     ", "     ", "     ", "     ", "  ## ", "  #  ", " #   "],
        '%' => ["##  #", "##  #", "   # ", "  #  ", " #   ", "#  ##", "#  ##"],
        '[' => ["  ## ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  ## "],
        ']' => [" ##  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ##  "],
        _ => ["     "; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_charset_glyph_is_authored() {
        for ch in CHARSET.chars() {
            if ch == ' ' {
                continue;
            }
            let rows = glyph(ch);
            assert!(
                rows.iter().any(|r| r.contains('#')),
                "glyph for {ch:?} is empty"
            );
            for row in rows {
                assert_eq!(row.len(), 5, "glyph {ch:?} row width");
            }
        }
    }

    #[test]
    fn atlas_contains_set_pixels_per_glyph() {
        let atlas = build_atlas();
        let width = atlas_width() as usize;
        for (i, ch) in CHARSET.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let any = (0..CELL_H as usize).any(|y| {
                (0..CELL_W as usize).any(|x| atlas[y * width + i * CELL_W as usize + x] != 0)
            });
            assert!(any, "no atlas pixels for {ch:?}");
        }
    }

    #[test]
    fn layout_advances_and_wraps() {
        let quads = layout("ab\ncd", 10.0, 20.0, 2.0);
        assert_eq!(quads.len(), 4);
        assert_eq!(quads[0].x, 10.0);
        assert_eq!(quads[1].x, 10.0 + 12.0); // 6px cell * scale 2
        assert_eq!(quads[2].x, 10.0); // new line resets x
        assert_eq!(quads[2].y, 20.0 + 16.0); // 8px cell * scale 2
        // Spaces advance without emitting quads.
        assert_eq!(layout("a b", 0.0, 0.0, 1.0).len(), 2);
    }

    #[test]
    fn lowercase_folds_to_uppercase() {
        let lower = layout("abc", 0.0, 0.0, 1.0);
        let upper = layout("ABC", 0.0, 0.0, 1.0);
        assert_eq!(lower.len(), upper.len());
        assert!(lower.iter().zip(&upper).all(|(a, b)| a.u0 == b.u0));
    }
}
