//! Mesh's own identity screens (design/mesh/SPEC-identity.md): onboarding in
//! two screens on first launch (or restoring a backup), and the unlock screen
//! on every start after, shown before anything else while the identity isn't
//! open. Once it's open, Mesh's Accounts panel is Your ID (`manage`).

use super::*;
use iced::widget::{column, pick_list, qr_code, row, scrollable};
use mesh::State;

/// The `mesh-node` side of the app.
#[derive(Default)]
pub(crate) struct Mesh {
    pub node: Option<mesh::Node>,
    /// The latest report; `None` until the helper's first.
    pub identity: Option<mesh::Identity>,
    /// The identity has been opened once this run, so the other networks
    /// have started (locking again doesn't stop them).
    pub started: bool,
    /// The `mesh://` link as a QR code, while unlocked.
    qr: Option<qr_code::Data>,
    /// The node's latest `meshNet`.
    net: Option<mesh::Net>,
    /// This device's live friend codes.
    codes: Vec<mesh::Code>,
    form: Form,
    settings: Settings,
}

/// What's typed into the onboarding, restore or unlock form.
#[derive(Default)]
struct Form {
    username: String,
    /// The new password (onboarding, restore) or the password (unlock).
    password: String,
    confirm: String,
    /// Onboarding's second screen (the "can't be recovered" warning).
    warned: bool,
    /// Restoring a backup instead of making a new identity.
    restoring: bool,
    backup: Option<PathBuf>,
    backup_password: String,
    /// The latest `idCheckPassword` verdict and its hint.
    verdict: Option<(bool, String)>,
    /// Sent; waiting for the helper.
    busy: bool,
    error: Option<String>,
}

/// Your ID's forms: change password and export a backup.
#[derive(Default)]
struct Settings {
    old: String,
    new: String,
    confirm: String,
    verdict: Option<(bool, String)>,
    export_password: String,
    /// Check an ID: what's pasted, and the safety number for it (with its QR).
    check: String,
    safety: Option<(mesh::Safety, Option<qr_code::Data>)>,
    /// The command waiting for an answer.
    busy: Option<&'static str>,
    /// The last answer, for the section of the command it answers: (fn, ok, message).
    note: Option<(String, bool, String)>,
    copied: bool,
    /// Network: the pasted ticket, the last ping's answer, and whether ours was copied.
    ping: String,
    pong: Option<mesh::Pong>,
    ticket_copied: bool,
    /// Friend codes: the chosen expiry and uses (labels; `None` is the default).
    expiry: Option<&'static str>,
    uses: Option<&'static str>,
    /// The code just made, and the one last copied.
    made: Option<String>,
    code_copied: Option<String>,
    /// The code Check an ID resolved, shown with the safety number.
    resolved: Option<String>,
}

/// Friend-code expiry choices (identity's), as labels and `codeCreate` arguments.
const EXPIRIES: [(&str, &str); 7] =
    [("30 minutes", "1800"), ("1 hour", "3600"), ("6 hours", "21600"), ("12 hours", "43200"), ("24 hours", "86400"), ("7 days", ""), ("Never expires", "never")];
const USES: [(&str, &str); 5] = [("Single use", ""), ("5 uses", "5"), ("10 uses", "10"), ("25 uses", "25"), ("Unlimited uses", "unlimited")];
const DEFAULT_EXPIRY: &str = "7 days";
const DEFAULT_USES: &str = "Single use";

