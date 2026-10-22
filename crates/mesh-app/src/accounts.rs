//! The Accounts page (design/DESIGN.md §3.4): every connected account, a
//! grid of every network in the registry, and a setup panel that links or
//! unlinks one. Everything per network comes from `mesh_core::networks`;
//! the only per-network code is `link()`, which reads each one's state.

use super::*;
// Named, so the glob above doesn't clash with std's `column!`.
use iced::widget::{column, image, row, svg};
use mesh_core::networks::{Auth, NETWORKS, NetworkSpec, Step};

/// A sign-in QR code, ready to draw.
pub(crate) enum LoginQr {
    Png(image::Handle),
    Svg(svg::Handle),
}

impl From<mesh_discord::Qr> for LoginQr {
    fn from(qr: mesh_discord::Qr) -> Self {
        match qr {
            mesh_discord::Qr::Png(bytes) => LoginQr::Png(image::Handle::from_bytes(bytes)),
            mesh_discord::Qr::Svg(bytes) => LoginQr::Svg(svg::Handle::from_memory(bytes)),
        }
    }
}

/// Where a network's account stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Link {
    Linked { name: String, meta: String },
    /// Its tab is up and signed out (showing a QR code).
    SignedOut,
    /// Its tab is up but hasn't said which yet.
    Connecting,
    /// No tab: the engine isn't running.
    Offline,
}

/// The setup panel: which network it shows and how far through it is.
#[derive(Debug, Default)]
pub(crate) struct Panel {
    pub selected: Option<&'static str>,
    pub step: usize,
    /// Linking from this panel: once linked, carry on to the sync step
    /// rather than jumping to the account's settings.
    pub linking: bool,
    pub confirm_sign_out: bool,
}

