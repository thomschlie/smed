//! smectl against a fake smed on a Unix socket: what it sends, prints and exits with.
// The fake smed and the helpers run outside `#[test]` functions, where clippy.toml's test
// exemption does not reach.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

/// What the fake smed does with a line smectl sent.
enum Reply {
    Lines(Vec<String>),
    Close,
}

struct Fake {
    api: u32,
    /// Sent right after the handshake.
    then: Vec<String>,
    /// Closes the connection after the handshake and `then`.
    close: bool,
    answer: fn(&Value) -> Reply,
}

impl Fake {
    fn api(api: u32, answer: fn(&Value) -> Reply) -> Self {
        Self {
            api,
            then: Vec::new(),
            close: false,
            answer,
        }
    }
}

fn nothing(_: &Value) -> Reply {
    Reply::Lines(Vec::new())
}

const STATE: &str = r#"{"type":"state","device":null,"lock":false,"lcd":false}"#;
const LCD: &str =
    r#"{"type":"lcd","owner":false,"brightness":100,"text_size":"small","theme":"light"}"#;

/// Runs smectl with `args` against `fake`; returns its output and the lines it sent after its
/// `hello`.
fn run(name: &str, fake: Fake, args: &[&str]) -> (Output, Vec<Value>) {
    let socket = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("smectl-{name}.sock"));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        // A smectl that never connects: fail, not hang.
        listener.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(start.elapsed().as_secs() < 5, "smectl did not connect");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => break result.unwrap().0,
            }
        };
        stream.set_nonblocking(false).unwrap();
        // A smectl that waits for something the fake never sends: fail, not hang.
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["client"], "smectl");
        let mut out = vec![
            json!({"type": "hello", "api": fake.api, "server": "fake 0"}).to_string(),
            STATE.to_owned(),
        ];
        if fake.api >= 2 {
            out.push(LCD.to_owned());
        }
        out.extend(fake.then);
        for line in out {
            writeln!(stream, "{line}").unwrap();
        }
        let mut received = Vec::new();
        if fake.close {
            return received;
        }
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                return received;
            }
            let message: Value = serde_json::from_str(&line).unwrap();
            let reply = (fake.answer)(&message);
            received.push(message);
            match reply {
                Reply::Lines(lines) => {
                    for line in lines {
                        writeln!(stream, "{line}").unwrap();
                    }
                }
                Reply::Close => return received,
            }
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_smectl"))
        .arg("--socket")
        .arg(&socket)
        .args(args)
        .output()
        .unwrap();
    let received = server.join().unwrap();
    (output, received)
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn state_prints_the_state_after_the_handshake() {
    let (output, received) = run("state", Fake::api(1, nothing), &["state"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), format!("{STATE}\n"));
    assert!(received.is_empty(), "{received:?}");
}

fn lock_answer(message: &Value) -> Reply {
    if message["type"] == "get_state" {
        return Reply::Lines(vec![
            r#"{"type":"state","device":null,"lock":true,"lcd":false}"#.into(),
        ]);
    }
    Reply::Lines(Vec::new())
}