#[derive(Debug, Clone)]
pub(crate) enum Msg {
    Username(String),
    Password(String),
    Confirm(String),
    /// Onboarding: on to the warning, or back from it.
    Warned(bool),
    /// Onboarding: restore a backup instead, or go back.
    Restore(bool),
    PickBackup,
    BackupPicked(Option<PathBuf>),
    BackupPassword(String),
    Submit,
    CopyId,
    KeepUnlocked(bool),
    Old(String),
    New(String),
    Again(String),
    ChangePassword,
    ExportPassword(String),
    Export,
    ExportTo(Option<PathBuf>),
    CheckInput(String),
    Check,
    CopyTicket,
    PingInput(String),
    Ping,
    Expiry(&'static str),
    Uses(&'static str),
    MakeCode,
    CopyCode(String),
    RevokeCode(String),
}

/// Mesh is open: the identity is unlocked, or there's no `mesh-node` to ask.
pub(crate) fn open(state: &App) -> bool {
    state.mesh_node.is_none() || state.mesh.identity.as_ref().is_some_and(|i| i.state == State::Unlocked)
}

/// The rest of Mesh is running (it starts the first time Mesh opens).
pub(crate) fn started(state: &App) -> bool {
    state.mesh_node.is_none() || state.mesh.started
}

pub(crate) fn event(state: &mut App, event: mesh::Event) -> Task<Message> {
    let m = &mut state.mesh;
    let unlocked = m.identity.as_ref().is_some_and(|i| i.state == State::Unlocked);
    match event {
        mesh::Event::Connected(node) => m.node = Some(node),
        mesh::Event::Identity(identity) => {
            let now_unlocked = identity.state == State::Unlocked;
            m.qr = identity.link.as_deref().filter(|_| now_unlocked).and_then(|link| qr_code::Data::new(link).ok());
            if !now_unlocked {
                m.codes.clear();
            }
            m.identity = Some(identity);
            m.settings.busy = None;
            if !now_unlocked {
                m.settings = Settings::default();
            }
            if now_unlocked {
                // Don't keep the password around once it has done its job.
                m.form = Form::default();
                if !m.started {
                    m.started = true;
                    return start_app(state);
                }
            }
            m.form.busy = false;
        }
        mesh::Event::Password { ok, message } if unlocked => m.settings.verdict = Some((ok, message)),
        mesh::Event::Password { ok, message } => m.form.verdict = Some((ok, message)),
        mesh::Event::Error(error) if matches!(error.func.as_str(), "idCreate" | "idUnlock" | "idImport") => {
            let f = &mut m.form;
            f.busy = false;
            // The helper gives one message for every way unlocking can fail.
            f.error = Some(error.message);
            f.password.clear();
            f.confirm.clear();
            f.backup_password.clear();
            f.verdict = None;
            f.warned = false;
        }
        mesh::Event::Error(error) => {
            let s = &mut m.settings;
            s.busy = None;
            if error.func == "idChangePassword" {
                s.old.clear();
            }
            s.note = Some((error.func, false, error.message));
        }
        mesh::Event::Safety(safety) => {
            let qr = qr_code::Data::new(&safety.qr).ok();
            m.settings.safety = Some((safety, qr));
            m.settings.busy = None;
        }
        mesh::Event::Done(func) => {
            let s = &mut m.settings;
            s.busy = None;
            let message = match func.as_str() {
                "idChangePassword" => {
                    (s.old, s.new, s.confirm, s.verdict) = Default::default();
                    "Password changed. Use the new one from now on."
                }
                _ => {
                    s.export_password.clear();
                    "Backup saved. Keep it somewhere other than this computer."
                }
            };
            s.note = Some((func, true, message.to_string()));
        }
        mesh::Event::Net(net) => m.net = Some(net),
        mesh::Event::Ticket(ticket) => {
            m.settings.busy = None;
            m.settings.ticket_copied = true;
            return iced::clipboard::write(ticket);
        }
        mesh::Event::Ping(pong) => {
            m.settings.busy = None;
            m.settings.pong = Some(pong);
        }
        mesh::Event::Codes(codes) => {
            m.codes = codes.codes;
            let s = &mut m.settings;
            // Also comes by itself when a seed answers, so only end our own wait.
            if matches!(s.busy, Some("codeCreate" | "codeRevoke")) {
                s.busy = None;
            }
            if codes.created.is_some() {
                s.made = codes.created;
            }
        }
        mesh::Event::Resolved(resolved) => {
            // Check an ID took a code: now the safety number with the ID it points to.
            if let Some(node) = &m.node {
                m.settings.busy = Some("safetyNumber");
                m.settings.resolved = Some(resolved.code);
                node.safety_number(&resolved.id);
            }
        }
        mesh::Event::Disconnected => {
            m.node = None;
            m.identity = None;
            m.qr = None;
            m.net = None;
            m.codes.clear();
            m.form.busy = false;
            m.settings = Settings::default();
        }
    }
    Task::none()
}

pub(crate) fn update(state: &mut App, msg: Msg) -> Task<Message> {
    let m = &mut state.mesh;
    let Some(node) = m.node.clone() else { return Task::none() };
    let identity = m.identity.clone();
    let username = identity.as_ref().and_then(|i| i.username.clone()).unwrap_or_default();
    let onboarding = identity.as_ref().is_some_and(|i| i.state == State::None);
    let (f, s) = (&mut m.form, &mut m.settings);
    let recheck = matches!(msg, Msg::Username(_) | Msg::Password(_));
    match msg {
        Msg::Username(value) => f.username = value,
        Msg::Password(value) => f.password = value,
        Msg::Confirm(value) => f.confirm = value,
        Msg::Warned(on) => f.warned = on && can_continue(f),
        Msg::Restore(on) => *f = Form { restoring: on, ..Form::default() },
        Msg::PickBackup => {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Mesh backup")
                        .add_filter("Mesh backup", &["mks"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                |path| Message::MeshForm(Msg::BackupPicked(path)),
            );
        }
        Msg::BackupPicked(path) => f.backup = path.or(f.backup.take()),
        Msg::BackupPassword(value) => f.backup_password = value,
        Msg::Submit => {
            if f.busy {
                return Task::none();
            }
            f.error = None;
            match (onboarding, f.restoring, &f.backup) {
                (false, ..) => node.unlock(&f.username, &f.password),
                (true, true, Some(path)) if can_restore(f) => {
                    node.import(&path.to_string_lossy(), &f.username, &f.backup_password, &f.password)
                }
                (true, false, _) if f.warned => node.create(&f.username, &f.password),
                (true, false, _) => {
                    f.warned = can_continue(f);
                    return Task::none();
                }
                _ => return Task::none(),
            }
            f.busy = true;
            return Task::none();
        }
        Msg::CopyId => {
            s.copied = true;
            return identity.and_then(|i| i.id).map_or_else(Task::none, iced::clipboard::write);
        }
        Msg::KeepUnlocked(on) => {
            s.busy = Some("idKeepUnlocked");
            s.note = None;
            node.keep_unlocked(on);
        }
        Msg::Old(value) => s.old = value,
        Msg::New(value) => {
            s.new = value;
            s.verdict = None;
            if !s.new.is_empty() {
                node.check_password(&username, &s.new);
            }
        }
        Msg::Again(value) => s.confirm = value,
        Msg::ChangePassword => {
            if s.busy.is_none() && can_change(s) {
                s.busy = Some("idChangePassword");
                s.note = None;
                node.change_password(&username, &s.old, &s.new);
            }
        }
        Msg::ExportPassword(value) => s.export_password = value,
        Msg::Export => {
            if s.busy.is_some() || s.export_password.is_empty() {
                return Task::none();
            }
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Save a Mesh backup")
                        .set_file_name("mesh-backup.mks")
                        .save_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                |path| Message::MeshForm(Msg::ExportTo(path)),
            );
        }
        Msg::ExportTo(Some(path)) => {
            s.busy = Some("idExport");
            s.note = None;
            node.export(&path.to_string_lossy(), &username, &s.export_password);
        }
        Msg::ExportTo(None) => {}
        Msg::CheckInput(value) => {
            s.check = value;
            s.safety = None;
            s.resolved = None;
            s.note = None;
        }
        Msg::Check => {
            if s.busy.is_none() && !s.check.trim().is_empty() {
                s.note = None;
                s.resolved = None;
                if is_code(&s.check) {
                    s.busy = Some("codeResolve");
                    node.resolve(s.check.trim());
                } else {
                    s.busy = Some("safetyNumber");
                    node.safety_number(s.check.trim());
                }
            }
        }
        Msg::Expiry(label) => s.expiry = Some(label),
        Msg::Uses(label) => s.uses = Some(label),
        Msg::MakeCode => {
            if s.busy.is_none() {
                let arg = |choices: &[(&str, &'static str)], label: Option<&str>| {
                    label.and_then(|l| choices.iter().find(|(c, _)| *c == l)).map_or("", |(_, a)| *a)
                };
                s.busy = Some("codeCreate");
                s.note = None;
                node.create_code(arg(&EXPIRIES, s.expiry), arg(&USES, s.uses));
            }
        }
        Msg::CopyCode(code) => {
            s.code_copied = Some(code.clone());
            return iced::clipboard::write(code);
        }
        Msg::RevokeCode(code) => {
            if s.busy.is_none() {
                s.busy = Some("codeRevoke");
                s.note = None;
                node.revoke_code(&code);
            }
        }
        Msg::CopyTicket => {
            if s.busy.is_none() {
                s.busy = Some("netTicket");
                s.note = None;
                node.ticket();
            }
        }
        Msg::PingInput(value) => {
            s.ping = value;
            s.pong = None;
            s.note = None;
        }
        Msg::Ping => {
            if s.busy.is_none() && !s.ping.trim().is_empty() {
                s.busy = Some("netPing");
                s.note = None;
                s.pong = None;
                node.ping(s.ping.trim());
            }
        }
    }
    f.error = None;
    // The password rule involves the username too, so either change rechecks it.
    if onboarding && recheck {
        if f.password.is_empty() {
            f.verdict = None;
        } else {
            node.check_password(&f.username, &f.password);
        }
    }
    Task::none()
}

/// Onboarding's first screen is filled in well enough to go on.
fn can_continue(f: &Form) -> bool {
    !f.username.trim().is_empty() && f.verdict.as_ref().is_some_and(|(ok, _)| *ok) && f.password == f.confirm
}

fn can_restore(f: &Form) -> bool {
    f.backup.is_some() && !f.backup_password.is_empty() && can_continue(f)
}

fn can_change(s: &Settings) -> bool {
    !s.old.is_empty() && s.verdict.as_ref().is_some_and(|(ok, _)| *ok) && s.new == s.confirm
}

/// Whether Check an ID's text is a friend code rather than an ID.
fn is_code(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    text.starts_with("mesh://code/") || !(text.starts_with("mesh1") || text.starts_with("mesh://"))
}

/// A code's limits, for example "until 3 Nov 14:00 · single use".
fn describe(c: &mesh::Code) -> String {
    let until = match c.expires_at.and_then(|t| Local.timestamp_opt(t as i64, 0).single()) {
        Some(t) => format!("until {}", t.format("%-d %b %H:%M")),
        None => "never expires".into(),
    };
    let uses = match c.max_uses {
        Some(1) => "single use".into(),
        Some(n) => format!("up to {n} uses"),
        None => "unlimited uses".into(),
    };
    format!("{until} · {uses}")
}

/// Network's status line, for example "Online · relay aps1 · 1 connection, direct".
fn net_line(net: Option<&mesh::Net>) -> String {
    let Some(net) = net.filter(|n| n.state != mesh::NetState::Off) else { return "Offline".into() };
    if net.state == mesh::NetState::Connecting {
        return "Connecting…".into();
    }
    let mut line = format!("Online · relay {}", net.relay.as_deref().map(relay_name).unwrap_or("?"));
    if net.connections > 0 {
        let s = if net.connections == 1 { "" } else { "s" };
        line.push_str(&format!(" · {} connection{s}{}", net.connections, if net.direct { ", direct" } else { "" }));
    }
    line
}

/// n0's relays by their short name (`aps1`); any other relay by its host.
fn relay_name(url: &str) -> &str {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or(url);
    let host = host.strip_suffix('.').unwrap_or(host);
    match host.strip_suffix(".relay.n0.iroh.link") {
        Some(name) => name.split('-').next().unwrap_or(name),
        None => host,
    }
}

/// "direct · 0.7 ms", "relay · 24 ms".
fn pong_line(pong: &mesh::Pong) -> String {
    let ms = if pong.ms < 10.0 { format!("{:.1}", pong.ms) } else { format!("{:.0}", pong.ms) };
    format!("{} · {ms} ms", pong.path)
}

/// The new-password hint: the helper's verdict, or that the two don't match.
fn hint<'a>(verdict: &Option<(bool, String)>, password: &str, confirm: &str, p: &theme::Palette) -> Element<'a, Message> {
    if !confirm.is_empty() && confirm != password {
        return text("The passwords don't match.").size(12).color(p.warning()).into();
    }
    match verdict {
        Some((true, _)) => text("Strong enough.").size(12).color(p.success()).into(),
        Some((false, message)) => text(message.clone()).size(12).color(p.warning()).into(),
        None => text("At least 10 characters, and not your username.").size(12).color(p.text_muted()).into(),
    }
}

fn field<'a>(label: &'static str, value: &str, on: fn(String) -> Msg, secure: bool, submit: Msg, p: &theme::Palette) -> Element<'a, Message> {
    column![
        ui::mono(label, 11.0, p.text_muted()),
        text_input("", value).on_input(move |v| Message::MeshForm(on(v))).on_submit(Message::MeshForm(submit)).secure(secure).padding(10),
    ]
    .spacing(6)
    .into()
}

fn link_button<'a>(label: &'static str, msg: Msg, p: &theme::Palette) -> Element<'a, Message> {
    button(ui::sans(label, 13.0, Weight::Normal, p.accent())).on_press(Message::MeshForm(msg)).padding(0).style(iced::widget::button::text).into()
}

