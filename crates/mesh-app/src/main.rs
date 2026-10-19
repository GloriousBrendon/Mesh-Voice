#![recursion_limit = "1024"]

mod rain;
mod theme;
mod ui;

use chrono::{DateTime, Local, TimeZone};
use iced::font::Weight;
use iced::widget::{
    Space, button, column, container, mouse_area, pick_list, responsive, row, scrollable, stack,
    text, text_input,
};
use iced::{Element, Length, Subscription, Task, Theme as IcedTheme, keyboard};
use mesh_core::{
    BackupStatus, MeshClient, MemberInfo, MeshUpdate, MyProfile, OwnedRoomId, OwnedUserId,
    MessageKind, Network, OwnedEventId, PendingContact, RoomSummary, SasEmoji, SpaceGroup, TimelineMessage,
    group_by_space,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// How many messages to fetch when opening a room.
const TIMELINE_LIMIT: u16 = 50;

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mesh")
}

fn theme_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mesh")
        .join("theme.toml")
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mesh")
        .join("config.toml")
}

/// Spawns the never-ending sync loop in the background and hands back a
/// shareable client handle. Detached from the `Task` that created it: sync
/// keeps running for as long as the process is alive.
async fn start_session(client: MeshClient) -> Arc<MeshClient> {
    let client = Arc::new(client);
    let sync_client = client.clone();
    tokio::spawn(async move {
        if let Err(err) = sync_client.run_sync().await {
            tracing::error!("sync loop stopped: {err}");
        }
    });
    client
}

/// Bridges the sync-driven update channel into the iced runtime, so the UI
/// reacts when sync delivers something instead of polling on a timer.
fn watch_updates(client: Arc<MeshClient>) -> Task<Message> {
    let receiver = client.subscribe_updates();
    Task::stream(iced::futures::stream::unfold(
        receiver,
        |mut receiver| async move {
            receiver.recv().await.map(|update| (Message::Update(update), receiver))
        },
    ))
}

struct App {
    palette: theme::Palette,
    theme_choice: String,
    iced_theme: IcedTheme,
    layout: theme::Layout,
    layout_from_hyprland: bool,
    density: theme::Density,
    extras: theme::Extras,
    discord_warning_seen: bool,
    /// When the theme files were last (re)read, "HH:MM", for the Rice page.
    theme_reloaded: String,
    /// Theme source files and their last-seen mtimes, polled for hot reload.
    theme_files: Vec<PathBuf>,
    theme_stamps: Vec<Option<std::time::SystemTime>>,
    screen: Screen,
    /// Elapsed seconds driving the animated background.
    rain_time: f32,
}

enum Screen {
    Booting,
    LoggedOut(LoginForm),
    LoggingIn(LoginForm),
    LoggedIn(Session),
}

#[derive(Default, Clone)]
struct LoginForm {
    homeserver: String,
    username: String,
    password: String,
    error: Option<String>,
}

struct Session {
    client: Arc<MeshClient>,
    rooms: Vec<RoomSummary>,
    selected_room: Option<OwnedRoomId>,
    timeline: Vec<TimelineMessage>,
    message_draft: String,
    status: Option<String>,
    verification: Option<VerificationPrompt>,
    show_settings: bool,
    settings_page: SettingsPage,
    call: Option<CallUi>,
    members: Vec<MemberInfo>,
    my_profile: Option<MyProfile>,
    self_muted: bool,
    self_deafened: bool,
    backup: BackupUi,
    /// Audio device pickers (each list has [`DEFAULT_DEVICE`] first).
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    selected_input: String,
    selected_output: String,
    /// From config.toml; shows the 🎧 Discord voice button when set.
    discord_voice_url: Option<String>,
    /// The pane with the accent border.
    focused: Pane,
    show_members: bool,
    /// Home first, then one group per space (the rail).
    groups: Vec<SpaceGroup>,
    /// The space shown in the channel pane; `None` is Home.
    active_space: Option<OwnedRoomId>,
    /// The Ctrl+K jump palette's query, while it's open.
    jump: Option<String>,
    /// Who's typing, per room.
    typing: HashMap<OwnedRoomId, Vec<String>>,
    /// Downloaded images by media key; `None` while loading.
    images: HashMap<String, Option<iced::widget::image::Handle>>,
    /// The add-friend dialog, while it's open.
    add_friend: Option<AddFriend>,
}

/// Text input id of the jump palette, to focus it on open.
const JUMP_INPUT: &str = "jump";

/// How many matches the jump palette lists.
const JUMP_RESULTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Accounts,
    Privacy,
    Rice,
    Keybinds,
    Voice,
}

/// The Rice page's switches.
#[derive(Debug, Clone, Copy)]
enum Extra {
    Rain,
    Translucent,
    Tags,
    PlainText,
}

/// The tiled shell's panes, for focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Rail,
    Channels,
    Chat,
    Members,
}

/// Sentinel shown in the device pickers for "use the system default".
const DEFAULT_DEVICE: &str = "(System default)";

/// Secure Backup UI state, shown in settings.
#[derive(Default)]
struct BackupUi {
    status: Option<BackupStatus>,
    /// A freshly generated recovery key to show the user once.
    new_key: Option<String>,
    /// The recovery key the user is typing to restore.
    key_input: String,
    busy: bool,
    message: Option<String>,
}

impl Session {
    fn new(client: Arc<MeshClient>) -> Self {
        Self {
            client,
            rooms: Vec::new(),
            selected_room: None,
            timeline: Vec::new(),
            message_draft: String::new(),
            status: None,
            verification: None,
            show_settings: false,
            settings_page: SettingsPage::Rice,
            call: None,
            members: Vec::new(),
            my_profile: None,
            self_muted: false,
            self_deafened: false,
            backup: BackupUi::default(),
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            selected_input: DEFAULT_DEVICE.to_string(),
            selected_output: DEFAULT_DEVICE.to_string(),
            discord_voice_url: theme::load_discord_voice_url(&config_path()),
            focused: Pane::Chat,
            show_members: true,
            groups: Vec::new(),
            active_space: None,
            jump: None,
            typing: HashMap::new(),
            images: HashMap::new(),
            add_friend: None,
        }
    }
}

/// UI-side view of the single active voice call.
struct CallUi {
    call_id: String,
    /// Human label for the other party (room name or caller id).
    peer: String,
    state: CallUiState,
}

#[derive(PartialEq)]
enum CallUiState {
    /// We placed a call and are waiting for the other side to answer.
    OutgoingRinging,
    /// Someone is calling us; waiting for accept/decline.
    IncomingRinging,
    /// Answered on both sides; media is negotiating.
    Connecting,
    /// Media is flowing.
    Connected,
}

impl CallUiState {
    fn label(&self) -> &'static str {
        match self {
            CallUiState::OutgoingRinging => "Ringing…",
            CallUiState::IncomingRinging => "Incoming call",
            CallUiState::Connecting => "Connecting…",
            CallUiState::Connected => "In call",
        }
    }
}

struct VerificationPrompt {
    user_id: OwnedUserId,
    flow_id: String,
    stage: VerifyStage,
}

enum VerifyStage {
    /// Incoming request, waiting for the user to accept or decline.
    Requested,
    /// Accepted; waiting for the SAS emojis to be negotiated.
    Accepted,
    /// Emojis on screen, waiting for the user to compare them.
    Emojis(Vec<SasEmoji>),
    /// User confirmed; waiting for the other side to do the same.
    Confirmed,
}

#[derive(Debug, Clone)]
enum Message {
    RestoreFinished(Result<Arc<MeshClient>, String>),
    HomeserverChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
    LoginSubmitted,
    LoginFinished(Result<Arc<MeshClient>, String>),
    RoomsLoaded(Vec<RoomSummary>),
    RoomSelected(OwnedRoomId),
    TimelineLoaded(OwnedRoomId, Result<Vec<TimelineMessage>, String>),
    Update(MeshUpdate),
    DraftChanged(String),
    SendMessage,
    MessageSent(Result<(), String>),
    AcceptVerification,
    ConfirmVerification,
    CancelVerification,
    VerificationActionFinished(Result<(), String>),
    ToggleSettings,
    OpenDiscordVoice,
    ThemeChosen(String),
    LogOut,
    PlaceCall,
    AcceptCall,
    HangUp,
    ToggleMute,
    ToggleDeafen,
    MembersLoaded(OwnedRoomId, Vec<MemberInfo>),
    ProfileLoaded(MyProfile),
    CallActionFinished(Result<(), String>),
    /// Animation frame for the background.
    Tick,
    /// Poll the theme files for changes.
    ThemeCheck,
    FocusPane(Pane),
    ToggleMembers,
    SpaceSelected(Option<OwnedRoomId>),
    /// Alt+n: the n-th space on the rail.
    SpaceKey(usize),
    OpenJump,
    JumpInput(String),
    /// Enter in the jump palette: go to the top match.
    JumpSubmit,
    JumpTo(OwnedRoomId),
    ImageLoaded(String, Result<Vec<u8>, String>),
    /// Click on a reaction pill: add ours, or take it back (`mine`).
    ToggleReaction {
        event_id: OwnedEventId,
        key: String,
        mine: Option<OwnedEventId>,
    },
    ReactionDone(Result<(), String>),
    OpenSettings(SettingsPage),
    DensityChosen(theme::Density),
    ToggleExtra(Extra),
    /// Copy the generated theme.toml to the clipboard.
    ExportTheme,
    DiscordWarningSeen,
    OpenAddFriend,
    FriendMode(FriendMode),
    FriendQuery(String),
    /// A profile lookup finished: (user id, display name).
    FriendProfile(String, Option<String>),
    FriendSend,
    FriendSent(Result<OwnedRoomId, String>),
    PendingLoaded(Vec<PendingContact>),
    PendingAccept(OwnedRoomId),
    PendingIgnore(OwnedRoomId),
    PendingCancel(OwnedRoomId, OwnedUserId),
    PendingDone(Result<(), String>),
    /// Esc: close settings (and any open dialog).
    Escape,
    SetupBackup,
    BackupSetupFinished(Result<String, String>),
    RecoveryKeyInput(String),
    RestoreBackup,
    BackupRestoreFinished(Result<(), String>),
    /// Result of the automatic backup enable on login.
    EnsureBackupFinished(Result<Option<String>, String>),
    InputDeviceSelected(String),
    OutputDeviceSelected(String),
}

fn boot() -> (App, Task<Message>) {
    let config = theme::load_config(&config_path());
    let theme_choice = theme::load_choice(&config_path());
    let palette = theme::palette_for_choice(&theme_choice, &theme_path());
    let iced_theme = palette.to_iced_theme();
    let theme_files = theme::watched_files(&theme_choice, &theme_path());

    let (screen, task) = if MeshClient::has_saved_session(&data_dir()) {
        let task = Task::perform(
            async move {
                match MeshClient::restore(data_dir()).await {
                    Ok(client) => Ok(start_session(client).await),
                    Err(err) => Err(err.to_string()),
                }
            },
            Message::RestoreFinished,
        );
        (Screen::Booting, task)
    } else {
        (Screen::LoggedOut(LoginForm::default()), Task::none())
    };

    (
        App {
            palette,
            theme_choice,
            iced_theme,
            layout: theme::layout_for_config(&config_path()),
            layout_from_hyprland: config.layout_from_hyprland.unwrap_or(true),
            density: config.density.unwrap_or_default(),
            extras: config.extras,
            discord_warning_seen: config.discord_warning_seen,
            theme_reloaded: Local::now().format("%H:%M").to_string(),
            theme_stamps: theme::stamps(&theme_files),
            theme_files,
            screen,
            rain_time: 0.0,
        },
        task,
    )
}

/// Enters the logged-in screen and kicks off the room list load plus the
/// push-update stream.
fn enter_session(state: &mut App, client: Arc<MeshClient>) -> Task<Message> {
    state.screen = Screen::LoggedIn(Session::new(client.clone()));
    let rooms_client = client.clone();
    let profile_client = client.clone();
    let backup_client = client.clone();
    Task::batch([
        Task::perform(async move { rooms_client.rooms().await }, Message::RoomsLoaded),
        Task::perform(
            async move { profile_client.my_profile().await },
            Message::ProfileLoaded,
        ),
        // Make sure key backup is on so keys persist going forward.
        Task::perform(
            async move { backup_client.ensure_secure_backup().await },
            Message::EnsureBackupFinished,
        ),
        watch_updates(client),
    ])
}

fn load_members(client: Arc<MeshClient>, room_id: OwnedRoomId) -> Task<Message> {
    Task::perform(
        async move {
            let members = client.room_members(&room_id).await;
            (room_id, members)
        },
        |(room_id, members)| Message::MembersLoaded(room_id, members),
    )
}

/// Starts downloads for the timeline's images that aren't cached or loading.
fn load_images(session: &mut Session) -> Task<Message> {
    let mut tasks = Vec::new();
    for message in &session.timeline {
        let MessageKind::Image(info) = &message.kind else {
            continue;
        };
        let key = info.media.key();
        if session.images.contains_key(&key) {
            continue;
        }
        session.images.insert(key.clone(), None);
        let (client, media) = (session.client.clone(), info.media.clone());
        tasks.push(Task::perform(
            async move { client.media(&media).await.map_err(|e| e.to_string()) },
            move |result| Message::ImageLoaded(key.clone(), result),
        ));
    }
    Task::batch(tasks)
}

fn load_timeline(client: Arc<MeshClient>, room_id: OwnedRoomId) -> Task<Message> {
    Task::perform(
        async move {
            let timeline = client
                .timeline(&room_id, TIMELINE_LIMIT)
                .await
                .map_err(|e| e.to_string());
            (room_id, timeline)
        },
        |(room_id, timeline)| Message::TimelineLoaded(room_id, timeline),
    )
}

