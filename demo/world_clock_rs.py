#!/usr/bin/env python3
"""World Clock adapted for Rust TCPScreen server.

Rust server uses 8px-wide Unifont for all text (no terminalio.FONT).
Layout recalculated for 8px character width.

Layout (128x64):
  Left side (~80px):           Right side (~48px):
  ┌──────────────────┐        ┌──────────────┐
  │    [TH flag]     │        │ [RU] 11:34   │
  │    15:34         │        │ [ES] 09:34   │
  │  Tue 25 Mar      │        │ GMT  08:34   │
  └──────────────────┘        └──────────────┘

Usage:
    python world_clock_rs.py [display-ip] [port]
"""

import sys
import time
from datetime import datetime, timezone, timedelta
from ledclient import LEDClient

WIDTH = 128
HEIGHT = 64

# Font metrics for Rust server (Unifont BDF)
CHAR_W = 8   # 8px per char at scale 1
CHAR_H = 16  # 16px cell height

# Layout: main clock on left, extra clocks on right
SIDE_X = 86   # right section start (128 - 40 = 88 for "HH:MM")

def is_madrid_dst(dt_utc):
    year = dt_utc.year
    mar31 = datetime(year, 3, 31, tzinfo=timezone.utc)
    dst_start = mar31 - timedelta(days=(mar31.weekday() + 1) % 7)
    dst_start = dst_start.replace(hour=1)
    oct31 = datetime(year, 10, 31, tzinfo=timezone.utc)
    dst_end = oct31 - timedelta(days=(oct31.weekday() + 1) % 7)
    dst_end = dst_end.replace(hour=1)
    return dst_start <= dt_utc < dst_end

def get_times():
    now_utc = datetime.now(timezone.utc)
    bangkok = now_utc + timedelta(hours=7)
    moscow = now_utc + timedelta(hours=3)
    madrid_offset = 2 if is_madrid_dst(now_utc) else 1
    madrid = now_utc + timedelta(hours=madrid_offset)
    return {
        "bangkok": bangkok,
        "moscow": moscow,
        "madrid": madrid,
        "utc": now_utc,
    }

# ---- Mini flag bitmaps (10x7) ----
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

# ---- Rendering ----
# With 8px font:
#   Scale 2: "HH:MM" = 5 chars * 16px = 80px, centered in 80px: x=0
#   Scale 1: "Tue 25 Mar" = 10 chars * 8px = 80px
#   Side clocks: "HH:MM" = 5 * 8 = 40px, fits in 48px

def render_static(client):
    """Draw elements that don't change: flags, background."""
    client.clear("000000")
    client.clear_text()

    # Thai flag centered in main section
    # Main section is 0..SIDE_X, flag is 10x7
    flag_x = (SIDE_X - 10) // 2
    client.show("fl_th", flag_x, 2)

    # Side flags: left-aligned, above each time
    client.show("fl_ru", SIDE_X, 1)
    client.show("fl_es", SIDE_X, 21)
    # GMT label as static text (drawn once, not a flag)
    client.text(SIDE_X, 47, "4488FF", 1, "GMT")


# Text slots: (x, y, color, scale)
# y = vertical center of text (Rust server treats y as center)
# Right side layout (3 rows in 64px):
#   Row 1: y=0..20  — RU flag at y=1, time at y=13
#   Row 2: y=20..40 — ES flag at y=21, time at y=33
#   Row 3: y=40..64 — GMT + time at y=52
TEXT_SLOTS = {
    "main":   (4, 26, "00FFAA", 2),           # Big clock, scale 2
    "date":   (4, 52, "666666", 1),            # Date below clock
    "moscow": (SIDE_X, 15, "FFFFFF", 1),       # Below RU flag
    "madrid": (SIDE_X, 35, "FFFFFF", 1),       # Below ES flag
    #"gmt":    (SIDE_X, 51, "4488FF", 1),       # GMT label
    "utc":    (SIDE_X, 58, "FFFFFF", 1),  # UTC time after "GMT" (3*8=24)
}

# Track previous text for each slot
prev_text = {}

def update_slot(client, slot, new_text):
    """Erase old text in black, draw new text in color."""
    x, y, color, scale = TEXT_SLOTS[slot]
    old = prev_text.get(slot)
    if old == new_text:
        return
    if old is not None:
        client.text(x, y, "000000", scale, old)
    client.text(x, y, color, scale, new_text)
    prev_text[slot] = new_text

def render_all_text(client, times):
    """Update all text slots — only changed ones get redrawn."""
    bkk = times["bangkok"]
    update_slot(client, "main", bkk.strftime("%H:%M"))
    update_slot(client, "date", bkk.strftime("%a %d %b"))
    update_slot(client, "moscow", times["moscow"].strftime("%H:%M"))
    update_slot(client, "madrid", times["madrid"].strftime("%H:%M"))
    #update_slot(client, "gmt", "GMT")
    update_slot(client, "utc", times["utc"].strftime("%H:%M"))


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "192.168.1.110"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 1337

    client = LEDClient(host, port)
    client.connect()
    client.ping()
    print(f"Connected to {host}:{port}")

    # Upload mini flags
    print("Uploading mini flags...")
    mini_flags = [
        ("fl_th", draw_thai_mini),
        ("fl_ru", draw_russian_mini),
        ("fl_es", draw_spanish_mini),
    ]
    for name, draw_func in mini_flags:
        w, h, rgb = make_flag_rgb(draw_func)
        resp = client.upload_rgb(name, w, h, rgb)
        print(f"  {name}: {resp}")
        time.sleep(0.2)

    print("Starting clock (Ctrl+C to stop)...")

    need_static = True
    last_minute = -1
    last_ping = time.time()
    PING_INTERVAL = 10

    try:
        while True:
            times = get_times()
            bkk = times["bangkok"]
            current_minute = bkk.minute

            try:
                if need_static:
                    render_static(client)
                    prev_text.clear()
                    need_static = False
                    last_minute = -1
                    last_ping = time.time()

                if current_minute != last_minute:
                    render_all_text(client, times)
                    last_minute = current_minute
                    last_ping = time.time()
                    print(f"\n  [updated display at :{current_minute:02d}]")
                elif time.time() - last_ping >= PING_INTERVAL:
                    client.ping()
                    last_ping = time.time()
            except (BrokenPipeError, ConnectionError, TimeoutError, OSError) as e:
                print(f"\nConnection lost: {e}, reconnecting...")
                client.close()
                time.sleep(1)
                client = LEDClient(host, port)
                client.connect()
                client.ping()
                for name, draw_func in mini_flags:
                    w, h, rgb = make_flag_rgb(draw_func)
                    client.upload_rgb(name, w, h, rgb)
                    time.sleep(0.2)
                print("Reconnected")
                need_static = True
                last_ping = time.time()
                continue

            bkk_s = bkk.strftime("%H:%M")
            msk = times["moscow"].strftime("%H:%M")
            mad = times["madrid"].strftime("%H:%M")
            utc = times["utc"].strftime("%H:%M")
            print(f"\r  BKK {bkk_s}  MSK {msk}  MAD {mad}  UTC {utc}", end="", flush=True)

            time.sleep(1)

    except KeyboardInterrupt:
        print("\nStopped")
    finally:
        client.close()


if __name__ == "__main__":
    main()