pub(crate) fn link(state: &App, network: Network) -> Link {
    match network {
        Network::Matrix => match &state.screen {
            Screen::LoggedIn(session) => match &session.my_profile {
                Some(me) => Link::Linked { name: me.user_id.to_string(), meta: my_server(session) },
                None => Link::Connecting,
            },
            _ => Link::SignedOut,
        },
        Network::Discord => match (&state.discord_state, &state.discord) {
            (Some(snapshot), _) => match &snapshot.me {
                Some(me) => Link::Linked {
                    name: me.name.clone(),
                    meta: format!("{} · {}", count(snapshot.guilds.len(), "server"), count(snapshot.dms.len(), "DM")),
                },
                None => Link::SignedOut,
            },
            _ if state.login_qr.contains_key("dc") => Link::SignedOut,
            (None, Some(_)) => Link::Connecting,
            // The native helper is always running; it just hasn't reported yet.
            (None, None) if state.discord_native.is_some() => Link::Connecting,
            (None, None) => Link::Offline,
        },
        Network::WhatsApp => match (&state.whatsapp_state, &state.whatsapp) {
            (Some(snapshot), _) => Link::Linked {
                name: phone(&snapshot.me),
                meta: count(snapshot.chats.len() + snapshot.archived.len(), "chat"),
            },
            (None, Some(_)) if state.login_qr.contains_key("wa") => Link::SignedOut,
            (None, Some(_)) => Link::Connecting,
            (None, None) => Link::Offline,
        },
    }
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// `27821234567@c.us` → `+27821234567`.
fn phone(id: &str) -> String {
    let digits: String = id.split(['@', ':']).next().unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() { id.to_string() } else { format!("+{digits}") }
}

/// Whether a step-2 switch is on: `sync_off` holds `<network id>:<key>`.
pub(crate) fn synced(state: &App, network: Network, key: &str) -> bool {
    !state.sync_off.iter().any(|off| off.split_once(':') == Some((network.spec().id, key)))
}

fn all_synced(state: &App, network: Network) -> bool {
    network.spec().sync.iter().all(|item| synced(state, network, item.key))
}

/// The engine snapshots for the rail and the channel list: as they came, or
/// less what the sync switches leave out.
pub(crate) fn shown_discord(state: &App) -> Option<&mesh_discord::Snapshot> {
    if all_synced(state, Network::Discord) { state.discord_state.as_ref() } else { state.discord_shown.as_ref() }
}

pub(crate) fn shown_whatsapp(state: &App) -> Option<&whatsapp::Snapshot> {
    if all_synced(state, Network::WhatsApp) { state.whatsapp_state.as_ref() } else { state.whatsapp_shown.as_ref() }
}

/// Rebuilds the filtered snapshots, after a new snapshot or a switch flips.
pub(crate) fn refresh_shown(state: &mut App) {
    let (dms, servers) = (synced(state, Network::Discord, "dms"), synced(state, Network::Discord, "servers"));
    state.discord_shown = state.discord_state.clone().filter(|_| !(dms && servers)).map(|mut shown| {
        if !dms {
            shown.dms.clear();
        }
        if !servers {
            shown.guilds.clear();
        }
        shown
    });
    let (chats, groups) = (synced(state, Network::WhatsApp, "chats"), synced(state, Network::WhatsApp, "groups"));
    state.whatsapp_shown = state.whatsapp_state.clone().filter(|_| !(chats && groups)).map(|mut shown| {
        let keep = |c: &whatsapp::Chat| if c.group { groups } else { chats };
        shown.chats.retain(keep);
        shown.archived.retain(keep);
        shown
    });
}

/// Notes that a network has been linked, so a later sign-out from elsewhere
/// shows it as needing a relink rather than vanishing.
pub(crate) fn remember_linked(state: &mut App, network: Network) {
    let id = network.spec().id;
    if !state.linked.iter().any(|l| l == id) {
        state.linked.push(id.to_string());
        let linked = state.linked.clone();
        persist(|c| c.linked = linked);
    }
}

/// Opens a network's sign-in tab in the engine (starting the engine if it's
/// down), without bringing it to the front: Mesh shows its QR code.
pub(crate) fn start_link(state: &App, network: Network) -> Result<(), String> {
    let spec = network.spec();
    let (Some(host), Some(url)) = (spec.engine, spec.login_url) else {
        return Ok(());
    };
    // The native helpers are always up and show their QR code when unlinked.
    if network == Network::WhatsApp && state.whatsapp_native.is_some() {
        return Ok(());
    }
    if network == Network::Discord && state.discord_native.is_some() {
        return Ok(());
    }
    match &state.discord_socket {
        Some(socket) if mesh_discord::is_running(socket) => mesh_discord::open(socket, host, url, false),
        _ => match &state.discord_command {
            Some(cmd) => run_command(cmd).map_err(|e| e.to_string()),
            None => Err("no engine configured (discord_engine_command)".into()),
        },
    }
}

/// Picks the network the panel opens on: the first ready one not linked yet.
pub(crate) fn default_pick(state: &App) -> &'static str {
    NETWORKS
        .iter()
        .filter(|n| !n.soon())
        .find(|n| Network::from_id(n.id).is_some_and(|net| !matches!(link(state, net), Link::Linked { .. })))
        .unwrap_or(&NETWORKS[0])
        .id
}

fn code_tile<'a>(spec: &NetworkSpec, size: f32, p: &theme::Palette) -> Element<'a, Message> {
    let fill = p.surface_alt();
    container(ui::mono_bold(spec.code, size / 3.0, p.network(spec)))
        .center_x(size)
        .center_y(size)
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(size / 4.0),
            ..container::Style::default()
        })
        .into()
}

fn badge<'a>(label: impl Into<String>, fg: iced::Color, fill: Option<iced::Color>, edge: iced::Color) -> Element<'a, Message> {
    container(ui::mono(label.into(), 10.0, fg).wrapping(text::Wrapping::None))
        .padding([1, 5])
        .style(move |_| container::Style {
            background: fill.map(Into::into),
            border: iced::border::rounded(4).color(edge).width(if fill.is_some() { 0 } else { 1 }),
            ..container::Style::default()
        })
        .into()
}

