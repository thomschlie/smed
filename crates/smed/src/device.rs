//! Access to the device: a trait the daemon talks to, the hidraw implementation, and a fake for
//! tests.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use smed_client::DeviceInfo;

/// USB vendor and product ID of the SpaceMouse Enterprise.
pub const VID: u16 = 0x256f;
pub const PID: u16 = 0xc633;

/// One HID device: input reports in, output and feature reports out. Reports start with their
/// report ID.
pub trait HidDevice: Send + Sync {
    /// Blocks for the next input report; `Ok(0)` when the device is gone.
    fn read_report(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_output(&self, report: &[u8]) -> io::Result<()>;
    fn set_feature(&self, report: &[u8]) -> io::Result<()>;
}

/// The device on `/dev/hidrawN`.
pub struct Hidraw {
    file: File,
}

impl Hidraw {
    pub fn open(node: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(node)?;
        Ok(Self { file })
    }
}

/// `HIDIOCSFEATURE(len)`: `_IOC(_IOC_READ | _IOC_WRITE, 'H', 0x06, len)` (linux/hidraw.h).
fn hidiocsfeature(len: usize) -> u64 {
    const IOC_READ_WRITE: u64 = 3;
    (IOC_READ_WRITE << 30) | ((len as u64 & 0x3fff) << 16) | ((b'H' as u64) << 8) | 0x06
}

impl HidDevice for Hidraw {
    fn read_report(&self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.file).read(buf)
    }

    fn write_output(&self, report: &[u8]) -> io::Result<()> {
        (&self.file).write_all(report)
    }

    fn set_feature(&self, report: &[u8]) -> io::Result<()> {
        let mut buf = report.to_vec();
        // SAFETY: the kernel reads (and may write back) `buf.len()` bytes, the size encoded in
        // the request, from a buffer that lives across the call.
        let result = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                hidiocsfeature(buf.len()) as _,
                buf.as_mut_ptr(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// A SpaceMouse Enterprise found in sysfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub node: PathBuf,
    pub info: DeviceInfo,
    /// The USB device's sysfs directory (for the LCD interface), if it could be resolved.
    pub usb: Option<PathBuf>,
}

/// The first SpaceMouse Enterprise under `/sys/class/hidraw`.
pub fn find() -> Option<Found> {
    find_in(Path::new("/sys/class/hidraw"), Path::new("/dev"))
}

/// [`find`] below another sysfs class directory and /dev, for tests.
pub fn find_in(class: &Path, dev: &Path) -> Option<Found> {
    let mut entries: Vec<_> = std::fs::read_dir(class).ok()?.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    entries.into_iter().find_map(|entry| {
        let sys = entry.path();
        let uevent = std::fs::read_to_string(sys.join("device/uevent")).ok()?;
        let (vid, pid, name) = parse_uevent(&uevent)?;
        (vid == VID && pid == PID).then(|| {
            let usb = usb_device(&sys);
            Found {
                node: dev.join(entry.file_name()),
                info: DeviceInfo::new(vid, pid, name, usb.as_deref().and_then(firmware)),
                usb,
            }
        })
    })
}

/// Vendor, product and name from a HID device's uevent (`HID_ID=0003:0000256F:0000C633`).
fn parse_uevent(uevent: &str) -> Option<(u16, u16, String)> {
    let mut ids = None;
    let mut name = String::new();
    for line in uevent.lines() {
        if let Some(value) = line.strip_prefix("HID_ID=") {
            let mut parts = value.split(':').skip(1);
            let vid = u32::from_str_radix(parts.next()?, 16).ok()?;
            let pid = u32::from_str_radix(parts.next()?, 16).ok()?;
            ids = Some((u16::try_from(vid).ok()?, u16::try_from(pid).ok()?));
        } else if let Some(value) = line.strip_prefix("HID_NAME=") {
            name = value.to_owned();
        }
    }
    let (vid, pid) = ids?;
    Some((vid, pid, name))
}

/// The USB device's sysfs directory: hidrawN/device is the HID device, its parent the USB
/// interface, and that one's parent the USB device.
fn usb_device(sys: &Path) -> Option<PathBuf> {
    let hid = std::fs::canonicalize(sys.join("device")).ok()?;
    Some(hid.parent()?.parent()?.to_path_buf())
}

/// The firmware version from the USB device's bcdDevice.
fn firmware(usb: &Path) -> Option<String> {
    format_bcd(std::fs::read_to_string(usb.join("bcdDevice")).ok()?.trim())
}

/// `"0442"` → `"4.42"`.
fn format_bcd(bcd: &str) -> Option<String> {
    if bcd.len() != 4 || !bcd.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let major = u8::from_str_radix(&bcd[..2], 16).ok()?;
    Some(format!("{major:x}.{}", &bcd[2..]))
}

/// A device for tests: hands out scripted reports, then reports end of file; records writes.
#[derive(Default)]
pub struct Fake {
    pub reports: std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>,
    pub written: std::sync::Mutex<Vec<Written>>,
}

/// What a [`Fake`] was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    Output(Vec<u8>),
    Feature(Vec<u8>),
}

impl Fake {
    pub fn with_reports(reports: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            reports: std::sync::Mutex::new(reports.into_iter().collect()),
            written: Default::default(),
        }
    }

