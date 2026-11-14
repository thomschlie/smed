//! smectl: command-line client for smed (docs/api.md).

use std::path::PathBuf;
use std::process::ExitCode;

use smed_client::{Client, ServerMessage, to_line};

const USAGE: &str = "usage: smectl [--socket PATH] COMMAND\n\
\n\
commands:\n\
  monitor        print every message from smed as a JSON line until interrupted\n\
  state          print smed's state\n\
  lock on|off    switch the rotation lock";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("smectl: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut socket = None;
    if args.first().map(String::as_str) == Some("--socket") {
        if args.len() < 2 {
            return Err(USAGE.into());
        }
        socket = Some(PathBuf::from(args.remove(1)));
        args.remove(0);
    }
    let command: Vec<&str> = args.iter().map(String::as_str).collect();
    if matches!(command.as_slice(), [] | ["-h" | "--help"]) {
        println!("{USAGE}");
        return Ok(());
    }

    let mut client = match &socket {
        Some(path) => Client::connect_to(path, "smectl"),
        None => Client::connect("smectl"),
    }
    .map_err(|e| format!("connecting to smed: {e}"))?;

    match command.as_slice() {
        ["monitor"] => loop {
            match client.next_message().map_err(|e| e.to_string())? {
                Some(message) => print!("{}", to_line(&message)),
                None => return Err("smed closed the connection".into()),
            }
        },
        ["state"] => {
            // The handshake is followed by a state message.
            let state = next(&mut client)?;
            print!("{}", to_line(&state));
            Ok(())
        }
        ["lock", on @ ("on" | "off")] => {
            let on = *on == "on";
            next(&mut client)?;
            client.set_lock(on).map_err(|e| e.to_string())?;
            client.get_state().map_err(|e| e.to_string())?;
            // Wait for the state that answers get_state: then smed has handled set_lock too.
            loop {
                if let ServerMessage::State { lock, .. } = next(&mut client)? {
                    println!("lock {}", if lock { "on" } else { "off" });
                    return Ok(());
                }
            }
        }
        _ => Err(USAGE.into()),
    }
}

fn next(client: &mut Client) -> Result<ServerMessage, String> {
    client
        .next_message()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "smed closed the connection".into())
}
