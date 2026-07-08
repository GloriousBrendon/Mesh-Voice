#![recursion_limit = "1024"]

mod rain;
mod theme;

use chrono::{DateTime, Local, TimeZone};
use iced::widget::{
    Space, button, column, container, pick_list, responsive, row, scrollable, stack, text,
    text_input,
};
use iced::{Element, Length, Subscription, Task, Theme as IcedTheme};
use mesh_core::{
    BackupStatus, MeshClient, MemberInfo, MeshUpdate, MyProfile, OwnedRoomId, OwnedUserId,
    RoomSummary, SasEmoji, TimelineMessage,
};
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
    let theme_choice = theme::load_choice(&config_path());
    let palette = theme::palette_for_choice(&theme_choice, &theme_path());
    let iced_theme = palette.to_iced_theme();

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
                session.rooms = rooms;
            }
            Task::none()
        }
        Message::RoomSelected(room_id) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.selected_room = Some(room_id.clone());
            session.timeline.clear();
            session.members.clear();
            session.status = None;
            session.show_settings = false;
            Task::batch([
                load_timeline(session.client.clone(), room_id.clone()),
                load_members(session.client.clone(), room_id),
            ])
        }
        Message::TimelineLoaded(room_id, result) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && session.selected_room.as_ref() == Some(&room_id)
            {
                match result {
                    Ok(timeline) => session.timeline = timeline,
                    Err(err) => session.status = Some(format!("Failed to load messages: {err}")),
                }
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
                        && call.call_id == call_id
                    {
                        call.state = CallUiState::Connecting;
                    }
                    // Carry the user's mic/deafen state into the new call.
                    let client = session.client.clone();
                    let (muted, deafened) = (session.self_muted, session.self_deafened);
                    Task::perform(
                        async move {
                            client.set_call_muted(muted).await;
                            client.set_call_deafened(deafened).await;
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
            let deafened = session.self_deafened;
            let client = session.client.clone();
            Task::perform(
                async move {
                    client.set_call_deafened(deafened).await;
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
                        session.backup.status = Some(BackupStatus::Enabled);
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
        Message::ToggleSettings => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.show_settings = !session.show_settings;
                if session.show_settings {
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
            state.palette = theme::palette_for_choice(&choice, &theme_path());
            state.iced_theme = state.palette.to_iced_theme();
            state.theme_choice = choice.clone();
            if let Err(err) = theme::save_choice(&config_path(), &choice) {
                tracing::warn!("failed to persist theme choice: {err}");
            }
            Task::none()
        }
        Message::LogOut => {
            if let Screen::LoggedIn(session) = &state.screen {
                let _ = session.client.logout_locally();
            }
            state.screen = Screen::LoggedOut(LoginForm::default());
            Task::none()
        }
    }
}

fn theme(state: &App) -> IcedTheme {
    state.iced_theme.clone()
}

fn subscription(_state: &App) -> Subscription<Message> {
    // Drive the animated background at ~15 fps.
    iced::time::every(std::time::Duration::from_millis(66)).map(|_| Message::Tick)
}

fn view(state: &App) -> Element<'_, Message> {
    let content = match &state.screen {
        Screen::Booting => center_text("Restoring session..."),
        Screen::LoggedOut(form) => login_view(form, false),
        Screen::LoggingIn(form) => login_view(form, true),
        Screen::LoggedIn(session) => session_view(session, state),
    };

    // The animated digital-rain background sits behind every screen.
    stack![
        rain::background(&state.palette, state.rain_time),
        content,
    ]
    .into()
}

fn center_text(label: &str) -> Element<'_, Message> {
    container(text(label.to_string()))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn login_view(form: &LoginForm, busy: bool) -> Element<'_, Message> {
    let mut form_column = column![
        text("Mesh").size(32),
        text("A lightweight Matrix client").size(14),
        Space::new().height(16),
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
    .spacing(12)
    .max_width(360);

    if let Some(error) = &form.error {
        form_column = form_column.push(text(error.clone()).size(13));
    }

    let submit = if busy {
        button(text("Logging in...")).padding(10)
    } else {
        button(text("Log in")).on_press(Message::LoginSubmitted).padding(10)
    };

    form_column = form_column.push(submit);

    container(form_column)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn session_view<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let palette = &state.palette;
    // `responsive` gives us the available size so we can drop the member list on
    // narrow windows — basic but keeps the chat usable when the window shrinks.
    responsive(move |size| {
        let sidebar = sidebar_view(session, palette);

        let main_panel: Element<'_, Message> = if session.show_settings {
            settings_view(session, state)
        } else {
            room_view(session, palette)
        };

        let main_area: Element<'_, Message> = if let Some(call) = &session.call {
            column![
                call_bar(call, palette),
                container(main_panel).height(Length::Fill),
            ]
            .into()
        } else {
            main_panel
        };

        let mut panes = row![sidebar, container(main_area).width(Length::Fill)];
        if !session.show_settings && session.selected_room.is_some() && size.width > 820.0 {
            panes = panes.push(member_list_view(&session.members, palette));
        }
        panes.into()
    })
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
            text(title).size(15).color(palette.text()).width(Length::Fill),
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

/// A Discord-style default avatar: a colored circle with the first initial.
fn avatar<'a>(name: &str, size: f32) -> Element<'a, Message> {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let color = avatar_color(name);
    container(text(initial).size(size * 0.42).color(iced::Color::WHITE))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .style(move |_theme| container::Style {
            background: Some(color.into()),
            border: iced::border::rounded(size / 2.0),
            ..container::Style::default()
        })
        .into()
}

/// Same color at a given alpha — used to let the rain glow faintly through
/// the side panels.
fn translucent(color: iced::Color, alpha: f32) -> iced::Color {
    iced::Color { a: alpha, ..color }
}

/// Deterministic avatar color from a name.
fn avatar_color(name: &str) -> iced::Color {
    const COLORS: [(u8, u8, u8); 6] = [
        (88, 101, 242),
        (87, 171, 111),
        (240, 171, 64),
        (237, 102, 105),
        (155, 120, 205),
        (64, 166, 214),
    ];
    let sum: usize = name.bytes().map(|b| b as usize).sum();
    let (r, g, b) = COLORS[sum % COLORS.len()];
    iced::Color::from_rgb8(r, g, b)
}

/// Right-hand member list panel.
fn member_list_view<'a>(members: &'a [MemberInfo], palette: &'a theme::Palette) -> Element<'a, Message> {
    let mut list = column![container(
        text(format!("Members — {}", members.len()))
            .size(12)
            .color(palette.text_muted()),
    )
    .padding([8, 8])]
    .spacing(2);

    for member in members {
        let name_color = if member.is_me {
            palette.accent()
        } else {
            palette.text()
        };
        list = list.push(
            container(
                row![
                    avatar(&member.display_name, 30.0),
                    text(&member.display_name).size(14).color(name_color),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            )
            .padding([4, 8]),
        );
    }

    let surface = translucent(palette.surface(), 0.86);
    container(scrollable(list).height(Length::Fill))
        .width(Length::Fixed(200.0))
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(surface.into()),
            ..container::Style::default()
        })
        .into()
}

