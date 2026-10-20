//! Discord voice for Mesh, driven through a hidden official Discord web
//! client. The client runs in its own browser (the `discord-voice` launcher,
//! behind the VPN, on Linux; Edge on Windows); this crate reaches it over the
//! Chrome DevTools Protocol on a unix socket or a localhost TCP port, installs
//! `agent.js` in the page, and streams voice state back. Mesh never talks to Discord itself, and stays Matrix-only in
//! `mesh-core`.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const AGENT: &str = include_str!("agent.js");

/// What Mesh hears from the engine.
#[derive(Debug, Clone)]
pub enum Event {
    /// Connected to the hidden client; use the handle to act.
    Connected(Engine),
    Snapshot(Snapshot),
    /// How an `Engine::accept_invite` is going.
    Invite(Invite),
    /// Lost it (browser closed, page gone); reconnects on its own.
    Disconnected,
}

/// Commands into the hidden client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::UnboundedSender<String>,
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
    /// Joins a server from an invite link or code; answers with `Event::Invite`.
    pub fn accept_invite(&self, link: &str) {
        self.call("acceptInvite", &[link]);
    }

    fn call(&self, function: &str, args: &[&str]) {
        let args: Vec<String> = args
            .iter()
            .map(|a| serde_json::to_string(a).unwrap_or_default())
            .collect();
        let _ = self
            .commands
            .send(format!("window.__mesh && window.__mesh.{function}({})", args.join(",")));
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

impl Voice {
    pub fn connected(&self) -> bool {
        self.channel_id.is_some()
    }
}

/// Keeps a connection to the engine at `socket` for as long as `events` is
/// open, reconnecting every few seconds when the hidden client isn't there.
/// `socket` is a unix socket path, or `host:port` for a browser started with
/// `--remote-debugging-port` (Windows).
pub async fn run(socket: PathBuf, events: mpsc::UnboundedSender<Event>) {
    while !events.is_closed() {
        let mut connected = false;
        if let Err(err) = session(&socket, &events, &mut connected).await {
            tracing::debug!("discord engine: {err}");
        }
        if connected {
            let _ = events.send(Event::Disconnected);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

type Result<T> = std::result::Result<T, String>;

async fn session(
    socket: &Path,
    events: &mpsc::UnboundedSender<Event>,
    connected: &mut bool,
) -> Result<()> {
    let page = discord_page(socket).await?;
    let stream = connect(socket).await?;
    let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://localhost{page}"), stream)
        .await
        .map_err(|e| e.to_string())?;

    let mut id = 0u64;
    let mut send = |method: &str, params: serde_json::Value| {
        id += 1;
        WsMessage::text(serde_json::json!({ "id": id, "method": method, "params": params }).to_string())
    };
    // The binding is how the page pushes snapshots; the new-document script
    // reinstalls the agent whenever Discord reloads.
    for msg in [
        send("Runtime.addBinding", serde_json::json!({ "name": "meshEmit" })),
        send("Runtime.addBinding", serde_json::json!({ "name": "meshInvite" })),
        send("Page.addScriptToEvaluateOnNewDocument", serde_json::json!({ "source": AGENT })),
        send("Runtime.evaluate", serde_json::json!({ "expression": AGENT })),
        send(
            "Runtime.evaluate",
            serde_json::json!({ "expression": "window.__mesh && window.__mesh.ready() && window.meshEmit(window.__mesh.snapshot())" }),
        ),
    ] {
        ws.send(msg).await.map_err(|e| e.to_string())?;
    }

    let (commands, mut command_rx) = mpsc::unbounded_channel();
    events
        .send(Event::Connected(Engine { commands }))
        .map_err(|_| "Mesh went away".to_string())?;
    *connected = true;

    let mut commands_open = true;
    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(incoming) = incoming else { return Err("engine closed".into()) };
                let WsMessage::Text(text) = incoming.map_err(|e| e.to_string())? else { continue };
                let Some(event) = binding_event(&text) else { continue };
                if matches!(event, Event::Invite(Invite::Waiting)) {
                    // Restores the window even when it's minimized.
                    ws.send(send("Page.bringToFront", serde_json::json!({}))).await.map_err(|e| e.to_string())?;
                }
                events.send(event).map_err(|_| "Mesh went away".to_string())?;
            }
            // Handles may all be dropped (nobody acting); keep streaming state.
            command = command_rx.recv(), if commands_open => {
                let Some(expression) = command else {
                    commands_open = false;
                    continue;
                };
                let msg = send("Runtime.evaluate", serde_json::json!({ "expression": expression }));
                ws.send(msg).await.map_err(|e| e.to_string())?;
            }
        }
    }
}

/// The event a binding call from the agent carries, if `text` is one.
fn binding_event(text: &str) -> Option<Event> {
    let msg: serde_json::Value = serde_json::from_str(text).ok()?;
    if msg["method"] != "Runtime.bindingCalled" {
        return None;
    }
    let payload = msg["params"]["payload"].as_str()?;
    match msg["params"]["name"].as_str()? {
        "meshEmit" => serde_json::from_str(payload).ok().map(Event::Snapshot),
        "meshInvite" => serde_json::from_str(payload).ok().map(Event::Invite),
        _ => None,
    }
}

/// Whether something is listening at the engine address (cheap; for deciding
/// whether to start the engine).
pub fn is_running(socket: &Path) -> bool {
    match tcp_address(socket) {
        Some(addr) => std::net::ToSocketAddrs::to_socket_addrs(addr)
            .ok()
            .and_then(|mut a| a.next())
            .is_some_and(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_ok()),
        None => socket.exists(),
    }
}

/// `host:port` if the engine address is one rather than a socket path.
fn tcp_address(socket: &Path) -> Option<&str> {
    let s = socket.to_str()?;
    let (host, port) = s.rsplit_once(':')?;
    let path_like = host.is_empty() || s.contains(['/', '\\']);
    (!path_like && port.parse::<u16>().is_ok()).then_some(s)
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

async fn connect(socket: &Path) -> Result<Box<dyn Io>> {
    if let Some(addr) = tcp_address(socket) {
        let stream = tokio::net::TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
        return Ok(Box::new(stream));
    }
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(socket).await.map_err(|e| e.to_string())?;
        Ok(Box::new(stream))
    }
    #[cfg(not(unix))]
    Err(format!("{} isn't host:port", socket.display()))
}

/// The DevTools websocket path of the Discord tab.
async fn discord_page(socket: &Path) -> Result<String> {
    let mut stream = connect(socket).await?;
    stream
        .write_all(b"GET /json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .map_err(|e| e.to_string())?;
    // DevTools keeps the connection open, so read exactly Content-Length.
    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .map_err(|_| "DevTools didn't answer".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&response);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_owned()))
                .and_then(|v| v.parse::<usize>().ok());
            if length.is_some_and(|len| body.len() >= len) {
                break;
            }
        }
    }
    let response = String::from_utf8_lossy(&response);
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or_default();
    page_path(body).ok_or_else(|| "no Discord tab open".to_string())
}

