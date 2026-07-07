//! Loads a color palette from disk so the UI can match the user's rice on
//! Linux, or a manually-picked preset on Windows, without changing any code.

// Presets and the per-token accessors are for the upcoming theme switcher and
// custom widget styles; they are part of the palette's public surface.
#![allow(dead_code)]

use iced::Color;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    pub background: String,
    pub surface: String,
    pub surface_alt: String,
    pub text: String,
    pub text_muted: String,
    pub accent: String,
    pub accent_text: String,
    pub danger: String,
    pub success: String,
    pub border: String,
}

impl Default for Palette {
    /// Catppuccin Mocha, a sane default until the user drops in their own rice colors.
    fn default() -> Self {
        Self {
            background: "#1e1e2e".into(),
            surface: "#181825".into(),
            surface_alt: "#313244".into(),
            text: "#cdd6f4".into(),
            text_muted: "#a6adc8".into(),
            accent: "#89b4fa".into(),
            accent_text: "#11111b".into(),
            danger: "#f38ba8".into(),
            success: "#a6e3a1".into(),
            border: "#45475a".into(),
        }
    }
}

impl Palette {
    pub fn load_or_default(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn background(&self) -> Color {
        parse_hex(&self.background)
    }
    pub fn surface(&self) -> Color {
        parse_hex(&self.surface)
    }
    pub fn surface_alt(&self) -> Color {
        parse_hex(&self.surface_alt)
    }
    pub fn text(&self) -> Color {
        parse_hex(&self.text)
    }
    pub fn text_muted(&self) -> Color {
        parse_hex(&self.text_muted)
    }
    pub fn accent(&self) -> Color {
        parse_hex(&self.accent)
    }
    pub fn accent_text(&self) -> Color {
        parse_hex(&self.accent_text)
    }
    pub fn danger(&self) -> Color {
        parse_hex(&self.danger)
    }
    pub fn success(&self) -> Color {
        parse_hex(&self.success)
    }
    pub fn border(&self) -> Color {
        parse_hex(&self.border)
    }
}

/// Sentinel theme choice meaning "load colors from theme.toml".
pub const CUSTOM: &str = "custom";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    theme: Option<String>,
}

/// The persisted theme choice: a preset name, or [`CUSTOM`] (the default).
pub fn load_choice(config_path: &PathBuf) -> String {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<Config>(&s).ok())
        .and_then(|c| c.theme)
        .unwrap_or_else(|| CUSTOM.to_string())
}

pub fn save_choice(config_path: &PathBuf, choice: &str) -> anyhow::Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let config = Config {
        theme: Some(choice.to_string()),
    };
    std::fs::write(config_path, toml::to_string_pretty(&config)?)?;
    Ok(())
}

/// Resolves a theme choice to a palette: [`CUSTOM`] reads theme.toml,
/// anything else looks up the presets (falling back to the default palette).
pub fn palette_for_choice(choice: &str, theme_toml: &PathBuf) -> Palette {
    if choice == CUSTOM {
        Palette::load_or_default(theme_toml)
    } else {
        presets()
            .into_iter()
            .find(|(name, _)| *name == choice)
            .map(|(_, palette)| palette)
            .unwrap_or_default()
    }
}

/// A handful of built-in presets, mainly meant for the Windows theme switcher.
pub fn presets() -> Vec<(&'static str, Palette)> {
    vec![
        ("Catppuccin Mocha", Palette::default()),
        (
            "Nord",
            Palette {
                background: "#2e3440".into(),
                surface: "#3b4252".into(),
                surface_alt: "#434c5e".into(),
                text: "#eceff4".into(),
                text_muted: "#d8dee9".into(),
                accent: "#88c0d0".into(),
                accent_text: "#2e3440".into(),
                danger: "#bf616a".into(),
                success: "#a3be8c".into(),
                border: "#4c566a".into(),
            },
        ),
        (
            "Gruvbox Dark",
            Palette {
                background: "#282828".into(),
                surface: "#3c3836".into(),
                surface_alt: "#504945".into(),
                text: "#ebdbb2".into(),
                text_muted: "#bdae93".into(),
                accent: "#d79921".into(),
                accent_text: "#282828".into(),
                danger: "#cc241d".into(),
                success: "#98971a".into(),
                border: "#665c54".into(),
            },
        ),
    ]
}

impl Palette {
    /// Builds a full iced [`iced::Theme::custom`] from the rice/preset colors,
    /// so every default widget style (buttons, inputs, scrollbars...) picks
    /// them up automatically without per-widget style closures.
    pub fn to_iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            "Mesh".to_string(),
            iced::theme::palette::Palette {
                background: self.background(),
                text: self.text(),
                primary: self.accent(),
                success: self.success(),
                warning: self.accent(),
                danger: self.danger(),
            },
        )
    }
}

fn parse_hex(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    Color::from_rgb8(r, g, b)
}
