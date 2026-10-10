/// Periodic connectivity check via ICMP ping.
///
/// Pings EXT_HOST (default 8.8.8.8). If it fails, pings INT_HOST (default: the
/// DHCP gateway) to tell "no internet" from "no LAN". The result is published
/// in NET_STATUS and drawn as a line at the bottom of the clock.
///
/// Hosts can be overridden (or the check disabled) at runtime via the
/// NETCHECK TCP command; overrides are not persisted across reboots.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use embassy_net::icmp::ping::{PingManager, PingParams};
use embassy_net::icmp::PacketMetadata;
use embassy_futures::select::select;
use embassy_net::{Ipv4Address, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use log::info;

use crate::framebuffer::{FBType, ShadowBuffer, COLS, ROWS};
use crate::sntp::parse_ipv4;

pub const NET_OK: u8 = 0;
/// Internal host reachable, external not — internet is down.
pub const NET_LAN_ONLY: u8 = 1;
/// Nothing reachable (or no IP at all).
pub const NET_DOWN: u8 = 2;

/// Starts as OK so no line is shown before the first check completes.
pub static NET_STATUS: AtomicU8 = AtomicU8::new(NET_OK);

/// Checks run only while the clock app is running.
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Runtime config from the NETCHECK command. Host value 0 = use default.
static USER_DISABLED: AtomicBool = AtomicBool::new(false);
static EXT_OVERRIDE: AtomicU32 = AtomicU32::new(0);
static INT_OVERRIDE: AtomicU32 = AtomicU32::new(0);
/// Wakes the task to re-check immediately after a config change.
static CONFIG_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Override hosts and enable checking. `int = None` → INT_HOST / DHCP gateway.
pub fn set_hosts(ext: Ipv4Address, int: Option<Ipv4Address>) {
    EXT_OVERRIDE.store(ext.to_bits(), Ordering::Relaxed);
    INT_OVERRIDE.store(int.map_or(0, |ip| ip.to_bits()), Ordering::Relaxed);
    USER_DISABLED.store(false, Ordering::Relaxed);
    CONFIG_CHANGED.signal(());
}

/// Drop overrides and enable checking with the compiled-in defaults.
pub fn reset() {
    EXT_OVERRIDE.store(0, Ordering::Relaxed);
    INT_OVERRIDE.store(0, Ordering::Relaxed);
    USER_DISABLED.store(false, Ordering::Relaxed);
    CONFIG_CHANGED.signal(());
}

pub fn disable() {
    USER_DISABLED.store(true, Ordering::Relaxed);
    CONFIG_CHANGED.signal(());
}

pub fn is_disabled() -> bool {
    USER_DISABLED.load(Ordering::Relaxed)
}

fn load_override(v: &AtomicU32) -> Option<Ipv4Address> {
    match v.load(Ordering::Relaxed) {
        0 => None,
        bits => Some(Ipv4Address::from_bits(bits)),
    }
}

pub fn ext_override() -> Option<Ipv4Address> {
    load_override(&EXT_OVERRIDE)
}

pub fn int_override() -> Option<Ipv4Address> {
    load_override(&INT_OVERRIDE)
}

const CHECK_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// Pings per host before declaring it unreachable (tolerates a lost packet).
const PING_ATTEMPTS: u8 = 3;
const DEFAULT_EXT_HOST: &str = "8.8.8.8";

const LINE_HEIGHT: usize = 1;

pub fn status() -> u8 {
    NET_STATUS.load(Ordering::Relaxed)
}

/// Draw the status line at the bottom of the screen if the network is not OK.
pub fn draw_status(shadow: &mut ShadowBuffer, fb: &mut FBType, status: u8) {
    let (r, g, b) = match status {
        NET_LAN_ONLY => (0xFF, 0x60, 0x00), // orange
        NET_DOWN => (0xFF, 0x00, 0x00),     // red
        _ => return,
    };
    shadow.fill_rect(fb, 0, ROWS - LINE_HEIGHT, COLS, LINE_HEIGHT, r, g, b);
}

/// Erase the status line (paint black).
pub fn clear_status(shadow: &mut ShadowBuffer, fb: &mut FBType) {
    shadow.fill_rect(fb, 0, ROWS - LINE_HEIGHT, COLS, LINE_HEIGHT, 0, 0, 0);
}

fn host_from_env(name: &str, value: Option<&str>) -> Option<Ipv4Address> {
    let value = value?;
    let ip = parse_ipv4(value);
    if ip.is_none() {
        info!("NETCHECK: invalid {}: {}", name, value);
    }
    ip
}

async fn reachable(pm: &mut PingManager<'_>, host: Ipv4Address) -> bool {
    let mut params = PingParams::new(host);
    params.set_count(1).set_timeout(PING_TIMEOUT);
    for _ in 0..PING_ATTEMPTS {
        if pm.ping(&params).await.is_ok() {
            return true;
        }
    }
    false
}

#[embassy_executor::task]
pub async fn netcheck_task(stack: Stack<'static>) {
    let ext_host = host_from_env("EXT_HOST", option_env!("EXT_HOST"))
        .or_else(|| parse_ipv4(DEFAULT_EXT_HOST))
        .unwrap();
    let int_host_env = host_from_env("INT_HOST", option_env!("INT_HOST"));

    let mut rx_meta = [PacketMetadata::EMPTY; 1];
    let mut rx_buf = [0u8; 128];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx_buf = [0u8; 128];
    let mut pm = PingManager::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);

    loop {
        if !ENABLED.load(Ordering::Relaxed) || is_disabled() {
            // Reset so a stale result isn't shown when the clock starts again
            NET_STATUS.store(NET_OK, Ordering::Relaxed);
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }

        let ext = ext_override().unwrap_or(ext_host);
        let status = if !stack.is_config_up() {
            NET_DOWN
        } else if reachable(&mut pm, ext).await {
            NET_OK
        } else {
            let int_host = int_override()
                .or(int_host_env)
                .or_else(|| stack.config_v4().and_then(|c| c.gateway));
            match int_host {
                Some(h) if reachable(&mut pm, h).await => NET_LAN_ONLY,
                _ => NET_DOWN,
            }
        };

        let prev = NET_STATUS.swap(status, Ordering::Relaxed);
        if prev != status {
            info!(
                "NETCHECK: {}",
                match status {
                    NET_OK => "internet OK",
                    NET_LAN_ONLY => "internet down, LAN OK",
                    _ => "network down",
                }
            );
        }

        select(Timer::after(CHECK_INTERVAL), CONFIG_CHANGED.wait()).await;
    }
}
