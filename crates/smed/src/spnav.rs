//! The spacenavd/libspnav socket protocol (docs/spnav.md), so that Blender, FreeCAD and other
//! libspnav clients work unchanged. Pure logic: bytes from a client go in, bytes for it come
//! out; the daemon's messages go in, events for every client come out.
//!
//! The wire format follows libspnav 1.2 (`proto.h`, `spnav.c`, BSD-3-Clause) and spacenavd 1.4
//! (protocol only, no code): native-endian 32-bit words, an event is 8 of them, a v1 request or
//! response is 8 too (type, 6 data words, status in the last).

use std::path::Path;
use std::time::Instant;

use smed_client::{DeviceInfo, Press, ServerMessage};

/// The socket's name in `$XDG_RUNTIME_DIR` (libspnav's per-user default since 2026-09; older
/// clients find it through `SPNAV_SOCKET`).
pub const SOCKET_IN_RUNTIME_DIR: &str = "spnav.sock";

/// spacenavd's system socket: if it is there, spacenavd probably runs as well.
pub const SPACENAVD_SOCKET: &str = "/var/run/spnav.sock";

/// One event or one v1 request/response: 8 native-endian `i32`.
pub const PACKET: usize = 32;
pub type Packet = [u8; PACKET];

/// Marks a v1 request (and the protocol change); its low 16 bits are the request.
const REQ_TAG: i32 = 0x7faa_0000;
const REQ_CHANGE_PROTO: i32 = 0x5500;
/// The highest protocol version smed speaks.
const MAX_PROTO: i32 = 1;

const REQ_SET_NAME: i32 = 0x1000;
const REQ_SET_SENS: i32 = 0x1001;
const REQ_GET_SENS: i32 = 0x1002;
const REQ_SET_EVMASK: i32 = 0x1003;
const REQ_GET_EVMASK: i32 = 0x1004;
const REQ_DEV_NAME: i32 = 0x2000;
const REQ_DEV_PATH: i32 = 0x2001;
const REQ_DEV_NAXES: i32 = 0x2002;
const REQ_DEV_NBUTTONS: i32 = 0x2003;
const REQ_DEV_USBID: i32 = 0x2004;
const REQ_DEV_TYPE: i32 = 0x2005;

/// Status in the last word of a response.
const OK: i32 = 0;
const FAILED: i32 = -1;

/// Event types (first word of an event).
const UEV_MOTION: i32 = 0;
const UEV_PRESS: i32 = 1;
const UEV_RELEASE: i32 = 2;
const UEV_DEV: i32 = 3;

/// Event mask bits (`SPNAV_EVMASK_*`).
pub const EVMASK_MOTION: u32 = 0x01;
pub const EVMASK_BUTTON: u32 = 0x02;
pub const EVMASK_DEV: u32 = 0x04;

/// `SPNAV_DEV_ADD`, `SPNAV_DEV_RM`.
const DEV_ADD: i32 = 0;
const DEV_RM: i32 = 1;

/// `SPNAV_DEV_SMENT`: the SpaceMouse Enterprise in libspnav's device enum.
pub const DEV_TYPE_ENTERPRISE: i32 = 0x20e;
/// Six axes: x, y, z, rx, ry, rz.
pub const AXES: i32 = 6;
/// The highest 3Dx button ID (Space, 176) plus one, so that a client that sizes a table by it
/// can index it with every ID it gets.
pub const BUTTONS: i32 = 177;

/// spacenavd's default dead zone: a raw axis value below this is 0.
pub const DEADZONE: i32 = 2;

/// Strings go in chunks of 24 bytes (data words 0-5); the last word holds the remaining
/// length, with this bit set on every chunk after the first.
const CHUNK: usize = 24;
const CONT_BIT: i32 = 0x1_0000;
/// The longest client name smed keeps (the protocol allows 64 KiB).
const MAX_NAME: usize = 256;

/// The ID a long press sends: the "opposite" function where 3DxWare has one (`Base.xml`, see
/// docs/protocol.md), else the key's own ID.
pub fn long_press_id(id: u8) -> u8 {
    match id {
        3 => 7,     // T -> Bottom
        5 => 4,     // R -> Left
        6 => 8,     // F -> Back
        9 => 10,    // Roll -> Roll CCW
        11 => 12,   // ISO -> ISO2
        103 => 139, // V1 -> Save V1
        104 => 140, // V2 -> Save V2
        105 => 141, // V3 -> Save V3
        other => other,
    }
}

