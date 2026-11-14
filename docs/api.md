# smed client API (version 1)

smed owns the SpaceMouse Enterprise and reports what it does to any number of clients. This is
smed's own API, the one that carries the whole device: every key by its 3Dx ID, short and long
press, and the rotation lock. The spacenavd/libspnav socket for unchanged clients (Blender,
FreeCAD) comes later and carries less.

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
{"type":"hello","api":1,"client":"solidcraft"}
```

smed answers with its own `hello`, then a `state` message:

```json
{"type":"hello","api":1,"server":"smed 0.1.0"}
{"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false}
```

- `api` is the highest version the side speaks. smed answers with
  `min(client api, server api)` and uses that. A version smed does not support at all gets an
  `error` message, and smed closes the connection.
- Before the handshake smed sends nothing. A message other than `hello` gets an `error`.

## Server → client

| type | fields | meaning |
| --- | --- | --- |
| `hello` | `api`, `server` | answer to the client's `hello` |
| `state` | `device` (object or `null`), `lock` (bool) | the full state: after the handshake and on `get_state` |
| `device` | `device` (object or `null`) | the device appeared (object) or went away (`null`) |
| `motion` | `t` [x, y, z], `r` [rx, ry, rz] | one motion report, int16 each, ~60 Hz while the cap moves |
| `key` | `key`, `id`, `press`, `down` | a key went down or up |
| `lock` | `on` (bool) | the rotation lock changed (Lock key or a client's `set_lock`) |
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

## Example session

```text
> {"type":"hello","api":1,"client":"smectl"}
< {"type":"hello","api":1,"server":"smed 0.1.0"}
< {"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false}
< {"type":"motion","t":[0,-12,3],"r":[5,0,0]}
< {"type":"key","key":"lock","id":27,"press":"short","down":true}
< {"type":"lock","on":true}
< {"type":"key","key":"lock","id":27,"press":"short","down":false}
> {"type":"set_lock","on":false}
< {"type":"lock","on":false}
```
