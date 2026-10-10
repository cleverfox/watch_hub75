/// TCP protocol parser — heap-free where possible

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::app::TzEntry;
use crate::bitmap_store::BitmapStore;
use crate::font;
use crate::framebuffer::{FBType, ShadowBuffer, COLS, ROWS};
use crate::text_overlay::TextOverlayManager;

pub enum CmdResult {
    None,
    Response(heapless::String<256>),
    NeedUpload {
        name: heapless::String<32>,
        width: u16,
        height: u16,
        total_bytes: usize,
        rle: bool,
    },
    Wait(u32),
    StartClock {
        unix_ts: i64,
        timezones: heapless::Vec<TzEntry, 5>,
        thai_numerals: bool,
    },
    StopApp,
    /// Set or clear the "upcoming event" banner on the running ClockApp.
    /// `time` empty string means: clear the event.
    SetClockEvent {
        time: heapless::String<8>,
        label: heapless::String<24>,
    },
}

fn parse_color(hex: &str) -> u32 {
    u32::from_str_radix(hex, 16).unwrap_or(0)
}

fn color_components(rgb: u32) -> (u8, u8, u8) {
    (
        ((rgb >> 16) & 0xFF) as u8,
        ((rgb >> 8) & 0xFF) as u8,
        (rgb & 0xFF) as u8,
    )
}

