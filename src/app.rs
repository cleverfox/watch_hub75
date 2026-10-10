/// On-device apps that run independently of TCP client connection.

use embassy_time::{Duration, Instant};
use heapless::String;
use log::info;

use crate::bitmap_store::BitmapStore;
use crate::font;
use crate::framebuffer::{FBType, ShadowBuffer};
use crate::time_util;

// ---- Embedded mini flags (10x7 pixels, RGB) ----
// Thai flag: red-white-blue-white-red horizontal stripes
const FLAG_TH: [(u8, u8, u8); 70] = {
    let mut f = [(0u8, 0u8, 0u8); 70];
    let mut i = 0;
    while i < 70 {
        let row = i / 10;
        f[i] = match row {
            0     => (0xED, 0x1C, 0x24), // red
            1     => (0xFF, 0xFF, 0xFF), // white
            2 | 3 | 4 => (0x24, 0x1D, 0x4F), // blue
            5     => (0xFF, 0xFF, 0xFF), // white
            6     => (0xED, 0x1C, 0x24), // red
            _     => (0, 0, 0),
        };
        i += 1;
    }
    f
};

// Russian flag: white-blue-red
const FLAG_RU: [(u8, u8, u8); 70] = {
    let mut f = [(0u8, 0u8, 0u8); 70];
    let mut i = 0;
    while i < 70 {
        let row = i / 10;
        f[i] = match row {
            0 | 1     => (0xFF, 0xFF, 0xFF), // white
            2 | 3 | 4 => (0x00, 0x39, 0xA6), // blue
            5 | 6     => (0xD5, 0x2B, 0x1E), // red
            _         => (0, 0, 0),
        };
        i += 1;
    }
    f
};

// Spanish flag: red-yellow-red
const FLAG_ES: [(u8, u8, u8); 70] = {
    let mut f = [(0u8, 0u8, 0u8); 70];
    let mut i = 0;
    while i < 70 {
        let row = i / 10;
        f[i] = match row {
            0 | 1     => (0xAA, 0x15, 0x1B), // red
            2 | 3 | 4 => (0xF1, 0xBF, 0x00), // yellow
            5 | 6     => (0xAA, 0x15, 0x1B), // red
            _         => (0, 0, 0),
        };
        i += 1;
    }
    f
};

/// Draw an embedded 10x7 flag at the given position
fn draw_embedded_flag(
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    flag: &[(u8, u8, u8); 70],
    x: usize,
    y: usize,
) {
    for py in 0..7 {
        for px in 0..10 {
            let (r, g, b) = flag[py * 10 + px];
            shadow.set_pixel(fb, x + px, y + py, r, g, b);
        }
    }
}

/// Look up a built-in flag by timezone name (case-insensitive)
fn get_builtin_flag(tz_name: &str) -> Option<&'static [(u8, u8, u8); 70]> {
    match tz_name {
        "BKK" | "bkk" => Some(&FLAG_TH),
        "MSK" | "msk" => Some(&FLAG_RU),
        "MAD" | "mad" => Some(&FLAG_ES),
        _ => None,
    }
}

// Layout constants matching world_clock_rs.py
const SIDE_X: i32 = 86;
const MAIN_FLAG_X: usize = 39; // (SIDE_X - 8) / 2
const MAIN_FLAG_Y: usize = 2;

// Text slot definitions: (x, y, r, g, b, scale)
struct TextSlot {
    x: i32,
    y: i32,
    r: u8,
    g: u8,
    b: u8,
    scale: u8,
}

const SLOT_MAIN: TextSlot = TextSlot { x: 4, y: 26, r: 0x00, g: 0xFF, b: 0xAA, scale: 2 };
// Date at its original vertical position — used when no upcoming event is shown.
const SLOT_DATE: TextSlot = TextSlot { x: 4, y: 52, r: 0x66, g: 0x66, b: 0x66, scale: 1 };
// Date shifted up — used when the event banner is shown below it.
const SLOT_DATE_COMPACT: TextSlot = TextSlot { x: 4, y: 42, r: 0x66, g: 0x66, b: 0x66, scale: 1 };
// Upcoming event banner. Stays in main section (x<SIDE_X) so it does not
// collide with the Moscow side clock at y=58.
const SLOT_EVENT: TextSlot = TextSlot { x: 4, y: 56, r: 0xFF, g: 0xAA, b: 0x44, scale: 1 };
const SLOT_GMT_LABEL: TextSlot = TextSlot { x: SIDE_X + 8, y: 7, r: 0x44, g: 0x88, b: 0xFF, scale: 1 };

