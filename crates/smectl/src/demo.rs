//! `smectl demo-page`: a page with labels and icons to try the LCD without an application.
//! The icons are simple shapes drawn for this demo.

use std::collections::BTreeMap;

use smed_client::{App, ClientMessage, Command, Environment};

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

/// The messages that set up the demo page, `focus` last.
pub fn messages() -> Vec<ClientMessage> {
    let svg = |id: &str, source: &str| ClientMessage::SetIcon {
        id: id.into(),
        svg: Some(source.into()),
        png: None,
    };
    let mut messages = vec![
        svg("cube", CUBE),
        svg("line", LINE),
        svg("circle", CIRCLE),
        svg("rectangle", RECTANGLE),
        svg("undo", UNDO),
        svg("redo", REDO),
        svg("extrude", EXTRUDE),
        ClientMessage::SetIcon {
            id: "disc".into(),
            svg: None,
            png: Some(DISC_PNG.into()),
        },
    ];
    // Key 1-10; 11 and 12 stay unassigned to show empty tiles.
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
    ];
    messages.push(ClientMessage::SetCommands {
        app: App {
            name: "smectl demo".into(),
            icon: Some("cube".into()),
        },
        commands: commands
            .iter()
            .map(|(id, label, icon)| Command {
                id: (*id).into(),
                label: (*label).into(),
                icon: icon.map(Into::into),
            })
            .collect(),
    });
    let keys: BTreeMap<String, String> = commands
        .iter()
        .enumerate()
        .map(|(index, (id, _, _))| ((index + 1).to_string(), (*id).into()))
        .collect();
    messages.push(ClientMessage::SetEnvironments {
        environments: vec![Environment {
            id: "demo".into(),
            name: "Demo".into(),
            keys,
        }],
    });
    messages.push(ClientMessage::Focus { on: true });
    messages
}