fn update(state: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::RestoreFinished(Ok(client)) => enter_session(state, client),
        Message::RestoreFinished(Err(err)) => {
            state.screen = Screen::LoggedOut(LoginForm {
                error: Some(format!("Session expired, please log in again ({err})")),
                ..LoginForm::default()
            });
            Task::none()
        }
        Message::HomeserverChanged(value) => {
            if let Screen::LoggedOut(form) = &mut state.screen {
                form.homeserver = value;
            }
            Task::none()
        }
        Message::UsernameChanged(value) => {
            if let Screen::LoggedOut(form) = &mut state.screen {
                form.username = value;
            }
            Task::none()
        }
        Message::PasswordChanged(value) => {
            if let Screen::LoggedOut(form) = &mut state.screen {
                form.password = value;
            }
            Task::none()
        }
        Message::LoginSubmitted => {
            let Screen::LoggedOut(form) = &state.screen else {
                return Task::none();
            };
            let (homeserver, username, password) =
                (form.homeserver.clone(), form.username.clone(), form.password.clone());
            state.screen = Screen::LoggingIn(form.clone());
            Task::perform(
                async move {
                    match MeshClient::login_with_password(
                        &homeserver,
                        &username,
                        &password,
                        data_dir(),
                    )
                    .await
                    {
                        Ok(client) => Ok(start_session(client).await),
                        Err(err) => Err(err.to_string()),
                    }
                },
                Message::LoginFinished,
            )
        }
        Message::LoginFinished(Ok(client)) => enter_session(state, client),
        Message::LoginFinished(Err(err)) => {
            let form = match &state.screen {
                Screen::LoggingIn(form) => form.clone(),
                _ => LoginForm::default(),
            };
            state.screen = Screen::LoggedOut(LoginForm {
                error: Some(err),
                ..form
            });
            Task::none()
        }
        Message::RoomsLoaded(rooms) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                // An invite we just accepted: load it now that it's readable.
                let joined_now = session.selected_room.as_ref().filter(|id| {
                    is_invite(session, id) && rooms.iter().any(|r| &r.id == *id && !r.is_invite)
                });
                let reload = joined_now.map(|id| {
                    Task::batch([
                        load_timeline(session.client.clone(), id.clone()),
                        load_members(session.client.clone(), id.clone()),
                    ])
                });
                session.groups = group_by_space(&rooms);
                if !session.groups.iter().any(|g| g.id() == session.active_space.as_ref()) {
                    session.active_space = None;
                }
                session.rooms = rooms;
                if let Some(reload) = reload {
                    return reload;
                }
            }
            Task::none()
        }
        Message::RoomSelected(room_id) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.selected_room = Some(room_id.clone());
            session.focused = Pane::Chat;
            session.timeline.clear();
            session.members.clear();
            session.status = None;
            session.show_settings = false;
            // An invite can't be read until it's accepted (the server says 403).
            if is_invite(session, &room_id) {
                return Task::none();
            }
            Task::batch([
                load_timeline(session.client.clone(), room_id.clone()),
                load_members(session.client.clone(), room_id),
            ])
        }
        Message::TimelineLoaded(room_id, result) => {
            let plain = state.extras.plain_text_mode;
            if let Screen::LoggedIn(session) = &mut state.screen
                && session.selected_room.as_ref() == Some(&room_id)
            {
                match result {
                    Ok(timeline) => {
                        session.timeline = timeline;
                        if !plain {
                            return load_images(session);
                        }
                    }
                    Err(err) => session.status = Some(format!("Failed to load messages: {err}")),
                }
            }
            Task::none()
        }
        Message::ImageLoaded(key, result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                match result {
                    Ok(bytes) => {
                        session
                            .images
                            .insert(key, Some(iced::widget::image::Handle::from_bytes(bytes)));
                    }
                    Err(err) => {
                        // Leave it out of the cache so the next timeline load retries.
                        tracing::warn!("image download failed: {err}");
                        session.images.remove(&key);
                    }
                }
            }
            Task::none()
        }
        Message::ToggleReaction { event_id, key, mine } => {
            let Screen::LoggedIn(session) = &state.screen else {
                return Task::none();
            };
            let Some(room_id) = session.selected_room.clone() else {
                return Task::none();
            };
            let client = session.client.clone();
            Task::perform(
                async move {
                    match mine {
                        Some(ours) => client.redact(&room_id, &ours).await,
                        None => client.react(&room_id, &event_id, &key).await,
                    }
                    .map_err(|e| e.to_string())
                },
                Message::ReactionDone,
            )
        }
        Message::ReactionDone(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Err(err) = result
            {
                session.status = Some(format!("Reaction failed: {err}"));
            }
            Task::none()
        }
        Message::Update(update) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            match update {
                MeshUpdate::RoomsChanged(ids) => {
                    let client = session.client.clone();
                    let mut tasks = vec![Task::perform(
                        async move { client.rooms().await },
                        Message::RoomsLoaded,
                    )];
                    if let Some(selected) = &session.selected_room
                        && ids.contains(selected)
                        && !is_invite(session, selected)
                    {
                        tasks.push(load_timeline(session.client.clone(), selected.clone()));
                        tasks.push(load_members(session.client.clone(), selected.clone()));
                    }
                    Task::batch(tasks)
                }
                MeshUpdate::VerificationRequested { user_id, flow_id } => {
                    session.verification = Some(VerificationPrompt {
                        user_id,
                        flow_id,
                        stage: VerifyStage::Requested,
                    });
                    Task::none()
                }
                MeshUpdate::VerificationEmojis { flow_id, emojis } => {
                    if let Some(prompt) = &mut session.verification
                        && prompt.flow_id == flow_id
                    {
                        prompt.stage = VerifyStage::Emojis(emojis);
                    }
                    Task::none()
                }
                MeshUpdate::VerificationDone { flow_id } => {
                    if session
                        .verification
                        .as_ref()
                        .is_some_and(|p| p.flow_id == flow_id)
                    {
                        session.verification = None;
                        session.status = Some("Device verified successfully.".to_string());
                    }
                    Task::none()
                }
                MeshUpdate::VerificationCancelled { flow_id, reason } => {
                    if session
                        .verification
                        .as_ref()
                        .is_some_and(|p| p.flow_id == flow_id)
                    {
                        session.verification = None;
                        session.status = Some(format!("Verification cancelled: {reason}"));
                    }
                    Task::none()
                }
                MeshUpdate::IncomingCall {
                    call_id,
                    room_id: _,
                    caller,
                } => {
                    // Don't clobber a call we're already in.
                    if session.call.is_none() {
                        session.call = Some(CallUi {
                            call_id,
                            peer: caller.localpart().to_owned(),
                            state: CallUiState::IncomingRinging,
                        });
                    }
                    Task::none()
                }
                MeshUpdate::CallConnecting { call_id } => {
                    if let Some(call) = &mut session.call
                        && (call.call_id == call_id || call.call_id.is_empty())
                    {
                        // An outgoing call learns its id here.
                        call.call_id = call_id;
                        call.state = CallUiState::Connecting;
                    }
                    // Carry the user's mic/deafen state into the new call.
                    let client = session.client.clone();
                    let (muted, deafened) = (session.self_muted, session.self_deafened);
                    Task::perform(
                        async move {
                            client.set_call_deafened(deafened).await;
                            client.set_call_muted(muted || deafened).await;
                            Ok(())
                        },
                        Message::CallActionFinished,
                    )
                }
                MeshUpdate::CallConnected { call_id } => {
                    if let Some(call) = &mut session.call
                        && call.call_id == call_id
                    {
                        call.state = CallUiState::Connected;
                    }
                    Task::none()
                }
                MeshUpdate::CallEnded { call_id, reason } => {
                    if session.call.as_ref().is_some_and(|c| c.call_id == call_id) {
                        session.call = None;
                        session.status = Some(format!("Call ended: {reason}"));
                    }
                    Task::none()
                }
                MeshUpdate::Typing { room_id, names } => {
                    session.typing.insert(room_id, names);
                    Task::none()
                }
            }
        }
        Message::DraftChanged(value) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.message_draft = value;
            }
            Task::none()
        }
        Message::SendMessage => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let Some(room_id) = session.selected_room.clone() else {
                return Task::none();
            };
            if session.message_draft.trim().is_empty() {
                return Task::none();
            }
            let client = session.client.clone();
            let body = std::mem::take(&mut session.message_draft);
            Task::perform(
                async move { client.send_message(&room_id, &body).await.map_err(|e| e.to_string()) },
                Message::MessageSent,
            )
        }
        Message::MessageSent(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.status = match result {
                    Ok(()) => None,
                    Err(err) => Some(format!("Failed to send: {err}")),
                };
            }
            Task::none()
        }
        Message::AcceptVerification => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let Some(prompt) = &mut session.verification else {
                return Task::none();
            };
            prompt.stage = VerifyStage::Accepted;
            let (client, user_id, flow_id) = (
                session.client.clone(),
                prompt.user_id.clone(),
                prompt.flow_id.clone(),
            );
            Task::perform(
                async move {
                    client
                        .accept_verification(&user_id, &flow_id)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::VerificationActionFinished,
            )
        }
        Message::ConfirmVerification => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let Some(prompt) = &mut session.verification else {
                return Task::none();
            };
            prompt.stage = VerifyStage::Confirmed;
            let (client, user_id, flow_id) = (
                session.client.clone(),
                prompt.user_id.clone(),
                prompt.flow_id.clone(),
            );
            Task::perform(
                async move {
                    client
                        .confirm_verification(&user_id, &flow_id)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::VerificationActionFinished,
            )
        }
        Message::CancelVerification => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let Some(prompt) = session.verification.take() else {
                return Task::none();
            };
            let client = session.client.clone();
            Task::perform(
                async move {
                    client
                        .cancel_verification(&prompt.user_id, &prompt.flow_id)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::VerificationActionFinished,
            )
        }
        Message::VerificationActionFinished(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Err(err) = result
            {
                session.verification = None;
                session.status = Some(format!("Verification failed: {err}"));
            }
            Task::none()
        }
        Message::PlaceCall => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            if session.call.is_some() {
                return Task::none();
            }
            let Some(room_id) = session.selected_room.clone() else {
                return Task::none();
            };
            let peer = session
                .rooms
                .iter()
                .find(|r| r.id == room_id)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "call".to_string());
            session.call = Some(CallUi {
                call_id: String::new(),
                peer,
                state: CallUiState::OutgoingRinging,
            });
            let client = session.client.clone();
            Task::perform(
                async move { client.place_call(&room_id).await },
                Message::CallActionFinished,
            )
        }
        Message::AcceptCall => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            if !session
                .call
                .as_ref()
                .is_some_and(|c| c.state == CallUiState::IncomingRinging)
            {
                return Task::none();
            }
            let client = session.client.clone();
            Task::perform(
                async move { client.accept_call().await },
                Message::CallActionFinished,
            )
        }
        Message::HangUp => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.call = None;
            let client = session.client.clone();
            Task::perform(
                async move { client.hangup().await },
                Message::CallActionFinished,
            )
        }
        Message::ToggleMute => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.self_muted = !session.self_muted;
            let muted = session.self_muted;
            let client = session.client.clone();
            Task::perform(
                async move {
                    client.set_call_muted(muted).await;
                    Ok(())
                },
                Message::CallActionFinished,
            )
        }
        Message::ToggleDeafen => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.self_deafened = !session.self_deafened;
            // Deafening also mutes the mic; undeafening unmutes it.
            session.self_muted = session.self_deafened;
            let (deafened, muted) = (session.self_deafened, session.self_muted);
            let client = session.client.clone();
            Task::perform(
                async move {
                    client.set_call_deafened(deafened).await;
                    client.set_call_muted(muted).await;
                    Ok(())
                },
                Message::CallActionFinished,
            )
        }
        Message::MembersLoaded(room_id, members) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && session.selected_room.as_ref() == Some(&room_id)
            {
                session.members = members;
            }
            Task::none()
        }
        Message::ProfileLoaded(profile) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.my_profile = Some(profile);
            }
            Task::none()
        }
        Message::Tick => {
            // Advance the background animation (~15 fps).
            state.rain_time += 0.066;
            Task::none()
        }
        Message::EnsureBackupFinished(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                match result {
                    Ok(Some(key)) => {
                        // Backup was just enabled; surface the key to save.
                        session.backup.status = Some(BackupStatus::Enabled);
                        session.backup.new_key = Some(key);
                        session.backup.message = Some(
                            "Secure Backup was enabled automatically. Open Settings to view and save your recovery key."
                                .to_string(),
                        );
                        session.status = Some(
                            "Secure Backup enabled — open Settings to save your recovery key."
                                .to_string(),
                        );
                    }
                    Ok(None) => {
                        session.backup.status = Some(session.client.backup_status());
                    }
                    Err(err) => tracing::warn!("auto backup enable failed: {err}"),
                }
            }
            Task::none()
        }
        Message::InputDeviceSelected(value) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.selected_input = value;
                apply_audio_devices(session);
            }
            Task::none()
        }
        Message::OutputDeviceSelected(value) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.selected_output = value;
                apply_audio_devices(session);
            }
            Task::none()
        }
        Message::CallActionFinished(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Err(err) = result
            {
                session.call = None;
                session.status = Some(format!("Call failed: {err}"));
            }
            Task::none()
        }
        Message::OpenDiscordVoice => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(url) = &session.discord_voice_url
                && let Err(err) = open_url(url)
            {
                session.status = Some(format!("Could not open {url}: {err}"));
            }
            Task::none()
        }
        Message::ToggleSettings => {
            match &mut state.screen {
                Screen::LoggedIn(session) if session.show_settings => {
                    session.show_settings = false;
                    Task::none()
                }
                Screen::LoggedIn(session) => {
                    let page = session.settings_page;
                    update(state, Message::OpenSettings(page))
                }
                _ => Task::none(),
            }
        }
        Message::OpenSettings(page) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.show_settings = true;
                session.settings_page = page;
                session.focused = Pane::Chat;
                {
                    session.backup.status = Some(session.client.backup_status());
                    // Refresh the audio device lists (default sentinel first).
                    session.input_devices = std::iter::once(DEFAULT_DEVICE.to_string())
                        .chain(session.client.audio_input_devices())
                        .collect();
                    session.output_devices = std::iter::once(DEFAULT_DEVICE.to_string())
                        .chain(session.client.audio_output_devices())
                        .collect();
                }
            }
            Task::none()
        }
        Message::SetupBackup => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.backup.busy = true;
            session.backup.message = Some("Setting up Secure Backup…".to_string());
            let client = session.client.clone();
            Task::perform(
                async move { client.enable_secure_backup().await },
                Message::BackupSetupFinished,
            )
        }
        Message::BackupSetupFinished(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.backup.busy = false;
                match result {
                    Ok(key) => {
                        session.backup.new_key = Some(key);
                        session.backup.message = Some(
                            "Secure Backup enabled. Save the recovery key below — it will not be shown again."
                                .to_string(),
                        );
                        session.backup.status = Some(session.client.backup_status());
                    }
                    Err(err) => session.backup.message = Some(format!("Setup failed: {err}")),
                }
            }
            Task::none()
        }
        Message::RecoveryKeyInput(value) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.backup.key_input = value;
            }
            Task::none()
        }
        Message::RestoreBackup => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let key = session.backup.key_input.trim().to_string();
            if key.is_empty() {
                return Task::none();
            }
            session.backup.busy = true;
            session.backup.message = Some("Restoring from backup…".to_string());
            let client = session.client.clone();
            Task::perform(
                async move { client.restore_secure_backup(&key).await },
                Message::BackupRestoreFinished,
            )
        }
        Message::BackupRestoreFinished(result) => {
            let mut reload = Task::none();
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.backup.busy = false;
                match result {
                    Ok(()) => {
                        session.backup.key_input.clear();
                        session.backup.message = Some(
                            "Restored from backup. Previously unreadable messages should decrypt shortly."
                                .to_string(),
                        );
                        session.backup.status = Some(session.client.backup_status());
                        // Reload the open room so newly-decryptable messages appear.
                        if let Some(room_id) = session.selected_room.clone() {
                            reload = load_timeline(session.client.clone(), room_id);
                        }
                    }
                    Err(err) => session.backup.message = Some(format!("Restore failed: {err}")),
                }
            }
            reload
        }
        Message::ThemeChosen(choice) => {
            if let Err(err) = theme::save_choice(&config_path(), &choice) {
                tracing::warn!("failed to persist theme choice: {err}");
            }
            state.theme_choice = choice;
            reload_theme(state);
            Task::none()
        }
        Message::FocusPane(pane) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.focused = pane;
            }
            Task::none()
        }
        Message::ToggleMembers => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.show_members = !session.show_members;
            }
            Task::none()
        }
        Message::Escape => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                if session.add_friend.is_some() {
                    session.add_friend = None;
                } else if session.jump.is_some() {
                    session.jump = None;
                } else {
                    session.show_settings = false;
                }
            }
            Task::none()
        }
        Message::SpaceSelected(space) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.active_space = space;
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::SpaceKey(n) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(group) = session.groups.get(n)
            {
                session.active_space = group.id().cloned();
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::OpenJump => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.jump = Some(String::new());
                return iced::widget::operation::focus(iced::widget::Id::new(JUMP_INPUT));
            }
            Task::none()
        }
        Message::JumpInput(query) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.jump = Some(query);
            }
            Task::none()
        }
        Message::JumpSubmit => {
            let Screen::LoggedIn(session) = &state.screen else {
                return Task::none();
            };
            match jump_matches(session).first() {
                Some(room) => {
                    let id = room.id.clone();
                    update(state, Message::JumpTo(id))
                }
                None => Task::none(),
            }
        }
        Message::JumpTo(room_id) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.jump = None;
            if session.groups.iter().any(|g| g.id() == Some(&room_id)) {
                session.active_space = Some(room_id);
                session.focused = Pane::Channels;
                return Task::none();
            }
            // Stay in the current space if the room is in it; otherwise the first one listing it.
            let in_group = |g: &SpaceGroup| g.rooms.iter().any(|r| r.id == room_id);
            if !session
                .groups
                .iter()
                .any(|g| g.id() == session.active_space.as_ref() && in_group(g))
                && let Some(group) = session.groups.iter().find(|g| in_group(g))
            {
                session.active_space = group.id().cloned();
            }
            update(state, Message::RoomSelected(room_id))
        }
        Message::DensityChosen(density) => {
            state.density = density;
            persist(|c| c.density = Some(density));
            Task::none()
        }
        Message::ToggleExtra(extra) => {
            let x = &mut state.extras;
            let flag = match extra {
                Extra::Rain => &mut x.digital_rain,
                Extra::Translucent => &mut x.translucent_panes,
                Extra::Tags => &mut x.show_network_tags,
                Extra::PlainText => &mut x.plain_text_mode,
            };
            *flag = !*flag;
            let extras = *x;
            persist(|c| c.extras = extras);
            // Turning plain-text off: fetch the images it skipped.
            match &mut state.screen {
                Screen::LoggedIn(session) if !extras.plain_text_mode => load_images(session),
                _ => Task::none(),
            }
        }
        Message::ExportTheme => {
            let toml = theme::export_toml(
                &state.theme_choice,
                &state.palette,
                &state.layout,
                state.layout_from_hyprland,
                state.density,
                &state.extras,
            );
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.status = Some("theme.toml copied to the clipboard.".to_string());
            }
            iced::clipboard::write(toml)
        }
        Message::OpenAddFriend => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.jump = None;
            session.add_friend = Some(AddFriend::default());
            Task::batch([
                load_pending(session.client.clone()),
                iced::widget::operation::focus(iced::widget::Id::new(FRIEND_INPUT)),
            ])
        }
        Message::FriendMode(mode) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(dialog) = &mut session.add_friend
            {
                dialog.mode = mode;
            }
            Task::none()
        }
        Message::FriendQuery(query) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let server = my_server(session);
            let Some(dialog) = &mut session.add_friend else {
                return Task::none();
            };
            dialog.query = query;
            dialog.note = None;
            // Look up the profile once the input is a complete Matrix ID.
            let Detected::Matrix { id, .. } = detect_friend(&dialog.query, dialog.mode, &server) else {
                return Task::none();
            };
            if dialog.profile.as_ref().is_some_and(|(looked_up, _)| *looked_up == id) {
                return Task::none();
            }
            let Ok(user_id) = OwnedUserId::try_from(id.as_str()) else {
                return Task::none();
            };
            let client = session.client.clone();
            Task::perform(
                async move { (id, client.display_name_of(&user_id).await) },
                |(id, name)| Message::FriendProfile(id, name),
            )
        }
        Message::FriendProfile(id, name) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(dialog) = &mut session.add_friend
            {
                dialog.profile = Some((id, name));
            }
            Task::none()
        }
        Message::FriendSend => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let server = my_server(session);
            let Some(dialog) = &mut session.add_friend else {
                return Task::none();
            };
            // Only Matrix can be sent; Discord requests aren't possible through the bridge.
            let Detected::Matrix { id, .. } = detect_friend(&dialog.query, dialog.mode, &server) else {
                return Task::none();
            };
            let Ok(user_id) = OwnedUserId::try_from(id.as_str()) else {
                return Task::none();
            };
            if dialog.busy {
                return Task::none();
            }
            dialog.busy = true;
            let client = session.client.clone();
            Task::perform(
                async move { client.start_dm(&user_id).await.map_err(|e| e.to_string()) },
                Message::FriendSent,
            )
        }
        Message::FriendSent(result) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let Some(dialog) = &mut session.add_friend else {
                return Task::none();
            };
            dialog.busy = false;
            match result {
                // Straight into the new (or existing) DM.
                Ok(room_id) => {
                    session.add_friend = None;
                    session.active_space = None;
                    let client = session.client.clone();
                    Task::batch([
                        Task::perform(async move { client.rooms().await }, Message::RoomsLoaded),
                        update(state, Message::RoomSelected(room_id)),
                    ])
                }
                Err(err) => {
                    dialog.note = Some((format!("Couldn't send: {err}"), true));
                    Task::none()
                }
            }
        }
        Message::PendingLoaded(pending) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(dialog) = &mut session.add_friend
            {
                dialog.pending = pending;
            }
            Task::none()
        }
        Message::PendingAccept(room_id) => pending_action(state, move |client| async move {
            client.accept_invite(&room_id).await
        }),
        Message::PendingIgnore(room_id) => pending_action(state, move |client| async move {
            client.leave_room(&room_id).await
        }),
        Message::PendingCancel(room_id, user_id) => pending_action(state, move |client| async move {
            client.cancel_invite(&room_id, &user_id).await
        }),
        Message::PendingDone(result) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            match &mut session.add_friend {
                Some(dialog) => {
                    dialog.note = result.err().map(|err| (format!("That didn't work: {err}"), true));
                }
                None => {
                    session.status = result.err().map(|err| format!("That didn't work: {err}"));
                }
            }
            let client = session.client.clone();
            Task::batch([
                load_pending(session.client.clone()),
                Task::perform(async move { client.rooms().await }, Message::RoomsLoaded),
            ])
        }
        Message::DiscordWarningSeen => {
            state.discord_warning_seen = true;
            persist(|c| c.discord_warning_seen = true);
            Task::none()
        }
        Message::ThemeCheck => {
            if theme::stamps(&state.theme_files) != state.theme_stamps {
                reload_theme(state);
            }
            Task::none()
        }
        Message::LogOut => {
            let task = match &state.screen {
                Screen::LoggedIn(session) => {
                    let client = session.client.clone();
                    Task::future(async move {
                        if let Err(err) = client.logout().await {
                            tracing::warn!("logout: {err}");
                        }
                    })
                    .discard()
                }
                _ => Task::none(),
            };
            state.screen = Screen::LoggedOut(LoginForm::default());
            task
        }
    }
}

