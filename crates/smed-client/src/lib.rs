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
//!
//! An application that puts its commands on the keys 1–12 and the LCD (API 2) sends its
//! catalogue and environments, says when its window has the focus, and runs the commands smed
//! reports:
//!
//! ```no_run
//! use smed_client::{App, Client, Command, Environment, ServerMessage};
//!
//! let mut client = Client::connect("example")?;
//! client.set_icon_svg("line", "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'>\
//!     <path d='M4 20 20 4' stroke='currentColor' stroke-width='2'/></svg>")?;
//! client.set_commands(
//!     &App::new("Example"),
//!     &[Command::new("sketch.line", "Line").with_icon("line"), Command::new("edit.undo", "Undo")],
//! )?;
//! client.set_environments(&[Environment::new("sketch", "Sketch")
//!     .with_key("1", "sketch.line")
//!     .with_key("9", "edit.undo")])?;
//! client.set_environment("sketch")?;
//! client.focus(true)?;
//! while let Some(message) = client.next_message()? {
//!     match message {
//!         ServerMessage::Command { id, .. } => println!("run {id}"),
//!         ServerMessage::Error { message } => eprintln!("smed: {message}"),
//!         _ => {}
//!     }
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! An application that reads the socket in its own loop pushes what it reads into a [`Lines`]
//! decoder instead, which hands out whole messages.
//!
//! The enums and structs are `#[non_exhaustive]`, so later API versions can add message types,
//! values, and fields of `App`, `Command`, `Environment` and `DeviceInfo` without breaking your
//! code: match messages with a `_` arm and their fields with `..`, build the structs with their
//! `new` functions, and send client messages through the [`Client`] methods rather than building
//! [`ClientMessage`] values. A new field in a message variant is still a breaking change of this
//! crate.
//!
//! Outside this crate, a struct cannot be built field by field:
//!
//! ```compile_fail
//! let command = smed_client::Command {
//!     id: "edit.undo".into(),
//!     label: "Undo".into(),
//!     icon: None,
//! };
//! ```

// The README's examples are compiled as doctests too, so they cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
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
#[non_exhaustive]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    /// The kernel's HID name.
    pub name: String,
    /// The firmware version from the USB descriptor (bcdDevice), such as `"4.42"`.
    pub firmware: Option<String>,
}

impl DeviceInfo {
    /// A device with USB IDs `vid`:`pid`, the kernel's HID name and the firmware version.
    pub fn new(vid: u16, pid: u16, name: impl Into<String>, firmware: Option<String>) -> Self {
        Self {
            vid,
            pid,
            name: name.into(),
            firmware,
        }
    }
}

/// Whether the device reported a key as pressed briefly or held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Press {
    Short,
    Long,
}

/// Text size on the LCD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TextSize {
    #[default]
    Small,
    Large,
}

/// Colour theme of the LCD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// The application as shown in the LCD's header (`set_commands`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct App {
    pub name: String,
    /// An icon `id` from `set_icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl App {
    /// An application called `name`, without an icon.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            icon: None,
        }
    }

    /// The same application with the icon `icon` (an `id` from `set_icon`).
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

/// One command of the client's catalogue (`set_commands`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Command {
    pub id: String,
    pub label: String,
    /// An icon `id` from `set_icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Command {
    /// The command `id`, shown as `label`, without an icon.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
        }
    }

    /// The same command with the icon `icon` (an `id` from `set_icon`).
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

/// One environment of the client (`set_environments`): a name and which command each key
/// runs there, by key name (`"1"`–`"12"`) and command `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

impl Environment {
    /// The environment `id`, shown as `name`, with no keys assigned.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            keys: BTreeMap::new(),
        }
    }

    /// The same environment with `key` (a key name, `"1"`–`"12"`) running `command` (an `id`
    /// from `set_commands`).
    pub fn with_key(mut self, key: impl Into<String>, command: impl Into<String>) -> Self {
        self.keys.insert(key.into(), command.into());
        self
    }
}

