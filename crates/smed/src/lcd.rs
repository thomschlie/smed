//! Which page the LCD shows: what each client sent (application, commands, environments,
//! icons), which client has the focus, and the LCD settings. Pure logic; the daemon draws.

use std::collections::HashMap;
use std::sync::Arc;

use smed_client::{App, Command, Environment, TextSize, Theme};

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
    icons: HashMap<String, Arc<Icon>>,
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

    pub fn set_commands(&mut self, client: u64, app: App, commands: Vec<Command>) -> bool {
        let Some(page) = self.clients.get_mut(&client) else {
            return false;
        };
        page.app = Some(app);
        page.commands = commands;
        self.shown(client)
    }

    pub fn set_environments(&mut self, client: u64, environments: Vec<Environment>) -> bool {
        let Some(page) = self.clients.get_mut(&client) else {
            return false;
        };
        page.environments = environments;
        self.shown(client)
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

    /// The page to show now: the owner's first environment, or smed's placeholder.
    pub fn page(&self) -> Page {
        let Some(client) = self.owner.and_then(|owner| self.clients.get(&owner)) else {
            return Page::placeholder();
        };
        let icon = |id: &Option<String>| id.as_ref().and_then(|id| client.icons.get(id)).cloned();
        let environment = client.environments.first();
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
        Command {
            id: id.into(),
            label: label.into(),
            icon: icon.map(Into::into),
        }
    }

    fn sketch() -> Environment {
        Environment {
            id: "sketch".into(),
            name: "Sketch".into(),
            keys: [
                ("1", "line"),
                ("3", "circle"),
                ("12", "gone"),
                ("t", "line"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        }
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
            !lcd.set_commands(1, App::default(), vec![]),
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
        assert!(lcd.set_commands(
            1,
            App {
                name: "SolidCraft".into(),
                icon: Some("pen".into())
            },
            vec![
                command("line", "Line", Some("pen")),
                command("circle", "Circle", Some("missing"))
            ],
        ));
        assert!(lcd.set_environments(1, vec![sketch()]));
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
        assert!(!lcd.set(Some(100), None, Some(Theme::Dark)));
        assert!(lcd.set(Some(250), Some(TextSize::Large), None));
        assert_eq!(lcd.settings().brightness, 100);
        assert_eq!(lcd.settings().text_size, TextSize::Large);
        assert!(lcd.set(Some(0), None, None));
        assert_eq!(lcd.settings().brightness, 0);
    }
}