/// The persistent bottom-left user panel: identity + mic/deafen controls,
/// and a disconnect button while in a call.
fn user_panel_view<'a>(session: &'a Session, palette: &'a theme::Palette) -> Element<'a, Message> {
    let name = session
        .my_profile
        .as_ref()
        .map(|p| p.display_name.clone())
        .unwrap_or_else(|| "…".to_string());
    let in_call = session.call.is_some();
    let status = session
        .call
        .as_ref()
        .map(|c| c.state.label())
        .unwrap_or("Not in a call");

    let mic_label = if session.self_muted { "🔇" } else { "🎙" };
    let deaf_label = if session.self_deafened { "🔇" } else { "🎧" };

    let mut controls = row![
        panel_button(mic_label, Message::ToggleMute, session.self_muted, palette),
        panel_button(deaf_label, Message::ToggleDeafen, session.self_deafened, palette),
    ]
    .spacing(6);
    if in_call {
        controls = controls.push(panel_button("📵", Message::HangUp, true, palette));
    }

    let identity = column![
        text(name).size(14).color(palette.text()),
        text(status).size(11).color(if in_call {
            palette.accent()
        } else {
            palette.text_muted()
        }),
    ]
    .spacing(1)
    .width(Length::Fill);

    let avatar_name = session
        .my_profile
        .as_ref()
        .map(|p| p.display_name.as_str())
        .unwrap_or("?");
    let surface_alt = palette.surface_alt();
    container(
        row![avatar(avatar_name, 34.0), identity, controls]
            .spacing(8)
            .align_y(iced::Alignment::Center),
    )
    .padding(8)
    .width(Length::Fill)
    .style(move |_theme| container::Style {
        background: Some(surface_alt.into()),
        border: iced::border::rounded(8),
        ..container::Style::default()
    })
    .into()
}

