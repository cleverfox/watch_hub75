#!/usr/bin/env python3
"""Pixelflut client for the LED Matrix display."""

import socket
import sys
import argparse


class LEDClient:
    """Client for Pixelflut LED Matrix display."""

    def __init__(self, host, port=1337):
        self.host = host
        self.port = port
        self.sock = None

    def connect(self):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.connect((self.host, self.port))
        self.sock.settimeout(30.0)

    def close(self):
        if self.sock:
            self.sock.close()
            self.sock = None

    def _send(self, cmd):
        self.sock.sendall((cmd + "\n").encode("utf-8"))

    def _send_recv(self, cmd):
        self._send(cmd)
        data = b""
        while not data.endswith(b"\n"):
            chunk = self.sock.recv(1024)
            if not chunk:
                break
            data += chunk
        return data.decode("utf-8").strip()

    def size(self):
        resp = self._send_recv("SIZE")
        parts = resp.split()
        return int(parts[1]), int(parts[2])

    def pixel(self, x, y, color):
        """Set pixel. color is hex string like 'FF0000'."""
        self._send(f"PX {x} {y} {color}")

    def get_pixel(self, x, y):
        """Read pixel color. Returns hex string."""
        resp = self._send_recv(f"PX {x} {y}")
        return resp.split()[-1]

    def rect(self, x, y, w, h, color):
        """Draw filled rectangle."""
        self._send(f"RECT {x} {y} {w} {h} {color}")

    def clear(self, color="000000"):
        """Clear screen."""
        self._send(f"CLR {color}")

    def text(self, x, y, color, scale, message):
        """Draw text as overlay label. Supports ASCII, Russian, Thai."""
        self._send(f"TEXT {x} {y} {color} {scale} {message}")

    def btext(self, x, y, color, scale, message):
        """Draw text directly into bitmap. No overlay, no accumulation."""
        self._send(f"BTEXT {x} {y} {color} {scale} {message}")

    def clear_text(self):
        """Clear text overlay only."""
        self._send("CLRTXT")

    def wait(self, ms):
        print(f"Wait {ms}ms")
        """Server-side delay in milliseconds."""
        self._send(f"WAIT {ms}")

    def ping(self):
        """Ping server. Returns True if alive, raises on failure."""
        resp = self._send_recv("PING")
        return resp == "PONG"

    def brightness(self, value):
        """Set display brightness 1-100."""
        resp = self._send_recv(f"BRIGHTNESS {value}")
        return resp

    def netcheck(self, *args):
        """Configure connectivity check. No args: status; "on"/"off";
        or ext_ip [int_ip]. Returns the response line."""
        return self._send_recv(" ".join(["NETCHECK", *args]))

    def upload(self, name, image):
        """Upload a PIL Image to display flash storage as raw RGB.
        Image is stored on the display and can be shown with show()."""
        img = image.convert("RGB")
        w, h = img.size
        raw = bytes(img.tobytes())
        cmd = f"UPLOAD {name} {w} {h}\n"
        self.sock.sendall(cmd.encode("utf-8"))
        self.sock.sendall(raw)
        # Read OK response
        data = b""
        while not data.endswith(b"\n"):
            chunk = self.sock.recv(1024)
            if not chunk:
                break
            data += chunk
        return data.decode("utf-8").strip()

    def upload_rgb(self, name, w, h, rgb_data):
        """Upload raw RGB bytes to display flash storage.
        rgb_data must be w*h*3 bytes."""
        cmd = f"UPLOAD {name} {w} {h}\n"
        self.sock.sendall(cmd.encode("utf-8"))
        self.sock.sendall(rgb_data)
        data = b""
        while not data.endswith(b"\n"):
            chunk = self.sock.recv(1024)
            if not chunk:
                break
            data += chunk
        return data.decode("utf-8").strip()

    def show(self, name, x=0, y=0):
        """Draw a stored bitmap from flash."""
        self._send(f"SHOW {name} {x} {y}")

    def list_bitmaps(self):
        """List stored bitmap names."""
        resp = self._send_recv("LIST")
        parts = resp.split()
        return parts[1:] if len(parts) > 1 else []

    def draw_image(self, x_off, y_off, image):
        """Draw a PIL Image at position. Sends pixels in batches."""
        pixels = image.convert("RGB").load()
        w, h = image.size
        batch = []
        for y in range(h):
            for x in range(w):
                r, g, b = pixels[x, y]
                color = f"{r:02x}{g:02x}{b:02x}"
                batch.append(f"PX {x_off + x} {y_off + y} {color}")
                if len(batch) >= 100:
                    self.sock.sendall(("\n".join(batch) + "\n").encode("utf-8"))
                    batch = []
        if batch:
            self.sock.sendall(("\n".join(batch) + "\n").encode("utf-8"))