/// Runs an accept / ignore / cancel on a pending contact, then reloads the list.
fn pending_action<F, Fut>(state: &App, action: F) -> Task<Message>
where
    F: FnOnce(Arc<MeshClient>) -> Fut,
    Fut: std::future::Future<Output = Result<(), mesh_core::MeshError>> + Send + 'static,
{
    let Screen::LoggedIn(session) = &state.screen else {
        return Task::none();
    };
    let job = action(session.client.clone());
    Task::perform(async move { job.await.map_err(|e| e.to_string()) }, Message::PendingDone)
}

/// Saves a config change, logging (not surfacing) a failure: the setting
/// still applies for this run.
fn persist(change: impl FnOnce(&mut theme::Config)) {
    if let Err(err) = theme::update_config(&config_path(), change) {
        tracing::warn!("failed to save config: {err}");
    }
}

/// Re-reads the palette and layout from the current theme choice's files.
fn reload_theme(state: &mut App) {
    state.palette = theme::palette_for_choice(&state.theme_choice, &theme_path());
    state.iced_theme = state.palette.to_iced_theme();
    state.layout = theme::layout_for_config(&config_path());
    state.layout_from_hyprland = theme::load_config(&config_path())
        .layout_from_hyprland
        .unwrap_or(true);
    state.theme_reloaded = Local::now().format("%H:%M").to_string();
    state.theme_files = theme::watched_files(&state.theme_choice, &theme_path());
    state.theme_stamps = theme::stamps(&state.theme_files);
}

fn theme(state: &App) -> IcedTheme {
    state.iced_theme.clone()
}

fn subscription(state: &App) -> Subscription<Message> {
    Subscription::batch([
        // Drive the animated background at ~15 fps (only while it's on).
        if state.extras.digital_rain {
            iced::time::every(std::time::Duration::from_millis(66)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        },
        // Hot-reload the rice: an mtime poll is cheap and works everywhere.
        iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::ThemeCheck),
        // Global shortcuts work even while an input has focus.
        iced::event::listen_with(|event, _status, _window| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                shortcut(key, modifiers)
            }
            _ => None,
        }),
    ])
}

fn shortcut(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    match key.as_ref() {
        keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Message::Escape),
        keyboard::Key::Character("k") if modifiers.control() => Some(Message::OpenJump),
        keyboard::Key::Character(c) if modifiers.alt() => match c.parse::<usize>() {
            Ok(n @ 1..=9) => Some(Message::SpaceKey(n)),
            _ => None,
        },
        _ => None,
    }
}

/// Rooms and spaces whose name contains the jump query, most recent first.
fn jump_matches(session: &Session) -> Vec<&RoomSummary> {
    let query = session.jump.as_deref().unwrap_or_default().to_lowercase();
    session
        .rooms
        .iter()
        .filter(|r| r.name.to_lowercase().contains(&query))
        .take(JUMP_RESULTS)
        .collect()
}

fn view(state: &App) -> Element<'_, Message> {
    let content = match &state.screen {
        Screen::Booting => center_text("Restoring session..."),
        Screen::LoggedOut(form) => login_view(form, false, state),
        Screen::LoggingIn(form) => login_view(form, true, state),
        Screen::LoggedIn(session) => session_view(session, state),
    };

    // The animated digital-rain background sits behind every screen.
    if state.extras.digital_rain {
        stack![rain::background(&state.palette, state.rain_time), content].into()
    } else {
        content
    }
}

fn center_text(label: &str) -> Element<'_, Message> {
    container(text(label.to_string()))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn login_view<'a>(form: &'a LoginForm, busy: bool, state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let mut form_column = column![
        row![
            ui::mono_bold(">_", 28.0, p.accent()),
            ui::mono_bold("mesh", 28.0, p.text_bright()),
        ]
        .spacing(10),
        ui::mono("a lightweight matrix client", 12.0, p.text_muted()),
        Space::new().height(12),
        text_input("Homeserver (e.g. https://matrix.org)", &form.homeserver)
            .on_input(Message::HomeserverChanged)
            .padding(10),
        text_input("Username", &form.username)
            .on_input(Message::UsernameChanged)
            .padding(10),
        text_input("Password", &form.password)
            .on_input(Message::PasswordChanged)
            .secure(true)
            .on_submit(Message::LoginSubmitted)
            .padding(10),
    ]
    .spacing(12);

    if let Some(error) = &form.error {
        form_column = form_column.push(text(error.clone()).size(13).color(p.danger()));
    }

    let (label, msg) = if busy {
        ("Logging in…", None)
    } else {
        ("Log in", Some(Message::LoginSubmitted))
    };
    form_column = form_column.push(
        ui::primary_button(ui::sans(label, 14.0, Weight::Semibold, p.accent_text()), msg, p, l)
            .padding(10),
    );

    let translucent = state.extras.translucent_panes;
    container(ui::pane(form_column.padding(28), true, p, l, translucent).max_width(420))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn session_view<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let (p, l, x) = (&state.palette, &state.layout, &state.extras);
    // `responsive` gives us the window size, so side panes can drop out on
    // narrow windows and leave the chat usable.
    responsive(move |size| {
        let wide = size.width > 1100.0;
        let panes: Element<'_, Message> = if session.show_settings {
            settings_shell(session, state, wide)
        } else {
            let rail = rail_view(session, p, l, x.show_network_tags);
            let mut panes = row![
                tile(session, Pane::Rail, rail, 64.0 + 2.0 * l.border_size, p, l, x),
                tile(session, Pane::Channels, channel_pane(session, p, l), 248.0, p, l, x),
                tile(session, Pane::Chat, chat_pane(session, state), 0.0, p, l, x),
            ]
            .spacing(l.gaps_out)
            .height(Length::Fill);
            if session.show_members && session.selected_room.is_some() && wide {
                let members = member_pane(session, p, x.show_network_tags);
                panes = panes.push(tile(session, Pane::Members, members, 236.0, p, l, x));
            }
            panes.into()
        };
        let shell = column![top_bar(session, p, l, x.translucent_panes), panes]
            .spacing(l.gaps_out)
            .padding(l.gaps_out);
        if let Some(dialog) = &session.add_friend {
            stack![shell, add_friend_view(session, dialog, p, l)].into()
        } else if session.jump.is_some() {
            stack![shell, jump_palette(session, p, l)].into()
        } else {
            shell.into()
        }
    })
    .into()
}

