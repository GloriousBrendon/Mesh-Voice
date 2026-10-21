//! Loads the UI colors and layout so Mesh wears the user's desktop: the rice's
//! `colors.conf` and `hyprland.conf` on Linux, a built-in preset or a
//! hand-written `theme.toml` anywhere. Five base colors come from the source;
//! the rest are derived (and contrast-checked) unless theme.toml sets them.


use iced::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// WCAG AA for normal text; derived text colors are pushed until they pass.
const MIN_CONTRAST: f32 = 4.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    /// `$bg2`: gaps / window background, inputs, code blocks.
    pub background: String,
    /// `$bg1`: pane fill.
    pub surface: String,
    /// `$surface`: selected rows, chips, active space tile.
    pub surface_alt: String,
    pub accent: String,
    pub text: String,
    // Derived when omitted (always omitted when following colors.conf).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_bright: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_muted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_ctrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_dim: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger: Option<String>,
}

impl Default for Palette {
    /// The "rice" defaults from design/DESIGN.md, used when colors.conf is missing.
    fn default() -> Self {
        Self {
            background: "#080c10".into(),
            surface: "#0f1822".into(),
            surface_alt: "#1f252e".into(),
            accent: "#d88f92".into(),
            text: "#a8b0be".into(),
            text_bright: None,
            text_muted: None,
            accent_text: None,
            border: None,
            border_ctrl: None,
            panel_dim: None,
            success: None,
            warning: None,
            danger: None,
        }
    }
}

/// A theme.toml either has the palette at top level (the original format) or
/// under `[colors]` (the exported format, next to `[layout]` and `[extras]`).
#[derive(Deserialize)]
struct ThemeFile {
    colors: Palette,
}

impl Palette {
    pub fn load_or_default(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| {
                toml::from_str::<ThemeFile>(&s)
                    .map(|f| f.colors)
                    .or_else(|_| toml::from_str::<Palette>(&s))
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Reads `$key = rgba(rrggbbaa)` lines from a rice `colors.conf`; keys it
    /// doesn't set keep their defaults.
    pub fn from_colors_conf(src: &str) -> Self {
        let mut palette = Palette::default();
        for (key, hex) in parse_colors_conf(src) {
            let slot = match key.as_str() {
                "bg2" => &mut palette.background,
                "bg1" => &mut palette.surface,
                "surface" => &mut palette.surface_alt,
                "accent" => &mut palette.accent,
                "text" => &mut palette.text,
                _ => continue,
            };
            *slot = hex;
        }
        palette
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
    pub fn accent(&self) -> Color {
        parse_hex(&self.accent)
    }
    pub fn text(&self) -> Color {
        parse_hex(&self.text)
    }
    /// Names, headings, active labels.
    pub fn text_bright(&self) -> Color {
        or_derive(&self.text_bright, || mix(self.text(), Color::WHITE, 0.7))
    }
    /// Secondary text; readable (≥ 4.5:1) on `surface_alt`.
    pub fn text_muted(&self) -> Color {
        or_derive(&self.text_muted, || {
            let base = self.surface_alt();
            ensure_contrast(mix(self.text(), base, 0.25), base)
        })
    }
    /// Text on an accent fill: near-black or near-white tinted toward the accent.
    pub fn accent_text(&self) -> Color {
        or_derive(&self.accent_text, || {
            let accent = self.accent();
            let dark = mix(Color::BLACK, accent, 0.12);
            let light = mix(Color::WHITE, accent, 0.12);
            if contrast(dark, accent) >= contrast(light, accent) {
                dark
            } else {
                light
            }
        })
    }
    /// Unfocused pane borders and separators.
    pub fn border(&self) -> Color {
        or_derive(&self.border, || mix(self.surface(), Color::WHITE, 0.08))
    }
    /// Input and chip borders.
    pub fn border_ctrl(&self) -> Color {
        or_derive(&self.border_ctrl, || mix(self.surface(), Color::WHITE, 0.12))
    }
    /// User panel and footer strips.
    pub fn panel_dim(&self) -> Color {
        or_derive(&self.panel_dim, || mix(self.background(), self.surface(), 0.5))
    }
    pub fn success(&self) -> Color {
        or_derive(&self.success, || parse_hex("#8fc79a"))
    }
    pub fn warning(&self) -> Color {
        or_derive(&self.warning, || parse_hex("#e0b36a"))
    }
    pub fn danger(&self) -> Color {
        or_derive(&self.danger, || parse_hex("#e07a7a"))
    }
    /// A network's tag colour, from the registry (each differs in lightness
    /// too, so tags aren't told apart by hue alone).
    pub fn network(&self, spec: &mesh_core::networks::NetworkSpec) -> Color {
        parse_hex(spec.color)
    }

    /// Builds a full iced [`iced::Theme::custom`] from the palette, so every
    /// default widget style (buttons, inputs, scrollbars...) picks it up.
    pub fn to_iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            "Mesh".to_string(),
            iced::theme::palette::Palette {
                background: self.background(),
                text: self.text(),
                primary: self.accent(),
                success: self.success(),
                warning: self.warning(),
                danger: self.danger(),
            },
        )
    }
}

/// Gaps, border and rounding, from hyprland.conf or the defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub gaps_in: f32,
    pub gaps_out: f32,
    pub border_size: f32,
    pub rounding: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            gaps_in: 4.0,
            gaps_out: 8.0,
            border_size: 2.0,
            rounding: 10.0,
        }
    }
}

