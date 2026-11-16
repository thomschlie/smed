//! The LCD's USB interface (interface 0, vendor class, bulk OUT endpoint 0x01) through
//! usbdevfs: `/dev/bus/usb/BBB/DDD`, claim the interface, bulk transfers. No libusb.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::frame::Frame;

/// Where frames go: the device, or a recorder in tests.
pub trait LcdSink: Send + Sync {
    /// One bulk OUT transfer to the LCD endpoint.
    fn bulk_out(&self, data: &[u8]) -> io::Result<()>;
}

/// Sends `frame` as its two transfers.
pub fn send(sink: &dyn LcdSink, frame: &Frame) -> io::Result<()> {
    sink.bulk_out(&frame.header)?;
    sink.bulk_out(&frame.image)
}

const LCD_INTERFACE: u32 = 0;
const LCD_ENDPOINT: u32 = 0x01;
/// A frame is ~10-60 KB; at USB 2.0 speeds this is generous.
const BULK_TIMEOUT_MS: u32 = 2000;

/// `_IOR('U', 15, unsigned int)` (linux/usbdevice_fs.h).
const USBDEVFS_CLAIMINTERFACE: u64 = ioc(2, 15, 4);
/// `_IOR('U', 16, unsigned int)`.
const USBDEVFS_RELEASEINTERFACE: u64 = ioc(2, 16, 4);
/// `_IOWR('U', 2, struct usbdevfs_bulktransfer)`.
const USBDEVFS_BULK: u64 = ioc(3, 2, std::mem::size_of::<BulkTransfer>() as u64);

const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | ((size & 0x3fff) << 16) | ((b'U' as u64) << 8) | nr
}

/// `struct usbdevfs_bulktransfer`.
#[repr(C)]
struct BulkTransfer {
    ep: libc::c_uint,
    len: libc::c_uint,
    timeout: libc::c_uint,
    data: *mut libc::c_void,
}

/// The LCD interface, claimed.
pub struct UsbLcd {
    file: Mutex<File>,
}

impl UsbLcd {
    /// Opens the USB device whose sysfs directory is `usb` (it has `busnum` and `devnum`) and
    /// claims interface 0.
    pub fn open(usb: &Path) -> io::Result<Self> {
        let node = node(usb)?;
        let file = OpenOptions::new().read(true).write(true).open(&node)?;
        let mut interface: libc::c_uint = LCD_INTERFACE;
        // SAFETY: the ioctl reads one unsigned int from a pointer valid across the call.
        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                USBDEVFS_CLAIMINTERFACE as _,
                &mut interface as *mut libc::c_uint,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!("claiming interface 0 of {}: {error}", node.display()),
            ));
        }
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl Drop for UsbLcd {
    fn drop(&mut self) {
        if let Ok(file) = self.file.lock() {
            let mut interface: libc::c_uint = LCD_INTERFACE;
            // SAFETY: as in `open`. Closing the file releases it anyway; this is tidiness.
            unsafe {
                libc::ioctl(
                    file.as_raw_fd(),
                    USBDEVFS_RELEASEINTERFACE as _,
                    &mut interface as *mut libc::c_uint,
                );
            }
        }
    }
}

impl LcdSink for UsbLcd {
    fn bulk_out(&self, data: &[u8]) -> io::Result<()> {
        let file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("LCD lock poisoned"))?;
        let len = libc::c_uint::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "transfer too large"))?;
        let mut transfer = BulkTransfer {
            ep: LCD_ENDPOINT,
            len,
            timeout: BULK_TIMEOUT_MS,
            // The kernel only reads from an OUT transfer's buffer.
            data: data.as_ptr() as *mut libc::c_void,
        };
        // SAFETY: `transfer` and the buffer it points to live across the call; for an OUT
        // endpoint the kernel copies `len` bytes from it.
        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                USBDEVFS_BULK as _,
                &mut transfer as *mut BulkTransfer,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if result as usize != data.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short bulk transfer: {result} of {} bytes", data.len()),
            ));
        }
        Ok(())
    }
}

/// `/dev/bus/usb/BBB/DDD` from the sysfs USB device directory.
fn node(usb: &Path) -> io::Result<PathBuf> {
    let read = |name: &str| -> io::Result<u32> {
        std::fs::read_to_string(usb.join(name))?
            .trim()
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
    };
    Ok(PathBuf::from(format!(
        "/dev/bus/usb/{:03}/{:03}",
        read("busnum")?,
        read("devnum")?
    )))
}

/// Records transfers, for tests.
#[derive(Default)]
pub struct FakeLcd {
    pub transfers: Mutex<Vec<Vec<u8>>>,
}

impl FakeLcd {
    pub fn transfers(&self) -> Vec<Vec<u8>> {
        self.transfers.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

impl LcdSink for FakeLcd {
    fn bulk_out(&self, data: &[u8]) -> io::Result<()> {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.push(data.to_vec());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ioctls_match_the_kernel_header() {
        // As the C macros compute them on x86-64 and arm64.
        assert_eq!(USBDEVFS_CLAIMINTERFACE, 0x8004_550f);
        assert_eq!(USBDEVFS_RELEASEINTERFACE, 0x8004_5510);
        assert_eq!(USBDEVFS_BULK, 0xc018_5502);
    }

    #[test]
    fn the_node_comes_from_busnum_and_devnum() {
        let dir = std::env::temp_dir().join(format!("smed-usb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("busnum"), "1\n").unwrap();
        std::fs::write(dir.join("devnum"), "12\n").unwrap();
        assert_eq!(node(&dir).unwrap(), Path::new("/dev/bus/usb/001/012"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_frame_is_two_transfers() {
        let sink = FakeLcd::default();
        let frame = Frame {
            header: vec![0x11; 512],
            image: vec![1, 2, 3],
        };
        send(&sink, &frame).unwrap();
        assert_eq!(sink.transfers(), vec![vec![0x11; 512], vec![1, 2, 3]]);
    }
}