/// The Ctrl+K palette: type to filter rooms and spaces, Enter or click to jump.
fn jump_palette<'a>(session: &'a Session, p: &theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let query = session.jump.as_deref().unwrap_or_default();
    let mut list = column![
        text_input("jump to a room or space…", query)
            .id(iced::widget::Id::new(JUMP_INPUT))
            .on_input(Message::JumpInput)
            .on_submit(Message::JumpSubmit)
            .padding(10)
            .size(14),
    ]
    .spacing(2);
    let matches = jump_matches(session);
    for (i, room) in matches.iter().enumerate() {
        let (icon, label) = if room.is_space {
            ("◆", room.name.clone())
        } else {
            (if room.is_direct { "@" } else { "#" }, room.name.clone())
        };
        let mut line = row![
            ui::mono(icon, 13.0, p.text_muted()),
            ui::sans(label, 14.0, Weight::Normal, p.text_bright()).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if i == 0 {
            line = line.push(ui::mono("Enter", 11.0, p.text_muted()));
        }
        list = list.push(ui::row_button(line, Message::JumpTo(room.id.clone()), i == 0, p, l));
    }
    if matches.is_empty() {
        list = list.push(container(ui::mono("no match", 12.0, p.text_muted())).padding(8));
    }
    let scrim = theme::alpha(p.background(), 0.6);
    mouse_area(
        container(iced::widget::opaque(
            ui::pane(list.padding(10), true, p, l, false).max_width(560),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .padding(iced::Padding { top: 96.0, ..iced::Padding::ZERO })
        .style(move |_| container::Style {
            background: Some(scrim.into()),
            ..container::Style::default()
        }),
    )
    .on_press(Message::Escape)
    .into()
}

fn net_color(network: Network, p: &theme::Palette) -> iced::Color {
    match network {
        Network::Matrix => p.proto_mx(),
        Network::Discord => p.proto_dc(),
    }
}

/// Your homeserver's name, e.g. `strange.lab`.
fn my_server(session: &Session) -> String {
    session
        .my_profile
        .as_ref()
        .map(|me| me.user_id.server_name().to_string())
        .unwrap_or_default()
}

fn is_invite(session: &Session, room_id: &OwnedRoomId) -> bool {
    session.rooms.iter().any(|r| &r.id == room_id && r.is_invite)
}

/// "You're invited": accept or decline, in place of a room's content.
fn invite_card<'a>(room: &RoomSummary, p: &'a theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let (fill, edge) = (p.panel_dim(), p.accent());
    let what = if room.is_space { "space" } else if room.is_direct { "DM" } else { "room" };
    container(
        column![
            ui::sans(format!("You're invited to {}", room.name), 15.0, Weight::Semibold, p.text_bright()),
            text(format!("Accept to join this {what} and read it, or decline to drop the invite."))
                .size(13)
                .color(p.text_muted()),
            row![
                ui::primary_button(
                    ui::sans("Accept", 13.0, Weight::Semibold, p.accent_text()),
                    Some(Message::PendingAccept(room.id.clone())),
                    p,
                    l,
                ),
                button(text("Decline").size(13))
                    .on_press(Message::PendingIgnore(room.id.clone()))
                    .padding([6, 12])
                    .style(flat_button_style(p)),
            ]
            .spacing(8),
        ]
        .spacing(8),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(move |_| container::Style {
        border: iced::border::rounded(8).color(edge).width(1),
        background: Some(fill.into()),
        ..container::Style::default()
    })
    .into()
}

fn active_group(session: &Session) -> Option<&SpaceGroup> {
    session
        .groups
        .iter()
        .find(|g| g.id() == session.active_space.as_ref())
}

/// Two-letter tile label: initials of the first two words, or the first two letters.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let letters: String = if words.len() >= 2 {
        words.iter().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).collect()
    } else {
        name.chars().filter(|c| c.is_alphanumeric()).take(2).collect()
    };
    letters.to_uppercase()
}

/// One focusable pane of the tiled shell; `width` 0 fills the rest.
fn tile<'a>(
    session: &Session,
    pane: Pane,
    content: Element<'a, Message>,
    width: f32,
    p: &theme::Palette,
    l: &theme::Layout,
    x: &theme::Extras,
) -> Element<'a, Message> {
    let width = if width > 0.0 { Length::Fixed(width) } else { Length::Fill };
    mouse_area(
        ui::pane(content, session.focused == pane, p, l, x.translucent_panes)
            .width(width)
            .height(Length::Fill),
    )
    .on_press(Message::FocusPane(pane))
    .into()
}

fn selected_room(session: &Session) -> Option<&RoomSummary> {
    session
        .selected_room
        .as_ref()
        .and_then(|id| session.rooms.iter().find(|r| &r.id == id))
}

/// How a room is named in headers: `#name` for rooms, plain for DMs.
fn room_label(room: &RoomSummary) -> String {
    if room.is_direct {
        room.name.clone()
    } else {
        format!("#{}", room.name)
    }
}

fn top_bar<'a>(
    session: &'a Session,
    p: &theme::Palette,
    l: &theme::Layout,
    translucent: bool,
) -> Element<'a, Message> {
    let wordmark = row![
        ui::mono_bold(">_", 14.0, p.accent()),
        ui::mono_bold("mesh", 14.0, p.text_bright()),
    ]
    .spacing(8);

    let mut crumb = row![].spacing(6).align_y(iced::Alignment::Center);
    if let Some(index) = session.groups.iter().position(|g| g.id() == session.active_space.as_ref())
        && index > 0
    {
        crumb = crumb.push(ui::chip(index.to_string(), p));
    }
    crumb = crumb.push(ui::mono(
        active_group(session).map_or("Home", |g| g.name()).to_lowercase(),
        12.0,
        p.text_muted(),
    ));
    if let Some(room) = selected_room(session) {
        crumb = crumb
            .push(ui::mono("/", 12.0, p.text_muted()))
            .push(ui::mono(room_label(room), 12.0, p.text_bright()));
    }

    let mut chips = row![].spacing(6).align_y(iced::Alignment::Center);
    if let Some(profile) = &session.my_profile {
        chips = chips.push(connection_chip(
            "mx",
            p.proto_mx(),
            profile.user_id.server_name().to_string(),
            p,
        ));
    }
    if session.rooms.iter().any(|r| r.network == Network::Discord) {
        chips = chips.push(connection_chip("dc", p.proto_dc(), "bridge".to_string(), p));
    }

    let (fill, edge, radius) = (p.background(), p.border_ctrl(), l.control_radius());
    let search = button(
        row![
            ui::mono("search or jump — rooms, spaces", 12.0, p.text_muted()).width(Length::Fill),
            ui::outline_chip(ui::mono("Ctrl K", 11.0, p.text_muted()), p),
        ]
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::OpenJump)
    .padding([4, 10])
    .width(Length::Fill)
    .style(move |_, _| button::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(radius).color(edge).width(1),
        ..button::Style::default()
    });

    ui::pane(
        row![
            wordmark,
            crumb,
            container(container(search).max_width(460)).width(Length::Fill).center_x(Length::Fill),
            chips,
            ui::primary_button(
                ui::sans("+ add friend", 13.0, Weight::Semibold, p.accent_text()),
                Some(Message::OpenAddFriend),
                p,
                l,
            ),
        ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
            .padding([6, 10]),
        false,
        p,
        l,
        translucent,
    )
    .width(Length::Fill)
    .into()
}

/// `● mx strange.lab`: a status dot, the network tag and a name.
fn connection_chip<'a>(
    proto: &'a str,
    color: iced::Color,
    name: String,
    p: &theme::Palette,
) -> Element<'a, Message> {
    ui::outline_chip(
        row![
            ui::swatch(p.success(), 7.0, 7.0, 3.5),
            ui::mono_bold(proto, 12.0, color),
            ui::mono(name, 12.0, p.text()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
        p,
    )
}

/// The space rail: Home, then (later) one tile per space, then "+".
fn rail_view<'a>(
    session: &'a Session,
    p: &theme::Palette,
    l: &theme::Layout,
    tags: bool,
) -> Element<'a, Message> {
    let home = rail_tile(
        text("⌂").size(20).color(p.text_bright()).into(),
        session.active_space.is_none(),
        Some(Message::SpaceSelected(None)),
        p,
        l,
    );
    let mut tiles = column![].spacing(8).align_x(iced::Alignment::Center);
    for (index, group) in session.groups.iter().enumerate().skip(1) {
        let active = group.id() == session.active_space.as_ref();
        let fg = if active { p.text_bright() } else { p.text() };
        let mut tile: Element<'_, Message> = rail_tile(
            ui::mono_bold(initials(group.name()), 13.0, fg).into(),
            active,
            Some(Message::SpaceSelected(group.id().cloned())),
            p,
            l,
        );
        let mentions = group.mentions();
        if mentions > 0 {
            let (fill, ring) = (p.danger(), p.surface());
            let badge = container(ui::mono_bold(mentions.to_string(), 10.0, p.background()))
                .padding([1, 5])
                .style(move |_| container::Style {
                    background: Some(fill.into()),
                    border: iced::border::rounded(9).color(ring).width(2),
                    ..container::Style::default()
                });
            tile = stack![
                tile,
                container(badge)
                    .width(52)
                    .height(50)
                    .align_x(iced::Alignment::End)
                    .align_y(iced::Alignment::End),
            ]
            .into();
        }
        let tag = group.network().tag();
        let key = match (index <= 9, tags) {
            (true, true) => format!("{index}·{tag}"),
            (true, false) => index.to_string(),
            (false, true) => tag.to_string(),
            (false, false) => String::new(),
        };
        tiles = tiles.push(
            iced::widget::tooltip(
                column![
                    tile,
                    ui::mono(key, 10.0, if active { p.text_bright() } else { p.text_muted() }),
                ]
                .spacing(2)
                .align_x(iced::Alignment::Center),
                container(text(group.name().to_string()).size(12).color(p.text())).padding([4, 8]),
                iced::widget::tooltip::Position::Right,
            ),
        );
    }
    let add = ui::not_yet(
        rail_tile(text("+").size(18).color(p.text_muted()).into(), false, None, p, l),
        "Joining or creating a space isn't supported yet.",
        p,
    );
    scrollable(
        column![home, ui::swatch(p.border(), 28.0, 2.0, 1.0), tiles, add]
            .spacing(8)
            .padding([10, 0])
            .width(Length::Fill)
            .align_x(iced::Alignment::Center),
    )
    .height(Length::Fill)
    .into()
}

/// A 44×44 rail tile; the active one gets a `surface_alt` fill and an accent ring.
fn rail_tile<'a>(
    content: Element<'a, Message>,
    active: bool,
    msg: Option<Message>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let fill = if active {
        p.surface_alt()
    } else {
        theme::mix(p.surface(), p.surface_alt(), 0.4)
    };
    let ring = if active { p.accent() } else { p.border_ctrl() };
    let radius = l.rounding;
    button(
        container(content)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .on_press_maybe(msg)
    .width(44)
    .height(44)
    .padding(0)
    .style(move |_, _| button::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(radius)
            .color(ring)
            .width(if active { 2 } else { 1 }),
        ..button::Style::default()
    })
    .into()
}

fn channel_pane<'a>(session: &'a Session, p: &'a theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let group = active_group(session);
    let mut title = row![
        ui::sans(group.map_or("Home", |g| g.name()), 15.0, Weight::Semibold, p.text_bright())
            .width(Length::Fill),
    ]
    .align_y(iced::Alignment::Center);
    if session.discord_voice_url.is_some() {
        title = title.push(
            iced::widget::tooltip(
                ui::icon_button("🎧", Some(Message::OpenDiscordVoice), false, p, l),
                container(text("Open Discord voice").size(12).color(p.text())).padding([4, 8]),
                iced::widget::tooltip::Position::Bottom,
            ),
        );
    }
    let subtitle = match group.filter(|g| g.space.is_some()) {
        Some(g) => format!("{} channels", g.rooms.len()),
        None => "friends & direct messages".to_string(),
    };
    let mut header = column![title, ui::mono(subtitle, 11.0, p.text_muted())].spacing(4);
    if group.is_some_and(|g| g.network() == Network::Discord) {
        header = header.push(
            row![
                ui::mono("dc", 11.0, p.proto_dc()),
                ui::mono(format!("bridged via {}", my_server(session)), 11.0, p.text_muted()),
            ]
            .spacing(6),
        );
    }
    let header = header
        .padding(iced::Padding { top: 14.0, right: 14.0, bottom: 10.0, left: 14.0 });

    let mut list = column![].spacing(2).padding([10, 8]);
    let rooms = group.map(|g| g.rooms.as_slice()).unwrap_or_default();
    let sections: [(&str, Vec<&RoomSummary>); 2] = if group.is_some_and(|g| g.space.is_some()) {
        [("Text", rooms.iter().collect()), ("", Vec::new())]
    } else {
        let (directs, others) = rooms.iter().partition(|r| r.is_direct);
        [("Direct messages", directs), ("Rooms", others)]
    };
    for (label, rooms) in sections {
        if rooms.is_empty() {
            continue;
        }
        list = list.push(ui::section(label, p));
        for room in rooms {
            let selected = session.selected_room.as_ref() == Some(&room.id);
            list = list.push(channel_row(room, selected, p, l));
        }
    }

    if let Some(space) = group.and_then(|g| g.space.as_ref()).filter(|s| s.is_invite) {
        list = column![invite_card(space, p, l)].padding([10, 8]).push(list);
    }
    column![
        header,
        ui::hline(p),
        scrollable(list).height(Length::Fill),
        ui::hline(p),
        user_panel(session, p, l),
    ]
    .into()
}

/// A channel row. Active: `surface_alt` fill and an accent bar; unread:
/// bright, semibold, with a count chip.
fn channel_row<'a>(
    room: &'a RoomSummary,
    selected: bool,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let unread = room.unread_notifications > 0;
    let color = if selected || unread { p.text_bright() } else { p.text() };
    let bar = if selected { p.accent() } else { iced::Color::TRANSPARENT };
    let mut label = row![
        ui::swatch(bar, 2.0, 18.0, 1.0),
        ui::mono(if room.is_direct { "@" } else { "#" }, 13.0, p.text_muted()),
        ui::sans(&room.name, 14.0, if unread { Weight::Semibold } else { Weight::Normal }, color)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    if room.is_invite {
        label = label.push(ui::mono("invite", 10.0, p.accent()));
    } else if unread {
        label = label.push(ui::chip(room.unread_notifications.to_string(), p));
    }
    ui::row_button(label, Message::RoomSelected(room.id.clone()), selected, p, l)
        .padding(iced::Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 0.0 })
        .into()
}

/// The user panel pinned under the channel list: you, mic, deafen, settings
/// (and hang up while in a call).
fn user_panel<'a>(session: &'a Session, p: &theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let (name, mxid) = session
        .my_profile
        .as_ref()
        .map(|me| (me.display_name.clone(), me.user_id.to_string()))
        .unwrap_or_else(|| ("…".to_string(), String::new()));
    let sub = match &session.call {
        Some(call) => ui::mono(call.state.label(), 10.0, p.accent()),
        None => ui::mono(mxid, 10.0, p.text_muted()),
    };

    let mut controls = row![
        ui::icon_button(
            if session.self_muted { "🔇" } else { "🎙" },
            Some(Message::ToggleMute),
            session.self_muted,
            p,
            l,
        ),
        ui::icon_button("🎧", Some(Message::ToggleDeafen), session.self_deafened, p, l),
    ]
    .spacing(2);
    if session.call.is_some() {
        controls = controls.push(ui::icon_button("📵", Some(Message::HangUp), true, p, l));
    }
    controls = controls.push(ui::icon_button(
        "⚙",
        Some(Message::ToggleSettings),
        session.show_settings,
        p,
        l,
    ));

    let fill = p.panel_dim();
    let radius = (l.rounding - l.border_size).max(0.0);
    container(
        row![
            ui::avatar(&name, 34.0, p.accent(), p, Some((p.success(), fill))),
            // Clipped so a long MXID can't run under the buttons.
            container(
                column![ui::sans(name, 13.0, Weight::Semibold, p.text_bright()), sub.wrapping(text::Wrapping::None)]
                    .spacing(1),
            )
            .width(Length::Fill)
            .clip(true),
            controls,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .padding(10)
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(iced::border::bottom(radius)),
        ..container::Style::default()
    })
    .into()
}

