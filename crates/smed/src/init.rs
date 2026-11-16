//! What the device needs after it appears. 3DxWare's sequence (docs/protocol.md): feature
//! `15 64`, `11 64`, output `04 01`, feature `13 1a`, `11 64`, `14 fc ff 07 1c`. smed sends the
//! same, with the LED report for the lock state it keeps; the LCD thread then sends brightness
//! and frames (daemon.rs).

use std::io;

use crate::device::HidDevice;

/// One step of the sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Feature(&'static [u8]),
    /// The LED output report for the current lock state.
    Led,
}

pub const SEQUENCE: [Step; 6] = [
    Step::Feature(&[0x15, 0x64]),
    Step::Feature(&[0x11, 0x64]),
    Step::Led,
    Step::Feature(&[0x13, 0x1a]),
    Step::Feature(&[0x11, 0x64]),
    Step::Feature(&[0x14, 0xfc, 0xff, 0x07, 0x1c]),
];

/// Runs [`SEQUENCE`], with `led` as the LED report.
pub fn run(device: &dyn HidDevice, led: &[u8]) -> io::Result<()> {
    for step in SEQUENCE {
        match step {
            Step::Feature(report) => device.set_feature(report)?,
            Step::Led => device.write_output(led)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Fake, Written};

    #[test]
    fn init_sends_3dxware_sequence_with_our_led() {
        let device = Fake::default();
        run(&device, &[0x04, 0x03]).unwrap();
        assert_eq!(
            device.written(),
            vec![
                Written::Feature(vec![0x15, 0x64]),
                Written::Feature(vec![0x11, 0x64]),
                Written::Output(vec![0x04, 0x03]),
                Written::Feature(vec![0x13, 0x1a]),
                Written::Feature(vec![0x11, 0x64]),
                Written::Feature(vec![0x14, 0xfc, 0xff, 0x07, 0x1c]),
            ]
        );
    }
}
