# smed client API (version 2)

smed owns the SpaceMouse Enterprise and reports what it does to any number of clients. This is
smed's own API, the one that carries the whole device: every key by its 3Dx ID, short and long
press, the rotation lock, and (version 2) the LCD with labels and icons from the application
that has the focus. The spacenavd/libspnav socket for unchanged clients (Blender, FreeCAD)
comes later and carries less.

Version 2 only adds to version 1: a client that says `"api":1` sees exactly what version 1
describes. Everything marked *API 2* below is sent to, and accepted from, connections that
agreed on version 2 only.

Rust clients use `crates/smed-client`. The protocol is small enough to speak without it.

## Transport

- A Unix stream socket at `$XDG_RUNTIME_DIR/smed/api.sock` (normally
  `/run/user/<uid>/smed/api.sock`). smed runs per user (systemd `--user`), so only that user
  reaches it.
- **JSON lines**: each message is one JSON object on one line, ending in `\n`, UTF-8.
- Each object has a `"type"` field. **Unknown types and unknown fields are ignored**, by both
  sides. This is how the API grows without a new version.

## Handshake

The client speaks first:

```json
{"type":"hello","api":2,"client":"solidcraft"}
```

smed answers with its own `hello`, then a `state` message, and with API 2 an `lcd` message:

```json
{"type":"hello","api":2,"server":"smed 0.2.0"}
{"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false,"lcd":true}
{"type":"lcd","owner":false,"brightness":100,"text_size":"small","theme":"light"}
```

With `"api":1` the same handshake is `hello` and `state` without the `lcd` field:

```json
{"type":"hello","api":1,"server":"smed 0.2.0"}
{"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false}
```

- `api` is the highest version the side speaks. smed answers with
  `min(client api, server api)` and uses that. A version smed does not support at all gets an
  `error` message, and smed closes the connection.
- Before the handshake smed sends nothing. A message other than `hello` gets an `error`.
- *API 2:* a line longer than 262144 bytes (256 KiB, icons included) gets an `error` and is
  dropped; the connection stays. With API 1 there is no such limit, as before.

## Server → client

