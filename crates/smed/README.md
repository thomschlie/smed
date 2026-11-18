# smed

A Linux driver daemon for the **3Dconnexion SpaceMouse Enterprise** (USB `256f:c633`), aiming at
what 3DxWare 10 does on Windows: every key, long presses, the rotation lock with its LED, and
the LCD with labels and icons from the application in use.

smed reads the device through hidraw and reports it to any number of clients over a Unix
socket at `$XDG_RUNTIME_DIR/smed/api.sock`, in JSON lines
([docs/api.md](https://github.com/thomschlie/smed/blob/master/docs/api.md)). Rust applications
use the [`smed-client`](https://crates.io/crates/smed-client) crate; `smectl` is the command
line.

`cargo install smed` installs only the binary. smed also needs device access for the logged-in
user (a udev rule) and is meant to run as a systemd user service: both are in the repository's
[`dist/`](https://github.com/thomschlie/smed/tree/master/dist), and the
[README](https://github.com/thomschlie/smed#install-from-source) shows how to install them.

Not affiliated with or endorsed by 3Dconnexion.

## Licence

MIT or Apache-2.0, at your option. The embedded Noto Sans font is under the SIL Open Font
License 1.1 ([fonts/OFL.txt](fonts/OFL.txt)).