/// A small icon button for the user panel; `active` tints it with the danger
/// color (muted / deafened / disconnect).
fn panel_button<'a>(
    label: &'static str,
    msg: Message,
    active: bool,
    palette: &theme::Palette,
) -> Element<'a, Message> {
    let text_color = if active {
        palette.danger()
    } else {
        palette.text()
    };
    let surface = palette.surface();
    button(text(label).size(15))
        .on_press(msg)
        .padding(6)
        .style(move |_theme, status| {
            let background = if matches!(status, button::Status::Hovered) {
                Some(surface.into())
            } else {
                None
            };
            button::Style {
                background,
                text_color,
                border: iced::border::rounded(6),
                ..button::Style::default()
            }
        })
        .into()
}

fn sidebar_view<'a>(session: &'a Session, palette: &'a theme::Palette) -> Element<'a, Message> {
    let mut room_list = column![].spacing(2);

    let (directs, rooms): (Vec<_>, Vec<_>) =
        session.rooms.iter().partition(|room| room.is_direct);

    for (label, group) in [("Direct messages", directs), ("Rooms", rooms)] {
        if group.is_empty() {
            continue;
        }
        room_list = room_list.push(
            container(text(label).size(12).color(palette.text_muted()))
                .padding([8, 8]),
        );
        for room in group {
            let selected = session.selected_room.as_ref() == Some(&room.id);
            room_list = room_list.push(room_button(room, selected, palette));
        }
    }

    let header = row![
        text("Mesh").size(18).color(palette.text()),
        Space::new().width(Length::Fill),
        button(text("⚙").size(14))
            .on_press(Message::ToggleSettings)
            .padding(6)
            .style(flat_button_style(palette)),
    ]
    .align_y(iced::Alignment::Center);

    let sidebar = column![
        header,
        scrollable(room_list).height(Length::Fill),
        user_panel_view(session, palette),
        button(text("Log out").size(13))
            .on_press(Message::LogOut)
            .padding(8)
            .style(flat_button_style(palette)),
    ]
    .spacing(8)
    .width(Length::Fixed(240.0))
    .padding(12);

    let surface = translucent(palette.surface(), 0.86);
    container(sidebar)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(surface.into()),
            ..container::Style::default()
        })
        .into()
}