pub(crate) fn view(state: &App) -> Element<'_, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let m = &state.mesh;
    let f = &m.form;
    let field = |label, value, on, secure| field(label, value, on, secure, Msg::Submit, p);
    let title = |sub: &'static str| {
        column![
            row![ui::mono_bold(">_", 28.0, p.accent()), ui::mono_bold("mesh", 28.0, p.text_bright())].spacing(10),
            ui::mono(sub, 12.0, p.text_muted()),
        ]
        .spacing(4)
    };
    let submit = |label: &'static str, ready: bool| {
        let label = if f.busy { "One moment…" } else { label };
        ui::primary_button(
            container(ui::sans(label, 14.0, Weight::Semibold, p.accent_text())).center_x(Length::Fill),
            (ready && !f.busy).then_some(Message::MeshForm(Msg::Submit)),
            p,
            l,
        )
        .width(Length::Fill)
        .padding(10)
    };
    let back = |msg: Msg| {
        button(ui::sans("Back", 14.0, Weight::Normal, p.text_bright()))
            .on_press_maybe((!f.busy).then_some(Message::MeshForm(msg)))
            .padding([10, 16])
            .style(flat_button_style(p))
    };

    let mut body = match m.identity.as_ref().map(|i| i.state) {
        None => column![title("starting…"), text("Starting Mesh's engine…").size(13).color(p.text())],
        Some(State::Unlocked) => column![],
        Some(State::Locked) => column![
            title("unlock your identity"),
            field("USERNAME", &f.username, Msg::Username, false),
            field("PASSWORD", &f.password, Msg::Password, true),
            submit("Unlock", !f.username.trim().is_empty() && !f.password.is_empty()),
        ],
        Some(State::None) if f.restoring => {
            let file = f.backup.as_ref().and_then(|b| b.file_name()).map(|n| n.to_string_lossy().into_owned());
            column![
                title("restore a backup"),
                text("This install becomes a new device of the identity in the backup.").size(13).color(p.text()),
                row![
                    button(ui::sans("Choose backup file…", 13.0, Weight::Normal, p.text_bright()))
                        .on_press(Message::MeshForm(Msg::PickBackup))
                        .padding([8, 12])
                        .style(flat_button_style(p)),
                    ui::mono(file.unwrap_or_else(|| "none chosen".into()), 12.0, p.text_muted()),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                field("ITS USERNAME", &f.username, Msg::Username, false),
                field("ITS PASSWORD", &f.backup_password, Msg::BackupPassword, true),
                field("NEW PASSWORD FOR THIS DEVICE", &f.password, Msg::Password, true),
                field("NEW PASSWORD AGAIN", &f.confirm, Msg::Confirm, true),
                hint(&f.verdict, &f.password, &f.confirm, p),
                row![back(Msg::Restore(false)), submit("Restore", can_restore(f))].spacing(8),
            ]
        }
        Some(State::None) if !f.warned => column![
            title("make your identity · 1 of 2"),
            text("No email, no phone number. Pick a username and a password; you'll need both every time Mesh starts.")
                .size(13)
                .color(p.text()),
            field("USERNAME", &f.username, Msg::Username, false),
            field("PASSWORD", &f.password, Msg::Password, true),
            field("PASSWORD AGAIN", &f.confirm, Msg::Confirm, true),
            hint(&f.verdict, &f.password, &f.confirm, p),
            submit("Continue", can_continue(f)),
            link_button("I have a backup", Msg::Restore(true), p),
        ],
        Some(State::None) => column![
            title("make your identity · 2 of 2"),
            ui::sans("Your password can't be recovered.", 15.0, Weight::Semibold, p.warning()),
            text(format!(
                "There's no account and no server, so there's no reset link. If you forget the username \"{}\" or its password, \
                 this identity and everyone who knows it are gone. Write them down somewhere safe.",
                f.username.trim()
            ))
            .size(13)
            .color(p.text()),
            row![back(Msg::Warned(false)), submit("I understand, create it", true)].spacing(8),
        ],
    }
    .spacing(14);
    if let Some(error) = &f.error {
        body = body.push(text(error.clone()).size(13).color(p.danger()));
    }

    container(ui::pane(body.padding(28), true, p, l, state.extras.translucent_panes).max_width(440))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// Your ID: Mesh's Accounts panel once it's open.
pub(crate) fn manage(state: &App) -> Element<'_, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let m = &state.mesh;
    let s = &m.settings;
    let Some(identity) = m.identity.as_ref().filter(|i| i.state == State::Unlocked) else {
        return text("Mesh isn't open.").size(13).color(p.text_muted()).into();
    };
    let note = |func: &str| -> Option<Element<'_, Message>> {
        let (_, ok, message) = s.note.as_ref().filter(|(f, ..)| f == func)?;
        Some(text(message.clone()).size(12).color(if *ok { p.success() } else { p.danger() }).into())
    };
    let action = |label: &'static str, busy_fn: &'static str, msg: Option<Msg>| {
        let label = if s.busy == Some(busy_fn) { "One moment…" } else { label };
        ui::primary_button(
            container(ui::sans(label, 13.0, Weight::Semibold, p.accent_text())).center_x(Length::Fill),
            msg.filter(|_| s.busy.is_none()).map(Message::MeshForm),
            p,
            l,
        )
        .width(Length::Fill)
        .padding([8, 12])
    };

    // Your ID: the text, copy, and its QR code (the mesh:// link).
    let mut id = column![
        heading("your id", p),
        text(identity.id.clone().unwrap_or_default()).font(iced::Font::MONOSPACE).size(12).color(p.text_bright()),
        button(ui::sans(if s.copied { "Copied" } else { "Copy ID" }, 13.0, Weight::Normal, p.text_bright()))
            .on_press(Message::MeshForm(Msg::CopyId))
            .padding([6, 12])
            .style(flat_button_style(p)),
    ]
    .spacing(8);
    if let Some(qr) = &m.qr {
        id = id.push(
            container(qr_code(qr).cell_size(4).style(|_| qr_code::Style { cell: iced::Color::BLACK, background: iced::Color::WHITE }))
                .padding(10)
                .style(|_| container::Style { background: Some(iced::Color::WHITE.into()), border: iced::border::rounded(8), ..container::Style::default() }),
        );
    }

    // Check an ID: the safety number both people read out (D7).
    let mut check = column![
        heading("check an id", p),
        text("Paste a friend's ID, mesh:// link or friend code. Read the number to each other: if it matches, nobody is in the middle.")
            .size(12)
            .color(p.text()),
        field("THEIR ID OR CODE", &s.check, Msg::CheckInput, false, Msg::Check, p),
        action("Check", if s.busy == Some("codeResolve") { "codeResolve" } else { "safetyNumber" }, (!s.check.trim().is_empty()).then_some(Msg::Check)),
    ]
    .spacing(8)
    .extend(note("codeResolve"))
    .extend(note("safetyNumber"));
    if let Some((safety, qr)) = &s.safety {
        let groups: Vec<&str> = safety.number.split_whitespace().collect();
        let mut digits = column![].spacing(4);
        for line in groups.chunks(4) {
            digits = digits.push(text(line.join("  ")).font(iced::Font::MONOSPACE).size(15).color(p.text_bright()));
        }
        if let Some(code) = &s.resolved {
            check = check.push(ui::sans(format!("The code {code} is this ID:"), 12.0, Weight::Normal, p.text()));
        }
        check = check.push(ui::mono(format!("with {}", safety.id), 11.0, p.text_muted())).push(digits);
        if let Some(qr) = qr {
            check = check.push(
                container(qr_code(qr).cell_size(3).style(|_| qr_code::Style { cell: iced::Color::BLACK, background: iced::Color::WHITE }))
                    .padding(10)
                    .style(|_| container::Style { background: Some(iced::Color::WHITE.into()), border: iced::border::rounded(8), ..container::Style::default() }),
            );
        }
    }

    // Friend codes: short, expiring stand-ins for the ID, held by the seeds.
    let expiry = s.expiry.unwrap_or(DEFAULT_EXPIRY);
    let mut codes = column![
        heading("friend codes", p),
        text("Give someone a short code instead of your ID. Whoever types it in sees your ID; check the safety number together after.")
            .size(12)
            .color(p.text()),
        row![
            pick_list(EXPIRIES.map(|(l, _)| l).to_vec(), Some(expiry), |l| Message::MeshForm(Msg::Expiry(l))).padding(6),
            pick_list(USES.map(|(l, _)| l).to_vec(), Some(s.uses.unwrap_or(DEFAULT_USES)), |l| Message::MeshForm(Msg::Uses(l))).padding(6),
        ]
        .spacing(8),
    ]
    .spacing(8);
    if expiry == "Never expires" {
        codes = codes.push(text("A code that never expires works until you revoke it.").size(12).color(p.warning()));
    }
    codes = codes.push(action("Make a code", "codeCreate", Some(Msg::MakeCode))).extend(note("codeCreate"));
    for c in &m.codes {
        let mut info = column![ui::mono(describe(c), 11.0, p.text_muted())].spacing(2);
        if !c.held {
            info = info.push(ui::mono("not saved to a seed yet", 11.0, p.warning()));
        }
        let new = s.made.as_deref() == Some(c.code.as_str());
        let copied = s.code_copied.as_deref() == Some(c.code.as_str());
        codes = codes.push(
            row![
                ui::mono_bold(c.code.clone(), 15.0, if new { p.accent() } else { p.text_bright() }),
                info,
                Space::new().width(Length::Fill),
                button(ui::sans(if copied { "Copied" } else { "Copy" }, 12.0, Weight::Normal, p.text_bright()))
                    .on_press(Message::MeshForm(Msg::CopyCode(c.code.clone())))
                    .padding([4, 10])
                    .style(flat_button_style(p)),
                button(ui::sans("Revoke", 12.0, Weight::Normal, p.danger()))
                    .on_press_maybe(s.busy.is_none().then(|| Message::MeshForm(Msg::RevokeCode(c.code.clone()))))
                    .padding([4, 10])
                    .style(flat_button_style(p)),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );
    }
    codes = codes.extend(note("codeRevoke"));

    // Network: is this device on Mesh, and can it reach another (a test until friends exist).
    let mut network = column![
        heading("network", p),
        ui::sans(net_line(m.net.as_ref()), 13.0, Weight::Normal, p.text_bright()),
        text("To test a connection, copy your ticket and send it to someone on another Mesh; paste theirs below.")
            .size(12)
            .color(p.text()),
        button(ui::sans(if s.ticket_copied { "Ticket copied" } else { "Copy test ticket" }, 13.0, Weight::Normal, p.text_bright()))
            .on_press_maybe(s.busy.is_none().then_some(Message::MeshForm(Msg::CopyTicket)))
            .padding([6, 12])
            .style(flat_button_style(p)),
    ]
    .spacing(8)
    .extend(note("netTicket"))
    .push(field("THEIR TICKET", &s.ping, Msg::PingInput, false, Msg::Ping, p))
    .push(action("Ping", "netPing", (!s.ping.trim().is_empty()).then_some(Msg::Ping)))
    .extend(note("netPing"));
    if let Some(pong) = &s.pong {
        network = network
            .push(ui::sans(format!("{} answered · {}", pong.name, pong_line(pong)), 13.0, Weight::Normal, p.success()))
            .push(ui::mono(pong.id.clone(), 11.0, p.text_muted()));
    }

    let mut devices = column![heading("devices", p)].spacing(6);
    for d in &identity.devices {
        let added = Local.timestamp_opt(d.created_at as i64, 0).single().map(|t| t.format("%-d %b %Y").to_string()).unwrap_or_default();
        devices = devices.push(
            row![
                ui::sans(d.name.clone(), 13.0, Weight::Normal, p.text_bright()),
                ui::mono(if d.this_device { format!("this device · {added}") } else { added }, 11.0, p.text_muted()),
            ]
            .spacing(8),
        );
    }

    let keep = column![
        switch_row(
            "Keep unlocked on this device",
            identity.keyring.as_deref().unwrap_or("Opens without asking. Only as safe as your computer login."),
            identity.keep_unlocked,
            (identity.keyring.is_none() && s.busy.is_none()).then(|| Message::MeshForm(Msg::KeepUnlocked(!identity.keep_unlocked))),
            p,
        ),
    ]
    .extend(note("idKeepUnlocked"));

    let password = column![
        heading("change password", p),
        field("CURRENT PASSWORD", &s.old, Msg::Old, true, Msg::ChangePassword, p),
        field("NEW PASSWORD", &s.new, Msg::New, true, Msg::ChangePassword, p),
        field("NEW PASSWORD AGAIN", &s.confirm, Msg::Again, true, Msg::ChangePassword, p),
        hint(&s.verdict, &s.new, &s.confirm, p),
        action("Change password", "idChangePassword", can_change(s).then_some(Msg::ChangePassword)),
    ]
    .spacing(8)
    .extend(note("idChangePassword"));

    let backup = column![
        heading("backup", p),
        text("A file that holds your identity. It opens with your username and your password.").size(12).color(p.text()),
        field("YOUR PASSWORD", &s.export_password, Msg::ExportPassword, true, Msg::Export, p),
        action("Export a backup…", "idExport", (!s.export_password.is_empty()).then_some(Msg::Export)),
    ]
    .spacing(8)
    .extend(note("idExport"));

    scrollable(column![id, codes, check, network, devices, keep, password, backup].spacing(22).padding(iced::Padding { right: 12.0, ..iced::Padding::ZERO }))
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_needs_a_good_matching_password() {
        let mut f = Form { username: "brendon".into(), password: "correct horse battery".into(), confirm: "correct horse battery".into(), ..Form::default() };
        assert!(!can_continue(&f), "no verdict yet");
        f.verdict = Some((false, "Add another word.".into()));
        assert!(!can_continue(&f));
        f.verdict = Some((true, String::new()));
        assert!(can_continue(&f));
        assert!(!can_restore(&f), "no backup chosen");
        f.backup = Some(PathBuf::from("/tmp/b.mks"));
        f.backup_password = "old one".into();
        assert!(can_restore(&f));
        f.confirm.push('x');
        assert!(!can_continue(&f), "mismatch");
        f.confirm.pop();
        f.username = "  ".into();
        assert!(!can_continue(&f), "no username");
    }

    #[test]
    fn codes() {
        for code in ["K7QM-3XPA", "k7qm3xpa", "mesh://code/K7QM-3XPA", " MESH://CODE/k7qm-3xpa "] {
            assert!(is_code(code), "{code}");
        }
        for id in ["mesh1qch7juuwr5jfh3ue964tak2jhx3qj5mt2dhnf8j4lusjfyl6llmsrutrz3", "mesh://id/mesh1abc", "MESH1ABC"] {
            assert!(!is_code(id), "{id}");
        }
        let mut c = mesh::Code { code: "K7QM-3XPA".into(), link: String::new(), created_at: 0, expires_at: None, max_uses: Some(1), held: true };
        assert_eq!(describe(&c), "never expires · single use");
        (c.max_uses, c.expires_at) = (None, Some(1_800_000_000));
        assert!(describe(&c).starts_with("until "), "{}", describe(&c));
        assert!(describe(&c).ends_with(" · unlimited uses"));
        c.max_uses = Some(25);
        assert!(describe(&c).ends_with("up to 25 uses"));
    }

    #[test]
    fn network_lines() {
        let mut net = mesh::Net { state: mesh::NetState::Online, relay: Some("https://aps1-1.relay.n0.iroh.link./".into()), direct: false, connections: 0 };
        assert_eq!(net_line(Some(&net)), "Online · relay aps1");
        (net.connections, net.direct) = (2, true);
        assert_eq!(net_line(Some(&net)), "Online · relay aps1 · 2 connections, direct");
        net.relay = Some("https://relay.example.org:8443/".into());
        assert!(net_line(Some(&net)).starts_with("Online · relay relay.example.org ·"));
        net.state = mesh::NetState::Connecting;
        assert_eq!(net_line(Some(&net)), "Connecting…");
        assert_eq!(net_line(None), "Offline");
        let pong = mesh::Pong { id: "mesh1abc".into(), name: "box".into(), path: "direct".into(), ms: 0.74 };
        assert_eq!(pong_line(&pong), "direct · 0.7 ms");
        assert_eq!(pong_line(&mesh::Pong { ms: 24.4, path: "relay".into(), ..pong }), "relay · 24 ms");
    }

    #[test]
    fn change_needs_the_old_and_a_good_new_password() {
        let mut s = Settings { old: "old".into(), new: "violet harbor quantum".into(), confirm: "violet harbor quantum".into(), ..Settings::default() };
        assert!(!can_change(&s), "no verdict yet");
        s.verdict = Some((true, String::new()));
        assert!(can_change(&s));
        s.old.clear();
        assert!(!can_change(&s));
    }
}