fn chat_pane<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let room = selected_room(session);
    let mut col = column![];
    if let Some(room) = room {
        let topic = room
            .topic
            .as_deref()
            .and_then(|t| t.lines().next())
            .unwrap_or_default();
        let mut header = row![
            ui::mono(if room.is_direct { "@" } else { "#" }, 18.0, p.text_muted()),
            ui::sans(&room.name, 15.0, Weight::Semibold, p.text_bright()),
            text(topic).size(13).color(p.text_muted()).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if session.call.is_none() {
            header = header.push(ui::icon_button("📞", Some(Message::PlaceCall), false, p, l));
        }
        let members_color = if session.show_members { p.text_bright() } else { p.text_muted() };
        header = header.push(
            button(text("👥").size(15).color(members_color))
                .on_press(Message::ToggleMembers)
                .padding(6)
                .style(flat_button_style(p)),
        );
        col = col.push(container(header).padding([10, 16])).push(ui::hline(p));
    }

    if let Some(call) = &session.call {
        col = col.push(container(call_bar(call, p)).padding([8, 16]));
    }
    if let Some(prompt) = &session.verification {
        col = col.push(container(verification_banner(prompt, p)).padding([8, 16]));
    }

    let Some(room) = room else {
        return col
            .push(
                container(
                    text("Pick a room from the sidebar to start chatting.")
                        .size(14)
                        .color(p.text_muted()),
                )
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .into();
    };

    if room.is_invite {
        return col
            .push(container(invite_card(room, p, l)).padding(16).height(Length::Fill))
            .into();
    }

    let me = session.my_profile.as_ref().map(|me| &me.user_id);
    col = col.push(
        scrollable(
            container(timeline_view(session, me, state.density, &state.extras, p))
                .padding(iced::Padding { top: 16.0, right: 16.0, bottom: 8.0, left: 16.0 }),
        )
        .height(Length::Fill)
        .anchor_bottom(),
    );
    if let Some(status) = &session.status {
        col = col.push(container(text(status.clone()).size(13).color(p.danger())).padding([0, 16]));
    }
    col.push(composer(session, room, p, l)).into()
}

fn composer<'a>(
    session: &'a Session,
    room: &RoomSummary,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let (muted, bright, selection) = (p.text_muted(), p.text_bright(), theme::alpha(p.accent(), 0.35));
    let placeholder = match room.network {
        Network::Discord => format!("message {}  —  sends to discord via bridge", room_label(room)),
        Network::Matrix => format!("message {}", room_label(room)),
    };
    let input = text_input(&placeholder, &session.message_draft)
        .on_input(Message::DraftChanged)
        .on_submit(Message::SendMessage)
        .padding(0)
        .size(14)
        .style(move |_, _| text_input::Style {
            background: iced::Color::TRANSPARENT.into(),
            border: iced::Border::default(),
            icon: muted,
            placeholder: muted,
            value: bright,
            selection,
        });
    let field = row![
        ui::not_yet(
            ui::icon_button("📎", None, false, p, l),
            "Attachments aren't supported yet.",
            p,
        ),
        input,
        ui::primary_button(text("➤").size(14), Some(Message::SendMessage), p, l),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let (fill, edge, radius) = (p.background(), p.border_ctrl(), l.control_radius() + 2.0);
    column![
        container(field)
            .padding([6, 8])
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(radius).color(edge).width(1),
                ..container::Style::default()
            }),
        row![
            typing_line(session.typing.get(&room.id).map(Vec::as_slice).unwrap_or_default(), p),
            Space::new().width(Length::Fill),
            ui::mono("Enter send · markdown on", 11.0, p.text_muted()),
        ],
    ]
    .spacing(6)
    .padding(iced::Padding { top: 8.0, right: 16.0, bottom: 14.0, left: 16.0 })
    .into()
}

fn member_pane<'a>(session: &'a Session, p: &theme::Palette, tags: bool) -> Element<'a, Message> {
    let mut list = column![ui::section(&format!("members — {}", session.members.len()), p)].spacing(2);
    for member in &session.members {
        let net = net_color(member.network, p);
        let fill = if member.is_me { p.accent() } else { net };
        list = list.push(
            container(
                row![
                    ui::avatar(&member.display_name, 32.0, fill, p, None),
                    ui::sans(&member.display_name, 13.0, Weight::Medium, p.text_bright())
                        .width(Length::Fill),
                    ui::mono(if tags { member.network.tag() } else { "" }, 10.0, net),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            )
            .padding(6),
        );
    }
    scrollable(container(list).padding([12, 10]))
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

fn call_bar<'a>(call: &'a CallUi, palette: &'a theme::Palette) -> Element<'a, Message> {
    let mut controls = row![].spacing(8).align_y(iced::Alignment::Center);
    match call.state {
        CallUiState::IncomingRinging => {
            controls = controls
                .push(button(text("Accept").size(13)).on_press(Message::AcceptCall).padding(8));
            controls = controls
                .push(button(text("Decline").size(13)).on_press(Message::HangUp).padding(8));
        }
        _ => {
            controls = controls
                .push(button(text("Hang up").size(13)).on_press(Message::HangUp).padding(8));
        }
    }

    let title = format!("📞  {}  —  {}", call.peer, call.state.label());
    let surface = palette.surface_alt();
    let border_color = palette.border();
    container(
        row![
            text(title).size(15).color(palette.text_bright()).width(Length::Fill),
            controls,
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding(12)
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(surface.into()),
        border: iced::border::rounded(8).color(border_color).width(1),
        ..container::Style::default()
    })
    .into()
}

/// Opens `url` in the default browser.
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ())
}

/// A borderless button that just shows text in the palette's text color.
fn flat_button_style(
    palette: &theme::Palette,
) -> impl Fn(&IcedTheme, button::Status) -> button::Style {
    let text_color = palette.text();
    let surface_alt = palette.surface_alt();
    move |_theme, status| {
        let background = if matches!(status, button::Status::Hovered) {
            Some(surface_alt.into())
        } else {
            None
        };
        button::Style {
            background,
            text_color,
            border: iced::border::rounded(6),
            ..button::Style::default()
        }
    }
}

fn timeline_view<'a>(
    session: &'a Session,
    me: Option<&OwnedUserId>,
    density: theme::Density,
    x: &theme::Extras,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let tags = x.show_network_tags;
    let mut list = column![].spacing(if density == theme::Density::Cozy { 4 } else { 2 });
    let mut previous_day: Option<chrono::NaiveDate> = None;
    let mut previous_sender: Option<&OwnedUserId> = None;

    for message in &session.timeline {
        let local: Option<DateTime<Local>> =
            Local.timestamp_millis_opt(message.timestamp_ms as i64).single();

        let mut day_changed = false;
        if let Some(local) = local {
            let day = local.date_naive();
            if previous_day != Some(day) {
                previous_day = Some(day);
                previous_sender = None;
                day_changed = true;
                list = list.push(
                    row![
                        ui::hline(p),
                        ui::mono(day_label(day).to_lowercase(), 11.0, p.text_muted()),
                        ui::hline(p),
                    ]
                    .spacing(10)
                    .padding([8, 0])
                    .align_y(iced::Alignment::Center),
                );
            }
        }
        let time_label = local
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default();
        let net = net_color(message.sender_network, p);
        let is_me = me == Some(&message.sender);

        if message.kind == MessageKind::System {
            let indent = if density == theme::Density::Cozy { 50.0 } else { 0.0 };
            list = list.push(
                row![
                    Space::new().width(indent),
                    ui::mono("→", 11.0, net),
                    ui::mono(&message.body, 11.0, p.text_muted()),
                    ui::mono(if tags { message.sender_network.tag() } else { "" }, 11.0, net),
                ]
                .spacing(8)
                .padding([4, 0]),
            );
            previous_sender = None;
            continue;
        }

        let irc = density == theme::Density::Irc;
        let content = message_content(message, session, irc, x.plain_text_mode, p);
        let name_color = if is_me { p.accent() } else { p.text_bright() };
        list = list.push(match density {
            theme::Density::Cozy => {
                // A new author (or a new day) starts a group with avatar + header;
                // their next messages sit under it.
                if day_changed || previous_sender != Some(&message.sender) {
                    let tag = match message.sender_network {
                        Network::Discord => "dc".to_string(),
                        Network::Matrix => format!("mx · {}", message.sender.server_name()),
                    };
                    let fill = if is_me { p.accent() } else { net };
                    let mut header = row![ui::sans(&message.sender_name, 14.0, Weight::Semibold, p.text_bright())]
                        .spacing(8)
                        .align_y(iced::Alignment::Center);
                    if tags {
                        header = header.push(ui::tag(tag, net, p));
                    }
                    Element::from(
                        row![
                            ui::avatar(&message.sender_name, 38.0, fill, p, None),
                            column![
                                header.push(ui::mono(time_label, 11.0, p.text_muted())),
                                content,
                            ]
                            .spacing(4)
                            .width(Length::Fill),
                        ]
                        .spacing(12)
                        .padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO }),
                    )
                } else {
                    row![Space::new().width(Length::Fixed(50.0)), content].into()
                }
            }
            theme::Density::Compact => row![
                ui::mono(time_label, 11.0, p.text_muted()).width(Length::Fixed(40.0)),
                ui::sans(&message.sender_name, 14.0, Weight::Semibold, name_color),
                ui::mono(if tags { message.sender_network.tag() } else { "" }, 10.0, net),
                content,
            ]
            .spacing(8)
            .into(),
            theme::Density::Irc => row![
                ui::mono(format!("[{time_label}]"), 12.0, p.text_muted()),
                ui::mono(format!("<{}>", message.sender_name), 12.0, if is_me { p.accent() } else { net }),
                content,
            ]
            .spacing(8)
            .into(),
        });

        previous_sender = Some(&message.sender);
    }

    list.into()
}

/// A message's body: prose (with inline `code`), fenced code blocks, an
/// image figure, and its reaction pills.
fn message_content<'a>(
    message: &'a TimelineMessage,
    session: &'a Session,
    all_mono: bool,
    plain: bool,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let body_color = theme::mix(p.text(), p.text_bright(), 0.4);
    let mut col = column![].spacing(6).width(Length::Fill);

    match &message.kind {
        MessageKind::Image(info) if plain => {
            let size = info.size.map(|s| format!(" · {}", human_size(s))).unwrap_or_default();
            col = col.push(ui::mono(format!("🖼 {}{size}", message.body), 12.0, p.text_muted()));
        }
        MessageKind::Image(info) => {
            col = col.push(image_figure(&message.body, info, session, p));
        }
        MessageKind::Undecryptable => {
            col = col.push(text(&message.body).size(13).color(p.warning()));
        }
        MessageKind::Emote => {
            col = col.push(
                text(format!("* {} {}", message.sender_name, message.body))
                    .size(14)
                    .color(body_color),
            );
        }
        _ => {
            for (is_code, part) in split_fences(&message.body) {
                col = col.push(if is_code {
                    code_block(part, p)
                } else {
                    prose(part, all_mono, body_color, p)
                });
            }
        }
    }

    if !message.reactions.is_empty()
        && let Some(event_id) = &message.event_id
    {
        let mut pills = row![].spacing(6);
        for reaction in &message.reactions {
            let mine = reaction.mine.is_some();
            let (fill, edge) = if mine {
                (theme::mix(p.accent(), p.surface(), 0.85), p.accent())
            } else {
                (theme::mix(p.surface(), p.surface_alt(), 0.5), p.border_ctrl())
            };
            let fg = if mine { p.text_bright() } else { p.text() };
            pills = pills.push(
                button(ui::mono(format!("{} {}", reaction.key, reaction.count), 11.0, fg))
                    .on_press(Message::ToggleReaction {
                        event_id: event_id.clone(),
                        key: reaction.key.clone(),
                        mine: reaction.mine.clone(),
                    })
                    .padding([3, 8])
                    .style(move |_, _| button::Style {
                        background: Some(fill.into()),
                        border: iced::border::rounded(12).color(edge).width(1),
                        ..button::Style::default()
                    }),
            );
        }
        col = col.push(pills);
    }
    col.into()
}

/// Splits a message into `(is_code, text)` parts at ``` fences; a fence's
/// first line is dropped when it's a language tag. An unclosed fence is code
/// to the end, like most renderers do.
fn split_fences(body: &str) -> Vec<(bool, &str)> {
    body.split("```")
        .enumerate()
        .filter_map(|(i, part)| {
            let is_code = i % 2 == 1;
            let part = if is_code {
                match part.split_once('\n') {
                    Some((first, rest)) if !first.trim().contains(' ') => rest,
                    _ => part,
                }
            } else {
                part
            };
            let part = part.trim_matches('\n');
            (!part.trim().is_empty()).then_some((is_code, part))
        })
        .collect()
}

/// Prose with `inline code` spans.
fn prose<'a>(body: &'a str, all_mono: bool, color: iced::Color, p: &theme::Palette) -> Element<'a, Message> {
    let font = if all_mono { ui::MONO } else { ui::SANS };
    let size = if all_mono { 12.0 } else { 14.0 };
    if !body.contains('`') {
        return text(body).font(font).size(size).color(color).into();
    }
    let (code_fill, code_fg) = (p.surface_alt(), p.text_bright());
    let spans: Vec<iced::widget::text::Span<'a, (), iced::Font>> = body
        .split('`')
        .enumerate()
        .map(|(i, part)| {
            let span = iced::widget::span(part).size(size);
            if i % 2 == 1 {
                span.font(ui::MONO)
                    .size(12)
                    .color(code_fg)
                    .background(code_fill)
                    .border(iced::border::rounded(4))
                    .padding([1, 5])
            } else {
                span.font(font).color(color)
            }
        })
        .collect();
    iced::widget::rich_text(spans).into()
}

fn code_block<'a>(code: &'a str, p: &theme::Palette) -> Element<'a, Message> {
    let (fill, edge) = (p.background(), p.border());
    container(ui::mono(code, 12.0, p.text()))
        .padding([12, 14])
        .max_width(520)
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(8).color(edge).width(1),
            ..container::Style::default()
        })
        .into()
}

