#!/usr/bin/env python3
"""Configure the clock's internet connectivity check (bottom status line).

The clock pings EXT_HOST; if that fails it pings INT_HOST and draws an
orange line (LAN only) or a red line (nothing reachable).
Settings are kept in RAM only and reset to build defaults on reboot.

Note: any TCP command marks the display as client-configured, which turns
off the NTP auto-started clock and the 10-minute no-time reboot.

Usage:
    python netcheck.py [-H display-ip] [-p port]              # show status
    python netcheck.py [-H display-ip] <ext_ip> [int_ip]      # set hosts
    python netcheck.py [-H display-ip] on|off                 # enable/disable
"""

import argparse
import ipaddress
import sys

from ledclient import LEDClient


def ipv4(value):
    try:
        return str(ipaddress.IPv4Address(value))
    except ipaddress.AddressValueError:
        raise argparse.ArgumentTypeError(f"invalid IPv4 address: {value}")


def main():
    parser = argparse.ArgumentParser(description="Configure LED clock netcheck")
    parser.add_argument("-H", "--host", default="192.168.1.110",
                        help="Display IP address (default: 192.168.1.110)")
    parser.add_argument("-p", "--port", type=int, default=1337, help="Port (default: 1337)")
    parser.add_argument("action", nargs="*",
                        help="'on', 'off', or <ext_ip> [int_ip]; empty = show status")
    args = parser.parse_args()

    if not args.action:
        cmd_args = []
    elif len(args.action) == 1 and args.action[0].lower() in ("on", "off"):
        cmd_args = [args.action[0].lower()]
    elif len(args.action) <= 2:
        try:
            cmd_args = [ipv4(a) for a in args.action]
        except argparse.ArgumentTypeError as e:
            parser.error(str(e))
    else:
        parser.error("expected 'on', 'off', or <ext_ip> [int_ip]")

    client = LEDClient(args.host, args.port)
    client.connect()
    try:
        resp = client.netcheck(*cmd_args)
        print(resp)
        if "ERR" in resp:
            sys.exit(1)
        # Show the resulting state after a change
        if cmd_args:
            print(client.netcheck())
    finally:
        client.close()


if __name__ == "__main__":
    main()
