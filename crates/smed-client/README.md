# smed-client

Client for [smed](https://github.com/thomschlie/smed), a Linux driver daemon for the
3Dconnexion SpaceMouse Enterprise.

smed owns the device and reports it to any number of clients over a Unix socket at
`$XDG_RUNTIME_DIR/smed/api.sock`, in JSON lines. It reports the six raw axes, every key by its
3Dconnexion button ID with short or long press, and the rotation lock, which clients can also
set, and draws the LCD for the application that has the focus. This crate holds the message
types both sides use, the table of key names and a small blocking client. The protocol is
specified in
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

With API 2, an application puts its commands on keys 1–12 and the device's LCD: it sends its
command catalogue (labels, SVG or PNG icons) and environments, says when its window has the
focus, and smed reports a `command` when a key with a command goes down:

```rust
use smed_client::{App, Client, Command, Environment, ServerMessage};

let mut client = Client::connect("example")?;
client.set_commands(&App::new("Example"), &[Command::new("edit.undo", "Undo")])?;
client.set_environments(&[Environment::new("main", "Main").with_key("1", "edit.undo")])?;
client.set_environment("main")?;
client.focus(true)?;
while let Some(message) = client.next_message()? {
    if let ServerMessage::Command { id, .. } = message {
        println!("run {id}");
    }
}
```

The message enums and structs are `#[non_exhaustive]`: later API versions can add message
types, values and fields without breaking your code.

Not affiliated with or endorsed by 3Dconnexion.

## Licence

MIT or Apache-2.0, at your option.