    pub fn written(&self) -> Vec<Written> {
        self.written.lock().map(|w| w.clone()).unwrap_or_default()
    }
}

impl HidDevice for Fake {
    fn read_report(&self, buf: &mut [u8]) -> io::Result<usize> {
        let next = self.reports.lock().ok().and_then(|mut r| r.pop_front());
        let Some(report) = next else { return Ok(0) };
        let len = report.len().min(buf.len());
        buf[..len].copy_from_slice(&report[..len]);
        Ok(len)
    }

    fn write_output(&self, report: &[u8]) -> io::Result<()> {
        if let Ok(mut written) = self.written.lock() {
            written.push(Written::Output(report.to_vec()));
        }
        Ok(())
    }

    fn set_feature(&self, report: &[u8]) -> io::Result<()> {
        if let Ok(mut written) = self.written.lock() {
            written.push(Written::Feature(report.to_vec()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feature_ioctl_matches_the_kernel_header() {
        // _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, 2) as the C macro computes it on x86/arm
        assert_eq!(hidiocsfeature(2), 0xc002_4806);
        assert_eq!(hidiocsfeature(5), 0xc005_4806);
    }

    #[test]
    fn uevent_gives_ids_and_name() {
        let uevent = "DRIVER=hid-generic\nHID_ID=0003:0000256F:0000C633\n\
                      HID_NAME=3Dconnexion SpaceMouse Enterprise\nHID_PHYS=usb-0000:00:1d.0-1/input1\n";
        assert_eq!(
            parse_uevent(uevent),
            Some((0x256f, 0xc633, "3Dconnexion SpaceMouse Enterprise".into()))
        );
        assert_eq!(parse_uevent("HID_NAME=x\n"), None);
    }

    #[test]
    fn bcd_device_reads_as_a_version() {
        assert_eq!(format_bcd("0442").as_deref(), Some("4.42"));
        assert_eq!(format_bcd("1001").as_deref(), Some("10.01"));
        assert_eq!(format_bcd("44"), None);
    }

    #[test]
    fn find_picks_the_enterprise_from_sysfs() {
        let root = std::env::temp_dir().join(format!("smed-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // usb device 1-1 (bcdDevice) / interface 1-1:1.1 / HID device / hidraw2
        let usb = root.join("devices/1-1");
        let hid = usb.join("1-1:1.1/0003:256F:C633.0002");
        std::fs::create_dir_all(&hid).unwrap();
        std::fs::write(usb.join("bcdDevice"), "0442\n").unwrap();
        std::fs::write(
            hid.join("uevent"),
            "HID_ID=0003:0000256F:0000C633\nHID_NAME=SpaceMouse Enterprise\n",
        )
        .unwrap();
        let other = root.join("devices/kbd");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("uevent"),
            "HID_ID=0003:0000046D:0000C31C\nHID_NAME=kbd\n",
        )
        .unwrap();
        // Another 3Dconnexion device: the same vendor, another product.
        let sibling = root.join("devices/wireless");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(
            sibling.join("uevent"),
            "HID_ID=0003:0000256F:0000C62E\nHID_NAME=SpaceMouse Wireless\n",
        )
        .unwrap();
        let class = root.join("class");
        for (name, target) in [
            ("hidraw0", &other),
            ("hidraw1", &sibling),
            ("hidraw2", &hid),
        ] {
            std::fs::create_dir_all(class.join(name)).unwrap();
            std::os::unix::fs::symlink(target, class.join(name).join("device")).unwrap();
        }
        let found = find_in(&class, Path::new("/dev")).unwrap();
        assert_eq!(found.node, Path::new("/dev/hidraw2"));
        assert_eq!(found.info.name, "SpaceMouse Enterprise");
        assert_eq!(found.info.firmware.as_deref(), Some("4.42"));
        assert_eq!(
            found.usb,
            Some(std::fs::canonicalize(&usb).unwrap()),
            "the USB device, for the LCD"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