def main():
    parser = argparse.ArgumentParser(description="Pixelflut LED Matrix Client")
    parser.add_argument("host", help="Display IP address")
    parser.add_argument("command", help="Command: size, clear, rect, text, pixel, image")
    parser.add_argument("args", nargs="*", help="Command arguments")
    parser.add_argument("-p", "--port", type=int, default=1337, help="Port (default: 1337)")
    args = parser.parse_args()

    client = LEDClient(args.host, args.port)
    client.connect()

    try:
        cmd = args.command.lower()

        if cmd == "size":
            w, h = client.size()
            print(f"{w} {h}")

        elif cmd == "clear":
            color = args.args[0] if args.args else "000000"
            client.clear(color)
            print("Cleared")

        elif cmd == "rect":
            if len(args.args) < 5:
                print("Usage: rect <x> <y> <w> <h> <color>")
                sys.exit(1)
            x, y, w, h = int(args.args[0]), int(args.args[1]), int(args.args[2]), int(args.args[3])
            client.rect(x, y, w, h, args.args[4])
            print(f"Rect at ({x},{y}) {w}x{h}")

        elif cmd == "text":
            if len(args.args) < 5:
                print("Usage: text <x> <y> <color> <scale> <message...>")
                sys.exit(1)
            x, y = int(args.args[0]), int(args.args[1])
            color = args.args[2]
            scale = int(args.args[3])
            message = " ".join(args.args[4:])
            client.text(x, y, color, scale, message)
            print(f"Text: {message}")

        elif cmd == "pixel":
            if len(args.args) < 3:
                print("Usage: pixel <x> <y> <color>")
                sys.exit(1)
            client.pixel(int(args.args[0]), int(args.args[1]), args.args[2])

        elif cmd == "image":
            if len(args.args) < 1:
                print("Usage: image <path> [x] [y]")
                sys.exit(1)
            from PIL import Image
            img = Image.open(args.args[0])
            x = int(args.args[1]) if len(args.args) > 1 else 0
            y = int(args.args[2]) if len(args.args) > 2 else 0
            # Resize if larger than display
            if img.width > 128 or img.height > 64:
                img.thumbnail((128, 64), Image.LANCZOS)
            client.draw_image(x, y, img)
            print(f"Image drawn ({img.width}x{img.height})")

        elif cmd == "clrtxt":
            client.clear_text()
            print("Text cleared")

        elif cmd == "wait":
            ms = int(args.args[0]) if args.args else 1000
            client.wait(ms)
            print(f"Wait {ms}ms")

        elif cmd == "upload":
            if len(args.args) < 1:
                print("Usage: upload <path> [name]")
                sys.exit(1)
            from PIL import Image
            img = Image.open(args.args[0])
            name = args.args[1] if len(args.args) > 1 else args.args[0].rsplit("/", 1)[-1].rsplit(".", 1)[0]
            if img.width > 128 or img.height > 64:
                img.thumbnail((128, 64), Image.LANCZOS)
            resp = client.upload(name, img)
            print(f"Uploaded: {resp}")

        elif cmd == "show":
            if len(args.args) < 1:
                print("Usage: show <name> [x] [y]")
                sys.exit(1)
            name = args.args[0]
            x = int(args.args[1]) if len(args.args) > 1 else 0
            y = int(args.args[2]) if len(args.args) > 2 else 0
            client.show(name, x, y)
            print(f"Showing {name}")

        elif cmd == "brightness":
            val = int(args.args[0]) if args.args else 50
            resp = client.brightness(val)
            print(f"Brightness: {resp}")

        elif cmd == "list":
            names = client.list_bitmaps()
            print("Stored bitmaps:", " ".join(names) if names else "(none)")

        else:
            print(f"Unknown command: {cmd}")
            print("Commands: size, clear, rect, text, pixel, image, clrtxt, wait, upload, show, list")
            sys.exit(1)

    finally:
        client.close()


if __name__ == "__main__":
    main()