/// The page: connected accounts and the network grid. The setup panel is
/// its own pane, beside it (`panel`).
pub(crate) fn page<'a>(state: &'a App, columns: usize) -> Element<'a, Message> {
    let p = &state.palette;
    let selected = state.accounts.selected.unwrap_or_else(|| default_pick(state));

    // Connected: every account that's linked, or was and needs relinking.
    let mut rows = column![];
    let mut count = 0;
    for spec in NETWORKS {
        let Some(network) = Network::from_id(spec.id) else { continue };
        let link = link(state, network);
        let (name, meta, label, color, warn) = match &link {
            Link::Linked { name, meta } => (name.clone(), meta.clone(), "● synced", p.success(), false),
            _ if network != Network::Matrix && !state.linked.iter().any(|id| id == spec.id) => continue,
            Link::Offline => (spec.name.to_string(), "the engine isn't running".to_string(), "▲ offline", p.warning(), true),
            Link::Connecting => (spec.name.to_string(), "not answering yet".to_string(), "… connecting", p.text_muted(), false),
            Link::SignedOut => (spec.name.to_string(), "signed out".to_string(), "▲ needs relink", p.warning(), true),
        };
        let mut title = row![ui::sans(name, 14.0, Weight::Semibold, p.text_bright())].spacing(8).align_y(iced::Alignment::Center);
        if network == Network::Matrix {
            title = title.push(badge("PRIMARY", p.accent_text(), Some(p.accent()), p.accent()));
        }
        let sub: Element<'a, Message> = if warn {
            ui::mono(format!("{label} · {meta}"), 11.0, color).into()
        } else {
            text(format!("{} · {meta}", spec.name)).size(12).color(p.text_muted()).into()
        };
        let mut line = row![code_tile(spec, 38.0, p), column![title, sub].spacing(2).width(Length::Fill)]
            .spacing(14)
            .align_y(iced::Alignment::Center);
        if !warn {
            line = line.push(ui::mono(label, 11.0, color));
        }
        if warn {
            line = line.push(
                ui::primary_button(ui::sans("Reconnect", 13.0, Weight::Semibold, p.accent_text()), Some(Message::AccountsReconnect(spec.id)), p, &state.layout)
                    .padding([6, 12]),
            );
        }
        line = line.push(
            button(ui::sans("⋯", 16.0, Weight::Semibold, p.text()))
                .on_press(Message::AccountsPick(spec.id))
                .padding([4, 10])
                .style(flat_button_style(p)),
        );
        if count > 0 {
            rows = rows.push(ui::hline(p));
        }
        rows = rows.push(container(line).padding([12, 14]));
        count += 1;
    }
    let edge = p.border();
    let connected = container(rows).style(move |_| container::Style {
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    });

    // Add a network: four to a row, or two when the window's narrow.
    let mut grid = column![].spacing(8);
    for chunk in NETWORKS.chunks(columns) {
        let mut line = row![].spacing(8);
        for spec in chunk {
            line = line.push(network_card(state, spec, selected == spec.id));
        }
        for _ in chunk.len()..columns {
            line = line.push(Space::new().width(Length::FillPortion(1)));
        }
        grid = grid.push(line);
    }

    column![
        page_header("Accounts", "Every network you use, in one list. Pick one to connect it.", p),
        column![heading(format!("connected — {count}"), p), connected].spacing(8),
        column![
            heading("add a network", p),
            grid,
            text("New networks show up here as they ship.").size(12).color(p.text_muted()),
        ]
        .spacing(8),
    ]
    .spacing(22)
    .into()
}

fn network_card<'a>(state: &'a App, spec: &'static NetworkSpec, on: bool) -> Element<'a, Message> {
    let p = &state.palette;
    let linked = Network::from_id(spec.id).is_some_and(|n| matches!(link(state, n), Link::Linked { .. } | Link::Connecting));
    let tag = if spec.soon() {
        badge("soon", p.text_muted(), None, p.border())
    } else if linked {
        badge("linked", p.success(), None, theme::mix(p.success(), p.surface(), 0.6))
    } else {
        badge("add", p.text_bright(), None, p.border_ctrl())
    };
    // "Soon" cards are dimmed but still open, to preview their sign-in.
    let name = if spec.soon() && !on { p.text() } else { p.text_bright() };
    let (edge, fill, hover) = (
        if on { p.accent() } else { p.border_ctrl() },
        on.then(|| theme::mix(p.accent(), p.surface(), 0.92)),
        p.surface_alt(),
    );
    button(
        column![
            row![code_tile(spec, 32.0, p), Space::new().width(Length::Fill), tag].align_y(iced::Alignment::Center),
            column![
                ui::sans(spec.name, 14.0, Weight::Semibold, name),
                text(spec.auth.label()).size(12).color(p.text_muted()),
            ]
            .spacing(1),
        ]
        .spacing(10),
    )
    .on_press(Message::AccountsPick(spec.id))
    .width(Length::FillPortion(1))
    .padding(12)
    .style(move |_, status| button::Style {
        background: match (fill, status) {
            (Some(fill), _) => Some(fill.into()),
            (None, button::Status::Hovered) => Some(hover.into()),
            _ => None,
        },
        border: iced::border::rounded(8).color(edge).width(1),
        ..button::Style::default()
    })
    .into()
}

