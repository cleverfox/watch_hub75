/// BDF font rendering with Thai combining character support
///
/// Font data is generated at compile time by build.rs from custom_font.bdf
/// Contains ASCII, Cyrillic, and Thai glyphs.

include!(concat!(env!("OUT_DIR"), "/font_data.rs"));

use crate::framebuffer::{FBType, ShadowBuffer, COLS, ROWS};

/// Thai combining characters that don't advance the cursor
/// These render above (or below) the previous consonant
fn is_thai_combining(cp: u32) -> bool {
    matches!(cp,
        0x0E31 |           // Mai han akat
        0x0E34..=0x0E3A |  // Sara i, sara ii, sara ue, sara uee, etc.
        0x0E47..=0x0E4E    // Maitaikhu, mai ek, mai tho, mai tri, mai chattawa, etc.
    )
}

/// Look up a glyph by Unicode codepoint (binary search)
pub fn get_glyph(codepoint: u32) -> Option<&'static GlyphData> {
    GLYPHS
        .binary_search_by_key(&codepoint, |&(cp, _)| cp)
        .ok()
        .map(|idx| &GLYPHS[idx].1)
}

/// Get bitmap row data for a glyph
pub fn glyph_bitmap_rows(glyph: &GlyphData) -> &'static [u8] {
    let start = glyph.bitmap_offset as usize;
    let len = glyph.bytes_per_row as usize * glyph.height as usize;
    &BITMAP_DATA[start..start + len]
}

/// Check if a pixel is set in a glyph bitmap
fn pixel_set(glyph: &GlyphData, row_data: &[u8], gx: usize, gy: usize) -> bool {
    let row_offset = gy * glyph.bytes_per_row as usize;
    if glyph.bytes_per_row == 1 {
        (row_data[row_offset] >> (7 - gx)) & 1 == 1
    } else {
        // 16px wide: 2 bytes per row
        let bit_idx = 15 - gx;
        let byte_idx = row_offset + (1 - bit_idx / 8);
        (row_data[byte_idx] >> (bit_idx % 8)) & 1 == 1
    }
}

/// Render a text string into the framebuffer with black outline for readability.
/// Returns the total width in pixels drawn.
pub fn render_text(
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    x: i32,
    y: i32,
    r: u8,
    g: u8,
    b: u8,
    scale: u8,
    text: &str,
) -> i32 {
    // Always draw outline offsets in black (erase or fresh outline),
    // then draw foreground on top. The "skip if unchanged" optimization
    // in set_pixel makes black-on-black a fast no-op.
    static OFFSETS: [(i32, i32); 8] = [
        (-1, -1), (-1, 0), (-1, 1),
        (0, -1),           (0, 1),
        (1, -1),  (1, 0),  (1, 1),
    ];
    for &(ox, oy) in &OFFSETS {
        render_text_inner(shadow, fb, x + ox, y + oy, 0, 0, 0, scale, text);
    }
    // Draw foreground text on top
    render_text_inner(shadow, fb, x, y, r, g, b, scale, text)
}

/// Inner text rendering — draws text at given position in given color.
/// y is treated as vertical CENTER of the text (matching CircuitPython Label behavior).
/// The BDF font has a 16px cell height. For scale S, text is 16*S pixels tall,
/// so the top of the text is at y - 8*S.
const FONT_CELL_HEIGHT: i32 = 16;
const FONT_BASELINE_OFFSET: i32 = 12; // baseline is ~12px from top in 16px cell

fn render_text_inner(
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    x: i32,
    y: i32,
    r: u8,
    g: u8,
    b: u8,
    scale: u8,
    text: &str,
) -> i32 {
    let scale = scale.max(1) as i32;
    let mut cx = x;

    // y = vertical center of text cell
    // top of cell = y - (FONT_CELL_HEIGHT * scale) / 2
    let cell_top = y - (FONT_CELL_HEIGHT * scale) / 2;

    for ch in text.chars() {
        let cp = ch as u32;

        let combining = is_thai_combining(cp);

        if let Some(glyph) = get_glyph(cp) {
            let row_data = glyph_bitmap_rows(glyph);
            let draw_x = if combining {
                cx - glyph.advance as i32 * scale
            } else {
                cx
            };

            // Glyph position relative to cell top:
            // baseline is at cell_top + FONT_BASELINE_OFFSET * scale
            // glyph top is at baseline - (glyph.height + glyph.dy) * scale
            let baseline = cell_top + FONT_BASELINE_OFFSET * scale;
            let glyph_top = baseline - (glyph.height as i32 + glyph.dy as i32) * scale;

            for gy in 0..glyph.height as usize {
                for gx in 0..glyph.width as usize {
                    if pixel_set(glyph, row_data, gx, gy) {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = draw_x + (gx as i32 + glyph.dx as i32) * scale + sx;
                                let py = glyph_top + gy as i32 * scale + sy;
                                if px >= 0 && px < COLS as i32 && py >= 0 && py < ROWS as i32 {
                                    shadow.set_pixel(fb, px as usize, py as usize, r, g, b);
                                }
                            }
                        }
                    }
                }
            }

            if !combining {
                cx += glyph.advance as i32 * scale;
            }
        } else {
            if !combining {
                cx += 6 * scale;
            }
        }
    }

    cx - x
}

/// Get the bounding box of rendered text without actually drawing.
/// Returns (width, height) where height is the full cell height × scale.
pub fn measure_text(text: &str, scale: u8) -> (i32, i32) {
    let scale = scale.max(1) as i32;
    let mut width: i32 = 0;

    for ch in text.chars() {
        let cp = ch as u32;
        if is_thai_combining(cp) {
            continue;
        }
        if let Some(glyph) = get_glyph(cp) {
            width += glyph.advance as i32 * scale;
        } else {
            width += 6 * scale;
        }
    }

    // Height is always the font cell height × scale
    let height = FONT_CELL_HEIGHT * scale;
    (width, height)
}