/// Process a single command line
pub fn process_command(
    line: &str,
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    bitmaps: &mut BitmapStore,
    text_mgr: &mut TextOverlayManager,
) -> CmdResult {
    let line = line.trim();
    if line.is_empty() {
        return CmdResult::None;
    }

    // Parse command without heap allocation — use iterator
    let mut iter = line.split_whitespace();
    let cmd_raw = match iter.next() {
        Some(c) => c,
        None => return CmdResult::None,
    };

    // Uppercase first word (max 10 chars)
    let mut cmd_buf = [0u8; 12];
    let cmd_len = cmd_raw.len().min(12);
    for (i, b) in cmd_raw.bytes().take(12).enumerate() {
        cmd_buf[i] = b.to_ascii_uppercase();
    }
    let cmd = core::str::from_utf8(&cmd_buf[..cmd_len]).unwrap_or("");

    // Collect remaining parts into fixed array
    let mut parts: [&str; 12] = [""; 12];
    let mut nparts = 0;
    for p in iter {
        if nparts < 12 {
            parts[nparts] = p;
            nparts += 1;
        }
    }

    match cmd {
        "SIZE" => {
            let mut s = heapless::String::<256>::new();
            let _ = core::fmt::write(&mut s, format_args!("SIZE {} {}\n", COLS, ROWS));
            CmdResult::Response(s)
        }

        "PING" => {
            let mut s = heapless::String::<256>::new();
            let _ = s.push_str("PONG\n");
            CmdResult::Response(s)
        }

        "HELP" => {
            let mut s = heapless::String::<256>::new();
            let _ = s.push_str("SIZE|PX x y [rrggbb]|RECT x y w h rrggbb|CLR|TEXT x y c s msg|CLRTXT|WAIT ms|UPLOAD n w h|SHOW n [x y]|LIST|BRIGHTNESS 1-100|PING\n");
            CmdResult::Response(s)
        }

        "PX" if nparts >= 2 => {
            let x: usize = parts[0].parse().unwrap_or(0);
            let y: usize = parts[1].parse().unwrap_or(0);
            if nparts >= 3 {
                let (r, g, b) = color_components(parse_color(parts[2]));
                shadow.set_pixel(fb, x, y, r, g, b);
                CmdResult::None
            } else {
                let (r, g, b) = shadow.get_pixel(x, y);
                let mut s = heapless::String::<256>::new();
                let _ = core::fmt::write(&mut s, format_args!("PX {} {} {:02x}{:02x}{:02x}\n", x, y, r, g, b));
                CmdResult::Response(s)
            }
        }

        "RECT" if nparts >= 5 => {
            let x: usize = parts[0].parse().unwrap_or(0);
            let y: usize = parts[1].parse().unwrap_or(0);
            let w: usize = parts[2].parse().unwrap_or(0);
            let h: usize = parts[3].parse().unwrap_or(0);
            let (r, g, b) = color_components(parse_color(parts[4]));
            shadow.fill_rect(fb, x, y, w, h, r, g, b);
            CmdResult::None
        }

        "CLR" => {
            let (r, g, b) = if nparts >= 1 && !parts[0].is_empty() {
                color_components(parse_color(parts[0]))
            } else {
                (0, 0, 0)
            };
            shadow.clear(fb, r, g, b);
            text_mgr.reset();
            CmdResult::None
        }

        "TEXT" if nparts >= 5 => {
            let x: i32 = parts[0].parse().unwrap_or(0);
            let y: i32 = parts[1].parse().unwrap_or(0);
            let (r, g, b) = color_components(parse_color(parts[2]));
            let scale: u8 = parts[3].parse().unwrap_or(1);
            // Message is everything after "TEXT x y color scale "
            let message = get_message_part(line, 5);
            let (tw, th) = font::measure_text(message, scale);
            font::render_text(shadow, fb, x, y, r, g, b, scale, message);
            // Bounding box: y is center, so top = y - th/2, include ±1 outline
            let bx = x - 1;
            let by = y - th / 2 - 1;
            let bw = tw + 2;
            let bh = th + 2;
            text_mgr.record(bx, by, bw, bh);
            CmdResult::None
        }

        "BTEXT" if nparts >= 5 => {
            let x: i32 = parts[0].parse().unwrap_or(0);
            let y: i32 = parts[1].parse().unwrap_or(0);
            let (r, g, b) = color_components(parse_color(parts[2]));
            let scale: u8 = parts[3].parse().unwrap_or(1);
            let message = get_message_part(line, 5);
            font::render_text(shadow, fb, x, y, r, g, b, scale, message);
            CmdResult::None
        }

        "CLRTXT" => {
            text_mgr.clear_all(shadow, fb);
            CmdResult::None
        }

        "UPLOAD" if nparts >= 3 => {
            let mut name = heapless::String::<32>::new();
            let _ = name.push_str(parts[0]);
            let w: u16 = parts[1].parse().unwrap_or(0);
            let h: u16 = parts[2].parse().unwrap_or(0);
            let total = w as usize * h as usize * 3;
            CmdResult::NeedUpload {
                name,
                width: w,
                height: h,
                total_bytes: total,
                rle: false,
            }
        }

        "UPLOADRLE" if nparts >= 4 => {
            let mut name = heapless::String::<32>::new();
            let _ = name.push_str(parts[0]);
            let w: u16 = parts[1].parse().unwrap_or(0);
            let h: u16 = parts[2].parse().unwrap_or(0);
            let rle_bytes: usize = parts[3].parse().unwrap_or(0);
            CmdResult::NeedUpload {
                name,
                width: w,
                height: h,
                total_bytes: rle_bytes,
                rle: true,
            }
        }

        "SHOW" if nparts >= 1 => {
            let name = parts[0];
            let x_off: usize = if nparts >= 2 { parts[1].parse().unwrap_or(0) } else { 0 };
            let y_off: usize = if nparts >= 3 { parts[2].parse().unwrap_or(0) } else { 0 };
            if bitmaps.show(shadow, fb, name, x_off, y_off) {
                CmdResult::None
            } else {
                let mut s = heapless::String::<256>::new();
                let _ = core::fmt::write(&mut s, format_args!("ERR {} not found\n", name));
                CmdResult::Response(s)
            }
        }

        "LIST" => {
            let names = bitmaps.list();
            let mut s = heapless::String::<256>::new();
            let _ = s.push_str("LIST");
            for name in names {
                let _ = s.push_str(" ");
                let _ = s.push_str(name);
            }
            let _ = s.push_str("\n");
            CmdResult::Response(s)
        }

        "BRIGHTNESS" if nparts >= 1 => {
            let val: u8 = parts[0].parse().unwrap_or(100).clamp(1, 100);
            shadow.set_brightness(fb, val);
            let mut s = heapless::String::<256>::new();
            let _ = core::fmt::write(&mut s, format_args!("BRIGHTNESS {}\n", val));
            CmdResult::Response(s)
        }

        "WAIT" if nparts >= 1 => {
            let ms: u32 = parts[0].parse().unwrap_or(0);
            CmdResult::Wait(ms)
        }

        "CLOCK" if nparts >= 3 => {
            // CLOCK <unix_ts> <tz1_name> <tz1_offset_min> [<tz2_name> <tz2_offset> ...]
            let unix_ts: i64 = parts[0].parse().unwrap_or(0);
            let mut timezones = heapless::Vec::<TzEntry, 5>::new();
            let mut i = 1;
            while i + 1 < nparts {
                let mut name = heapless::String::<8>::new();
                let _ = name.push_str(parts[i]);
                let offset: i16 = parts[i + 1].parse().unwrap_or(0);
                let _ = timezones.push(TzEntry {
                    name,
                    offset_minutes: offset,
                });
                i += 2;
            }
            // Check for optional THAI keyword after timezone pairs
            let thai_numerals = i < nparts && parts[i].eq_ignore_ascii_case("THAI");

            if timezones.is_empty() {
                CmdResult::None
            } else {
                let mut s = heapless::String::<256>::new();
                let _ = core::fmt::write(
                    &mut s,
                    format_args!("CLOCK OK {} timezones\n", timezones.len()),
                );
                // Return StartClock — coordinator will start the app and send response
                CmdResult::StartClock { unix_ts, timezones, thai_numerals }
            }
        }

        "NETCHECK" => {
            // NETCHECK               : report config/status
            // NETCHECK off | on      : disable / enable with default hosts
            // NETCHECK ext_ip [int_ip]: override hosts (and enable)
            let mut s = heapless::String::<256>::new();
            if nparts == 0 {
                let state = if crate::netcheck::is_disabled() {
                    "off"
                } else {
                    match crate::netcheck::status() {
                        crate::netcheck::NET_OK => "ok",
                        crate::netcheck::NET_LAN_ONLY => "lan-only",
                        _ => "down",
                    }
                };
                let _ = core::fmt::write(&mut s, format_args!("NETCHECK {}", state));
                match crate::netcheck::ext_override() {
                    Some(ip) => { let _ = core::fmt::write(&mut s, format_args!(" ext={}", ip)); }
                    None => { let _ = s.push_str(" ext=default"); }
                }
                match crate::netcheck::int_override() {
                    Some(ip) => { let _ = core::fmt::write(&mut s, format_args!(" int={}", ip)); }
                    None => { let _ = s.push_str(" int=default"); }
                }
                let _ = s.push('\n');
            } else if parts[0].eq_ignore_ascii_case("off") {
                crate::netcheck::disable();
                let _ = s.push_str("NETCHECK OFF\n");
            } else if parts[0].eq_ignore_ascii_case("on") {
                crate::netcheck::reset();
                let _ = s.push_str("NETCHECK ON\n");
            } else {
                let ext = crate::sntp::parse_ipv4(parts[0]);
                let int = if nparts >= 2 { crate::sntp::parse_ipv4(parts[1]).map(Some) } else { Some(None) };
                match (ext, int) {
                    (Some(ext), Some(int)) => {
                        crate::netcheck::set_hosts(ext, int);
                        let _ = s.push_str("NETCHECK OK\n");
                    }
                    _ => { let _ = s.push_str("NETCHECK ERR bad-ip\n"); }
                }
            }
            CmdResult::Response(s)
        }

        "CLKEVT" => {
            // CLKEVT [hh:mm [label...]]
            // - no args  : clear the event banner
            // - hh:mm    : show time only
            // - hh:mm L  : show "hh:mm L..."
            let mut time = heapless::String::<8>::new();
            let mut label = heapless::String::<24>::new();
            if nparts >= 1 {
                let _ = time.push_str(truncate_str(parts[0], 8));
                let _ = label.push_str(truncate_str(get_message_part(line, 2), 24));
            }
            CmdResult::SetClockEvent { time, label }
        }

        "APPSTOP" => CmdResult::StopApp,

        _ => CmdResult::None,
    }
}