impl Layout {
    /// Corner radius for buttons and inputs.
    pub fn control_radius(&self) -> f32 {
        self.rounding * 0.6
    }

    /// Reads `general { gaps_in gaps_out border_size }` and
    /// `decoration { rounding }`, following `source = …` includes.
    pub fn from_hyprland(path: &Path) -> Self {
        let mut layout = Layout::default();
        let mut seen = Vec::new();
        read_hyprland(path, &mut layout, &mut seen);
        layout
    }
}

fn read_hyprland(path: &Path, layout: &mut Layout, seen: &mut Vec<PathBuf>) {
    // Guards against include cycles.
    if seen.iter().any(|p| p == path) {
        return;
    }
    seen.push(path.to_path_buf());
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    let mut sources = Vec::new();
    apply_hyprland(&src, layout, &mut sources);
    for source in sources {
        read_hyprland(&source, layout, seen);
    }
}

/// Applies one hyprland.conf's layout keys; collects its `source` includes.
/// Both block (`general { gaps_in = 4 }`) and path (`general:gaps_in = 4`)
/// syntax work. ponytail: globs in `source` aren't expanded.
fn apply_hyprland(src: &str, layout: &mut Layout, sources: &mut Vec<PathBuf>) {
    let mut blocks: Vec<String> = Vec::new();
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(name) = line.strip_suffix('{') {
            blocks.push(name.trim().to_string());
            continue;
        }
        if line == "}" {
            blocks.pop();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if blocks.is_empty() && key == "source" {
            sources.push(expand_home(value));
            continue;
        }
        let path = blocks.iter().map(String::as_str).chain([key]).collect::<Vec<_>>().join(":");
        // Gaps may be CSS-style ("5 10 5 10"); the first number is enough here.
        let Some(number) = value
            .split([' ', ','])
            .next()
            .and_then(|v| v.parse::<f32>().ok())
        else {
            continue;
        };
        match path.as_str() {
            "general:gaps_in" => layout.gaps_in = number,
            "general:gaps_out" => layout.gaps_out = number,
            "general:border_size" => layout.border_size = number,
            "decoration:rounding" => layout.rounding = number,
            _ => {}
        }
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// `(key, "#rrggbb")` pairs from `$key = rgba(rrggbbaa)` / `rgb(rrggbb)` /
/// `#rrggbb` lines. Alpha is dropped.
fn parse_colors_conf(src: &str) -> Vec<(String, String)> {
    src.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim().trim_start_matches('$').to_string();
            let value = value.trim();
            let hex = value
                .strip_prefix("rgba(")
                .or_else(|| value.strip_prefix("rgb("))
                .and_then(|v| v.strip_suffix(')'))
                .or_else(|| value.strip_prefix('#'))?;
            let hex = hex.get(..6)?;
            hex.chars()
                .all(|c| c.is_ascii_hexdigit())
                .then(|| (key, format!("#{}", hex.to_ascii_lowercase())))
        })
        .collect()
}