// Side clock slots: correspond to timezone indices 1 (UTC), 2 (Madrid), 3 (Moscow)
// Layout is top-to-bottom: UTC, Madrid, Moscow
const SIDE_SLOTS: [TextSlot; 3] = [
    TextSlot { x: SIDE_X, y: 18, r: 0xFF, g: 0xFF, b: 0xFF, scale: 1 },  // UTC
    TextSlot { x: SIDE_X, y: 38, r: 0xFF, g: 0xFF, b: 0xFF, scale: 1 },  // Madrid
    TextSlot { x: SIDE_X, y: 58, r: 0xFF, g: 0xFF, b: 0xFF, scale: 1 },  // Moscow
];

// Side flag positions: only for timezones that have flags (indices 1=Madrid, 2=Moscow)
// UTC uses GMT text label instead of flag
const SIDE_FLAG_POS: [(usize, usize); 2] = [
    (SIDE_X as usize + 15, 24),  // ES flag (above Madrid time)
    (SIDE_X as usize + 15, 44),  // RU flag (above Moscow time)
];

/// Maximum number of timezone entries
const MAX_TZ: usize = 5;
/// Number of tracked text slots: main + date + 3 side clocks + event = 6
/// (gmt label is drawn as static and not tracked as a slot)
const MAX_TEXT_SLOTS: usize = 6;
/// Slot index for the date — reused whether it is drawn at normal or compact Y.
const SLOT_IDX_DATE: usize = 1;
/// Slot index for the upcoming-event banner.
const SLOT_IDX_EVENT: usize = 5;

#[derive(Clone)]
pub struct TzEntry {
    pub name: String<8>,
    pub offset_minutes: i16,
}

/// Default timezones for the NTP auto-started clock: BKK +7h, UTC, MAD +2h, MSK +3h
pub fn default_timezones() -> heapless::Vec<TzEntry, MAX_TZ> {
    let mut tzs = heapless::Vec::new();
    for (name, offset_minutes) in [("BKK", 420), ("UTC", 0), ("MAD", 120), ("MSK", 180)] {
        let _ = tzs.push(TzEntry { name: String::try_from(name).unwrap(), offset_minutes });
    }
    tzs
}

pub enum ActiveApp {
    None,
    Clock(ClockApp),
}

impl ActiveApp {
    pub fn is_running(&self) -> bool {
        !matches!(self, ActiveApp::None)
    }

    pub fn tick(
        &mut self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        bitmaps: &BitmapStore,
    ) -> Option<Duration> {
        match self {
            ActiveApp::None => None,
            ActiveApp::Clock(clock) => Some(clock.tick(shadow, fb, bitmaps)),
        }
    }

    pub fn stop(&mut self) {
        if self.is_running() {
            info!("App stopped");
        }
        *self = ActiveApp::None;
    }
}

pub struct ClockApp {
    sync_unix: i64,
    sync_instant: Instant,
    timezones: heapless::Vec<TzEntry, MAX_TZ>,
    last_minute: i32,
    needs_static: bool,
    prev_texts: [String<16>; MAX_TEXT_SLOTS],
    thai_numerals: bool,
    /// Upcoming event to display below the date. `time` empty = no event.
    event_time: String<8>,
    event_label: String<24>,
    /// Which date layout was used last tick — detects none↔event transitions
    /// so we can force a static redraw and erase the old date position.
    prev_event_shown: bool,
}

impl ClockApp {
    pub fn new(unix_ts: i64, timezones: heapless::Vec<TzEntry, MAX_TZ>, thai_numerals: bool) -> Self {
        info!(
            "Clock started: unix={}, {} timezones, thai={}",
            unix_ts,
            timezones.len(),
            thai_numerals,
        );
        ClockApp {
            sync_unix: unix_ts,
            sync_instant: Instant::now(),
            timezones,
            last_minute: -1,
            needs_static: true,
            prev_texts: Default::default(),
            thai_numerals,
            event_time: String::new(),
            event_label: String::new(),
            prev_event_shown: false,
        }
    }

