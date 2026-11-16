//! Client for smed, the SpaceMouse Enterprise driver daemon.
//!
//! smed speaks JSON lines over a Unix socket (`docs/api.md` in the repository is the
//! specification). This crate holds the message types both sides use, the table of key names
//! and a small blocking client:
//!
//! ```no_run
//! use smed_client::{Client, ServerMessage};
//!
//! let mut client = Client::connect("example")?;
//! while let Some(message) = client.next_message()? {
//!     if let ServerMessage::Key { key, press, down, .. } = message {
//!         println!("{key} {press:?} {}", if down { "down" } else { "up" });
//!     }
//! }
//! # Ok::<(), std::io::Error>(())
//! ```

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The highest API version this crate speaks.
pub const API_VERSION: u32 = 2;

/// The largest message smed accepts, in bytes (one line, icons included).
pub const MAX_MESSAGE: usize = 256 * 1024;

/// The socket's path below `$XDG_RUNTIME_DIR`.
pub const SOCKET_IN_RUNTIME_DIR: &str = "smed/api.sock";

/// Where smed listens for the current user: `$XDG_RUNTIME_DIR/smed/api.sock`, or `None` when
/// `XDG_RUNTIME_DIR` is not set.
pub fn default_socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(dir).join(SOCKET_IN_RUNTIME_DIR))
}

/// The device smed has, as reported in `state` and `device`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    /// The kernel's HID name.
    pub name: String,
    /// The firmware version from the USB descriptor (bcdDevice), such as `"4.42"`.
    pub firmware: Option<String>,
}

/// Whether the device reported a key as pressed briefly or held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Press {
    Short,
    Long,
}

/// Text size on the LCD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    #[default]
    Small,
    Large,
}

/// Colour theme of the LCD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// The application as shown in the LCD's header (`set_commands`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    /// An icon `id` from `set_icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// One command of the client's catalogue (`set_commands`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub label: String,
    /// An icon `id` from `set_icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// One environment of the client (`set_environments`): a name and which command each key
/// runs there, by key name (`"1"`–`"12"`) and command `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

/// What smed sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello {
        api: u32,
        server: String,
    },
    State {
        device: Option<DeviceInfo>,
        lock: bool,
        /// API 2: whether smed can draw on the LCD. Absent with API 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lcd: Option<bool>,
    },
    Device {
        device: Option<DeviceInfo>,
    },
    /// Translation `t` and rotation `r` in the raw device frame, rotation zeroed while locked.
    Motion {
        t: [i16; 3],
        r: [i16; 3],
    },
    Key {
        key: String,
        id: u8,
        press: Press,
        down: bool,
    },
    Lock {
        on: bool,
    },
    /// API 2: whether this connection has the LCD, and the LCD settings.
    Lcd {
        owner: bool,
        brightness: u8,
        text_size: TextSize,
        theme: Theme,
    },
    /// API 2: the answer to `get_screen`: the current LCD page as a PNG (base64), drawn as it
    /// goes to the device.
    Screen {
        png: String,
    },
    Error {
        message: String,
    },
    /// A message type this crate does not know (from a newer smed); ignore it.
    #[serde(other)]
    Unknown,
}

/// What a client sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        api: u32,
        client: String,
    },
    SetLock {
        on: bool,
    },
    GetState,
    /// API 2: an icon for tiles and the header, kept per connection under `id`. One of `svg`
    /// (SVG source) and `png` (base64).
    SetIcon {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        svg: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        png: Option<String>,
    },
    /// API 2: the application and its command catalogue; replaces the previous one.
    SetCommands {
        app: App,
        commands: Vec<Command>,
    },
    /// API 2: the environments with their key assignments; replaces the previous ones.
    SetEnvironments {
        environments: Vec<Environment>,
    },
    /// API 2: this client's window got (`true`) or lost (`false`) the input focus.
    Focus {
        on: bool,
    },
    /// API 2: change the LCD settings (for every client) until smed restarts.
    SetLcd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brightness: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_size: Option<TextSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme: Option<Theme>,
    },
    /// API 2: ask for the current LCD page as a PNG; smed answers with `screen`.
    GetScreen,
    /// A message type smed does not know (from a newer client); smed ignores it.
    #[serde(other)]
    Unknown,
}

/// The printed keys and their 3Dx IDs (docs/protocol.md, "Key numbering").
pub const KEYS: [(u8, &str); 31] = [
    (1, "menu"),
    (2, "fit"),
    (3, "t"),
    (5, "r"),
    (6, "f"),
    (9, "roll"),
    (11, "iso"),
    (13, "1"),
    (14, "2"),
    (15, "3"),
    (16, "4"),
    (17, "5"),
    (18, "6"),
    (19, "7"),
    (20, "8"),
    (21, "9"),
    (22, "10"),
    (23, "esc"),
    (24, "alt"),
    (25, "shift"),
    (26, "ctrl"),
    (27, "lock"),
    (36, "enter"),
    (37, "delete"),
    (77, "11"),
    (78, "12"),
    (103, "v1"),
    (104, "v2"),
    (105, "v3"),
    (175, "tab"),
    (176, "space"),
];

