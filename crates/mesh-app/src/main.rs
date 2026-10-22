#![recursion_limit = "1024"]
// No console window next to the app on Windows (debug builds keep it for logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod cache;
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
    MediaRef, MessageKind, Network, OwnedEventId, PendingContact, RoomSummary, SasEmoji, SpaceGroup, TimelineMessage,
    group_by_space,
};
use mesh_discord::whatsapp;
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
    /// When the theme files were last (re)read, "HH:MM", for the Rice page.
    theme_reloaded: String,
    /// Socket of the hidden Discord client (Discord voice), if configured.
    discord_socket: Option<PathBuf>,
    /// Command that starts it (`discord_engine_command`).
    discord_command: Option<String>,
    /// The native WhatsApp helper beside Mesh's own binary, when it's there;
    /// WhatsApp then runs in it instead of the engine browser.
    whatsapp_native: Option<PathBuf>,
    /// The native Discord helper (`mesh-dc`) beside Mesh's own binary, when it's
    /// there; Discord then runs in it instead of the engine browser.
    discord_native: Option<PathBuf>,
    /// Its email + password sign-in form.
    dc_sign_in: accounts::DcSignIn,
    /// Handle to drive it, while connected.
    discord: Option<mesh_discord::Engine>,
    /// Its latest voice state.
    discord_state: Option<mesh_discord::Snapshot>,
    /// WhatsApp Web in the same engine browser: a handle while connected, and
    /// its chat list.
    whatsapp: Option<whatsapp::Engine>,
    whatsapp_state: Option<whatsapp::Snapshot>,
    /// Each signed-out engine site's current QR code, by network id.
    login_qr: HashMap<&'static str, accounts::LoginQr>,
    /// The engine snapshots less what the sync switches leave out, while
    /// any are off (`accounts::refresh_shown`).
    discord_shown: Option<mesh_discord::Snapshot>,
    whatsapp_shown: Option<whatsapp::Snapshot>,
    /// Networks that have been linked (so a sign-out elsewhere reads as
    /// "needs relink"), and the sync switches that are off (`wa:groups`).
    linked: Vec<String>,
    sync_off: Vec<String>,
    accounts: accounts::Panel,
    /// The call's video, by track: the latest frame of each camera
    /// (WhatsApp's ids start `wa-`).
    video: HashMap<String, VideoTile>,
    /// WhatsApp's call, while there is one.
    wa_call: Option<whatsapp::Call>,
    /// Whether Mesh's window has focus (no notifications for the chat in view).
    window_focused: bool,
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

/// The new WhatsApp chat picker: a name to search contacts, or a number.
#[derive(Default)]
struct NewChat {
    query: String,
    contacts: Vec<whatsapp::Contact>,
    /// Waiting for WhatsApp to open the chat.
    busy: bool,
    error: Option<String>,
}

/// One camera in a call: its latest frame, already decoded (a handle from
/// pixels draws in the same frame, so swapping frames never blanks).
struct VideoTile {
    frame: iced::widget::image::Handle,
    /// Ours (the self view), drawn small.
    local: bool,
    /// When the last frame came; a camera that stops sending goes away.
    at: std::time::Instant,
}

/// A camera that sent nothing for this long is off (or the call ended).
const VIDEO_STALE: std::time::Duration = std::time::Duration::from_millis(1500);

/// A GIF as one sprite sheet (its frames in a grid), drawn a frame at a time
/// by cropping. One image rather than one per frame, because iced evicts
/// images that weren't drawn last frame and re-decodes them on a worker,
/// which blanks a frame-per-image GIF between frames (it flashes).
struct Animation {
    sheet: iced::widget::image::Handle,
    /// One frame's size.
    frame: (u32, u32),
    cols: u32,
    count: u32,
    delay_ms: u64,
}

/// The voice note playing (or paused) in Mesh.
struct Player {
    /// Its media key (`wa-file:<id>`).
    key: String,
    /// Where it's paused, in samples of the original (while `out` is `None`).
    position: usize,
    out: Option<Playing>,
    /// Waiting for WhatsApp to decode it (or for it to be sped up).
    loading: bool,
}

/// A voice note coming out of the speaker.
struct Playing {
    _stream: mesh_core::PlaybackHandle,
    /// Samples still to play.
    left: Arc<std::sync::Mutex<std::collections::VecDeque<i16>>>,
    /// How many were queued, starting `from` samples into the original, at `speed`.
    queued: usize,
    from: usize,
    speed: f32,
}

impl Player {
    /// How far in it is, in samples of the original.
    fn position(&self) -> usize {
        match &self.out {
            Some(p) => {
                let played = p.queued.saturating_sub(p.left.lock().map(|l| l.len()).unwrap_or(0));
                p.from + (played as f32 * p.speed) as usize
            }
            None => self.position,
        }
    }
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
    discord_voice: Option<theme::DiscordVoice>,
    /// The pane with the accent border.
    focused: Pane,
    show_members: bool,
    /// Home first, then one group per space (the rail).
    groups: Vec<SpaceGroup>,
    /// The space shown in the channel pane; `None` is Home.
    active_space: Option<OwnedRoomId>,
    /// The Discord server (from the hidden client) shown in the channel pane
    /// instead of a space.
    active_guild: Option<String>,
    /// Showing the WhatsApp groups list (its own pane, off the rail) instead
    /// of Home/a space/a guild.
    active_wa_groups: bool,
    /// Which rail folders are expanded: [Discord, Matrix].
    folders_open: [bool; 3],
    /// Home's network chip; `None` is All.
    home_filter: Option<Network>,
    /// The emoji picker is open under the composer.
    emoji_open: bool,
    /// A file chosen in the composer, waiting to send (with the draft as caption).
    pending_attachment: Option<Attachment>,
    /// A file send is in flight (so the send button waits).
    sending_attachment: bool,
    /// The message under the mouse (its web id), for the action bar.
    hovered: Option<String>,
    /// The message whose quick-reaction strip is open.
    reacting: Option<String>,
    /// The message the next send replies to: (web id, quote).
    replying: Option<(String, mesh_core::Quote)>,
    /// The forward picker, while open: (message web id, chat filter).
    forwarding: Option<(String, String)>,
    /// Playing GIFs' frames, by media key (their first frame is in `images`).
    animations: HashMap<String, Animation>,
    /// The GIF clock, advanced by `MediaTick`.
    anim_ms: u64,
    /// The full-screen image: (shown media key, bigger copy's key).
    lightbox: Option<(String, Option<String>)>,
    /// Decoded voice notes (48 kHz mono), by media key.
    voice_pcm: HashMap<String, Arc<Vec<i16>>>,
    /// The voice note playing (or paused) in Mesh.
    player: Option<Player>,
    /// Voice note speed: 1, 1.5 or 2.
    voice_speed: f32,
    /// The new WhatsApp chat picker, while open.
    new_chat: Option<NewChat>,
    /// Archived WhatsApp chats are listed (under Home).
    show_archived: bool,
    /// The WhatsApp chat whose Delete is waiting for a yes.
    confirm_delete: Option<String>,
    /// The WhatsApp chat being deleted (closed once it's gone).
    deleting: Option<String>,
    /// The Discord text channel or DM shown in the chat pane (through the
    /// hidden client) instead of `selected_room`.
    web_chat: Option<WebChat>,
    /// The Ctrl+K jump palette's query, while it's open.
    jump: Option<String>,
    /// Who's typing, per room.
    typing: HashMap<OwnedRoomId, Vec<String>>,
    /// Downloaded images by media key; `None` while loading.
    images: HashMap<String, Option<iced::widget::image::Handle>>,
    /// The add-friend dialog, while it's open.
    add_friend: Option<AddFriend>,
    /// The join-a-Discord-server dialog, while it's open.
    join_server: Option<JoinServer>,
    /// Avatar thumbnails by media key; `None` while loading.
    avatars: HashMap<String, Option<iced::widget::image::Handle>>,
}

/// Text input id of the jump palette, to focus it on open.
const JUMP_INPUT: &str = "jump";
const FORWARD_INPUT: &str = "forward";
const NEW_CHAT_INPUT: &str = "new-chat";

/// Text input id of the message composer, to refocus after picking an emoji.
const COMPOSER_INPUT: &str = "composer";

/// Puts the cursor in the message box (on opening a chat, picking an emoji).
fn focus_composer() -> Task<Message> {
    iced::widget::operation::focus(iced::widget::Id::new(COMPOSER_INPUT))
}

/// A small curated set for the composer's emoji picker; expand if it's missed.
const EMOJI: &[&str] = &[
    "😀", "😂", "🙂", "😉", "😍", "😘", "😎", "🤔", "😅", "😭", "😡", "🥳", "😴", "🙄", "😇", "🤗",
    "👍", "👎", "👏", "🙏", "💪", "👌", "🤝", "🫶", "❤️", "🔥", "✨", "🎉", "💯", "😢", "😱", "🤷",
];

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
            discord_voice: theme::load_discord_voice(&config_path()),
            focused: Pane::Chat,
            show_members: true,
            groups: Vec::new(),
            active_space: None,
            active_guild: None,
            active_wa_groups: false,
            folders_open: [true, true, true],
            home_filter: None,
            emoji_open: false,
            pending_attachment: None,
            sending_attachment: false,
            hovered: None,
            reacting: None,
            replying: None,
            forwarding: None,
            animations: HashMap::new(),
            anim_ms: 0,
            lightbox: None,
            voice_pcm: HashMap::new(),
            player: None,
            voice_speed: 1.0,
            new_chat: None,
            show_archived: false,
            confirm_delete: None,
            deleting: None,
            web_chat: None,
            jump: None,
            typing: HashMap::new(),
            images: HashMap::new(),
            add_friend: None,
            join_server: None,
            avatars: HashMap::new(),
        }
    }
}

/// A file chosen in the composer, ready to send.
#[derive(Debug, Clone)]
struct Attachment {
    filename: String,
    mime: String,
    data: Vec<u8>,
}

/// The clipboard's image (a screenshot, say) as a PNG attachment.
fn clipboard_image() -> Option<Attachment> {
    let image = arboard::Clipboard::new().ok()?.get_image().ok()?;
    let pixels = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned())?;
    let mut data = Vec::new();
    pixels.write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png).ok()?;
    let filename = format!("screenshot-{}.png", Local::now().format("%Y%m%d-%H%M%S"));
    Some(Attachment { filename, mime: "image/png".to_string(), data })
}

/// A best-effort MIME type from a filename's extension (images and the common
/// ones), else a generic binary type. ponytail: small built-in map; the
/// backends re-sniff as needed.
fn mime_guess_from(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// A chat from a hidden web client (a Discord channel or DM, a WhatsApp
/// chat), as the chat pane needs it.
#[derive(Debug, Clone, PartialEq)]
struct WebChat {
    network: Network,
    id: String,
    name: String,
    /// One-to-one (a Discord DM, a WhatsApp chat with one person): @ icon, call button.
    dm: bool,
}

/// The hidden web clients, for sending a web chat's actions to its own.
/// Built from `App`'s fields, so it can sit beside a `&mut` session.
struct Clients<'a> {
    discord: Option<&'a mesh_discord::Engine>,
    whatsapp: Option<&'a whatsapp::Engine>,
}

impl Clients<'_> {
    /// Streams the chat's messages into the timeline; false if its client
    /// isn't running.
    fn open(&self, chat: &WebChat) -> bool {
        match chat.network {
            Network::Discord => self.discord.map(|e| e.open_channel(&chat.id)).is_some(),
            Network::WhatsApp => self.whatsapp.map(|e| e.open_chat(&chat.id)).is_some(),
            Network::Matrix => false,
        }
    }
    fn close(&self, chat: &WebChat) {
        match chat.network {
            Network::Discord => self.discord.map(mesh_discord::Engine::close_channel),
            Network::WhatsApp => self.whatsapp.map(whatsapp::Engine::close_chat),
            Network::Matrix => None,
        };
    }
    /// `reply_to` (a web message id) quotes that message; WhatsApp only.
    fn send(&self, chat: &WebChat, text: &str, reply_to: Option<&str>) -> bool {
        match chat.network {
            Network::Discord => self.discord.map(|e| e.send_message(&chat.id, text)).is_some(),
            Network::WhatsApp => self.whatsapp.map(|e| e.send_message(&chat.id, text, reply_to)).is_some(),
            Network::Matrix => false,
        }
    }
    /// WhatsApp's media (message photos, its profile picture CDN) goes
    /// through its own tab; everything else through Discord's.
    fn fetch_media(&self, url: &str) {
        if url.starts_with("wa-") || url.contains(".whatsapp.net/") {
            self.whatsapp.map(|e| e.fetch_media(url));
        } else {
            self.discord.map(|e| e.fetch_media(url));
        }
    }
}

/// WhatsApp messages as timeline rows, like `discord_timeline` (photos are
/// decrypted in WhatsApp's tab, fetched by `load_web_media`). ponytail: made-up
/// `@wa_<number>:whatsapp.net` user ids; voice notes, video and documents
/// show as labels.
fn whatsapp_timeline(messages: whatsapp::Messages) -> Vec<TimelineMessage> {
    let mut timeline = Vec::new();
    for m in messages.messages {
        let number: String = m.author_id.split(['@', ':']).next().unwrap_or_default().chars().filter(char::is_ascii_alphanumeric).collect();
        let Ok(sender) = OwnedUserId::try_from(format!("@wa_{number}:whatsapp.net")) else {
            continue;
        };
        let row = |body: String, kind| TimelineMessage {
            event_id: None,
            sender: sender.clone(),
            sender_name: m.author.clone(),
            sender_avatar: m.avatar.clone().map(MediaRef::web),
            sender_network: Network::WhatsApp,
            body,
            kind,
            reactions: Vec::new(),
            timestamp_ms: m.timestamp_ms,
            web_id: Some(m.id.clone()),
            reply_to: None,
        };
        let first = timeline.len();
        if m.system {
            timeline.push(row(format!("{} {}", m.author, m.content), MessageKind::System));
            continue;
        }
        if let Some(image) = &m.image {
            // Stickers come down 160 px wide; draw them at that size.
            let (width, height, label) = if image.sticker {
                let scale = |v: Option<u64>| v.map(|v| v * 160 / image.width.unwrap_or(160).max(1));
                (Some(160), scale(image.height), "sticker")
            } else {
                (image.width, image.height, if image.gif { "GIF" } else { "photo" })
            };
            // A GIF plays from its frames; a photo has a bigger copy for full screen.
            let (media, full) = if image.gif {
                (whatsapp::gif_url(&m.id), None)
            } else {
                (whatsapp::image_url(&m.id), (!image.sticker).then(|| MediaRef::web(whatsapp::big_url(&m.id))))
            };
            let info = mesh_core::ImageInfo { media: MediaRef::web(media), width, height, size: None, full };
            timeline.push(row(label.to_string(), MessageKind::Image(info)));
        }
        if let Some(f) = &m.file {
            let info = mesh_core::FileInfo {
                media: MediaRef::web(whatsapp::file_url(&m.id)),
                name: f.name.clone(),
                kind: f.kind.clone(),
                size: f.size,
                duration: f.duration,
                waveform: f.waveform.clone(),
                // Filled into `images` from the message as it arrives.
                thumbnail: f.thumb.as_ref().map(|_| mesh_core::ImageInfo {
                    media: MediaRef::web(format!("wa-thumb:{}", m.id)),
                    width: f.width,
                    height: f.height,
                    size: None,
                    full: None,
                }),
            };
            timeline.push(row(f.name.clone(), MessageKind::File(info)));
        }
        if !m.content.is_empty() {
            timeline.push(row(m.content.clone(), MessageKind::Text));
        }
        // A message can take several rows (photo, then caption): the quote
        // goes above the first, the reactions under the last.
        if let Some(q) = &m.quote
            && let Some(row) = timeline.get_mut(first)
        {
            row.reply_to = Some(mesh_core::Quote { author: q.author.clone(), text: q.text.clone() });
        }
        if timeline.len() > first
            && let Some(row) = timeline.last_mut()
        {
            row.reactions = m
                .reactions
                .iter()
                .map(|r| mesh_core::Reaction { key: r.key.clone(), count: r.count as usize, mine: None, by_me: r.mine })
                .collect();
        }
    }
    timeline
}

/// Discord messages as timeline rows, so the Matrix timeline view draws them:
/// each image becomes its own image row after the text (avatars and images
/// are web media, fetched through the hidden client by `load_web_media`).
/// ponytail: authors get a made-up `@discord_<id>:discord.com` user id,
/// other attachments are links and there are no reactions; give Discord its
/// own message type when those matter.
fn discord_timeline(messages: mesh_discord::Messages) -> Vec<TimelineMessage> {
    let mut timeline = Vec::new();
    for m in messages.messages {
        let Ok(sender) = OwnedUserId::try_from(format!("@discord_{}:discord.com", m.author_id)) else {
            continue;
        };
        let row = |body: String, kind| TimelineMessage {
            event_id: None,
            sender: sender.clone(),
            sender_name: m.author.clone(),
            sender_avatar: m.avatar.clone().map(MediaRef::web),
            sender_network: Network::Discord,
            body,
            kind,
            reactions: Vec::new(),
            timestamp_ms: m.timestamp_ms,
            web_id: None,
            reply_to: None,
        };
        let body = if m.system { format!("{} {}", m.author, m.content) } else { m.content.clone() };
        let body = body.trim().to_string();
        let (images, files): (Vec<_>, Vec<_>) = m.attachments.iter().partition(|a| a.preview.is_some());
        if !body.is_empty() || m.attachments.is_empty() {
            timeline.push(row(body, if m.system { MessageKind::System } else { MessageKind::Text }));
        }
        // Videos, audio, documents: a card that opens the CDN URL in the browser.
        for a in files {
            let info = mesh_core::FileInfo {
                media: MediaRef::web(a.url.clone()),
                name: a.filename.clone(),
                kind: file_kind(&a.filename).to_string(),
                size: a.size,
                duration: None,
                waveform: Vec::new(),
                thumbnail: None,
            };
            timeline.push(row(a.filename.clone(), MessageKind::File(info)));
        }
        for a in images {
            let Some(preview) = a.preview.clone() else { continue };
            let full = Some(MediaRef::web(a.url.clone()));
            let info = mesh_core::ImageInfo { media: MediaRef::web(preview), width: a.width, height: a.height, size: a.size, full };
            timeline.push(row(a.filename.clone(), MessageKind::Image(info)));
        }
    }
    timeline
}

/// A label for an attachment, from its extension.
fn file_kind(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "mov" | "mkv" | "webm" | "avi") => "Video",
        Some("mp3" | "ogg" | "opus" | "wav" | "m4a" | "flac") => "Audio",
        Some("pdf") => "PDF",
        Some("zip" | "rar" | "7z" | "gz") => "Archive",
        _ => "File",
    }
}

/// A picture a hidden client fetched: into whichever cache is waiting for it.
fn web_media_loaded(state: &mut App, url: String, bytes: Result<Vec<u8>, String>) -> Task<Message> {
    let Screen::LoggedIn(session) = &state.screen else {
        return Task::none();
    };
    let pending = |map: &HashMap<String, Option<iced::widget::image::Handle>>| matches!(map.get(&url), Some(None));
    if pending(&session.avatars) {
        update(state, Message::AvatarLoaded(url, bytes))
    } else if pending(&session.images) {
        update(state, Message::ImageLoaded(url, bytes))
    } else {
        Task::none()
    }
}

