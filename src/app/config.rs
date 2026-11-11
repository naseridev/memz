use crate::app::state::{SortMode, ViewMode};
use ratatui::style::Color;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreset {
    #[default]
    EmberOrange,
    CyberCyan,
    MatrixGreen,
    RoyalPurple,
    CrimsonRed,
    SolarGold,
    ArcticBlue,
    RosePink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TickMode {
    #[default]
    Auto,
    Fast,
    Normal,
    Eco,
}

impl TickMode {
    pub fn step_faster(self) -> Self {
        match self {
            Self::Auto | Self::Normal => Self::Fast,
            Self::Eco => Self::Normal,
            Self::Fast => Self::Auto,
        }
    }

    pub fn step_slower(self) -> Self {
        match self {
            Self::Auto | Self::Fast => Self::Normal,
            Self::Normal => Self::Eco,
            Self::Eco => Self::Auto,
        }
    }

    pub fn label(self, active_ms: u64) -> String {
        let ms = if active_ms > 0 { active_ms } else { 1000 };
        match self {
            Self::Auto => format!("Auto ({:.1}s)", ms as f64 / 1000.0),
            Self::Fast => String::from("Fast (0.5s)"),
            Self::Normal => String::from("Normal (1.0s)"),
            Self::Eco => String::from("Eco (2.0s)"),
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fast => "fast",
            Self::Normal => "normal",
            Self::Eco => "eco",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "fast" => Self::Fast,
            "normal" => Self::Normal,
            "eco" => Self::Eco,
            _ => Self::Auto,
        }
    }
}

pub const THEME_PRESETS: [ThemePreset; 8] = [
    ThemePreset::EmberOrange,
    ThemePreset::CyberCyan,
    ThemePreset::MatrixGreen,
    ThemePreset::RoyalPurple,
    ThemePreset::CrimsonRed,
    ThemePreset::SolarGold,
    ThemePreset::ArcticBlue,
    ThemePreset::RosePink,
];

impl ThemePreset {
    pub fn name(self) -> &'static str {
        match self {
            Self::EmberOrange => "Ember Orange",
            Self::CyberCyan => "Cyber Cyan",
            Self::MatrixGreen => "Matrix Green",
            Self::RoyalPurple => "Royal Purple",
            Self::CrimsonRed => "Crimson Red",
            Self::SolarGold => "Solar Gold",
            Self::ArcticBlue => "Arctic Blue",
            Self::RosePink => "Rose Pink",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::EmberOrange => Color::Rgb(255, 115, 40),
            Self::CyberCyan => Color::Rgb(0, 215, 255),
            Self::MatrixGreen => Color::Rgb(50, 225, 100),
            Self::RoyalPurple => Color::Rgb(180, 110, 255),
            Self::CrimsonRed => Color::Rgb(255, 75, 95),
            Self::SolarGold => Color::Rgb(255, 200, 45),
            Self::ArcticBlue => Color::Rgb(90, 165, 255),
            Self::RosePink => Color::Rgb(255, 110, 185),
        }
    }

    pub fn index(self) -> usize {
        THEME_PRESETS
            .iter()
            .position(|&preset| preset == self)
            .unwrap_or(0)
    }

    fn as_key(self) -> &'static str {
        match self {
            Self::EmberOrange => "ember_orange",
            Self::CyberCyan => "cyber_cyan",
            Self::MatrixGreen => "matrix_green",
            Self::RoyalPurple => "royal_purple",
            Self::CrimsonRed => "crimson_red",
            Self::SolarGold => "solar_gold",
            Self::ArcticBlue => "arctic_blue",
            Self::RosePink => "rose_pink",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "cyber_cyan" => Self::CyberCyan,
            "matrix_green" => Self::MatrixGreen,
            "royal_purple" => Self::RoyalPurple,
            "crimson_red" => Self::CrimsonRed,
            "solar_gold" => Self::SolarGold,
            "arctic_blue" => Self::ArcticBlue,
            "rose_pink" => Self::RosePink,
            _ => Self::EmberOrange,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    pub theme: ThemePreset,
    pub view_mode: ViewMode,
    pub sort_mode: SortMode,
    pub tick_mode: TickMode,
    pub grouped: bool,
}

impl UserConfig {
    pub fn load() -> Self {
        let Some(path) = config_file_path() else {
            return Self::default();
        };

        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };

        let mut cfg = Self::default();

        for line in content.lines() {
            let Some((raw_key, raw_val)) = line.split_once('=') else {
                continue;
            };

            let key = raw_key.trim();
            let val = raw_val.trim();

            match key {
                "theme" => cfg.theme = ThemePreset::from_key(val),
                "view_mode" => cfg.view_mode = ViewMode::from_key(val),
                "sort_mode" => cfg.sort_mode = SortMode::from_key(val),
                "tick_mode" => cfg.tick_mode = TickMode::from_key(val),
                "grouped" => cfg.grouped = val == "true",
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self) {
        let Some(path) = config_file_path() else {
            return;
        };

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let body = format!(
            "theme={}\nview_mode={}\nsort_mode={}\ntick_mode={}\ngrouped={}\n",
            self.theme.as_key(),
            self.view_mode.as_key(),
            self.sort_mode.as_key(),
            self.tick_mode.as_key(),
            self.grouped
        );

        let _ = fs::write(path, body);
    }
}

fn config_file_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".config")))
        .unwrap_or_else(env::temp_dir);

    Some(base.join("memz").join("state.conf"))
}
