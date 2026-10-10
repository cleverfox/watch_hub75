/// On-device apps that run independently of TCP client connection.

use embassy_time::{Duration, Instant};
use heapless::String;
use log::info;

use crate::bitmap_store::BitmapStore;
use crate::font;
use crate::framebuffer::{FBType, ShadowBuffer};
use crate::time_util;

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
const SLOT_DATE: TextSlot = TextSlot { x: 4, y: 52, r: 0x66, g: 0x66, b: 0x66, scale: 1 };
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
/// Number of tracked text slots: main + date + gmt_label + up to 3 side clocks = 6
const MAX_TEXT_SLOTS: usize = 6;

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
}

impl ClockApp {
    pub fn new(unix_ts: i64, timezones: heapless::Vec<TzEntry, MAX_TZ>) -> Self {
        info!(
            "Clock started: unix={}, {} timezones",
            unix_ts,
            timezones.len()
        );
        ClockApp {
            sync_unix: unix_ts,
            sync_instant: Instant::now(),
            timezones,
            last_minute: -1,
            needs_static: true,
            prev_texts: Default::default(),
        }
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

        // Main flag: fl_<name> centered in left section
        if self.timezones.len() > 0 {
            let mut flag_name = String::<16>::new();
            let _ = core::fmt::write(&mut flag_name, format_args!("fl_{}", self.timezones[0].name));
            let name_lower: String<16> = lowercase(&flag_name);
            bitmaps.show(shadow, fb, &name_lower, MAIN_FLAG_X, MAIN_FLAG_Y);
        }

        // Side clocks layout (top to bottom):
        //   Slot 0 (tz[1]): GMT label (no flag) — for UTC
        //   Slot 1 (tz[2]): ES flag — for Madrid
        //   Slot 2 (tz[3]): RU flag — for Moscow
        //
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
            let mut flag_name = String::<16>::new();
            let _ = core::fmt::write(&mut flag_name, format_args!("fl_{}", self.timezones[*tz_idx].name));
            let name_lower: String<16> = lowercase(&flag_name);
            let (fx, fy) = SIDE_FLAG_POS[i];
            bitmaps.show(shadow, fb, &name_lower, fx, fy);
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

        // Slot 0: main clock HH:MM
        if tz_count > 0 {
            let dt = time_util::unix_to_datetime(now_unix, offsets[0]);
            let text = time_util::format_hhmm(&dt);
            self.update_slot(shadow, fb, 0, &SLOT_MAIN, &text);

            // Slot 1: date
            let date_text = time_util::format_date(&dt);
            self.update_slot(shadow, fb, 1, &SLOT_DATE, &date_text);
        }

        // Slots 2..4: side clocks
        for i in 0..SIDE_SLOTS.len().min(tz_count.saturating_sub(1)) {
            let dt = time_util::unix_to_datetime(now_unix, offsets[i + 1]);
            let text = time_util::format_hhmm(&dt);
            self.update_slot(shadow, fb, 2 + i, &SIDE_SLOTS[i], &text);
        }
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