/// Asks the hidden Discord client for the snapshot's pictures (server icons,
/// DM and voice avatars) that aren't cached or loading.
fn load_snapshot_media(session: &mut Session, snapshot: &mesh_discord::Snapshot, engine: &mesh_discord::Engine) {
    let members = snapshot.guilds.iter().flat_map(|g| g.voice_channels.iter().flat_map(|c| &c.members));
    let urls = snapshot
        .guilds
        .iter()
        .filter_map(|g| g.icon.as_ref())
        .chain(snapshot.dms.iter().filter_map(|d| d.avatar.as_ref()))
        .chain(members.chain(&snapshot.voice.members).filter_map(|m| m.avatar.as_ref()));
    for url in urls {
        if wants(&mut session.avatars, cache::Kind::Avatar, url) {
            engine.fetch_media(url);
        }
    }
}

/// Plays a decoded voice note from `from` (samples of the original) at the
/// chosen speed on the chosen speaker, replacing whatever was playing. A
/// speed it hasn't been stretched to yet is worked out off the UI thread
/// first (`VoiceStretched` then plays it).
fn play_voice(session: &mut Session, key: String, from: usize) -> Task<Message> {
    let Some(pcm) = session.voice_pcm.get(&key).cloned() else {
        return Task::none();
    };
    session.player = None; // stops the old stream first
    let speed = session.voice_speed;
    let from = from.min(pcm.len());
    let source = if speed == 1.0 {
        pcm
    } else if let Some(fast) = session.voice_pcm.get(&stretched_key(&key, speed)) {
        fast.clone()
    } else {
        session.player = Some(Player { key: key.clone(), position: from, out: None, loading: true });
        return Task::perform(async move { Arc::new(mesh_core::time_stretch(&pcm, speed)) }, move |samples| {
            Message::VoiceStretched { key: key.clone(), speed, samples, from }
        });
    };
    let start = ((from as f32 / speed) as usize).min(source.len());
    let device = (session.selected_output != DEFAULT_DEVICE).then(|| session.selected_output.clone());
    match mesh_core::start_playback(device) {
        Ok((stream, left)) => {
            if let Ok(mut left) = left.lock() {
                left.extend(&source[start..]);
            }
            let playing = Playing { _stream: stream, left, queued: source.len() - start, from, speed };
            session.player = Some(Player { key, position: from, out: Some(playing), loading: false });
        }
        Err(err) => session.status = Some(format!("Couldn't play it: {err}")),
    }
    Task::none()
}

/// Where a voice note sped up to `speed` is kept in `voice_pcm`.
fn stretched_key(key: &str, speed: f32) -> String {
    format!("{key}@{speed}")
}

/// How often `MediaTick` fires (a GIF frame is ~83 ms).
const MEDIA_TICK_MS: u64 = 80;

/// Shows a GIF: its first frame as the still in `images`, all of them in
/// `animations`.
fn show_frames(
    images: &mut HashMap<String, Option<iced::widget::image::Handle>>,
    animations: &mut HashMap<String, Animation>,
    key: &str,
    delay_ms: u64,
    frames: Vec<Vec<u8>>,
) {
    let decoded: Vec<image::RgbaImage> =
        frames.iter().filter_map(|f| image::load_from_memory(f).ok().map(|i| i.to_rgba8())).collect();
    let (Some(first), Some(still)) = (decoded.first(), frames.into_iter().next()) else {
        return;
    };
    let (w, h) = first.dimensions();
    let count = decoded.len() as u32;
    let cols = (count as f32).sqrt().ceil() as u32;
    let mut sheet = image::RgbaImage::new(w * cols, h * count.div_ceil(cols));
    for (i, frame) in (0..).zip(&decoded) {
        image::imageops::replace(&mut sheet, frame, i64::from(i % cols * w), i64::from(i / cols * h));
    }
    let sheet = iced::widget::image::Handle::from_rgba(sheet.width(), sheet.height(), sheet.into_raw());
    images.insert(key.to_string(), Some(iced::widget::image::Handle::from_bytes(still)));
    animations.insert(key.to_string(), Animation { sheet, frame: (w, h), cols, count, delay_ms: delay_ms.max(20) });
}

/// GIF frames for the disk cache: the delay, then each frame length-prefixed.
fn pack_frames(delay_ms: u64, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (delay_ms as u32).to_le_bytes().to_vec();
    for frame in frames {
        out.extend((frame.len() as u32).to_le_bytes());
        out.extend(frame);
    }
    out
}

fn unpack_frames(bytes: &[u8]) -> Option<(u64, Vec<Vec<u8>>)> {
    let word = |b: &[u8]| b.get(..4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize);
    let delay = word(bytes)? as u64;
    let (mut rest, mut frames) = (&bytes[4..], Vec::new());
    while !rest.is_empty() {
        let len = word(rest)?;
        frames.push(rest.get(4..4 + len)?.to_vec());
        rest = &rest[4 + len..];
    }
    (!frames.is_empty()).then_some((delay, frames))
}

/// The image to draw for `key`: a GIF's current frame (cropped out of its
/// sprite sheet), else the downloaded picture.
fn drawn_image(session: &Session, key: &str) -> Option<iced::widget::Image<iced::widget::image::Handle>> {
    if let Some(anim) = session.animations.get(key) {
        let i = (session.anim_ms / anim.delay_ms) as u32 % anim.count;
        let (w, h) = anim.frame;
        let region = iced::Rectangle { x: i % anim.cols * w, y: i / anim.cols * h, width: w, height: h };
        return Some(iced::widget::image(anim.sheet.clone()).crop(region));
    }
    session.images.get(key)?.as_ref().map(|handle| iced::widget::image(handle.clone()))
}

/// Whether `key` still needs downloading into `map`: false when it's there
/// already or was on disk (loaded now); otherwise it's marked loading.
fn wants(map: &mut HashMap<String, Option<iced::widget::image::Handle>>, kind: cache::Kind, key: &str) -> bool {
    if map.contains_key(key) {
        return false;
    }
    let cached = cache::get(kind, key).map(iced::widget::image::Handle::from_bytes);
    let missing = cached.is_none();
    map.insert(key.to_string(), cached);
    missing
}

/// A downloaded Discord picture (from `load_snapshot_media`), if there is one.
fn web_picture<'a>(session: &'a Session, url: Option<&str>) -> Option<&'a iced::widget::image::Handle> {
    session.avatars.get(url?)?.as_ref()
}

