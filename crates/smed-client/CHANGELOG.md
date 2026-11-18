# Changelog

All notable changes to `smed-client`. The crate follows [Semantic Versioning](https://semver.org/);
as long as the major version is 0, a new minor version (0.x) may break code.

## 0.2.0 (2026-11-18)

API 2 of smed: an application puts its commands on keys 1–12 and the device's LCD. The JSON lines
of API 1 are unchanged, and a client that says `"api":1` sees exactly what it saw before
([docs/api.md](https://github.com/thomschlie/smed/blob/master/docs/api.md)).

### Added

- `API_VERSION` is 2. New client messages, each with a `Client` method: `set_icon`
  (`set_icon_svg`, `set_icon_png`), `set_commands`, `set_environments`, `set_environment`,
  `focus`, `set_lcd` and `get_screen`.
- New server messages: `Lcd` (the LCD settings), `Command` (a key with a command went down) and
  `Screen` (`screen_png()` decodes it).
- `ServerMessage::State` has an optional `lcd` field (API 2: whether smed can draw on the LCD).
- `App`, `Command` and `Environment` with `new()` and `with_*` builders; `DeviceInfo::new`;
  `TextSize` and `Theme`.
- `Lines`, a decoder without I/O for applications that read the socket in their own loop, and
  `LineError` (`Invalid` or `Overlong`). `MAX_MESSAGE` (256 KiB, the line limit of API 2) and
  `MAX_SERVER_LINE`.

### Changed

- The message enums, `Press`, `TextSize`, `Theme`, `DeviceInfo`, `App`, `Command` and
  `Environment` are `#[non_exhaustive]`, so later API versions can add message types, values and
  struct fields without breaking your code.

### Fixed

- `Client::next_message` lost the part of a line it had read before a read timeout, and the next
  call failed on the rest. It now keeps it.

### Upgrading from 0.1

- `match` on `ServerMessage` or `ClientMessage` needs a `_` arm, and patterns on a variant's
  fields need `..`.
- Build a `DeviceInfo` with `DeviceInfo::new(...)` instead of a struct literal.
- Prefer the `Client` methods to building `ClientMessage` values yourself.

## 0.1.0 (2026-11-14)

First release: the message types of smed's API 1, the table of key names and a blocking `Client`.