fn packet(words: [i32; 8]) -> Packet {
    let mut bytes = [0_u8; PACKET];
    for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *chunk = word.to_ne_bytes();
    }
    bytes
}

fn words(bytes: &[u8]) -> [i32; 8] {
    let mut words = [0_i32; 8];
    for (word, chunk) in words.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *word = i32::from_ne_bytes(*chunk);
    }
    words
}

/// What the daemon reports to every spnav client, before each client's mask and sensitivity.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// x, y, z, rx, ry, rz after the dead zone; `period`: ms since the device's last motion
    /// report.
    Motion {
        axes: [i32; 6],
        period: i32,
    },
    Button {
        bnum: i32,
        press: bool,
    },
    Device {
        added: bool,
        vid: u16,
        pid: u16,
    },
}

/// Turns the daemon's messages into spnav events: keys by 3Dx ID (a long press by
/// [`long_press_id`]), motion with spacenavd's dead zone and one all-zero event when the cap
/// comes back to rest.
#[derive(Debug)]
pub struct Translator {
    at_rest: bool,
    /// When the last motion report came, sent on or not.
    last_motion: Option<Instant>,
    device: Option<(u16, u16)>,
}

impl Default for Translator {
    fn default() -> Self {
        Self {
            at_rest: true,
            last_motion: None,
            device: None,
        }
    }
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The event for `message` at `now`, if spnav clients get one.
    pub fn event(&mut self, message: &ServerMessage, now: Instant) -> Option<Event> {
        match message {
            ServerMessage::Motion { t, r } => {
                let mut axes = [0_i32; 6];
                for (axis, &raw) in axes.iter_mut().zip(t.iter().chain(r)) {
                    let raw = i32::from(raw);
                    *axis = if raw.abs() < DEADZONE { 0 } else { raw };
                }
                let period = self.last_motion.map_or(0, |last| {
                    i32::try_from(now.saturating_duration_since(last).as_millis())
                        .unwrap_or(i32::MAX)
                });
                self.last_motion = Some(now);
                let rest = axes == [0; 6];
                if rest && self.at_rest {
                    return None;
                }
                self.at_rest = rest;
                Some(Event::Motion { axes, period })
            }
            ServerMessage::Key {
                id, press, down, ..
            } => {
                let id = if *press == Press::Long {
                    long_press_id(*id)
                } else {
                    *id
                };
                Some(Event::Button {
                    bnum: i32::from(id),
                    press: *down,
                })
            }
            ServerMessage::Device { device: Some(info) } => {
                self.device = Some((info.vid, info.pid));
                Some(Event::Device {
                    added: true,
                    vid: info.vid,
                    pid: info.pid,
                })
            }
            ServerMessage::Device { device: None } => {
                let (vid, pid) = self.device.take()?;
                self.at_rest = true;
                self.last_motion = None;
                Some(Event::Device {
                    added: false,
                    vid,
                    pid,
                })
            }
            _ => None,
        }
    }
}

/// The device as a spnav client may ask about it.
#[derive(Clone, Copy, Debug)]
pub struct DeviceView<'a> {
    pub info: &'a DeviceInfo,
    /// The hidraw node smed reads.
    pub path: Option<&'a str>,
}

/// One spnav client's state: protocol version, event mask, sensitivity, name.
#[derive(Debug)]
pub struct Client {
    proto: i32,
    evmask: u32,
    sens: f32,
    name: Option<String>,
    /// Bytes of a word (v0) or request (v1) not complete yet.
    pending: Vec<u8>,
    /// A name arriving in chunks: what came so far and how much is still to come.
    name_chunks: Option<(Vec<u8>, usize)>,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            proto: 0,
            // spacenavd's mask for a client that never asks for v1.
            evmask: EVMASK_MOTION | EVMASK_BUTTON,
            sens: 1.0,
            name: None,
            pending: Vec::new(),
            name_chunks: None,
        }
    }
}

impl Client {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn proto(&self) -> i32 {
        self.proto
    }

    /// The client's sensitivity: 1.0 until it sets one.
    pub fn sensitivity(&self) -> f32 {
        self.sens
    }