/// Asks the hidden Discord client for the timeline's web avatars and images
/// that aren't cached or loading; they arrive as `Event::Media`.
fn load_web_media(session: &mut Session, clients: &Clients) {
    for message in &session.timeline {
        if let Some(url) = message.sender_avatar.as_ref().and_then(MediaRef::web_url)
            && wants(&mut session.avatars, cache::Kind::Avatar, url)
        {
            clients.fetch_media(url);
        }
        if let MessageKind::Image(info) = &message.kind
            && let Some(url) = info.media.web_url()
        {
            if url.starts_with("wa-gif:") {
                if !session.images.contains_key(url) {
                    match cache::get(cache::Kind::Media, url).and_then(|b| unpack_frames(&b)) {
                        Some((delay, frames)) => show_frames(&mut session.images, &mut session.animations, url, delay, frames),
                        None => {
                            session.images.insert(url.to_string(), None);
                            clients.fetch_media(url);
                        }
                    }
                }
            } else if wants(&mut session.images, cache::Kind::Media, url) {
                clients.fetch_media(url);
            }
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
    /// The native Discord sign-in form.
    DcSignInLogin(String),
    DcSignInPassword(String),
    DcSignInCode(String),
    DcSignInSubmit,
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
    /// A fire-and-forget task finished; nothing to do.
    Noop,
    ToggleEmoji,
    InsertEmoji(&'static str),
    /// Open the native file picker for an attachment.
    PickAttachment,
    /// The picker finished (None if cancelled or unreadable).
    AttachmentPicked(Option<Attachment>),
    /// Ctrl+V: attach the clipboard's image, if it holds one (text pastes
    /// into the input as usual).
    PasteImage,
    ClearAttachment,
    /// A web client's file send finished.
    AttachmentSent(Result<(), String>),
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
    AvatarLoaded(String, Result<Vec<u8>, String>),
    /// Click on a reaction pill: add ours, or take it back (`mine`).
    ToggleReaction {
        event_id: OwnedEventId,
        key: String,
        mine: Option<OwnedEventId>,
    },
    ReactionDone(Result<(), String>),
    OpenSettings(SettingsPage),
    Discord(mesh_discord::Event),
    WhatsApp(whatsapp::Event),
    /// Join a Discord voice channel, or call a Discord DM.
    DiscordJoin(String),
    DiscordLeave,
    /// Camera on or off in the Discord call.
    ToggleVideo,
    /// Share our screen in the Discord call, or stop.
    ToggleScreenShare,
    /// Watch (true) or stop watching someone's screen share, by their id.
    WatchStream(String, bool),
    /// Call the open WhatsApp chat (with our camera on if true).
    WaCallStart(bool),
    /// Answer WhatsApp's ringing call (with our camera on if true).
    WaCallAccept(bool),
    WaCallDecline,
    WaCallEnd,
    WaCallMute,
    WaCallCamera,
    WaCallScreen,
    /// Drop video from cameras that stopped sending.
    VideoPrune,
    /// Show a Discord server (by id) from the hidden client.
    GuildSelected(String),
    /// Expand or collapse a rail folder.
    ToggleFolder(Network),
    /// Show the WhatsApp groups list.
    ShowWhatsAppGroups,
    HomeFilter(Option<Network>),
    /// Start the hidden Discord client with `discord_engine_command`.
    StartDiscord,
    /// Accounts page: show this network (by registry id) in the setup panel.
    AccountsPick(&'static str),
    /// Open the Accounts page on a network (the top bar's status dots).
    AccountsOpen(&'static str),
    /// Pick a network and start signing it in again.
    AccountsReconnect(&'static str),
    AccountsStep(usize),
    AccountsDone,
    /// Unlink a network (asks once, then does it).
    SignOut(Network),
    /// Flip a sync switch: network id, item key.
    ToggleSync(&'static str, &'static str),
    /// Show a Discord text channel or DM in the chat pane.
    WebChatSelected(WebChat),
    WindowFocused(bool),
    OpenJoinServer,
    JoinServerLink(String),
    /// Open a link from a message in the default browser.
    OpenUrl(String),
    /// The mouse entered / left a message (by web id).
    HoverMessage(String),
    UnhoverMessage(String),
    /// Open or close a message's quick-reaction strip.
    ToggleReactPicker(String),
    /// React to a web message; "" takes ours back.
    React { id: String, emoji: String },
    /// Reply to a web message with the next send.
    ReplyTo(String, mesh_core::Quote),
    CancelReply,
    /// Play or pause a voice note (by media key).
    VoiceToggle(String),
    /// Play a voice note from this far in (0–1).
    VoiceSeek(String, f32),
    /// Cycle voice note speed: 1× → 1.5× → 2×.
    VoiceSpeed,
    /// A voice note sped up (off the UI thread), ready to play from `from`.
    VoiceStretched { key: String, speed: f32, samples: Arc<Vec<i16>>, from: usize },
    /// Advances GIFs and the voice player's progress.
    MediaTick,
    /// Show an image full screen: (its media key, a bigger copy's key).
    OpenImage(String, Option<String>),
    CloseLightbox,
    /// Open the new WhatsApp chat picker.
    NewChatOpen,
    NewChatQuery(String),
    /// Open the chat with this contact id or phone number.
    NewChatStart(String),
    /// Show or hide archived WhatsApp chats.
    ToggleArchived,
    /// Archive (true) or unarchive a WhatsApp chat.
    WaArchive(String, bool),
    /// Ask before deleting a WhatsApp chat, then do or don't.
    WaDeleteAsk(String),
    WaDeleteConfirm,
    WaDeleteCancel,
    /// Open the forward picker for a web message.
    ForwardStart(String),
    ForwardQuery(String),
    ForwardTo(String),
    /// Download a message attachment (by its media key) and open it in the
    /// OS's default app.
    OpenMedia(String),
    /// Accept the pasted invite through the hidden Discord client.
    JoinServerSubmit,
    DensityChosen(theme::Density),
    ToggleExtra(Extra),
    /// Copy the generated theme.toml to the clipboard.
    ExportTheme,
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

    let whatsapp_native = whatsapp_native();
    let discord_native = discord_native();
    // Start the hidden Discord client (once) if it isn't running yet, unless the
    // native helper has replaced it.
    if discord_native.is_none()
        && let (Some(socket), Some(cmd)) = (discord_socket(&config), &config.discord_engine_command)
        && !mesh_discord::is_running(&socket)
        && let Err(err) = run_command(cmd)
    {
        tracing::warn!("could not start the Discord engine ({cmd}): {err}");
    }
    // The engine command opens Discord's window only (Edge drops the other
    // URL), so open each linked engine site's tab once the browser's up.
    if let Some(socket) = discord_socket(&config) {
        let sites: Vec<_> = config
            .linked
            .iter()
            .filter_map(|id| mesh_core::networks::spec(id))
            .filter(|spec| !(spec.id == "wa" && whatsapp_native.is_some()))
            .filter(|spec| !(spec.id == "dc" && discord_native.is_some()))
            .filter_map(|spec| Some((spec.engine?, spec.login_url?)))
            .collect();
        std::thread::spawn(move || {
            if sites.is_empty() {
                return;
            }
            for _ in 0..30 {
                if mesh_discord::is_running(&socket) {
                    // Let a browser that's just starting list its own tabs first.
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    for (host, url) in sites {
                        if let Err(err) = mesh_discord::open(&socket, host, url, false) {
                            tracing::warn!("could not open {host} in the engine: {err}");
                        }
                    }
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }

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
            theme_reloaded: Local::now().format("%H:%M").to_string(),
            discord_socket: discord_socket(&config),
            discord_command: config.discord_engine_command.clone(),
            whatsapp_native: whatsapp_native.clone(),
            discord_native,
            dc_sign_in: accounts::DcSignIn::default(),
            discord: None,
            discord_state: None,
            whatsapp: None,
            whatsapp_state: None,
            login_qr: HashMap::new(),
            discord_shown: None,
            whatsapp_shown: None,
            linked: config.linked.clone(),
            sync_off: config.sync_off.clone(),
            accounts: accounts::Panel::default(),
            video: HashMap::new(),
            wa_call: None,
            window_focused: true,
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
    // Discord often connects before the session exists; catch up on its pictures.
    if let (Screen::LoggedIn(session), Some(snapshot), Some(engine)) =
        (&mut state.screen, &state.discord_state, &state.discord)
    {
        load_snapshot_media(session, snapshot, engine);
    }
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

/// Starts thumbnail downloads for avatars that aren't cached or loading.
fn load_avatars(session: &mut Session, media: impl IntoIterator<Item = MediaRef>) -> Task<Message> {
    let mut tasks = Vec::new();
    for media in media {
        let key = media.key();
        // Web media (Discord) comes through the hidden client instead.
        if media.web_url().is_some() || !wants(&mut session.avatars, cache::Kind::Avatar, &key) {
            continue;
        }
        let client = session.client.clone();
        tasks.push(Task::perform(
            async move { client.avatar_thumbnail(&media).await.map_err(|e| e.to_string()) },
            move |result| Message::AvatarLoaded(key.clone(), result),
        ));
    }
    Task::batch(tasks)
}

/// A downloaded avatar, if there is one.
fn picture<'a>(session: &'a Session, media: Option<&MediaRef>) -> Option<&'a iced::widget::image::Handle> {
    session.avatars.get(&media?.key())?.as_ref()
}

/// Starts downloads for the timeline's images that aren't cached or loading.
fn load_images(session: &mut Session) -> Task<Message> {
    let mut tasks = Vec::new();
    for message in &session.timeline {
        let MessageKind::Image(info) = &message.kind else {
            continue;
        };
        let key = info.media.key();
        if info.media.web_url().is_some() || !wants(&mut session.images, cache::Kind::Media, &key) {
            continue;
        }
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
                let avatars: Vec<MediaRef> = session.rooms.iter().filter_map(|r| r.avatar.clone()).collect();
                let avatars = load_avatars(session, avatars);
                if let Some(reload) = reload {
                    return Task::batch([reload, avatars]);
                }
                return avatars;
            }
            Task::none()
        }
        Message::RoomSelected(room_id) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.selected_room = Some(room_id.clone());
            if let Some(chat) = session.web_chat.take() {
                Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() }.close(&chat);
            }
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
                focus_composer(),
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
                        let avatars: Vec<MediaRef> =
                            session.timeline.iter().filter_map(|m| m.sender_avatar.clone()).collect();
                        let avatars = load_avatars(session, avatars);
                        // Opening a room marks it read up to its newest event.
                        let mut tasks = vec![avatars];
                        if !plain {
                            tasks.push(load_images(session));
                        }
                        if let Some(event_id) = session.timeline.iter().rev().find_map(|m| m.event_id.clone()) {
                            let client = session.client.clone();
                            tasks.push(Task::perform(
                                async move { client.mark_read(&room_id, &event_id).await },
                                |()| Message::Noop,
                            ));
                        }
                        return Task::batch(tasks);
                    }
                    Err(err) => session.status = Some(format!("Failed to load messages: {err}")),
                }
            }
            Task::none()
        }
        Message::AvatarLoaded(key, result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                match result {
                    Ok(bytes) => {
                        cache::put(cache::Kind::Avatar, &key, &bytes);
                        session
                            .avatars
                            .insert(key, Some(iced::widget::image::Handle::from_bytes(bytes)));
                    }
                    // Keep the `None` so it isn't retried every refresh; initials show instead.
                    Err(err) => tracing::debug!("avatar download failed: {err}"),
                }
            }
            Task::none()
        }
        Message::ImageLoaded(key, result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                match result {
                    Ok(bytes) => {
                        cache::put(cache::Kind::Media, &key, &bytes);
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
                MeshUpdate::Notify { room_id, title, body } => {
                    if !(state.window_focused && session.selected_room.as_ref() == Some(&room_id)) {
                        notify(title, body);
                    }
                    Task::none()
                }
            }
        }
        Message::WindowFocused(focused) => {
            state.window_focused = focused;
            Task::none()
        }
        Message::Noop => Task::none(),
        Message::DraftChanged(value) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.message_draft = value;
            }
            Task::none()
        }
        Message::ToggleEmoji => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.emoji_open = !session.emoji_open;
            }
            Task::none()
        }
        Message::PickAttachment => Task::perform(
            async {
                let file = rfd::AsyncFileDialog::new().pick_file().await?;
                let data = file.read().await;
                let filename = file.file_name();
                let mime = mime_guess_from(&filename);
                Some(Attachment { filename, mime, data })
            },
            Message::AttachmentPicked,
        ),
        Message::PasteImage => match &state.screen {
            Screen::LoggedIn(session) if session.selected_room.is_some() || session.web_chat.is_some() => {
                Task::perform(async { clipboard_image() }, |image| match image {
                    Some(attachment) => Message::AttachmentPicked(Some(attachment)),
                    None => Message::Noop,
                })
            }
            _ => Task::none(),
        },
        Message::AttachmentPicked(attachment) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.pending_attachment = attachment;
            }
            Task::none()
        }
        Message::ClearAttachment => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.pending_attachment = None;
            }
            Task::none()
        }
        Message::AttachmentSent(result) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.sending_attachment = false;
                match result {
                    Ok(()) => {
                        session.pending_attachment = None;
                        session.message_draft.clear();
                    }
                    Err(err) => session.status = Some(format!("Couldn't send the file: {err}")),
                }
            }
            Task::none()
        }
        Message::InsertEmoji(emoji) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.message_draft.push_str(emoji);
            }
            // Keep the picker open for several in a row; back to typing.
            iced::widget::operation::focus(iced::widget::Id::new(COMPOSER_INPUT))
        }
        Message::SendMessage => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            // A chosen file sends with the draft as its caption (caption may
            // be empty). Routed to the chat's own backend.
            if let Some(attachment) = session.pending_attachment.clone() {
                if session.sending_attachment {
                    return Task::none();
                }
                let caption = session.message_draft.clone();
                match &session.web_chat {
                    Some(chat) if chat.network == Network::WhatsApp => match &state.whatsapp {
                        Some(engine) => {
                            engine.send_file(&chat.id, &attachment.filename, &attachment.mime, &attachment.data, &caption);
                            session.sending_attachment = true;
                        }
                        None => session.status = Some("WhatsApp isn't running, so this can't be sent.".to_string()),
                    },
                    Some(chat) if chat.network == Network::Discord => {
                        session.status = Some("Sending files to Discord isn't ready yet — only WhatsApp and Matrix.".to_string());
                        let _ = chat;
                    }
                    _ => {
                        let Some(room_id) = session.selected_room.clone() else {
                            session.status = Some("Pick a chat first.".to_string());
                            return Task::none();
                        };
                        session.sending_attachment = true;
                        let client = session.client.clone();
                        return Task::perform(
                            async move {
                                client
                                    .send_attachment(
                                        &room_id,
                                        &attachment.filename,
                                        &attachment.mime,
                                        attachment.data,
                                        Some(&caption),
                                    )
                                    .await
                                    .map_err(|e| e.to_string())
                            },
                            Message::AttachmentSent,
                        );
                    }
                }
                return Task::none();
            }
            if session.message_draft.trim().is_empty() {
                return Task::none();
            }
            // A web chat sends through its hidden client; the echo comes back
            // with its messages.
            if let Some(chat) = &session.web_chat {
                let reply_to = session.replying.as_ref().map(|(id, _)| id.as_str());
                if (Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() }).send(chat, &session.message_draft, reply_to) {
                    session.message_draft.clear();
                    session.replying = None;
                } else {
                    session.status = Some("That chat's client isn't running, so this can't be sent.".to_string());
                }
                return Task::none();
            }
            let Some(room_id) = session.selected_room.clone() else {
                return Task::none();
            };
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
        Message::ToggleVideo => {
            if let (Some(engine), Some(snapshot)) = (&state.discord, &state.discord_state) {
                engine.set_video(!snapshot.voice.self_video);
            }
            Task::none()
        }
        Message::WaCallStart(video) => {
            if let (Some(engine), Screen::LoggedIn(session)) = (&state.whatsapp, &state.screen)
                && let Some(chat) = session.web_chat.as_ref().filter(|c| c.network == Network::WhatsApp)
            {
                engine.call_start(&chat.id, video);
            }
            Task::none()
        }
        Message::WaCallAccept(video) => {
            if let Some(engine) = &state.whatsapp {
                engine.call_accept(video);
            }
            Task::none()
        }
        Message::WaCallDecline | Message::WaCallEnd | Message::WaCallMute | Message::WaCallCamera | Message::WaCallScreen => {
            if let (Some(engine), Some(call)) = (&state.whatsapp, &state.wa_call) {
                match message {
                    Message::WaCallDecline => engine.call_decline(),
                    Message::WaCallEnd => engine.call_end(),
                    Message::WaCallMute => engine.call_mute(!call.muted),
                    Message::WaCallScreen => engine.call_screen(!call.screen),
                    _ => engine.call_camera(!call.camera),
                }
            }
            Task::none()
        }
        Message::ToggleScreenShare => {
            if let (Some(engine), Some(snapshot)) = (&state.discord, &state.discord_state) {
                engine.share_screen(!snapshot.voice.self_stream);
            }
            Task::none()
        }
        Message::WatchStream(owner, watch) => {
            if let Some(engine) = &state.discord {
                engine.watch_stream(&owner, watch);
            }
            Task::none()
        }
        Message::VideoPrune => {
            state.video.retain(|_, tile| tile.at.elapsed() < VIDEO_STALE);
            Task::none()
        }
        // In Discord voice, mute/deafen act on that call.
        Message::ToggleMute if in_discord_voice(state) => {
            if let Some(engine) = &state.discord {
                engine.toggle_mute();
            }
            Task::none()
        }
        Message::ToggleDeafen if in_discord_voice(state) => {
            if let Some(engine) = &state.discord {
                engine.toggle_deaf();
            }
            Task::none()
        }
        Message::DcSignInLogin(value) => {
            state.dc_sign_in.login = value;
            Task::none()
        }
        Message::DcSignInPassword(value) => {
            state.dc_sign_in.password = value;
            Task::none()
        }
        Message::DcSignInCode(value) => {
            state.dc_sign_in.code = value;
            Task::none()
        }
        Message::DcSignInSubmit => {
            let f = &mut state.dc_sign_in;
            if let Some(engine) = &state.discord {
                if f.asking_code {
                    engine.sign_in_code(f.code.trim());
                } else {
                    engine.sign_in(f.login.trim(), &f.password);
                }
                f.busy = true;
                f.error = None;
            }
            Task::none()
        }
        Message::Discord(event) => {
            match event {
                mesh_discord::Event::SignIn(result) => {
                    let f = &mut state.dc_sign_in;
                    f.busy = false;
                    match result {
                        mesh_discord::SignIn::Code => f.asking_code = true,
                        mesh_discord::SignIn::Failed { error } => f.error = Some(error),
                    }
                }
                mesh_discord::Event::Connected(engine) => {
                    if let Screen::LoggedIn(session) = &state.screen
                        && let Some(chat) = session.web_chat.as_ref().filter(|c| c.network == Network::Discord)
                    {
                        engine.open_channel(&chat.id);
                    }
                    state.discord = Some(engine);
                }
                mesh_discord::Event::Snapshot(snapshot) => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && let Some(engine) = &state.discord
                    {
                        load_snapshot_media(session, &snapshot, engine);
                    }
                    if snapshot.me.is_some() {
                        // Signed in: forget the form, password included.
                        state.dc_sign_in = accounts::DcSignIn::default();
                        state.login_qr.remove("dc");
                        accounts::remember_linked(state, Network::Discord);
                    }
                    let was_sharing = state.discord_state.as_ref().is_some_and(|s| s.voice.self_stream);
                    let shared = snapshot.voice.self_stream && !was_sharing;
                    state.discord_state = Some(snapshot);
                    accounts::refresh_shown(state);
                    // The share picker came up in the Discord window; once
                    // something's picked, Mesh goes back in front of it.
                    if shared {
                        return iced::window::latest().and_then(iced::window::gain_focus);
                    }
                }
                mesh_discord::Event::Qr(qr) => {
                    state.login_qr.insert("dc", qr.into());
                }
                mesh_discord::Event::Video(mesh_discord::Video::Frame { id, local, width, height, rgba }) => {
                    let frame = iced::widget::image::Handle::from_rgba(width, height, rgba);
                    state.video.insert(id, VideoTile { frame, local, at: std::time::Instant::now() });
                }
                mesh_discord::Event::Video(mesh_discord::Video::Ended { id }) => {
                    state.video.remove(&id);
                }
                mesh_discord::Event::Disconnected => {
                    state.video.retain(|id, _| id.starts_with("wa-"));
                    state.discord = None;
                    state.discord_state = None;
                    state.login_qr.remove("dc");
                    accounts::refresh_shown(state);
                }
                mesh_discord::Event::Messages(messages) => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && session.web_chat.as_ref().is_some_and(|c| c.id == messages.channel_id)
                    {
                        session.timeline = discord_timeline(messages);
                        load_web_media(session, &Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() });
                    }
                }
                mesh_discord::Event::Media { url, bytes } => return web_media_loaded(state, url, bytes),
                mesh_discord::Event::Notify(n) => {
                    let open = match &state.screen {
                        Screen::LoggedIn(session) => session.web_chat.as_ref().is_some_and(|c| c.id == n.channel_id),
                        _ => false,
                    };
                    if !(state.window_focused && open) {
                        notify(n.title, n.body);
                    }
                }
                mesh_discord::Event::Invite(result) => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && let Some(dialog) = &mut session.join_server
                    {
                        use mesh_discord::Invite;
                        dialog.busy = result == Invite::Waiting;
                        dialog.note = Some(match result {
                            Invite::Waiting => (
                                "Discord wants a captcha. Finish it in the Discord window that just opened.".to_string(),
                                false,
                            ),
                            Invite::Joined { guild } => (
                                format!("✓ Joined {}.", guild.unwrap_or_else(|| "the server".to_string())),
                                false,
                            ),
                            Invite::Failed { error } => (format!("✗ {error}"), true),
                        });
                    }
                }
            }
            Task::none()
        }
        Message::WhatsApp(event) => {
            match event {
                whatsapp::Event::Connected(engine) => {
                    if let Screen::LoggedIn(session) = &state.screen
                        && let Some(chat) = session.web_chat.as_ref().filter(|c| c.network == Network::WhatsApp)
                    {
                        engine.open_chat(&chat.id);
                    }
                    state.whatsapp = Some(engine);
                }
                whatsapp::Event::Snapshot(snapshot) => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && let Some(engine) = &state.whatsapp
                    {
                        for url in snapshot.chats.iter().chain(&snapshot.archived).filter_map(|c| c.avatar.as_ref()) {
                            if wants(&mut session.avatars, cache::Kind::Avatar, url) {
                                engine.fetch_media(url);
                            }
                        }
                    }
                    state.login_qr.remove("wa");
                    accounts::remember_linked(state, Network::WhatsApp);
                    state.whatsapp_state = Some(snapshot);
                    accounts::refresh_shown(state);
                }
                whatsapp::Event::Qr(qr) => {
                    state.login_qr.insert("wa", qr.into());
                }
                whatsapp::Event::Call(Ok(call)) => {
                    let call = call.filter(|c| c.phase != "ended");
                    let ringing = |c: &Option<whatsapp::Call>| c.as_ref().is_some_and(|c| c.phase == "incoming");
                    if ringing(&call) && !ringing(&state.wa_call)
                        && let Some(c) = &call
                    {
                        let kind = if c.video { "video call" } else { "call" };
                        notify(format!("WhatsApp {kind}"), format!("{} is calling", c.name));
                    }
                    if call.is_none() {
                        state.video.retain(|id, _| !id.starts_with("wa-"));
                    }
                    let sharing = |c: &Option<whatsapp::Call>| c.as_ref().is_some_and(|c| c.screen);
                    let shared = sharing(&call) && !sharing(&state.wa_call);
                    state.wa_call = call;
                    // The share picker came up in WhatsApp's window; once
                    // something's picked, Mesh goes back in front of it.
                    if shared {
                        return iced::window::latest().and_then(iced::window::gain_focus);
                    }
                }
                whatsapp::Event::Call(Err(error)) => {
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        session.status = Some(format!("WhatsApp: {error}"));
                    }
                }
                whatsapp::Event::Video(mesh_discord::Video::Frame { id, local, width, height, rgba }) => {
                    let frame = iced::widget::image::Handle::from_rgba(width, height, rgba);
                    state.video.insert(id, VideoTile { frame, local, at: std::time::Instant::now() });
                }
                whatsapp::Event::Video(mesh_discord::Video::Ended { id }) => {
                    state.video.remove(&id);
                }
                whatsapp::Event::Messages(messages) => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && session.web_chat.as_ref().is_some_and(|c| c.network == Network::WhatsApp && c.id == messages.chat_id)
                    {
                        for m in &messages.messages {
                            let key = format!("wa-thumb:{}", m.id);
                            if !session.images.contains_key(&key)
                                && let Some(jpeg) = m.file.as_ref().and_then(whatsapp::FileMeta::thumbnail)
                            {
                                session.images.insert(key, Some(iced::widget::image::Handle::from_bytes(jpeg)));
                            }
                        }
                        session.timeline = whatsapp_timeline(messages);
                        load_web_media(session, &Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() });
                    }
                }
                whatsapp::Event::Media { url, bytes } => return web_media_loaded(state, url, bytes),
                whatsapp::Event::Notify(n) => {
                    let open = match &state.screen {
                        Screen::LoggedIn(session) => session.web_chat.as_ref().is_some_and(|c| c.id == n.chat_id),
                        _ => false,
                    };
                    if !(state.window_focused && open) {
                        notify(n.title, n.body);
                    }
                }
                whatsapp::Event::Sent { result, .. } => return update(state, Message::AttachmentSent(result)),
                whatsapp::Event::File { id, filename, bytes } => {
                    // Kept for the day, so opening it again skips the download.
                    let outcome = bytes.and_then(|data| {
                        let path = cache::put_file(&whatsapp::file_url(&id), &safe_filename(&filename), &data)
                            .map_err(|e| e.to_string())?;
                        open_url(&path.to_string_lossy()).map_err(|e| e.to_string())
                    });
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        session.status = match outcome {
                            Ok(()) => None,
                            Err(err) => Some(format!("Couldn't open that: {err}")),
                        };
                    }
                }
                whatsapp::Event::Frames { url, delay_ms, frames } => {
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        match frames {
                            Ok(frames) if !frames.is_empty() => {
                                cache::put(cache::Kind::Media, &url, &pack_frames(delay_ms, &frames));
                                show_frames(&mut session.images, &mut session.animations, &url, delay_ms, frames);
                            }
                            other => {
                                // Out of the cache so the next timeline load retries.
                                tracing::warn!("gif {url}: {:?}", other.err());
                                session.images.remove(&url);
                            }
                        }
                    }
                }
                whatsapp::Event::Pcm { id, samples } => {
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        let key = whatsapp::file_url(&id);
                        let waiting = session.player.as_ref().is_some_and(|pl| pl.key == key && pl.loading);
                        match samples {
                            Ok(samples) => {
                                session.voice_pcm.insert(key.clone(), Arc::new(samples));
                                if waiting {
                                    return play_voice(session, key, 0);
                                }
                            }
                            Err(err) => {
                                session.status = Some(format!("Couldn't play that voice note: {err}"));
                                if waiting {
                                    session.player = None;
                                }
                            }
                        }
                    }
                }
                whatsapp::Event::Contacts { query, contacts } => {
                    if let Screen::LoggedIn(session) = &mut state.screen
                        && let Some(picker) = session.new_chat.as_mut().filter(|p| p.query == query)
                    {
                        picker.contacts = contacts;
                    }
                }
                whatsapp::Event::ChatStarted(result) => {
                    let Screen::LoggedIn(session) = &mut state.screen else {
                        return Task::none();
                    };
                    match result {
                        Ok(chat) => {
                            session.new_chat = None;
                            let open = WebChat { network: Network::WhatsApp, id: chat.id, name: chat.name, dm: !chat.group };
                            return update(state, Message::WebChatSelected(open));
                        }
                        Err(err) => {
                            if let Some(picker) = &mut session.new_chat {
                                picker.busy = false;
                                picker.error = Some(err);
                            }
                        }
                    }
                }
                whatsapp::Event::ChatAction { chat_id, result } => {
                    let Screen::LoggedIn(session) = &mut state.screen else {
                        return Task::none();
                    };
                    let deleted = session.deleting.as_ref() == Some(&chat_id);
                    if deleted {
                        session.deleting = None;
                    }
                    session.status = match result {
                        Ok(()) if deleted => Some("Chat deleted.".to_string()),
                        Ok(()) => None,
                        Err(err) => Some(format!("Couldn't do that: {err}")),
                    };
                    // A deleted chat that's open closes.
                    if deleted && session.status.as_deref() == Some("Chat deleted.") && session.web_chat.as_ref().is_some_and(|c| c.id == chat_id) {
                        if let Some(engine) = &state.whatsapp {
                            engine.close_chat();
                        }
                        session.web_chat = None;
                        session.timeline.clear();
                    }
                }
                whatsapp::Event::Forwarded(result) => {
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        session.status = Some(match result {
                            Ok(()) => "Forwarded.".to_string(),
                            Err(err) => format!("Couldn't forward: {err}"),
                        });
                    }
                }
                whatsapp::Event::Disconnected => {
                    state.wa_call = None;
                    state.video.retain(|id, _| !id.starts_with("wa-"));
                    state.whatsapp = None;
                    state.whatsapp_state = None;
                    state.login_qr.remove("wa");
                    accounts::refresh_shown(state);
                }
            }
            Task::none()
        }
        Message::OpenJoinServer => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.jump = None;
            session.join_server = Some(JoinServer::default());
            iced::widget::operation::focus(iced::widget::Id::new(INVITE_INPUT))
        }
        Message::JoinServerLink(link) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(dialog) = &mut session.join_server
            {
                dialog.link = link;
            }
            Task::none()
        }
        Message::JoinServerSubmit => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(dialog) = &mut session.join_server
                && !dialog.busy
                && !dialog.link.trim().is_empty()
            {
                match &state.discord {
                    Some(engine) => {
                        engine.accept_invite(dialog.link.trim());
                        dialog.busy = true;
                        dialog.note = None;
                    }
                    None => {
                        dialog.note = Some(("✗ Mesh's Discord client isn't running; see the README.".to_string(), true));
                    }
                }
            }
            Task::none()
        }
        Message::DiscordJoin(channel_id) => {
            match &state.discord {
                Some(engine) => engine.join(&channel_id),
                None => {
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        session.status = Some("Discord voice isn't running (open it with the headphones button).".to_string());
                    }
                }
            }
            Task::none()
        }
        Message::DiscordLeave => {
            if let Some(engine) = &state.discord {
                engine.leave();
            }
            Task::none()
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
                let task = load_avatars(session, members.iter().filter_map(|m| m.avatar.clone()));
                session.members = members;
                return task;
            }
            Task::none()
        }
        Message::ProfileLoaded(profile) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                let task = load_avatars(session, profile.avatar.clone());
                session.my_profile = Some(profile);
                return task;
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
                && let Some(voice) = &session.discord_voice
            {
                let (what, result) = match voice {
                    theme::DiscordVoice::Url(url) => (url, open_url(url)),
                    theme::DiscordVoice::Command(cmd) => (cmd, run_command(cmd)),
                };
                if let Err(err) = result {
                    session.status = Some(format!("Could not open {what}: {err}"));
                }
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
                if session.lightbox.is_some() {
                    session.lightbox = None;
                } else if session.new_chat.is_some() {
                    session.new_chat = None;
                } else if session.confirm_delete.is_some() {
                    session.confirm_delete = None;
                } else if session.add_friend.is_some() {
                    session.add_friend = None;
                } else if session.join_server.is_some() {
                    session.join_server = None;
                } else if session.jump.is_some() {
                    session.jump = None;
                } else if session.forwarding.is_some() {
                    session.forwarding = None;
                } else if session.reacting.is_some() {
                    session.reacting = None;
                } else if session.replying.is_some() {
                    session.replying = None;
                } else if session.emoji_open {
                    session.emoji_open = false;
                } else {
                    session.show_settings = false;
                }
            }
            Task::none()
        }
        Message::SpaceSelected(space) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.active_space = space;
                session.active_guild = None;
                session.active_wa_groups = false;
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::SpaceKey(n) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(group) = session.groups.get(n)
            {
                session.active_space = group.id().cloned();
                session.active_guild = None;
                session.active_wa_groups = false;
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::GuildSelected(guild) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.active_space = None;
                session.active_guild = Some(guild);
                session.active_wa_groups = false;
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::ToggleFolder(network) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                let open = &mut session.folders_open[folder_index(network)];
                *open = !*open;
            }
            Task::none()
        }
        Message::ShowWhatsAppGroups => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.active_space = None;
                session.active_guild = None;
                session.active_wa_groups = true;
                session.focused = Pane::Channels;
            }
            Task::none()
        }
        Message::HomeFilter(filter) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.home_filter = filter;
            }
            Task::none()
        }
        Message::WebChatSelected(channel) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let clients = Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() };
            if let Some(previous) = session.web_chat.as_ref().filter(|c| c.network != channel.network) {
                clients.close(previous);
            }
            if !clients.open(&channel) {
                let name = channel.network.spec().name;
                session.status = Some(format!("{name} isn't running."));
            }
            session.selected_room = None;
            session.web_chat = Some(channel);
            (session.hovered, session.reacting, session.replying) = (None, None, None);
            session.confirm_delete = None;
            session.focused = Pane::Chat;
            session.timeline.clear();
            session.members.clear();
            session.show_settings = false;
            focus_composer()
        }
        Message::StartDiscord => {
            if let Some(cmd) = &state.discord_command
                && let Err(err) = run_command(cmd)
                && let Screen::LoggedIn(session) = &mut state.screen
            {
                session.status = Some(format!("Couldn't start Discord: {err}"));
            }
            Task::none()
        }
        Message::AccountsOpen(id) => {
            let task = update(state, Message::OpenSettings(SettingsPage::Accounts));
            Task::batch([task, update(state, Message::AccountsPick(id))])
        }
        Message::AccountsPick(id) => {
            state.accounts = accounts::Panel { selected: Some(id), ..accounts::Panel::default() };
            if let Some(network) = Network::from_id(id)
                && matches!(accounts::link(state, network), accounts::Link::Offline)
            {
                return update(state, Message::AccountsReconnect(id));
            }
            Task::none()
        }
        Message::AccountsReconnect(id) => {
            state.accounts = accounts::Panel { selected: Some(id), linking: true, ..accounts::Panel::default() };
            if let Some(network) = Network::from_id(id)
                && let Err(err) = accounts::start_link(state, network)
                && let Screen::LoggedIn(session) = &mut state.screen
            {
                session.status = Some(format!("Couldn't open {}: {err}", network.spec().name));
            }
            Task::none()
        }
        Message::AccountsStep(step) => {
            state.accounts.step = step;
            Task::none()
        }
        Message::AccountsDone => {
            state.accounts.step = 0;
            state.accounts.linking = false;
            Task::none()
        }
        Message::SignOut(network) => {
            if !state.accounts.confirm_sign_out {
                state.accounts.confirm_sign_out = true;
                return Task::none();
            }
            state.accounts.confirm_sign_out = false;
            let id = network.spec().id;
            state.linked.retain(|l| l != id);
            let linked = state.linked.clone();
            persist(|c| c.linked = linked);
            match network {
                Network::Matrix => return update(state, Message::LogOut),
                Network::Discord => {
                    if let Some(engine) = &state.discord {
                        engine.sign_out();
                    }
                }
                Network::WhatsApp => {
                    if let Some(engine) = &state.whatsapp {
                        engine.sign_out();
                    }
                }
            }
            Task::none()
        }
        Message::ToggleSync(id, key) => {
            let entry = format!("{id}:{key}");
            if let Some(i) = state.sync_off.iter().position(|e| *e == entry) {
                state.sync_off.remove(i);
            } else {
                state.sync_off.push(entry);
            }
            let off = state.sync_off.clone();
            persist(|c| c.sync_off = off);
            accounts::refresh_shown(state);
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
            session.active_guild = None;
            session.active_wa_groups = false;
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
        Message::OpenUrl(url) => {
            if let Err(err) = open_url(&url) {
                tracing::warn!("open url {url}: {err}");
            }
            Task::none()
        }
        Message::HoverMessage(id) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.hovered = Some(id);
            }
            Task::none()
        }
        Message::UnhoverMessage(id) => {
            // Only if it's still this one: entering the next can come first.
            if let Screen::LoggedIn(session) = &mut state.screen
                && session.hovered.as_ref() == Some(&id)
            {
                session.hovered = None;
            }
            Task::none()
        }
        Message::ToggleReactPicker(id) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.reacting = if session.reacting.as_ref() == Some(&id) { None } else { Some(id) };
            }
            Task::none()
        }
        Message::React { id, emoji } => {
            if let Some(engine) = &state.whatsapp {
                engine.react(&id, &emoji);
            }
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.reacting = None;
            }
            Task::none()
        }
        Message::ReplyTo(id, quote) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.replying = Some((id, quote));
            }
            focus_composer()
        }
        Message::CancelReply => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.replying = None;
            }
            Task::none()
        }
        Message::VoiceToggle(key) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            let total = session.voice_pcm.get(&key).map_or(0, |pcm| pcm.len());
            if let Some(player) = session.player.as_mut().filter(|pl| pl.key == key) {
                if player.loading {
                    return Task::none();
                }
                if player.out.is_some() {
                    // Pause: stop the stream, remember where.
                    player.position = player.position();
                    player.out = None;
                    return Task::none();
                }
                let from = if player.position >= total { 0 } else { player.position };
                return play_voice(session, key, from);
            }
            session.player = None;
            if total > 0 {
                return play_voice(session, key, 0);
            } else if let Some(id) = key.strip_prefix("wa-file:")
                && let Some(engine) = &state.whatsapp
            {
                engine.voice_pcm(id);
                session.player = Some(Player { key, position: 0, out: None, loading: true });
            }
            Task::none()
        }
        Message::VoiceSeek(key, at) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            match session.voice_pcm.get(&key).map(|pcm| pcm.len()) {
                Some(total) => play_voice(session, key, (total as f32 * at.clamp(0.0, 1.0)) as usize),
                // Not decoded yet: start it from the top.
                None => update(state, Message::VoiceToggle(key)),
            }
        }
        Message::VoiceSpeed => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.voice_speed = match session.voice_speed {
                s if s < 1.25 => 1.5,
                s if s < 1.75 => 2.0,
                _ => 1.0,
            };
            // Carry on from the same spot at the new speed.
            match &session.player {
                Some(pl) if pl.out.is_some() => {
                    let (key, at) = (pl.key.clone(), pl.position());
                    play_voice(session, key, at)
                }
                _ => Task::none(),
            }
        }
        Message::VoiceStretched { key, speed, samples, from } => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            session.voice_pcm.insert(stretched_key(&key, speed), samples);
            let waiting = session.player.as_ref().is_some_and(|pl| pl.key == key && pl.loading);
            if waiting && session.voice_speed == speed {
                return play_voice(session, key, from);
            }
            Task::none()
        }
        Message::MediaTick => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.anim_ms += MEDIA_TICK_MS;
                // A finished voice note stops and rewinds.
                if let Some(player) = &mut session.player
                    && let Some(playing) = &player.out
                    && playing.left.lock().map(|l| l.is_empty()).unwrap_or(true)
                {
                    player.out = None;
                    player.position = 0;
                }
            }
            Task::none()
        }
        Message::OpenImage(key, full) => {
            let Screen::LoggedIn(session) = &mut state.screen else {
                return Task::none();
            };
            if let Some(full) = &full
                && wants(&mut session.images, cache::Kind::Media, full)
            {
                (Clients { discord: state.discord.as_ref(), whatsapp: state.whatsapp.as_ref() }).fetch_media(full);
            }
            session.lightbox = Some((key, full));
            Task::none()
        }
        Message::CloseLightbox => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.lightbox = None;
            }
            Task::none()
        }
        Message::NewChatOpen => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.new_chat = Some(NewChat::default());
            }
            iced::widget::operation::focus(iced::widget::Id::new(NEW_CHAT_INPUT))
        }
        Message::NewChatQuery(query) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(picker) = &mut session.new_chat
            {
                if let Some(engine) = &state.whatsapp {
                    engine.search_contacts(&query);
                }
                picker.query = query;
                picker.error = None;
            }
            Task::none()
        }
        Message::NewChatStart(who) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(picker) = &mut session.new_chat
            {
                match &state.whatsapp {
                    Some(engine) => {
                        engine.start_chat(&who);
                        picker.busy = true;
                    }
                    None => picker.error = Some("WhatsApp isn't running.".to_string()),
                }
            }
            Task::none()
        }
        Message::ToggleArchived => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.show_archived = !session.show_archived;
            }
            Task::none()
        }
        Message::WaArchive(chat_id, archive) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(engine) = &state.whatsapp
            {
                engine.set_archived(&chat_id, archive);
                session.status = Some(if archive { "Archiving…" } else { "Unarchiving…" }.to_string());
            }
            Task::none()
        }
        Message::WaDeleteAsk(chat_id) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.confirm_delete = Some(chat_id);
            }
            Task::none()
        }
        Message::WaDeleteCancel => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.confirm_delete = None;
            }
            Task::none()
        }
        Message::WaDeleteConfirm => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some(chat_id) = session.confirm_delete.take()
                && let Some(engine) = &state.whatsapp
            {
                engine.delete_chat(&chat_id);
                session.deleting = Some(chat_id);
                session.status = Some("Deleting…".to_string());
            }
            Task::none()
        }
        Message::ForwardStart(id) => {
            if let Screen::LoggedIn(session) = &mut state.screen {
                session.forwarding = Some((id, String::new()));
            }
            iced::widget::operation::focus(iced::widget::Id::new(FORWARD_INPUT))
        }
        Message::ForwardQuery(query) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some((_, filter)) = &mut session.forwarding
            {
                *filter = query;
            }
            Task::none()
        }
        Message::ForwardTo(chat_id) => {
            if let Screen::LoggedIn(session) = &mut state.screen
                && let Some((id, _)) = session.forwarding.take()
            {
                session.status = Some(match &state.whatsapp {
                    Some(engine) => {
                        engine.forward(&id, &chat_id);
                        "Forwarding…".to_string()
                    }
                    None => "WhatsApp isn't running, so this can't be forwarded.".to_string(),
                });
            }
            Task::none()
        }
        Message::OpenMedia(url) => {
            // WhatsApp attachments ("wa-file:<id>") are encrypted: its hidden
            // client decrypts them, then `Event::File` opens the file. Anything
            // else (a Discord CDN URL) opens straight in the browser.
            if let Some(path) = cache::file(&url) {
                if let Err(err) = open_url(&path.to_string_lossy()) {
                    tracing::warn!("open {}: {err}", path.display());
                }
            } else if let Some(id) = url.strip_prefix("wa-file:") {
                if let Some(engine) = &state.whatsapp {
                    engine.save_media(id);
                    if let Screen::LoggedIn(session) = &mut state.screen {
                        session.status = Some("Opening…".to_string());
                    }
                }
            } else if let Err(err) = open_url(&url) {
                tracing::warn!("open media {url}: {err}");
            }
            Task::none()
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
                    session.active_guild = None;
                    session.active_wa_groups = false;
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
        match (&state.discord_native, &state.discord_socket) {
            (Some(exe), _) => Subscription::run_with(exe.clone(), discord_native_events),
            (None, Some(socket)) => Subscription::run_with(socket.clone(), discord_events),
            (None, None) => Subscription::none(),
        },
        match (&state.whatsapp_native, &state.discord_socket) {
            (Some(exe), _) => Subscription::run_with(exe.clone(), whatsapp_native_events),
            (None, Some(socket)) => Subscription::run_with(socket.clone(), whatsapp_events),
            (None, None) => Subscription::none(),
        },
        // Drive the animated background at ~15 fps (only while it's on).
        if state.extras.digital_rain {
            iced::time::every(std::time::Duration::from_millis(66)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        },
        // GIF frames and the voice player's progress, only while one is showing.
        if media_moving(state) {
            iced::time::every(std::time::Duration::from_millis(MEDIA_TICK_MS)).map(|_| Message::MediaTick)
        } else {
            Subscription::none()
        },
        // Cameras that stop sending (no "ended" from a remote that just turns
        // its camera off) go away within a couple of seconds.
        if state.video.is_empty() {
            Subscription::none()
        } else {
            iced::time::every(std::time::Duration::from_millis(500)).map(|_| Message::VideoPrune)
        },
        // Hot-reload the rice: an mtime poll is cheap and works everywhere.
        iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::ThemeCheck),
        // Global shortcuts work even while an input has focus.
        iced::event::listen_with(|event, _status, _window| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                shortcut(key, modifiers)
            }
            iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
            _ => None,
        }),
    ])
}

