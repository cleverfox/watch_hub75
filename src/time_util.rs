/// Unix timestamp → civil time conversion (no_std, no dependencies)

use heapless::String;

const DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAY_NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(month: u8, year: i32) -> u8 {
    if month == 2 && is_leap_year(year) {
        29
    } else {
        DAYS_IN_MONTH[month as usize - 1]
    }
}

#[derive(Clone, Copy)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,  // 1-12
    pub day: u8,    // 1-31
    pub hour: u8,   // 0-23
    pub minute: u8, // 0-59
    pub second: u8, // 0-59
    pub weekday: u8, // 0=Mon, 1=Tue, ..., 6=Sun
}

/// Convert unix timestamp (seconds since 1970-01-01) + timezone offset to DateTime
pub fn unix_to_datetime(unix_secs: i64, offset_minutes: i16) -> DateTime {
    let total_secs = unix_secs + offset_minutes as i64 * 60;

    let day_secs = total_secs.rem_euclid(86400) as u32;
    let hour = (day_secs / 3600) as u8;
    let minute = ((day_secs % 3600) / 60) as u8;
    let second = (day_secs % 60) as u8;

    // Days since epoch (1970-01-01 was a Thursday = day 0)
    let mut days = (total_secs.div_euclid(86400)) as i32;
    // Weekday: 1970-01-01 was Thursday. 0=Thu,1=Fri,...,3=Sun,4=Mon,...,6=Wed
    // We want 0=Mon: Thursday = day 3
    let weekday = ((days.rem_euclid(7) + 3) % 7) as u8;

    // Convert days since epoch to year/month/day
    let mut year = 1970;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let mut month: u8 = 1;
    loop {
        let dim = days_in_month(month, year) as i32;
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    let day = days as u8 + 1;

    DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        weekday,
    }
}

/// Format as "HH:MM"
pub fn format_hhmm(dt: &DateTime) -> String<8> {
    let mut s = String::new();
    let _ = core::fmt::write(&mut s, format_args!("{:02}:{:02}", dt.hour, dt.minute));
    s
}

/// Format as "Tue 25 Mar"
pub fn format_date(dt: &DateTime) -> String<16> {
    let mut s = String::new();
    let day_name = DAY_NAMES[dt.weekday as usize % 7];
    let month_name = MONTH_NAMES[(dt.month as usize).saturating_sub(1) % 12];
    let _ = core::fmt::write(&mut s, format_args!("{} {:2} {}", day_name, dt.day, month_name));
    s
}
