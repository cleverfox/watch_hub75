/// Display coordinator — multiplexes TCP commands and on-device app ticking.

extern crate alloc;
use alloc::vec::Vec;

use embassy_futures::select::{self, Either};
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{Read, Write};
use heapless::Vec as HVec;
use log::info;

use crate::app::{ActiveApp, ClockApp};
use crate::bitmap_store::BitmapStore;
use crate::framebuffer::{FBType, ShadowBuffer};
use crate::protocol::{self, CmdResult};
use crate::netcheck;
use crate::sntp;
use crate::text_overlay::TextOverlayManager;

const PORT: u16 = 1337;

/// Main display coordinator loop — owns shadow/fb, runs TCP + apps
pub async fn display_coordinator(
    stack: Stack<'static>,
    shadow: &'static mut ShadowBuffer,
    fb: &'static mut FBType,
) {
    let mut bitmaps = BitmapStore::new();
    let mut text_mgr = TextOverlayManager::new();
    let mut app = ActiveApp::None;
    // Network status currently drawn over the clock
    let mut net_shown = netcheck::NET_OK;

    info!("Display coordinator started on port {}", PORT);

    loop {
        let mut rx_buffer = [0u8; 4096];
        let mut tx_buffer = [0u8; 512];

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(60)));

        // Outer loop: wait for client OR tick app.
        // A single accept() future is kept alive across ticks: calling accept()
        // again re-runs listen(), which fails with InvalidState while a
        // handshake is in progress (and dropping the socket then RSTs it).
        let accepted = {
            let mut accept_fut = core::pin::pin!(socket.accept(PORT));
            loop {
                match select::select(
                    accept_fut.as_mut(),
                    Timer::after(Duration::from_secs(1)),
                )
                .await
                {
                    Either::First(Ok(())) => break true,
                    Either::First(Err(e)) => {
                        info!("Accept error: {:?}", e);
                        break false;
                    }
                    Either::Second(()) => {
                        // Timer fired — maybe start the NTP clock, tick the app
                        if let Some((unix_ts, at)) = sntp::NTP_TIME.try_take() {
                            if !sntp::CLIENT_CONFIGURED.load(Ordering::Relaxed) && !app.is_running() {
                                let unix_ts = unix_ts + (Instant::now() - at).as_secs() as i64;
                                info!("NTP: auto-starting clock with time {}", unix_ts);
                                app = ActiveApp::Clock(ClockApp::new(
                                    unix_ts, crate::app::default_timezones(), false,
                                ));
                            }
                        }
                        tick_app(&mut app, shadow, fb, &bitmaps, &mut net_shown);
                    }
                }
            }
        };

        if !accepted {
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }

        info!("Client connected");
        let mut first_cmd = true;

        handle_client(
            &mut socket,
            shadow,
            fb,
            &mut bitmaps,
            &mut text_mgr,
            &mut app,
            &mut first_cmd,
            &mut net_shown,
        )
        .await;

        info!("Client disconnected");
        socket.close();
    }
}

/// Tick the app and overlay the network status line while it runs.
fn tick_app(
    app: &mut ActiveApp,
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    bitmaps: &BitmapStore,
    net_shown: &mut u8,
) {
    let status = netcheck::status();
    if let ActiveApp::Clock(clk) = app {
        if status == netcheck::NET_OK && *net_shown != netcheck::NET_OK {
            // Erase the line and redraw texts whose pixels it may have covered
            netcheck::clear_status(shadow, fb);
            clk.invalidate_texts();
        }
    }
    app.tick(shadow, fb, bitmaps);
    netcheck::set_enabled(matches!(app, ActiveApp::Clock(_)));
    // Redrawn every tick: clock text outlines can paint over the bottom row
    if app.is_running() {
        netcheck::draw_status(shadow, fb, status);
    }
    *net_shown = status;
}