/// Check if a command is a drawing command that should stop an active app
pub fn is_drawing_command(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    // Compare case-insensitively
    first.eq_ignore_ascii_case("PX")
        || first.eq_ignore_ascii_case("RECT")
        || first.eq_ignore_ascii_case("TEXT")
        || first.eq_ignore_ascii_case("BTEXT")
        || first.eq_ignore_ascii_case("CLR")
        || first.eq_ignore_ascii_case("CLRTXT")
        || first.eq_ignore_ascii_case("SHOW")
}

/// Get the message part of a command (everything from the Nth whitespace-delimited word onwards)
fn get_message_part(line: &str, skip_words: usize) -> &str {
    let mut start = 0;
    let mut words_skipped = 0;
    let bytes = line.as_bytes();
    let len = bytes.len();

    // Skip leading whitespace and words
    while start < len && words_skipped < skip_words {
        // Skip whitespace
        while start < len && bytes[start] == b' ' {
            start += 1;
        }
        // Skip word
        while start < len && bytes[start] != b' ' {
            start += 1;
        }
        words_skipped += 1;
    }
    // Skip whitespace before message
    while start < len && bytes[start] == b' ' {
        start += 1;
    }
    &line[start..]
}

/// Truncate to at most `max` bytes on a char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Largest bitmap accepted: a full screen of RGB888.
pub const MAX_BITMAP_BYTES: usize = COLS * ROWS * 3;