/// An image with a mono caption (`name` · `W×H · size`), scaled to fit 440×320.
fn image_figure<'a>(
    name: &'a str,
    info: &'a mesh_core::ImageInfo,
    session: &'a Session,
    p: &theme::Palette,
) -> Element<'a, Message> {
    const MAX_W: f32 = 440.0;
    const MAX_H: f32 = 320.0;
    let (w, h) = match (info.width, info.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let scale = (MAX_W / w as f32).min(MAX_H / h as f32).min(1.0);
            (w as f32 * scale, h as f32 * scale)
        }
        _ => (MAX_W, 220.0),
    };
    let picture: Element<'_, Message> = match session.images.get(&info.media.key()) {
        Some(Some(handle)) => iced::widget::image(handle.clone())
            .width(w)
            .height(h)
            .content_fit(iced::ContentFit::Contain)
            .into(),
        _ => container(ui::mono("loading…", 11.0, p.text_muted()))
            .width(w)
            .height(h)
            .center_x(w)
            .center_y(h)
            .into(),
    };

    let mut details = Vec::new();
    if let (Some(w), Some(h)) = (info.width, info.height) {
        details.push(format!("{w}×{h}"));
    }
    if let Some(size) = info.size {
        details.push(human_size(size));
    }
    let caption_fill = p.panel_dim();
    let caption = container(
        row![
            ui::mono(name, 11.0, p.text_muted()).width(Length::Fill),
            ui::mono(details.join(" · "), 11.0, p.text_muted()),
        ]
        .spacing(10),
    )
    .padding([6, 10])
    .width(w)
    .style(move |_| container::Style {
        background: Some(caption_fill.into()),
        ..container::Style::default()
    });

    let edge = p.border_ctrl();
    container(column![picture, caption])
        .clip(true)
        .style(move |_| container::Style {
            border: iced::border::rounded(8).color(edge).width(1),
            ..container::Style::default()
        })
        .into()
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} B"),
    }
}

/// "alice is typing…" under the composer.
fn typing_line<'a>(names: &[String], p: &theme::Palette) -> Element<'a, Message> {
    let verb = match names.len() {
        0 => return Space::new().into(),
        1 => " is typing…",
        _ => " are typing…",
    };
    let who = if names.len() > 3 {
        "several people".to_string()
    } else {
        names.join(", ")
    };
    row![ui::mono(who, 11.0, p.proto_mx()), ui::mono(verb, 11.0, p.text_muted())].into()
}

fn day_label(day: chrono::NaiveDate) -> String {
    let today = Local::now().date_naive();
    if day == today {
        "Today".to_string()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".to_string()
    } else {
        day.format("%A, %-d %B %Y").to_string()
    }
}

fn verification_banner<'a>(
    prompt: &'a VerificationPrompt,
    palette: &'a theme::Palette,
) -> Element<'a, Message> {
    let content: Element<'_, Message> = match &prompt.stage {
        VerifyStage::Requested => column![
            text(format!("Verification request from {}", prompt.user_id))
                .size(14)
                .color(palette.text()),
            row![
                button(text("Accept").size(13))
                    .on_press(Message::AcceptVerification)
                    .padding(8),
                button(text("Decline").size(13))
                    .on_press(Message::CancelVerification)
                    .padding(8),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into(),
        VerifyStage::Accepted => column![
            text("Verification accepted — waiting for the other device...")
                .size(14)
                .color(palette.text()),
            button(text("Cancel").size(13))
                .on_press(Message::CancelVerification)
                .padding(8),
        ]
        .spacing(8)
        .into(),
        VerifyStage::Emojis(emojis) => {
            let emoji_row = emojis.iter().fold(row![].spacing(12), |r, emoji| {
                r.push(
                    column![
                        text(emoji.symbol.clone()).size(28),
                        text(emoji.description.clone())
                            .size(11)
                            .color(palette.text_muted()),
                    ]
                    .spacing(2)
                    .align_x(iced::Alignment::Center),
                )
            });
            column![
                text("Compare these emojis with the other device:")
                    .size(14)
                    .color(palette.text()),
                emoji_row,
                row![
                    button(text("They match").size(13))
                        .on_press(Message::ConfirmVerification)
                        .padding(8),
                    button(text("They don't match").size(13))
                        .on_press(Message::CancelVerification)
                        .padding(8),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into()
        }
        VerifyStage::Confirmed => text("Waiting for the other side to confirm...")
            .size(14)
            .color(palette.text())
            .into(),
    };

    let surface = palette.surface();
    let border_color = palette.border();
    container(content)
        .padding(12)
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(surface.into()),
            border: iced::border::rounded(8).color(border_color).width(1),
            ..container::Style::default()
        })
        .into()
}

/// Pushes the current mic/speaker choice down into the call engine.
fn apply_audio_devices(session: &Session) {
    let input =
        (session.selected_input != DEFAULT_DEVICE).then(|| session.selected_input.clone());
    let output =
        (session.selected_output != DEFAULT_DEVICE).then(|| session.selected_output.clone());
    session.client.set_audio_devices(input, output);
}

/// Microphone / speaker pickers for the settings screen.
fn audio_view<'a>(session: &'a Session, palette: &'a theme::Palette) -> Element<'a, Message> {
    let mic = row![
        text("Microphone").size(13).width(Length::Fixed(90.0)).color(palette.text()),
        pick_list(
            session.input_devices.clone(),
            Some(session.selected_input.clone()),
            Message::InputDeviceSelected,
        )
        .padding(6),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let speaker = row![
        text("Speaker").size(13).width(Length::Fixed(90.0)).color(palette.text()),
        pick_list(
            session.output_devices.clone(),
            Some(session.selected_output.clone()),
            Message::OutputDeviceSelected,
        )
        .padding(6),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    column![
        text("Audio").size(14).color(palette.text_muted()),
        mic,
        speaker,
    ]
    .spacing(8)
    .into()
}

/// The settings layout: nav │ page │ (on the Rice page) preview + export.
fn settings_shell<'a>(session: &'a Session, state: &'a App, wide: bool) -> Element<'a, Message> {
    let (p, l, x) = (&state.palette, &state.layout, &state.extras);
    let mut panes = row![
        tile(session, Pane::Channels, settings_nav(session, p, l), 220.0, p, l, x),
        tile(session, Pane::Chat, settings_page(session, state), 0.0, p, l, x),
    ]
    .spacing(l.gaps_out)
    .height(Length::Fill);
    if session.settings_page == SettingsPage::Rice && wide {
        panes = panes.push(
            column![
                ui::pane(rice_preview(p), false, p, l, x.translucent_panes).width(Length::Fill),
                ui::pane(rice_export(state), false, p, l, x.translucent_panes)
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .spacing(l.gaps_out)
            .width(300),
        );
    }
    panes.into()
}

fn settings_nav<'a>(session: &'a Session, p: &'a theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let item = |label: &'static str, page: SettingsPage| -> Element<'a, Message> {
        let active = session.settings_page == page;
        let bar = if active { p.accent() } else { iced::Color::TRANSPARENT };
        let color = if active { p.text_bright() } else { p.text() };
        let weight = if active { Weight::Semibold } else { Weight::Normal };
        ui::row_button(
            row![ui::swatch(bar, 2.0, 18.0, 1.0), ui::sans(label, 14.0, weight, color)]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            Message::OpenSettings(page),
            active,
            p,
            l,
        )
        .padding(iced::Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 0.0 })
        .into()
    };
    let disabled = |label: &'static str, why: &'static str| {
        ui::not_yet(
            container(ui::sans(label, 14.0, Weight::Normal, p.text_muted())).padding([7, 10]),
            why,
            p,
        )
    };
    column![
        button(
            row![
                ui::mono("← back", 12.0, p.text_muted()).width(Length::Fill),
                ui::outline_chip(ui::mono("Esc", 11.0, p.text_muted()), p),
            ]
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::ToggleSettings)
        .padding([6, 8])
        .style(flat_button_style(p)),
        ui::section("user", p),
        item("Accounts & bridges", SettingsPage::Accounts),
        button(ui::sans("Friends", 14.0, Weight::Normal, p.text()))
            .on_press(Message::OpenAddFriend)
            .width(Length::Fill)
            .padding([7, 10])
            .style(flat_button_style(p)),
        item("Privacy & E2EE", SettingsPage::Privacy),
        ui::section("app", p),
        item("Rice", SettingsPage::Rice),
        item("Keybinds", SettingsPage::Keybinds),
        item("Voice & audio", SettingsPage::Voice),
        disabled("Notifications", "Desktop notifications aren't supported yet."),
        Space::new().height(Length::Fill),
        container(ui::mono(
            concat!("mesh ", env!("CARGO_PKG_VERSION"), "\niced · matrix-rust-sdk"),
            11.0,
            p.text_muted(),
        ))
        .padding(10),
    ]
    .spacing(2)
    .padding([14, 10])
    .height(Length::Fill)
    .into()
}

fn settings_page<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let p = &state.palette;
    let page: Element<'a, Message> = match session.settings_page {
        SettingsPage::Rice => rice_page(state),
        SettingsPage::Accounts => accounts_page(session, state),
        SettingsPage::Privacy => column![
            page_header("Privacy & E2EE", "Your message keys, backed up so they follow you.", p),
            backup_view(&session.backup, p),
        ]
        .spacing(20)
        .into(),
        SettingsPage::Voice => voice_page(session, p),
        SettingsPage::Keybinds => keybinds_page(p),
    };
    scrollable(container(page).padding([24, 28]).width(Length::Fill))
        .height(Length::Fill)
        .into()
}

fn page_header<'a>(title: &'a str, sub: &'a str, p: &theme::Palette) -> Element<'a, Message> {
    column![
        ui::sans(title, 22.0, Weight::Semibold, p.text_bright()),
        text(sub).size(14).color(p.text_muted()),
    ]
    .spacing(4)
    .into()
}

/// A mono uppercase heading for a settings section.
fn heading<'a>(label: impl Into<String>, p: &theme::Palette) -> Element<'a, Message> {
    ui::mono(label.into().to_uppercase(), 12.0, p.text_muted()).into()
}

fn rice_page<'a>(state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let choice = state.theme_choice.as_str();
    let is_preset = choice != theme::RICE && choice != theme::CUSTOM;
    let source = match choice {
        theme::RICE => "rice",
        theme::CUSTOM => "toml",
        _ => "preset",
    };

    let rice_meta = if theme::colors_conf_path().exists() {
        (format!("● watching · reloaded {}", state.theme_reloaded), p.success())
    } else {
        ("not found · using defaults".to_string(), p.warning())
    };
    let toml_meta = if theme_path().exists() {
        ("hand-edited".to_string(), p.text_muted())
    } else {
        ("not found · using defaults".to_string(), p.warning())
    };
    let first_preset = theme::presets()[0].0.to_string();
    let sources = column![
        source_card(
            "Follow my rice",
            theme::colors_conf_path().display().to_string(),
            rice_meta,
            choice == theme::RICE,
            Message::ThemeChosen(theme::RICE.to_string()),
            p,
        ),
        source_card(
            "Built-in preset",
            theme::presets().iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" · "),
            (format!("{} presets", theme::presets().len()), p.text_muted()),
            is_preset,
            Message::ThemeChosen(if is_preset { choice.to_string() } else { first_preset }),
            p,
        ),
        source_card(
            "Custom theme.toml",
            theme_path().display().to_string(),
            toml_meta,
            choice == theme::CUSTOM,
            Message::ThemeChosen(theme::CUSTOM.to_string()),
            p,
        ),
    ]
    .spacing(8);
    let mut source_section = column![heading("theme source", p), sources].spacing(8);
    if is_preset {
        let mut chips = row![].spacing(6);
        for (name, _) in theme::presets() {
            chips = chips.push(segment(name, choice == name, Message::ThemeChosen(name.to_string()), p, l));
        }
        source_section = source_section.push(chips);
    }

    // Base tokens use the rice's names when following it.
    let rice_keys = choice == theme::RICE;
    let base = [
        ("$bg2", "background", p.background(), "Gaps / window background, inputs, code blocks"),
        ("$bg1", "surface", p.surface(), "Pane background"),
        ("$surface", "surface_alt", p.surface_alt(), "Selected rows, chips, active space tile"),
        ("$accent", "accent", p.accent(), "Focused pane border, active indicators, primary buttons"),
        ("$text", "text", p.text(), "Body text"),
    ];
    let mut table = column![];
    for (rice_key, field, color, used_for) in base {
        let key = if rice_keys { rice_key } else { field };
        table = table.push(token_row(key.to_string(), color, used_for.to_string(), source, p.proto_mx(), p));
    }
    let muted_contrast = theme::contrast(p.text_muted(), p.surface_alt());
    let derived = [
        ("text_bright", &p.text_bright, p.text_bright(), "Names, headings, active labels".to_string()),
        ("text_muted", &p.text_muted, p.text_muted(), format!("Timestamps, hints · {muted_contrast:.1}:1 on surface_alt")),
        ("accent_text", &p.accent_text, p.accent_text(), "Text on accent fills".to_string()),
        ("border", &p.border, p.border(), "Unfocused pane borders, separators".to_string()),
        ("border_ctrl", &p.border_ctrl, p.border_ctrl(), "Input and chip borders".to_string()),
        ("panel_dim", &p.panel_dim, p.panel_dim(), "User panel and footer strips".to_string()),
    ];
    for (key, set, color, used_for) in derived {
        let (tag, tag_color) = if set.is_some() { ("set", p.text_muted()) } else { ("derived", p.warning()) };
        table = table.push(token_row(key.to_string(), color, used_for, tag, tag_color, p));
    }
    let edge = p.border();
    let tokens = column![
        heading(
            match choice {
                theme::RICE => "tokens — read from colors.conf".to_string(),
                theme::CUSTOM => "tokens — read from theme.toml".to_string(),
                _ => format!("tokens — {choice}"),
            },
            p,
        ),
        container(table).style(move |_| container::Style {
            border: iced::border::rounded(8).color(edge).width(1),
            ..container::Style::default()
        }),
        text("Muted text is derived and nudged until it reaches 4.5:1 contrast, so a dark rice never turns into unreadable grey.")
            .size(12)
            .color(p.text_muted()),
    ]
    .spacing(8);

    let knobs = row![
        knob("gaps_in", l.gaps_in, 24.0, p),
        knob("gaps_out", l.gaps_out, 24.0, p),
        knob("border_size", l.border_size, 8.0, p),
        knob("rounding", l.rounding, 20.0, p),
    ]
    .spacing(8);
    let mut densities = row![].spacing(2);
    for density in theme::Density::ALL {
        densities = densities.push(segment(
            density.label(),
            state.density == density,
            Message::DensityChosen(density),
            p,
            l,
        ));
    }
    let (fill, ctrl) = (p.background(), p.border_ctrl());
    let layout = column![
        heading(
            if state.layout_from_hyprland {
                "layout — mirrors hyprland.conf"
            } else {
                "layout — defaults (layout_from_hyprland = false)"
            },
            p,
        ),
        knobs,
        row![
            ui::sans("Message density", 14.0, Weight::Normal, p.text_bright()),
            container(densities).padding(3).style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(8).color(ctrl).width(1),
                ..container::Style::default()
            }),
            ui::mono(state.density.hint(), 12.0, p.text_muted()),
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(10);

    let x = &state.extras;
    let extras = column![
        heading("extras", p),
        switch_row("Digital rain background", "Animated canvas behind the panes, drawn in your accent and muted text", x.digital_rain, Extra::Rain, p),
        switch_row("Translucent panes", "Panes at 92% opacity so Hyprland's blur shows through", x.translucent_panes, Extra::Translucent, p),
        switch_row("Show platform tags", "mx / dc next to names, so you always know where a message goes", x.show_network_tags, Extra::Tags, p),
        switch_row("Plain-text mode", "No image downloads or previews: just text, links and code", x.plain_text_mode, Extra::PlainText, p),
    ]
    .spacing(2);

    column![
        page_header("Rice", "Mesh wears your desktop. Change your theme once, every pane follows.", p),
        source_section,
        tokens,
        layout,
        extras,
    ]
    .spacing(20)
    .into()
}

