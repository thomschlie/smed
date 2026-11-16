//! `smectl demo-page`: a page with labels and icons to try the LCD without an application.
//! The icons are simple shapes drawn for this demo.

use std::io;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use smed_client::{App, Client, Command, Environment};

const CUBE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 2 21 7v10l-9 5-9-5V7z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/><path d="M3 7l9 5 9-5M12 12v10" fill="none" stroke="currentColor" stroke-width="2"/></svg>"#;
const LINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 20 20 4" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/><circle cx="4" cy="20" r="2.5" fill="currentColor"/><circle cx="20" cy="4" r="2.5" fill="currentColor"/></svg>"#;
const CIRCLE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5" fill="none" stroke="currentColor" stroke-width="2.5"/><circle cx="12" cy="12" r="1.8" fill="currentColor"/></svg>"#;
const RECTANGLE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="3" y="6" width="18" height="12" rx="1" fill="none" stroke="currentColor" stroke-width="2.5"/></svg>"#;
const UNDO: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9 5 4 10l5 5" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M4 10h10a6 6 0 0 1 0 12h-3" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/></svg>"#;
const REDO: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M15 5l5 5-5 5" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M20 10H10a6 6 0 0 0 0 12h3" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/></svg>"#;
/// Fixed colours instead of `currentColor`: drawn as they are in both themes.
const EXTRUDE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 16h12l4-4H8z" fill="#5aa0e6"/><path d="M8 12V4h12v8" fill="none" stroke="#e6a03c" stroke-width="2"/><path d="M14 9V2m-3 3 3-3 3 3" fill="none" stroke="#e6a03c" stroke-width="2" stroke-linecap="round"/></svg>"##;
/// An orange disc, 32×32 PNG: the PNG path.
const DISC_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAvklEQVR42t2XzQ2AIAxGWYAVnIYd2IEduoEHp2EHtnEDrQlcCII/0FYP70I03xMEWrXOk+JEfVFAIwZxCERcHNMjBSzika2Bj892Ezi+LFwIzgnx3VcC8CA4B54KLB3CE8tdAegYXp2JszXfBmGuCISBAqElYAeGJ2xNwBMI+DMBTRCe0CUBQyhgSgKOUMCVBIBQAEQKsC8B+0/Ivg3ZDyIRRzH7ZSTiOmYvSESUZCKKUhFluYjGRExr9s/ueAdSsKMVaIRG/QAAAABJRU5ErkJggg==";

/// The command on key 11: switches between the two environments.
pub const SWITCH: &str = "switch";

/// The environment to switch to from `env`.
pub fn other_environment(env: &str) -> &'static str {
    if env == "demo" { "shapes" } else { "demo" }
}

/// Sends the demo page: icons, commands, the two environments, then `focus`.
pub fn send(client: &mut Client) -> io::Result<()> {
    for (id, svg) in [
        ("cube", CUBE),
        ("line", LINE),
        ("circle", CIRCLE),
        ("rectangle", RECTANGLE),
        ("undo", UNDO),
        ("redo", REDO),
        ("extrude", EXTRUDE),
    ] {
        client.set_icon_svg(id, svg)?;
    }
    let disc = BASE64
        .decode(DISC_PNG)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    client.set_icon_png("disc", &disc)?;
    // "Demo": keys 1-10, and 11 switches to "Shapes" (12 stays empty to show an empty tile).
    // "Shapes": a few of the same commands on other keys, and 11 switches back.
    let commands = [
        ("line", "Line", Some("line")),
        ("circle", "Circle", Some("circle")),
        ("rectangle", "Rectangle", Some("rectangle")),
        ("extrude", "Extrude", Some("extrude")),
        ("png", "PNG icon", Some("disc")),
        ("text", "Text only", None),
        ("long", "A label far too long for one tile", Some("cube")),
        ("unbreakable", "Supercalifragilistic", None),
        ("undo", "Undo", Some("undo")),
        ("redo", "Redo", Some("redo")),
        (SWITCH, "Switch environment", Some("cube")),
    ];
    let catalogue: Vec<Command> = commands
        .iter()
        .map(|(id, label, icon)| {
            let command = Command::new(*id, *label);
            match icon {
                Some(icon) => command.with_icon(*icon),
                None => command,
            }
        })
        .collect();
    client.set_commands(&App::new("smectl demo").with_icon("cube"), &catalogue)?;
    let demo = commands.iter().enumerate().fold(
        Environment::new("demo", "Demo"),
        |env, (index, (id, _, _))| env.with_key((index + 1).to_string(), *id),
    );
    let shapes = [
        ("1", "circle"),
        ("2", "rectangle"),
        ("4", "extrude"),
        ("7", "undo"),
        ("8", "redo"),
        ("11", SWITCH),
    ]
    .into_iter()
    .fold(Environment::new("shapes", "Shapes"), |env, (key, id)| {
        env.with_key(key, id)
    });
    client.set_environments(&[demo, shapes])?;
    client.focus(true)
}