fn room_button<'a>(
    room: &'a RoomSummary,
    selected: bool,
    palette: &theme::Palette,
) -> Element<'a, Message> {
    let mut label = row![
        text(&room.name).size(14).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    if room.unread_notifications > 0 {
        let accent = palette.accent();
        let accent_text = palette.accent_text();
        label = label.push(
            container(
                text(room.unread_notifications.to_string())
                    .size(11)
                    .color(accent_text),
            )
            .padding([2, 6])
            .style(move |_theme| container::Style {
                background: Some(accent.into()),
                border: iced::border::rounded(8),
                ..container::Style::default()
            }),
        );
    }

    let text_color = palette.text();
    let surface_alt = palette.surface_alt();
    button(label)
        .on_press(Message::RoomSelected(room.id.clone()))
        .width(Length::Fill)
        .padding(8)
        .style(move |_theme, status| {
            let background = if selected || matches!(status, button::Status::Hovered) {
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
        })
        .into()
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

fn room_view<'a>(session: &'a Session, palette: &'a theme::Palette) -> Element<'a, Message> {
    let selected = session
        .selected_room
        .as_ref()
        .and_then(|id| session.rooms.iter().find(|r| &r.id == id));

    let header_label = selected
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Select a room".to_string());

    let mut header = row![
        text(header_label).size(20).color(palette.text()).width(Length::Fill),
    ]
    .align_y(iced::Alignment::Center);
    if session.selected_room.is_some() && session.call.is_none() {
        header = header.push(
            button(text("📞 Call").size(13))
                .on_press(Message::PlaceCall)
                .padding(8),
        );
    }

    let mut main_panel = column![header].spacing(12).padding(12);

    if let Some(topic) = selected.and_then(|r| r.topic.as_ref()) {
        main_panel = main_panel.push(
            text(topic.clone()).size(12).color(palette.text_muted()),
        );
    }

    if let Some(prompt) = &session.verification {
        main_panel = main_panel.push(verification_banner(prompt, palette));
    }

    let body: Element<'_, Message> = if session.selected_room.is_some() {
        scrollable(timeline_view(&session.timeline, palette))
            .height(Length::Fill)
            .anchor_bottom()
            .into()
    } else {
        container(
            text("Pick a room from the sidebar to start chatting.")
                .size(14)
                .color(palette.text_muted()),
        )
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    };
    main_panel = main_panel.push(body);

    if let Some(status) = &session.status {
        main_panel = main_panel.push(text(status.clone()).size(13).color(palette.danger()));
    }

    main_panel = main_panel.push(
        row![
            text_input("Message", &session.message_draft)
                .on_input(Message::DraftChanged)
                .on_submit(Message::SendMessage)
                .padding(10),
            button(text("Send")).on_press(Message::SendMessage).padding(10),
        ]
        .spacing(8),
    );

    main_panel.into()
}

fn timeline_view<'a>(
    timeline: &'a [TimelineMessage],
    palette: &'a theme::Palette,
) -> Element<'a, Message> {
    let mut list = column![].spacing(3).padding([0, 8]);
    let mut previous_day: Option<chrono::NaiveDate> = None;
    let mut previous_sender: Option<&OwnedUserId> = None;

    for message in timeline {
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
                    container(text(day_label(day)).size(12).color(palette.text_muted()))
                        .padding([6, 0])
                        .center_x(Length::Fill),
                );
            }
        }

        let time_label = local
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default();

        // A new author (or a new day) starts a fresh group with avatar + header;
        // consecutive messages from the same author are indented under it.
        let new_group = day_changed || previous_sender != Some(&message.sender);
        if new_group {
            list = list.push(
                row![
                    avatar(&message.sender_name, 38.0),
                    column![
                        row![
                            text(&message.sender_name).size(13).color(palette.accent()),
                            text(time_label).size(11).color(palette.text_muted()),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        text(&message.body).size(14).color(palette.text()),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                ]
                .spacing(10),
            );
        } else {
            list = list.push(row![
                Space::new().width(Length::Fixed(48.0)),
                text(&message.body).size(14).color(palette.text()).width(Length::Fill),
            ]);
        }

        previous_sender = Some(&message.sender);
    }

    list.into()
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

fn settings_view<'a>(session: &'a Session, state: &'a App) -> Element<'a, Message> {
    let palette = &state.palette;
    let mut choices = column![].spacing(6);

    let mut entries: Vec<(String, String)> = theme::presets()
        .into_iter()
        .map(|(name, _)| (name.to_string(), name.to_string()))
        .collect();
    entries.push((
        "Custom (theme.toml)".to_string(),
        theme::CUSTOM.to_string(),
    ));

    for (label, choice) in entries {
        let active = state.theme_choice == choice;
        let marker = if active { "●" } else { "○" };
        let accent = palette.accent();
        let text_color = palette.text();
        let surface_alt = palette.surface_alt();
        choices = choices.push(
            button(
                row![
                    text(marker).size(13).color(if active { accent } else { text_color }),
                    text(label).size(14),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .on_press(Message::ThemeChosen(choice))
            .width(Length::Fixed(280.0))
            .padding(8)
            .style(move |_theme, status| {
                let background = if active || matches!(status, button::Status::Hovered) {
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
            }),
        );
    }

    let content = column![
        row![
            text("Settings").size(20).color(palette.text()),
            Space::new().width(Length::Fill),
            button(text("Close").size(13))
                .on_press(Message::ToggleSettings)
                .padding(8)
                .style(flat_button_style(palette)),
        ]
        .align_y(iced::Alignment::Center),
        text("Theme").size(14).color(palette.text_muted()),
        choices,
        text(format!(
            "Custom colors are read from {}",
            theme_path().display()
        ))
        .size(12)
        .color(palette.text_muted()),
        Space::new().height(8),
        audio_view(session, palette),
        Space::new().height(8),
        backup_view(&session.backup, palette),
    ]
    .spacing(12)
    .padding(12)
    .max_width(520);

    scrollable(content).height(Length::Fill).into()
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

    let setup_label = if matches!(backup.status, Some(BackupStatus::Enabled)) {
        "Reset Secure Backup"
    } else {
        "Set up Secure Backup"
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

    iced::application(boot, update, view)
        .theme(theme)
        .subscription(subscription)
        .title("Mesh")
        .run()
}