/// A radio card for the theme source.
fn source_card<'a>(
    title: &'a str,
    path: String,
    (meta, meta_color): (String, iced::Color),
    on: bool,
    msg: Message,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let edge = if on { p.accent() } else { p.border_ctrl() };
    let fill = if on { theme::mix(p.surface(), p.surface_alt(), 0.5) } else { iced::Color::TRANSPARENT };
    let dot = if on { p.accent() } else { iced::Color::TRANSPARENT };
    let radio = container(ui::swatch(dot, 6.0, 6.0, 3.0))
        .center_x(16)
        .center_y(16)
        .style(move |_| container::Style {
            border: iced::border::rounded(8).color(edge).width(2),
            ..container::Style::default()
        });
    button(
        row![
            radio,
            column![
                ui::sans(title, 14.0, Weight::Medium, p.text_bright()),
                ui::mono(path, 12.0, p.text_muted()),
            ]
            .spacing(2)
            .width(Length::Fill),
            ui::mono(meta, 11.0, meta_color),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .on_press(msg)
    .width(Length::Fill)
    .padding([12, 14])
    .style(move |_, _| button::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..button::Style::default()
    })
    .into()
}

fn token_row<'a>(
    key: String,
    color: iced::Color,
    used_for: String,
    tag: &'a str,
    tag_color: iced::Color,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let edge = p.border_ctrl();
    let chip = container(Space::new()).width(22).height(22).style(move |_| container::Style {
        background: Some(color.into()),
        border: iced::border::rounded(5).color(edge).width(1),
        ..container::Style::default()
    });
    column![
        ui::hline(p),
        row![
            ui::mono(key, 12.0, p.text_bright()).width(120),
            chip,
            ui::mono(theme::to_hex(color), 12.0, p.text()).width(80),
            text(used_for).size(13).color(p.text_muted()).width(Length::Fill),
            ui::mono(tag, 12.0, tag_color),
        ]
        .spacing(12)
        .padding([8, 14])
        .align_y(iced::Alignment::Center),
    ]
    .into()
}

/// A read-only layout value with a bar showing it against `max`.
fn knob<'a>(key: &'a str, value: f32, max: f32, p: &theme::Palette) -> Element<'a, Message> {
    let filled = ((value / max).clamp(0.0, 1.0) * 100.0).round() as u16;
    let (fill, edge, track, accent) = (p.panel_dim(), p.border(), p.surface_alt(), p.accent());
    let bar = container(
        row![
            container(Space::new())
                .width(Length::FillPortion(filled.max(1)))
                .height(4)
                .style(move |_| container::Style {
                    background: Some(accent.into()),
                    border: iced::border::rounded(2),
                    ..container::Style::default()
                }),
            Space::new().width(Length::FillPortion(100 - filled.min(99))),
        ],
    )
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(track.into()),
        border: iced::border::rounded(2),
        ..container::Style::default()
    });
    container(
        column![
            ui::mono(key, 11.0, p.text_muted()),
            row![
                ui::mono_bold(format!("{value}"), 18.0, p.text_bright()),
                ui::mono(" px", 11.0, p.text_muted()),
            ]
            .align_y(iced::Alignment::End),
            bar,
        ]
        .spacing(4),
    )
    .padding([10, 12])
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    })
    .into()
}

/// One option of a segmented control (density, preset, add-friend mode).
fn segment<'a>(label: &'a str, on: bool, msg: Message, p: &theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let (fill, fg) = if on {
        (p.surface_alt(), p.text_bright())
    } else {
        (iced::Color::TRANSPARENT, p.text_muted())
    };
    let radius = l.control_radius();
    button(ui::mono(label, 12.0, fg))
        .on_press(msg)
        .padding([6, 14])
        .style(move |_, _| button::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(radius),
            ..button::Style::default()
        })
        .into()
}

fn switch_row<'a>(title: &'a str, sub: &'a str, on: bool, extra: Extra, p: &theme::Palette) -> Element<'a, Message> {
    let (track, knob) = if on { (p.accent(), p.accent_text()) } else { (p.border_ctrl(), p.text_muted()) };
    let switch = container(ui::swatch(knob, 16.0, 16.0, 8.0))
        .width(38)
        .height(22)
        .padding(3)
        .align_x(if on { iced::Alignment::End } else { iced::Alignment::Start })
        .style(move |_| container::Style {
            background: Some(track.into()),
            border: iced::border::rounded(11),
            ..container::Style::default()
        });
    column![
        button(
            row![
                column![
                    ui::sans(title, 14.0, Weight::Normal, p.text_bright()),
                    text(sub).size(12).color(p.text_muted()),
                ]
                .width(Length::Fill),
                switch,
            ]
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::ToggleExtra(extra))
        .padding([10, 0])
        .style(|_, _| button::Style::default()),
        ui::hline(p),
    ]
    .into()
}

/// A tiny drawing of the shell in the current colors.
fn rice_preview<'a>(p: &theme::Palette) -> Element<'a, Message> {
    let mini = |content: Element<'a, Message>, width: Length, focused: bool| {
        let (fill, edge) = (p.surface(), if focused { p.accent() } else { p.border() });
        container(content)
            .width(width)
            .height(Length::Fill)
            .padding(6)
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(4).color(edge).width(1),
                ..container::Style::default()
            })
    };
    let line = |color: iced::Color, width: f32| ui::swatch(color, width, 5.0, 2.0);
    let (bg, ctrl) = (p.background(), p.border_ctrl());
    let input = container(Space::new()).width(Length::Fill).height(14).style(move |_| container::Style {
        background: Some(bg.into()),
        border: iced::border::rounded(3).color(ctrl).width(1),
        ..container::Style::default()
    });
    let window = row![
        mini(column![ui::swatch(p.surface_alt(), 12.0, 12.0, 3.0), ui::swatch(p.accent(), 12.0, 12.0, 3.0)].spacing(4).into(), Length::Fixed(24.0), false),
        mini(column![line(p.border_ctrl(), 40.0), line(p.text(), 30.0), line(p.text_muted(), 36.0)].spacing(5).into(), Length::Fixed(60.0), false),
        mini(
            column![
                line(p.text(), 60.0),
                line(p.border_ctrl(), 100.0),
                line(p.text(), 50.0),
                ui::swatch(p.background(), 110.0, 30.0, 3.0),
                Space::new().height(Length::Fill),
                input,
            ]
            .spacing(6)
            .into(),
            Length::Fill,
            true,
        ),
    ]
    .spacing(5)
    .padding(5)
    .height(190);
    let fill = p.background();
    column![
        ui::mono("LIVE PREVIEW", 11.0, p.text_muted()),
        container(window).style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(6),
            ..container::Style::default()
        }),
    ]
    .spacing(10)
    .padding(14)
    .into()
}

fn rice_export<'a>(state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let toml = theme::export_toml(
        &state.theme_choice,
        p,
        l,
        state.layout_from_hyprland,
        state.density,
        &state.extras,
    );
    let fill = p.background();
    column![
        ui::mono("THEME.TOML", 11.0, p.text_muted()),
        scrollable(
            container(ui::mono(toml, 11.0, p.text()))
                .padding(12)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(fill.into()),
                    border: iced::border::rounded(6),
                    ..container::Style::default()
                }),
        )
        .height(Length::Fill),
        row![
            ui::primary_button(
                container(ui::sans("Export theme", 13.0, Weight::Semibold, p.accent_text())).center_x(Length::Fill),
                Some(Message::ExportTheme),
                p,
                l,
            )
            .width(Length::Fill),
            ui::not_yet(
                button(container(ui::sans("Share to #rice-showcase", 13.0, Weight::Normal, p.text_muted())).center_x(Length::Fill))
                    .padding([6, 10])
                    .width(Length::Fill)
                    .style(flat_button_style(p)),
                "Sharing a theme to a room isn't built yet: paste the exported text.",
                p,
            ),
        ]
        .spacing(8),
        ui::mono("Export copies it to the clipboard.", 11.0, p.text_muted()),
    ]
    .spacing(10)
    .padding(14)
    .into()
}

fn voice_page<'a>(session: &'a Session, p: &'a theme::Palette) -> Element<'a, Message> {
    let discord = match &session.discord_voice_url {
        Some(url) => format!("The 🎧 button opens {url} for Discord voice."),
        None => "Set discord_voice_url in config.toml to get a 🎧 button for Discord voice.".to_string(),
    };
    column![
        page_header("Voice & audio", "Devices for 1:1 calls, and Discord voice.", p),
        audio_view(session, p),
        text(discord).size(13).color(p.text_muted()),
        text("Mesh calls have no echo cancellation yet: use headphones, or an EasyEffects input preset with an echo canceller.")
            .size(13)
            .color(p.text_muted()),
    ]
    .spacing(20)
    .into()
}

fn keybinds_page<'a>(p: &theme::Palette) -> Element<'a, Message> {
    let mut rows = column![].spacing(2);
    for (keys, what) in [
        ("Alt+1..9", "Switch to space n on the rail"),
        ("Ctrl+K", "Jump to a room or space"),
        ("Esc", "Close settings, dialogs and the jump palette"),
        ("Enter", "Send the message"),
    ] {
        rows = rows.push(
            column![
                row![
                    container(ui::outline_chip(ui::mono(keys, 12.0, p.text_bright()), p)).width(140),
                    text(what).size(14).color(p.text()),
                ]
                .spacing(12)
                .padding([8, 0])
                .align_y(iced::Alignment::Center),
                ui::hline(p),
            ],
        );
    }
    column![
        page_header("Keybinds", "Keyboard first. These work everywhere in Mesh.", p),
        rows,
        text("Rebinding isn't supported yet.").size(12).color(p.text_muted()),
    ]
    .spacing(20)
    .into()
}

fn accounts_page<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let (p, l) = (&state.palette, &state.layout);
    let (name, mxid) = session
        .my_profile
        .as_ref()
        .map(|me| (me.display_name.clone(), me.user_id.to_string()))
        .unwrap_or_default();
    let (fill, edge) = (p.panel_dim(), p.border());
    let badge_fill = theme::mix(p.accent(), p.surface(), 0.8);
    let primary = container(ui::mono("PRIMARY", 10.0, p.accent()))
        .padding([1, 5])
        .style(move |_| container::Style {
            background: Some(badge_fill.into()),
            border: iced::border::rounded(4),
            ..container::Style::default()
        });
    let backup = match session.backup.status {
        Some(BackupStatus::Enabled) => ("key backup on", p.success()),
        Some(BackupStatus::Incomplete) => ("key backup needs your recovery key", p.warning()),
        Some(BackupStatus::Disabled) => ("no key backup", p.warning()),
        _ => ("checking key backup…", p.text_muted()),
    };
    let account = container(
        row![
            container(ui::mono_bold(initials(&my_server(session)), 13.0, p.text_bright()))
                .center_x(40)
                .center_y(40)
                .style({
                    let tile = p.surface_alt();
                    move |_| container::Style {
                        background: Some(tile.into()),
                        border: iced::border::rounded(8),
                        ..container::Style::default()
                    }
                }),
            column![
                row![ui::mono(mxid, 13.0, p.text_bright()), primary].spacing(8).align_y(iced::Alignment::Center),
                row![
                    ui::mono(format!("{name} · {}", my_server(session)), 11.0, p.text_muted()),
                    ui::mono("· syncing", 11.0, p.success()),
                    ui::mono(format!("· {}", backup.0), 11.0, backup.1),
                ]
                .spacing(6),
            ]
            .spacing(4)
            .width(Length::Fill),
            button(text("Log out").size(13))
                .on_press(Message::LogOut)
                .padding([6, 12])
                .style(flat_button_style(p)),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .padding([12, 14])
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    });

    let add_account = ui::not_yet(
        container(ui::mono("+ add a homeserver", 12.0, p.text_muted())).padding([10, 14]),
        "Multiple accounts aren't supported yet.",
        p,
    );

    let bridged = session.rooms.iter().filter(|r| r.network == Network::Discord).count();
    let bridge_line = if bridged > 0 {
        format!("mautrix-discord on {} · {bridged} bridged rooms", my_server(session))
    } else {
        format!("No bridged Discord rooms seen yet. DM your bridge bot on {} and send `login-qr` to link an account.", my_server(session))
    };
    let mut discord = column![
        heading("discord through a bridge", p),
        row![ui::mono("dc", 12.0, p.proto_dc()), text(bridge_line).size(13).color(p.text())].spacing(8),
    ]
    .spacing(8);
    if !state.discord_warning_seen {
        let (warn_fill, warn) = (theme::mix(p.warning(), p.surface(), 0.88), p.warning());
        discord = discord.push(
            container(
                column![
                    ui::sans("Discord doesn't allow third-party clients", 14.0, Weight::Semibold, warn),
                    text("A personal account linked through a bridge can be flagged or banned by Discord. Use an account you can afford to lose.")
                        .size(13)
                        .color(p.text()),
                    ui::primary_button(ui::sans("Got it", 13.0, Weight::Semibold, p.accent_text()), Some(Message::DiscordWarningSeen), p, l),
                ]
                .spacing(8),
            )
            .padding([12, 14])
            .style(move |_| container::Style {
                background: Some(warn_fill.into()),
                border: iced::border::rounded(8).color(warn).width(1),
                ..container::Style::default()
            }),
        );
    }

    column![
        page_header("Accounts & bridges", "Where your messages go, and through what.", p),
        column![heading("matrix homeservers", p), account, add_account].spacing(8),
        discord,
    ]
    .spacing(20)
    .into()
}

/// Which network the add-friend input is read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FriendMode {
    #[default]
    Auto,
    Matrix,
    Discord,
}

/// What the add-friend input looks like.
#[derive(Debug, Clone, PartialEq)]
enum Detected {
    Empty,
    /// `guessed`: a bare name completed with your homeserver.
    Matrix { id: String, guessed: bool },
    Discord(String),
    Bad(String),
}

