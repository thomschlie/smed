//! smed: driver daemon for the SpaceMouse Enterprise (USB 256f:c633). See docs/api.md.

use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use smed::config::{self, Config};
use smed::daemon::Shared;
use smed::device::{self, Hidraw};
use smed::usb::{LcdSink, UsbLcd};

/// How often smed looks for the device while it is absent.
const RESCAN: Duration = Duration::from_secs(1);

const USAGE: &str = "usage: smed [--socket PATH] [--config PATH]\n\
                     Listens on $XDG_RUNTIME_DIR/smed/api.sock unless --socket is given;\n\
                     reads $XDG_CONFIG_HOME/smed/config.toml unless --config is given.";

fn main() {
    if let Err(message) = run() {
        eprintln!("smed: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut socket = None;
    let mut config_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--config" => config_path = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--version" => {
                println!("smed {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(USAGE.into()),
        }
    }
    let socket = match socket {
        Some(path) => path,
        None => smed_client::default_socket_path().ok_or("XDG_RUNTIME_DIR is not set")?,
    };
    let listener = listen(&socket)?;
    eprintln!(
        "smed {}: listening on {}",
        env!("CARGO_PKG_VERSION"),
        socket.display()
    );

    // A broken config file must not cost the user the device: say so, use the defaults.
    let config = match config_path.or_else(config::default_path) {
        Some(path) => Config::load(&path).unwrap_or_else(|error| {
            eprintln!("smed: config: {error}; using the defaults");
            Config::default()
        }),
        None => Config::default(),
    };

    let shared = Shared::with_settings(config.lcd);
    let drawing = shared.clone();
    std::thread::spawn(move || drawing.run_lcd());
    let accepting = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let shared = accepting.clone();
            std::thread::spawn(move || {
                if let Err(error) = shared.serve_client(stream) {
                    eprintln!("smed: client: {error}");
                }
            });
        }
    });
    run_devices(&shared)
}

/// Binds the socket, creating its directory (0700) and replacing a stale socket from a smed
/// that did not shut down cleanly. A socket someone answers on is another smed: an error.
fn listen(socket: &Path) -> Result<UnixListener, String> {
    if let Some(dir) = socket.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(socket) {
        if !meta.file_type().is_socket() {
            return Err(format!("{} exists and is not a socket", socket.display()));
        }
        if UnixStream::connect(socket).is_ok() {
            return Err(format!("another smed listens on {}", socket.display()));
        }
        std::fs::remove_file(socket).map_err(|e| format!("removing {}: {e}", socket.display()))?;
    }
    UnixListener::bind(socket).map_err(|e| format!("binding {}: {e}", socket.display()))
}

/// Finds, runs and re-finds the device forever.
fn run_devices(shared: &Arc<Shared>) -> Result<(), String> {
    let mut said_absent = false;
    loop {
        let Some(found) = device::find() else {
            if !said_absent {
                eprintln!("smed: no SpaceMouse Enterprise found, waiting for it");
                said_absent = true;
            }
            std::thread::sleep(RESCAN);
            continue;
        };
        said_absent = false;
        let node = found.node.display().to_string();
        match Hidraw::open(&found.node) {
            Ok(hidraw) => {
                eprintln!(
                    "smed: {} on {node} (firmware {})",
                    found.info.name,
                    found.info.firmware.as_deref().unwrap_or("unknown")
                );
                let lcd = open_lcd(found.usb.as_deref());
                match shared.run_device(Arc::new(hidraw), found.info, lcd) {
                    Ok(()) => eprintln!("smed: {node} closed"),
                    Err(error) => eprintln!("smed: {node}: {error}"),
                }
            }
            Err(error) => eprintln!("smed: opening {node}: {error}"),
        }
        std::thread::sleep(RESCAN);
    }
}

/// The LCD interface, or `None` with a warning: without it, motion and keys still work.
fn open_lcd(usb: Option<&Path>) -> Option<Arc<dyn LcdSink>> {
    let Some(usb) = usb else {
        eprintln!("smed: warning: USB device of the SpaceMouse not found, no LCD");
        return None;
    };
    match UsbLcd::open(usb) {
        Ok(lcd) => Some(Arc::new(lcd)),
        Err(error) => {
            eprintln!("smed: warning: no LCD: {error}");
            None
        }
    }
}
