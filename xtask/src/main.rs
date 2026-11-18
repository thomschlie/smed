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

const USAGE: &str = "usage: cargo xtask check\n\n  check   run every gate (formatting, lints, tests, MSRV, packaging, semver)\n";

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
    tools_installed()?;

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
    packaging()?;
    step(
        "semver-checks (smed-client against its latest release)",
        // The manifest, not --package: semver-checks would also find the unpacked copies
        // in target/package/ and call the name ambiguous.
        cargo([
            "semver-checks",
            "--manifest-path",
            "crates/smed-client/Cargo.toml",
        ]),
    )?;
    Ok(())
}

/// The crates as crates.io would get them: what each package holds, that each builds from its
/// `.crate` alone, that smed-client's tests and doctests (the README's examples among them)
/// pass outside the workspace, and that `cargo install` of the binaries works.
fn packaging() -> Result {
    for (name, extra) in PACKAGES {
        let output = cargo(["package", "--list", "--locked", "--package", name])
            .output()
            .map_err(|e| format!("cargo package --list: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo package --list --package {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let list = String::from_utf8_lossy(&output.stdout);
        for file in ["LICENSE-MIT", "LICENSE-APACHE", "README.md"]
            .iter()
            .chain(extra.iter())
        {
            if !list.lines().any(|line| line == *file) {
                return Err(format!("the {name} package lacks {file}"));
            }
        }
    }
    step(
        "package (each crate built from its .crate)",
        cargo(["package", "--workspace", "--exclude", "xtask", "--locked"]),
    )?;

    let version = workspace_version()?;
    let packages = target_dir()?.join("package");
    let scratch = env::temp_dir().join(format!("smed-xtask-{}", std::process::id()));
    let result = packaged_crates(&version, &packages, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    result
}

/// Every published crate with the files its package must hold besides the licences and README.
const PACKAGES: [(&str, &[&str]); 3] = [
    (
        "smed",
        &[
            "fonts/OFL.txt",
            "fonts/NotoSans-Regular.ttf",
            "fonts/NotoSans-SemiBold.ttf",
        ],
    ),
    ("smed-client", &[]),
    ("smectl", &[]),
];

fn packaged_crates(version: &str, packages: &Path, scratch: &Path) -> Result {
    fs::create_dir_all(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let unpack = |name: &str| -> Result<PathBuf> {
        let file = packages.join(format!("{name}-{version}.crate"));
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&file)
            .arg("-C")
            .arg(scratch)
            .status()
            .map_err(|e| format!("tar: {e}"))?;
        if !status.success() {
            return Err(format!("cannot unpack {}", file.display()));
        }
        Ok(scratch.join(format!("{name}-{version}")))
    };
    let client = unpack("smed-client")?;
    let mut tests = cargo(["test", "--locked"]);
    tests
        .current_dir(&client)
        .env("CARGO_TARGET_DIR", scratch.join("target"));
    step(
        "smed-client's tests from its .crate, outside the workspace",
        tests,
    )?;

    // smed-client 0.2 may not be on crates.io yet: the binaries take the packed one.
    let patch = format!(
        "patch.crates-io.smed-client.path={:?}",
        client.display().to_string()
    );
    let root = scratch.join("install");
    for name in ["smed", "smectl"] {
        let dir = unpack(name)?;
        let mut install = cargo(["install", "--debug", "--locked", "--config", &patch]);
        install
            .arg("--path")
            .arg(&dir)
            .arg("--root")
            .arg(&root)
            .env("CARGO_TARGET_DIR", scratch.join("target"));
        step(&format!("cargo install {name} from its .crate"), install)?;
    }
    let smed = Command::new(root.join("bin/smed"))
        .arg("--version")
        .output()
        .map_err(|e| format!("installed smed: {e}"))?;
    if !smed.status.success() || !String::from_utf8_lossy(&smed.stdout).contains(version) {
        return Err(format!(
            "the installed smed --version does not say {version}: {}",
            String::from_utf8_lossy(&smed.stdout)
        ));
    }
    let smectl = Command::new(root.join("bin/smectl"))
        .arg("--help")
        .output()
        .map_err(|e| format!("installed smectl: {e}"))?;
    if !smectl.status.success()
        || !String::from_utf8_lossy(&smectl.stdout).contains("usage: smectl")
    {
        return Err("the installed smectl does not print its usage".into());
    }
    Ok(())
}

/// Cargo's build directory: `target/` unless CARGO_TARGET_DIR or a cargo config moves it.
fn target_dir() -> Result<PathBuf> {
    let output = cargo(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let json = String::from_utf8_lossy(&output.stdout);
    // A plain path in JSON: no escapes to undo in the paths cargo writes here.
    let key = "\"target_directory\":\"";
    json.find(key)
        .and_then(|at| json[at + key.len()..].split('"').next())
        .filter(|path| output.status.success() && !path.contains('\\'))
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata names no target_directory".into())
}

/// `[workspace.package]`'s version, the one every published crate has.
fn workspace_version() -> Result<String> {
    let manifest = fs::read_to_string("Cargo.toml").map_err(|e| format!("Cargo.toml: {e}"))?;
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .map(str::to_owned)
        .ok_or_else(|| "no version in Cargo.toml's [workspace.package]".into())
}

/// The cargo tools the gates run, with the versions they are tested with. A different version
/// is refused, not used: a build machine that installs them takes these pins.
const TOOLS: [(&str, &str); 1] = [("semver-checks", "0.51.0")];

fn tools_installed() -> Result {
    for (tool, version) in TOOLS {
        let have = cargo([tool, "--version"])
            .output()
            .ok()
            .filter(|o| o.status.success());
        let have = have.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        if have.as_deref() != Some(&format!("cargo-{tool} {version}")) {
            return Err(format!(
                "cargo-{tool} {version} is needed (found: {}): cargo +{TOOLCHAIN} install --locked cargo-{tool} --version {version}",
                have.as_deref().unwrap_or("none")
            ));
        }
    }
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
