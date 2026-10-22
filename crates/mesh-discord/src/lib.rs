//! Discord (and, in [`whatsapp`], WhatsApp) for Mesh, driven through the
//! official web clients running hidden in the engine browser (the
//! `discord-voice` launcher, behind the VPN, on Linux; Edge on Windows; later
//! the homelab). [`engine`] reaches each tab over the Chrome DevTools Protocol
//! and installs its agent; this module is Discord's side. Mesh never talks to
//! Discord itself, and stays Matrix-only in `mesh-core`.
//! ponytail: the crate is still named for Discord; rename it (mesh-web) when
//! a third site arrives.

mod engine;
pub mod whatsapp;

pub use engine::{Qr, Video, is_running, open, show};
use serde::Deserialize;
use std::path::PathBuf;
use tokio::sync::mpsc;

const AGENT: &str = include_str!("agent.js");

/// What Mesh hears from the engine.
#[derive(Debug, Clone)]
pub enum Event {
    /// Connected to the hidden client; use the handle to act.
    Connected(Engine),
    Snapshot(Snapshot),
    /// How an `Engine::accept_invite` is going.
    Invite(Invite),
    /// The open channel's latest messages (after `Engine::open_channel`, and
    /// again whenever they change).
    Messages(Messages),
    /// A new DM, or a mention of us, worth a desktop notification.
    Notify(Notify),
    /// A picture asked for with `Engine::fetch_media`: its bytes, or why not.
    Media { url: String, bytes: std::result::Result<Vec<u8>, String> },
    /// Logged out: the login page's current QR code (a new one every so often).
    Qr(Qr),
    /// A video call frame (someone's camera, or ours), or its end.
    Video(Video),
    /// Lost it (browser closed, page gone); reconnects on its own.
    Disconnected,
}

/// Commands into the hidden client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::UnboundedSender<engine::Command>,
}

impl Engine {
    /// Joins a server voice channel, or starts/joins a DM call (which rings).
    pub fn join(&self, channel_id: &str) {
        self.call("join", &[channel_id]);
    }
    pub fn leave(&self) {
        self.call("leave", &[]);
    }
    pub fn toggle_mute(&self) {
        self.call("toggleMute", &[]);
    }
    pub fn toggle_deaf(&self) {
        self.call("toggleDeaf", &[]);
    }
    /// Turns our camera on or off, in the call or for the next one.
    pub fn set_video(&self, on: bool) {
        self.call("setVideo", &[if on { "true" } else { "false" }]);
    }
    /// Starts or stops watching someone's screen share.
    pub fn watch_stream(&self, owner_id: &str, watch: bool) {
        self.call(if watch { "watchStream" } else { "stopWatching" }, &[owner_id]);
    }
    /// Shares our screen (or a window) in the call, or stops. Sharing brings
    /// the Discord window up, since the browser asks what to share there.
    pub fn share_screen(&self, on: bool) {
        if on {
            let _ = self.commands.send(engine::Command::BringToFront);
        }
        self.call("shareScreen", &[if on { "true" } else { "false" }]);
    }
    /// Joins a server from an invite link or code; answers with `Event::Invite`.
    pub fn accept_invite(&self, link: &str) {
        self.call("acceptInvite", &[link]);
    }
    /// Shows a text channel or DM in Mesh: its messages stream back as
    /// `Event::Messages` until another channel is opened or it's closed.
    pub fn open_channel(&self, channel_id: &str) {
        self.call("open", &[channel_id]);
    }
    pub fn close_channel(&self) {
        self.call("open", &[]);
    }
    /// Logs this Discord session out; the tab goes back to its login page.
    pub fn sign_out(&self) {
        self.call("signOut", &[]);
    }
    /// Sends through Discord's own client (which handles captchas, slowmode
    /// and the rest itself).
    pub fn send_message(&self, channel_id: &str, content: &str) {
        self.call("send", &[channel_id, content]);
    }
    /// Downloads a Discord picture through the client's own network (the VPN
    /// on Linux); answers with `Event::Media`.
    pub fn fetch_media(&self, url: &str) {
        self.call("fetchMedia", &[url]);
    }