/// Whether something on screen animates: a loaded GIF in the chat or the
/// full-screen view, or a voice note playing.
fn media_moving(state: &App) -> bool {
    let Screen::LoggedIn(session) = &state.screen else {
        return false;
    };
    let gif = |key: &str| session.animations.contains_key(key);
    session.player.as_ref().is_some_and(|pl| pl.out.is_some())
        || session.lightbox.as_ref().is_some_and(|(key, _)| gif(key))
        || session.timeline.iter().any(|m| matches!(&m.kind, MessageKind::Image(info) if gif(&info.media.key())))
}

/// A desktop notification, shown off the UI thread (the D-Bus / WinRT call
/// can block for a moment). ponytail: on Windows it shows under PowerShell's
/// name; give the Start Menu shortcut an AppUserModelID and pass it to
/// `app_id` for "Mesh".
fn notify(title: String, body: String) {
    std::thread::spawn(move || {
        if let Err(err) = notify_rust::Notification::new().appname("Mesh").summary(&title).body(&body).show() {
            tracing::warn!("notification failed: {err}");
        }
    });
}

/// Where Mesh finds the hidden Discord client: the config, else the
/// launcher's default socket on Linux, or Edge's DevTools port on Windows.
fn discord_socket(config: &theme::Config) -> Option<PathBuf> {
    config.discord_engine_socket.as_ref().map(PathBuf::from).or_else(|| {
        if cfg!(windows) {
            Some(PathBuf::from("127.0.0.1:9222"))
        } else {
            std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("discord-engine.sock"))
        }
    })
}

fn in_discord_voice(state: &App) -> bool {
    state.discord_state.as_ref().is_some_and(|s| s.voice.connected())
}

/// Streams the hidden Discord client's events (reconnecting by itself).
fn discord_events(socket: &PathBuf) -> impl iced::futures::Stream<Item = Message> + use<> {
    let socket = socket.clone();
    iced::stream::channel(64, async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(mesh_discord::run(socket, tx));
        while let Some(event) = rx.recv().await {
            if output.send(Message::Discord(event)).await.is_err() {
                break;
            }
        }
    })
}

/// Streams WhatsApp Web's events from the same engine (reconnecting by itself).
fn whatsapp_events(socket: &PathBuf) -> impl iced::futures::Stream<Item = Message> + use<> {
    let socket = socket.clone();
    iced::stream::channel(64, async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(whatsapp::run(socket, tx));
        while let Some(event) = rx.recv().await {
            if output.send(Message::WhatsApp(event)).await.is_err() {
                break;
            }
        }
    })
}

/// `mesh-wa` next to Mesh's own binary, if it was installed with it.
fn whatsapp_native() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name(if cfg!(windows) { "mesh-wa.exe" } else { "mesh-wa" });
    exe.is_file().then_some(exe)
}

/// `mesh-dc` next to Mesh's own binary, if it was installed with it.
fn discord_native() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name(if cfg!(windows) { "mesh-dc.exe" } else { "mesh-dc" });
    exe.is_file().then_some(exe)
}

/// Streams the native Discord helper's events (restarting it by itself).
fn discord_native_events(exe: &PathBuf) -> impl iced::futures::Stream<Item = Message> + use<> {
    let exe = exe.clone();
    iced::stream::channel(64, async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(mesh_discord::run_native(exe, data_dir().join("discord"), tx));
        while let Some(event) = rx.recv().await {
            if output.send(Message::Discord(event)).await.is_err() {
                break;
            }
        }
    })
}

/// Streams the native WhatsApp helper's events (restarting it by itself).
fn whatsapp_native_events(exe: &PathBuf) -> impl iced::futures::Stream<Item = Message> + use<> {
    let exe = exe.clone();
    iced::stream::channel(64, async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(whatsapp::run_native(exe, data_dir().join("whatsapp"), tx));
        while let Some(event) = rx.recv().await {
            if output.send(Message::WhatsApp(event)).await.is_err() {
                break;
            }
        }
    })
}

fn shortcut(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    match key.as_ref() {
        keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Message::Escape),
        keyboard::Key::Character("k") if modifiers.control() => Some(Message::OpenJump),
        keyboard::Key::Character("v") if modifiers.control() => Some(Message::PasteImage),
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
            settings_shell(session, state, size.width)
        } else {
            // What the Accounts sync switches leave in.
            let (discord, whatsapp) = (accounts::shown_discord(state), accounts::shown_whatsapp(state));
            let rail = rail_view(session, discord, whatsapp, state.discord_command.is_some(), p, l, x.show_network_tags);
            let mut panes = row![
                tile(session, Pane::Rail, rail, 64.0 + 2.0 * l.border_size, p, l, x),
                tile(session, Pane::Channels, channel_pane(session, discord, whatsapp, p, l), 248.0, p, l, x),
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
        let guild = if session.active_wa_groups {
            Some("WhatsApp groups")
        } else {
            active_guild(session, state.discord_state.as_ref()).map(|g| g.name.as_str())
        };
        let shell = column![top_bar(session, guild, accounts::link(state, Network::Discord), accounts::link(state, Network::WhatsApp), p, l, x.translucent_panes), panes]
            .spacing(l.gaps_out)
            .padding(l.gaps_out);
        if let Some((shown, full)) = &session.lightbox {
            stack![shell, lightbox_view(session, shown, full.as_deref(), p)].into()
        } else if let Some(dialog) = &session.add_friend {
            stack![shell, add_friend_view(session, dialog, p, l)].into()
        } else if let Some(dialog) = &session.join_server {
            stack![shell, join_server_view(dialog, state.discord.is_some(), p, l)].into()
        } else if session.jump.is_some() {
            stack![shell, jump_palette(session, p, l)].into()
        } else if let Some(picker) = &session.new_chat {
            stack![shell, new_chat_picker(picker, p, l)].into()
        } else if let Some((_, query)) = &session.forwarding {
            stack![shell, forward_picker(query, state.whatsapp_state.as_ref(), p, l)].into()
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

/// An image full screen: the bigger copy once it's in, the chat's meanwhile.
/// Click anywhere or press Esc to close.
fn lightbox_view<'a>(session: &'a Session, shown: &str, full: Option<&str>, p: &theme::Palette) -> Element<'a, Message> {
    let big = full.and_then(|key| drawn_image(session, key));
    let still_loading = full.is_some() && big.is_none();
    let picture: Element<'a, Message> = match big.or_else(|| drawn_image(session, shown)) {
        Some(image) => image
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(iced::ContentFit::Contain)
            .into(),
        None => ui::mono("loading…", 12.0, p.text_muted()).into(),
    };
    let hint = if still_loading { "loading full size… · esc to close" } else { "esc to close" };
    let scrim = theme::alpha(p.background(), 0.92);
    mouse_area(
        container(column![picture, ui::mono(hint, 11.0, p.text_muted())].spacing(8).align_x(iced::Alignment::Center))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(32)
            .center(Length::Fill)
            .style(move |_| container::Style {
                background: Some(scrim.into()),
                ..container::Style::default()
            }),
    )
    .on_press(Message::CloseLightbox)
    .into()
}

