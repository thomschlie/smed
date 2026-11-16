//! Which page the LCD shows: what each client sent (application, commands, environments,
//! icons), which client has the focus, and the LCD settings. Pure logic; the daemon draws.

use std::collections::HashMap;
use std::sync::Arc;

use smed_client::{App, Command, Environment, Press, ServerMessage, TextSize, Theme, key_id};

use crate::config::{LcdSettings, MAX_BRIGHTNESS};
use crate::render::{Icon, Page, TILES, Tile};

/// What one client has sent.
#[derive(Debug, Default)]
struct ClientPage {
    /// The name from the client's `hello`, shown until it sends `set_commands`.
    name: String,
    app: Option<App>,
    commands: Vec<Command>,
    environments: Vec<Environment>,
    /// The environment `set_environment` chose; `None` (or one no longer sent) is the first.
    environment: Option<String>,
    icons: HashMap<String, Arc<Icon>>,
}

impl ClientPage {
    /// The environment in use: the chosen one, else the first.
    fn environment(&self) -> Option<&Environment> {
        self.environment
            .as_ref()
            .and_then(|id| self.environments.iter().find(|e| &e.id == id))
            .or_else(|| self.environments.first())
    }

    /// Icon ids the commands and the app use that this connection has not sent.
    fn unknown_icons(&self) -> Vec<String> {
        let mut unknown: Vec<String> = self
            .app
            .iter()
            .filter_map(|app| app.icon.as_ref())
            .chain(self.commands.iter().filter_map(|c| c.icon.as_ref()))
            .filter(|id| !self.icons.contains_key(*id))
            .cloned()
            .collect();
        unknown.sort();
        unknown.dedup();
        unknown
    }
}

/// The keys that have a tile, by their v1 names.
const TILE_KEYS: [&str; TILES] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12",
];

/// What a `set_*` request did: whether the LCD needs a new frame, and a problem to report as
/// `error` (the request is applied all the same, except where a method says otherwise).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub redraw: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct Lcd {
    settings: LcdSettings,
    clients: HashMap<u64, ClientPage>,
    owner: Option<u64>,
}

