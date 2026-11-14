# smed-client

Client for [smed](https://github.com/thomschlie/smed), a Linux driver daemon for the
3Dconnexion SpaceMouse Enterprise.

smed owns the device and reports it to any number of clients over a Unix socket at
`$XDG_RUNTIME_DIR/smed/api.sock`, in JSON lines. It reports the six raw axes, every key by its
3Dconnexion button ID with short or long press, and the rotation lock, which clients can also
set. This crate holds the message types both sides use, the table of key names and a small
blocking client. The protocol is specified in
[docs/api.md](https://github.com/thomschlie/smed/blob/master/docs/api.md).

```rust
use smed_client::{Client, ServerMessage};

let mut client = Client::connect("example")?;
while let Some(message) = client.next_message()? {
    if let ServerMessage::Key { key, press, down, .. } = message {
        println!("{key} {press:?} {}", if down { "down" } else { "up" });
    }
}
```

Not affiliated with or endorsed by 3Dconnexion.

## Licence

MIT or Apache-2.0, at your option.