    fn call(&self, function: &str, args: &[&str]) {
        let _ = self.commands.send(engine::call(function, args));
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Invite {
    /// Discord hasn't answered yet, most likely because it's showing a
    /// captcha; the engine window has been brought up for it.
    Waiting,
    Joined { guild: Option<String> },
    Failed { error: String },
}

/// Voice state as the official client sees it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    pub me: Option<User>,
    pub voice: Voice,
    pub guilds: Vec<Guild>,
    /// DMs and group DMs, most recent first.
    #[serde(default)]
    pub dms: Vec<Dm>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Dm {
    pub id: String,
    pub name: String,
    /// The other person, for a one-to-one DM.
    pub recipient_id: Option<String>,
    /// The other person's picture, or a group DM's icon.
    #[serde(default)]
    pub avatar: Option<String>,
    /// When the last message was sent (ms since the epoch), for sorting.
    #[serde(default)]
    pub last_ms: u64,
    pub unread: bool,
    pub mentions: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Notify {
    pub channel_id: String,
    /// "B345T", or "B345T in #general".
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Messages {
    pub channel_id: String,
    /// Oldest first.
    pub messages: Vec<DiscordMessage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiscordMessage {
    pub id: String,
    pub author_id: String,
    pub author: String,
    /// The author's picture (fetch it with `Engine::fetch_media`).
    #[serde(default)]
    pub avatar: Option<String>,
    /// The text, or for a system message what happened ("started a call").
    pub content: String,
    pub system: bool,
    pub timestamp_ms: u64,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
    /// For images: a small copy from Discord's resizing proxy.
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub width: Option<u64>,
    #[serde(default)]
    pub height: Option<u64>,
    /// Bytes.
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

/// Where we are in voice right now.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Voice {
    pub channel_id: Option<String>,
    pub guild_id: Option<String>,
    pub channel_name: Option<String>,
    /// Discord's RTC state, e.g. `RTC_CONNECTED`, `DISCONNECTED`.
    pub connection: String,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Our camera is on.
    #[serde(default)]
    pub self_video: bool,
    /// Other people's screen shares in this channel.
    #[serde(default)]
    pub streams: Vec<Stream>,
    /// We're sharing our screen.
    #[serde(default)]
    pub self_stream: bool,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub speaking: bool,
    pub mute: bool,
    pub deaf: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub text_channels: Vec<TextChannel>,
    pub voice_channels: Vec<VoiceChannel>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextChannel {
    pub id: String,
    pub name: String,
    /// The category it's under, if any.
    pub category: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceChannel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    pub members: Vec<Member>,
}

/// Someone's screen share (Go Live) in our voice channel.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Stream {
    pub owner_id: String,
    pub name: String,
    /// We're watching it (its video comes in as a call video).
    pub watching: bool,
}

impl Voice {
    pub fn connected(&self) -> bool {
        self.channel_id.is_some()
    }
}

/// Keeps a connection to the Discord tab at `socket` for as long as `events`
/// is open, reconnecting every few seconds when it isn't there. `socket` is a
/// unix socket path, or `host:port` for a browser started with
/// `--remote-debugging-port` (Windows).
pub async fn run(socket: PathBuf, events: mpsc::UnboundedSender<Event>) {
    let site = engine::Site {
        host: "discord.com",
        agent: AGENT,
        bindings: &["meshEmit", "meshInvite", "meshMessages", "meshNotify", "meshMedia", "meshQr", "meshVideo"],
        kickoff: "window.__mesh && window.__mesh.ready() && window.meshEmit(window.__mesh.snapshot())",
    };
    let (tx, mut raw) = mpsc::unbounded_channel();
    tokio::spawn(engine::run(socket, site, tx));
    let mut commands = None;
    while let Some(raw) = raw.recv().await {
        let event = match raw {
            engine::Raw::Connected(tx) => {
                commands = Some(tx.clone());
                Event::Connected(Engine { commands: tx })
            }
            engine::Raw::Binding { name, payload } => {
                let Some(event) = binding_event(&name, &payload) else { continue };
                if matches!(event, Event::Invite(Invite::Waiting))
                    && let Some(commands) = &commands
                {
                    let _ = commands.send(engine::Command::BringToFront);
                }
                event
            }
            engine::Raw::Disconnected => Event::Disconnected,
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

/// Drives the native Discord helper (`mesh-dc.exe`) over stdio instead of the
/// engine browser: the same bindings, as `{"binding":..,"payload":".."}` lines,
/// and commands in as `{"fn":..,"args":[..]}`. Mirrors `whatsapp::run_native`.
/// Restarts the helper by itself while `events` is open.
pub async fn run_native(exe: PathBuf, data: PathBuf, events: mpsc::UnboundedSender<Event>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    while !events.is_closed() {
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("--data").arg(&data).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("could not start {}: {err}", exe.display());
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        };
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { return };
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut connected = false;
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let Some((name, payload)) = native_line(&line) else { continue };
                    let event = if name == "dcReady" {
                        connected = true;
                        Event::Connected(Engine { commands: commands.clone() })
                    } else {
                        match binding_event(&name, &payload) {
                            Some(event) => event,
                            None => continue,
                        }
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                }
                command = command_rx.recv() => {
                    let Some(engine::Command::Call { function, args }) = command else { continue };
                    let line = serde_json::json!({ "fn": function, "args": args }).to_string() + "\n";
                    if stdin.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = child.kill().await;
        if connected && events.send(Event::Disconnected).is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// A helper stdout line's (binding, payload).
fn native_line(line: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    Some((v["binding"].as_str()?.to_owned(), v["payload"].as_str()?.to_owned()))
}

/// The event an agent binding call carries, if it's one we know.
fn binding_event(name: &str, payload: &str) -> Option<Event> {
    match name {
        "meshEmit" => serde_json::from_str(payload).ok().map(Event::Snapshot),
        "meshInvite" => serde_json::from_str(payload).ok().map(Event::Invite),
        "meshMessages" => serde_json::from_str(payload).ok().map(Event::Messages),
        "meshNotify" => serde_json::from_str(payload).ok().map(Event::Notify),
        "meshMedia" => engine::media(payload).map(|(url, bytes)| Event::Media { url, bytes }),
        "meshQr" => engine::qr(payload).map(Event::Qr),
        "meshVideo" => engine::video(payload).map(Event::Video),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binding_snapshot() {
        let payload = r#"{"me":{"id":"1","name":"me","avatar":null},"voice":{"channel_id":"9","guild_id":null,"channel_name":"call","connection":"RTC_CONNECTED","self_mute":false,"self_deaf":false,"members":[{"id":"2","name":"b","avatar":null,"speaking":true,"mute":false,"deaf":false}]},"guilds":[]}"#;
        let Some(Event::Snapshot(snapshot)) = binding_event("meshEmit", payload) else { panic!("no snapshot") };
        assert_eq!(snapshot.voice.channel_id.as_deref(), Some("9"));
        assert!(snapshot.voice.members[0].speaking);
        assert!(binding_event("somethingElse", "{}").is_none());
    }

    #[test]
    fn parses_messages() {
        let payload = r#"{"channel_id":"5","messages":[{"id":"1","author_id":"2","author":"B345T","content":"hi","system":false,"timestamp_ms":1760889619720,"attachments":[{"filename":"a.png","url":"https://cdn/a.png"}]}]}"#;
        let Some(Event::Messages(m)) = binding_event("meshMessages", payload) else { panic!("no messages") };
        assert_eq!(m.channel_id, "5");
        assert_eq!(m.messages[0].author, "B345T");
        assert_eq!(m.messages[0].attachments[0].filename, "a.png");

        let payload = r#"{"channel_id":"5","title":"B345T","body":"hi"}"#;
        let Some(Event::Notify(n)) = binding_event("meshNotify", payload) else { panic!("no notify") };
        assert_eq!((n.channel_id.as_str(), n.title.as_str()), ("5", "B345T"));
    }

    #[test]
    fn decodes_media() {
        let media = |payload: &str| match binding_event("meshMedia", payload) {
            Some(Event::Media { url, bytes }) => (url, bytes),
            other => panic!("not media: {other:?}"),
        };
        assert_eq!(media(r#"{"url":"u","data":"aGk="}"#), ("u".into(), Ok(b"hi".to_vec())));
        assert_eq!(media(r#"{"url":"u","error":"HTTP 404"}"#), ("u".into(), Err("HTTP 404".into())));
    }

    #[test]
    fn parses_invite_results() {
        let invite = |payload: &str| match binding_event("meshInvite", payload) {
            Some(Event::Invite(invite)) => invite,
            other => panic!("not an invite: {other:?}"),
        };
        assert_eq!(invite(r#"{"state":"waiting"}"#), Invite::Waiting);
        assert_eq!(invite(r#"{"state":"joined","guild":"404"}"#), Invite::Joined { guild: Some("404".into()) });
        assert_eq!(
            invite(r#"{"state":"failed","error":"Unknown Invite"}"#),
            Invite::Failed { error: "Unknown Invite".into() }
        );
    }
}
