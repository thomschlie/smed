//! The device state smed keeps: keys down, the rotation lock and its LED. Pure logic: reports
//! go in, messages for clients and output reports for the device come out.

use smed_client::{LOCK_KEY_ID, Press, ServerMessage, key_name};

use crate::protocol::{REPORT_LED, Report, decode};

/// Something to do after a report or a request.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Send to every client.
    Send(ServerMessage),
    /// Write this output report to the device.
    Output(Vec<u8>),
}

/// LED bit 0: always set by 3DxWare (meaning open, docs/protocol.md).
const LED_BASE: u8 = 0x01;
/// LED bit 1: the rotation-lock LED by the Lock key.
const LED_LOCK: u8 = 0x02;

#[derive(Debug, Default)]
pub struct Core {
    lock: bool,
    short: Vec<u8>,
    long: Vec<u8>,
}

impl Core {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock(&self) -> bool {
        self.lock
    }

    /// The LED output report for the current lock state.
    pub fn led_report(&self) -> Vec<u8> {
        let lock = if self.lock { LED_LOCK } else { 0 };
        vec![REPORT_LED, LED_BASE | lock]
    }

    /// One input report from the device.
    pub fn on_report(&mut self, bytes: &[u8]) -> Vec<Action> {
        match decode(bytes) {
            Some(Report::Motion { t, r }) => {
                let r = if self.lock { [0; 3] } else { r };
                vec![Action::Send(ServerMessage::Motion { t, r })]
            }
            Some(Report::Keys(ids)) => self.keys(Press::Short, ids),
            Some(Report::LongKeys(ids)) => self.keys(Press::Long, ids),
            Some(Report::LegacyKeys(_) | Report::Other(_)) | None => Vec::new(),
        }
    }

    /// Switches the rotation lock; nothing happens when it already is `on`.
    pub fn set_lock(&mut self, on: bool) -> Vec<Action> {
        if self.lock == on {
            return Vec::new();
        }
        self.lock = on;
        vec![
            Action::Output(self.led_report()),
            Action::Send(ServerMessage::Lock { on }),
        ]
    }

    /// The device went away: every key still down goes up. The lock stays.
    pub fn device_gone(&mut self) -> Vec<Action> {
        let mut actions = self.keys(Press::Short, Vec::new());
        actions.extend(self.keys(Press::Long, Vec::new()));
        actions
    }

    /// The keys of one kind now down: `up` for those gone, then `down` for the new ones.
    fn keys(&mut self, press: Press, now: Vec<u8>) -> Vec<Action> {
        let before = match press {
            Press::Short => std::mem::replace(&mut self.short, now.clone()),
            Press::Long => std::mem::replace(&mut self.long, now.clone()),
        };
        let event = |id: u8, down: bool| {
            Action::Send(ServerMessage::Key {
                key: key_name(id),
                id,
                press,
                down,
            })
        };
        let mut actions: Vec<Action> = before
            .iter()
            .filter(|id| !now.contains(id))
            .map(|&id| event(id, false))
            .collect();
        for &id in now.iter().filter(|id| !before.contains(id)) {
            actions.push(event(id, true));
            if id == LOCK_KEY_ID {
                actions.extend(self.set_lock(!self.lock));
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(report: u8, ids: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0_u8; 13];
        bytes[0] = report;
        bytes[1..=ids.len()].copy_from_slice(ids);
        bytes
    }

    fn key(name: &str, id: u8, press: Press, down: bool) -> Action {
        Action::Send(ServerMessage::Key {
            key: name.into(),
            id,
            press,
            down,
        })
    }

    #[test]
    fn a_short_press_is_down_then_up() {
        // docs/protocol.md: at release 0x03 + 0x1c with the key, ~100 ms later both zeroed.
        let mut core = Core::new();
        assert!(core.on_report(&[0x03, 0, 0, 0, 0x01]).is_empty());
        assert_eq!(
            core.on_report(&keys(0x1c, &[3])),
            vec![key("t", 3, Press::Short, true)]
        );
        assert!(
            core.on_report(&keys(0x1c, &[3])).is_empty(),
            "a repeat is no new press"
        );
        assert_eq!(
            core.on_report(&keys(0x1c, &[])),
            vec![key("t", 3, Press::Short, false)]
        );
    }

    #[test]
    fn a_long_press_is_down_while_held_then_up_at_release() {
        // docs/protocol.md: 0x1d with the key's own ID, 0x03 + 0x1c zeroed, 0x1d zeroed at release.
        let mut core = Core::new();
        assert_eq!(
            core.on_report(&keys(0x1d, &[103])),
            vec![key("v1", 103, Press::Long, true)]
        );
        assert!(core.on_report(&keys(0x1c, &[])).is_empty());
        assert_eq!(
            core.on_report(&keys(0x1d, &[])),
            vec![key("v1", 103, Press::Long, false)]
        );
    }

    #[test]
    fn several_keys_change_at_once_ups_first() {
        let mut core = Core::new();
        core.on_report(&keys(0x1c, &[25, 3]));
        assert_eq!(
            core.on_report(&keys(0x1c, &[25, 176])),
            vec![
                key("t", 3, Press::Short, false),
                key("space", 176, Press::Short, true)
            ]
        );
    }

    #[test]
    fn the_lock_key_toggles_lock_led_and_rotation() {
        let mut core = Core::new();
        assert_eq!(core.led_report(), vec![0x04, 0x01]);
        assert_eq!(
            core.on_report(&keys(0x1c, &[27])),
            vec![
                key("lock", 27, Press::Short, true),
                Action::Output(vec![0x04, 0x03]),
                Action::Send(ServerMessage::Lock { on: true }),
            ]
        );
        assert_eq!(
            core.on_report(&[0x01, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0]),
            vec![Action::Send(ServerMessage::Motion {
                t: [1, 2, 3],
                r: [0, 0, 0]
            })]
        );
        core.on_report(&keys(0x1c, &[]));
        // A long press of Lock toggles it too.
        let actions = core.on_report(&keys(0x1d, &[27]));
        assert_eq!(actions[1], Action::Output(vec![0x04, 0x01]));
        assert!(!core.lock());
        assert_eq!(
            core.on_report(&[0x01, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0]),
            vec![Action::Send(ServerMessage::Motion {
                t: [1, 2, 3],
                r: [4, 5, 6]
            })]
        );
    }

    #[test]
    fn set_lock_only_acts_on_a_change() {
        let mut core = Core::new();
        assert!(core.set_lock(false).is_empty());
        assert_eq!(core.set_lock(true).len(), 2);
        assert!(core.set_lock(true).is_empty());
    }

    #[test]
    fn keys_down_go_up_when_the_device_goes() {
        let mut core = Core::new();
        core.on_report(&keys(0x1d, &[1]));
        core.set_lock(true);
        assert_eq!(core.device_gone(), vec![key("menu", 1, Press::Long, false)]);
        assert!(core.lock(), "the lock survives a re-plug");
    }
}