pub fn colors_conf_path() -> PathBuf {
    std::env::var_os("MESH_THEME_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/themes/active/colors.conf"))
}

pub fn hyprland_conf_path() -> PathBuf {
    home().join(".config/hypr/hyprland.conf")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Theme choice meaning "follow the rice's colors.conf" (the default).
pub const RICE: &str = "rice";
/// Theme choice meaning "load colors from theme.toml".
pub const CUSTOM: &str = "custom";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub theme: Option<String>,
    /// Page to open for Discord voice (e.g. a self-hosted browser running
    /// Discord web). Hand-edited; when set, the sidebar shows a 🎧 button.
    pub discord_voice_url: Option<String>,
    /// Command the 🎧 button runs instead (e.g. a script that shows a voice
    /// window); wins over `discord_voice_url`.
    pub discord_voice_command: Option<String>,
    /// DevTools socket of the hidden Discord client that Mesh drives for
    /// Discord voice (default: `$XDG_RUNTIME_DIR/discord-engine.sock`).
    pub discord_engine_socket: Option<String>,
    /// Command that starts that client when its socket isn't there yet.
    pub discord_engine_command: Option<String>,
    /// Read gaps/border/rounding from hyprland.conf (default on).
    pub layout_from_hyprland: Option<bool>,
    pub density: Option<Density>,
    #[serde(default)]
    pub extras: Extras,
    /// Networks (registry ids) that have been linked, so one signed out
    /// elsewhere shows as needing a relink.
    #[serde(default)]
    pub linked: Vec<String>,
    /// Accounts sync switches that are off, as `<network id>:<key>`.
    #[serde(default)]
    pub sync_off: Vec<String>,
}

/// The Rice page's switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Extras {
    pub digital_rain: bool,
    /// Panes at 92% opacity so Hyprland's blur shows through.
    pub translucent_panes: bool,
    pub show_network_tags: bool,
    /// No image downloads or previews: just text, links and code.
    pub plain_text_mode: bool,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            digital_rain: true,
            translucent_panes: true,
            show_network_tags: true,
            plain_text_mode: false,
        }
    }
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Cozy, Density::Irc];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Cozy => "cozy",
            Density::Irc => "irc",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Density::Compact => "no avatars, one line per message",
            Density::Cozy => "avatars, grouped by author",
            Density::Irc => "[14:02] <nick> like it's 1999",
        }
    }
}

/// The current look as a theme.toml (the `[colors]` format this module
/// loads), for the Rice page's export.
pub fn export_toml(choice: &str, palette: &Palette, layout: &Layout, from_hyprland: bool, density: Density, extras: &Extras) -> String {
    let source = match choice {
        RICE => "rice",
        CUSTOM => "toml",
        _ => "preset",
    };
    format!(
        "# generated by mesh from {source}\n\
         source = \"{source}\"\n\
         watch  = true\n\n\
         [colors]\n\
         background  = \"{}\"\n\
         surface     = \"{}\"\n\
         surface_alt = \"{}\"\n\
         accent      = \"{}\"\n\
         text        = \"{}\"\n\n\
         [layout]\n\
         from_hyprland = {from_hyprland}\n\
         gaps_in     = {}\n\
         gaps_out    = {}\n\
         border_size = {}\n\
         rounding    = {}\n\
         density     = \"{}\"\n\n\
         [extras]\n\
         digital_rain      = {}\n\
         translucent_panes = {}\n\
         show_network_tags = {}\n\
         plain_text_mode   = {}\n",
        palette.background,
        palette.surface,
        palette.surface_alt,
        palette.accent,
        palette.text,
        layout.gaps_in,
        layout.gaps_out,
        layout.border_size,
        layout.rounding,
        density.label(),
        extras.digital_rain,
        extras.translucent_panes,
        extras.show_network_tags,
        extras.plain_text_mode,
    )
}

