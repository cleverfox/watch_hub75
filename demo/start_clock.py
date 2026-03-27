#!/usr/bin/env python3
"""Start the on-device world clock on the LED matrix.

Uploads flag bitmaps, then sends CLOCK command with current time
and timezone configuration. The ESP32 runs the clock independently
after this script exits.

Usage:
    python start_clock.py [display-ip] [port]
"""

import sys
import time
from ledclient import LEDClient

# Mini flag bitmaps (10x7 pixels)
def make_flag_rgb(draw_func, w=10, h=7):
    buf = bytearray(w * h * 3)
    draw_func(buf, w, h)
    return w, h, bytes(buf)

def _fill(buf, w, x0, y0, fw, fh, r, g, b):
    for y in range(y0, y0 + fh):
        for x in range(x0, x0 + fw):
            off = (y * w + x) * 3
            buf[off] = r
            buf[off + 1] = g
            buf[off + 2] = b

def draw_thai_mini(buf, w, h):
    _fill(buf, w, 0, 0, w, 1, 0xED, 0x1C, 0x24)
    _fill(buf, w, 0, 1, w, 1, 0xFF, 0xFF, 0xFF)
    _fill(buf, w, 0, 2, w, 3, 0x24, 0x1D, 0x4F)
    _fill(buf, w, 0, 5, w, 1, 0xFF, 0xFF, 0xFF)
    _fill(buf, w, 0, 6, w, 1, 0xED, 0x1C, 0x24)

def draw_russian_mini(buf, w, h):
    _fill(buf, w, 0, 0, w, 2, 0xFF, 0xFF, 0xFF)
    _fill(buf, w, 0, 2, w, 3, 0x00, 0x39, 0xA6)
    _fill(buf, w, 0, 5, w, 2, 0xD5, 0x2B, 0x1E)

def draw_spanish_mini(buf, w, h):
    _fill(buf, w, 0, 0, w, 2, 0xAA, 0x15, 0x1B)
    _fill(buf, w, 0, 2, w, 3, 0xF1, 0xBF, 0x00)
    _fill(buf, w, 0, 5, w, 2, 0xAA, 0x15, 0x1B)


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "192.168.1.110"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 1337

    client = LEDClient(host, port)
    client.connect()
    client.ping()
    print(f"Connected to {host}:{port}")

    # Upload flag bitmaps
    # Names must match: fl_<lowercase_tz_name>
    print("Uploading flags...")
    flags = [
        ("fl_bkk", draw_thai_mini),
        ("fl_msk", draw_russian_mini),
        ("fl_mad", draw_spanish_mini),
    ]
    for name, draw_func in flags:
        w, h, rgb = make_flag_rgb(draw_func)
        resp = client.upload_rgb(name, w, h, rgb)
        print(f"  {name}: {resp}")
        time.sleep(0.2)

    # Send CLOCK command: unix_timestamp TZ_NAME OFFSET_MINUTES ...
    # Order: main clock first, then side clocks top-to-bottom (UTC, MAD, MSK)
    unix_ts = int(time.time())
    clock_cmd = f"CLOCK {unix_ts} BKK 420 UTC 0 MAD 120 MSK 180"
    print(f"Sending: {clock_cmd}")

    # Use _send_recv to get the response
    client._send(clock_cmd)
    # Read response
    resp = b""
    try:
        while True:
            chunk = client.sock.recv(256)
            if not chunk:
                break
            resp += chunk
            if b"\n" in resp:
                break
    except Exception:
        pass

    print(f"Response: {resp.decode().strip()}")
    print("Clock started on device. You can disconnect now.")

    client.close()


if __name__ == "__main__":
    main()