/// Reads the add-friend input: `@name:server` is Matrix, a new-style
/// Discord username (`[a-z0-9_.]{2,32}`) is Discord, and the usual mistakes
/// get a specific explanation.
fn detect_friend(query: &str, mode: FriendMode, my_server: &str) -> Detected {
    let v = query.trim();
    if v.is_empty() {
        return Detected::Empty;
    }
    let is_discord = |v: &str| {
        (2..=32).contains(&v.len())
            && v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
    };
    let is_matrix = v.starts_with('@')
        && v.split_once(':').is_some_and(|(local, server)| local.len() > 1 && !server.is_empty())
        && OwnedUserId::try_from(v).is_ok();
    if mode != FriendMode::Discord && is_matrix {
        return Detected::Matrix { id: v.to_string(), guessed: false };
    }
    let lower = v.to_lowercase();
    if mode == FriendMode::Matrix && is_discord(&lower) && !my_server.is_empty() {
        return Detected::Matrix { id: format!("@{lower}:{my_server}"), guessed: true };
    }
    if mode != FriendMode::Matrix && is_discord(&lower) {
        return Detected::Discord(lower);
    }
    if mode != FriendMode::Matrix
        && let Some((_, tag)) = v.rsplit_once('#')
        && tag.len() == 4
        && tag.chars().all(|c| c.is_ascii_digit())
    {
        return Detected::Bad("Discord dropped #1234 tags — use their new username.".to_string());
    }
    if v.starts_with('@') && !v.contains(':') {
        return Detected::Bad(format!("Matrix IDs need a server: {v}:matrix.org"));
    }
    Detected::Bad("Not a Matrix ID or Discord username.".to_string())
}

/// The add-friend dialog's state.
#[derive(Default)]
struct AddFriend {
    mode: FriendMode,
    query: String,
    /// The last profile lookup: (user id, display name if found).
    profile: Option<(String, Option<String>)>,
    pending: Vec<PendingContact>,
    busy: bool,
    /// Last result, and whether it's an error.
    note: Option<(String, bool)>,
}

/// Text input id of the add-friend field, to focus it on open.
const FRIEND_INPUT: &str = "friend";

const DISCORD_REQUESTS_UNSUPPORTED: &str = "mautrix-discord can't send Discord friend requests. Message them in a server you share; the DM shows up here once it's bridged.";

fn load_pending(client: Arc<MeshClient>) -> Task<Message> {
    Task::perform(async move { client.pending_contacts().await }, Message::PendingLoaded)
}

/// The add-friend dialog, over the shell.
fn add_friend_view<'a>(
    session: &'a Session,
    dialog: &'a AddFriend,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let server = my_server(session);
    let detected = detect_friend(&dialog.query, dialog.mode, &server);
    let (tag, tag_color) = match &detected {
        Detected::Matrix { .. } => ("mx", p.proto_mx()),
        Detected::Discord(_) => ("dc", p.proto_dc()),
        _ => ("··", p.text_muted()),
    };
    let (hint, hint_color) = match &detected {
        Detected::Empty => (
            "Start typing — @user:server for Matrix, a plain username for Discord.".to_string(),
            p.text_muted(),
        ),
        Detected::Matrix { id, guessed: true } => (format!("✓ matrix · assumed your homeserver — {id}"), p.proto_mx()),
        Detected::Matrix { id, .. } => (
            format!("✓ matrix user on {}", id.split_once(':').map_or("", |(_, s)| s)),
            p.proto_mx(),
        ),
        Detected::Discord(_) => (format!("✓ discord username · via your bridge on {server}"), p.proto_dc()),
        Detected::Bad(why) => (format!("✗ {why}"), p.danger()),
    };
    let edge = match &detected {
        Detected::Bad(_) => p.danger(),
        Detected::Matrix { .. } | Detected::Discord(_) => p.accent(),
        Detected::Empty => p.border_ctrl(),
    };

    let send: Element<'_, Message> = match &detected {
        Detected::Matrix { .. } => ui::primary_button(
            ui::sans(if dialog.busy { "Sending…" } else { "Send DM invite" }, 14.0, Weight::Semibold, p.accent_text()),
            (!dialog.busy).then_some(Message::FriendSend),
            p,
            l,
        )
        .padding([10, 16])
        .into(),
        Detected::Discord(_) => ui::not_yet(
            ui::primary_button(ui::sans("Send friend request", 14.0, Weight::Semibold, p.accent_text()), None, p, l)
                .padding([10, 16]),
            DISCORD_REQUESTS_UNSUPPORTED,
            p,
        ),
        _ => ui::primary_button(ui::sans("Send", 14.0, Weight::Semibold, p.accent_text()), None, p, l)
            .padding([10, 16])
            .into(),
    };
    let (muted, bright, selection) = (p.text_muted(), p.text_bright(), theme::alpha(p.accent(), 0.35));
    let input = text_input("@someone:matrix.org  or  discord_name", &dialog.query)
        .id(iced::widget::Id::new(FRIEND_INPUT))
        .on_input(Message::FriendQuery)
        .on_submit(Message::FriendSend)
        .padding([10, 0])
        .size(14)
        .font(ui::MONO)
        .style(move |_, _| text_input::Style {
            background: iced::Color::TRANSPARENT.into(),
            border: iced::Border::default(),
            icon: muted,
            placeholder: muted,
            value: bright,
            selection,
        });
    let fill = p.background();
    let field = container(
        row![
            container(ui::mono_bold(tag, 11.0, tag_color)).width(20),
            input,
            send,
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .padding(iced::Padding { top: 4.0, right: 4.0, bottom: 4.0, left: 14.0 })
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    });

    let mut modes = row![].spacing(6);
    for (mode, label) in [
        (FriendMode::Auto, "auto-detect"),
        (FriendMode::Matrix, "matrix"),
        (FriendMode::Discord, "discord"),
    ] {
        modes = modes.push(segment(label, dialog.mode == mode, Message::FriendMode(mode), p, l));
    }

    let mut col = column![
        row![
            column![
                ui::sans("Add a friend", 20.0, Weight::Semibold, p.text_bright()),
                text("Paste anything — a Matrix ID, a Discord username. Mesh works out where it goes.")
                    .size(14)
                    .color(p.text_muted()),
            ]
            .spacing(4)
            .width(Length::Fill),
            ui::icon_button("✕", Some(Message::Escape), false, p, l),
        ],
        modes,
        column![
            ui::mono("USER ID OR USERNAME", 11.0, p.text_muted()),
            field,
            ui::mono(hint, 12.0, hint_color),
        ]
        .spacing(8),
    ]
    .spacing(18);

    // Preview card: who this reaches, and from which account.
    let me = session.my_profile.as_ref().map(|m| m.user_id.to_string()).unwrap_or_default();
    let preview = match &detected {
        Detected::Matrix { id, .. } => {
            let name = dialog
                .profile
                .as_ref()
                .filter(|(looked_up, _)| looked_up == id)
                .and_then(|(_, name)| name.clone())
                .unwrap_or_else(|| id.trim_start_matches('@').split(':').next().unwrap_or_default().to_string());
            Some((name, format!("{id}  →  from {me}"), p.proto_mx()))
        }
        Detected::Discord(name) => Some((name.clone(), format!("{name}  →  through your bridge on {server}"), p.proto_dc())),
        _ => None,
    };
    if let Some((name, route, color)) = preview {
        let (card, card_edge) = (theme::mix(p.surface(), p.surface_alt(), 0.4), p.border_ctrl());
        col = col.push(
            container(
                row![
                    ui::avatar(&name, 40.0, color, p, None),
                    column![
                        ui::sans(name, 14.0, Weight::Semibold, p.text_bright()),
                        ui::mono(route, 12.0, p.text_muted()),
                    ]
                    .spacing(2),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            )
            .padding([12, 14])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(card.into()),
                border: iced::border::rounded(8).color(card_edge).width(1),
                ..container::Style::default()
            }),
        );
    }

    let explain = if matches!(detected, Detected::Discord(_)) {
        DISCORD_REQUESTS_UNSUPPORTED
    } else {
        "Matrix has no friend list of its own, so Mesh opens an encrypted DM invite. It's recorded in your account data (m.direct), so it follows you to every device."
    };
    let box_edge = p.border_ctrl();
    col = col.push(
        container(text(explain).size(12).color(p.text_muted()))
            .padding([10, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                border: iced::border::rounded(8).color(box_edge).width(1),
                ..container::Style::default()
            }),
    );
    if let Some((note, error)) = &dialog.note {
        col = col.push(ui::mono(note, 12.0, if *error { p.danger() } else { p.success() }));
    }

    let mut pending = column![ui::mono(format!("PENDING — {}", dialog.pending.len()), 11.0, p.text_muted())].spacing(6);
    for contact in &dialog.pending {
        let color = net_color(contact.network, p);
        let mut line = row![
            ui::avatar(contact.user_id.localpart(), 32.0, color, p, None),
            column![
                ui::mono(contact.user_id.to_string(), 13.0, p.text_bright()),
                text(if contact.incoming { "wants to connect" } else { "invite sent" })
                    .size(11)
                    .color(p.text_muted()),
            ]
            .width(Length::Fill),
            ui::mono(contact.network.tag(), 10.0, color),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        if contact.incoming {
            line = line
                .push(
                    ui::primary_button(ui::sans("Accept", 13.0, Weight::Semibold, p.accent_text()), Some(Message::PendingAccept(contact.room_id.clone())), p, l),
                )
                .push(
                    button(text("Ignore").size(13))
                        .on_press(Message::PendingIgnore(contact.room_id.clone()))
                        .padding([6, 12])
                        .style(flat_button_style(p)),
                );
        } else {
            line = line.push(
                button(text("Cancel").size(13))
                    .on_press(Message::PendingCancel(contact.room_id.clone(), contact.user_id.clone()))
                    .padding([6, 12])
                    .style(flat_button_style(p)),
            );
        }
        pending = pending.push(column![container(line).padding([8, 4]), ui::hline(p)]);
    }
    col = col.push(scrollable(pending).height(Length::Fill));

    let scrim = theme::alpha(p.background(), 0.6);
    container(iced::widget::opaque(
        ui::pane(col.padding(24), true, p, l, false)
            .max_width(720)
            .max_height(720),
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(move |_| container::Style {
        background: Some(scrim.into()),
        ..container::Style::default()
    })
    .into()
}

/// Secure Backup controls: status, set-up (shows the recovery key once), and
/// restore-with-recovery-key to recover previously-undecryptable messages.
fn backup_view<'a>(backup: &'a BackupUi, palette: &'a theme::Palette) -> Element<'a, Message> {
    let status_text = match backup.status {
        Some(BackupStatus::Enabled) => "Enabled — your message keys are backed up.",
        Some(BackupStatus::Disabled) => {
            "Not set up. Enabling it stops message keys from being lost across sessions."
        }
        Some(BackupStatus::Incomplete) => {
            "A backup exists, but this device is missing the keys. Restore with your recovery key below."
        }
        Some(BackupStatus::Unknown) | None => "Checking backup status…",
    };

    // Anything but "Disabled" means a backup exists, and the button would replace it.
    let setup_label = if matches!(backup.status, Some(BackupStatus::Disabled)) {
        "Set up Secure Backup"
    } else {
        "Reset Secure Backup (old backup becomes unreadable)"
    };
    let setup_btn: Element<'_, Message> = if backup.busy {
        button(text("Working…").size(13)).padding(8).into()
    } else {
        button(text(setup_label).size(13))
            .on_press(Message::SetupBackup)
            .padding(8)
            .into()
    };

    let mut col = column![
        text("Secure Backup").size(14).color(palette.text_muted()),
        text(status_text).size(13).color(palette.text()),
        setup_btn,
    ]
    .spacing(8);

    if let Some(key) = &backup.new_key {
        let surface_alt = palette.surface_alt();
        col = col.push(
            text("Recovery key — write this down, it will not be shown again:")
                .size(12)
                .color(palette.danger()),
        );
        col = col.push(
            container(text(key.clone()).size(15).color(palette.text()))
                .padding(10)
                .width(Length::Fill)
                .style(move |_theme| container::Style {
                    background: Some(surface_alt.into()),
                    border: iced::border::rounded(6),
                    ..container::Style::default()
                }),
        );
    }

    col = col.push(
        text("Have a recovery key? Restore your message keys:")
            .size(12)
            .color(palette.text_muted()),
    );
    let mut restore_row = row![
        text_input("Recovery key", &backup.key_input)
            .on_input(Message::RecoveryKeyInput)
            .on_submit(Message::RestoreBackup)
            .padding(8),
    ]
    .spacing(8);
    if !backup.busy {
        restore_row = restore_row.push(
            button(text("Restore").size(13))
                .on_press(Message::RestoreBackup)
                .padding(8),
        );
    }
    col = col.push(restore_row);

    if let Some(message) = &backup.message {
        col = col.push(text(message.clone()).size(12).color(palette.accent()));
    }

    col.into()
}

fn main() -> iced::Result {
    // WebRTC (DTLS) and matrix-sdk both pull rustls, and with two crypto
    // providers in the tree rustls can't auto-pick one — install `ring`
    // explicitly, or the DTLS handshake panics mid-call. Must run first.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt::init();

    let app = ui::FONTS
        .into_iter()
        .fold(iced::application(boot, update, view), |app, font| app.font(font));
    app.default_font(ui::SANS)
        .theme(theme)
        .subscription(subscription)
        .title("Mesh")
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_split_prose_and_code() {
        let body = "look:\n```toml\ngaps_in = 4\n```\nnice\n```\nraw code here\n```";
        assert_eq!(
            split_fences(body),
            [(false, "look:"), (true, "gaps_in = 4"), (false, "nice"), (true, "raw code here")]
        );
        assert_eq!(split_fences("```unclosed\nx"), [(true, "x")]);
        assert_eq!(split_fences("plain"), [(false, "plain")]);
    }

    #[test]
    fn friend_detection() {
        use Detected::*;
        let d = |q: &str, mode| detect_friend(q, mode, "strange.lab");
        assert_eq!(d("", FriendMode::Auto), Empty);
        assert_eq!(
            d(" @nix:tiling.club ", FriendMode::Auto),
            Matrix { id: "@nix:tiling.club".into(), guessed: false }
        );
        assert_eq!(
            d("Nix", FriendMode::Matrix),
            Matrix { id: "@nix:strange.lab".into(), guessed: true }
        );
        assert_eq!(d("wayland_wanda", FriendMode::Auto), Discord("wayland_wanda".into()));
        assert!(matches!(d("@nix:tiling.club", FriendMode::Discord), Bad(_)));
        assert!(matches!(d("wanda#1234", FriendMode::Auto), Bad(why) if why.contains("#1234")));
        assert!(matches!(d("@nix", FriendMode::Auto), Bad(why) if why.contains("@nix:matrix.org")));
        assert!(matches!(d("no spaces allowed", FriendMode::Auto), Bad(_)));
    }

    #[test]
    fn initials_from_words_or_letters() {
        assert_eq!(initials("tiling club"), "TC");
        assert_eq!(initials("Nagbreek"), "NA");
        assert_eq!(initials("strange.lab"), "ST");
    }
}