/// What smed sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// API 2: a key the focused client assigned a command to went down: `id` of the command,
    /// `env` the environment it is assigned in, `key` the key's name, `press` short or long.
    Command {
        id: String,
        env: String,
        key: String,
        press: Press,
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

impl ServerMessage {
    /// The PNG of a `screen` message, decoded; `None` for other messages or invalid base64.
    pub fn screen_png(&self) -> Option<Vec<u8>> {
        match self {
            Self::Screen { png } => BASE64.decode(png).ok(),
            _ => None,
        }
    }
}

/// What a client sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// API 2: show environment `id` (from `set_environments`); its keys run their commands.
    SetEnvironment {
        id: String,
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

/// The longest line [`Lines`] accepts from smed. smed's longest, a `screen` with the LCD page as
/// a PNG, stays well below it.
pub const MAX_SERVER_LINE: usize = 1024 * 1024;

/// Splits what smed sends into messages, without doing any I/O: for an application that reads
/// the socket in its own loop (a read timeout, other work between reads). [`Client`] reads
/// through it too.
///
/// ```
/// use smed_client::{Lines, ServerMessage};
///
/// let mut lines = Lines::new();
/// lines.push(br#"{"type":"lock","#);
/// assert!(lines.next_message()?.is_none()); // the rest of the line is still to come
/// lines.push(b"\"on\":true}\n");
/// assert_eq!(lines.next_message()?, Some(ServerMessage::Lock { on: true }));
/// # Ok::<(), smed_client::LineError>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct Lines {
    pending: Vec<u8>,
    /// How many bytes at the start of `pending` are known to hold no newline.
    scanned: usize,
    /// How many bytes the newline search has looked at in all, for the test that each byte is
    /// looked at once.
    #[cfg(test)]
    looked: usize,
}

/// What [`Lines::next_message`] found wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineError {
    /// A line that is not a message (not valid JSON, or a known type with wrong fields). The
    /// line is dropped; the following ones still read.
    #[non_exhaustive]
    Invalid { reason: String },
    /// A line longer than [`MAX_SERVER_LINE`]: everything pushed so far is dropped. That is no
    /// smed; close the connection.
    Overlong,
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { reason } => write!(f, "a line that is not a message: {reason}"),
            Self::Overlong => write!(f, "a line longer than {MAX_SERVER_LINE} bytes"),
        }
    }
}

impl std::error::Error for LineError {}

/// As `InvalidData`, with the [`LineError`] inside (`get_ref` and `downcast_ref` give it back).
impl From<LineError> for io::Error {
    fn from(error: LineError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, error)
    }
}

impl Lines {
    /// A decoder with nothing pushed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes read from smed's socket.
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next complete message, or `None` until more bytes are pushed. Blank lines and
    /// message types this crate does not know are skipped. Each call looks only at bytes it has
    /// not looked at yet, so a long line read in many parts costs no more than read in one.
    pub fn next_message(&mut self) -> Result<Option<ServerMessage>, LineError> {
        loop {
            let unseen = &self.pending[self.scanned..];
            #[cfg(test)]
            {
                self.looked += unseen.len();
            }
            let found = unseen.iter().position(|&byte| byte == b'\n');
            let Some(end) = found.map(|at| self.scanned + at) else {
                self.scanned = self.pending.len();
                if self.pending.len() > MAX_SERVER_LINE {
                    self.pending.clear();
                    self.scanned = 0;
                    return Err(LineError::Overlong);
                }
                return Ok(None);
            };
            if end > MAX_SERVER_LINE {
                self.pending.clear();
                self.scanned = 0;
                return Err(LineError::Overlong);
            }
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.scanned = 0;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<ServerMessage>(&line) {
                Ok(ServerMessage::Unknown) => continue,
                Ok(message) => return Ok(Some(message)),
                Err(error) => {
                    return Err(LineError::Invalid {
                        reason: error.to_string(),
                    });
                }
            }
        }
    }