/// How tightly the timeline is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// No avatars, one line per message.
    Compact,
    /// Avatars and author headers, grouped by author.
    #[default]
    Cozy,
    /// `[14:02] <nick> text`, all mono.
    Irc,
}

pub fn load_config(config_path: &Path) -> Config {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<Config>(&s).ok())
        .unwrap_or_default()
}

/// Changes some keys and writes the file back, keeping the (hand-edited) rest.
pub fn update_config(config_path: &Path, change: impl FnOnce(&mut Config)) -> anyhow::Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut config = load_config(config_path);
    change(&mut config);
    std::fs::write(config_path, toml::to_string_pretty(&config)?)?;
    Ok(())
}

/// What the 🎧 button does.
#[derive(Debug, Clone)]
pub enum DiscordVoice {
    Command(String),
    Url(String),
}

pub fn load_discord_voice(config_path: &Path) -> Option<DiscordVoice> {
    let config = load_config(config_path);
    config
        .discord_voice_command
        .map(DiscordVoice::Command)
        .or(config.discord_voice_url.map(DiscordVoice::Url))
}

/// The persisted theme choice: a preset name, [`RICE`] (the default) or [`CUSTOM`].
pub fn load_choice(config_path: &Path) -> String {
    load_config(config_path)
        .theme
        .unwrap_or_else(|| RICE.to_string())
}

pub fn save_choice(config_path: &Path, choice: &str) -> anyhow::Result<()> {
    update_config(config_path, |c| c.theme = Some(choice.to_string()))
}

/// Resolves a theme choice to a palette: [`RICE`] reads colors.conf,
/// [`CUSTOM`] reads theme.toml, anything else looks up the presets.
pub fn palette_for_choice(choice: &str, theme_toml: &Path) -> Palette {
    match choice {
        RICE => std::fs::read_to_string(colors_conf_path())
            .map(|s| Palette::from_colors_conf(&s))
            .unwrap_or_default(),
        CUSTOM => Palette::load_or_default(theme_toml),
        _ => presets()
            .into_iter()
            .find(|(name, _)| *name == choice)
            .map(|(_, palette)| palette)
            .unwrap_or_default(),
    }
}

pub fn layout_for_config(config_path: &Path) -> Layout {
    if load_config(config_path).layout_from_hyprland.unwrap_or(true) {
        Layout::from_hyprland(&hyprland_conf_path())
    } else {
        Layout::default()
    }
}

/// The files a theme choice reads, for hot reload. hyprland.conf's includes
/// aren't listed. ponytail: edits to a `source`d file reload only once
/// hyprland.conf itself is touched; list includes too if that bites.
pub fn watched_files(choice: &str, theme_toml: &Path) -> Vec<PathBuf> {
    let mut files = vec![hyprland_conf_path()];
    match choice {
        RICE => files.push(colors_conf_path()),
        CUSTOM => files.push(theme_toml.to_path_buf()),
        _ => {}
    }
    files
}

/// Modification times of `files` (`None` for missing ones), compared between
/// polls to spot a change.
pub fn stamps(files: &[PathBuf]) -> Vec<Option<SystemTime>> {
    files
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect()
}

