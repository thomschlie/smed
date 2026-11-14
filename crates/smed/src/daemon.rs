//! The running daemon: the state shared between the device thread and one thread per client.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use smed_client::{API_VERSION, ClientMessage, DeviceInfo, ServerMessage, to_line};

use crate::core::{Action, Core};
use crate::device::HidDevice;
use crate::init;

/// How long a write to a client may block before the client is dropped: a client that stops
/// reading must not stall the device.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// The largest input report the device sends is 13 bytes; room to spare.
const REPORT_BUFFER: usize = 64;

pub struct Shared {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    core: Core,
    device: Option<DeviceInfo>,
    handle: Option<Arc<dyn HidDevice>>,
    clients: Vec<Client>,
    next_client: u64,
}

struct Client {
    id: u64,
    stream: UnixStream,
}

impl State {
    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Output(report) => {
                    if let Some(handle) = &self.handle
                        && let Err(error) = handle.write_output(&report)
                    {
                        eprintln!("smed: writing output report {report:02x?}: {error}");
                    }
                }
                Action::Send(message) => self.broadcast(&message),
            }
        }
    }

    fn broadcast(&mut self, message: &ServerMessage) {
        let line = to_line(message);
        self.clients
            .retain_mut(|client| client.stream.write_all(line.as_bytes()).is_ok());
    }

    fn state_message(&self) -> ServerMessage {
        ServerMessage::State {
            device: self.device.clone(),
            lock: self.core.lock(),
        }
    }
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves the state usable: every field stays valid.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs `device` until it goes away: the init sequence, then every report through the core.
    /// Returns the read error that ended it, or `Ok` at end of file.
    pub fn run_device(&self, device: Arc<dyn HidDevice>, info: DeviceInfo) -> io::Result<()> {
        {
            let mut state = self.state();
            let led = state.core.led_report();
            init::run(device.as_ref(), &led)?;
            state.handle = Some(device.clone());
            state.device = Some(info.clone());
            state.broadcast(&ServerMessage::Device { device: Some(info) });
        }
        let mut buf = [0_u8; REPORT_BUFFER];
        let result = loop {
            match device.read_report(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(len) => {
                    let mut state = self.state();
                    let actions = state.core.on_report(&buf[..len]);
                    state.apply(actions);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
        };
        let mut state = self.state();
        state.handle = None;
        state.device = None;
        let actions = state.core.device_gone();
        state.apply(actions);
        state.broadcast(&ServerMessage::Device { device: None });
        result
    }

    /// Talks to one client until it disconnects.
    pub fn serve_client(&self, stream: UnixStream) -> io::Result<()> {
        stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))?;
        let mut writer = stream.try_clone()?;
        let mut lines = BufReader::new(stream).lines();

        let Some(first) = lines.next().transpose()? else {
            return Ok(());
        };
        let (api, name) = match serde_json::from_str::<ClientMessage>(&first) {
            Ok(ClientMessage::Hello { api, client }) if api >= 1 => (api.min(API_VERSION), client),
            Ok(ClientMessage::Hello { api, .. }) => {
                return send_error(&mut writer, &format!("unsupported API version {api}"));
            }
            _ => return send_error(&mut writer, "expected hello"),
        };
        let id = {
            let mut state = self.state();
            let hello = ServerMessage::Hello {
                api,
                server: format!("smed {}", env!("CARGO_PKG_VERSION")),
            };
            writer.write_all(to_line(&hello).as_bytes())?;
            writer.write_all(to_line(&state.state_message()).as_bytes())?;
            state.next_client += 1;
            let id = state.next_client;
            state.clients.push(Client {
                id,
                stream: writer.try_clone()?,
            });
            id
        };
        eprintln!("smed: client {id} ({name}) connected, API {api}");

        let result: io::Result<()> = (|| {
            for line in lines {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let mut state = self.state();
                match serde_json::from_str::<ClientMessage>(&line) {
                    Ok(ClientMessage::SetLock { on }) => {
                        let actions = state.core.set_lock(on);
                        state.apply(actions);
                    }
                    Ok(ClientMessage::GetState) => {
                        writer.write_all(to_line(&state.state_message()).as_bytes())?;
                    }
                    Ok(ClientMessage::Hello { .. } | ClientMessage::Unknown) => {}
                    Err(error) => {
                        let message = ServerMessage::Error {
                            message: format!("invalid message: {error}"),
                        };
                        writer.write_all(to_line(&message).as_bytes())?;
                    }
                }
            }
            Ok(())
        })();
        self.state().clients.retain(|client| client.id != id);
        eprintln!("smed: client {id} ({name}) disconnected");
        result
    }
}