#[test]
fn lock_sends_set_lock_and_prints_the_lock_smed_reports() {
    let (output, received) = run("lock-on", Fake::api(1, lock_answer), &["lock", "on"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "lock on\n");
    assert_eq!(
        received,
        [
            json!({"type": "set_lock", "on": true}),
            json!({"type": "get_state"})
        ]
    );

    let (output, received) = run("lock-off", Fake::api(1, nothing_state), &["lock", "off"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "lock off\n");
    assert_eq!(received[0], json!({"type": "set_lock", "on": false}));
}

fn nothing_state(message: &Value) -> Reply {
    if message["type"] == "get_state" {
        return Reply::Lines(vec![STATE.into()]);
    }
    Reply::Lines(Vec::new())
}

#[test]
fn lcd_without_options_prints_the_settings() {
    let (output, received) = run("lcd", Fake::api(2, nothing), &["lcd"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), format!("{LCD}\n"));
    assert!(received.is_empty(), "{received:?}");
}

fn set_lcd_answer(message: &Value) -> Reply {
    if message["type"] == "set_lcd" {
        let brightness = &message["brightness"];
        return Reply::Lines(vec![
            json!({"type": "lcd", "owner": false, "brightness": brightness, "text_size": "small", "theme": "light"})
                .to_string(),
        ]);
    }
    Reply::Lines(Vec::new())
}

#[test]
fn lcd_with_options_sends_set_lcd_and_prints_the_answer() {
    let (output, received) = run(
        "lcd-set",
        Fake::api(2, set_lcd_answer),
        &["lcd", "--brightness", "40"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(received, [json!({"type": "set_lcd", "brightness": 40})]);
    let printed: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(printed["brightness"], 40);
}

fn refuse(message: &Value) -> Reply {
    if message["type"] == "set_lcd" || message["type"] == "get_screen" {
        return Reply::Lines(vec![r#"{"type":"error","message":"not now"}"#.into()]);
    }
    Reply::Lines(Vec::new())
}

#[test]
fn an_error_from_smed_is_printed_and_fails() {
    for (name, args) in [
        ("lcd-error", &["lcd", "--theme", "dark"][..]),
        ("screenshot-error", &["screenshot", "never.png"][..]),
    ] {
        let (output, _) = run(name, Fake::api(2, refuse), args);
        assert!(!output.status.success(), "{name}");
        assert_eq!(stderr(&output), "smectl: not now\n", "{name}");
    }
}

#[test]
fn the_lcd_commands_need_api_2() {
    for (name, args) in [
        ("api1-lcd", &["lcd"][..]),
        ("api1-demo", &["demo-page"][..]),
        ("api1-screenshot", &["screenshot", "never.png"][..]),
    ] {
        let (output, received) = run(name, Fake::api(1, nothing), args);
        assert!(!output.status.success(), "{name}");
        assert_eq!(
            stderr(&output),
            "smectl: fake 0 speaks API 1; the LCD needs 2\n",
            "{name}"
        );
        assert!(received.is_empty(), "{name}: {received:?}");
    }
}

/// A PNG's first bytes; smectl writes what smed sends without decoding it.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsmectl";

fn screen(message: &Value) -> Reply {
    if message["type"] == "get_screen" {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.encode(PNG);
        return Reply::Lines(vec![
            LCD.into(),
            json!({"type": "screen", "png": png}).to_string(),
        ]);
    }
    Reply::Lines(Vec::new())
}

#[test]
fn screenshot_saves_the_png_smed_sends() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("smectl-screenshot.png");
    let _ = std::fs::remove_file(&path);
    let file = path.to_str().unwrap();
    let (output, received) = run("screenshot", Fake::api(2, screen), &["screenshot", file]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), format!("{file}\n"));
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    assert_eq!(received, [json!({"type": "get_screen"})]);
}

fn switch(message: &Value) -> Reply {
    match message["type"].as_str() {
        Some("focus") => Reply::Lines(vec![
            r#"{"type":"command","id":"switch","env":"demo","key":"11","press":"short"}"#.into(),
        ]),
        Some("set_environment") => Reply::Close,
        _ => Reply::Lines(Vec::new()),
    }
}

#[test]
fn demo_page_sends_the_page_and_key_11_switches_the_environment() {
    let (output, received) = run("demo", Fake::api(2, switch), &["demo-page"]);
    let types: Vec<&str> = received
        .iter()
        .map(|m| m["type"].as_str().unwrap())
        .collect();
    let (icons, rest) = types.split_at(types.iter().take_while(|t| **t == "set_icon").count());
    assert!(!icons.is_empty(), "{types:?}");
    assert_eq!(
        rest,
        [
            "set_commands",
            "set_environments",
            "focus",
            "set_environment"
        ]
    );
    let n = icons.len();
    // The demo environment puts the commands on the keys from "1" on.
    let keys = &received[n + 1]["environments"][0]["keys"];
    assert_eq!(received[n + 1]["environments"][0]["id"], "demo");
    assert!(keys.get("1").is_some() && keys.get("0").is_none(), "{keys}");
    assert_eq!(received[n + 2], json!({"type": "focus", "on": true}));
    assert_eq!(
        received[n + 3],
        json!({"type": "set_environment", "id": "shapes"})
    );
    let printed: Vec<Value> = stdout(&output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        printed
            .iter()
            .any(|m| m["type"] == "command" && m["id"] == "switch"),
        "{printed:?}"
    );
    // smed closed the connection: the only way demo-page ends besides Ctrl-C.
    assert!(!output.status.success());
    assert!(stderr(&output).ends_with("smectl: smed closed the connection\n"));
}

#[test]
fn monitor_prints_every_message_until_smed_closes() {
    let key = r#"{"type":"key","key":"1","id":1,"press":"short","down":true}"#;
    let fake = Fake {
        then: vec![key.into()],
        close: true,
        ..Fake::api(1, nothing)
    };
    let (output, _) = run("monitor", fake, &["monitor"]);
    assert_eq!(stdout(&output), format!("{STATE}\n{key}\n"));
    assert!(!output.status.success());
    assert_eq!(stderr(&output), "smectl: smed closed the connection\n");
}

#[test]
fn an_unknown_command_prints_the_usage_and_fails() {
    let (output, received) = run("unknown", Fake::api(1, nothing), &["frobnicate"]);
    assert!(!output.status.success());
    assert!(stderr(&output).starts_with("smectl: usage: smectl"));
    assert!(received.is_empty());

    // --socket with its path but no command: the usage on stdout, and success.
    let output = Command::new(env!("CARGO_BIN_EXE_smectl"))
        .args(["--socket", "/nonexistent/smed.sock"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("usage: smectl"));

    // --socket without its path: the usage, before connecting anywhere.
    let output = Command::new(env!("CARGO_BIN_EXE_smectl"))
        .arg("--socket")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).starts_with("smectl: usage: smectl"));
}