/// Check an upload's declared size before its payload is read.
pub fn validate_upload(width: u16, height: u16, total_bytes: usize, rle: bool) -> Result<(), &'static str> {
    let pixels = width as usize * height as usize;
    if pixels == 0 || pixels * 3 > MAX_BITMAP_BYTES {
        return Err("bad-size");
    }
    // RLE worst case is one 4-byte run per pixel
    if rle && (total_bytes == 0 || total_bytes > pixels * 4) {
        return Err("bad-rle-length");
    }
    Ok(())
}

/// Decode RLE data: [count, r, g, b, count, r, g, b, ...]
/// Output is capped at `max_len` bytes.
pub fn decode_rle(data: &[u8], max_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max_len);
    for run in data.chunks_exact(4) {
        let count = (run[0] as usize).min((max_len - out.len()) / 3);
        for _ in 0..count {
            out.extend_from_slice(&run[1..4]);
        }
        if out.len() + 3 > max_len {
            break;
        }
    }
    out
}

/// Complete an UPLOAD (raw or RLE)
pub fn complete_upload(
    bitmaps: &mut BitmapStore,
    name: &str,
    width: u16,
    height: u16,
    data: Vec<u8>,
    rle: bool,
) -> heapless::String<64> {
    let expected = width as usize * height as usize * 3;
    let rgb_data = if rle {
        decode_rle(&data, expected)
    } else {
        data
    };
    let mut s = heapless::String::<64>::new();
    // BitmapStore::show indexes w*h*3 bytes — never store a short bitmap
    if rgb_data.len() != expected {
        let _ = core::fmt::write(&mut s, format_args!("ERR {} size-mismatch\n", name));
        return s;
    }
    bitmaps.store(name, width, height, rgb_data);
    let _ = core::fmt::write(&mut s, format_args!("OK {} {} {}\n", name, width, height));
    s
}