    /// Update the upcoming-event banner.
    /// Empty `time` clears the banner.
    pub fn set_event(&mut self, time: &str, label: &str) {
        self.event_time.clear();
        self.event_label.clear();
        let _ = self.event_time.push_str(&time[..time.len().min(self.event_time.capacity())]);
        let _ = self.event_label.push_str(&label[..label.len().min(self.event_label.capacity())]);
        // Force an immediate redraw — the layout may need to flip between
        // normal-date and compact-date + event.
        self.last_minute = -1;
    }

    fn current_unix(&self) -> i64 {
        let elapsed = Instant::now() - self.sync_instant;
        self.sync_unix + elapsed.as_secs() as i64
    }

    /// Tick the clock. Returns duration until next tick.
    pub fn tick(
        &mut self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        bitmaps: &BitmapStore,
    ) -> Duration {
        let now_unix = self.current_unix();

        // Use first timezone for minute-change detection
        let main_tz = self.timezones.first().map(|t| t.offset_minutes).unwrap_or(0);
        let main_dt = time_util::unix_to_datetime(now_unix, main_tz);
        let current_minute = main_dt.hour as i32 * 60 + main_dt.minute as i32;

        if current_minute == self.last_minute && !self.needs_static {
            return Duration::from_secs(1);
        }

        if self.needs_static {
            self.draw_static(shadow, fb, bitmaps);
            self.needs_static = false;
            // Reset prev_texts to force full text redraw
            for t in self.prev_texts.iter_mut() {
                t.clear();
            }
        }

        self.draw_times(shadow, fb, now_unix);
        self.last_minute = current_minute;

        Duration::from_secs(1)
    }

    fn draw_static(
        &self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        bitmaps: &BitmapStore,
    ) {
        shadow.clear(fb, 0, 0, 0);

        // Main flag: try uploaded bitmap, then built-in, then text
        if self.timezones.len() > 0 {
            let tz = &self.timezones[0];
            let mut flag_name = String::<16>::new();
            let _ = core::fmt::write(&mut flag_name, format_args!("fl_{}", tz.name));
            let name_lower: String<16> = lowercase(&flag_name);
            if !bitmaps.show(shadow, fb, &name_lower, MAIN_FLAG_X, MAIN_FLAG_Y) {
                // No uploaded bitmap — try built-in flag
                if let Some(flag) = get_builtin_flag(&tz.name) {
                    draw_embedded_flag(shadow, fb, flag, MAIN_FLAG_X, MAIN_FLAG_Y);
                } else {
                    // No flag at all — render TZ name as text
                    font::render_text(
                        shadow, fb,
                        MAIN_FLAG_X as i32, MAIN_FLAG_Y as i32 + 4,
                        0x88, 0x88, 0x88, 1, &tz.name,
                    );
                }
            }
        }

        // Side clocks layout (top to bottom):
        //   Slot 0 (tz[1]): GMT label (no flag) — for UTC
        //   Slot 1 (tz[2]): ES flag — for Madrid
        //   Slot 2 (tz[3]): RU flag — for Moscow

        // GMT label at top of side panel
        font::render_text(
            shadow, fb,
            SLOT_GMT_LABEL.x, SLOT_GMT_LABEL.y,
            SLOT_GMT_LABEL.r, SLOT_GMT_LABEL.g, SLOT_GMT_LABEL.b,
            SLOT_GMT_LABEL.scale,
            "GMT",
        );

        // Side flags for tz[2] and tz[3] (skip tz[1] which is UTC/GMT with text label)
        for (i, tz_idx) in [2usize, 3].iter().enumerate() {
            if *tz_idx >= self.timezones.len() {
                break;
            }
            if i >= SIDE_FLAG_POS.len() {
                break;
            }
            let tz = &self.timezones[*tz_idx];
            let mut flag_name = String::<16>::new();
            let _ = core::fmt::write(&mut flag_name, format_args!("fl_{}", tz.name));
            let name_lower: String<16> = lowercase(&flag_name);
            let (fx, fy) = SIDE_FLAG_POS[i];
            if !bitmaps.show(shadow, fb, &name_lower, fx, fy) {
                // Try built-in flag
                if let Some(flag) = get_builtin_flag(&tz.name) {
                    draw_embedded_flag(shadow, fb, flag, fx, fy);
                } else {
                    // Text fallback
                    font::render_text(
                        shadow, fb,
                        fx as i32, fy as i32 + 4,
                        0x88, 0x88, 0x88, 1, &tz.name,
                    );
                }
            }
        }
    }