/// The 3Dx ID of the rotation-lock key.
pub const LOCK_KEY_ID: u8 = 27;

/// The name of the key with 3Dx ID `id`, or `"id<N>"` for an ID not on the device's keys.
pub fn key_name(id: u8) -> String {
    KEYS.iter()
        .find(|(key_id, _)| *key_id == id)
        .map_or_else(|| format!("id{id}"), |(_, name)| (*name).to_owned())
}

/// The 3Dx ID of the key named `name`.
pub fn key_id(name: &str) -> Option<u8> {
    KEYS.iter()
        .find(|(_, key_name)| *key_name == name)
        .map(|(id, _)| *id)
}

/// One message as a line, `\n` included.
pub fn to_line<T: Serialize>(message: &T) -> String {
    // Serializing these types cannot fail: no maps with non-string keys, no foreign Serialize.
    let mut line = serde_json::to_string(message).unwrap_or_default();
    line.push('\n');
    line
}

/// A connection to smed, after the handshake.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    /// The API version both sides agreed on.
    pub api: u32,
    /// smed's name and version from its `hello`.
    pub server: String,
}

impl Client {
    /// Connects to smed at [`default_socket_path`] and introduces itself as `client`.
    pub fn connect(client: &str) -> io::Result<Self> {
        let path = default_socket_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
        Self::connect_to(&path, client)
    }

