//! The running daemon: the state shared between the device thread, the LCD thread and one
//! thread per client.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use smed_client::{API_VERSION, ClientMessage, DeviceInfo, MAX_MESSAGE, ServerMessage, to_line};

use crate::config::LcdSettings;
use crate::core::{Action, Core};
use crate::device::HidDevice;
use crate::frame;
use crate::init;
use crate::lcd::Lcd;
use crate::render::{self, Icon};
use crate::usb::{self, LcdSink};

/// How long a write to a client may block before the client is dropped: a client that stops
/// reading must not stall the device.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// The largest input report the device sends is 13 bytes; room to spare.
const REPORT_BUFFER: usize = 64;

/// Changes to the page within this time make one frame (a client sends several messages at
/// start).
const LCD_DEBOUNCE: Duration = Duration::from_millis(50);

/// After a failed frame, try again this often, this long apart (a device that was just
/// re-plugged can time out for a moment).
const LCD_RETRIES: u32 = 3;
const LCD_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Feature report: LCD brightness 0-100 (docs/protocol.md).
const REPORT_BRIGHTNESS: u8 = 0x11;

/// The first API version with the LCD messages.
const API_LCD: u32 = 2;

pub struct Shared {
    state: Mutex<State>,
    /// Wakes the LCD thread when `State::lcd_dirty` is set.
    lcd_wake: Condvar,
}

#[derive(Default)]
struct State {
    core: Core,
    device: Option<DeviceInfo>,
    handle: Option<Arc<dyn HidDevice>>,
    clients: Vec<Client>,
    next_client: u64,
    lcd: Lcd,
    /// The LCD's USB interface, while the device is there and it could be claimed.
    lcd_sink: Option<Arc<dyn LcdSink>>,
    /// The LCD needs a new frame (and brightness).
    lcd_dirty: bool,
}

struct Client {
    id: u64,
    api: u32,
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

    fn state_message(&self, api: u32) -> ServerMessage {
        ServerMessage::State {
            device: self.device.clone(),
            lock: self.core.lock(),
            lcd: (api >= API_LCD).then_some(self.lcd_sink.is_some()),
        }
    }

    fn lcd_message(&self, client: u64) -> ServerMessage {
        let settings = self.lcd.settings();
        ServerMessage::Lcd {
            owner: self.lcd.owner() == Some(client),
            brightness: settings.brightness,
            text_size: settings.text_size,
            theme: settings.theme,
        }
    }

