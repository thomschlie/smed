#!/usr/bin/env python3
"""Checks smed's spnav socket through the system's libspnav (docs/spnav.md).

Opens the socket as any libspnav client does (honouring SPNAV_SOCKET), gives a name, checks
the protocol version and the device requests, and prints the events that arrive for a few
seconds: move the cap and press keys meanwhile. Needs libspnav (e.g. Fedora `libspnav`, Debian
`libspnav0`) and a running smed with the device. Exits 1 if a check fails.

Usage: tools/spnav-check.py [--seconds N] [--lib PATH]
"""

import argparse
import ctypes
import ctypes.util
import sys
import time

# libspnav's client-side event types (spnav.h).
EVENT_MOTION = 1
EVENT_BUTTON = 2
EVENT_DEV = 3


class Event(ctypes.Union):
    # spnav_event: every member starts with `int type`; the largest (motion) is 8 ints and a
    # pointer. Read as ints.
    _fields_ = [("type", ctypes.c_int), ("words", ctypes.c_int * 16)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--seconds", type=float, default=10.0, help="how long to print events")
    parser.add_argument("--lib", help="libspnav to load (default: the system's)")
    args = parser.parse_args()

    name = args.lib or ctypes.util.find_library("spnav") or "libspnav.so.0"
    spnav = ctypes.CDLL(name)
    spnav.spnav_cfg_get_sens.restype = ctypes.c_float

    failed = []

    def check(what: str, ok: bool, got) -> None:
        print(f"{'ok  ' if ok else 'FAIL'} {what}: {got}")
        if not ok:
            failed.append(what)

    if spnav.spnav_open() == -1:
        print("FAIL spnav_open: no socket (SPNAV_SOCKET set? smed running?)")
        return 1
    check("protocol", spnav.spnav_protocol() == 1, spnav.spnav_protocol())
    spnav.spnav_client_name(b"spnav-check")

    buf = ctypes.create_string_buffer(256)
    length = spnav.spnav_dev_name(buf, len(buf))
    check("device name", length > 0, buf.value.decode(errors="replace"))
    length = spnav.spnav_dev_path(buf, len(buf))
    check("device path", length > 0 and buf.value.startswith(b"/dev/"), buf.value.decode())
    check("axes", spnav.spnav_dev_axes() == 6, spnav.spnav_dev_axes())
    check("buttons", spnav.spnav_dev_buttons() == 177, spnav.spnav_dev_buttons())
    vend, prod = ctypes.c_uint(), ctypes.c_uint()
    usbid = spnav.spnav_dev_usbid(ctypes.byref(vend), ctypes.byref(prod))
    check(
        "usb id",
        usbid == 0 and (vend.value, prod.value) == (0x256F, 0xC633),
        f"{vend.value:04x}:{prod.value:04x}",
    )
    check("device type", spnav.spnav_dev_type() == 0x20E, hex(spnav.spnav_dev_type()))
    # smed keeps no spacenavd configuration: a configuration request fails.
    check("config request fails", spnav.spnav_cfg_get_sens() < 0.0, spnav.spnav_cfg_get_sens())

    print(f"events for {args.seconds:g} s: move the cap, press keys")
    event = Event()
    end = time.monotonic() + args.seconds
    while time.monotonic() < end:
        kind = spnav.spnav_poll_event(ctypes.byref(event))
        if kind == 0:
            time.sleep(0.005)
            continue
        w = event.words
        if kind == EVENT_MOTION:
            print(f"motion t {w[1]:5} {w[2]:5} {w[3]:5}  r {w[4]:5} {w[5]:5} {w[6]:5}  period {w[7]}")
        elif kind == EVENT_BUTTON:
            print(f"button {w[2]:3} {'press' if w[1] else 'release'}")
        elif kind == EVENT_DEV:
            print(f"device {'added' if w[1] == 0 else 'removed'} {w[4]:04x}:{w[5]:04x}")
        else:
            print(f"event type {kind}")
    spnav.spnav_close()

    if failed:
        print(f"{len(failed)} check(s) failed: {', '.join(failed)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