/// Built-in presets, mainly meant for Windows where there's no rice to follow.
pub fn presets() -> Vec<(&'static str, Palette)> {
    let preset = |[background, surface, surface_alt, text, text_muted, accent, accent_text, danger, success, border]: [&str; 10]| Palette {
        background: background.into(),
        surface: surface.into(),
        surface_alt: surface_alt.into(),
        text: text.into(),
        accent: accent.into(),
        text_muted: Some(text_muted.into()),
        accent_text: Some(accent_text.into()),
        danger: Some(danger.into()),
        success: Some(success.into()),
        border: Some(border.into()),
        ..Palette::default()
    };
    vec![
        (
            "Catppuccin Mocha",
            preset([
                "#1e1e2e", "#181825", "#313244", "#cdd6f4", "#a6adc8", "#89b4fa", "#11111b",
                "#f38ba8", "#a6e3a1", "#45475a",
            ]),
        ),
        (
            "Nord",
            preset([
                "#2e3440", "#3b4252", "#434c5e", "#eceff4", "#d8dee9", "#88c0d0", "#2e3440",
                "#bf616a", "#a3be8c", "#4c566a",
            ]),
        ),
        (
            "Gruvbox Dark",
            preset([
                "#282828", "#3c3836", "#504945", "#ebdbb2", "#bdae93", "#d79921", "#282828",
                "#cc241d", "#98971a", "#665c54",
            ]),
        ),
    ]
}

fn or_derive(value: &Option<String>, derive: impl FnOnce() -> Color) -> Color {
    value.as_deref().map(parse_hex).unwrap_or_else(derive)
}

fn parse_hex(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let channel = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    Color::from_rgb8(channel(0), channel(2), channel(4))
}

