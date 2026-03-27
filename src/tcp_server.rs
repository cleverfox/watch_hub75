/// TCP server task — runs on Core 0 alongside WiFi

extern crate alloc;
use alloc::vec::Vec;

use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read, Write};
use heapless::Vec as HVec;
use log::info;

use crate::bitmap_store::BitmapStore;
use crate::framebuffer::{FBType, ShadowBuffer};
use crate::protocol::{self, CmdResult};
use crate::text_overlay::TextOverlayManager;

const PORT: u16 = 1337;

/// Main TCP server loop
pub async fn tcp_server_task(
    stack: Stack<'static>,
    shadow: &'static mut ShadowBuffer,
    fb: &'static mut FBType,
) {
    let mut bitmaps = BitmapStore::new();
    let mut text_mgr = TextOverlayManager::new();

    info!("TCP server listening on port {}", PORT);

    loop {
        let mut rx_buffer = [0u8; 4096];
        let mut tx_buffer = [0u8; 512];

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(60)));

        if let Err(e) = socket.accept(PORT).await {
            info!("Accept error: {:?}", e);
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }

        info!("Client connected");
        let mut first_cmd = true;

        handle_client(&mut socket, shadow, fb, &mut bitmaps, &mut text_mgr, &mut first_cmd).await;

        info!("Client disconnected");
        socket.close();
    }
}

async fn handle_client(
    socket: &mut TcpSocket<'_>,
    shadow: &mut ShadowBuffer,
    fb: &mut FBType,
    bitmaps: &mut BitmapStore,
    text_mgr: &mut TextOverlayManager,
    first_cmd: &mut bool,
) {
    let mut line_buf: HVec<u8, 512> = HVec::new();
    let mut read_buf = [0u8; 2048];

    loop {
        match socket.read(&mut read_buf).await {
            Ok(0) => {
                info!("Client sent EOF");
                break;
            }
            Ok(n) => {
                let mut pos = 0;
                while pos < n {
                    if let Some(nl) = read_buf[pos..n].iter().position(|&b| b == b'\n') {
                        let _ = line_buf.extend_from_slice(&read_buf[pos..pos + nl]);
                        pos += nl + 1;

                        if let Ok(line) = core::str::from_utf8(&line_buf) {
                            let line = line.trim();
                            if !line.is_empty() {
                                // Log command (truncate long ones)
                                if line.len() <= 60 {
                                    info!("CMD: {}", line);
                                } else {
                                    info!("CMD: {}... ({}b)", &line[..40], line.len());
                                }

                                if *first_cmd {
                                    text_mgr.clear_all(shadow, fb);
                                    *first_cmd = false;
                                }

                                match protocol::process_command(line, shadow, fb, bitmaps, text_mgr) {
                                    CmdResult::None => {}
                                    CmdResult::Response(resp) => {
                                        info!("RSP: {}", resp.trim_end());
                                        let _ = socket.write_all(resp.as_bytes()).await;
                                    }
                                    CmdResult::NeedUpload { name, width, height, total_bytes } => {
                                        info!("UPLOAD: {} {}x{} = {} bytes", name, width, height, total_bytes);
                                        let remaining = &read_buf[pos..n];
                                        info!("  remaining in buffer: {} bytes", remaining.len());
                                        match read_upload_data(socket, remaining, total_bytes).await {
                                            Some((data, leftover)) => {
                                                info!("  received {} bytes, leftover {}", data.len(), leftover.len());
                                                let resp = protocol::complete_upload(bitmaps, &name, width, height, data);
                                                info!("  RSP: {}", resp.trim_end());
                                                let _ = socket.write_all(resp.as_bytes()).await;
                                                pos = n;
                                            }
                                            None => {
                                                info!("  UPLOAD failed (timeout/disconnect)");
                                                return;
                                            }
                                        }
                                    }
                                    CmdResult::Wait(ms) => {
                                        info!("WAIT {}ms", ms);
                                        Timer::after(Duration::from_millis(ms as u64)).await;
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
            Err(e) => {
                info!("Socket read error: {:?}", e);
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
    info!("  read_upload_data: need {} bytes, have {} initial", total, initial.len());
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
            Ok(Ok(0)) => {
                info!("  upload: EOF after {} bytes", data.len());
                return None;
            }
            Ok(Ok(n)) => {
                data.extend_from_slice(&read_buf[..n]);
            }
            Ok(Err(e)) => {
                info!("  upload: read error {:?} after {} bytes", e, data.len());
                return None;
            }
            Err(_) => {
                info!("  upload: timeout after {} bytes", data.len());
                return None;
            }
        }
    }

    let leftover = if data.len() > total {
        data[total..].to_vec()
    } else {
        Vec::new()
    };
    data.truncate(total);
    info!("  upload complete: {} bytes received", total);
    Some((data, leftover))
}
