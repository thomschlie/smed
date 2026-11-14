# SpaceMouse Enterprise (USB 256f:c633): what we know about the protocol

What we know about the device, each item with its source. Most of it is measured: usbmon
captures of 3DxWare 10.9.15 on Windows 11 with our unit (the `[cap-…]` sources below). The
captures themselves are not published, since they carry 3DxWare's traffic and LCD images.

## Device (our unit, `lsusb -v`, 2026-11-12, before 3DxWare ever saw it)

USB 2.0, `bcdDevice` **4.42** (firmware as shipped/last host), no serial string, 2 interfaces:
interface 0 vendor class 255 with bulk OUT EP 0x01 (LCD); interface 1 HID with interrupt IN
EP 0x82 and OUT EP 0x03.

## Interfaces

| Interface | What | Source |
| --- | --- | --- |
| HID (hidraw) | motion, keys, long press, LED, LCD power | Blender GHOST NDOF code [blender]; space-elevator [se] |
| 0, vendor class | LCD frames: bulk OUT endpoint 0x01 (format below) | [se], [cap-3dx] |

## HID reports

| Report | Direction | Meaning | Source |
| --- | --- | --- | --- |
| 0x01 | in | all 6 axes in one report: 13 bytes = ID + x, y, z, rx, ry, rz (16-bit signed, LE), ~60 Hz while moving | [cap-plug] |
| 0x02 | in | rotation rx, ry, rz on older devices; not seen from the Enterprise so far | [blender], [spnav] |
| 0x03 | in | legacy button bitmask, 4 bytes LE; only 22 of the 31 keys have a bit (table below) | [cap-keys] |
| 0x1c | in | keys held, as 3Dx button IDs: 12 bytes, one ID per byte, 0 = none; all zero on release. Sent right after 0x03 | [blender], [cap-keys] |
| 0x1d | in | long press: 12 bytes like 0x1c, the key's **own** ID (T = 3, V1 = 103, key 1 = 13 …); zeroed on release | [cap-long] |
| 0x11 | feature, out | **LCD brightness 0–100**: `11 00` (slider left: dark), `11 32` (middle), `11 64` (right, default). Without it after plug-in the LCD stays dark. 3DxWare resends it with every page change | [trial], [cap-3dx], [cap-bright] |
| 0x13 | feature, out | `13 1a` once at init; meaning open | [cap-3dx] |
| 0x14 | feature, out | `14 fc ff 07 1c` 2× at init; meaning open (a key mask? `0x1c07fffc`) | [cap-3dx] |
| 0x15 | feature, out | `15 64` once at init; not touched by the LCD brightness slider, so probably the key/ring backlight level (no UI for it seen yet) | [cap-3dx], [cap-bright] |
| 0x04 | output (interrupt OUT EP 0x03) | LED bitmask: bit 1 (`0x02`) = rotation-lock LED (small blue LED by the Lock key), bit 0 always set (blue ring/backlight? open). `04 03` lock on, `04 01` off. The **host** toggles it: the device only reports the Lock key (ID 27) | [cap-keys], [cap-lock], photos |

3DxWare's init after enumeration, in order: SET_IDLE; SET_REPORT feature `15 64`, `11 64`;
`04 01` (interrupt OUT); `13 1a`; `11 64`; `14 fc ff 07 1c`; `11 64`; `14 fc ff 07 1c`; then
the first LCD frame about 0.6 s later [cap-3dx].

## LCD frame (bulk OUT EP 0x01)

One frame = two bulk transfers [cap-3dx], [cap-blender]:

1. a **512-byte header**:

   | Offset | Value | Meaning |
   | --- | --- | --- |
   | 0 | `0x11` / `0x18` / `0x15` | frame kind: `0x11` full frame; `0x18` transition step (see below); `0x15` seen once, the very first frame after init |
   | 1 | `0x0f` | constant so far |
   | 2–5 | u32 LE | length of the compressed image that follows (matches the 2nd transfer exactly) |
   | 6–10 | 0 | |
   | 11–14 | `3f 01 ef 00` on `0x11`/`0x15`, 0 on `0x18` | open (319, 239 as u16 LE: not the panel size) |
   | 15 | `0x0f` | constant so far |
   | 16–511 | 0 | |

2. the image: **raw deflate** (no zlib header) of **640×150 pixels, RGB565 little-endian**
   (192000 bytes inflated). Decoded, it is exactly what the LCD shows (checked against the
   device, 2026-11-13).

