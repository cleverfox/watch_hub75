#!/usr/bin/env python3
"""Flag demo over TCP - uploads flag bitmaps once, then loops with SHOW/WAIT."""

import sys
import struct
from ledclient import LEDClient

WIDTH = 128
HEIGHT = 64


def make_rgb_data(draw_func):
    """Create raw RGB bytes (w*h*3) by calling draw_func(buf, w, h)."""
    buf = bytearray(WIDTH * HEIGHT * 3)
    draw_func(buf)
    return bytes(buf)


def set_pixel(buf, x, y, r, g, b):
    if 0 <= x < WIDTH and 0 <= y < HEIGHT:
        off = (y * WIDTH + x) * 3
        buf[off] = r
        buf[off + 1] = g
        buf[off + 2] = b


def fill_rect(buf, x0, y0, w, h, r, g, b):
    for y in range(max(0, y0), min(y0 + h, HEIGHT)):
        for x in range(max(0, x0), min(x0 + w, WIDTH)):
            off = (y * WIDTH + x) * 3
            buf[off] = r
            buf[off + 1] = g
            buf[off + 2] = b


def draw_uk(buf):
    # Blue background
    fill_rect(buf, 0, 0, WIDTH, HEIGHT, 0x01, 0x21, 0x69)

    # White diagonals
    for i in range(WIDTH):
        yc = (i * HEIGHT) // WIDTH
        for t in range(-5, 6):
            set_pixel(buf, i, yc + t, 0xFF, 0xFF, 0xFF)
            set_pixel(buf, WIDTH - 1 - i, yc + t, 0xFF, 0xFF, 0xFF)

    # Red diagonals
    for i in range(WIDTH):
        yc = (i * HEIGHT) // WIDTH
        for t in range(-2, 3):
            set_pixel(buf, i, yc + t, 0xC8, 0x10, 0x2E)
            set_pixel(buf, WIDTH - 1 - i, yc + t, 0xC8, 0x10, 0x2E)

    # White cross
    fill_rect(buf, 0, HEIGHT // 2 - 7, WIDTH, 14, 0xFF, 0xFF, 0xFF)
    fill_rect(buf, WIDTH // 2 - 10, 0, 20, HEIGHT, 0xFF, 0xFF, 0xFF)

    # Red cross
    fill_rect(buf, 0, HEIGHT // 2 - 4, WIDTH, 8, 0xC8, 0x10, 0x2E)
    fill_rect(buf, WIDTH // 2 - 6, 0, 12, HEIGHT, 0xC8, 0x10, 0x2E)


def draw_spain(buf):
    fill_rect(buf, 0, 0, WIDTH, 16, 0xAA, 0x15, 0x1B)
    fill_rect(buf, 0, 16, WIDTH, 32, 0xF1, 0xBF, 0x00)
    fill_rect(buf, 0, 48, WIDTH, 16, 0xAA, 0x15, 0x1B)


def draw_thai(buf):
    fill_rect(buf, 0, 0, WIDTH, 11, 0xED, 0x1C, 0x24)
    fill_rect(buf, 0, 11, WIDTH, 11, 0xFF, 0xFF, 0xFF)
    fill_rect(buf, 0, 22, WIDTH, 21, 0x24, 0x1D, 0x4F)
    fill_rect(buf, 0, 43, WIDTH, 11, 0xFF, 0xFF, 0xFF)
    fill_rect(buf, 0, 54, WIDTH, 10, 0xED, 0x1C, 0x24)


def draw_russia(buf):
    fill_rect(buf, 0, 0, WIDTH, 21, 0xFF, 0xFF, 0xFF)
    fill_rect(buf, 0, 21, WIDTH, 22, 0x00, 0x39, 0xA6)
    fill_rect(buf, 0, 43, WIDTH, 21, 0xD5, 0x2B, 0x1E)


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <display-ip> [port]")
        sys.exit(1)

    host = sys.argv[1]
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 1337

    client = LEDClient(host, port)
    client.connect()
    print(f"Connected to {host}:{port}")

    # Ping to verify connection
    client.ping()
    print("Server is alive")

    # Upload all flag bitmaps (only once)
    print("Uploading flag bitmaps...")
    flags = [
        ("uk", draw_uk),
        ("spain", draw_spain),
        ("thai", draw_thai),
        ("russia", draw_russia),
    ]
    for name, draw_func in flags:
        rgb = make_rgb_data(draw_func)
        resp = client.upload_rgb(name, WIDTH, HEIGHT, rgb)
        print(f"  {name}: {resp}")

    print("Bitmaps uploaded. Starting demo loop (Ctrl+C to stop)...")

    scenes = [
        ("uk",     "Hello",                          True),
        ("spain",  "Hola!",                          True),
        ("thai",   "\u0e2a\u0e27\u0e31\u0e2a\u0e14\u0e35", False),
        ("russia", "\u041f\u0440\u0438\u0432\u0435\u0442",  False),
    ]

    loop_count = 0
    try:
        while True:
            for flag_name, greeting, is_ascii in scenes:
                print(f"flag {flag_name}\n")
                client.show(flag_name)
                client.wait(1000)

                print(f"text {greeting}\n")
                client.text(28 if is_ascii else 16, 32, "FFFFFF", 2, greeting)
                client.wait(2000)

                print(f"clear\n")
                client.clear_text()

            loop_count += 1
            # Ping every loop to keep connection alive and detect failures
            try:
                client.ping()
            except Exception as e:
                print(f"\nConnection lost: {e}")
                print("Reconnecting...")
                client.close()
                client = LEDClient(host, port)
                client.connect()
                client.ping()
                print("Reconnected, re-uploading bitmaps...")
                for name, draw_func in flags:
                    rgb = make_rgb_data(draw_func)
                    client.upload_rgb(name, WIDTH, HEIGHT, rgb)
                print("Ready")

    except KeyboardInterrupt:
        print(f"\nStopped after {loop_count} loops")
    finally:
        client.close()


if __name__ == "__main__":
    main()