    /// Sends `lcd` to every client that speaks API 2: after a change of owner or settings.
    fn broadcast_lcd(&mut self) {
        let lines: Vec<(u64, String)> = self
            .clients
            .iter()
            .filter(|client| client.api >= API_LCD)
            .map(|client| (client.id, to_line(&self.lcd_message(client.id))))
            .collect();
        self.clients.retain_mut(|client| {
            lines
                .iter()
                .find(|(id, _)| *id == client.id)
                .is_none_or(|(_, line)| client.stream.write_all(line.as_bytes()).is_ok())
        });
    }
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Self::with_settings(LcdSettings::default())
    }

    /// With the LCD settings from the config file.
    pub fn with_settings(settings: LcdSettings) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                lcd: Lcd::new(settings),
                ..State::default()
            }),
            lcd_wake: Condvar::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves the state usable: every field stays valid.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks the LCD thread for a new frame.
    fn redraw(&self, state: &mut State) {
        state.lcd_dirty = true;
        self.lcd_wake.notify_all();
    }

    /// Runs `device` until it goes away: the init sequence, then every report through the core.
    /// `lcd` is its LCD interface, if it could be claimed. Returns the read error that ended
    /// it, or `Ok` at end of file.
    pub fn run_device(
        &self,
        device: Arc<dyn HidDevice>,
        info: DeviceInfo,
        lcd: Option<Arc<dyn LcdSink>>,
    ) -> io::Result<()> {
        {
            let mut state = self.state();
            let led = state.core.led_report();
            init::run(device.as_ref(), &led)?;
            state.handle = Some(device.clone());
            state.device = Some(info.clone());
            state.lcd_sink = lcd;
            self.redraw(&mut state);
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
        state.lcd_sink = None;
        let actions = state.core.device_gone();
        state.apply(actions);
        state.broadcast(&ServerMessage::Device { device: None });
        result
    }

    /// The LCD thread: waits for a change, lets more changes arrive for [`LCD_DEBOUNCE`], then
    /// draws; retries a failed frame up to [`LCD_RETRIES`] times. Never returns.
    pub fn run_lcd(&self) {
        let mut failures = 0;
        loop {
            {
                let mut state = self.state();
                while !state.lcd_dirty {
                    state = self
                        .lcd_wake
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
            std::thread::sleep(LCD_DEBOUNCE);
            if self.draw() {
                failures = 0;
            } else if failures < LCD_RETRIES {
                failures += 1;
                std::thread::sleep(LCD_RETRY_DELAY);
                let mut state = self.state();
                self.redraw(&mut state);
            } else {
                failures = 0;
            }
        }
    }

    /// Sends the brightness and the current page to the device, if there is one. Drawing
    /// happens outside the lock. False when a write to the device failed.
    pub fn draw(&self) -> bool {
        let (page, settings, handle, sink) = {
            let mut state = self.state();
            state.lcd_dirty = false;
            (
                state.lcd.page(),
                state.lcd.settings(),
                state.handle.clone(),
                state.lcd_sink.clone(),
            )
        };
        let mut ok = true;
        // 3DxWare sends the brightness with every page; without it the LCD stays dark.
        if let Some(handle) = handle
            && let Err(error) = handle.set_feature(&[REPORT_BRIGHTNESS, settings.brightness])
        {
            eprintln!("smed: setting LCD brightness: {error}");
            ok = false;
        }
        if let Some(sink) = sink {
            let canvas = render::render(&page, &settings);
            let frame = frame::encode(canvas.data());
            if let Err(error) = usb::send(sink.as_ref(), &frame) {
                eprintln!("smed: sending an LCD frame: {error}");
                ok = false;
            }
        }
        ok
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
            state.next_client += 1;
            let id = state.next_client;
            writer.write_all(to_line(&hello).as_bytes())?;
            writer.write_all(to_line(&state.state_message(api)).as_bytes())?;
            if api >= API_LCD {
                writer.write_all(to_line(&state.lcd_message(id)).as_bytes())?;
            }
            state.clients.push(Client {
                id,
                api,
                stream: writer.try_clone()?,
            });
            state.lcd.connect(id, &name);
            id
        };
        eprintln!("smed: client {id} ({name}) connected, API {api}");

        let result: io::Result<()> = (|| {
            for line in lines {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                if line.len() > MAX_MESSAGE {
                    let message = ServerMessage::Error {
                        message: format!("message longer than {MAX_MESSAGE} bytes"),
                    };
                    writer.write_all(to_line(&message).as_bytes())?;
                    continue;
                }
                let mut state = self.state();
                match serde_json::from_str::<ClientMessage>(&line) {
                    Ok(ClientMessage::SetLock { on }) => {
                        let actions = state.core.set_lock(on);
                        state.apply(actions);
                    }
                    Ok(ClientMessage::GetState) => {
                        writer.write_all(to_line(&state.state_message(api)).as_bytes())?;
                    }
                    Ok(message) if api >= API_LCD => {
                        if let Err(message) = self.lcd_request(&mut state, id, message) {
                            let error = ServerMessage::Error { message };
                            writer.write_all(to_line(&error).as_bytes())?;
                        }
                    }
                    // API 1 knows none of the other messages.
                    Ok(_) => {}
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
        let mut state = self.state();
        state.clients.retain(|client| client.id != id);
        if state.lcd.disconnect(id) {
            self.redraw(&mut state);
            state.broadcast_lcd();
        }
        drop(state);
        eprintln!("smed: client {id} ({name}) disconnected");
        result
    }

    /// One API 2 request of client `id`; `Err` is the message for an `error` reply.
    fn lcd_request(
        &self,
        state: &mut State,
        id: u64,
        message: ClientMessage,
    ) -> Result<(), String> {
        let shown = match message {
            ClientMessage::SetIcon {
                id: icon_id,
                svg,
                png,
            } => {
                let icon = match (svg, png) {
                    (Some(svg), None) => Icon::from_svg(&svg),
                    (None, Some(png)) => Icon::from_png_base64(&png),
                    _ => Err("set_icon needs exactly one of svg and png".into()),
                }
                .map_err(|e| format!("icon {icon_id}: {e}"))?;
                state.lcd.set_icon(id, icon_id, icon)
            }
            ClientMessage::SetCommands { app, commands } => {
                state.lcd.set_commands(id, app, commands)
            }
            ClientMessage::SetEnvironments { environments } => {
                state.lcd.set_environments(id, environments)
            }
            ClientMessage::Focus { on } => {
                let changed = state.lcd.focus(id, on);
                if changed {
                    state.broadcast_lcd();
                }
                changed
            }
            ClientMessage::SetLcd {
                brightness,
                text_size,
                theme,
            } => {
                let changed = state.lcd.set(brightness, text_size, theme);
                if changed {
                    state.broadcast_lcd();
                } else {
                    // The answer to set_lcd, also when nothing changed.
                    let line = to_line(&state.lcd_message(id));
                    if let Some(client) = state.clients.iter_mut().find(|c| c.id == id) {
                        let _ = client.stream.write_all(line.as_bytes());
                    }
                }
                changed
            }
            ClientMessage::Hello { .. }
            | ClientMessage::SetLock { .. }
            | ClientMessage::GetState
            | ClientMessage::Unknown => false,
        };
        if shown {
            self.redraw(state);
        }
        Ok(())
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
    use crate::usb::FakeLcd;
    use smed_client::{Press, TextSize, Theme};

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

    fn lcd(owner: bool, brightness: u8) -> ServerMessage {
        ServerMessage::Lcd {
            owner,
            brightness,
            text_size: TextSize::Small,
            theme: Theme::Dark,
        }
    }

    /// Connects with `api`, reads the handshake; returns the client after it.
    fn hello(shared: &Arc<Shared>, api: u32, name: &str) -> (UnixStream, BufReader<UnixStream>) {
        let (mut client, mut reader) = connect(shared);
        writeln!(
            client,
            r#"{{"type":"hello","api":{api},"client":"{name}"}}"#
        )
        .unwrap();
        assert!(matches!(read(&mut reader), ServerMessage::Hello { .. }));
        assert!(matches!(read(&mut reader), ServerMessage::State { .. }));
        if api >= 2 {
            assert!(matches!(read(&mut reader), ServerMessage::Lcd { .. }));
        }
        (client, reader)
    }

    /// Sends `lines`, then `get_state`, and reads up to its answer: then smed has handled
    /// them all. Returns what came before the `state`.
    fn exchange(
        client: &mut UnixStream,
        reader: &mut BufReader<UnixStream>,
        lines: &[&str],
    ) -> Vec<ServerMessage> {
        for line in lines {
            writeln!(client, "{line}").unwrap();
        }
        writeln!(client, r#"{{"type":"get_state"}}"#).unwrap();
        let mut seen = Vec::new();
        loop {
            match read(reader) {
                ServerMessage::State { .. } => return seen,
                other => seen.push(other),
            }
        }
    }

    /// A device and an LCD interface that stay attached, without a device thread.
    fn attach(shared: &Arc<Shared>) -> (Arc<Fake>, Arc<FakeLcd>) {
        let hid = Arc::new(Fake::default());
        let sink = Arc::new(FakeLcd::default());
        let mut state = shared.state();
        state.handle = Some(hid.clone());
        state.lcd_sink = Some(sink.clone());
        (hid, sink)
    }

    const ICON: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 4 4'><rect width='4' height='4' fill='currentColor'/></svg>"#;

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
                api: 2,
                server: format!("smed {}", env!("CARGO_PKG_VERSION"))
            }
        );
        assert_eq!(
            read(&mut reader),
            ServerMessage::State {
                device: None,
                lock: false,
                lcd: Some(false)
            }
        );
        assert_eq!(read(&mut reader), lcd(false, 100));

        let mut lock = vec![0_u8; 13];
        lock[0] = 0x1c;
        lock[1] = 27;
        let device = Arc::new(Fake::with_reports([
            lock,
            vec![0x01, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0],
        ]));
        shared.run_device(device.clone(), info(), None).unwrap();

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
                lock: false,
                lcd: Some(false)
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

    #[test]
    fn an_api_1_client_sees_nothing_of_the_lcd() {
        let shared = Shared::new();
        let (mut client, mut reader) = connect(&shared);
        client
            .write_all(b"{\"type\":\"hello\",\"api\":1,\"client\":\"old\"}\n")
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains(r#""api":1"#));
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            line, "{\"type\":\"state\",\"device\":null,\"lock\":false}\n",
            "the v1 state line, without lcd"
        );
        let seen = exchange(
            &mut client,
            &mut reader,
            &[
                r#"{"type":"set_lcd","brightness":10}"#,
                r#"{"type":"focus","on":true}"#,
            ],
        );
        assert!(seen.is_empty(), "ignored: {seen:?}");
        assert_eq!(shared.state().lcd.settings().brightness, 100);
        assert_eq!(shared.state().lcd.owner(), None);
    }

    #[test]
    fn the_focused_client_page_reaches_the_lcd() {
        let shared = Shared::new();
        let (hid, sink) = attach(&shared);
        let (mut client, mut reader) = hello(&shared, 2, "demo");
        let seen = exchange(
            &mut client,
            &mut reader,
            &[
                &format!(r#"{{"type":"set_icon","id":"pen","svg":"{ICON}"}}"#),
                r#"{"type":"set_commands","app":{"name":"Demo","icon":"pen"},"commands":[{"id":"line","label":"Line","icon":"pen"}]}"#,
                r#"{"type":"set_environments","environments":[{"id":"sketch","name":"Sketch","keys":{"1":"line"}}]}"#,
            ],
        );
        assert!(seen.is_empty(), "{seen:?}");
        assert!(!shared.state().lcd_dirty, "not shown without focus");

        let seen = exchange(&mut client, &mut reader, &[r#"{"type":"focus","on":true}"#]);
        assert_eq!(seen, vec![lcd(true, 100)]);
        assert!(shared.state().lcd_dirty);
        let page = shared.state().lcd.page();
        assert_eq!(page.title, "Demo");
        assert_eq!(page.tiles[0].as_ref().unwrap().label, "Line");

        assert!(shared.draw());
        assert!(!shared.state().lcd_dirty);
        assert_eq!(hid.written(), vec![Written::Feature(vec![0x11, 100])]);
        let transfers = sink.transfers();
        assert_eq!(transfers.len(), 2, "header and image");
        assert_eq!(transfers[0][0], 0x11);
        assert_eq!(
            u32::from_le_bytes(transfers[0][2..6].try_into().unwrap()) as usize,
            transfers[1].len()
        );
    }

    #[test]
    fn set_lcd_changes_settings_for_everyone_and_always_answers() {
        let shared = Shared::new();
        let (hid, _sink) = attach(&shared);
        let (mut a, mut a_reader) = hello(&shared, 2, "a");
        let (mut b, mut b_reader) = hello(&shared, 2, "b");
        let seen = exchange(
            &mut a,
            &mut a_reader,
            &[r#"{"type":"set_lcd","brightness":30,"theme":"light"}"#],
        );
        let changed = ServerMessage::Lcd {
            owner: false,
            brightness: 30,
            text_size: TextSize::Small,
            theme: Theme::Light,
        };
        assert_eq!(seen, vec![changed.clone()]);
        assert_eq!(exchange(&mut b, &mut b_reader, &[]), vec![changed.clone()]);
        assert!(shared.state().lcd_dirty, "settings redraw");
        shared.draw();
        assert_eq!(hid.written(), vec![Written::Feature(vec![0x11, 30])]);

        let seen = exchange(
            &mut a,
            &mut a_reader,
            &[r#"{"type":"set_lcd","brightness":30}"#],
        );
        assert_eq!(seen, vec![changed], "answered without a change");
        assert!(
            exchange(&mut b, &mut b_reader, &[]).is_empty(),
            "others hear nothing"
        );
        assert!(!shared.state().lcd_dirty);
    }

    #[test]
    fn focus_moves_between_clients_and_goes_with_a_disconnect() {
        let shared = Shared::new();
        let (mut a, mut a_reader) = hello(&shared, 2, "a");
        let (mut b, mut b_reader) = hello(&shared, 2, "b");
        assert_eq!(
            exchange(&mut a, &mut a_reader, &[r#"{"type":"focus","on":true}"#]),
            vec![lcd(true, 100)]
        );
        assert_eq!(exchange(&mut b, &mut b_reader, &[]), vec![lcd(false, 100)]);
        assert_eq!(
            exchange(&mut b, &mut b_reader, &[r#"{"type":"focus","on":true}"#]),
            vec![lcd(true, 100)]
        );
        assert_eq!(exchange(&mut a, &mut a_reader, &[]), vec![lcd(false, 100)]);
        shared.state().lcd_dirty = false;
        drop(b);
        drop(b_reader);
        // a hears that nobody has the LCD now.
        assert_eq!(read(&mut a_reader), lcd(false, 100));
        assert_eq!(shared.state().lcd.owner(), None);
        assert!(shared.state().lcd_dirty, "back to smed's page");
    }

    #[test]
    fn bad_icons_and_long_lines_get_an_error_and_the_connection_stays() {
        let shared = Shared::new();
        let (mut client, mut reader) = hello(&shared, 2, "t");
        let long = format!(
            r#"{{"type":"set_icon","id":"big","png":"{}"}}"#,
            "A".repeat(MAX_MESSAGE)
        );
        let seen = exchange(
            &mut client,
            &mut reader,
            &[
                r#"{"type":"set_icon","id":"x","svg":"<svg"}"#,
                r#"{"type":"set_icon","id":"x"}"#,
                &format!(r#"{{"type":"set_icon","id":"x","svg":"{ICON}","png":"AAAA"}}"#),
                &long,
                r#"{"type":"set_lcd","theme":"blue"}"#,
            ],
        );
        assert_eq!(seen.len(), 5, "{seen:?}");
        for message in &seen {
            assert!(
                matches!(message, ServerMessage::Error { .. }),
                "{message:?}"
            );
        }
        if let ServerMessage::Error { message } = &seen[0] {
            assert!(message.starts_with("icon x: invalid SVG"), "{message}");
        }
    }

    struct BrokenLcd;

    impl LcdSink for BrokenLcd {
        fn bulk_out(&self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        }
    }

    #[test]
    fn a_failed_frame_is_reported_for_a_retry() {
        let shared = Shared::new();
        shared.state().lcd_sink = Some(Arc::new(BrokenLcd));
        assert!(!shared.draw());
        shared.state().lcd_sink = Some(Arc::new(FakeLcd::default()));
        assert!(shared.draw());
    }
}