| type | fields | meaning |
| --- | --- | --- |
| `hello` | `api`, `server` | answer to the client's `hello` |
| `state` | `device` (object or `null`), `lock` (bool), *API 2:* `lcd` (bool) | the full state: after the handshake and on `get_state`. `lcd`: smed can draw on the LCD (the device is there and its LCD interface could be claimed) |
| `device` | `device` (object or `null`) | the device appeared (object) or went away (`null`) |
| `motion` | `t` [x, y, z], `r` [rx, ry, rz] | one motion report, int16 each, ~60 Hz while the cap moves |
| `key` | `key`, `id`, `press`, `down` | a key went down or up |
| `lock` | `on` (bool) | the rotation lock changed (Lock key or a client's `set_lock`) |
| `lcd` | `owner` (bool), `brightness` (0–100), `text_size` (`"small"`/`"large"`), `theme` (`"light"`/`"dark"`) | *API 2.* `owner`: this connection has the LCD. Sent after the handshake, when the owner or a setting changes, and in answer to every `set_lcd` |
| `command` | `id`, `env`, `key`, `press` | *API 2.* A key with a command went down (see "Commands"); only to the client that has the LCD |
| `screen` | `png` (base64) | *API 2.* The answer to `get_screen`: the page smed shows now, drawn as it goes to the LCD (also without a device), as a 640×150 PNG |
| `error` | `message` | the last request failed |

`device` object: `vid`, `pid` (numbers), `name` (string, the kernel's HID name), `firmware`
(string such as `"4.42"`, from bcdDevice, or `null`).

### motion

The raw device frame, the same as HID report 0x01: x right, y towards the user, z down; `r`
rotates about the same axes. smed scales and maps nothing. Full deflection is about ±350.
While the rotation lock is on, `r` is `[0,0,0]`. A cap back at rest sends a final all-zero
report.

### key

```json
{"type":"key","key":"t","id":3,"press":"short","down":true}
{"type":"key","key":"t","id":3,"press":"short","down":false}
{"type":"key","key":"t","id":3,"press":"long","down":true}
```

- `id` is the 3Dx button ID the device reports. It equals Blender's NDOF enum (docs/protocol.md,
  "Key numbering"). `key` is the printed key's name from the table below.
- `press` is `"short"` or `"long"`. **The device decides which**:
  - Short: the device sends nothing at key-down. At release it reports the key for about
    100 ms. So a short press arrives as `down` and then `up` about 100 ms later, **after** the
    finger has left the key. Act on `down`.
  - Long: once the hold threshold passes, the device reports the key as long-pressed. Then
    `down` arrives while the key is still held, and `up` arrives at release.
- One physical press is either short or long, never both.
- smed attaches no meaning to any key, the Lock key included (see below). Deciding what T long
  does is the app's job. 3DxWare's defaults are T → bottom, R → left, F → back, Roll → roll
  the other way, ISO → the other ISO view, V1–V3 → save the view.
- The keyboard keys (Esc, Alt, Shift, Ctrl, Enter, Delete, Tab, Space) are API events only.
  smed does not type them (a later version adds that, with focus detection).

| key | id | | key | id |
| --- | --- | --- | --- | --- |
| `menu` | 1 | | `esc` | 23 |
| `fit` | 2 | | `alt` | 24 |
| `t` | 3 | | `shift` | 25 |
| `r` | 5 | | `ctrl` | 26 |
| `f` | 6 | | `lock` | 27 |
| `roll` | 9 | | `enter` | 36 |
| `iso` | 11 | | `delete` | 37 |
| `1`–`10` | 13–22 | | `v1`–`v3` | 103–105 |
| `11`, `12` | 77, 78 | | `tab`, `space` | 175, 176 |

An ID that is not in the table is still sent, with `key` set to `"id<N>"`.

### Rotation lock

The lock is smed's state, not a client's. A `down` of the Lock key (short or long) toggles it.
smed then lights the LED by the key, zeroes `r` in every `motion`, and sends `lock` to every
client. The `key` event for Lock is sent as well. The lock survives clients coming and going
and a re-plug of the device, but not a restart of smed.

## Client → server

| type | fields | effect |
| --- | --- | --- |
| `hello` | `api`, `client` | handshake (above) |
| `set_lock` | `on` (bool) | set the rotation lock; every client gets `lock` if it changed |
| `get_state` | — | smed answers with `state` |
| `set_icon` | `id`, and `svg` (SVG source) or `png` (base64 PNG) | *API 2.* Keep an icon under `id` for this connection; replaces one with the same `id` |
| `set_commands` | `app` {`name`, `icon`?}, `commands` [{`id`, `label`, `icon`?}] | *API 2.* The application and its command catalogue; replaces the previous ones |
| `set_environments` | `environments` [{`id`, `name`, `keys` {key name: command `id`}}] | *API 2.* The environments and which command each key runs in them; replaces the previous ones |
| `set_environment` | `id` | *API 2.* Use environment `id` (from `set_environments`): its labels show, its keys run their commands |
| `focus` | `on` (bool) | *API 2.* This client's window got or lost the input focus |
| `set_lcd` | `brightness`?, `text_size`?, `theme`? | *API 2.* Change the LCD settings, for every client, until smed restarts; smed answers with `lcd` |
| `get_screen` | — | *API 2.* smed answers with `screen` |

## The LCD (API 2)

The LCD (640×150) shows one page: a header with the application's icon, name and environment,
then 12 tiles over the keys 1–6 (top row) and 7–12 (bottom row), each with an icon above its
label. **smed draws it** from what clients send; a client sends no pictures of pages.

- **Who has it.** The client that last sent `"focus":true` owns the LCD; smed tells every
  API 2 client with `lcd` (`owner`). A `focus` `false` from the owner, or its disconnect,
  leaves the LCD without an owner: smed then shows its own page. A client should send
  `focus` whenever its window gains or loses the input focus (smed does not watch windows
  yet). `key`, `motion` and `lock` still go to every client.
- **What it shows.** The owner's `app` (`name`, else the name from its `hello`), its
  environment's `name` (the one `set_environment` chose, else the first), and for each key `"1"`–`"12"` in that environment's `keys` the
  command's `label` and `icon`. A key with no command, or with a command `id` smed does not
  know, shows an empty tile; an icon `id` smed does not know is left out. Only the keys 1–12 have tiles;
  other key names in `keys` are kept for later versions.
- **Icons.** SVG is drawn without text elements and without raster images inside it.
  `currentColor` is drawn in the theme's text colour, unless the SVG's root sets `color`;
  icons with fixed colours keep them. A PNG is scaled to fit; it may be at most 512×512 pixels. Icons are square-fitted and
  belong to the connection: send them before the commands that use them, and again after a
  reconnect. An icon that does not parse gets an `error`.
- **Replace, not patch.** Each `set_commands`, `set_environments` replaces that part
  completely; when the client disconnects, smed forgets everything it sent.
- **Settings.** `brightness`, `text_size` and `theme` are smed's, shared by all clients:
  from smed's config file at start (`$XDG_CONFIG_HOME/smed/config.toml`, section `[lcd]`),
  changed by `set_lcd` until smed restarts. smed never writes the file.
- Changes within about 50 ms make one frame; smed sends a frame only when the page changed.

```json
{"type":"set_icon","id":"line","svg":"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path d='M4 20 20 4' stroke='currentColor' stroke-width='2'/></svg>"}
{"type":"set_commands","app":{"name":"SolidCraft","icon":"logo"},"commands":[{"id":"sketch.line","label":"Line","icon":"line"},{"id":"edit.undo","label":"Undo"}]}
{"type":"set_environments","environments":[{"id":"sketch","name":"Sketch","keys":{"1":"sketch.line","9":"edit.undo"}}]}
{"type":"focus","on":true}
```

### Commands

The key assignment is the client's catalogue as smed shows it: when a key 1–12 goes down and
the environment in use assigns a command to it, smed sends the client that has the LCD

```json
{"type":"command","id":"sketch.line","env":"sketch","key":"1","press":"short"}
```

right after the `key` event, which every client still gets. `press` is the key's (`"short"` or
`"long"`, as with `key`); what a long press of a command key means is the client's choice. A
client that registers commands should act on `command` and ignore `key` for 1–12. Keys without
a command send `key` only. Switch environments with