async fn handle_client(
    socket: &mut TcpSocket<'_>,
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    bitmaps: &mut BitmapStore,
    text_mgr: &mut TextOverlayManager,
    app: &mut ActiveApp,
    first_cmd: &mut bool,
    net_shown: &mut u8,
) {
    let mut line_buf: HVec<u8, 512> = HVec::new();
    let mut read_buf = [0u8; 2048];

    loop {
        // Select between socket read and app tick
        let read_result = if app.is_running() {
            match select::select(
                socket.read(&mut read_buf),
                Timer::after(Duration::from_secs(1)),
            )
            .await
            {
                Either::First(result) => Some(result),
                Either::Second(()) => {
                    tick_app(app, shadow, fb, bitmaps, net_shown);
                    continue;
                }
            }
        } else {
            Some(socket.read(&mut read_buf).await)
        };

        let n = match read_result {
            Some(Ok(0)) => {
                info!("Client sent EOF");
                break;
            }
            Some(Ok(n)) => n,
            Some(Err(e)) => {
                info!("Socket read error: {:?}", e);
                break;
            }
            None => continue,
        };

        let mut pos = 0;
        while pos < n {
            if let Some(nl) = read_buf[pos..n].iter().position(|&b| b == b'\n') {
                let _ = line_buf.extend_from_slice(&read_buf[pos..pos + nl]);
                pos += nl + 1;

                if let Ok(line) = core::str::from_utf8(&line_buf) {
                    let line = line.trim();
                    if !line.is_empty() {
                        if line.len() <= 60 {
                            info!("CMD: {}", line);
                        } else {
                            info!("CMD: {}... ({}b)", &line[..40], line.len());
                        }

                        if *first_cmd {
                            text_mgr.clear_all(shadow, fb);
                            *first_cmd = false;
                            sntp::CLIENT_CONFIGURED.store(true, Ordering::Relaxed);
                        }

                        // Stop app if drawing command arrives
                        if app.is_running() && protocol::is_drawing_command(line) {
                            app.stop();
                            netcheck::set_enabled(false);
                        }

                        match protocol::process_command(
                            line, shadow, fb, bitmaps, text_mgr,
                        ) {
                            CmdResult::None => {}
                            CmdResult::Response(resp) => {
                                info!("RSP: {}", resp.trim_end());
                                let _ = socket.write_all(resp.as_bytes()).await;
                            }
                            CmdResult::NeedUpload {
                                name,
                                width,
                                height,
                                total_bytes,
                                rle,
                            } => {
                                info!(
                                    "UPLOAD{}: {} {}x{} = {} bytes",
                                    if rle { "RLE" } else { "" },
                                    name, width, height, total_bytes
                                );
                                let remaining = &read_buf[pos..n];
                                match read_upload_data(socket, remaining, total_bytes)
                                    .await
                                {
                                    Some((data, _leftover)) => {
                                        let resp = protocol::complete_upload(
                                            bitmaps, &name, width, height, data, rle,
                                        );
                                        info!("  RSP: {}", resp.trim_end());
                                        let _ =
                                            socket.write_all(resp.as_bytes()).await;
                                        pos = n; // consumed remaining
                                    }
                                    None => {
                                        info!("  UPLOAD failed");
                                        return;
                                    }
                                }
                            }
                            CmdResult::Wait(ms) => {
                                info!("WAIT {}ms", ms);
                                Timer::after(Duration::from_millis(ms as u64)).await;
                            }
                            CmdResult::StartClock { unix_ts, timezones, thai_numerals } => {
                                info!(
                                    "Starting clock app: ts={}, {} tzs, thai={}",
                                    unix_ts,
                                    timezones.len(),
                                    thai_numerals,
                                );
                                *app = ActiveApp::Clock(ClockApp::new(
                                    unix_ts, timezones, thai_numerals,
                                ));
                                // Immediately tick to draw the clock
                                tick_app(app, shadow, fb, bitmaps, net_shown);
                                let mut s = heapless::String::<64>::new();
                                let _ = s.push_str("CLOCK OK\n");
                                let _ = socket.write_all(s.as_bytes()).await;
                            }
                            CmdResult::StopApp => {
                                app.stop();
                                netcheck::set_enabled(false);
                                let mut s = heapless::String::<64>::new();
                                let _ = s.push_str("APPSTOP OK\n");
                                let _ = socket.write_all(s.as_bytes()).await;
                            }
                            CmdResult::SetClockEvent { time, label } => {
                                // Apply to the running clock app if any.
                                match app {
                                    ActiveApp::Clock(clk) => {
                                        clk.set_event(time.as_str(), label.as_str());
                                        let mut s = heapless::String::<64>::new();
                                        if time.is_empty() {
                                            let _ = s.push_str("CLKEVT CLEAR\n");
                                        } else {
                                            let _ = s.push_str("CLKEVT OK\n");
                                        }
                                        let _ = socket.write_all(s.as_bytes()).await;
                                    }
                                    _ => {
                                        let mut s = heapless::String::<64>::new();
                                        let _ = s.push_str("CLKEVT ERR no-clock\n");
                                        let _ = socket.write_all(s.as_bytes()).await;
                                    }
                                }
                            }
                        }
                    }
                }
                line_buf.clear();
            } else {
                let _ = line_buf.extend_from_slice(&read_buf[pos..n]);
                break;
            }
        }
    }
}

/// Read exactly `total` bytes of binary data for UPLOAD command
async fn read_upload_data(
    socket: &mut TcpSocket<'_>,
    initial: &[u8],
    total: usize,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut data = Vec::with_capacity(total + 64);
    data.extend_from_slice(initial);

    let mut read_buf = [0u8; 2048];
    while data.len() < total {
        match embassy_time::with_timeout(
            Duration::from_secs(30),
            socket.read(&mut read_buf),
        )
        .await
        {
            Ok(Ok(0)) => return None,
            Ok(Ok(n)) => data.extend_from_slice(&read_buf[..n]),
            Ok(Err(_)) => return None,
            Err(_) => return None,
        }
    }

    let leftover = if data.len() > total {
        data[total..].to_vec()
    } else {
        Vec::new()
    };
    data.truncate(total);
    Some((data, leftover))
}