/// The setup panel, for the selected network: sign in → sync → done, or the
/// account's settings and sign-out once it's linked.
pub(crate) fn panel<'a>(state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let id = state.accounts.selected.unwrap_or_else(|| default_pick(state));
    let Some(spec) = mesh_core::networks::spec(id) else {
        return Space::new().into();
    };
    let network = Network::from_id(spec.id);
    let link = network.map(|n| link(state, n));
    let panel = &state.accounts;
    let ever_linked = state.linked.iter().any(|l| l == spec.id);
    let linked = matches!(link, Some(Link::Linked { .. }));
    let managing = matches!(link, Some(Link::Linked { .. } | Link::Connecting)) && !panel.linking;

    let title = if managing {
        spec.name.to_string()
    } else if ever_linked {
        format!("Reconnect {}", spec.name)
    } else {
        format!("Connect {}", spec.name)
    };
    let header = row![
        code_tile(spec, 40.0, p),
        column![
            ui::sans(title, 17.0, Weight::Semibold, p.text_bright()),
            text(match (&link, managing) {
                (Some(Link::Connecting), true) => "connecting…".to_string(),
                (_, true) => "connected".to_string(),
                _ => format!("{} · about a minute", spec.auth.label()),
            })
                .size(12)
                .color(p.text_muted()),
        ],
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center);

    let mut body = column![header].spacing(18).height(Length::Fill);
    if managing {
        body = body.push(manage(state, spec, network, link.as_ref(), panel.confirm_sign_out));
    } else {
        let steps = spec.auth.steps();
        // Linking skips ahead by itself: no button to say "I've scanned it".
        let at = panel.step.max(if linked { 1 } else { 0 }).min(steps.len() - 1);
        body = body.push(progress(steps, at, p));
        if spec.soon() {
            let edge = p.border_ctrl();
            body = body.push(
                container(text("In development. This is the sign-in it will use; nothing to set up yet.").size(12).color(p.text()))
                    .padding([10, 12])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        border: iced::border::rounded(8).color(edge).width(1),
                        ..container::Style::default()
                    }),
            );
        }
        let content: Element<'a, Message> = match steps[at] {
            Step::SignIn | Step::Code => sign_in(state, spec, link.as_ref()),
            Step::Sync => sync_switches(state, spec, network),
            Step::Done => done(spec, p),
        };
        body = body.push(container(content).height(Length::Fill));
        if spec.unofficial {
            body = body.push(unofficial(spec, p));
        }
        let (label, next) = match steps[at] {
            _ if spec.soon() => ("Coming soon", None),
            Step::SignIn | Step::Code => (
                match (spec.auth, &link) {
                    (_, Some(Link::Offline)) => "Starting…",
                    (Auth::Qr, _) => "Waiting for your phone…",
                    _ => "Continue",
                },
                None,
            ),
            Step::Sync => ("Connect", Some(Message::AccountsStep(at + 1))),
            Step::Done => ("Done", Some(Message::AccountsDone)),
        };
        let mut buttons = row![].spacing(8);
        if steps[at] == Step::Sync {
            buttons = buttons.push(
                button(ui::sans("Back", 14.0, Weight::Normal, p.text_bright()))
                    .on_press(Message::AccountsStep(at - 1))
                    .padding([10, 16])
                    .style(flat_button_style(p)),
            );
        }
        buttons = buttons.push(
            ui::primary_button(container(ui::sans(label, 14.0, Weight::Semibold, p.accent_text())).center_x(Length::Fill), next, p, l)
                .width(Length::Fill)
                .padding([10, 16]),
        );
        body = body.push(buttons);
    }
    body = body.push(ui::mono(format!("via {}", spec.via()), 11.0, p.text_muted()));
    container(body).padding(22).width(380).height(Length::Fill).into()
}