    /// The name the client gave (`spnav_client_name`), if any.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Bytes read from the client; returns what to write back.
    pub fn on_bytes(&mut self, bytes: &[u8], device: Option<DeviceView<'_>>) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.proto == 0 {
                if self.pending.len() < 4 {
                    break;
                }
                let word: Vec<u8> = self.pending.drain(..4).collect();
                let word = i32::from_ne_bytes([word[0], word[1], word[2], word[3]]);
                out.extend(self.on_v0_word(word));
            } else {
                if self.pending.len() < PACKET {
                    break;
                }
                let request: Vec<u8> = self.pending.drain(..PACKET).collect();
                out.extend(self.on_request(words(&request), device));
            }
        }
        out
    }

    /// Protocol v0: a word is the protocol change or a sensitivity (a float's bits).
    fn on_v0_word(&mut self, word: i32) -> Vec<u8> {
        if (word & !0xff) == (REQ_TAG | REQ_CHANGE_PROTO) {
            self.proto = (word & 0xff).min(MAX_PROTO);
            if self.proto > 0 {
                self.evmask = EVMASK_MOTION | EVMASK_BUTTON | EVMASK_DEV;
            }
            return (REQ_TAG | REQ_CHANGE_PROTO | self.proto)
                .to_ne_bytes()
                .to_vec();
        }
        let sens = f32::from_bits(word.cast_unsigned());
        if sens.is_finite() {
            self.sens = sens;
        }
        Vec::new()
    }

    /// One v1 request: the answer, or nothing for `SET_NAME`.
    fn on_request(&mut self, mut rr: [i32; 8], device: Option<DeviceView<'_>>) -> Vec<u8> {
        let reply = |rr: &mut [i32; 8], status: i32| {
            rr[7] = status;
            packet(*rr).to_vec()
        };
        match rr[0] & 0xffff {
            REQ_SET_NAME => {
                self.name_chunk(&rr);
                Vec::new()
            }
            REQ_SET_SENS => {
                let sens = f32::from_bits(rr[1].cast_unsigned());
                if sens.is_finite() {
                    self.sens = sens;
                    reply(&mut rr, OK)
                } else {
                    reply(&mut rr, FAILED)
                }
            }
            REQ_GET_SENS => {
                rr[1] = self.sens.to_bits().cast_signed();
                reply(&mut rr, OK)
            }
            REQ_SET_EVMASK => {
                self.evmask = rr[1].cast_unsigned();
                reply(&mut rr, OK)
            }
            REQ_GET_EVMASK => {
                rr[1] = self.evmask.cast_signed();
                reply(&mut rr, OK)
            }
            request @ (REQ_DEV_NAME | REQ_DEV_PATH) => {
                let text = device.and_then(|device| {
                    if request == REQ_DEV_NAME {
                        Some(device.info.name.as_str())
                    } else {
                        device.path
                    }
                });
                match text {
                    Some(text) => string_reply(rr[0], text),
                    None => reply(&mut rr, FAILED),
                }
            }
            request @ (REQ_DEV_NAXES | REQ_DEV_NBUTTONS | REQ_DEV_USBID | REQ_DEV_TYPE) => {
                let Some(device) = device else {
                    return reply(&mut rr, FAILED);
                };
                match request {
                    REQ_DEV_NAXES => rr[1] = AXES,
                    REQ_DEV_NBUTTONS => rr[1] = BUTTONS,
                    REQ_DEV_USBID => {
                        rr[1] = i32::from(device.info.vid);
                        rr[2] = i32::from(device.info.pid);
                    }
                    _ => rr[1] = DEV_TYPE_ENTERPRISE,
                }
                reply(&mut rr, OK)
            }
            // The configuration requests (SCFG_*, GCFG_*, CFG_*) and anything unknown: smed
            // keeps no spacenavd configuration.
            _ => reply(&mut rr, FAILED),
        }
    }

    /// One chunk of `SET_NAME` (libspnav's `spnav_send_str`).
    fn name_chunk(&mut self, rr: &[i32; 8]) {
        let status = rr[7];
        if status < 0 {
            return;
        }
        let remaining = usize::try_from(status & 0xffff).unwrap_or(0);
        let bytes: Vec<u8> = rr[1..7].iter().flat_map(|w| w.to_ne_bytes()).collect();
        let (mut name, expect) = if status & CONT_BIT == 0 {
            (Vec::new(), remaining)
        } else {
            match self.name_chunks.take() {
                Some((name, expect)) if expect == remaining => (name, expect),
                // A continuation without its start: drop it.
                _ => return,
            }
        };
        let take = expect.min(CHUNK);
        name.extend_from_slice(&bytes[..take]);
        let left = expect - take;
        if left == 0 {
            name.truncate(MAX_NAME);
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            self.name = Some(String::from_utf8_lossy(&name[..end]).into_owned());
        } else {
            self.name_chunks = Some((name, left));
        }
    }

    /// The bytes of `event` for this client, or `None` if its mask leaves the event out.
    pub fn encode(&self, event: &Event) -> Option<Packet> {
        match *event {
            Event::Motion { axes, period } => {
                if self.evmask & EVMASK_MOTION == 0 {
                    return None;
                }
                let mut words = [UEV_MOTION, 0, 0, 0, 0, 0, 0, period];
                for (word, axis) in words[1..7].iter_mut().zip(axes) {
                    // As spacenavd: the value times the client's sensitivity, truncated (`as`
                    // saturates where C's cast would be undefined).
                    *word = (axis as f32 * self.sens) as i32;
                }
                Some(packet(words))
            }
            Event::Button { bnum, press } => {
                if self.evmask & EVMASK_BUTTON == 0 {
                    return None;
                }
                let kind = if press { UEV_PRESS } else { UEV_RELEASE };
                Some(packet([kind, bnum, i32::from(press), 0, 0, 0, 0, 0]))
            }
            Event::Device { added, vid, pid } => {
                if self.evmask & EVMASK_DEV == 0 {
                    return None;
                }
                let op = if added { DEV_ADD } else { DEV_RM };
                Some(packet([
                    UEV_DEV,
                    op,
                    0,
                    DEV_TYPE_ENTERPRISE,
                    i32::from(vid),
                    i32::from(pid),
                    0,
                    0,
                ]))
            }
        }
    }
}