    /// Whether part of a line is waiting for the rest.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// A connection to smed, after the handshake.
pub struct Client {
    lines: Lines,
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
        let mut this = Self {
            lines: Lines::new(),
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

    /// Sends an SVG icon under `id` (API 2), for `set_commands`. `currentColor` takes the
    /// LCD's text colour.
    pub fn set_icon_svg(&mut self, id: &str, svg: &str) -> io::Result<()> {
        self.send(&ClientMessage::SetIcon {
            id: id.to_owned(),
            svg: Some(svg.to_owned()),
            png: None,
        })
    }

    /// Sends a PNG icon (the file's bytes) under `id` (API 2), for `set_commands`.
    pub fn set_icon_png(&mut self, id: &str, png: &[u8]) -> io::Result<()> {
        self.send(&ClientMessage::SetIcon {
            id: id.to_owned(),
            svg: None,
            png: Some(BASE64.encode(png)),
        })
    }

    /// Sends the application and its command catalogue (API 2); replaces the previous one.
    pub fn set_commands(&mut self, app: &App, commands: &[Command]) -> io::Result<()> {
        self.send(&ClientMessage::SetCommands {
            app: app.clone(),
            commands: commands.to_vec(),
        })
    }

    /// Sends the environments with their key assignments (API 2); replaces the previous ones.
    pub fn set_environments(&mut self, environments: &[Environment]) -> io::Result<()> {
        self.send(&ClientMessage::SetEnvironments {
            environments: environments.to_vec(),
        })
    }

    /// Shows environment `id` (from `set_environments`) on the keys and the LCD (API 2).
    pub fn set_environment(&mut self, id: &str) -> io::Result<()> {
        self.send(&ClientMessage::SetEnvironment { id: id.to_owned() })
    }

    /// Changes the LCD settings for every client until smed restarts (API 2); `None` keeps a
    /// setting. smed answers with `lcd`.
    pub fn set_lcd(
        &mut self,
        brightness: Option<u8>,
        text_size: Option<TextSize>,
        theme: Option<Theme>,
    ) -> io::Result<()> {
        self.send(&ClientMessage::SetLcd {
            brightness,
            text_size,
            theme,
        })
    }

    /// Asks for the current LCD page (API 2); smed answers with `screen`, see
    /// [`ServerMessage::screen_png`].
    pub fn get_screen(&mut self) -> io::Result<()> {
        self.send(&ClientMessage::GetScreen)
    }

    /// The next message, blocking; `None` once smed closed the connection. Unknown message
    /// types are skipped. A [`LineError`] comes as `InvalidData` with the `LineError` inside
    /// (`get_ref()` and `downcast_ref`): after `Invalid` the next call reads on, after `Overlong`
    /// close the connection. With a read
    /// timeout on [`Client::stream`], a timeout is returned as the error it is (`WouldBlock` or
    /// `TimedOut`) and a line it split is kept: the next call goes on with it.
    pub fn next_message(&mut self) -> io::Result<Option<ServerMessage>> {
        let mut chunk = [0_u8; 4096];
        loop {
            if let Some(message) = self.lines.next_message()? {
                return Ok(Some(message));
            }
            match (&self.writer).read(&mut chunk) {
                Ok(0) => return Ok(None),
                Ok(len) => self.lines.push(&chunk[..len]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
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
            r#"{"type":"state","device":null,"lock":false,"lcd":false}"#,
            ServerMessage::State {
                device: None,
                lock: false,
                lcd: Some(false),
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
            r#"{"type":"hello","api":2,"server":"smed 0.2.0"}"#,
            ServerMessage::Hello {
                api: 2,
                server: "smed 0.2.0".into(),
            },
        );
        round_trip_server(
            r#"{"type":"lcd","owner":true,"brightness":40,"text_size":"small","theme":"light"}"#,
            ServerMessage::Lcd {
                owner: true,
                brightness: 40,
                text_size: TextSize::Small,
                theme: Theme::Light,
            },
        );
        round_trip_server(
            r#"{"type":"command","id":"sketch.line","env":"sketch","key":"1","press":"short"}"#,
            ServerMessage::Command {
                id: "sketch.line".into(),
                env: "sketch".into(),
                key: "1".into(),
                press: Press::Short,
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
            (
                r#"{"type":"hello","api":2,"client":"solidcraft"}"#,
                ClientMessage::Hello {
                    api: 2,
                    client: "solidcraft".into(),
                },
            ),
            (r#"{"type":"focus","on":true}"#, ClientMessage::Focus { on: true }),
            (r#"{"type":"get_screen"}"#, ClientMessage::GetScreen),
            (
                r#"{"type":"set_environment","id":"part"}"#,
                ClientMessage::SetEnvironment { id: "part".into() },
            ),
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

    /// A client on one end of a socket pair, and the other end as smed.
    fn client_pair() -> (Client, std::io::BufReader<UnixStream>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let client = Client {
            lines: Lines::new(),
            writer: ours,
            api: API_VERSION,
            server: String::new(),
        };
        (client, std::io::BufReader::new(theirs))
    }

    fn sent(smed: &mut std::io::BufReader<UnixStream>) -> String {
        use std::io::BufRead;
        let mut line = String::new();
        smed.read_line(&mut line).unwrap();
        line.trim_end().to_owned()
    }

    // The same lines as in client_messages_match_the_spec, from the Client methods and the
    // constructors.
    #[test]
    fn client_methods_send_the_spec_lines() {
        let (mut client, mut smed) = client_pair();
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path d='M4 20 20 4' stroke='currentColor' stroke-width='2'/></svg>";
        client.set_icon_svg("line", svg).unwrap();
        assert_eq!(
            sent(&mut smed),
            format!(r#"{{"type":"set_icon","id":"line","svg":"{svg}"}}"#)
        );
        client
            .set_commands(
                &App::new("SolidCraft").with_icon("logo"),
                &[
                    Command::new("sketch.line", "Line").with_icon("line"),
                    Command::new("edit.undo", "Undo"),
                ],
            )
            .unwrap();
        assert_eq!(
            sent(&mut smed),
            r#"{"type":"set_commands","app":{"name":"SolidCraft","icon":"logo"},"commands":[{"id":"sketch.line","label":"Line","icon":"line"},{"id":"edit.undo","label":"Undo"}]}"#
        );
        client
            .set_environments(&[Environment::new("sketch", "Sketch")
                .with_key("9", "edit.undo")
                .with_key("1", "sketch.line")])
            .unwrap();
        assert_eq!(
            sent(&mut smed),
            r#"{"type":"set_environments","environments":[{"id":"sketch","name":"Sketch","keys":{"1":"sketch.line","9":"edit.undo"}}]}"#
        );
        client.set_environment("part").unwrap();
        assert_eq!(sent(&mut smed), r#"{"type":"set_environment","id":"part"}"#);
        client.focus(true).unwrap();
        assert_eq!(sent(&mut smed), r#"{"type":"focus","on":true}"#);
        client.set_lcd(Some(40), None, None).unwrap();
        assert_eq!(sent(&mut smed), r#"{"type":"set_lcd","brightness":40}"#);
        client.get_screen().unwrap();
        assert_eq!(sent(&mut smed), r#"{"type":"get_screen"}"#);
    }

    #[test]
    fn png_icons_and_screens_are_base64() {
        const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
        let (mut client, mut smed) = client_pair();
        client.set_icon_png("logo", PNG_SIGNATURE).unwrap();
        assert_eq!(
            sent(&mut smed),
            r#"{"type":"set_icon","id":"logo","png":"iVBORw0KGgo="}"#
        );
        let screen = ServerMessage::Screen {
            png: "iVBORw0KGgo=".into(),
        };
        assert_eq!(screen.screen_png().as_deref(), Some(PNG_SIGNATURE));
        let broken = ServerMessage::Screen { png: "*".into() };
        assert_eq!(broken.screen_png(), None);
        assert_eq!(ServerMessage::Lock { on: true }.screen_png(), None);
    }

    #[test]
    fn constructors_leave_the_optional_parts_empty() {
        assert_eq!(
            App::new("Example"),
            App {
                name: "Example".into(),
                icon: None
            }
        );
        assert_eq!(Command::new("a", "A").icon, None);
        assert!(Environment::new("e", "E").keys.is_empty());
        assert_eq!(
            DeviceInfo::new(0x256f, 0xc633, "SpaceMouse", Some("4.42".into())),
            DeviceInfo {
                vid: 0x256f,
                pid: 0xc633,
                name: "SpaceMouse".into(),
                firmware: Some("4.42".into()),
            }
        );
    }

    #[test]
    fn a_line_split_by_a_read_timeout_arrives_whole() {
        let (mut client, smed) = client_pair();
        let mut smed = smed.into_inner();
        client
            .stream()
            .set_read_timeout(Some(std::time::Duration::from_millis(20)))
            .unwrap();
        smed.write_all(br#"{"type":"lock","#).unwrap();
        let timeout = client.next_message().unwrap_err();
        assert!(matches!(
            timeout.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        smed.write_all(b"\"on\":true}\n").unwrap();
        assert_eq!(
            client.next_message().unwrap(),
            Some(ServerMessage::Lock { on: true })
        );
        drop(smed);
        assert_eq!(client.next_message().unwrap(), None);
    }

    #[test]
    fn lines_hands_out_whole_messages_in_order() {
        let mut lines = Lines::new();
        assert!(lines.is_empty());
        lines.push(b"\n  \n{\"type\":\"lock\",\"on\":true}\n{\"type\":\"osd\"}\n{\"type\":\"lo");
        assert_eq!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Lock { on: true })
        );
        // The unknown type is skipped; the rest of the last line is still to come.
        assert_eq!(lines.next_message().unwrap(), None);
        assert!(!lines.is_empty());
        lines.push(b"ck\",\"on\":false}\n");
        assert_eq!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Lock { on: false })
        );
        assert!(lines.is_empty());
    }

    #[test]
    fn lines_reports_invalid_json_and_reads_on() {
        let mut lines = Lines::new();
        lines.push(b"not json\n{\"type\":\"lock\",\"on\":true}\n");
        let error = lines.next_message().unwrap_err();
        assert!(matches!(error, LineError::Invalid { .. }), "{error:?}");
        assert_eq!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Lock { on: true })
        );
    }

    #[test]
    fn lines_refuses_an_overlong_line() {
        let mut lines = Lines::new();
        // Exactly the limit, then the newline: accepted.
        let mut longest = br#"{"type":"screen","png":""#.to_vec();
        longest.resize(MAX_SERVER_LINE - 2, b'A');
        longest.extend_from_slice(b"\"}");
        assert_eq!(longest.len(), MAX_SERVER_LINE);
        lines.push(&longest);
        lines.push(b"\n");
        assert!(matches!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Screen { .. })
        ));
        // One byte more, still without a newline: refused, and everything dropped.
        lines.push(&vec![b'x'; MAX_SERVER_LINE + 1]);
        assert_eq!(lines.next_message().unwrap_err(), LineError::Overlong);
        assert!(lines.is_empty());
        // One byte more with its newline in the same push: refused too.
        let mut over = vec![b' '; MAX_SERVER_LINE + 1];
        over.push(b'\n');
        lines.push(&over);
        assert_eq!(lines.next_message().unwrap_err(), LineError::Overlong);
    }

    #[test]
    fn lines_looks_at_each_byte_once() {
        // The longest line, pushed in 4 KiB parts: each call looks only at the new part.
        let mut longest = br#"{"type":"screen","png":""#.to_vec();
        longest.resize(MAX_SERVER_LINE - 2, b'A');
        longest.extend_from_slice(b"\"}\n");
        let mut lines = Lines::new();
        let mut handed_out = 0;
        for part in longest.chunks(4096) {
            lines.push(part);
            if lines.scanned < lines.pending.len() {
                let before = lines.scanned;
                let message = lines.next_message().unwrap();
                if message.is_none() {
                    assert_eq!(lines.scanned, lines.pending.len());
                    assert!(before <= lines.scanned);
                } else {
                    assert!(matches!(message, Some(ServerMessage::Screen { .. })));
                    assert!(lines.is_empty());
                    handed_out += 1;
                }
            }
        }
        assert_eq!(handed_out, 1);
        // Each byte once: a search from the start of the buffer at every call would look at
        // about 128 times as many.
        assert_eq!(lines.looked, longest.len());
        // After a line, the bytes behind it are looked at afresh.
        lines.push(b"{\"type\":\"lock\",\"on\":true}\n{\"type\":\"lock\",\"on\":false}\n");
        assert_eq!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Lock { on: true })
        );
        assert_eq!(
            lines.next_message().unwrap(),
            Some(ServerMessage::Lock { on: false })
        );
    }

    #[test]
    fn client_reports_line_errors_as_invalid_data_with_the_line_error_inside() {
        let (mut client, smed) = client_pair();
        let mut smed = smed.into_inner();
        smed.write_all(b"not json\n").unwrap();
        let error = client.next_message().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let inner = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<LineError>());
        assert!(
            matches!(inner, Some(LineError::Invalid { .. })),
            "{error:?}"
        );
        smed.write_all(b"{\"type\":\"lock\",\"on\":true}\n")
            .unwrap();
        assert_eq!(
            client.next_message().unwrap(),
            Some(ServerMessage::Lock { on: true })
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