fn send_error(writer: &mut UnixStream, message: &str) -> io::Result<()> {
    let error = ServerMessage::Error {
        message: message.to_owned(),
    };
    writer.write_all(to_line(&error).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Fake, Written};
    use smed_client::Press;

    fn read(reader: &mut BufReader<UnixStream>) -> ServerMessage {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn connect(shared: &Arc<Shared>) -> (UnixStream, BufReader<UnixStream>) {
        let (client, server) = UnixStream::pair().unwrap();
        let shared = shared.clone();
        std::thread::spawn(move || shared.serve_client(server));
        let reader = BufReader::new(client.try_clone().unwrap());
        (client, reader)
    }

    fn info() -> DeviceInfo {
        DeviceInfo {
            vid: 0x256f,
            pid: 0xc633,
            name: "SpaceMouse Enterprise".into(),
            firmware: Some("4.42".into()),
        }
    }

    fn key(name: &str, id: u8, down: bool) -> ServerMessage {
        ServerMessage::Key {
            key: name.into(),
            id,
            press: Press::Short,
            down,
        }
    }

    #[test]
    fn a_client_sees_handshake_device_keys_lock_and_removal() {
        let shared = Shared::new();
        let (mut client, mut reader) = connect(&shared);
        client
            .write_all(b"{\"type\":\"hello\",\"api\":7,\"client\":\"test\"}\n")
            .unwrap();
        assert_eq!(
            read(&mut reader),
            ServerMessage::Hello {
                api: 1,
                server: format!("smed {}", env!("CARGO_PKG_VERSION"))
            }
        );
        assert_eq!(
            read(&mut reader),
            ServerMessage::State {
                device: None,
                lock: false
            }
        );

        let mut lock = vec![0_u8; 13];
        lock[0] = 0x1c;
        lock[1] = 27;
        let device = Arc::new(Fake::with_reports([
            lock,
            vec![0x01, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0],
        ]));
        shared.run_device(device.clone(), info()).unwrap();

        assert_eq!(
            read(&mut reader),
            ServerMessage::Device {
                device: Some(info())
            }
        );
        assert_eq!(read(&mut reader), key("lock", 27, true));
        assert_eq!(read(&mut reader), ServerMessage::Lock { on: true });
        assert_eq!(
            read(&mut reader),
            ServerMessage::Motion {
                t: [1, 2, 3],
                r: [0, 0, 0]
            }
        );
        assert_eq!(read(&mut reader), key("lock", 27, false));
        assert_eq!(read(&mut reader), ServerMessage::Device { device: None });

        let written = device.written();
        assert_eq!(
            written[2],
            Written::Output(vec![0x04, 0x01]),
            "init LED: unlocked"
        );
        assert_eq!(written.last(), Some(&Written::Output(vec![0x04, 0x03])));

        client
            .write_all(b"{\"type\":\"set_lock\",\"on\":false}\n{\"type\":\"get_state\"}\n")
            .unwrap();
        assert_eq!(read(&mut reader), ServerMessage::Lock { on: false });
        assert_eq!(
            read(&mut reader),
            ServerMessage::State {
                device: None,
                lock: false
            }
        );
    }

    #[test]
    fn no_hello_no_service() {
        let shared = Shared::new();
        let (mut client, mut reader) = connect(&shared);
        client.write_all(b"{\"type\":\"get_state\"}\n").unwrap();
        assert_eq!(
            read(&mut reader),
            ServerMessage::Error {
                message: "expected hello".into()
            }
        );
        let mut rest = String::new();
        assert_eq!(
            reader.read_line(&mut rest).unwrap(),
            0,
            "smed closed the connection"
        );
    }

    #[test]
    fn a_bad_line_gets_an_error_and_the_connection_stays() {
        let shared = Shared::new();
        let (mut client, mut reader) = connect(&shared);
        client
            .write_all(b"{\"type\":\"hello\",\"api\":1,\"client\":\"t\"}\nnot json\n{\"type\":\"get_state\"}\n")
            .unwrap();
        read(&mut reader);
        read(&mut reader);
        assert!(matches!(read(&mut reader), ServerMessage::Error { .. }));
        assert!(matches!(read(&mut reader), ServerMessage::State { .. }));
    }
}