/// A string answer (`spnav_send_str`): chunks of 24 bytes, the remaining length in the last
/// word, the continuation bit on every chunk after the first.
fn string_reply(kind: i32, text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let left = bytes.len() - at;
        let mut rr = [0_i32; 8];
        rr[0] = kind;
        let mut data = [0_u8; CHUNK];
        let take = left.min(CHUNK);
        data[..take].copy_from_slice(&bytes[at..at + take]);
        for (word, chunk) in rr[1..7].iter_mut().zip(data.as_chunks::<4>().0) {
            *word = i32::from_ne_bytes(*chunk);
        }
        let left = i32::try_from(left).unwrap_or(0xffff) & 0xffff;
        rr[7] = if at == 0 { left } else { left | CONT_BIT };
        out.extend_from_slice(&packet(rr));
        at += take;
        if at >= bytes.len() {
            break;
        }
    }
    out
}

/// Signs that spacenavd runs too: its system socket, or a process named `spacenavd` under
/// `proc`. Each sign is one line for the warning.
pub fn spacenavd_signs(system_socket: &Path, proc: &Path) -> Vec<String> {
    let mut signs = Vec::new();
    if std::fs::symlink_metadata(system_socket).is_ok() {
        signs.push(format!("{} exists", system_socket.display()));
    }
    if let Ok(entries) = std::fs::read_dir(proc) {
        for entry in entries.flatten() {
            let pid = entry.file_name();
            if !pid.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm"))
                && comm.trim_end() == "spacenavd"
            {
                signs.push(format!("spacenavd runs (pid {})", pid.to_string_lossy()));
            }
        }
    }
    signs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn info() -> DeviceInfo {
        DeviceInfo::new(0x256f, 0xc633, "3Dconnexion SpaceMouse Enterprise", None)
    }

    fn view(info: &DeviceInfo) -> Option<DeviceView<'_>> {
        Some(DeviceView {
            info,
            path: Some("/dev/hidraw1"),
        })
    }

    /// A v1 request as libspnav's `request()` writes it: the tag, then the data words.
    fn request(req: i32, data: [i32; 6]) -> Vec<u8> {
        let mut rr = [REQ_TAG | req, 0, 0, 0, 0, 0, 0, 0];
        rr[1..7].copy_from_slice(&data);
        packet(rr).to_vec()
    }

    fn v1() -> Client {
        let mut client = Client::new();
        let answer = client.on_bytes(&(REQ_TAG | REQ_CHANGE_PROTO | 1).to_ne_bytes(), None);
        assert_eq!(answer, (REQ_TAG | REQ_CHANGE_PROTO | 1).to_ne_bytes());
        client
    }

    #[test]
    fn the_protocol_change_is_answered_and_capped_at_v1() {
        let mut client = Client::new();
        // A client asking for v3 gets v1, as spacenavd answers.
        let answer = client.on_bytes(&(REQ_TAG | REQ_CHANGE_PROTO | 3).to_ne_bytes(), None);
        assert_eq!(answer, (REQ_TAG | REQ_CHANGE_PROTO | 1).to_ne_bytes());
        assert_eq!(client.proto(), 1);
        assert_eq!(client.evmask, EVMASK_MOTION | EVMASK_BUTTON | EVMASK_DEV);
    }

    #[test]
    fn a_client_asking_for_v0_stays_at_v0() {
        let mut client = Client::new();
        let answer = client.on_bytes(&(REQ_TAG | REQ_CHANGE_PROTO).to_ne_bytes(), None);
        assert_eq!(answer, (REQ_TAG | REQ_CHANGE_PROTO).to_ne_bytes());
        assert_eq!(client.proto(), 0);
        assert_eq!(
            client.evmask,
            EVMASK_MOTION | EVMASK_BUTTON,
            "no device events"
        );
        // Still v0: the next word is a sensitivity, not a request.
        assert!(client.on_bytes(&0.5_f32.to_ne_bytes(), None).is_empty());
        assert_eq!(client.sensitivity(), 0.5);
    }

    #[test]
    fn a_v0_client_sets_its_sensitivity_with_a_float() {
        let mut client = Client::new();
        // libspnav without an answer to the protocol change sends 1.0; later values count.
        assert!(client.on_bytes(&2.5_f32.to_ne_bytes(), None).is_empty());
        assert_eq!(client.proto(), 0);
        assert_eq!(client.evmask, EVMASK_MOTION | EVMASK_BUTTON);
        let event = Event::Motion {
            axes: [10, -10, 3, 0, 0, 100],
            period: 8,
        };
        assert_eq!(
            client.encode(&event),
            Some(packet([UEV_MOTION, 25, -25, 7, 0, 0, 250, 8]))
        );
        // NaN is not taken.
        assert!(client.on_bytes(&f32::NAN.to_ne_bytes(), None).is_empty());
        assert_eq!(client.sens, 2.5);
    }

    #[test]
    fn requests_split_across_reads_are_put_together() {
        let mut client = v1();
        let bytes = request(REQ_DEV_NAXES, [0; 6]);
        assert!(client.on_bytes(&bytes[..10], view(&info())).is_empty());
        let answer = client.on_bytes(&bytes[10..], view(&info()));
        assert_eq!(
            answer,
            packet([REQ_TAG | REQ_DEV_NAXES, 6, 0, 0, 0, 0, 0, OK])
        );
    }

    #[test]
    fn device_requests_answer_about_the_enterprise() {
        let info = info();
        let mut client = v1();
        let ask =
            |client: &mut Client, req| words(&client.on_bytes(&request(req, [0; 6]), view(&info)));
        assert_eq!(ask(&mut client, REQ_DEV_NBUTTONS)[1], 177);
        assert_eq!(ask(&mut client, REQ_DEV_TYPE)[1], 0x20e);
        let usbid = ask(&mut client, REQ_DEV_USBID);
        assert_eq!((usbid[1], usbid[2], usbid[7]), (0x256f, 0xc633, OK));
    }

    #[test]
    fn without_a_device_the_device_requests_fail() {
        let mut client = v1();
        for req in [
            REQ_DEV_NAME,
            REQ_DEV_PATH,
            REQ_DEV_NAXES,
            REQ_DEV_NBUTTONS,
            REQ_DEV_USBID,
            REQ_DEV_TYPE,
        ] {
            let answer = words(&client.on_bytes(&request(req, [0; 6]), None));
            assert_eq!((answer[0], answer[7]), (REQ_TAG | req, FAILED), "{req:#x}");
        }
    }

    #[test]
    fn the_device_name_comes_in_chunks_of_24_bytes() {
        let info = info();
        let mut client = v1();
        let answer = client.on_bytes(&request(REQ_DEV_NAME, [0; 6]), view(&info));
        // "3Dconnexion SpaceMouse Enterprise" is 33 bytes: two packets.
        assert_eq!(answer.len(), 2 * PACKET);
        let first = words(&answer[..PACKET]);
        let second = words(&answer[PACKET..]);
        assert_eq!((first[0], first[7]), (REQ_TAG | REQ_DEV_NAME, 33));
        assert_eq!(
            (second[0], second[7]),
            (REQ_TAG | REQ_DEV_NAME, 9 | CONT_BIT)
        );
        assert_eq!(&answer[4..28], b"3Dconnexion SpaceMouse E");
        assert_eq!(&answer[PACKET + 4..PACKET + 13], b"nterprise");

        let answer = client.on_bytes(&request(REQ_DEV_PATH, [0; 6]), view(&info));
        assert_eq!(answer.len(), PACKET);
        assert_eq!(&answer[4..16], b"/dev/hidraw1");
        assert_eq!(words(&answer)[7], 12);
    }

    #[test]
    fn an_empty_string_is_one_packet() {
        let answer = string_reply(REQ_TAG | REQ_DEV_PATH, "");
        assert_eq!(
            answer,
            packet([REQ_TAG | REQ_DEV_PATH, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn set_name_gets_no_answer_and_names_the_client() {
        // libspnav's spnav_client_name: REQ_SET_NAME without the tag, chunks of 24 bytes.
        let mut client = v1();
        let mut first = [REQ_SET_NAME, 0, 0, 0, 0, 0, 0, 30];
        let name = b"a-client-with-a-long-name-30ch";
        assert_eq!(name.len(), 30);
        for (word, chunk) in first[1..7].iter_mut().zip(name[..24].as_chunks::<4>().0) {
            *word = i32::from_ne_bytes(*chunk);
        }
        let mut rest = [0_u8; 24];
        rest[..6].copy_from_slice(&name[24..]);
        let mut second = [REQ_SET_NAME, 0, 0, 0, 0, 0, 0, 6 | CONT_BIT];
        for (word, chunk) in second[1..7].iter_mut().zip(rest.as_chunks::<4>().0) {
            *word = i32::from_ne_bytes(*chunk);
        }
        assert!(client.on_bytes(&packet(first), None).is_empty());
        assert_eq!(client.name(), None, "not complete yet");
        assert!(client.on_bytes(&packet(second), None).is_empty());
        assert_eq!(client.name(), Some("a-client-with-a-long-name-30ch"));

        let mut short = [REQ_SET_NAME, 0, 0, 0, 0, 0, 0, 7];
        short[1] = i32::from_ne_bytes(*b"blen");
        short[2] = i32::from_ne_bytes(*b"der\0");
        assert!(client.on_bytes(&packet(short), None).is_empty());
        assert_eq!(client.name(), Some("blender"));
    }

    #[test]
    fn a_continuation_without_its_start_is_dropped() {
        let mut client = v1();
        let stray = [REQ_SET_NAME, 0x41414141, 0, 0, 0, 0, 0, 4 | CONT_BIT];
        assert!(client.on_bytes(&packet(stray), None).is_empty());
        assert_eq!(client.name(), None);
        // A continuation whose remaining length does not follow from the start: dropped too.
        let start = [REQ_SET_NAME, 0x41414141, 0, 0, 0, 0, 0, 30];
        let wrong = [REQ_SET_NAME, 0x42424242, 0, 0, 0, 0, 0, 5 | CONT_BIT];
        client.on_bytes(&packet(start), None);
        client.on_bytes(&packet(wrong), None);
        assert_eq!(client.name(), None);
        // A negative length is no name.
        client.on_bytes(&packet([REQ_SET_NAME, 0x41414141, 0, 0, 0, 0, 0, -4]), None);
        assert_eq!(client.name(), None);
    }

    #[test]
    fn an_empty_name_is_a_name() {
        let mut client = v1();
        client.on_bytes(&packet([REQ_SET_NAME, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(client.name(), Some(""));
    }

    #[test]
    fn sensitivity_and_mask_are_set_and_read_back() {
        let mut client = v1();
        let answer = client.on_bytes(
            &request(
                REQ_SET_SENS,
                [0.5_f32.to_bits().cast_signed(), 0, 0, 0, 0, 0],
            ),
            None,
        );
        assert_eq!(words(&answer)[7], OK);
        let answer = words(&client.on_bytes(&request(REQ_GET_SENS, [0; 6]), None));
        assert_eq!(f32::from_bits(answer[1].cast_unsigned()), 0.5);
        let nan = f32::NAN.to_bits().cast_signed();
        let answer = words(&client.on_bytes(&request(REQ_SET_SENS, [nan, 0, 0, 0, 0, 0]), None));
        assert_eq!(answer[7], FAILED);

        let answer = client.on_bytes(&request(REQ_SET_EVMASK, [0x02, 0, 0, 0, 0, 0]), None);
        assert_eq!(words(&answer)[7], OK);
        let answer = words(&client.on_bytes(&request(REQ_GET_EVMASK, [0; 6]), None));
        assert_eq!(answer[1], 0x02);
        let motion = Event::Motion {
            axes: [1; 6],
            period: 0,
        };
        assert_eq!(client.encode(&motion), None, "motion masked out");
        client.on_bytes(&request(REQ_SET_EVMASK, [0x01, 0, 0, 0, 0, 0]), None);
        let button = Event::Button {
            bnum: 3,
            press: true,
        };
        assert_eq!(client.encode(&button), None, "buttons masked out");
        assert!(client.encode(&motion).is_some());
        client.on_bytes(&request(REQ_SET_EVMASK, [0x02, 0, 0, 0, 0, 0]), None);
        assert!(
            client
                .encode(&Event::Button {
                    bnum: 3,
                    press: true
                })
                .is_some()
        );
    }

    #[test]
    fn configuration_requests_fail_and_change_nothing() {
        let info = info();
        let mut client = v1();
        // SCFG_SENS, GCFG_DEADZONE, SCFG_BNMAP, SCFG_KBMAP, CFG_SAVE, CFG_RESET, unknown.
        for req in [0x3000, 0x3005, 0x300a, 0x300e, 0x3ffe, 0x3fff, 0x4242] {
            let answer = words(&client.on_bytes(&request(req, [1; 6]), view(&info)));
            assert_eq!(answer[0], REQ_TAG | req, "the type is echoed: {req:#x}");
            assert_eq!(answer[7], FAILED, "{req:#x}");
        }
        assert_eq!(client.sens, 1.0);
    }

    #[test]
    fn buttons_and_device_events_have_spacenavds_layout() {
        let client = v1();
        assert_eq!(
            client.encode(&Event::Button {
                bnum: 176,
                press: true
            }),
            Some(packet([UEV_PRESS, 176, 1, 0, 0, 0, 0, 0]))
        );
        assert_eq!(
            client.encode(&Event::Button {
                bnum: 176,
                press: false
            }),
            Some(packet([UEV_RELEASE, 176, 0, 0, 0, 0, 0, 0]))
        );
        assert_eq!(
            client.encode(&Event::Device {
                added: false,
                vid: 0x256f,
                pid: 0xc633
            }),
            Some(packet([UEV_DEV, DEV_RM, 0, 0x20e, 0x256f, 0xc633, 0, 0]))
        );
        // A v0 client has no device events.
        assert_eq!(
            Client::new().encode(&Event::Device {
                added: true,
                vid: 1,
                pid: 2
            }),
            None
        );
    }

    fn key(id: u8, press: Press, down: bool) -> ServerMessage {
        ServerMessage::Key {
            key: smed_client::key_name(id),
            id,
            press,
            down,
        }
    }

    #[test]
    fn every_key_goes_out_by_its_3dx_id_and_long_presses_by_the_table() {
        let mut translator = Translator::new();
        let now = Instant::now();
        for (id, _) in smed_client::KEYS {
            assert_eq!(
                translator.event(&key(id, Press::Short, true), now),
                Some(Event::Button {
                    bnum: i32::from(id),
                    press: true
                })
            );
        }
        let table = [
            (3, 7),
            (5, 4),
            (6, 8),
            (9, 10),
            (11, 12),
            (103, 139),
            (104, 140),
            (105, 141),
        ];
        for (id, _) in smed_client::KEYS {
            let want = table
                .iter()
                .find(|(key, _)| *key == id)
                .map_or(id, |&(_, long)| long);
            for down in [true, false] {
                assert_eq!(
                    translator.event(&key(id, Press::Long, down), now),
                    Some(Event::Button {
                        bnum: i32::from(want),
                        press: down
                    }),
                    "long {id}"
                );
            }
        }
    }

    fn motion(t: [i16; 3], r: [i16; 3]) -> ServerMessage {
        ServerMessage::Motion { t, r }
    }

    #[test]
    fn motion_has_the_dead_zone_one_final_zero_and_the_period() {
        let mut translator = Translator::new();
        let start = Instant::now();
        assert_eq!(
            translator.event(&motion([0, 0, 0], [0, 0, 0]), start),
            None,
            "at rest from the start"
        );
        assert_eq!(
            translator.event(&motion([1, -1, 2], [-2, 300, -350]), start),
            Some(Event::Motion {
                axes: [0, 0, 2, -2, 300, -350],
                period: 0
            })
        );
        assert_eq!(
            translator.event(
                &motion([1, 0, 0], [0, -1, 0]),
                start + Duration::from_millis(8)
            ),
            Some(Event::Motion {
                axes: [0; 6],
                period: 8
            }),
            "inside the dead zone is rest: one all-zero event"
        );
        assert_eq!(
            translator.event(
                &motion([0, 0, 0], [0, 0, 0]),
                start + Duration::from_millis(16)
            ),
            None,
            "and no more"
        );
        assert_eq!(
            translator.event(
                &motion([5, 0, 0], [0, 0, 0]),
                start + Duration::from_millis(40)
            ),
            Some(Event::Motion {
                axes: [5, 0, 0, 0, 0, 0],
                period: 24
            }),
            "the period counts from the last report, also one not sent on"
        );
    }

    #[test]
    fn the_device_comes_and_goes() {
        let mut translator = Translator::new();
        let now = Instant::now();
        assert_eq!(
            translator.event(&ServerMessage::Device { device: None }, now),
            None,
            "no device to remove"
        );
        assert_eq!(
            translator.event(
                &ServerMessage::Device {
                    device: Some(info())
                },
                now
            ),
            Some(Event::Device {
                added: true,
                vid: 0x256f,
                pid: 0xc633
            })
        );
        translator.event(&motion([9, 0, 0], [0, 0, 0]), now);
        assert_eq!(
            translator.event(&ServerMessage::Device { device: None }, now),
            Some(Event::Device {
                added: false,
                vid: 0x256f,
                pid: 0xc633
            })
        );
        assert_eq!(
            translator.event(&motion([0, 0, 0], [0, 0, 0]), now),
            None,
            "a new device starts at rest"
        );
        assert_eq!(
            translator.event(&ServerMessage::Lock { on: true }, now),
            None
        );
    }

    #[test]
    fn spacenavd_is_seen_by_its_socket_or_its_process() {
        let dir = std::env::temp_dir().join(format!("smed-spnav-signs-{}", std::process::id()));
        let proc = dir.join("proc");
        std::fs::create_dir_all(proc.join("42")).unwrap();
        std::fs::create_dir_all(proc.join("43")).unwrap();
        std::fs::create_dir_all(proc.join("self")).unwrap();
        std::fs::write(proc.join("42/comm"), "spacenavd\n").unwrap();
        std::fs::write(proc.join("43/comm"), "bash\n").unwrap();
        std::fs::write(proc.join("self/comm"), "spacenavd\n").unwrap();
        let socket = dir.join("spnav.sock");
        assert_eq!(
            spacenavd_signs(&socket, &proc),
            vec!["spacenavd runs (pid 42)".to_string()]
        );
        std::fs::write(&socket, "").unwrap();
        assert_eq!(spacenavd_signs(&socket, &proc).len(), 2);
        assert!(spacenavd_signs(&socket, &proc)[0].ends_with("spnav.sock exists"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(spacenavd_signs(&socket, &proc).is_empty());
    }
}