**Focus change** (3DxWare switching to another application's tiles): `11 64` (feature),
`04 01` (output), `14 fc ff 07 1c` (feature), then one `0x11` frame of the old page, 7 `0x18`
frames and a final `0x11` frame of the new page, about 40 ms apart (~0.35 s). The `0x18`
frames are black except shrinking (old) or growing (new) icon/label artwork: a zoom
transition. Whether the device blends `0x18` frames or just shows them is open; a driver can
send `0x11` frames only.

3DxWare also resends the current page (`0x11`) every few seconds to tens of seconds without a
visible change.

## Key numbering

Measured on the device with 3DxWare 10.9.15 [cap-keys]: each press sends 0x03 then 0x1c
(4 ms apart), each release the same two reports zeroed (~100 ms press).

| Key | 0x03 bit | 0x1c ID | Blender enum (GHOST) | spacenavd 1.4 index |
| --- | --- | --- | --- | --- |
| 1–10 | 12–21 | 13–22 | BUTTON_1..10 | 0–9 |
| 11, 12 | — | 77, 78 | BUTTON_11, 12 | 10, 11 |
| Menu | 0 | 1 | MENU | 12 |
| Fit | 1 | 2 | FIT | 13 |
| T (top) | 2 | 3 | TOP | 14 |
| R (right) | 4 | 5 | RIGHT | 15 |
| F (front) | 5 | 6 | FRONT | 16 |
| Roll | 8 | 9 | ROLL_CW | 17 |
| Esc, Alt, Shift, Ctrl | 22–25 | 23–26 | ESC, ALT, SHIFT, CTRL | 18–21 |
| Lock (rotation) | 26 | 27 | ROTATE | 22 |
| Enter, Delete | — | 36, 37 | ENTER, DELETE | 23, 24 |
| Tab, Space | — | 175, 176 | TAB, SPACE | 25, 26 |
| V1–V3 | — | 103–105 | V1–V3 | 27–29 |
| ISO | 10 | 11 | ISO1 | 30 |

So **0x1c carries exactly the IDs Blender's enum expects** (MENU 1 … SPACE 176) and smed can
pass them through. The legacy 0x03 mask has no bit for 11, 12, Enter, Delete, Tab, Space and
V1–V3; the bits it has follow the old 3Dx numbering minus one (bit n = ID n+1 for Menu…ISO,
Esc…Lock; 1–10 are bits 12–21 = IDs 13–22).

Blender's enum (for reference, [blender]): MENU 1, FIT 2, TOP 3, LEFT 4, RIGHT 5, FRONT 6,
BOTTOM 7, BACK 8, ROLL_CW 9, ROLL_CCW 10, ISO1 11, ISO2 12, BUTTON_1..10 = 13..22, ESC 23,
ALT 24, SHIFT 25, CTRL 26, ROTATE 27, PANZOOM 28, DOMINANT 29, PLUS 30, MINUS 31, ENTER 36,
DELETE 37, BUTTON_11/12 = 77/78, V1–V3 = 103–105, SAVE_V1–V3 = 139–141, TAB 175, SPACE 176.
Under stock spacenavd the keys arrive scrambled in Blender (measured [trial]): spacenavd
numbers them 0–30 and its `bnmap` cannot reach IDs ≥ 64.

## Short vs. long press

Measured [cap-keys], [cap-long]: the device decides short vs. long itself.

- **Short press:** nothing at key-down; at release (or shortly after) it sends 0x03 + 0x1c with
  the key, then both zeroed ~100 ms later. So a host never sees how long a short press lasted.
- **Long press:** once the hold threshold passes it sends **0x1d with the key's own ID**, then
  0x03 + 0x1c zeroed; at release 0x1d zeroed. Seen for T, R, F, Roll, ISO, V1–V3 and key 1, so
  every key can long-press. No 0x1c for that press at all.
- 0x1d does **not** carry the "opposite" ID. 3DxWare (and Blender on Windows, via `Base.xml`)
  map it: T → BOTTOM 7, R → LEFT 4, F → BACK 8, Roll → ROLL_CCW 10, ISO → ISO2 12,
  V1–V3 → SAVE_V1–V3 139–141. smed has to do this mapping to give Blender the long-press
  functions.
- Holding key 1 sent nothing to the LCD or LEDs: the tile highlight/OSD is not a device
  feature visible on USB here (3DxWare shows the OSD on screen).

## Open

- The long-press threshold (time from key-down to 0x1d): not visible on USB, needs a stopwatch or video.
- The LED report, and which LED spacenavd's `led=` drives.
- Whether `0x18` frames are blended by the device; header bytes 11–14.
- LCD header bytes 11–14; feature reports 0x13 and 0x14; whether 0x15 is the backlight and
  bit 0 of output 0x04 the ring.

## Sources

- [blender] Blender `intern/ghost/intern/GHOST_NDOFManager.cc`, 4.3+ (PR !124155 removed the
  Unix table for 0xC633),
  https://projects.blender.org/blender/blender/src/branch/main/intern/ghost/intern/GHOST_NDOFManager.cc
  and `GHOST_SystemWin32.cc` (RawInput, reports 0x1C/0x1D).
- [spnav] spacenavd 1.4 `src/dev_usb_linux.c` / `bnhack_sment`; libspnav protocol (protocol
  only, GPL code not copied).
- [se] space-elevator, commit 8c44e65f5b27317f58b90fae486b9417f0a3c026 (reference only, no code
  copied), https://github.com/tophcodes/space-elevator/tree/8c44e65.
- [cap-plug] capture 2026-11-13: attach to Windows, no user logged in
  (3DxWare idle): enumeration and motion only.
- [cap-3dx] capture 2026-11-13: re-attach with 3DxWare 10.9.15
  running in the logged-in session.
- [cap-blender] capture 2026-11-13: Blender 5.3 (llvmpipe) started,
  focus moved between Blender, its console and the desktop (8 page changes in 67 s).
- [cap-keys] capture 2026-11-13: all 31 keys pressed once, in order,
  Blender in focus.
- [cap-long] capture 2026-11-13: T, R, F, Roll, ISO, V1–V3, key 1 each
  held ~2 s, Blender in focus.
- [cap-lock] capture 2026-11-13 and photos of the LED: Lock pressed once
  (LED on → off); 3DxWare answers the key with `11 64`, `04 01`, `14 fc ff 07 1c`.
- [cap-bright] capture 2026-11-13: 3DxWare LCD Settings: brightness min,
  middle, max; layout text/icons/both; text size large/small. Layout and text size only
  change the rendered frames (host-side rendering).
- [trial] our test of stock spacenavd 1.4 with Blender 5.3 on Fedora 44, 2026-11-12.