/// Whether the new-chat input is a phone number rather than a name.
fn looks_like_number(query: &str) -> bool {
    let digits = query.chars().filter(char::is_ascii_digit).count();
    digits >= 7 && query.trim().chars().all(|c| c.is_ascii_digit() || " +-()".contains(c))
}

/// Start a WhatsApp chat: search your contacts by name or number, or type a
/// phone number to message someone who isn't one.
fn new_chat_picker<'a>(picker: &'a NewChat, p: &theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    let number = looks_like_number(&picker.query).then(|| picker.query.trim().to_string());
    let first = number.clone().or_else(|| picker.contacts.first().map(|c| c.id.clone()));
    let mut list = column![
        text_input("new WhatsApp chat: a name or a phone number…", &picker.query)
            .id(iced::widget::Id::new(NEW_CHAT_INPUT))
            .on_input(Message::NewChatQuery)
            .on_submit_maybe(first.filter(|_| !picker.busy).map(Message::NewChatStart))
            .padding(10)
            .size(14),
    ]
    .spacing(2);
    if picker.busy {
        list = list.push(container(ui::mono("opening…", 12.0, p.text_muted())).padding(8));
    }
    if let Some(error) = &picker.error {
        list = list.push(container(ui::mono(error, 12.0, p.danger())).padding(8));
    }
    let mut first_row = true;
    if let Some(number) = number {
        let line = row![
            ui::mono("+", 13.0, p.text_muted()),
            ui::sans(format!("Message {number} on WhatsApp"), 14.0, Weight::Normal, p.text_bright()).width(Length::Fill),
            ui::mono("Enter", 11.0, p.text_muted()),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        list = list.push(ui::row_button(line, Message::NewChatStart(number), true, p, l));
        first_row = false;
    }
    for contact in &picker.contacts {
        let mut line = row![
            ui::mono("@", 13.0, p.text_muted()),
            ui::sans(&contact.name, 14.0, Weight::Normal, p.text_bright()).width(Length::Fill),
            ui::mono(format!("+{}", contact.number), 11.0, p.text_muted()),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if first_row {
            line = line.push(ui::mono("Enter", 11.0, p.text_muted()));
        }
        list = list.push(ui::row_button(line, Message::NewChatStart(contact.id.clone()), first_row, p, l));
        first_row = false;
    }
    if picker.query.trim().is_empty() {
        list = list.push(container(ui::mono("type a contact's name, or a number like 082 123 4567", 12.0, p.text_muted())).padding(8));
    } else if first_row && !picker.busy && picker.error.is_none() {
        list = list.push(container(ui::mono("no contact by that name", 12.0, p.text_muted())).padding(8));
    }
    let scrim = theme::alpha(p.background(), 0.6);
    mouse_area(
        container(iced::widget::opaque(ui::pane(list.padding(10), true, p, l, false).max_width(560)))
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

/// Pick a WhatsApp chat to forward a message to; type to filter.
fn forward_picker<'a>(
    query: &'a str,
    snapshot: Option<&'a whatsapp::Snapshot>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let needle = query.to_lowercase();
    let matches: Vec<&whatsapp::Chat> = snapshot
        .map(|s| s.chats.iter().filter(|c| c.name.to_lowercase().contains(&needle)).take(12).collect())
        .unwrap_or_default();
    let mut list = column![
        text_input("forward to…", query)
            .id(iced::widget::Id::new(FORWARD_INPUT))
            .on_input(Message::ForwardQuery)
            .on_submit_maybe(matches.first().map(|c| Message::ForwardTo(c.id.clone())))
            .padding(10)
            .size(14),
    ]
    .spacing(2);
    for (i, chat) in matches.iter().enumerate() {
        let mut line = row![
            ui::mono(if chat.group { "#" } else { "@" }, 13.0, p.text_muted()),
            ui::sans(&chat.name, 14.0, Weight::Normal, p.text_bright()).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if i == 0 {
            line = line.push(ui::mono("Enter", 11.0, p.text_muted()));
        }
        list = list.push(ui::row_button(line, Message::ForwardTo(chat.id.clone()), i == 0, p, l));
    }
    if matches.is_empty() {
        list = list.push(container(ui::mono("no match", 12.0, p.text_muted())).padding(8));
    }
    let scrim = theme::alpha(p.background(), 0.6);
    mouse_area(
        container(iced::widget::opaque(ui::pane(list.padding(10), true, p, l, false).max_width(560)))
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
    p.network(network.spec())
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

fn active_guild<'a>(session: &Session, discord: Option<&'a mesh_discord::Snapshot>) -> Option<&'a mesh_discord::Guild> {
    let id = session.active_guild.as_deref()?;
    discord?.guilds.iter().find(|g| g.id == id)
}

/// The Discord server a bridged space stands for, from its portals.
fn guild_of(group: &SpaceGroup) -> Option<String> {
    group.rooms.iter().find_map(|r| r.bridged.as_ref()?.guild_id.clone())
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
    guild: Option<&str>,
    discord: accounts::Link,
    whatsapp: accounts::Link,
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
        guild.unwrap_or_else(|| active_group(session).map_or("Home", |g| g.name())).to_lowercase(),
        12.0,
        p.text_muted(),
    ));
    if let Some(room) = selected_room(session) {
        crumb = crumb
            .push(ui::mono("/", 12.0, p.text_muted()))
            .push(ui::mono(room_label(room), 12.0, p.text_bright()));
    }

    // Compact per-platform status dots: green = fully connected, yellow =
    // partly (or waiting to be linked), red = off. Clicking opens that
    // network on the Accounts page, which starts linking it if it's down.
    let mut chips = row![].spacing(6).align_y(iced::Alignment::Center);
    // Matrix: logged in or not.
    let mx = session.my_profile.as_ref().map_or(Health::Off, |p| {
        let _ = p;
        Health::On
    });
    chips = chips.push(connection_chip(Network::Matrix, mx, p));
    // Discord: bridge (text in Matrix) + the hidden client (voice, servers).
    let bridge = session.rooms.iter().any(|r| r.network == Network::Discord);
    let health = |link: &accounts::Link| match link {
        accounts::Link::Linked { .. } => Health::On,
        accounts::Link::SignedOut | accounts::Link::Connecting => Health::Partial,
        accounts::Link::Offline => Health::Off,
    };
    let dc = match (bridge, health(&discord)) {
        (false, Health::On) | (true, Health::Off) => Health::Partial,
        (_, h) => h,
    };
    chips = chips.push(connection_chip(Network::Discord, dc, p));
    // WhatsApp: the hidden WhatsApp Web tab.
    chips = chips.push(connection_chip(Network::WhatsApp, health(&whatsapp), p));

    let (fill, edge, radius) = (p.background(), p.border_ctrl(), l.control_radius());
    let search = button(
        row![
            ui::icon(ui::Icon::Search, 14.0, p.text_muted()),
            ui::mono("search or jump — rooms, spaces", 12.0, p.text_muted()).width(Length::Fill),
            ui::outline_chip(ui::mono("Ctrl K", 11.0, p.text_muted()), p),
        ]
        .spacing(8)
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
                row![
                    ui::icon(ui::Icon::UserPlus, 15.0, p.accent_text()),
                    ui::sans("add friend", 13.0, Weight::Semibold, p.accent_text()),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
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
/// A platform's connection health, shown as a dot colour.
#[derive(Clone, Copy)]
enum Health {
    On,
    Partial,
    Off,
}

/// A compact, clickable platform status: a dot (green/yellow/red) and its tag.
fn connection_chip<'a>(
    network: Network,
    health: Health,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let dot = match health {
        Health::On => p.success(),
        Health::Partial => p.warning(),
        Health::Off => p.danger(),
    };
    let (fill, edge, radius) = (p.panel_dim(), p.border_ctrl(), 999.0);
    button(
        row![ui::swatch(dot, 7.0, 7.0, 3.5), ui::mono_bold(network.tag(), 12.0, net_color(network, p))]
            .spacing(6)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Message::AccountsOpen(network.spec().id))
    .padding([5, 10])
    .style(move |_, status| button::Style {
        background: Some(if matches!(status, button::Status::Hovered) { edge } else { fill }.into()),
        border: iced::border::rounded(radius).color(edge).width(1),
        ..button::Style::default()
    })
    .into()
}

/// The space rail: Home, then (later) one tile per space, then "+".
/// The rail: Home, then a Discord folder (the hidden client's servers) and a
/// Matrix folder (spaces), each expanding in place like Discord's folders.
fn rail_view<'a>(
    session: &'a Session,
    discord: Option<&'a mesh_discord::Snapshot>,
    whatsapp: Option<&'a whatsapp::Snapshot>,
    can_start_discord: bool,
    p: &theme::Palette,
    l: &theme::Layout,
    tags: bool,
) -> Element<'a, Message> {
    let home = rail_tile(
        ui::icon(ui::Icon::Home, 20.0, p.text_bright()),
        session.active_space.is_none() && session.active_guild.is_none() && !session.active_wa_groups,
        Some(Message::SpaceSelected(None)),
        p,
        l,
    );
    let guilds = discord.map(|d| d.guilds.as_slice()).unwrap_or_default();
    let mut dc: Vec<Element<'a, Message>> = guilds
        .iter()
        .map(|guild| {
            let active = session.active_guild.as_deref() == Some(guild.id.as_str());
            let face: Element<'_, Message> = match web_picture(session, guild.icon.as_deref()) {
                Some(handle) => iced::widget::image(handle.clone())
                    .width(40)
                    .height(40)
                    .border_radius((l.rounding - 2.0).max(0.0))
                    .content_fit(iced::ContentFit::Cover)
                    .into(),
                None => ui::mono_bold(initials(&guild.name), 13.0, if active { p.text_bright() } else { p.text() }).into(),
            };
            iced::widget::tooltip(
                rail_tile(face, active, Some(Message::GuildSelected(guild.id.clone())), p, l),
                container(text(&guild.name).size(12).color(p.text())).padding([4, 8]),
                iced::widget::tooltip::Position::Right,
            )
            .into()
        })
        .collect();
    if discord.is_none() && can_start_discord {
        dc.push(
            iced::widget::tooltip(
                rail_tile(ui::icon(ui::Icon::Discord, 20.0, p.text_muted()), false, Some(Message::StartDiscord), p, l),
                container(text("Discord isn't running. Click to start it.").size(12).color(p.text())).padding([4, 8]),
                iced::widget::tooltip::Position::Right,
            )
            .into(),
        );
    }
    // Bridged spaces only for servers the client doesn't list (it isn't
    // running, say), and never the bridge's own empty spaces.
    let mut mx = Vec::new();
    for (index, group) in session.groups.iter().enumerate().skip(1) {
        if group.network() == Network::Discord {
            let listed = guild_of(group).is_some_and(|id| guilds.iter().any(|g| g.id == id));
            if !group.rooms.is_empty() && !listed {
                dc.push(space_tile(session, index, group, p, l, tags));
            }
        } else {
            mx.push(space_tile(session, index, group, p, l, tags));
        }
    }
    // WhatsApp: one "Groups" tile holding the group chats, so Home stays 1:1.
    let wa_groups: Vec<&whatsapp::Chat> = whatsapp.map(|w| w.chats.iter().filter(|c| c.group).collect()).unwrap_or_default();
    let mut wa: Vec<Element<'a, Message>> = Vec::new();
    if !wa_groups.is_empty() {
        let unread: u64 = wa_groups.iter().map(|c| c.unread).sum();
        let tile = rail_badge(
            rail_tile(ui::icon(ui::Icon::Users, 20.0, if session.active_wa_groups { p.text_bright() } else { p.text() }), session.active_wa_groups, Some(Message::ShowWhatsAppGroups), p, l),
            unread,
            p,
        );
        wa.push(
            iced::widget::tooltip(
                column![tile, ui::mono("groups", 10.0, p.text_muted())].spacing(2).align_x(iced::Alignment::Center),
                container(text("WhatsApp groups").size(12).color(p.text())).padding([4, 8]),
                iced::widget::tooltip::Position::Right,
            )
            .into(),
        );
    }
    let mut folders = column![].spacing(8).align_x(iced::Alignment::Center);
    for (network, tiles) in [(Network::Discord, dc), (Network::Matrix, mx), (Network::WhatsApp, wa)] {
        if !tiles.is_empty() {
            let open = session.folders_open[folder_index(network)];
            folders = folders.push(rail_folder(network, open, tiles, p, l));
        }
    }
    let add = iced::widget::tooltip(
        rail_tile(ui::icon(ui::Icon::Plus, 18.0, p.text_muted()), false, Some(Message::OpenJoinServer), p, l),
        container(text("Join a Discord server").size(12).color(p.text())).padding([4, 8]),
        iced::widget::tooltip::Position::Right,
    );
    scrollable(
        column![home, ui::swatch(p.border(), 28.0, 2.0, 1.0), folders, add]
            .spacing(8)
            .padding([10, 0])
            .width(Length::Fill)
            .align_x(iced::Alignment::Center),
    )
    .height(Length::Fill)
    .into()
}

/// Overlays a `danger` count badge on a rail tile's bottom-right (nothing if 0).
fn rail_badge<'a>(tile: Element<'a, Message>, count: u64, p: &theme::Palette) -> Element<'a, Message> {
    if count == 0 {
        return tile;
    }
    let (fill, ring) = (p.danger(), p.surface());
    let badge = container(ui::mono_bold(count.to_string(), 10.0, p.background()))
        .padding([1, 5])
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(9).color(ring).width(2),
            ..container::Style::default()
        });
    stack![
        tile,
        container(badge).width(52).height(50).align_x(iced::Alignment::End).align_y(iced::Alignment::End),
    ]
    .into()
}

/// Rail folder order: Discord, Matrix, WhatsApp.
fn folder_index(network: Network) -> usize {
    match network {
        Network::Discord => 0,
        Network::Matrix => 1,
        Network::WhatsApp => 2,
    }
}

/// A network's folder: its logo and ▾/▸ toggle it; open, its tiles follow on
/// a tinted strip.
fn rail_folder<'a>(
    network: Network,
    open: bool,
    tiles: Vec<Element<'a, Message>>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let (icon, color) = match network {
        Network::Discord => (ui::Icon::Discord, net_color(Network::Discord, p)),
        Network::Matrix => (ui::Icon::Matrix, net_color(Network::Matrix, p)),
        Network::WhatsApp => (ui::Icon::Phone, net_color(Network::WhatsApp, p)),
    };
    let head = column![
        rail_tile(ui::icon(icon, 22.0, color), false, Some(Message::ToggleFolder(network)), p, l),
        ui::mono(format!("{} {}", network.tag(), if open { "▾" } else { "▸" }), 10.0, p.text_muted()),
    ]
    .spacing(2)
    .align_x(iced::Alignment::Center);
    if !open {
        return head.into();
    }
    let strip = theme::mix(p.surface(), p.surface_alt(), 0.5);
    let radius = l.rounding + 4.0;
    container(column![head].extend(tiles).spacing(8).align_x(iced::Alignment::Center))
        .padding([6, 4])
        .style(move |_| container::Style {
            background: Some(strip.into()),
            border: iced::border::rounded(radius),
            ..container::Style::default()
        })
        .into()
}

/// A space's rail tile: its picture (or network logo), mention badge, and
/// `Alt+n` key.
fn space_tile<'a>(
    session: &'a Session,
    index: usize,
    group: &'a SpaceGroup,
    p: &theme::Palette,
    l: &theme::Layout,
    tags: bool,
) -> Element<'a, Message> {
    let active = group.id() == session.active_space.as_ref();
    let fg = if active { p.text_bright() } else { p.text() };
    // The space's own picture (a Discord server icon, say), else its network's logo.
    let face: Element<'_, Message> =
        match picture(session, group.space.as_ref().and_then(|s| s.avatar.as_ref())) {
            Some(handle) => iced::widget::image(handle.clone())
                .width(40)
                .height(40)
                .border_radius((l.rounding - 2.0).max(0.0))
                .content_fit(iced::ContentFit::Cover)
                .into(),
            None => match group.network() {
                Network::Discord => ui::icon(ui::Icon::Discord, 22.0, if active { net_color(Network::Discord, p) } else { fg }),
                Network::Matrix | Network::WhatsApp => ui::icon(ui::Icon::Matrix, 22.0, if active { net_color(Network::Matrix, p) } else { fg }),
            },
        };
    let tile: Element<'_, Message> = rail_tile(
        face,
        active,
        Some(Message::SpaceSelected(group.id().cloned())),
        p,
        l,
    );
    let tile = rail_badge(tile, group.mentions(), p);
    let tag = group.network().tag();
    let key = match (index <= 9, tags) {
        (true, true) => format!("{index}·{tag}"),
        (true, false) => index.to_string(),
        (false, true) => tag.to_string(),
        (false, false) => String::new(),
    };
    iced::widget::tooltip(
        column![
            tile,
            ui::mono(key, 10.0, if active { p.text_bright() } else { p.text_muted() }),
        ]
        .spacing(2)
        .align_x(iced::Alignment::Center),
        container(text(group.name().to_string()).size(12).color(p.text())).padding([4, 8]),
        iced::widget::tooltip::Position::Right,
    )
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