    /// Connects to smed at `path` and introduces itself as `client`.
    pub fn connect_to(path: &Path, client: &str) -> io::Result<Self> {
        let writer = UnixStream::connect(path)?;
        let reader = BufReader::new(writer.try_clone()?);
        let mut this = Self {
            reader,
            writer,
            api: 0,
            server: String::new(),
        };
        this.send(&ClientMessage::Hello {
            api: API_VERSION,
            client: client.to_owned(),
        })?;
        match this.next_message()? {
            Some(ServerMessage::Hello { api, server }) => {
                this.api = api;
                this.server = server;
                Ok(this)
            }
            Some(ServerMessage::Error { message }) => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected hello from smed, got {other:?}"),
            )),
        }
    }

    /// Sends one message.
    pub fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
        self.writer.write_all(to_line(message).as_bytes())
    }

    /// Asks smed to switch the rotation lock on or off.
    pub fn set_lock(&mut self, on: bool) -> io::Result<()> {
        self.send(&ClientMessage::SetLock { on })
    }

    /// Asks smed for a `state` message.
    pub fn get_state(&mut self) -> io::Result<()> {
        self.send(&ClientMessage::GetState)
    }

    /// Tells smed whether this client's window has the input focus (API 2): the focused
    /// client has the LCD.
    pub fn focus(&mut self, on: bool) -> io::Result<()> {
        self.send(&ClientMessage::Focus { on })
    }

    /// The next message, blocking; `None` once smed closed the connection. Unknown message
    /// types are skipped; a line that is not valid JSON is an error.
    pub fn next_message(&mut self) -> io::Result<Option<ServerMessage>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ServerMessage>(&line) {
                Ok(ServerMessage::Unknown) => continue,
                Ok(message) => return Ok(Some(message)),
                Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
            }
        }
    }

    /// The underlying stream, e.g. to set a read timeout or shut it down from another thread.
    pub fn stream(&self) -> &UnixStream {
        &self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_server(line: &str, message: ServerMessage) {
        assert_eq!(
            serde_json::from_str::<ServerMessage>(line).ok(),
            Some(message.clone())
        );
        assert_eq!(to_line(&message), format!("{line}\n"));
    }

    // The lines are the examples in docs/api.md.
    #[test]
    fn server_messages_match_the_spec() {
        round_trip_server(
            r#"{"type":"hello","api":1,"server":"smed 0.2.0"}"#,
            ServerMessage::Hello {
                api: 1,
                server: "smed 0.2.0".into(),
            },
        );
        round_trip_server(
            r#"{"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false,"lcd":true}"#,
            ServerMessage::State {
                device: Some(DeviceInfo {
                    vid: 0x256f,
                    pid: 0xc633,
                    name: "3Dconnexion SpaceMouse Enterprise".into(),
                    firmware: Some("4.42".into()),
                }),
                lock: false,
                lcd: Some(true),
            },
        );
        round_trip_server(
            r#"{"type":"lcd","owner":false,"brightness":100,"text_size":"small","theme":"light"}"#,
            ServerMessage::Lcd {
                owner: false,
                brightness: 100,
                text_size: TextSize::Small,
                theme: Theme::Light,
            },
        );
        round_trip_server(
            r#"{"type":"state","device":{"vid":9583,"pid":50739,"name":"3Dconnexion SpaceMouse Enterprise","firmware":"4.42"},"lock":false}"#,
            ServerMessage::State {
                device: Some(DeviceInfo {
                    vid: 0x256f,
                    pid: 0xc633,
                    name: "3Dconnexion SpaceMouse Enterprise".into(),
                    firmware: Some("4.42".into()),
                }),
                lock: false,
                lcd: None,
            },
        );
        round_trip_server(
            r#"{"type":"screen","png":"iVBORw0KGgo="}"#,
            ServerMessage::Screen {
                png: "iVBORw0KGgo=".into(),
            },
        );
        round_trip_server(
            r#"{"type":"device","device":null}"#,
            ServerMessage::Device { device: None },
        );
        round_trip_server(
            r#"{"type":"motion","t":[0,-12,3],"r":[5,0,0]}"#,
            ServerMessage::Motion {
                t: [0, -12, 3],
                r: [5, 0, 0],
            },
        );
        round_trip_server(
            r#"{"type":"key","key":"t","id":3,"press":"long","down":true}"#,
            ServerMessage::Key {
                key: "t".into(),
                id: 3,
                press: Press::Long,
                down: true,
            },
        );
        round_trip_server(
            r#"{"type":"lock","on":true}"#,
            ServerMessage::Lock { on: true },
        );
    }

    #[test]
    fn client_messages_match_the_spec() {
        let cases = [
            (
                r#"{"type":"hello","api":1,"client":"smectl"}"#,
                ClientMessage::Hello {
                    api: 1,
                    client: "smectl".into(),
                },
            ),
            (
                r#"{"type":"set_lock","on":false}"#,
                ClientMessage::SetLock { on: false },
            ),
            (r#"{"type":"get_state"}"#, ClientMessage::GetState),
            (
                r#"{"type":"set_icon","id":"line","svg":"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path d='M4 20 20 4' stroke='currentColor' stroke-width='2'/></svg>"}"#,
                ClientMessage::SetIcon {
                    id: "line".into(),
                    svg: Some("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path d='M4 20 20 4' stroke='currentColor' stroke-width='2'/></svg>".into()),
                    png: None,
                },
            ),
            (
                r#"{"type":"set_commands","app":{"name":"SolidCraft","icon":"logo"},"commands":[{"id":"sketch.line","label":"Line","icon":"line"},{"id":"edit.undo","label":"Undo"}]}"#,
                ClientMessage::SetCommands {
                    app: App {
                        name: "SolidCraft".into(),
                        icon: Some("logo".into()),
                    },
                    commands: vec![
                        Command {
                            id: "sketch.line".into(),
                            label: "Line".into(),
                            icon: Some("line".into()),
                        },
                        Command {
                            id: "edit.undo".into(),
                            label: "Undo".into(),
                            icon: None,
                        },
                    ],
                },
            ),
            (
                r#"{"type":"set_environments","environments":[{"id":"sketch","name":"Sketch","keys":{"1":"sketch.line","9":"edit.undo"}}]}"#,
                ClientMessage::SetEnvironments {
                    environments: vec![Environment {
                        id: "sketch".into(),
                        name: "Sketch".into(),
                        keys: [("1", "sketch.line"), ("9", "edit.undo")]
                            .into_iter()
                            .map(|(k, v)| (k.into(), v.into()))
                            .collect(),
                    }],
                },
            ),
            (r#"{"type":"focus","on":true}"#, ClientMessage::Focus { on: true }),
            (r#"{"type":"get_screen"}"#, ClientMessage::GetScreen),
            (
                r#"{"type":"set_lcd","brightness":40}"#,
                ClientMessage::SetLcd {
                    brightness: Some(40),
                    text_size: None,
                    theme: None,
                },
            ),
        ];
        for (line, message) in cases {
            assert_eq!(
                serde_json::from_str::<ClientMessage>(line).ok(),
                Some(message.clone())
            );
            assert_eq!(to_line(&message), format!("{line}\n"));
        }
    }

    #[test]
    fn unknown_types_and_fields_are_ignored() {
        assert_eq!(
            serde_json::from_str::<ServerMessage>(r#"{"type":"osd","page":2}"#).ok(),
            Some(ServerMessage::Unknown)
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(r#"{"type":"lock","on":true,"by":"key"}"#).ok(),
            Some(ServerMessage::Lock { on: true })
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"set_osd"}"#).ok(),
            Some(ClientMessage::Unknown)
        );
    }

    #[test]
    fn every_key_has_a_unique_id_and_name() {
        for (index, (id, name)) in KEYS.iter().enumerate() {
            assert_eq!(key_id(name), Some(*id));
            assert_eq!(key_name(*id), *name);
            assert!(
                KEYS[index + 1..]
                    .iter()
                    .all(|(other, n)| other != id && n != name)
            );
        }
        assert_eq!(key_name(LOCK_KEY_ID), "lock");
        assert_eq!(key_name(200), "id200");
    }
}
