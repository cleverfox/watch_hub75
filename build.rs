use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Parsed glyph from BDF font
struct Glyph {
    encoding: u32,
    width: u8,
    height: u8,
    dx: i8,       // x offset from origin
    dy: i8,       // y offset from baseline
    advance: u8,  // horizontal advance (DWIDTH x)
    bitmap: Vec<u8>, // packed row data, 1 bit per pixel, MSB first
}

fn main() {
    println!("cargo:rerun-if-changed=custom_font.bdf");

    let bdf_path = Path::new("custom_font.bdf");
    if !bdf_path.exists() {
        panic!("custom_font.bdf not found in project root");
    }

    let glyphs = parse_bdf(bdf_path);
    generate_font_rs(&glyphs);
}

fn parse_bdf(path: &Path) -> BTreeMap<u32, Glyph> {
    let file = fs::File::open(path).expect("Failed to open BDF");
    let reader = BufReader::new(file);
    let mut glyphs = BTreeMap::new();

    let mut in_char = false;
    let mut in_bitmap = false;
    let mut encoding: u32 = 0;
    let mut bbw: u8 = 0;
    let mut bbh: u8 = 0;
    let mut bbx: i8 = 0;
    let mut bby: i8 = 0;
    let mut dwidth_x: u8 = 0;
    let mut bitmap_rows: Vec<u8> = Vec::new();

    for line in reader.lines() {
        let line = line.unwrap();
        let line = line.trim();

        if line.starts_with("STARTCHAR") {
            in_char = true;
            in_bitmap = false;
            encoding = 0;
            bbw = 0;
            bbh = 0;
            bbx = 0;
            bby = 0;
            dwidth_x = 8;
            bitmap_rows.clear();
        } else if line == "ENDCHAR" {
            if in_char && encoding > 0 {
                glyphs.insert(encoding, Glyph {
                    encoding,
                    width: bbw,
                    height: bbh,
                    dx: bbx,
                    dy: bby,
                    advance: dwidth_x,
                    bitmap: bitmap_rows.clone(),
                });
            }
            in_char = false;
            in_bitmap = false;
        } else if in_char {
            if line.starts_with("ENCODING ") {
                encoding = line[9..].parse().unwrap_or(0);
            } else if line.starts_with("BBX ") {
                let parts: Vec<&str> = line[4..].split_whitespace().collect();
                if parts.len() >= 4 {
                    bbw = parts[0].parse().unwrap_or(0);
                    bbh = parts[1].parse().unwrap_or(0);
                    bbx = parts[2].parse().unwrap_or(0);
                    bby = parts[3].parse().unwrap_or(0);
                }
            } else if line.starts_with("DWIDTH ") {
                let parts: Vec<&str> = line[7..].split_whitespace().collect();
                dwidth_x = parts[0].parse().unwrap_or(8);
            } else if line == "BITMAP" {
                in_bitmap = true;
            } else if in_bitmap {
                // Parse hex row data
                if let Ok(val) = u32::from_str_radix(line, 16) {
                    // Store as bytes (up to 4 bytes covers up to 32px wide glyphs)
                    if bbw <= 8 {
                        bitmap_rows.push((val >> (line.len() * 4 - 8)) as u8);
                    } else {
                        // 16px wide glyph: 2 bytes per row
                        let shift = line.len() * 4 - 16;
                        bitmap_rows.push((val >> (shift + 8)) as u8);
                        bitmap_rows.push((val >> shift) as u8);
                    }
                }
            }
        }
    }

    glyphs
}

fn generate_font_rs(glyphs: &BTreeMap<u32, Glyph>) {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("font_data.rs");

    let mut code = String::new();

    code.push_str("// Auto-generated from custom_font.bdf by build.rs\n\n");

    code.push_str("#[derive(Clone, Copy)]\n");
    code.push_str("pub struct GlyphData {\n");
    code.push_str("    pub width: u8,\n");
    code.push_str("    pub height: u8,\n");
    code.push_str("    pub dx: i8,\n");
    code.push_str("    pub dy: i8,\n");
    code.push_str("    pub advance: u8,\n");
    code.push_str("    pub bytes_per_row: u8,\n");
    code.push_str("    pub bitmap_offset: u16,\n");
    code.push_str("}\n\n");

    // Collect all bitmap bytes into a single array
    let mut all_bitmap_bytes: Vec<u8> = Vec::new();
    let mut glyph_entries: Vec<(u32, Glyph, u16)> = Vec::new();

    for (cp, glyph) in glyphs {
        let offset = all_bitmap_bytes.len() as u16;
        all_bitmap_bytes.extend_from_slice(&glyph.bitmap);
        glyph_entries.push((*cp, Glyph {
            encoding: glyph.encoding,
            width: glyph.width,
            height: glyph.height,
            dx: glyph.dx,
            dy: glyph.dy,
            advance: glyph.advance,
            bitmap: Vec::new(), // not used in output
        }, offset));
    }

    // Bitmap data array
    code.push_str(&format!("pub static BITMAP_DATA: [u8; {}] = [\n    ", all_bitmap_bytes.len()));
    for (i, b) in all_bitmap_bytes.iter().enumerate() {
        code.push_str(&format!("0x{:02X}, ", b));
        if (i + 1) % 16 == 0 {
            code.push_str("\n    ");
        }
    }
    code.push_str("\n];\n\n");

    // Glyph lookup table (sorted by codepoint for binary search)
    code.push_str(&format!("pub static GLYPHS: [(u32, GlyphData); {}] = [\n", glyph_entries.len()));
    for (cp, glyph, offset) in &glyph_entries {
        let bytes_per_row = if glyph.width <= 8 { 1u8 } else { 2u8 };
        code.push_str(&format!(
            "    (0x{:04X}, GlyphData {{ width: {}, height: {}, dx: {}, dy: {}, advance: {}, bytes_per_row: {}, bitmap_offset: {} }}),\n",
            cp, glyph.width, glyph.height, glyph.dx, glyph.dy, glyph.advance, bytes_per_row, offset
        ));
    }
    code.push_str("];\n");

    fs::write(dest, code).expect("Failed to write font_data.rs");
}