impl Lcd {
    pub fn new(settings: LcdSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> LcdSettings {
        self.settings
    }

    /// The client that has the LCD: the last one to report `focus: true`.
    pub fn owner(&self) -> Option<u64> {
        self.owner
    }

    pub fn connect(&mut self, client: u64, name: &str) {
        self.clients.insert(
            client,
            ClientPage {
                name: name.to_owned(),
                ..ClientPage::default()
            },
        );
    }

    /// Forgets the client. True when it had the LCD.
    pub fn disconnect(&mut self, client: u64) -> bool {
        self.clients.remove(&client);
        if self.owner == Some(client) {
            self.owner = None;
            return true;
        }
        false
    }

    /// The `set_*` messages below return whether the LCD needs a new frame: only when the
    /// client has it.
    fn shown(&self, client: u64) -> bool {
        self.owner == Some(client)
    }

    pub fn set_icon(&mut self, client: u64, id: String, icon: Icon) -> bool {
        let Some(page) = self.clients.get_mut(&client) else {
            return false;
        };
        page.icons.insert(id, Arc::new(icon));
        self.shown(client)
    }

    /// Replaces the catalogue. Icon ids not sent yet are reported; their tiles show no icon.
    pub fn set_commands(&mut self, client: u64, app: App, commands: Vec<Command>) -> Outcome {
        let Some(page) = self.clients.get_mut(&client) else {
            return Outcome::default();
        };
        page.app = Some(app);
        page.commands = commands;
        let unknown = page.unknown_icons();
        Outcome {
            redraw: self.shown(client),
            error: (!unknown.is_empty()).then(|| format!("unknown icon: {}", unknown.join(", "))),
        }
    }

    /// Replaces the environments. Unknown command ids and names that are no key are reported;
    /// those keys stay empty. Key names without a tile (`"t"`, `"v1"` …) are kept quietly.
    pub fn set_environments(&mut self, client: u64, environments: Vec<Environment>) -> Outcome {
        let Some(page) = self.clients.get_mut(&client) else {
            return Outcome::default();
        };
        let mut problems = Vec::new();
        for environment in &environments {
            for (key, command) in &environment.keys {
                if key_id(key).is_none() {
                    problems.push(format!("{}: no key \"{key}\"", environment.id));
                } else if !page.commands.iter().any(|c| &c.id == command) {
                    problems.push(format!("{}: unknown command {command}", environment.id));
                }
            }
        }
        page.environments = environments;
        Outcome {
            redraw: self.shown(client),
            error: (!problems.is_empty()).then(|| problems.join("; ")),
        }
    }

    /// Switches to environment `id`. An id the client has not sent is an error and changes
    /// nothing.
    pub fn set_environment(&mut self, client: u64, id: String) -> Outcome {
        let Some(page) = self.clients.get_mut(&client) else {
            return Outcome::default();
        };
        if !page.environments.iter().any(|e| e.id == id) {
            return Outcome {
                redraw: false,
                error: Some(format!("unknown environment {id}")),
            };
        }
        let before = page.environment().map(|e| e.id.clone());
        let changed = before.as_deref() != Some(id.as_str());
        page.environment = Some(id);
        Outcome {
            redraw: changed && self.shown(client),
            error: None,
        }
    }

    /// The `command` for a press of `key` (a v1 key name): the owner, and the message, if
    /// the owner's environment runs a command on that key. Only the keys 1-12 run commands.
    pub fn command(&self, key: &str, press: Press) -> Option<(u64, ServerMessage)> {
        if !TILE_KEYS.contains(&key) {
            return None;
        }
        let owner = self.owner?;
        let page = self.clients.get(&owner)?;
        let environment = page.environment()?;
        let id = environment.keys.get(key)?;
        page.commands.iter().any(|c| &c.id == id).then(|| {
            (
                owner,
                ServerMessage::Command {
                    id: id.clone(),
                    env: environment.id.clone(),
                    key: key.to_owned(),
                    press,
                },
            )
        })
    }

    /// True when the owner changed.
    pub fn focus(&mut self, client: u64, on: bool) -> bool {
        if !self.clients.contains_key(&client) {
            return false;
        }
        let owner = match (on, self.owner) {
            (true, _) => Some(client),
            (false, Some(owner)) if owner == client => None,
            (false, owner) => owner,
        };
        let changed = owner != self.owner;
        self.owner = owner;
        changed
    }

    /// Changes the given settings; brightness is capped at 100. True when one changed.
    pub fn set(
        &mut self,
        brightness: Option<u8>,
        text_size: Option<TextSize>,
        theme: Option<Theme>,
    ) -> bool {
        let before = self.settings;
        if let Some(brightness) = brightness {
            self.settings.brightness = brightness.min(MAX_BRIGHTNESS);
        }
        if let Some(text_size) = text_size {
            self.settings.text_size = text_size;
        }
        if let Some(theme) = theme {
            self.settings.theme = theme;
        }
        self.settings != before
    }

    /// The page to show now: the owner's environment, or smed's placeholder.
    pub fn page(&self) -> Page {
        let Some(client) = self.owner.and_then(|owner| self.clients.get(&owner)) else {
            return Page::placeholder();
        };
        let icon = |id: &Option<String>| id.as_ref().and_then(|id| client.icons.get(id)).cloned();
        let environment = client.environment();
        let mut tiles: [Option<Tile>; TILES] = Default::default();
        if let Some(environment) = environment {
            for (index, tile) in tiles.iter_mut().enumerate() {
                let key = (index + 1).to_string();
                *tile = environment
                    .keys
                    .get(&key)
                    .and_then(|command| client.commands.iter().find(|c| &c.id == command))
                    .map(|command| Tile {
                        label: command.label.clone(),
                        icon: icon(&command.icon),
                    });
            }
        }
        Page {
            title: client
                .app
                .as_ref()
                .map_or_else(|| client.name.clone(), |app| app.name.clone()),
            environment: environment.map(|environment| environment.name.clone()),
            app_icon: client.app.as_ref().and_then(|app| icon(&app.icon)),
            tiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"/>"#;

    fn command(id: &str, label: &str, icon: Option<&str>) -> Command {
        let command = Command::new(id, label);
        match icon {
            Some(icon) => command.with_icon(icon),
            None => command,
        }
    }

    fn sketch() -> Environment {
        Environment::new("sketch", "Sketch")
            .with_key("1", "line")
            .with_key("3", "circle")
            .with_key("12", "gone")
            .with_key("t", "line")
    }

    fn labels(page: &Page) -> Vec<Option<&str>> {
        page.tiles
            .iter()
            .map(|tile| tile.as_ref().map(|t| t.label.as_str()))
            .collect()
    }

    #[test]
    fn without_a_focused_client_smed_shows_its_own_page() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "solidcraft");
        assert!(
            !lcd.set_commands(1, App::default(), vec![]).redraw,
            "not shown yet"
        );
        assert_eq!(lcd.page().title, "smed");
        assert!(lcd.page().tiles.iter().all(Option::is_none));
    }

    #[test]
    fn the_focused_client_page_shows_its_first_environment() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "solidcraft");
        assert!(lcd.focus(1, true));
        assert_eq!(
            lcd.page().title,
            "solidcraft",
            "the hello name before set_commands"
        );
        assert!(lcd.set_icon(1, "pen".into(), Icon::from_svg(SVG).unwrap()));
        let outcome = lcd.set_commands(
            1,
            App::new("SolidCraft").with_icon("pen"),
            vec![
                command("line", "Line", Some("pen")),
                command("circle", "Circle", Some("missing")),
            ],
        );
        assert_eq!(
            outcome,
            Outcome {
                redraw: true,
                error: Some("unknown icon: missing".into())
            },
            "applied, and the unknown icon reported"
        );
        let outcome = lcd.set_environments(1, vec![sketch()]);
        assert!(outcome.redraw);
        assert_eq!(
            outcome.error.as_deref(),
            Some("sketch: unknown command gone"),
            "\"t\" is a key without a tile: kept quietly"
        );
        let page = lcd.page();
        assert_eq!(page.title, "SolidCraft");
        assert_eq!(page.environment.as_deref(), Some("Sketch"));
        assert!(page.app_icon.is_some());
        let mut expected = vec![None; 12];
        expected[0] = Some("Line");
        expected[2] = Some("Circle");
        assert_eq!(
            labels(&page),
            expected,
            "unknown commands and keys stay empty"
        );
        assert!(page.tiles[0].as_ref().unwrap().icon.is_some());
        assert!(
            page.tiles[2].as_ref().unwrap().icon.is_none(),
            "unknown icon id"
        );
    }

    #[test]
    fn focus_goes_to_the_last_client_reporting_it() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "a");
        lcd.connect(2, "b");
        assert!(lcd.focus(1, true));
        assert!(!lcd.focus(1, true), "no change");
        assert!(lcd.focus(2, true));
        assert_eq!(lcd.owner(), Some(2));
        assert!(
            !lcd.focus(1, false),
            "losing focus it does not have changes nothing"
        );
        assert_eq!(lcd.owner(), Some(2));
        assert!(lcd.focus(2, false));
        assert_eq!(lcd.owner(), None, "no stack: nobody has it now");
        assert!(lcd.focus(1, true));
        assert!(lcd.disconnect(1));
        assert_eq!(lcd.owner(), None);
        assert!(!lcd.disconnect(2));
        assert!(!lcd.focus(9, true), "unknown client");
    }

    #[test]
    fn a_client_state_is_replaced_and_dropped_with_it() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "a");
        lcd.focus(1, true);
        lcd.set_commands(1, App::default(), vec![command("line", "Line", None)]);
        lcd.set_environments(1, vec![sketch()]);
        lcd.set_commands(1, App::default(), vec![command("circle", "Kreis", None)]);
        let mut expected = vec![None; 12];
        expected[2] = Some("Kreis");
        assert_eq!(labels(&lcd.page()), expected);
        lcd.disconnect(1);
        lcd.connect(1, "a");
        lcd.focus(1, true);
        assert!(lcd.page().tiles.iter().all(Option::is_none));
    }

    #[test]
    fn settings_change_only_when_different() {
        let mut lcd = Lcd::default();
        assert!(!lcd.set(Some(100), None, Some(Theme::Light)));
        assert!(lcd.set(Some(250), Some(TextSize::Large), None));
        assert_eq!(lcd.settings().brightness, 100);
        assert_eq!(lcd.settings().text_size, TextSize::Large);
        assert!(lcd.set(Some(0), None, None));
        assert_eq!(lcd.settings().brightness, 0);
    }

    fn two_environments(lcd: &mut Lcd) {
        lcd.connect(1, "a");
        lcd.set_commands(
            1,
            App::default(),
            vec![command("line", "Line", None), command("box", "Box", None)],
        );
        // "t" has no tile: assigned here, it must still run nothing.
        let env = |id: &str, name: &str, key1: &str| {
            Environment::new(id, name)
                .with_key("1", key1)
                .with_key("t", key1)
        };
        lcd.set_environments(
            1,
            vec![env("sketch", "Sketch", "line"), env("part", "Part", "box")],
        );
    }

    #[test]
    fn set_environment_switches_the_page_and_the_commands() {
        let mut lcd = Lcd::default();
        two_environments(&mut lcd);
        assert_eq!(lcd.command("1", Press::Short), None, "no owner, no command");
        lcd.focus(1, true);
        assert_eq!(
            labels(&lcd.page())[0],
            Some("Line"),
            "the first environment"
        );
        assert_eq!(
            lcd.command("1", Press::Short),
            Some((
                1,
                ServerMessage::Command {
                    id: "line".into(),
                    env: "sketch".into(),
                    key: "1".into(),
                    press: Press::Short
                }
            ))
        );
        assert_eq!(
            lcd.set_environment(1, "part".into()),
            Outcome {
                redraw: true,
                error: None
            }
        );
        assert_eq!(lcd.page().environment.as_deref(), Some("Part"));
        assert_eq!(labels(&lcd.page())[0], Some("Box"));
        let Some((_, ServerMessage::Command { id, env, press, .. })) =
            lcd.command("1", Press::Long)
        else {
            panic!("a command");
        };
        assert_eq!(
            (id.as_str(), env.as_str(), press),
            ("box", "part", Press::Long)
        );
        assert!(!lcd.set_environment(1, "part".into()).redraw, "no change");
        assert_eq!(
            lcd.set_environment(1, "drawing".into()),
            Outcome {
                redraw: false,
                error: Some("unknown environment drawing".into())
            }
        );
        assert_eq!(lcd.page().environment.as_deref(), Some("Part"), "unchanged");
    }

    #[test]
    fn only_assigned_keys_1_to_12_run_commands() {
        let mut lcd = Lcd::default();
        two_environments(&mut lcd);
        lcd.focus(1, true);
        assert!(lcd.command("2", Press::Short).is_none(), "unassigned");
        assert!(
            lcd.command("t", Press::Short).is_none(),
            "\"t\" is assigned, but has no tile: no command"
        );
        lcd.connect(2, "b");
        lcd.focus(2, true);
        assert!(
            lcd.command("1", Press::Short).is_none(),
            "the owner has nothing on 1"
        );
    }

    #[test]
    fn a_missing_icon_is_named_once() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "a");
        let outcome = lcd.set_commands(
            1,
            App::new("A").with_icon("x"),
            vec![
                command("1", "One", Some("y")),
                command("2", "Two", Some("x")),
                command("3", "Three", Some("y")),
            ],
        );
        assert_eq!(outcome.error.as_deref(), Some("unknown icon: x, y"));
    }

    #[test]
    fn a_name_that_is_no_key_is_reported() {
        let mut lcd = Lcd::default();
        lcd.connect(1, "a");
        lcd.set_commands(1, App::default(), vec![command("line", "Line", None)]);
        let outcome = lcd.set_environments(
            1,
            vec![
                Environment::new("e", "E")
                    .with_key("13", "line")
                    .with_key("v1", "line"),
            ],
        );
        assert_eq!(outcome.error.as_deref(), Some("e: no key \"13\""));
    }
}