fn page_path(json: &str) -> Option<String> {
    let targets: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let page = targets
        .iter()
        .find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.contains("discord.com")))?;
    let url = page["webSocketDebuggerUrl"].as_str()?;
    // ws://localhost/devtools/page/<id> → /devtools/page/<id>
    Some(url.split_once("://")?.1.split_once('/').map(|(_, p)| format!("/{p}"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_discord_tab() {
        let json = r#"[{"type":"service_worker","url":"https://discord.com/sw.js","webSocketDebuggerUrl":"ws://localhost/devtools/x"},
                       {"type":"page","url":"https://discord.com/channels/@me","webSocketDebuggerUrl":"ws://localhost/devtools/page/ABC"}]"#;
        assert_eq!(page_path(json).as_deref(), Some("/devtools/page/ABC"));
        assert_eq!(page_path("[]"), None);
    }

    #[test]
    fn tells_tcp_from_socket_paths() {
        assert_eq!(tcp_address(Path::new("127.0.0.1:9222")), Some("127.0.0.1:9222"));
        assert_eq!(tcp_address(Path::new("localhost:9222")), Some("localhost:9222"));
        assert_eq!(tcp_address(Path::new("/run/user/1000/discord-engine.sock")), None);
        assert_eq!(tcp_address(Path::new(r"C:\Users\me\engine.sock")), None);
    }

    #[test]
    fn parses_binding_snapshot() {
        let payload = r#"{"me":{"id":"1","name":"me","avatar":null},"voice":{"channel_id":"9","guild_id":null,"channel_name":"call","connection":"RTC_CONNECTED","self_mute":false,"self_deaf":false,"members":[{"id":"2","name":"b","avatar":null,"speaking":true,"mute":false,"deaf":false}]},"guilds":[]}"#;
        let msg = serde_json::json!({ "method": "Runtime.bindingCalled", "params": { "name": "meshEmit", "payload": payload } });
        let Some(Event::Snapshot(snapshot)) = binding_event(&msg.to_string()) else { panic!("no snapshot") };
        assert_eq!(snapshot.voice.channel_id.as_deref(), Some("9"));
        assert!(snapshot.voice.members[0].speaking);
        assert!(binding_event(r#"{"id":3,"result":{}}"#).is_none());
    }

    #[test]
    fn parses_invite_results() {
        let invite = |payload: &str| {
            let msg = serde_json::json!({ "method": "Runtime.bindingCalled", "params": { "name": "meshInvite", "payload": payload } });
            match binding_event(&msg.to_string()) {
                Some(Event::Invite(invite)) => invite,
                other => panic!("not an invite: {other:?}"),
            }
        };
        assert_eq!(invite(r#"{"state":"waiting"}"#), Invite::Waiting);
        assert_eq!(invite(r#"{"state":"joined","guild":"404"}"#), Invite::Joined { guild: Some("404".into()) });
        assert_eq!(
            invite(r#"{"state":"failed","error":"Unknown Invite"}"#),
            Invite::Failed { error: "Unknown Invite".into() }
        );
    }
}
