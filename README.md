# smed

A Linux driver daemon for the **3Dconnexion SpaceMouse Enterprise** (USB `256f:c633`), aiming at
what 3DxWare 10 does on Windows: every key, long presses, the rotation lock with its LED, later
the LCD.

> **Status: early (0.1).** smed reads the device and reports all of it to clients over its own
> API: the six axes, all 31 keys by their 3Dconnexion IDs with short and long press, and the
> rotation lock with its LED. Not yet: the spacenavd socket for unchanged clients (Blender,
> FreeCAD), keyboard keys as real key presses, the LCD, per-application profiles.

Not affiliated with or endorsed by 3Dconnexion. "3Dconnexion" and "SpaceMouse" are trademarks
of their owner and are used here only to name the hardware.

## Why

spacenavd drives the motion and the keys of most 3Dconnexion devices, but the Enterprise needs
more. Blender expects 3Dconnexion's button IDs (up to 176), which spacenavd cannot map past 63.
Long presses (HID report `0x1d`), the rotation-lock LED and the LCD are not handled at all.

## What it does

- Runs per user (`systemd --user`) and owns the device through hidraw. Unplugging and
  re-plugging is handled.
- Reports to any number of clients over a Unix socket at `$XDG_RUNTIME_DIR/smed/api.sock`, in
  JSON lines ([docs/api.md](docs/api.md)):
  - motion: the raw axes;
  - every key by its 3Dconnexion ID, with whether the press was short or long;
  - the rotation lock. smed keeps the lock: the Lock key toggles it, the LED follows, and
    rotation is zeroed while it is on. Clients can switch it too.
- Attaches no meaning to keys: what a key does is the application's choice.

What we know about the device's protocol is in [docs/protocol.md](docs/protocol.md).

| Crate | What |
| --- | --- |
| `crates/smed` | the daemon |
| `crates/smed-client` | API message types, key table and a blocking client, for Rust applications |
| `crates/smectl` | command line: `smectl monitor`, `smectl state`, `smectl lock on\|off` |

## Install from source

Needs Rust 1.88 or newer.

```bash
cargo install --locked --root ~/.local --path crates/smed
cargo install --locked --root ~/.local --path crates/smectl

# device access for the logged-in user (as root)
sudo install -m 644 dist/70-smed.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger

# the user service
mkdir -p ~/.config/systemd/user
sed 's|^ExecStart=/usr/bin/smed|ExecStart=%h/.local/bin/smed|' dist/smed.service \
    > ~/.config/systemd/user/smed.service
systemctl --user daemon-reload
systemctl --user enable --now smed

smectl monitor    # every event as a JSON line
```

smed and spacenavd should not both run: stop spacenavd while smed is in use
(`sudo systemctl disable --now spacenavd`).

## Clients

- [SolidCraft](https://github.com/tangency-labs/solidcraft): a backend for smed is in
  preparation (all keys, long-press jobs, the lock).
- Your own: speak the JSON lines from [docs/api.md](docs/api.md) directly, or use the
  `smed-client` crate.

## Plans

1. The spacenavd/libspnav socket with 3Dconnexion button IDs, so Blender and FreeCAD work
   unchanged; keyboard keys (Esc, Ctrl, …) as real key presses for the focused window.
2. The LCD: brightness, pages, labels sent by the applications; per-application profiles.
3. An on-screen display, radial menus, macros, a settings tool.

## Licence

`MIT OR Apache-2.0` ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). No
spacenavd (GPL) code is used, only its documented socket protocol.