fn channel_pane<'a>(
    session: &'a Session,
    discord: Option<&'a mesh_discord::Snapshot>,
    whatsapp: Option<&'a whatsapp::Snapshot>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    if session.active_wa_groups {
        return wa_groups_pane(session, whatsapp, p, l);
    }
    if session.active_guild.is_some() {
        return guild_pane(session, discord, p, l);
    }
    let group = active_group(session);
    let mut title = row![
        ui::sans(group.map_or("Home", |g| g.name()), 15.0, Weight::Semibold, p.text_bright())
            .width(Length::Fill),
    ]
    .align_y(iced::Alignment::Center);
    if whatsapp.is_some() && !group.is_some_and(|g| g.space.is_some()) {
        title = title.push(iced::widget::tooltip(
            ui::icon_button(ui::Icon::Plus, Some(Message::NewChatOpen), false, p, l),
            container(text("New WhatsApp chat").size(12).color(p.text())).padding([4, 8]),
            iced::widget::tooltip::Position::Bottom,
        ));
    }
    if session.discord_voice.is_some() {
        title = title.push(
            iced::widget::tooltip(
                ui::icon_button(ui::Icon::Headphones, Some(Message::OpenDiscordVoice), false, p, l),
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
                ui::mono(Network::Discord.tag(), 11.0, net_color(Network::Discord, p)),
                ui::mono(format!("bridged via {}", my_server(session)), 11.0, p.text_muted()),
            ]
            .spacing(6),
        );
    }
    let home = !group.is_some_and(|g| g.space.is_some());
    if home {
        // Compact icon toggles (with tooltips) so they don't overflow.
        let mut chips = row![filter_chip(None, ui::Icon::Home, "All", p.text_bright(), session.home_filter, p, l)].spacing(4);
        for (icon, label, color, filter) in [
            (ui::Icon::Matrix, "Matrix", net_color(Network::Matrix, p), Network::Matrix),
            (ui::Icon::Discord, "Discord", net_color(Network::Discord, p), Network::Discord),
            (ui::Icon::Phone, "WhatsApp", p.success(), Network::WhatsApp),
        ] {
            chips = chips.push(filter_chip(Some(filter), icon, label, color, session.home_filter, p, l));
        }
        header = header.push(chips);
    }
    let header = header
        .padding(iced::Padding { top: 14.0, right: 14.0, bottom: 10.0, left: 14.0 });

    let mut list = column![].spacing(2).padding([10, 8]);
    let rooms = group.map(|g| g.rooms.as_slice()).unwrap_or_default();
    let sections: [(&str, Vec<&RoomSummary>); 2] = if !home {
        [("Text", rooms.iter().collect()), ("", Vec::new())]
    } else {
        let (directs, others) = rooms
            .iter()
            .filter(|r| session.home_filter.is_none_or(|n| r.network == n))
            .partition(|r| r.is_direct);
        [("Direct messages", directs), ("Rooms", others)]
    };
    // With the hidden client running, Discord DMs come from it (no bridge in
    // the way) and the bridged copies of them are left out.
    let shows = |network| home && session.home_filter.is_none_or(|n| n == network);
    let discord_dms = match discord {
        Some(d) if shows(Network::Discord) => d.dms.as_slice(),
        _ => &[],
    };
    // WhatsApp chats (groups too) sit with the DMs: they're conversations.
    let whatsapp_chats = match whatsapp {
        Some(w) if shows(Network::WhatsApp) => w.chats.as_slice(),
        _ => &[],
    };
    for (label, rooms) in sections {
        let dm_section = home && label == "Direct messages";
        let mut rows: Vec<(u64, Element<'a, Message>)> = rooms
            .into_iter()
            .filter(|r| !(dm_section && discord.is_some() && r.bridged.as_ref().is_some_and(|b| b.guild_id.is_none())))
            .map(|room| {
                let selected = session.selected_room.as_ref() == Some(&room.id);
                (room.last_activity_ms.unwrap_or(0), channel_row(room, &session.avatars, selected, p, l))
            })
            .collect();
        if dm_section {
            rows.extend(discord_dms.iter().map(|dm| {
                let open = WebChat { network: Network::Discord, id: dm.id.clone(), name: dm.name.clone(), dm: true };
                (dm.last_ms, web_chat_row(session, open, dm.avatar.as_deref(), dm.unread, dm.mentions, p, l))
            }));
            rows.extend(whatsapp_chats.iter().filter(|chat| !chat.group).map(|chat| {
                let open = WebChat { network: Network::WhatsApp, id: chat.id.clone(), name: chat.name.clone(), dm: !chat.group };
                (chat.last_ms, web_chat_row(session, open, chat.avatar.as_deref(), chat.unread > 0, chat.unread, p, l))
            }));
            rows.sort_by_key(|(last, _)| std::cmp::Reverse(*last));
        }
        if rows.is_empty() {
            continue;
        }
        list = list.push(ui::section(label, p));
        for (_, row) in rows {
            list = list.push(row);
        }
    }

    // Archived WhatsApp chats: one row that opens them, like WhatsApp's.
    if let Some(w) = whatsapp.filter(|w| shows(Network::WhatsApp) && !w.archived.is_empty()) {
        let arrow = if session.show_archived { "▾" } else { "▸" };
        let line = row![ui::mono(format!("{arrow} archived ({})", w.archived.len()), 12.0, p.text_muted())];
        list = list.push(ui::row_button(line, Message::ToggleArchived, false, p, l));
        if session.show_archived {
            for chat in &w.archived {
                let open = WebChat { network: Network::WhatsApp, id: chat.id.clone(), name: chat.name.clone(), dm: !chat.group };
                list = list.push(web_chat_row(session, open, chat.avatar.as_deref(), chat.unread > 0, chat.unread, p, l));
            }
        }
    }

    // A bridged server's voice channels, from the hidden client. (Servers it
    // lists get their own rail tile and guild_pane instead.)
    if let Some(discord) = discord
        && let Some(guild) = group
            .filter(|g| g.space.is_some())
            .and_then(guild_of)
            .and_then(|id| discord.guilds.iter().find(|x| x.id == id))
    {
        list = list.push(voice_channels(session, guild, discord.voice.channel_id.as_deref(), p, l));
    }

    if let Some(space) = group.and_then(|g| g.space.as_ref()).filter(|s| s.is_invite) {
        list = column![invite_card(space, p, l)].padding([10, 8]).push(list);
    }
    column![
        header,
        ui::hline(p),
        scrollable(list).height(Length::Fill),
        ui::hline(p),
        user_panel(session, discord.map(|d| &d.voice), p, l),
    ]
    .into()
}

/// A server's voice channels: click to join; who's in each, lit while talking.
fn voice_channels<'a>(
    session: &'a Session,
    guild: &'a mesh_discord::Guild,
    current: Option<&str>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let mut col = column![ui::section("voice", p)].spacing(2);
    for channel in &guild.voice_channels {
        col = col.push(voice_channel(session, channel, current, p, l));
    }
    col.into()
}

/// One voice channel row (click to join), with who's in it under it.
fn voice_channel<'a>(
    session: &'a Session,
    channel: &'a mesh_discord::VoiceChannel,
    current: Option<&str>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let here = current == Some(channel.id.as_str());
    let color = if here { p.success() } else { p.text() };
    let mut line = row![
        ui::icon(ui::Icon::Voice, 15.0, color),
        ui::sans(&channel.name, 14.0, if here { Weight::Semibold } else { Weight::Normal }, color).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    if here {
        line = line.push(ui::mono("connected", 10.0, p.success()));
    }
    let mut col = column![ui::row_button(line, Message::DiscordJoin(channel.id.clone()), here, p, l)].spacing(2);
    for member in &channel.members {
        col = col.push(container(voice_member(member, web_picture(session, member.avatar.as_deref()), 22.0, p)).padding(iced::Padding { left: 30.0, ..iced::Padding::ZERO }));
    }
    col.into()
}

/// A Discord server straight from the hidden client: its categories, each
/// with text channels (opening the bridged room, when there is one) and
/// voice channels (click to join).
/// The WhatsApp groups list (off its rail tile): group chats, most recent
/// first, opening in the chat pane like any other web chat.
fn wa_groups_pane<'a>(
    session: &'a Session,
    whatsapp: Option<&'a whatsapp::Snapshot>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let footer = user_panel(session, None, p, l);
    let mut groups: Vec<&whatsapp::Chat> =
        whatsapp.map(|w| w.chats.iter().filter(|c| c.group).collect()).unwrap_or_default();
    groups.sort_by_key(|c| std::cmp::Reverse(c.last_ms));
    let header = column![
        ui::sans("WhatsApp groups", 15.0, Weight::Semibold, p.text_bright()),
        ui::mono(format!("{} groups", groups.len()), 11.0, p.text_muted()),
    ]
    .spacing(4)
    .padding(iced::Padding { top: 14.0, right: 14.0, bottom: 10.0, left: 14.0 });
    let mut list = column![].spacing(2).padding([10, 8]);
    for chat in groups {
        let open = WebChat { network: Network::WhatsApp, id: chat.id.clone(), name: chat.name.clone(), dm: false };
        list = list.push(web_chat_row(session, open, chat.avatar.as_deref(), chat.unread > 0, chat.unread, p, l));
    }
    column![header, ui::hline(p), scrollable(list).height(Length::Fill), ui::hline(p), footer].into()
}

fn guild_pane<'a>(
    session: &'a Session,
    discord: Option<&'a mesh_discord::Snapshot>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let footer = user_panel(session, discord.map(|d| &d.voice), p, l);
    let Some(guild) = active_guild(session, discord) else {
        let body = container(text("Discord isn't running, or you left this server.").size(13).color(p.text_muted()))
            .padding(14)
            .height(Length::Fill);
        return column![body, ui::hline(p), footer].into();
    };
    let header = column![
        ui::sans(&guild.name, 15.0, Weight::Semibold, p.text_bright()),
        ui::mono(format!("{} channels", guild.text_channels.len() + guild.voice_channels.len()), 11.0, p.text_muted()),
        row![ui::mono(Network::Discord.tag(), 11.0, net_color(Network::Discord, p)), ui::mono("from your Discord account", 11.0, p.text_muted())].spacing(6),
    ]
    .spacing(4)
    .padding(iced::Padding { top: 14.0, right: 14.0, bottom: 10.0, left: 14.0 });

    // Categories in the order Discord lists them; uncategorised first.
    let mut categories: Vec<Option<&str>> = Vec::new();
    let in_order = guild.text_channels.iter().map(|c| c.category.as_deref());
    for category in in_order.chain(guild.voice_channels.iter().map(|c| c.category.as_deref())) {
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    categories.sort_by_key(|c| c.is_some());

    let current = discord.and_then(|d| d.voice.channel_id.as_deref());
    let mut list = column![].spacing(2).padding([10, 8]);
    for category in categories {
        if let Some(name) = category {
            list = list.push(ui::section(name, p));
        }
        for channel in guild.text_channels.iter().filter(|c| c.category.as_deref() == category) {
            let open = WebChat { network: Network::Discord, id: channel.id.clone(), name: channel.name.clone(), dm: false };
            list = list.push(web_chat_row(session, open, None, false, 0, p, l));
        }
        for channel in guild.voice_channels.iter().filter(|c| c.category.as_deref() == category) {
            list = list.push(voice_channel(session, channel, current, p, l));
        }
    }
    column![header, ui::hline(p), scrollable(list).height(Length::Fill), ui::hline(p), footer].into()
}

/// Someone in a Discord call: avatar (ringed while talking), name, mute/deaf.
fn voice_member<'a>(
    member: &'a mesh_discord::Member,
    picture: Option<&iced::widget::image::Handle>,
    size: f32,
    p: &theme::Palette,
) -> Element<'a, Message> {
    let ring = if member.speaking { p.success() } else { iced::Color::TRANSPARENT };
    let face = container(ui::avatar(&member.name, picture, size, net_color(Network::Discord, p), p, None))
        .padding(2)
        .style(move |_| container::Style {
            border: iced::border::rounded(size).color(ring).width(2),
            ..container::Style::default()
        });
    let mut line = row![face, text(&member.name).size(13).color(p.text()).width(Length::Fill)]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if member.deaf {
        line = line.push(ui::icon(ui::Icon::HeadphoneOff, 13.0, p.danger()));
    } else if member.mute {
        line = line.push(ui::icon(ui::Icon::MicOff, 13.0, p.danger()));
    }
    line.into()
}

/// A Discord text channel or DM row (from the hidden client), drawn like
/// `channel_row`; click to read it in the chat pane.
fn web_chat_row<'a>(
    session: &Session,
    channel: WebChat,
    picture: Option<&str>,
    unread: bool,
    count: u64,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let selected = session.web_chat.as_ref().is_some_and(|c| c.id == channel.id);
    let color = if selected || unread { p.text_bright() } else { p.text() };
    let bar = if selected { p.accent() } else { iced::Color::TRANSPARENT };
    let face = match web_picture(session, picture) {
        Some(handle) => ui::avatar(&channel.name, Some(handle), 20.0, p.surface_alt(), p, None),
        None => ui::icon(if channel.dm { ui::Icon::At } else { ui::Icon::Hash }, 14.0, p.text_muted()),
    };
    let mut label = row![
        ui::swatch(bar, 2.0, 18.0, 1.0),
        face,
        ui::sans(channel.name.clone(), 14.0, if unread { Weight::Semibold } else { Weight::Normal }, color)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    if count > 0 {
        label = label.push(ui::chip(count.to_string(), p));
    }
    // Server channels need no tag; DMs and chats say where they're from.
    if channel.dm || channel.network != Network::Discord {
        label = label.push(ui::mono(channel.network.tag(), 10.0, net_color(channel.network, p)));
    }
    ui::row_button(label, Message::WebChatSelected(channel), selected, p, l)
        .padding(iced::Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 0.0 })
        .into()
}

/// A channel row. Active: `surface_alt` fill and an accent bar; unread:
/// bright, semibold, with a count chip.
fn channel_row<'a>(
    room: &'a RoomSummary,
    avatars: &'a HashMap<String, Option<iced::widget::image::Handle>>,
    selected: bool,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let unread = room.unread_notifications > 0;
    let color = if selected || unread { p.text_bright() } else { p.text() };
    let bar = if selected { p.accent() } else { iced::Color::TRANSPARENT };
    let mut label = row![
        ui::swatch(bar, 2.0, 18.0, 1.0),
        match room.avatar.as_ref().and_then(|a| avatars.get(&a.key())).and_then(Option::as_ref) {
            Some(handle) => ui::avatar(&room.name, Some(handle), 20.0, p.surface_alt(), p, None),
            None => ui::icon(if room.is_direct { ui::Icon::At } else { ui::Icon::Hash }, 14.0, p.text_muted()),
        },
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
fn user_panel<'a>(
    session: &'a Session,
    discord: Option<&mesh_discord::Voice>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let (name, mxid) = session
        .my_profile
        .as_ref()
        .map(|me| (me.display_name.clone(), me.user_id.to_string()))
        .unwrap_or_else(|| ("…".to_string(), String::new()));
    // In Discord voice, the buttons drive that call; otherwise Matrix calls.
    let discord = discord.filter(|v| v.connected());
    let (muted, deafened) = match discord {
        Some(v) => (v.self_mute, v.self_deaf),
        None => (session.self_muted, session.self_deafened),
    };
    let sub = match (discord, &session.call) {
        (Some(v), _) => ui::mono(format!("dc voice · {}", v.channel_name.as_deref().unwrap_or("call")), 10.0, p.success()),
        (None, Some(call)) => ui::mono(call.state.label(), 10.0, p.accent()),
        (None, None) => ui::mono(mxid, 10.0, p.text_muted()),
    };

    let mut controls = row![
        ui::icon_button(
            if muted { ui::Icon::MicOff } else { ui::Icon::Mic },
            Some(Message::ToggleMute),
            muted,
            p,
            l,
        ),
        ui::icon_button(
            if deafened { ui::Icon::HeadphoneOff } else { ui::Icon::Headphones },
            Some(Message::ToggleDeafen),
            deafened,
            p,
            l,
        ),
    ]
    .spacing(2);
    if discord.is_some() {
        controls = controls.push(ui::icon_button(ui::Icon::PhoneOff, Some(Message::DiscordLeave), true, p, l));
    } else if session.call.is_some() {
        controls = controls.push(ui::icon_button(ui::Icon::PhoneOff, Some(Message::HangUp), true, p, l));
    }
    controls = controls.push(ui::icon_button(
        ui::Icon::Settings,
        Some(Message::ToggleSettings),
        session.show_settings,
        p,
        l,
    ));

    let fill = p.panel_dim();
    let radius = (l.rounding - l.border_size).max(0.0);
    container(
        row![
            ui::avatar(
                &name,
                picture(session, session.my_profile.as_ref().and_then(|me| me.avatar.as_ref())),
                34.0,
                p.accent(),
                p,
                Some((p.success(), fill)),
            ),
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
    if let Some(channel) = &session.web_chat {
        let mut header = row![
            ui::icon(if channel.dm { ui::Icon::At } else { ui::Icon::Hash }, 18.0, p.text_muted()),
            ui::sans(&channel.name, 15.0, Weight::Semibold, p.text_bright()).width(Length::Fill),
            ui::mono(channel.network.tag(), 11.0, net_color(channel.network, p)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if channel.network == Network::WhatsApp {
            let flat = |label: &'static str, color: iced::Color, press: Message| {
                button(text(label).size(12).color(color)).on_press(press).padding([4, 10]).style(flat_button_style(p))
            };
            if session.confirm_delete.as_deref() == Some(channel.id.as_str()) {
                header = header
                    .push(ui::mono("delete this chat?", 11.0, p.danger()))
                    .push(flat("Delete", p.danger(), Message::WaDeleteConfirm))
                    .push(flat("Cancel", p.text(), Message::WaDeleteCancel));
            } else {
                let archived = state.whatsapp_state.as_ref().is_some_and(|w| w.archived.iter().any(|c| c.id == channel.id));
                header = header
                    .push(flat(if archived { "Unarchive" } else { "Archive" }, p.text(), Message::WaArchive(channel.id.clone(), !archived)))
                    .push(flat("Delete", p.text_muted(), Message::WaDeleteAsk(channel.id.clone())));
            }
        }
        if channel.dm && channel.network == Network::WhatsApp && state.wa_call.is_none() {
            header = header
                .push(ui::icon_button(ui::Icon::Phone, Some(Message::WaCallStart(false)), false, p, l))
                .push(ui::icon_button(ui::Icon::Video, Some(Message::WaCallStart(true)), false, p, l));
        }
        if channel.dm && channel.network == Network::Discord {
            let in_call = state.discord_state.as_ref().and_then(|d| d.voice.channel_id.as_deref()) == Some(channel.id.as_str());
            header = header.push(if in_call {
                ui::icon_button(ui::Icon::PhoneOff, Some(Message::DiscordLeave), true, p, l)
            } else {
                ui::icon_button(ui::Icon::Phone, Some(Message::DiscordJoin(channel.id.clone())), false, p, l)
            });
        }
        col = col.push(container(header).padding([10, 16])).push(ui::hline(p));
    } else if let Some(room) = room {
        let topic = room
            .topic
            .as_deref()
            .and_then(|t| t.lines().next())
            .unwrap_or_default();
        let mut header = row![
            ui::icon(if room.is_direct { ui::Icon::At } else { ui::Icon::Hash }, 18.0, p.text_muted()),
            ui::sans(&room.name, 15.0, Weight::Semibold, p.text_bright()),
            text(topic).size(13).color(p.text_muted()).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        // Discord DMs call through the hidden Discord client; Matrix rooms
        // use Mesh's own calls. Server channels have their voice channels instead.
        let discord_voice = state.discord_state.as_ref().map(|d| &d.voice);
        match room.bridged.as_ref() {
            Some(ids) if ids.guild_id.is_none() => {
                let in_this_call = discord_voice.and_then(|v| v.channel_id.as_deref()) == Some(ids.channel_id.as_str());
                header = header.push(if in_this_call {
                    ui::icon_button(ui::Icon::PhoneOff, Some(Message::DiscordLeave), true, p, l)
                } else {
                    ui::icon_button(ui::Icon::Phone, Some(Message::DiscordJoin(ids.channel_id.clone())), false, p, l)
                });
            }
            Some(_) => {}
            None if session.call.is_none() => {
                header = header.push(ui::icon_button(ui::Icon::Phone, Some(Message::PlaceCall), false, p, l));
            }
            None => {}
        }
        let members_color = if session.show_members { p.text_bright() } else { p.text_muted() };
        header = header.push(
            button(ui::icon(ui::Icon::Users, 17.0, members_color))
                .on_press(Message::ToggleMembers)
                .padding(6)
                .style(flat_button_style(p)),
        );
        col = col.push(container(header).padding([10, 16])).push(ui::hline(p));
    }

    if let Some(call) = &session.call {
        col = col.push(container(call_bar(call, p)).padding([8, 16]));
    }
    if let Some(snapshot) = state.discord_state.as_ref().filter(|s| s.voice.connected()) {
        let discord_video = state.video.iter().filter(|(id, _)| !id.starts_with("wa-")).map(|(_, t)| t);
        col = col.push(container(discord_call_bar(session, &snapshot.voice, discord_video, p, l)).padding([8, 16]));
    }
    if let Some(call) = &state.wa_call {
        let wa_video = state.video.iter().filter(|(id, _)| id.starts_with("wa-")).map(|(_, t)| t);
        col = col.push(container(wa_call_bar(call, wa_video, p, l)).padding([8, 16]));
    }
    if let Some(prompt) = &session.verification {
        col = col.push(container(verification_banner(prompt, p)).padding([8, 16]));
    }

    // Timeline, status line and composer: the same for a room or a Discord channel.
    let me = session.my_profile.as_ref().map(|me| &me.user_id);
    let body = |mut col: iced::widget::Column<'a, Message>, placeholder: String, typing_in: Option<&OwnedRoomId>| {
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
        col.push(composer(session, placeholder, typing_in, p, l))
    };

    if let Some(channel) = &session.web_chat {
        let (hash, account) = match channel.network {
            Network::WhatsApp => ("", channel.network.spec().name),
            _ => (if channel.dm { "" } else { "#" }, channel.network.spec().name),
        };
        let placeholder = format!("message {hash}{}  —  sends with your {account} account", channel.name);
        return body(col, placeholder, None).into();
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

    let placeholder = match room.network {
        Network::Discord => format!("message {}  —  sends to discord via bridge", room_label(room)),
        Network::Matrix | Network::WhatsApp => format!("message {}", room_label(room)),
    };
    body(col, placeholder, Some(&room.id)).into()
}

/// The message box; `typing_in` is the Matrix room whose typing line shows under it.
fn composer<'a>(
    session: &'a Session,
    placeholder: String,
    typing_in: Option<&OwnedRoomId>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let (muted, bright, selection) = (p.text_muted(), p.text_bright(), theme::alpha(p.accent(), 0.35));
    let input = text_input(&placeholder, &session.message_draft)
        .id(iced::widget::Id::new(COMPOSER_INPUT))
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
    // The emoji button is itself an emoji, so it also shows whether this
    // platform renders them.
    let emoji_fill = if session.emoji_open { p.surface_alt() } else { iced::Color::TRANSPARENT };
    let toggle_radius = l.control_radius();
    let emoji_toggle = button(text("🙂").size(17))
        .on_press(Message::ToggleEmoji)
        .padding(6)
        .style(move |_, _| button::Style {
            background: Some(emoji_fill.into()),
            border: iced::border::rounded(toggle_radius),
            ..button::Style::default()
        });
    let field = row![
        ui::icon_button(ui::Icon::Paperclip, Some(Message::PickAttachment), false, p, l),
        emoji_toggle,
        input,
        ui::primary_button(ui::icon(ui::Icon::Send, 16.0, p.accent_text()), Some(Message::SendMessage), p, l),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let (fill, edge, radius) = (p.background(), p.border_ctrl(), l.control_radius() + 2.0);
    let mut outer = column![];
    if session.emoji_open {
        outer = outer.push(emoji_picker(p, l));
    }
    if let Some(att) = &session.pending_attachment {
        let label = if session.sending_attachment {
            format!("sending {} …", att.filename)
        } else {
            format!("📎 {} · {}", att.filename, human_size(att.data.len() as u64))
        };
        let mut chip = row![ui::mono(label, 12.0, p.text_bright())]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if !session.sending_attachment {
            chip = chip.push(ui::icon_button(ui::Icon::Close, Some(Message::ClearAttachment), false, p, l));
        }
        outer = outer.push(
            container(chip).padding([6, 10]).style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(radius).color(edge).width(1),
                ..container::Style::default()
            }),
        );
    }
    if let Some((_, quote)) = &session.replying {
        outer = outer.push(
            container(
                row![
                    column![
                        ui::mono(format!("replying to {}", quote.author), 11.0, p.accent()),
                        ui::mono(quote.text.lines().next().unwrap_or_default(), 11.0, p.text_muted()),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    ui::icon_button(ui::Icon::Close, Some(Message::CancelReply), false, p, l),
                ]
                .align_y(iced::Alignment::Center),
            )
            .padding([6, 10])
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(radius).color(edge).width(1),
                ..container::Style::default()
            }),
        );
    }
    outer.push(
        container(field)
            .padding([6, 8])
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(radius).color(edge).width(1),
                ..container::Style::default()
            }),
    )
    .push(
        row![
            typing_line(typing_in.and_then(|id| session.typing.get(id)).map(Vec::as_slice).unwrap_or_default(), p),
            Space::new().width(Length::Fill),
            ui::mono("Enter send · markdown on", 11.0, p.text_muted()),
        ],
    )
    .spacing(6)
    .padding(iced::Padding { top: 8.0, right: 16.0, bottom: 14.0, left: 16.0 })
    .into()
}

