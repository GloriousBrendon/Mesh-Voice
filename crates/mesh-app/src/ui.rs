//! Styled building blocks for the tiled shell. Colors come from the
//! [`Palette`], sizes from the [`Layout`]; nothing here hardcodes either.

use iced::font::Weight;
use iced::widget::{Space, Text, button, container, stack, svg, text, tooltip};
use iced::{Alignment, Color, Element, Font, Length, border};

use crate::Message;
use crate::theme::{Layout, Palette, alpha, mix};

pub const SANS: Font = Font::with_name("IBM Plex Sans");
pub const MONO: Font = Font::with_name("JetBrains Mono");

/// The bundled fonts, registered at startup so Linux and Windows look the same.
pub const FONTS: [&[u8]; 5] = [
    include_bytes!("../fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
];

/// The bundled icons: Lucide (ISC) for UI, Simple Icons (CC0) for networks.
#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Mic,
    MicOff,
    Headphones,
    HeadphoneOff,
    Phone,
    PhoneOff,
    Settings,
    Users,
    Paperclip,
    Send,
    Home,
    Plus,
    Hash,
    At,
    Voice,
    Close,
    Search,
    UserPlus,
    Matrix,
    Discord,
    Video,
    VideoOff,
    ScreenShare,
    ScreenShareOff,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Icon::Mic => include_bytes!("../icons/mic.svg"),
            Icon::MicOff => include_bytes!("../icons/mic-off.svg"),
            Icon::Headphones => include_bytes!("../icons/headphones.svg"),
            Icon::HeadphoneOff => include_bytes!("../icons/headphone-off.svg"),
            Icon::Phone => include_bytes!("../icons/phone.svg"),
            Icon::PhoneOff => include_bytes!("../icons/phone-off.svg"),
            Icon::Settings => include_bytes!("../icons/settings.svg"),
            Icon::Users => include_bytes!("../icons/users.svg"),
            Icon::Paperclip => include_bytes!("../icons/paperclip.svg"),
            Icon::Send => include_bytes!("../icons/send-horizontal.svg"),
            Icon::Home => include_bytes!("../icons/house.svg"),
            Icon::Plus => include_bytes!("../icons/plus.svg"),
            Icon::Hash => include_bytes!("../icons/hash.svg"),
            Icon::At => include_bytes!("../icons/at-sign.svg"),
            Icon::Voice => include_bytes!("../icons/volume-2.svg"),
            Icon::Video => include_bytes!("../icons/video.svg"),
            Icon::VideoOff => include_bytes!("../icons/video-off.svg"),
            Icon::ScreenShare => include_bytes!("../icons/screen-share.svg"),
            Icon::ScreenShareOff => include_bytes!("../icons/screen-share-off.svg"),
            Icon::Close => include_bytes!("../icons/x.svg"),
            Icon::Search => include_bytes!("../icons/search.svg"),
            Icon::UserPlus => include_bytes!("../icons/user-plus.svg"),
            Icon::Matrix => include_bytes!("../icons/brand-matrix.svg"),
            Icon::Discord => include_bytes!("../icons/brand-discord.svg"),
        }
    }
}

/// An icon at `size` px, tinted `color`.
pub fn icon<'a>(which: Icon, size: f32, color: Color) -> Element<'a, Message> {
    svg(svg::Handle::from_memory(which.bytes()))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

pub fn weight(font: Font, weight: Weight) -> Font {
    Font { weight, ..font }
}

/// Sans text in a given weight.
pub fn sans<'a>(content: impl text::IntoFragment<'a>, size: f32, w: Weight, color: Color) -> Text<'a> {
    text(content).font(weight(SANS, w)).size(size).color(color)
}

pub fn mono<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> Text<'a> {
    text(content).font(MONO).size(size).color(color)
}

pub fn mono_bold<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> Text<'a> {
    text(content).font(weight(MONO, Weight::Bold)).size(size).color(color)
}

/// A section label: mono 11, uppercase, muted.
pub fn section<'a>(label: &str, p: &Palette) -> Element<'a, Message> {
    container(mono(label.to_uppercase(), 11.0, p.text_muted()))
        .padding([6, 8])
        .into()
}

/// One tiled pane: `surface` fill, `border_size` border (accent when
/// focused, like Hyprland's active border), `rounding` corners.
pub fn pane<'a>(
    content: impl Into<Element<'a, Message>>,
    focused: bool,
    p: &Palette,
    l: &Layout,
    translucent: bool,
) -> container::Container<'a, Message> {
    // 92% lets Hyprland's blur (and the rain) show through.
    let fill = if translucent { alpha(p.surface(), 0.92) } else { p.surface() };
    let edge = if focused { p.accent() } else { p.border() };
    let (width, radius) = (l.border_size, l.rounding);
    container(content)
        .padding(l.border_size)
        .clip(true)
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: border::rounded(radius).color(edge).width(width),
            ..container::Style::default()
        })
}

/// A 1 px separator.
pub fn hline<'a>(p: &Palette) -> Element<'a, Message> {
    let color = p.border();
    container(Space::new())
        .width(Length::Fill)
        .height(1)
        .style(move |_| container::Style {
            background: Some(color.into()),
            ..container::Style::default()
        })
        .into()
}

/// A filled block (a status dot, an accent bar).
pub fn swatch<'a>(color: Color, width: f32, height: f32, radius: f32) -> Element<'a, Message> {
    container(Space::new())
        .width(width)
        .height(height)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: border::rounded(radius),
            ..container::Style::default()
        })
        .into()
}