fn progress<'a>(steps: &[Step], at: usize, p: &theme::Palette) -> Element<'a, Message> {
    let mut bar = row![].spacing(6);
    // A phone code is part of signing in, so it shares that segment.
    let shown: Vec<(usize, Step)> = steps.iter().copied().enumerate().filter(|(_, s)| *s != Step::Code).collect();
    for (n, (i, step)) in shown.iter().enumerate() {
        let current = *i == at || (*step == Step::SignIn && steps.get(at) == Some(&Step::Code));
        let label = match step {
            Step::SignIn | Step::Code => "sign in",
            Step::Sync => "sync",
            Step::Done => "done",
        };
        let fill = if *i <= at { p.accent() } else { p.border_ctrl() };
        bar = bar.push(
            column![
                container(Space::new()).width(Length::Fill).height(3).style(move |_| container::Style {
                    background: Some(fill.into()),
                    border: iced::border::rounded(2),
                    ..container::Style::default()
                }),
                ui::mono(format!("{} {label}", n + 1), 11.0, if current { p.text_bright() } else { p.text_muted() }),
            ]
            .spacing(6)
            .width(Length::FillPortion(1)),
        );
    }
    bar.into()
}

/// Step 1, by how the network signs in.
fn sign_in<'a>(state: &'a App, spec: &'static NetworkSpec, link: Option<&Link>) -> Element<'a, Message> {
    let p = &state.palette;
    let field = |label: &'static str, placeholder: &'static str| {
        column![
            ui::mono(label, 11.0, p.text_muted()),
            text_input(placeholder, "").padding([10, 12]).size(13),
        ]
        .spacing(6)
    };
    match spec.auth {
        Auth::Qr => {
            let qr = state.login_qr.get(spec.id).filter(|_| matches!(link, Some(Link::SignedOut)));
            let code: Element<'a, Message> = match qr {
                Some(LoginQr::Png(handle)) => image(handle.clone()).width(200).height(200).into(),
                Some(LoginQr::Svg(handle)) => svg(handle.clone()).width(200).height(200).into(),
                None => container(text(match link {
                    None => "The network supplies the code.",
                    Some(Link::Offline) => "Starting the engine…",
                    _ => "Getting a code…",
                })
                .size(12)
                .color(iced::Color::from_rgb8(0x50, 0x58, 0x66)))
                .center_x(200)
                .center_y(200)
                .into(),
            };
            let light = p.text_bright();
            let mut how = column![].spacing(4);
            for (i, step) in spec.how.iter().enumerate() {
                how = how.push(text(format!("{}. {step}", i + 1)).size(13).color(p.text()));
            }
            column![
                container(
                    container(code).padding(12).style(move |_| container::Style {
                        background: Some(light.into()),
                        border: iced::border::rounded(8),
                        ..container::Style::default()
                    })
                )
                .center_x(Length::Fill),
                how,
            ]
            .spacing(12)
            .into()
        }
        Auth::Password => column![
            field("HOMESERVER", "matrix.org"),
            field("USERNAME", "@you:matrix.org"),
            field("PASSWORD", ""),
            text("Mesh holds one Matrix account for now; you're signed in with it.").size(12).color(p.text_muted()),
        ]
        .spacing(12)
        .into(),
        Auth::Phone => column![
            field("PHONE NUMBER", "+27 …"),
            text(format!("{} sends you a login code. Type it on the next screen.", spec.name)).size(13).color(p.text()),
        ]
        .spacing(12)
        .into(),
        Auth::Browser => column![
            button(container(ui::sans(format!("Sign in to {} in your browser", spec.name), 14.0, Weight::Semibold, p.text_bright())).center_x(Length::Fill))
                .width(Length::Fill)
                .padding(12)
                .style(flat_button_style(p)),
            text(spec.browser_note.unwrap_or_default()).size(13).color(p.text()),
        ]
        .spacing(12)
        .into(),
    }
}