```json
{"type":"set_environment","id":"part"}
```

### Errors

A request is applied as far as it can be; what does not fit is reported with one `error`
after it, and the connection stays:

- `set_commands`: an icon `id` not sent with `set_icon` (the tile shows no icon);
- `set_environments`: a command `id` not in the catalogue (that key stays empty), or a name in
  `keys` that is no key (`"13"`); names of keys without a tile (`"t"`, `"v1"`) are kept quietly;
- `set_environment`: an `id` not among the environments: nothing changes;
- `set_icon`: SVG or PNG that does not parse, or a PNG over 512×512: the icon is not kept.

Send `set_icon` before `set_commands` and the catalogue before `set_environments`, so nothing
is reported that is only out of order.

## Example session

```text
> {"type":"hello","api":1,"client":"smectl"}
< {"type":"hello","api":1,"server":"smed 0.2.0"}
< {"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false}
< {"type":"motion","t":[0,-12,3],"r":[5,0,0]}
< {"type":"key","key":"lock","id":27,"press":"short","down":true}
< {"type":"lock","on":true}
< {"type":"key","key":"lock","id":27,"press":"short","down":false}
> {"type":"set_lock","on":false}
< {"type":"lock","on":false}
```

With API 2:

```text
> {"type":"hello","api":2,"client":"smectl"}
< {"type":"hello","api":2,"server":"smed 0.2.0"}
< {"type":"state","device":null,"lock":false,"lcd":false}
< {"type":"lcd","owner":false,"brightness":100,"text_size":"small","theme":"light"}
> {"type":"focus","on":true}
< {"type":"lcd","owner":true,"brightness":100,"text_size":"small","theme":"light"}
> {"type":"set_lcd","brightness":40}
< {"type":"lcd","owner":true,"brightness":40,"text_size":"small","theme":"light"}
> {"type":"get_screen"}
< {"type":"screen","png":"iVBORw0KGgo="}
```

(The `png` is shortened here; it is a whole 640×150 PNG.)
