//! smed's gates, run with `cargo xtask check` from anywhere in the repository.
//!
//! Every check a change has to pass before it is merged is here, so that a contributor, the
//! maintainer's build machine and CI all run the same thing. Each step stops the run at its
//! first failure and says what failed; a missing tool or toolchain is a failure, never a skip.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The toolchain rust-toolchain.toml pins.
const TOOLCHAIN: &str = "1.99.1";
/// Cargo.toml's `rust-version`: the oldest Rust smed promises to build on.
const MSRV: &str = "1.88";

const USAGE: &str = "usage: cargo xtask check\n\n  check   run every gate (formatting, lints, tests, MSRV)\n";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check"] => check(),
        ["help" | "--help" | "-h"] => {
            print!("{USAGE}");
            Ok(())
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => {
            eprintln!("xtask: all gates passed");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("xtask: FAILED: {message}");
            ExitCode::FAILURE
        }
    }
}

type Result<T = ()> = std::result::Result<T, String>;

fn check() -> Result {
    let root = workspace_root();
    env::set_current_dir(&root).map_err(|e| format!("cd {}: {e}", root.display()))?;
    pins_agree()?;
    toolchain_installed(MSRV)?;

    step("formatting", cargo(["fmt", "--all", "--check"]))?;
    step(
        "clippy",
        cargo([
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    step("tests", cargo(["test", "--workspace", "--locked"]))?;
    // Through rustup: inside `cargo xtask`, $CARGO is the pinned toolchain's own cargo, which
    // knows no `+toolchain`; RUSTUP_TOOLCHAIN (set by that cargo) would override the choice.
    let mut msrv = Command::new("rustup");
    msrv.env_remove("RUSTUP_TOOLCHAIN")
        .args(["run", MSRV, "cargo"])
        .args([
            "check",
            "--workspace",
            "--exclude",
            "xtask",
            "--all-targets",
            "--locked",
        ]);
    step(&format!("MSRV {MSRV}"), msrv)?;
    Ok(())
}

/// The repository root: xtask lives one level below it.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

fn cargo<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // The cargo that runs xtask: the pinned toolchain's.
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.args(args);
    command
}

/// Runs one gate, printing its name first.
fn step(name: &str, mut command: Command) -> Result {
    eprintln!("xtask: == {name}");
    let status = command
        .status()
        .map_err(|e| format!("{name}: cannot run {command:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} ({status})"))
    }
}

/// TOOLCHAIN and MSRV here, rust-toolchain.toml's channel and Cargo.toml's rust-version: one
/// changed without the others would test something else than what is promised.
fn pins_agree() -> Result {
    let read = |path: &str| fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    let toolchain = read("rust-toolchain.toml")?;
    let manifest = read("Cargo.toml")?;
    if !toolchain.contains(&format!("channel = \"{TOOLCHAIN}\"")) {
        return Err(format!(
            "rust-toolchain.toml does not pin {TOOLCHAIN} (xtask's TOOLCHAIN)"
        ));
    }
    if !manifest.contains(&format!("rust-version = \"{MSRV}\"")) {
        return Err(format!(
            "Cargo.toml's rust-version is not {MSRV} (xtask's MSRV)"
        ));
    }
    Ok(())
}

/// The MSRV check needs a second toolchain; without it the gate stops instead of skipping.
fn toolchain_installed(version: &str) -> Result {
    let output = Command::new("rustup")
        .args(["toolchain", "list"])
        .output()
        .map_err(|e| format!("rustup not found ({e}); the gates need rustup for the MSRV check"))?;
    let list = String::from_utf8_lossy(&output.stdout);
    if list
        .lines()
        .any(|line| line.starts_with(&format!("{version}-")) || line == version)
    {
        Ok(())
    } else {
        Err(format!(
            "Rust {version} (the MSRV) is not installed: rustup toolchain install {version} --profile minimal"
        ))
    }
}
