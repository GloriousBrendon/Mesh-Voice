//! Discord voice for Mesh, driven through a hidden official Discord web
//! client. The client runs in its own browser (the `discord-voice` launcher,
//! behind the VPN); this crate reaches it over the Chrome DevTools Protocol
//! on a unix socket, installs `agent.js` in the page, and streams voice state
//! back. Mesh never talks to Discord itself, and stays Matrix-only in
//! `mesh-core`.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const AGENT: &str = include_str!("agent.js");

/// What Mesh hears from the engine.
#[derive(Debug, Clone)]
pub enum Event {
    /// Connected to the hidden client; use the handle to act.
    Connected(Engine),
    Snapshot(Snapshot),
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
    pub voice_channels: Vec<VoiceChannel>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceChannel {
    pub id: String,
    pub name: String,
    pub members: Vec<Member>,
}

impl Voice {
    pub fn connected(&self) -> bool {
        self.channel_id.is_some()
    }
}

/// Keeps a connection to the engine at `socket` for as long as `events` is
/// open, reconnecting every few seconds when the hidden client isn't there.
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
    let stream = UnixStream::connect(socket).await.map_err(|e| e.to_string())?;
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
                if let Some(snapshot) = binding_snapshot(&text) {
                    events.send(Event::Snapshot(snapshot)).map_err(|_| "Mesh went away".to_string())?;
                }
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

/// A `meshEmit` binding call carrying a snapshot, if `text` is one.
fn binding_snapshot(text: &str) -> Option<Snapshot> {
    let msg: serde_json::Value = serde_json::from_str(text).ok()?;
    if msg["method"] != "Runtime.bindingCalled" || msg["params"]["name"] != "meshEmit" {
        return None;
    }
    serde_json::from_str(msg["params"]["payload"].as_str()?).ok()
}

/// The DevTools websocket path of the Discord tab.
async fn discord_page(socket: &Path) -> Result<String> {
    let mut stream = UnixStream::connect(socket).await.map_err(|e| e.to_string())?;
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
    fn parses_binding_snapshot() {
        let payload = r#"{"me":{"id":"1","name":"me","avatar":null},"voice":{"channel_id":"9","guild_id":null,"channel_name":"call","connection":"RTC_CONNECTED","self_mute":false,"self_deaf":false,"members":[{"id":"2","name":"b","avatar":null,"speaking":true,"mute":false,"deaf":false}]},"guilds":[]}"#;
        let msg = serde_json::json!({ "method": "Runtime.bindingCalled", "params": { "name": "meshEmit", "payload": payload } });
        let snapshot = binding_snapshot(&msg.to_string()).unwrap();
        assert_eq!(snapshot.voice.channel_id.as_deref(), Some("9"));
        assert!(snapshot.voice.members[0].speaking);
        assert!(binding_snapshot(r#"{"id":3,"result":{}}"#).is_none());
    }
}
