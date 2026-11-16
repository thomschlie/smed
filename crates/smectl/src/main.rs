//! smectl: command-line client for smed (docs/api.md).

mod demo;

use std::path::PathBuf;
use std::process::ExitCode;

use smed_client::{Client, ServerMessage, TextSize, Theme, to_line};

const USAGE: &str = "usage: smectl [--socket PATH] COMMAND\n\
\n\
commands:\n\
  monitor        print every message from smed as a JSON line until interrupted\n\
  state          print smed's state\n\
  lock on|off    switch the rotation lock\n\
  lcd [--brightness 0-100] [--text-size small|large] [--theme dark|light]\n\
                 print the LCD settings, or change them until smed restarts\n\
  demo-page      show a demo page on the LCD until interrupted; key 11 switches it\n\
  screenshot FILE.png\n\
                 save what smed draws on the LCD now (works without the device)";

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
        ["lcd", options @ ..] => {
            let request = lcd_options(options)?;
            need_lcd_api(&client)?;
            // smed sends `lcd` after the handshake, and in answer to every `set_lcd`.
            let current = next_lcd(&mut client)?;
            let settings = match request {
                Some((brightness, text_size, theme)) => {
                    client
                        .set_lcd(brightness, text_size, theme)
                        .map_err(|e| e.to_string())?;
                    next_lcd(&mut client)?
                }
                None => current,
            };
            print!("{}", to_line(&settings));
            Ok(())
        }
        ["demo-page"] => {
            need_lcd_api(&client)?;
            demo::send(&mut client).map_err(|e| e.to_string())?;
            eprintln!(
                "smectl: demo page sent; key 11 switches the environment; it stays while \
                 smectl runs (Ctrl-C ends it)"
            );
            loop {
                match client.next_message().map_err(|e| e.to_string())? {
                    Some(message) => {
                        print!("{}", to_line(&message));
                        // The callback: key 11's command switches the environment.
                        if let ServerMessage::Command { id, env, .. } = &message
                            && id == demo::SWITCH
                        {
                            let other = demo::other_environment(env);
                            client.set_environment(other).map_err(|e| e.to_string())?;
                        }
                    }
                    None => return Err("smed closed the connection".into()),
                }
            }
        }
        ["screenshot", path] => {
            need_lcd_api(&client)?;
            client.get_screen().map_err(|e| e.to_string())?;
            loop {
                match next(&mut client)? {
                    message @ ServerMessage::Screen { .. } => {
                        let bytes = message.screen_png().ok_or("smed sent invalid base64")?;
                        std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))?;
                        println!("{path}");
                        return Ok(());
                    }
                    ServerMessage::Error { message } => return Err(message),
                    _ => {}
                }
            }
        }
        _ => Err(USAGE.into()),
    }
}

/// Brightness, text size and theme for `set_lcd`; `None` keeps a setting.
type LcdRequest = (Option<u8>, Option<TextSize>, Option<Theme>);

fn need_lcd_api(client: &Client) -> Result<(), String> {
    if client.api < 2 {
        return Err(format!(
            "{} speaks API {}; the LCD needs 2",
            client.server, client.api
        ));
    }
    Ok(())
}

/// The `set_lcd` settings from the options, or `None` without options.
fn lcd_options(options: &[&str]) -> Result<Option<LcdRequest>, String> {
    let (mut brightness, mut text_size, mut theme) = (None, None, None);
    let mut options = options.iter();
    while let Some(option) = options.next() {
        let value = options.next().ok_or(USAGE)?;
        match *option {
            "--brightness" => {
                brightness = Some(
                    value
                        .parse::<u8>()
                        .ok()
                        .filter(|b| *b <= 100)
                        .ok_or("brightness is 0-100")?,
                );
            }
            "--text-size" => {
                text_size = Some(match *value {
                    "small" => TextSize::Small,
                    "large" => TextSize::Large,
                    _ => return Err("text size is small or large".into()),
                });
            }
            "--theme" => {
                theme = Some(match *value {
                    "dark" => Theme::Dark,
                    "light" => Theme::Light,
                    _ => return Err("theme is dark or light".into()),
                });
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(
        (brightness.is_some() || text_size.is_some() || theme.is_some())
            .then_some((brightness, text_size, theme)),
    )
}

/// The next `lcd` message, skipping others (the handshake's `state`, motion, keys).
fn next_lcd(client: &mut Client) -> Result<ServerMessage, String> {
    loop {
        match next(client)? {
            ServerMessage::Error { message } => return Err(message),
            message @ ServerMessage::Lcd { .. } => return Ok(message),
            _ => {}
        }
    }
}

fn next(client: &mut Client) -> Result<ServerMessage, String> {
    client
        .next_message()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "smed closed the connection".into())
}
