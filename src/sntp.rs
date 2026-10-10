/// Minimal SNTP client — sends one request, returns unix timestamp.
///
/// SNTP v4 protocol: send 48-byte request to port 123, read 48-byte response.
/// Timestamp at bytes 40-43 is seconds since 1900-01-01.
/// Subtract NTP_EPOCH_OFFSET to get unix epoch.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_net::udp::UdpSocket;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use log::info;

const NTP_PORT: u16 = 123;
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800; // seconds between 1900 and 1970
const NTP_TIMEOUT: Duration = Duration::from_secs(5);
const SYNC_RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// Reboot if no time was obtained within this window after boot
/// (e.g. router came up after us following a power outage).
const SYNC_DEADLINE: Duration = Duration::from_secs(10 * 60);

/// Time obtained by `ntp_sync_task`: (unix_secs, instant it was received).
pub static NTP_TIME: Signal<CriticalSectionRawMutex, (i64, Instant)> = Signal::new();
/// Set once a TCP client has sent a command — it owns the display from then on,
/// so the NTP auto-clock and the no-time reboot are suppressed.
pub static CLIENT_CONFIGURED: AtomicBool = AtomicBool::new(false);

/// Keeps retrying SNTP until it succeeds. If no time is obtained within
/// SYNC_DEADLINE and no TCP client took over the display, reboots.
#[embassy_executor::task]
pub async fn ntp_sync_task(stack: Stack<'static>, server_ip: &'static str) {
    let deadline = Instant::now() + SYNC_DEADLINE;
    loop {
        if CLIENT_CONFIGURED.load(Ordering::Relaxed) {
            info!("SNTP: TCP client configured display, stopping sync");
            return;
        }
        if Instant::now() >= deadline {
            info!("SNTP: no time after {}s and no TCP client, rebooting", SYNC_DEADLINE.as_secs());
            Timer::after(Duration::from_millis(100)).await; // let the log flush
            esp_hal::system::software_reset();
        }
        if stack.is_config_up() {
            if let Some(ts) = get_unix_time(stack, server_ip).await {
                NTP_TIME.signal((ts, Instant::now()));
                return;
            }
        }
        Timer::after(SYNC_RETRY_INTERVAL).await;
    }
}

/// Try to get current unix timestamp via SNTP.
/// Returns Some(unix_secs) on success, None on failure.
pub async fn get_unix_time(stack: Stack<'static>, server_ip: &str) -> Option<i64> {
    let ip: embassy_net::Ipv4Address = match parse_ipv4(server_ip) {
        Some(ip) => ip,
        None => {
            info!("SNTP: invalid server IP: {}", server_ip);
            return None;
        }
    };

    let mut rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut rx_buf = [0u8; 64];
    let mut tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut tx_buf = [0u8; 64];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if let Err(e) = socket.bind(0) {
        info!("SNTP: bind failed: {:?}", e);
        return None;
    }

    // Build SNTP request: byte 0 = 0x23 (LI=0, VN=4, Mode=3=client)
    let mut request = [0u8; 48];
    request[0] = 0x23;

    let endpoint = embassy_net::IpEndpoint::new(
        embassy_net::IpAddress::Ipv4(ip),
        NTP_PORT,
    );

    info!("SNTP: querying {}...", server_ip);

    if let Err(e) = socket.send_to(&request, endpoint).await {
        info!("SNTP: send failed: {:?}", e);
        return None;
    }

    // Wait for response with timeout
    let mut response = [0u8; 48];
    match with_timeout(NTP_TIMEOUT, socket.recv_from(&mut response)).await {
        Ok(Ok((_len, _from))) => {
            // Transmit timestamp at bytes 40-43 (big-endian seconds since 1900)
            let ntp_secs = u32::from_be_bytes([
                response[40],
                response[41],
                response[42],
                response[43],
            ]) as u64;

            if ntp_secs < NTP_EPOCH_OFFSET {
                info!("SNTP: invalid timestamp: {}", ntp_secs);
                return None;
            }

            let unix_secs = (ntp_secs - NTP_EPOCH_OFFSET) as i64;
            info!("SNTP: got time: unix={}", unix_secs);
            Some(unix_secs)
        }
        Ok(Err(e)) => {
            info!("SNTP: recv failed: {:?}", e);
            None
        }
        Err(_) => {
            info!("SNTP: timeout");
            None
        }
    }
}

pub fn parse_ipv4(s: &str) -> Option<embassy_net::Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut i = 0;
    for part in s.split('.') {
        if i >= 4 {
            return None;
        }
        octets[i] = part.parse().ok()?;
        i += 1;
    }
    if i == 4 {
        Some(embassy_net::Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
    } else {
        None
    }
}
