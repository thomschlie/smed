//! smed's settings file: `$XDG_CONFIG_HOME/smed/config.toml` (or `~/.config/smed/config.toml`).
//! smed reads it at start and never writes it.
//!
//! ```toml
//! [lcd]
//! brightness = 100      # 0-100
//! text_size = "small"   # or "large"
//! theme = "dark"        # or "light"
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;
use smed_client::{TextSize, Theme};

/// The LCD settings: from the file at start, then changed by `set_lcd` until smed restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LcdSettings {
    pub brightness: u8,
    pub text_size: TextSize,
    pub theme: Theme,
}

impl Default for LcdSettings {
    fn default() -> Self {
        Self {
            brightness: MAX_BRIGHTNESS,
            text_size: TextSize::Small,
            theme: Theme::Dark,
        }
    }
}

/// Feature report 0x11 takes 0-100 (docs/protocol.md).
pub const MAX_BRIGHTNESS: u8 = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub lcd: LcdSettings,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        config.lcd.brightness = config.lcd.brightness.min(MAX_BRIGHTNESS);
        Ok(config)
    }

    /// Reads `path`. A missing file gives the defaults; an unreadable or invalid one is an
    /// error.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// `$XDG_CONFIG_HOME/smed/config.toml`, else `$HOME/.config/smed/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(dir.join("smed/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_gives_the_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(
            Config::default().lcd,
            LcdSettings {
                brightness: 100,
                text_size: TextSize::Small,
                theme: Theme::Dark
            }
        );
    }

    #[test]
    fn lcd_settings_are_read_and_brightness_is_capped() {
        let config =
            Config::parse("[lcd]\nbrightness = 250\ntext_size = \"large\"\ntheme = \"light\"\n")
                .unwrap();
        assert_eq!(
            config.lcd,
            LcdSettings {
                brightness: 100,
                text_size: TextSize::Large,
                theme: Theme::Light
            }
        );
        assert_eq!(
            Config::parse("[lcd]\nbrightness = 40\n")
                .unwrap()
                .lcd
                .brightness,
            40
        );
    }

    #[test]
    fn a_typo_is_an_error() {
        assert!(Config::parse("[lcd]\nbrigthness = 40\n").is_err());
        assert!(Config::parse("[lcd]\ntheme = \"blue\"\n").is_err());
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let path = std::env::temp_dir().join("smed-no-such-dir/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }
}