pub fn to_hex(c: Color) -> String {
    let [r, g, b, _] = c.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Linear blend: `t = 0` is `a`, `t = 1` is `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

/// Same color at a given alpha.
pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

fn luminance(c: Color) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio, 1.0 (none) to 21.0 (black on white).
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Moves `fg` away from `bg` (lighter on dark backgrounds, darker on light
/// ones) until it reaches [`MIN_CONTRAST`].
fn ensure_contrast(mut fg: Color, bg: Color) -> Color {
    let target = if luminance(bg) > 0.18 {
        Color::BLACK
    } else {
        Color::WHITE
    };
    for _ in 0..40 {
        if contrast(fg, bg) >= MIN_CONTRAST {
            break;
        }
        fg = mix(fg, target, 0.05);
    }
    fg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_choice_keeps_other_keys() {
        let path = std::env::temp_dir().join(format!("mesh-config-test-{}.toml", std::process::id()));
        std::fs::write(&path, "theme = \"Nord\"\ndiscord_voice_url = \"https://x\"\n").unwrap();
        save_choice(&path, "Gruvbox Dark").unwrap();
        assert_eq!(load_choice(&path), "Gruvbox Dark");
        assert!(matches!(load_discord_voice(&path), Some(DiscordVoice::Url(u)) if u == "https://x"));
        update_config(&path, |c| c.discord_voice_command = Some("toggle".into())).unwrap();
        assert!(matches!(load_discord_voice(&path), Some(DiscordVoice::Command(c)) if c == "toggle"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn colors_conf_maps_rice_keys() {
        let src = "# generated\n$bg1 = rgba(2e3440ff)\nbg2 = rgba(242933ff)\n$surface=rgb(3B4252)\n$accent = #88c0d0\n$text = rgba(zzzzzzff)\n$other = rgba(ffffffff)\n";
        let p = Palette::from_colors_conf(src);
        assert_eq!(p.surface, "#2e3440");
        assert_eq!(p.background, "#242933");
        assert_eq!(p.surface_alt, "#3b4252");
        assert_eq!(p.accent, "#88c0d0");
        // Unparseable value keeps the default.
        assert_eq!(p.text, Palette::default().text);
    }

    #[test]
    fn hyprland_reads_blocks_paths_and_sources() {
        let src = "source = ~/inc.conf\n# general { gaps_in = 99 }\ngeneral {\n  gaps_in = 5\n  gaps_out = 20 10 20 10\n  border_size = 3 # thick\n  snap {\n    gaps_in = 77\n  }\n}\ndecoration {\n  rounding = 12\n  blur {\n    size = 4\n  }\n}\ngeneral:border_size = 1\n";
        let mut layout = Layout::default();
        let mut sources = Vec::new();
        apply_hyprland(src, &mut layout, &mut sources);
        assert_eq!(
            layout,
            Layout { gaps_in: 5.0, gaps_out: 20.0, border_size: 1.0, rounding: 12.0 }
        );
        assert_eq!(sources, vec![dirs::home_dir().unwrap_or_default().join("inc.conf")]);
    }

    #[test]
    fn missing_hyprland_gives_defaults() {
        assert_eq!(Layout::from_hyprland(Path::new("/nonexistent/hypr.conf")), Layout::default());
    }

    #[test]
    fn contrast_ratio_bounds() {
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color::WHITE, Color::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn derived_text_passes_contrast() {
        let light = Palette {
            background: "#eeeeee".into(),
            surface: "#f8f8f8".into(),
            surface_alt: "#e0e0e0".into(),
            text: "#bbbbbb".into(),
            ..Palette::default()
        };
        for p in [Palette::default(), light] {
            assert!(contrast(p.text_muted(), p.surface_alt()) >= MIN_CONTRAST);
            assert!(contrast(p.accent_text(), p.accent()) >= MIN_CONTRAST);
        }
    }

    #[test]
    fn derived_defaults_match_spec() {
        // Close to the hand-picked values in design/DESIGN.md.
        let p = Palette::default();
        let near = |c: Color, hex: &str| {
            let (a, b) = (c.into_rgba8(), parse_hex(hex).into_rgba8());
            (0..3).all(|i| a[i].abs_diff(b[i]) <= 12)
        };
        assert!(near(p.border(), "#1f2a36"));
        assert!(near(p.border_ctrl(), "#253040"));
        assert!(near(p.panel_dim(), "#0b1219"));
        assert!(near(p.text_bright(), "#e3e7ee"));
        assert!(near(p.accent_text(), "#1a0d0e"));
    }

    #[test]
    fn export_loads_back() {
        let path = std::env::temp_dir().join(format!("mesh-export-{}.toml", std::process::id()));
        let palette = presets()[2].1.clone();
        let text = export_toml(RICE, &palette, &Layout::default(), true, Density::Irc, &Extras::default());
        assert!(toml::from_str::<toml::Table>(&text).is_ok());
        std::fs::write(&path, text).unwrap();
        assert_eq!(Palette::load_or_default(&path).accent, palette.accent);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn extras_default_when_missing() {
        let config: Config = toml::from_str("theme = \"rice\"\n[extras]\ndigital_rain = false\n").unwrap();
        assert!(!config.extras.digital_rain);
        assert!(config.extras.show_network_tags);
    }

    #[test]
    fn theme_toml_nested_or_flat() {
        let dir = std::env::temp_dir();
        let nested = dir.join(format!("mesh-theme-nested-{}.toml", std::process::id()));
        std::fs::write(&nested, "source = \"rice\"\n[colors]\nbackground = \"#000000\"\nsurface = \"#111111\"\nsurface_alt = \"#222222\"\naccent = \"#ff0000\"\ntext = \"#eeeeee\"\n[layout]\ngaps_in = 4\n").unwrap();
        assert_eq!(Palette::load_or_default(&nested).accent, "#ff0000");
        let flat = dir.join(format!("mesh-theme-flat-{}.toml", std::process::id()));
        std::fs::write(&flat, toml::to_string(&presets()[1].1).unwrap()).unwrap();
        assert_eq!(Palette::load_or_default(&flat).accent, "#88c0d0");
        let _ = std::fs::remove_file(nested);
        let _ = std::fs::remove_file(flat);
    }
}