/// The emoji picker shown under the composer: a grid of [`EMOJI`]; clicking
/// one appends it to the draft and keeps the picker open.
fn emoji_picker<'a>(p: &'a theme::Palette, l: &theme::Layout) -> Element<'a, Message> {
    const PER_ROW: usize = 8;
    let (fill, edge, radius) = (p.background(), p.border_ctrl(), l.control_radius() + 2.0);
    let mut grid = column![].spacing(2);
    for chunk in EMOJI.chunks(PER_ROW) {
        let mut line = row![].spacing(2);
        for emoji in chunk {
            line = line.push(
                button(text(*emoji).size(20))
                    .on_press(Message::InsertEmoji(emoji))
                    .padding(4)
                    .style(flat_button_style(p)),
            );
        }
        grid = grid.push(line);
    }
    container(grid)
        .padding(6)
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(radius).color(edge).width(1),
            ..container::Style::default()
        })
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
                    ui::avatar(&member.display_name, picture(session, member.avatar.as_ref()), 32.0, fill, p, None),
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

/// The Discord call you're in: where, who (lit while talking), mute, leave.
fn discord_call_bar<'a>(
    session: &'a Session,
    voice: &'a mesh_discord::Voice,
    video: impl Iterator<Item = &'a VideoTile>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let mut people = row![].spacing(10).align_y(iced::Alignment::Center);
    for member in &voice.members {
        people = people.push(voice_member(member, web_picture(session, member.avatar.as_deref()), 26.0, p));
    }
    let status = if voice.connection.contains("CONNECTED") && !voice.connection.contains("DIS") {
        p.success()
    } else {
        p.warning()
    };
    let (fill, edge) = (p.surface_alt(), p.border());
    let mut body = column![
        row![
            ui::icon(ui::Icon::Voice, 17.0, status),
            ui::mono_bold(Network::Discord.tag(), 12.0, net_color(Network::Discord, p)),
            ui::sans(voice.channel_name.as_deref().unwrap_or("call"), 14.0, Weight::Semibold, p.text_bright())
                .width(Length::Fill),
            ui::icon_button(
                if voice.self_mute { ui::Icon::MicOff } else { ui::Icon::Mic },
                Some(Message::ToggleMute),
                voice.self_mute,
                p,
                l,
            ),
            ui::icon_button(
                if voice.self_video { ui::Icon::Video } else { ui::Icon::VideoOff },
                Some(Message::ToggleVideo),
                !voice.self_video,
                p,
                l,
            ),
            ui::icon_button(
                if voice.self_stream { ui::Icon::ScreenShare } else { ui::Icon::ScreenShareOff },
                Some(Message::ToggleScreenShare),
                !voice.self_stream,
                p,
                l,
            ),

            ui::icon_button(ui::Icon::PhoneOff, Some(Message::DiscordLeave), true, p, l),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
        scrollable(people).direction(iced::widget::scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::new().width(2).scroller_width(2),
        )),
    ]
    .spacing(8);
    // Other people's screen shares: Discord only sends one once you watch it.
    if !voice.streams.is_empty() {
        let mut shares = row![ui::mono("sharing:", 11.0, p.text_muted())].spacing(8).align_y(iced::Alignment::Center);
        for stream in &voice.streams {
            let label = if stream.watching { format!("■ stop watching {}", stream.name) } else { format!("▶ watch {}", stream.name) };
            shares = shares.push(
                button(text(label).size(12).color(p.text_bright()))
                    .on_press(Message::WatchStream(stream.owner_id.clone(), !stream.watching))
                    .padding([4, 10])
                    .style(flat_button_style(p)),
            );
        }
        body = body.push(shares);
    }
    if let Some(tiles) = video_tiles(video, p) {
        body = body.push(tiles);
    }
    container(body)
    .padding(12)
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    })
    .into()
}

/// WhatsApp's call: ringing (answer / decline) or under way (mute, camera,
/// hang up), with its video.
fn wa_call_bar<'a>(
    call: &'a whatsapp::Call,
    video: impl Iterator<Item = &'a VideoTile>,
    p: &theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let status = match call.phase.as_str() {
        "incoming" => if call.video { "video call · ringing" } else { "calling you" },
        "outgoing" => "ringing…",
        "connecting" => "connecting…",
        _ => "in call",
    };
    let mut controls = row![].spacing(8).align_y(iced::Alignment::Center);
    if call.phase == "incoming" {
        let answer = |label: &'static str, video: bool| {
            ui::primary_button(ui::sans(label, 13.0, Weight::Semibold, p.accent_text()), Some(Message::WaCallAccept(video)), p, l)
                .padding([6, 12])
        };
        if call.video {
            controls = controls.push(answer("Answer with video", true));
        }
        controls = controls.push(answer(if call.video { "Voice only" } else { "Answer" }, false));
        controls = controls.push(ui::icon_button(ui::Icon::PhoneOff, Some(Message::WaCallDecline), true, p, l));
    } else {
        controls = controls
            .push(ui::icon_button(if call.muted { ui::Icon::MicOff } else { ui::Icon::Mic }, Some(Message::WaCallMute), call.muted, p, l))
            .push(ui::icon_button(
                if call.camera { ui::Icon::Video } else { ui::Icon::VideoOff },
                Some(Message::WaCallCamera),
                !call.camera,
                p,
                l,
            ))
            .push(ui::icon_button(
                if call.screen { ui::Icon::ScreenShare } else { ui::Icon::ScreenShareOff },
                Some(Message::WaCallScreen),
                !call.screen,
                p,
                l,
            ))
            .push(ui::icon_button(ui::Icon::PhoneOff, Some(Message::WaCallEnd), true, p, l));
    }
    let mut body = column![
        row![
            ui::icon(ui::Icon::Phone, 17.0, if call.phase == "active" { p.success() } else { p.warning() }),
            ui::mono_bold(Network::WhatsApp.tag(), 12.0, net_color(Network::WhatsApp, p)),
            ui::sans(&call.name, 14.0, Weight::Semibold, p.text_bright()),
            text(status).size(12).color(p.text_muted()).width(Length::Fill),
            controls,
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8);
    if let Some(tiles) = video_tiles(video, p) {
        body = body.push(tiles);
    }
    let (fill, edge) = (p.surface_alt(), p.border());
    container(body)
        .padding(12)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(8).color(edge).width(1),
            ..container::Style::default()
        })
        .into()
}

/// The call's cameras: everyone else's large, ours small at the end.
fn video_tiles<'a>(video: impl Iterator<Item = &'a VideoTile>, p: &theme::Palette) -> Option<Element<'a, Message>> {
    let live: Vec<&VideoTile> = video.filter(|t| t.at.elapsed() < VIDEO_STALE).collect();
    if live.is_empty() {
        return None;
    }
    let edge = p.border();
    let tile = |t: &'a VideoTile, height: f32| {
        container(iced::widget::image(t.frame.clone()).height(height).content_fit(iced::ContentFit::Contain))
            .clip(true)
            .style(move |_| container::Style {
                border: iced::border::rounded(8).color(edge).width(1),
                ..container::Style::default()
            })
    };
    let mut tiles = row![].spacing(8).align_y(iced::Alignment::End);
    for t in live.iter().filter(|t| !t.local) {
        tiles = tiles.push(tile(t, 320.0));
    }
    for t in live.iter().filter(|t| t.local) {
        tiles = tiles.push(tile(t, 110.0));
    }
    Some(container(tiles).center_x(Length::Fill).into())
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

    let title = format!("{}  —  {}", call.peer, call.state.label());
    let surface = palette.surface_alt();
    let border_color = palette.border();
    container(
        row![
            ui::icon(ui::Icon::Phone, 17.0, palette.accent()),
            text(title).size(15).color(palette.text_bright()).width(Length::Fill),
            controls,
        ]
        .spacing(10)
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

/// Runs a shell command in the background, without waiting for it.
fn run_command(cmd: &str) -> std::io::Result<()> {
    // cmd doesn't understand Rust's \" quoting, so pass the line as is; and
    // don't flash a console window while it runs.
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd).creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    command.spawn().map(|_| ())
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
    p: &'a theme::Palette,
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
                        Network::Matrix => format!("{} · {}", Network::Matrix.tag(), message.sender.server_name()),
                        other => other.tag().to_string(),
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
                            ui::avatar(
                                &message.sender_name,
                                picture(session, message.sender_avatar.as_ref()),
                                38.0,
                                fill,
                                p,
                                None,
                            ),
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
    p: &'a theme::Palette,
) -> Element<'a, Message> {
    let body_color = theme::mix(p.text(), p.text_bright(), 0.4);
    let mut col = column![].spacing(6).width(Length::Fill);

    if let Some(quote) = &message.reply_to {
        col = col.push(quote_block(quote, p));
    }

    match &message.kind {
        MessageKind::Image(info) if plain => {
            let size = info.size.map(|s| format!(" · {}", human_size(s))).unwrap_or_default();
            col = col.push(ui::mono(format!("🖼 {}{size}", message.body), 12.0, p.text_muted()));
        }
        MessageKind::Image(info) => {
            col = col.push(image_figure(&message.body, info, session, p));
        }
        MessageKind::File(info) if let Some(poster) = &info.thumbnail => {
            col = col.push(video_figure(info, poster, session, p));
        }
        MessageKind::File(info) if info.kind == "Voice note" => {
            col = col.push(voice_player(info, session, p));
        }
        MessageKind::File(info) => {
            col = col.push(file_figure(info, p));
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

    if !message.reactions.is_empty() {
        let mut pills = row![].spacing(6);
        for reaction in &message.reactions {
            // Matrix toggles by event; WhatsApp sends the emoji (or "" to undo).
            let press = match (&message.event_id, &message.web_id) {
                (Some(event_id), _) => Some(Message::ToggleReaction {
                    event_id: event_id.clone(),
                    key: reaction.key.clone(),
                    mine: reaction.mine.clone(),
                }),
                (None, Some(id)) => Some(Message::React {
                    id: id.clone(),
                    emoji: if reaction.by_me { String::new() } else { reaction.key.clone() },
                }),
                (None, None) => None,
            };
            let mine = reaction.by_me;
            let (fill, edge) = if mine {
                (theme::mix(p.accent(), p.surface(), 0.85), p.accent())
            } else {
                (theme::mix(p.surface(), p.surface_alt(), 0.5), p.border_ctrl())
            };
            let fg = if mine { p.text_bright() } else { p.text() };
            pills = pills.push(
                button(ui::mono(format!("{} {}", reaction.key, reaction.count), 11.0, fg))
                    .on_press_maybe(press)
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

    // WhatsApp messages get an action bar on hover: react, reply, forward.
    let Some(id) = message.web_id.as_deref().filter(|_| message.sender_network == Network::WhatsApp) else {
        return col.into();
    };
    let picking = session.reacting.as_deref() == Some(id);
    if picking {
        col = col.push(quick_reactions(message, id, p));
    }
    let mut layers = stack![col];
    if picking || session.hovered.as_deref() == Some(id) {
        layers = layers.push(
            container(message_actions(message, id, p))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right),
        );
    }
    mouse_area(layers)
        .on_enter(Message::HoverMessage(id.to_string()))
        .on_exit(Message::UnhoverMessage(id.to_string()))
        .into()
}

/// The quoted message above a reply: its author and a line of its text.
fn quote_block<'a>(quote: &'a mesh_core::Quote, p: &theme::Palette) -> Element<'a, Message> {
    let (fill, edge) = (p.panel_dim(), theme::mix(p.accent(), p.border(), 0.5));
    let line = quote.text.lines().next().unwrap_or_default();
    container(
        column![
            ui::sans(&quote.author, 12.0, Weight::Semibold, p.accent()),
            text(line).size(13).color(p.text_muted()),
        ]
        .spacing(2),
    )
    .padding([4, 10])
    .max_width(520)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(6).color(edge).width(1),
        ..container::Style::default()
    })
    .into()
}

/// What a reply to `message` quotes.
fn quote_of(message: &TimelineMessage) -> mesh_core::Quote {
    let text = match &message.kind {
        MessageKind::Image(_) => "Photo".to_string(),
        MessageKind::File(info) => info.kind.clone(),
        _ => message.body.chars().take(200).collect(),
    };
    mesh_core::Quote { author: message.sender_name.clone(), text }
}

/// The hover bar on a WhatsApp message.
fn message_actions<'a>(message: &TimelineMessage, id: &str, p: &'a theme::Palette) -> Element<'a, Message> {
    let action = |glyph: &'static str, tip: &'static str, press: Message| {
        iced::widget::tooltip(
            button(text(glyph).size(15)).on_press(press).padding([2, 7]).style(flat_button_style(p)),
            container(text(tip).size(12)).padding([4, 8]),
            iced::widget::tooltip::Position::Top,
        )
    };
    let (fill, edge) = (p.surface(), p.border_ctrl());
    container(
        row![
            action("🙂", "React", Message::ToggleReactPicker(id.to_string())),
            action("↩", "Reply", Message::ReplyTo(id.to_string(), quote_of(message))),
            action("↪", "Forward", Message::ForwardStart(id.to_string())),
        ]
        .spacing(2),
    )
    .padding(2)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    })
    .into()
}

/// WhatsApp's quick reactions; picking the one you already used takes it back.
const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

