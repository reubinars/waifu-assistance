//! `%LOCALAPPDATA%\island\config.toml`.

use crate::view::{DisplayMode, Spot};
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
    /// Run (a URI or a program) when a jump finds no terminal window; "" = do nothing.
    #[serde(default)]
    pub terminal_launch: String,
    /// Where the island sits across the main monitor: 0 = far left, 0.5 = centre, 1 = far right.
    /// Older configs called it `position`.
    #[serde(alias = "position")]
    pub position_x: f32,
    /// And down it: 0 = top, 1 = bottom (just above the taskbar). Attached ignores it.
    pub position_y: f32,
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
            terminal_launch: String::new(),
            position_x: 0.5,
            position_y: 0.0,
        }
    }
}

impl Config {
    /// Missing keys take their defaults.
    pub fn parse(s: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn spot(&self) -> Spot {
        Spot::new(self.display_mode, self.position_x, self.position_y)
    }

    /// Saves where the island was moved to; attached only moves sideways, so the pill keeps its height.
    pub fn set_spot(&mut self, s: Spot) {
        self.position_x = s.x;
        if self.display_mode == DisplayMode::Pill {
            self.position_y = s.y;
        }
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

    #[test]
    fn terminal_launch_is_optional_and_roundtrips() {
        assert_eq!(Config::default().terminal_launch, "");
        let warp = Config::parse("terminal_launch = \"warp://tab_config/herdr\"").unwrap();
        assert_eq!(warp.terminal_launch, "warp://tab_config/herdr");
        assert_eq!(Config::parse(&warp.to_toml()).unwrap(), warp);
    }

    #[test]
    fn position_defaults_to_the_top_centre_and_roundtrips() {
        assert_eq!(Config::default().spot(), Spot::TOP_CENTRE);
        let moved = Config::parse("position_x = 0.3\nposition_y = 1.0").unwrap();
        assert_eq!(moved.spot(), Spot { x: 0.3, y: 1.0 });
        assert_eq!(Config::parse(&moved.to_toml()).unwrap(), moved);
    }

    #[test]
    fn an_old_top_edge_position_still_loads() {
        let old = Config::parse("position = 0.3").unwrap();
        assert_eq!(old.spot(), Spot { x: 0.3, y: 0.0 });
    }

    #[test]
    fn attached_keeps_the_pills_height() {
        let mut c = Config { display_mode: DisplayMode::Attached, position_y: 1.0, ..Config::default() };
        assert_eq!(c.spot(), Spot { x: 0.5, y: 0.0 });
        c.set_spot(Spot { x: 0.1, y: 0.0 });
        c.display_mode = DisplayMode::Pill;
        assert_eq!(c.spot(), Spot { x: 0.1, y: 1.0 });
    }
}