/// A small mono tag with a tinted outline, e.g. a network tag (`mx`, `dc`).
pub fn tag<'a>(label: impl text::IntoFragment<'a>, color: Color, p: &Palette) -> Element<'a, Message> {
    let outline = mix(color, p.surface(), 0.6);
    container(mono(label, 10.0, color))
        .padding([1, 4])
        .style(move |_| container::Style {
            border: border::rounded(4).color(outline).width(1),
            ..container::Style::default()
        })
        .into()
}

/// A filled chip: `surface_alt` with bright mono text (counts, key numbers).
pub fn chip<'a>(label: impl text::IntoFragment<'a>, p: &Palette) -> Element<'a, Message> {
    let fill = p.surface_alt();
    container(mono(label, 11.0, p.text_bright()))
        .padding([1, 6])
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: border::rounded(8),
            ..container::Style::default()
        })
        .into()
}

/// An outlined chip (connection status, keybind hints).
pub fn outline_chip<'a>(content: impl Into<Element<'a, Message>>, p: &Palette) -> Element<'a, Message> {
    let edge = p.border_ctrl();
    container(content)
        .padding([4, 8])
        .style(move |_| container::Style {
            border: border::rounded(4).color(edge).width(1),
            ..container::Style::default()
        })
        .into()
}

/// A borderless icon button. `active` = toggled on (muted, deafened...),
/// drawn in `danger` on a dark red fill.
pub fn icon_button<'a>(
    which: Icon,
    msg: Option<Message>,
    active: bool,
    p: &Palette,
    l: &Layout,
) -> button::Button<'a, Message> {
    let color = if active { p.danger() } else { p.text() };
    let active_fill = mix(p.danger(), p.panel_dim(), 0.82);
    let hover = p.surface_alt();
    let radius = l.control_radius();
    button(
        container(icon(which, 17.0, color))
            .center_x(Length::Fixed(20.0))
            .center_y(Length::Fixed(20.0)),
    )
    .on_press_maybe(msg)
    .padding(6)
    .style(move |_, status| button::Style {
        background: if active {
            Some(active_fill.into())
        } else if matches!(status, button::Status::Hovered) {
            Some(hover.into())
        } else {
            None
        },
        text_color: color,
        border: border::rounded(radius),
        ..button::Style::default()
    })
}

/// Accent-filled primary button.
pub fn primary_button<'a>(
    content: impl Into<Element<'a, Message>>,
    msg: Option<Message>,
    p: &Palette,
    l: &Layout,
) -> button::Button<'a, Message> {
    let (fill, fg) = (p.accent(), p.accent_text());
    let hover = mix(fill, Color::WHITE, 0.12);
    let radius = l.control_radius();
    button(content)
        .on_press_maybe(msg)
        .padding([6, 10])
        .style(move |_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered => hover,
                    button::Status::Disabled => alpha(fill, 0.5),
                    _ => fill,
                }
                .into(),
            ),
            text_color: fg,
            border: border::rounded(radius),
            ..button::Style::default()
        })
}

/// A row button: transparent, `surface_alt` when selected or hovered.
pub fn row_button<'a>(
    content: impl Into<Element<'a, Message>>,
    msg: Message,
    selected: bool,
    p: &Palette,
    l: &Layout,
) -> button::Button<'a, Message> {
    let (fill, fg) = (p.surface_alt(), p.text());
    let radius = l.control_radius();
    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .padding([6, 8])
        .style(move |_, status| button::Style {
            background: (selected || matches!(status, button::Status::Hovered)).then(|| fill.into()),
            text_color: fg,
            border: border::rounded(radius),
            ..button::Style::default()
        })
}

/// Wraps a control that can't work yet with an explanatory tooltip.
pub fn not_yet<'a>(
    content: impl Into<Element<'a, Message>>,
    why: &'a str,
    p: &Palette,
) -> Element<'a, Message> {
    let (fill, edge, fg) = (p.panel_dim(), p.border_ctrl(), p.text());
    tooltip(
        content,
        container(text(why).size(12).color(fg))
            .padding([4, 8])
            .max_width(280)
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: border::rounded(4).color(edge).width(1),
                ..container::Style::default()
            }),
        tooltip::Position::Bottom,
    )
    .into()
}

/// A round initials avatar; `fill` is the network color (or accent for you).
/// `status` adds a presence dot at the bottom-right, ringed in `ring`.
pub fn avatar<'a>(
    name: &str,
    picture: Option<&iced::widget::image::Handle>,
    size: f32,
    fill: Color,
    p: &Palette,
    status: Option<(Color, Color)>,
) -> Element<'a, Message> {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let fg = p.background();
    let disc: Element<'a, Message> = match picture {
        Some(handle) => iced::widget::image(handle.clone())
            .width(size)
            .height(size)
            .border_radius(size / 2.0)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(mono_bold(initial, size * 0.4, fg))
            .center_x(Length::Fixed(size))
            .center_y(Length::Fixed(size))
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: border::rounded(size / 2.0),
                ..container::Style::default()
            })
            .into(),
    };
    let Some((dot, ring)) = status else {
        return disc;
    };
    let dot = container(Space::new()).width(10).height(10).style(move |_| container::Style {
        background: Some(dot.into()),
        border: border::rounded(5).color(ring).width(2),
        ..container::Style::default()
    });
    stack![
        disc,
        container(dot)
            .width(size)
            .height(size)
            .align_x(Alignment::End)
            .align_y(Alignment::End),
    ]
    .into()
}