fn quick_reactions<'a>(message: &TimelineMessage, id: &str, p: &'a theme::Palette) -> Element<'a, Message> {
    // WhatsApp reports ❤ without the emoji variation selector; compare without it.
    let bare = |e: &str| e.trim_end_matches('\u{fe0f}').to_string();
    let mine = message.reactions.iter().find(|r| r.by_me).map(|r| bare(&r.key));
    let mut strip = row![].spacing(2);
    for emoji in QUICK_REACTIONS {
        let again = mine.as_deref() == Some(bare(emoji).as_str());
        strip = strip.push(
            button(text(emoji).size(18))
                .on_press(Message::React { id: id.to_string(), emoji: if again { String::new() } else { emoji.to_string() } })
                .padding([2, 6])
                .style(flat_button_style(p)),
        );
    }
    strip.into()
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

/// Splits text into (is_link, slice) runs: `http(s)://…` tokens at a word
/// boundary are links, everything else is plain. Trailing punctuation
/// (`.`, `)`, `,`…) stays out of the link so "see https://x.com." works.
fn link_segments(s: &str) -> Vec<(bool, &str)> {
    let mut segs = Vec::new();
    let (mut text_start, mut prev_ws) = (0usize, true);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if prev_ws && (s[i..].starts_with("http://") || s[i..].starts_with("https://")) {
            let rel = s[i..].find(char::is_whitespace).unwrap_or(s.len() - i);
            let url = s[i..i + rel].trim_end_matches([',', '.', '!', '?', ')', ']', '}', '>', '"', '\'', ':', ';']);
            let end = i + url.len();
            if text_start < i {
                segs.push((false, &s[text_start..i]));
            }
            segs.push((true, &s[i..end]));
            text_start = end;
            while chars.peek().is_some_and(|&(j, _)| j < end) {
                chars.next();
            }
            prev_ws = false;
            continue;
        }
        prev_ws = c.is_whitespace();
    }
    if text_start < s.len() {
        segs.push((false, &s[text_start..]));
    }
    segs
}

/// Prose with `inline code` spans and clickable links.
fn prose<'a>(body: &'a str, all_mono: bool, color: iced::Color, p: &theme::Palette) -> Element<'a, Message> {
    let font = if all_mono { ui::MONO } else { ui::SANS };
    let size = if all_mono { 12.0 } else { 14.0 };
    let has_link = body.contains("http://") || body.contains("https://");
    if !body.contains('`') && !has_link {
        return text(body).font(font).size(size).color(color).into();
    }
    let (code_fill, code_fg, link_color) = (p.surface_alt(), p.text_bright(), p.accent());
    let mut spans: Vec<iced::widget::text::Span<'a, String, iced::Font>> = Vec::new();
    for (i, part) in body.split('`').enumerate() {
        if i % 2 == 1 {
            spans.push(
                iced::widget::span(part)
                    .font(ui::MONO)
                    .size(12)
                    .color(code_fg)
                    .background(code_fill)
                    .border(iced::border::rounded(4))
                    .padding([1, 5]),
            );
        } else {
            for (is_link, seg) in link_segments(part) {
                let span = iced::widget::span(seg).font(font).size(size);
                spans.push(if is_link {
                    span.color(link_color).underline(true).link(seg.to_string())
                } else {
                    span.color(color)
                });
            }
        }
    }
    iced::widget::rich_text(spans).on_link_click(Message::OpenUrl).into()
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
    let key = info.media.key();
    let picture: Element<'_, Message> = match drawn_image(session, &key) {
        Some(image) => mouse_area(image.width(w).height(h).content_fit(iced::ContentFit::Contain))
        .on_press(Message::OpenImage(key.clone(), info.full.as_ref().map(MediaRef::key)))
        .interaction(iced::mouse::Interaction::Pointer)
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

/// A video as its preview with a ▶ on it; click to play it in your video
/// player. ponytail: the preview is WhatsApp's tiny one, so it's soft.
fn video_figure<'a>(
    info: &'a mesh_core::FileInfo,
    poster: &'a mesh_core::ImageInfo,
    session: &'a Session,
    p: &'a theme::Palette,
) -> Element<'a, Message> {
    const MAX: f32 = 320.0;
    let (w, h) = match (poster.width, poster.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let scale = (MAX / w as f32).min(MAX / h as f32).min(1.0);
            (w as f32 * scale, h as f32 * scale)
        }
        _ => (MAX, 180.0),
    };
    let (fill, edge, badge_fill) = (p.panel_dim(), p.border_ctrl(), theme::alpha(p.background(), 0.75));
    let picture: Element<'a, Message> = match session.images.get(&poster.media.key()) {
        Some(Some(handle)) => iced::widget::image(handle.clone()).width(w).height(h).content_fit(iced::ContentFit::Cover).into(),
        _ => Space::new().width(w).height(h).into(),
    };
    let badge = container(ui::mono("▶", 20.0, p.text_bright())).padding([8, 14]).style(move |_| container::Style {
        background: Some(badge_fill.into()),
        border: iced::border::rounded(24),
        ..container::Style::default()
    });
    let mut details = Vec::new();
    if let Some(secs) = info.duration {
        details.push(format!("{}:{:02}", secs as u64 / 60, secs as u64 % 60));
    }
    if let Some(size) = info.size {
        details.push(human_size(size));
    }
    let caption = container(
        row![ui::mono("video · click to play", 11.0, p.text_muted()).width(Length::Fill), ui::mono(details.join(" · "), 11.0, p.text_muted())]
            .spacing(10),
    )
    .padding([6, 10])
    .width(w);
    let url = info.media.web_url().unwrap_or_default().to_string();
    mouse_area(
        container(column![stack![picture, container(badge).width(w).height(h).center_x(w).center_y(h)], caption])
            .clip(true)
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::border::rounded(8).color(edge).width(1),
                ..container::Style::default()
            }),
    )
    .on_press(Message::OpenMedia(url))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

/// A voice note played inside Mesh: ▶/⏸, a text waveform (click a bar to
/// jump there) and the time.
fn voice_player<'a>(info: &'a mesh_core::FileInfo, session: &'a Session, p: &'a theme::Palette) -> Element<'a, Message> {
    const BARS: usize = 32;
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let rate = mesh_core::SAMPLE_RATE as usize;
    let key = info.media.key();
    let total = session
        .voice_pcm
        .get(&key)
        .map(|pcm| pcm.len())
        .unwrap_or_else(|| (info.duration.unwrap_or(0.0) * rate as f64) as usize)
        .max(1);
    let player = session.player.as_ref().filter(|pl| pl.key == key);
    let position = player.map_or(0, Player::position).min(total);
    let glyph = match player {
        Some(pl) if pl.loading => "…",
        Some(pl) if pl.out.is_some() => "⏸",
        _ => "▶",
    };
    let played = position as f32 / total as f32;
    let mut bars = row![];
    for (i, level) in waveform_bars(&info.waveform, BARS).into_iter().enumerate() {
        let at = i as f32 / BARS as f32;
        let color = if at < played { p.accent() } else { p.text_muted() };
        bars = bars.push(
            mouse_area(ui::mono(LEVELS[level as usize * 7 / 100].to_string(), 16.0, color))
                .on_press(Message::VoiceSeek(key.clone(), at))
                .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    let clock = |samples: usize| format!("{}:{:02}", samples / rate / 60, samples / rate % 60);
    let (fill, edge) = (p.surface_alt(), p.border_ctrl());
    container(
        row![
            button(ui::mono(glyph, 14.0, p.accent()))
                .on_press(Message::VoiceToggle(key.clone()))
                .padding([2, 8])
                .style(flat_button_style(p)),
            bars,
            ui::mono(format!("{} / {}", clock(position), clock(total)), 11.0, p.text_muted()),
            button(ui::mono(format!("{}×", session.voice_speed), 11.0, p.text()))
                .on_press(Message::VoiceSpeed)
                .padding([2, 6])
                .style(flat_button_style(p)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .padding([6, 10])
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    })
    .into()
}

/// A waveform (0–100 each) squeezed or stretched to `bars` levels (0–100),
/// keeping each span's peak; flat when there's none.
fn waveform_bars(wave: &[u8], bars: usize) -> Vec<u8> {
    (0..bars)
        .map(|i| {
            if wave.is_empty() {
                return 30;
            }
            let start = i * wave.len() / bars;
            let end = ((i + 1) * wave.len() / bars).max(start + 1).min(wave.len());
            wave[start..end].iter().copied().max().unwrap_or(0).min(100)
        })
        .collect()
}

/// Keeps a filename to its basename and safe characters, so a crafted name
/// can't escape the temp directory.
fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() { "mesh-file".to_string() } else { cleaned }
}

/// A video / voice note / document as a clickable card; opening it downloads
/// the file through the hidden client and hands it to the OS player/viewer.
fn file_figure<'a>(info: &'a mesh_core::FileInfo, p: &theme::Palette) -> Element<'a, Message> {
    let url = info.media.web_url().unwrap_or_default().to_string();
    let size = info.size.map(|s| format!(" · {}", human_size(s))).unwrap_or_default();
    let (surface, edge) = (p.surface_alt(), p.border_ctrl());
    let label = row![
        ui::icon(ui::Icon::Paperclip, 16.0, p.accent()),
        column![
            ui::sans(&info.name, 13.0, Weight::Semibold, p.text_bright()),
            ui::mono(format!("{}{size} · tap to open", info.kind), 11.0, p.text_muted()),
        ]
        .spacing(2),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);
    button(label)
        .on_press(Message::OpenMedia(url))
        .padding([8, 12])
        .style(move |_, status| button::Style {
            background: Some(if matches!(status, button::Status::Hovered) { edge } else { surface }.into()),
            border: iced::border::rounded(8).color(edge).width(1),
            ..button::Style::default()
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
    row![ui::mono(who, 11.0, net_color(Network::Matrix, p)), ui::mono(verb, 11.0, p.text_muted())].into()
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
fn settings_shell<'a>(session: &'a Session, state: &'a App, width: f32) -> Element<'a, Message> {
    let (p, l, x) = (&state.palette, &state.layout, &state.extras);
    let wide = width > 1100.0;
    let mut panes = row![
        tile(session, Pane::Channels, settings_nav(session, p, l), 220.0, p, l, x),
        tile(session, Pane::Chat, settings_page(session, state, width), 0.0, p, l, x),
    ]
    .spacing(l.gaps_out)
    .height(Length::Fill);
    if session.settings_page == SettingsPage::Accounts {
        panes = panes.push(ui::pane(accounts::panel(state), true, p, l, x.translucent_panes).height(Length::Fill));
    }
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
        item("Accounts", SettingsPage::Accounts),
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

fn settings_page<'a>(session: &'a Session, state: &'a App, width: f32) -> Element<'a, Message> {
    let p = &state.palette;
    let page: Element<'a, Message> = match session.settings_page {
        SettingsPage::Rice => rice_page(state),
        SettingsPage::Accounts => accounts::page(state, if width > 1300.0 { 4 } else { 2 }),
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
        table = table.push(token_row(key.to_string(), color, used_for.to_string(), source, net_color(Network::Matrix, p), p));
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
        switch_row("Digital rain background", "Animated canvas behind the panes, drawn in your accent and muted text", x.digital_rain, Some(Message::ToggleExtra(Extra::Rain)), p),
        switch_row("Translucent panes", "Panes at 92% opacity so Hyprland's blur shows through", x.translucent_panes, Some(Message::ToggleExtra(Extra::Translucent)), p),
        switch_row("Show platform tags", "mx / dc next to names, so you always know where a message goes", x.show_network_tags, Some(Message::ToggleExtra(Extra::Tags)), p),
        switch_row("Plain-text mode", "No image downloads or previews: just text, links and code", x.plain_text_mode, Some(Message::ToggleExtra(Extra::PlainText)), p),
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
/// A Home filter toggle: a network icon (tooltip = its name), lit when active.
fn filter_chip<'a>(
    filter: Option<Network>,
    icon: ui::Icon,
    label: &'a str,
    color: iced::Color,
    current: Option<Network>,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let on = current == filter;
    let (fill, radius) = (if on { p.surface_alt() } else { iced::Color::TRANSPARENT }, l.control_radius());
    let button = button(ui::icon(icon, 16.0, if on { color } else { p.text_muted() }))
        .on_press(Message::HomeFilter(filter))
        .padding([5, 9])
        .style(move |_, _| button::Style {
            background: Some(fill.into()),
            border: iced::border::rounded(radius),
            ..button::Style::default()
        });
    iced::widget::tooltip(
        button,
        container(text(label).size(12).color(p.text())).padding([4, 8]),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

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

fn switch_row<'a>(title: &'a str, sub: &'a str, on: bool, msg: Option<Message>, p: &theme::Palette) -> Element<'a, Message> {
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
        .on_press_maybe(msg)
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
    let discord = match &session.discord_voice {
        Some(theme::DiscordVoice::Url(url)) => format!("The 🎧 button opens {url} for Discord voice."),
        Some(theme::DiscordVoice::Command(cmd)) => format!("The 🎧 button runs `{cmd}` for Discord voice."),
        None => "Set discord_voice_command or discord_voice_url in config.toml to get a 🎧 button for Discord voice.".to_string(),
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
    Detected::Bad(format!("Not a {} ID or {} username.", Network::Matrix.spec().name, Network::Discord.spec().name))
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
        Detected::Matrix { .. } => (Network::Matrix.tag(), net_color(Network::Matrix, p)),
        Detected::Discord(_) => (Network::Discord.tag(), net_color(Network::Discord, p)),
        _ => ("··", p.text_muted()),
    };
    let (hint, hint_color) = match &detected {
        Detected::Empty => (
            "Start typing — @user:server for Matrix, a plain username for Discord.".to_string(),
            p.text_muted(),
        ),
        Detected::Matrix { id, guessed: true } => (format!("✓ {} · assumed your homeserver — {id}", Network::Matrix.tag()), net_color(Network::Matrix, p)),
        Detected::Matrix { id, .. } => (
            format!("✓ {} user on {}", Network::Matrix.tag(), id.split_once(':').map_or("", |(_, s)| s)),
            net_color(Network::Matrix, p),
        ),
        Detected::Discord(_) => (format!("✓ {} username · via your bridge on {server}", Network::Discord.tag()), net_color(Network::Discord, p)),
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
            ui::icon_button(ui::Icon::Close, Some(Message::Escape), false, p, l),
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
            Some((name, format!("{id}  →  from {me}"), net_color(Network::Matrix, p)))
        }
        Detected::Discord(name) => Some((name.clone(), format!("{name}  →  through your bridge on {server}"), net_color(Network::Discord, p))),
        _ => None,
    };
    if let Some((name, route, color)) = preview {
        let (card, card_edge) = (theme::mix(p.surface(), p.surface_alt(), 0.4), p.border_ctrl());
        col = col.push(
            container(
                row![
                    ui::avatar(&name, None, 40.0, color, p, None),
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
            ui::avatar(contact.user_id.localpart(), None, 32.0, color, p, None),
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

/// The join-a-Discord-server dialog's state.
#[derive(Default)]
struct JoinServer {
    link: String,
    /// Waiting on Discord (or on a captcha in its window).
    busy: bool,
    /// Last result, and whether it's an error.
    note: Option<(String, bool)>,
}

/// Text input id of the invite field, to focus it on open.
const INVITE_INPUT: &str = "invite";

/// The join-a-Discord-server dialog, over the shell.
fn join_server_view<'a>(
    dialog: &'a JoinServer,
    engine_up: bool,
    p: &'a theme::Palette,
    l: &theme::Layout,
) -> Element<'a, Message> {
    let ready = engine_up && !dialog.busy && !dialog.link.trim().is_empty();
    let join = ui::primary_button(
        ui::sans(if dialog.busy { "Joining…" } else { "Join" }, 14.0, Weight::Semibold, p.accent_text()),
        ready.then_some(Message::JoinServerSubmit),
        p,
        l,
    )
    .padding([10, 16]);
    let (muted, bright, selection) = (p.text_muted(), p.text_bright(), theme::alpha(p.accent(), 0.35));
    let input = text_input("https://discord.gg/…", &dialog.link)
        .id(iced::widget::Id::new(INVITE_INPUT))
        .on_input(Message::JoinServerLink)
        .on_submit(Message::JoinServerSubmit)
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
    let (fill, edge) = (p.background(), p.border_ctrl());
    let field = container(
        row![container(ui::mono_bold(Network::Discord.tag(), 11.0, net_color(Network::Discord, p))).width(20), input, join]
            .spacing(10)
            .align_y(iced::Alignment::Center),
    )
    .padding(iced::Padding { top: 4.0, right: 4.0, bottom: 4.0, left: 14.0 })
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::border::rounded(8).color(edge).width(1),
        ..container::Style::default()
    });

    let mut col = column![
        row![
            column![
                ui::sans("Join a Discord server", 20.0, Weight::Semibold, p.text_bright()),
                text("Paste an invite link. Mesh accepts it through your Discord account.")
                    .size(14)
                    .color(p.text_muted()),
            ]
            .spacing(4)
            .width(Length::Fill),
            ui::icon_button(ui::Icon::Close, Some(Message::Escape), false, p, l),
        ],
        column![ui::mono("INVITE LINK", 11.0, p.text_muted()), field].spacing(8),
    ]
    .spacing(18);
    if !engine_up {
        col = col.push(ui::mono("Mesh's Discord client isn't running; see the README.", 12.0, p.danger()));
    }
    if let Some((note, error)) = &dialog.note {
        col = col.push(ui::mono(note, 12.0, if *error { p.danger() } else { p.success() }));
    }

    let scrim = theme::alpha(p.background(), 0.6);
    container(iced::widget::opaque(ui::pane(col.padding(24), true, p, l, false).max_width(560)))
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
    std::thread::spawn(cache::prune);

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
    fn links_split_from_text() {
        assert_eq!(link_segments("plain text"), [(false, "plain text")]);
        assert_eq!(
            link_segments("see https://x.com/a now"),
            [(false, "see "), (true, "https://x.com/a"), (false, " now")]
        );
        // Trailing punctuation stays out of the link.
        assert_eq!(
            link_segments("go to http://a.co."),
            [(false, "go to "), (true, "http://a.co"), (false, ".")]
        );
        // Not a link mid-word (no boundary), and unicode before it is fine.
        assert_eq!(link_segments("café https://é.example"), [(false, "café "), (true, "https://é.example")]);
        assert_eq!(link_segments("email=http://x"), [(false, "email=http://x")]);
    }

    #[test]
    fn gif_frames_round_trip_and_waveform_fits() {
        let frames = vec![b"one".to_vec(), Vec::new(), b"three".to_vec()];
        assert_eq!(unpack_frames(&pack_frames(83, &frames)), Some((83, frames)));
        assert_eq!(unpack_frames(&[1, 0]), None);
        assert_eq!(waveform_bars(&[10, 90, 20, 40], 2), [90, 40]);
        assert_eq!(waveform_bars(&[50], 3), [50, 50, 50]);
        assert_eq!(waveform_bars(&[], 2), [30, 30]);
    }

    #[test]
    fn tells_numbers_from_names() {
        assert!(looks_like_number("082 123 4567"));
        assert!(looks_like_number("+27 (82) 123-4567"));
        assert!(!looks_like_number("Brendon"));
        assert!(!looks_like_number("123"));
        assert!(!looks_like_number("Room 1234567"));
    }

    #[test]
    fn filenames_cant_escape() {
        assert_eq!(safe_filename("video.mp4"), "video.mp4");
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename(r"C:\Windows\evil.exe"), "evil.exe");
        assert_eq!(safe_filename("..."), "mesh-file");
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