/// Step 2: what shows up in Mesh.
fn sync_switches<'a>(state: &'a App, spec: &'static NetworkSpec, network: Option<Network>) -> Element<'a, Message> {
    let p = &state.palette;
    let mut list = column![text("Pick what shows up in Mesh. You can change this later.").size(13).color(p.text())].spacing(4);
    for item in spec.sync {
        let on = network.is_none_or(|n| synced(state, n, item.key));
        let toggle = network.is_some().then_some(Message::ToggleSync(spec.id, item.key));
        list = list.push(switch_row(item.label, "", on, toggle, p));
    }
    list.into()
}

fn done<'a>(spec: &'static NetworkSpec, p: &theme::Palette) -> Element<'a, Message> {
    let (fill, fg) = (theme::mix(p.success(), p.surface(), 0.85), p.success());
    column![
        container(ui::sans("✓", 24.0, Weight::Semibold, fg)).center_x(52).center_y(52).style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(26),
            ..container::Style::default()
        }),
        ui::sans(format!("{} is connected", spec.name), 16.0, Weight::Semibold, p.text_bright()),
        text("Chats appear under Home as they sync, usually in under a minute.").size(13).color(p.text_muted()),
    ]
    .spacing(10)
    .padding([30, 0])
    .align_x(iced::Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn unofficial<'a>(spec: &'static NetworkSpec, p: &theme::Palette) -> Element<'a, Message> {
    row![
        ui::mono("▲", 12.0, p.warning()),
        text(format!("{} doesn't officially support other apps. Most people are fine, but the account could be flagged.", spec.name))
            .size(12)
            .color(theme::mix(p.warning(), p.text(), 0.5)),
    ]
    .spacing(8)
    .into()
}

/// A linked account: who it is, what syncs, and signing out.
fn manage<'a>(
    state: &'a App,
    spec: &'static NetworkSpec,
    network: Option<Network>,
    link: Option<&Link>,
    confirm: bool,
) -> Element<'a, Message> {
    let p = &state.palette;
    let (name, meta, status, color) = match link {
        Some(Link::Linked { name, meta }) => (name.clone(), meta.clone(), "● synced", p.success()),
        _ => (format!("Your {} account", spec.name), "not answering yet".to_string(), "… connecting", p.text_muted()),
    };
    let mut col = column![
        column![
            ui::sans(name, 14.0, Weight::Semibold, p.text_bright()),
            row![ui::mono(status, 11.0, color), text(meta).size(12).color(p.text_muted())].spacing(8),
        ]
        .spacing(4),
    ]
    .spacing(18)
    .height(Length::Fill);
    if !spec.sync.is_empty() {
        col = col.push(sync_switches(state, spec, network));
    }
    if spec.unofficial {
        col = col.push(unofficial(spec, p));
    }
    let Some(network) = network else { return col.into() };
    let danger = p.danger();
    let label = if confirm {
        format!("Really sign out of {}? Click again", spec.name)
    } else if network == Network::Matrix {
        "Sign out".to_string()
    } else {
        format!("Unlink {}", spec.name)
    };
    col.push(Space::new().height(Length::Fill))
        .push(
            button(container(ui::sans(label, 14.0, Weight::Semibold, danger)).center_x(Length::Fill))
                .on_press(Message::SignOut(network))
                .width(Length::Fill)
                .padding([10, 16])
                .style(move |_, status| button::Style {
                    background: matches!(status, button::Status::Hovered).then(|| theme::alpha(danger, 0.12).into()),
                    border: iced::border::rounded(6).color(danger).width(1),
                    ..button::Style::default()
                }),
        )
        .push(
            text(match network {
                Network::Matrix => "Signs this device out of your homeserver.",
                _ => "Signs Mesh's browser out. Your account and chats stay as they are.",
            })
            .size(12)
            .color(p.text_muted()),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_numbers() {
        assert_eq!(phone("27821234567@c.us"), "+27821234567");
        assert_eq!(phone("27821234567:12@s.whatsapp.net"), "+27821234567");
        assert_eq!(phone("someone"), "someone");
    }
}
