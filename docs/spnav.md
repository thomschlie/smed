# The spacenavd/libspnav socket

smed also serves the socket protocol of spacenavd, so that Blender, FreeCAD and every other
program built on [libspnav](https://github.com/FreeSpacenav/libspnav) work unchanged. It carries
less than smed's own API ([api.md](api.md)): motion and keys, no long-press flag, no LCD. What
differs from spacenavd is how the keys are numbered: by the 3Dconnexion IDs the device reports,
which is what Blender expects for this device.

smed implements the protocol from libspnav 1.2 (`proto.h`, `spnav.c`) and spacenavd 1.4's
documented behaviour; no code of either is used.

## Where

- smed listens on `$XDG_RUNTIME_DIR/spnav.sock` (normally `/run/user/<uid>/spnav.sock`), per
  user like its API socket.
- libspnav 1.2 and Blender look in `$SPNAV_SOCKET`, then at `socket =` in `/etc/spnavrc`, then
  at `/var/run/spnav.sock` (spacenavd's). `dist/environment.d/smed.conf` sets
  `SPNAV_SOCKET=${XDG_RUNTIME_DIR}/spnav.sock` for the whole session: install it to
  `~/.config/environment.d/` and log in again. libspnav after 1.2 also tries
  `$XDG_RUNTIME_DIR/spnav.sock` by itself; Blender checks the path in `$SPNAV_SOCKET` itself, so
  it still needs the variable.
- Blender connects once, when it starts, and does not reconnect: smed has to be running by then
  (the user service starts at login), and Blender needs a restart after smed restarts.
- Switch the socket off with `[spnav] enabled = false` in `~/.config/smed/config.toml`, then
  remove `smed.conf` from `environment.d` too: smed cannot take the variable back.

## Protocol

Every word is a native-endian 32-bit integer. An event is 8 words; a v1 request and its answer
are 8 words too: the type, 6 data words, the status (0 ok, -1 failed) in the last.

- **Version.** libspnav's first word is `0x7faa5500 | version`. smed answers with the same word
  for the version it speaks, at most 1, as spacenavd does. A client that gets no answer, or never
  asks, speaks v0: then every word it sends is its sensitivity as a float.
- **Requests (v1).** smed answers:

  | Request | Answer |
  | --- | --- |
  | `SET_NAME` (0x1000) | none; the name is kept for the log |
  | `SET_SENS`, `GET_SENS` (0x1001, 0x1002) | this client's sensitivity (a float's bits) |
  | `SET_EVMASK`, `GET_EVMASK` (0x1003, 0x1004) | this client's event mask |
  | `DEV_NAME`, `DEV_PATH` (0x2000, 0x2001) | the kernel's name of the device, its hidraw node; strings in chunks of 24 bytes |
  | `DEV_NAXES` (0x2002) | 6 |
  | `DEV_NBUTTONS` (0x2003) | 177: the highest button ID (Space, 176) plus one |
  | `DEV_USBID` (0x2004) | `0x256f`, `0xc633` |
  | `DEV_TYPE` (0x2005) | `0x20e`, the SpaceMouse Enterprise in libspnav's device list |

  Without a device the device requests fail. Every configuration request (`SCFG_*`, `GCFG_*`,
  `CFG_SAVE`, `CFG_RESTORE`, `CFG_RESET`) and anything unknown fails: smed keeps no spacenavd
  configuration (no `spnavcfg`).
- **Events.** Motion `[0, x, y, z, rx, ry, rz, period]`, press `[1, id, 1, …]`, release
  `[2, id, 0, …]`, device `[3, op, 0, 0x20e, vid, pid, …]` (op 0 added, 1 removed). The mask
  decides which a client gets: v0 clients motion and buttons, v1 clients also the device events
  unless they set another mask. Raw axis and raw button events are never sent.

## Keys

Every key goes out by its 3Dconnexion ID, the same as `id` in smed's API ([api.md](api.md),
"key"): Menu 1, Fit 2, T 3, R 5, F 6, Roll 9, ISO 11, 1–10 13–22, Esc 23, Alt 24, Shift 25,
Ctrl 26, Lock 27, Enter 36, Delete 37, 11 77, 12 78, V1–V3 103–105, Tab 175, Space 176. These
are the values of Blender's NDOF button list, so Blender needs no mapping. spacenavd numbers the
same keys 0–30; a program configured for that numbering needs its keys assigned again.

A short press is a press and a release, sent when the device reports it (after the key went up,
docs/protocol.md). A long press is a press when the device reports it held and a release when
the key goes up, by this table, after 3DxWare's own:

| Key | Long press sends |
| --- | --- |
| T 3 | Bottom 7 |
| R 5 | Left 4 |
| F 6 | Back 8 |
| Roll 9 | Roll CCW 10 |
| ISO 11 | ISO2 12 |
| V1–V3 103–105 | Save V1–V3 139–141 |
| every other key | its own ID, held: a held Ctrl stays held |

The Lock key toggles smed's rotation lock and goes out as button 27 as well.

## Motion

As spacenavd 1.4 with its default settings, so that a client behaves as it does there:

- the six axes in the device's order and signs (x right, y towards the user, z down, then the
  rotations about them; full deflection about ±350);
- a value below 2 is 0 (spacenavd's default dead zone);
- each value times the client's sensitivity (1.0 unless the client sets one), truncated;
- `period` is the time since the device's last motion report in milliseconds.

In addition, when the cap comes back to rest, smed sends one all-zero event and then nothing
until it moves again; spacenavd sends none. While the rotation lock is on, the rotations are 0.
smed has no axis settings of its own for spnav clients yet.

## Next to spacenavd

smed reads the device itself; spacenavd should not run at the same time (it reads the device
too, and its key mappings would act as well). smed warns in its log when it sees
`/var/run/spnav.sock` or a `spacenavd` process at start. Stop it with
`sudo systemctl disable --now spacenavd`.

## Checking a client library

`tools/spnav-check.py` talks to the running smed through the system's libspnav (Python
`ctypes`, no build): it opens the socket, gives a name, asks the device requests and prints
events for a few seconds. Run it while smed and the device are there:

```bash
SPNAV_SOCKET=$XDG_RUNTIME_DIR/spnav.sock python3 tools/spnav-check.py
```
