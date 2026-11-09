//! `%LOCALAPPDATA%\island\config.toml`.

use crate::view::DisplayMode;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display_mode: DisplayMode,
    pub start_with_windows: bool,
    /// "" = the default distro.
    pub wsl_distro: String,
    /// Path inside WSL; `$HOME` is expanded by the shell.
    pub bridge_path: String,
    /// Window class to bring forward after focusing an agent.
    pub terminal_window_class: String,
    /// Optional executable filename; takes precedence over the window class.
    #[serde(default)]
    pub terminal_process: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            display_mode: DisplayMode::Pill,
            start_with_windows: true,
            wsl_distro: String::new(),
            bridge_path: "$HOME/.local/bin/island-bridge".into(),
            terminal_window_class: "CASCADIA_HOSTING_WINDOW_CLASS".into(),
            terminal_process: String::new(),
        }
    }
}

impl Config {
    /// Missing keys take their defaults.
    pub fn parse(s: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_partial() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let d = Config::default();
        assert_eq!(d.display_mode, DisplayMode::Pill);
        assert!(d.start_with_windows);
        assert_eq!(d.wsl_distro, "");
        assert_eq!(d.bridge_path, "$HOME/.local/bin/island-bridge");
        assert_eq!(d.terminal_window_class, "CASCADIA_HOSTING_WINDOW_CLASS");
        let c = Config::parse("display_mode = \"attached\"").unwrap();
        assert_eq!(c.display_mode, DisplayMode::Attached);
        assert!(c.start_with_windows);
        assert!(Config::parse("display_mode = \"floating\"").is_err());
    }

    #[test]
    fn config_roundtrip() {
        let c = Config { wsl_distro: "Ubuntu".into(), ..Config::default() };
        assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn terminal_process_is_optional_and_roundtrips() {
        let old = Config::parse("terminal_window_class = \"custom\"").unwrap();
        assert_eq!(old.terminal_process, "");
        let warp = Config::parse("terminal_process = \"WaRp.ExE\"").unwrap();
        assert_eq!(warp.terminal_process, "WaRp.ExE");
        assert_eq!(Config::parse(&warp.to_toml()).unwrap(), warp);
    }
}
