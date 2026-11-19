//! Decoding the device's HID input reports (docs/protocol.md, "HID reports").

/// Report IDs the device sends.
pub const REPORT_MOTION: u8 = 0x01;
pub const REPORT_ROTATION: u8 = 0x02;
pub const REPORT_LEGACY_KEYS: u8 = 0x03;
pub const REPORT_KEYS: u8 = 0x1c;
pub const REPORT_LONG_KEYS: u8 = 0x1d;
/// Output report: LED bitmask.
pub const REPORT_LED: u8 = 0x04;

/// One input report, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// All six axes (report 0x01, 13 bytes): x, y, z, rx, ry, rz.
    Motion { t: [i16; 3], r: [i16; 3] },
    /// Keys reported after a short press (0x1c): their 3Dx IDs, sorted, no zeros.
    Keys(Vec<u8>),
    /// Keys held past the long-press threshold (0x1d): their 3Dx IDs, sorted, no zeros.
    LongKeys(Vec<u8>),
    /// The legacy key bitmask (0x03); smed reads keys from 0x1c instead.
    LegacyKeys(u32),
    /// A report smed does not use (or a 0x01/0x02 too short to hold six axes).
    Other(u8),
}

fn i16_at(bytes: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn ids(payload: &[u8]) -> Vec<u8> {
    let mut ids: Vec<u8> = payload.iter().copied().filter(|&id| id != 0).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Decodes one report as hidraw delivers it: report ID first. `None` for an empty read.
pub fn decode(bytes: &[u8]) -> Option<Report> {
    let (&id, payload) = bytes.split_first()?;
    Some(match id {
        REPORT_MOTION if payload.len() >= 12 => Report::Motion {
            t: [i16_at(payload, 0), i16_at(payload, 2), i16_at(payload, 4)],
            r: [i16_at(payload, 6), i16_at(payload, 8), i16_at(payload, 10)],
        },
        REPORT_LEGACY_KEYS if payload.len() >= 4 => Report::LegacyKeys(u32::from_le_bytes([
            payload[0], payload[1], payload[2], payload[3],
        ])),
        REPORT_KEYS => Report::Keys(ids(payload)),
        REPORT_LONG_KEYS => Report::LongKeys(ids(payload)),
        other => Report::Other(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_is_six_little_endian_axes() {
        // x = 1, y = -2, z = 300, rx = -300, ry = 0x7fff, rz = -0x8000
        let report = [
            0x01, 0x01, 0x00, 0xfe, 0xff, 0x2c, 0x01, 0xd4, 0xfe, 0xff, 0x7f, 0x00, 0x80,
        ];
        assert_eq!(
            decode(&report),
            Some(Report::Motion {
                t: [1, -2, 300],
                r: [-300, i16::MAX, i16::MIN]
            })
        );
    }

    #[test]
    fn short_motion_and_legacy_reports_are_not_decoded() {
        assert_eq!(decode(&[0x01, 0x01, 0x00]), Some(Report::Other(0x01)));
        assert_eq!(decode(&[0x03, 0x00, 0x00, 0x04]), Some(Report::Other(0x03)));
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn key_reports_carry_ids_one_per_byte() {
        let mut report = [0_u8; 13];
        report[0] = 0x1c;
        report[1] = 3; // T
        report[5] = 175; // Tab
        assert_eq!(decode(&report), Some(Report::Keys(vec![3, 175])));
        report[0] = 0x1d;
        assert_eq!(decode(&report), Some(Report::LongKeys(vec![3, 175])));
        assert_eq!(decode(&[0x1c; 1]), Some(Report::Keys(vec![])));
    }

    #[test]
    fn legacy_mask_is_little_endian() {
        // Lock is bit 26 (docs/protocol.md)
        assert_eq!(
            decode(&[0x03, 0x00, 0x00, 0x00, 0x04]),
            Some(Report::LegacyKeys(1 << 26))
        );
    }
}
