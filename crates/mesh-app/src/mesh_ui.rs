//! Mesh's own identity screens (design/mesh/SPEC-identity.md): onboarding in
//! two screens on first launch, and the unlock screen on every start after,
//! shown before anything else while the identity isn't open.

use super::*;
use iced::widget::{column, row};
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
    form: Form,
}

/// What's typed into the onboarding or unlock form.
#[derive(Default)]
struct Form {
    username: String,
    password: String,
    confirm: String,
    /// Onboarding's second screen (the "can't be recovered" warning).
    warned: bool,
    /// The latest `idCheckPassword` verdict and its hint.
    verdict: Option<(bool, String)>,
    /// Sent; waiting for the helper.
    busy: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Msg {
    Username(String),
    Password(String),
    Confirm(String),
    /// Onboarding: on to the warning, or back from it.
    Warned(bool),
    Submit,
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
    match event {
        mesh::Event::Connected(node) => m.node = Some(node),
        mesh::Event::Identity(identity) => {
            let unlocked = identity.state == State::Unlocked;
            m.identity = Some(identity);
            if unlocked {
                // Don't keep the password around once it has done its job.
                m.form = Form::default();
                if !m.started {
                    m.started = true;
                    return start_app(state);
                }
            }
            m.form.busy = false;
        }
        mesh::Event::Password { ok, message } => m.form.verdict = Some((ok, message)),
        mesh::Event::Error(error) => {
            m.form.busy = false;
            // The helper gives one message for every way unlocking can fail.
            m.form.error = Some(error.message);
            m.form.password.clear();
            m.form.confirm.clear();
            m.form.verdict = None;
            m.form.warned = false;
        }
        mesh::Event::Disconnected => {
            m.node = None;
            m.identity = None;
            m.form.busy = false;
        }
    }
    Task::none()
}

pub(crate) fn update(state: &mut App, msg: Msg) -> Task<Message> {
    let m = &mut state.mesh;
    let onboarding = m.identity.as_ref().is_some_and(|i| i.state == State::None);
    let f = &mut m.form;
    let recheck = matches!(msg, Msg::Username(_) | Msg::Password(_));
    match msg {
        Msg::Username(value) => f.username = value,
        Msg::Password(value) => f.password = value,
        Msg::Confirm(value) => f.confirm = value,
        Msg::Warned(on) => f.warned = on && can_continue(f),
        Msg::Submit => {
            let Some(node) = &m.node else { return Task::none() };
            if f.busy {
                return Task::none();
            }
            if !onboarding {
                f.busy = true;
                node.unlock(&f.username, &f.password);
            } else if f.warned {
                f.busy = true;
                node.create(&f.username, &f.password);
            } else {
                f.warned = can_continue(f);
            }
            f.error = None;
            return Task::none();
        }
    }
    f.error = None;
    // The password rule involves the username too, so either change rechecks it.
    if onboarding && recheck && let Some(node) = &m.node {
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

pub(crate) fn view(state: &App) -> Element<'_, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let m = &state.mesh;
    let f = &m.form;
    let field = |label: &'static str, value: &str, on: fn(String) -> Msg, secure: bool| {
        column![
            ui::mono(label, 11.0, p.text_muted()),
            text_input("", value).on_input(move |v| Message::MeshForm(on(v))).on_submit(Message::MeshForm(Msg::Submit)).secure(secure).padding(10),
        ]
        .spacing(6)
    };
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

    let mut body = match m.identity.as_ref().map(|i| i.state) {
        None => column![title("starting…"), text("Starting Mesh's engine…").size(13).color(p.text())],
        Some(State::Unlocked) => column![],
        Some(State::Locked) => column![
            title("unlock your identity"),
            field("USERNAME", &f.username, Msg::Username, false),
            field("PASSWORD", &f.password, Msg::Password, true),
            submit("Unlock", !f.username.trim().is_empty() && !f.password.is_empty()),
        ],
        Some(State::None) if !f.warned => {
            let mut hint = match &f.verdict {
                Some((ok, message)) => text(if *ok { "Strong enough.".to_string() } else { message.clone() })
                    .size(12)
                    .color(if *ok { p.success() } else { p.warning() }),
                None => text("At least 10 characters, and not your username.").size(12).color(p.text_muted()),
            };
            if !f.confirm.is_empty() && f.confirm != f.password {
                hint = text("The passwords don't match.").size(12).color(p.warning());
            }
            column![
                title("make your identity · 1 of 2"),
                text("No email, no phone number. Pick a username and a password; you'll need both every time Mesh starts.")
                    .size(13)
                    .color(p.text()),
                field("USERNAME", &f.username, Msg::Username, false),
                field("PASSWORD", &f.password, Msg::Password, true),
                field("PASSWORD AGAIN", &f.confirm, Msg::Confirm, true),
                hint,
                submit("Continue", can_continue(f)),
            ]
        }
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
            row![
                button(ui::sans("Back", 14.0, Weight::Normal, p.text_bright()))
                    .on_press_maybe((!f.busy).then_some(Message::MeshForm(Msg::Warned(false))))
                    .padding([10, 16])
                    .style(flat_button_style(p)),
                submit("I understand, create it", true),
            ]
            .spacing(8),
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
        f.confirm.push('x');
        assert!(!can_continue(&f), "mismatch");
        f.confirm.pop();
        f.username = "  ".into();
        assert!(!can_continue(&f), "no username");
    }
}