    fn draw_times(
        &mut self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        now_unix: i64,
    ) {
        // Copy timezone offsets to avoid borrow conflict with self.update_slot
        let mut offsets = [0i16; MAX_TZ];
        let tz_count = self.timezones.len();
        for (i, tz) in self.timezones.iter().enumerate() {
            offsets[i] = tz.offset_minutes;
        }

        let event_shown = !self.event_time.is_empty();

        // If the event presence changed since last tick, the old date was
        // drawn at the other Y. Erase it before drawing at the new Y so we
        // don't leave ghost pixels.
        if event_shown != self.prev_event_shown {
            let old_slot = if self.prev_event_shown { &SLOT_DATE_COMPACT } else { &SLOT_DATE };
            let old = &self.prev_texts[SLOT_IDX_DATE];
            if !old.is_empty() {
                font::render_text(
                    shadow, fb, old_slot.x, old_slot.y,
                    0, 0, 0, old_slot.scale, old.as_str(),
                );
            }
            // Force the date slot to re-render at the new position.
            self.prev_texts[SLOT_IDX_DATE].clear();
        }

        // Slot 0: main clock HH:MM (or Thai numerals if enabled)
        if tz_count > 0 {
            let dt = time_util::unix_to_datetime(now_unix, offsets[0]);
            if self.thai_numerals {
                let text = time_util::format_hhmm_thai(&dt);
                self.update_slot(shadow, fb, 0, &SLOT_MAIN, text.as_str());
            } else {
                let text = time_util::format_hhmm(&dt);
                self.update_slot(shadow, fb, 0, &SLOT_MAIN, text.as_str());
            }

            // Slot 1: date — choose Y based on whether the event banner is active.
            let date_text = time_util::format_date(&dt);
            let date_slot = if event_shown { &SLOT_DATE_COMPACT } else { &SLOT_DATE };
            self.update_slot(shadow, fb, SLOT_IDX_DATE, date_slot, &date_text);
        }

        // Slots 2..4: side clocks
        for i in 0..SIDE_SLOTS.len().min(tz_count.saturating_sub(1)) {
            let dt = time_util::unix_to_datetime(now_unix, offsets[i + 1]);
            let text = time_util::format_hhmm(&dt);
            self.update_slot(shadow, fb, 2 + i, &SIDE_SLOTS[i], &text);
        }

        // Slot 5: upcoming-event banner.
        // Build "HH:MM label" (or just "HH:MM" if no label). Format in a
        // small heapless buffer to avoid allocation.
        if event_shown {
            let mut line = String::<32>::new();
            let _ = line.push_str(self.event_time.as_str());
            if !self.event_label.is_empty() {
                let _ = line.push(' ');
                let _ = line.push_str(self.event_label.as_str());
            }
            // Truncate to 16 chars for the slot's prev_texts buffer.
            let fit = {
                let mut end = line.len().min(16);
                while end > 0 && !line.is_char_boundary(end) {
                    end -= 1;
                }
                end
            };
            self.update_slot(shadow, fb, SLOT_IDX_EVENT, &SLOT_EVENT, &line[..fit]);
        } else {
            // Clear event slot if it had anything before.
            if !self.prev_texts[SLOT_IDX_EVENT].is_empty() {
                self.update_slot(shadow, fb, SLOT_IDX_EVENT, &SLOT_EVENT, "");
            }
        }

        self.prev_event_shown = event_shown;
    }

    fn update_slot(
        &mut self,
        shadow: &mut ShadowBuffer,
        fb: &mut FBType,
        slot_idx: usize,
        slot: &TextSlot,
        new_text: &str,
    ) {
        if slot_idx >= MAX_TEXT_SLOTS {
            return;
        }

        let old = &self.prev_texts[slot_idx];
        if old.as_str() == new_text {
            return; // unchanged
        }

        // Erase old text in black
        if !old.is_empty() {
            font::render_text(shadow, fb, slot.x, slot.y, 0, 0, 0, slot.scale, old.as_str());
        }

        // Draw new text
        font::render_text(
            shadow, fb, slot.x, slot.y, slot.r, slot.g, slot.b, slot.scale, new_text,
        );

        // Track
        self.prev_texts[slot_idx].clear();
        let _ = self.prev_texts[slot_idx].push_str(new_text);
    }
}

fn lowercase(s: &str) -> String<16> {
    let mut out = String::<16>::new();
    for c in s.chars() {
        let _ = out.push(if c.is_ascii_uppercase() {
            (c as u8 + 32) as char
        } else {
            c
        });
    }
    out
}
