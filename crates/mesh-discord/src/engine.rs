//! The engine browser, platform-neutral: finds a site's tab over the Chrome
//! DevTools Protocol (unix socket or host:port), installs that site's agent
//! (again on every reload), and relays the agent's binding calls out and
//! commands in. Each site (Discord, WhatsApp) maps these to its own events.

use futures_util::{SinkExt, StreamExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// A web client the engine drives.
pub(crate) struct Site {
    /// Picks its tab: the first page whose URL contains this.
    pub host: &'static str,
    /// Installed as `window.__mesh`.
    pub agent: &'static str,
    /// The `window.<name>(payload)` functions the agent reports through.
    pub bindings: &'static [&'static str],
    /// Evaluated once the agent is in, to push the current state.
    pub kickoff: &'static str,
}

pub(crate) enum Command {
    /// A JavaScript expression to evaluate in the page.
    Eval(String),
    /// Restore and focus the tab's window (for a captcha, or the browser's
    /// "choose what to share" picker).
    BringToFront,
}

pub(crate) enum Raw {
    Connected(mpsc::UnboundedSender<Command>),
    Binding { name: String, payload: String },
    Disconnected,
}

/// `window.__mesh.<function>(args…)` with JSON-quoted string arguments.
pub(crate) fn call(function: &str, args: &[&str]) -> Command {
    let args: Vec<String> = args.iter().map(|a| serde_json::to_string(a).unwrap_or_default()).collect();
    Command::Eval(format!("window.__mesh && window.__mesh.{function}({})", args.join(",")))
}

/// Keeps a connection to `site` at `socket` for as long as `raw` is open,
/// reconnecting every few seconds when the tab isn't there. `socket` is a
/// unix socket path, or `host:port` for a browser started with
/// `--remote-debugging-port`.
pub(crate) async fn run(socket: PathBuf, site: Site, raw: mpsc::UnboundedSender<Raw>) {
    while !raw.is_closed() {
        let mut connected = false;
        if let Err(err) = session(&socket, &site, &raw, &mut connected).await {
            tracing::debug!("engine ({}): {err}", site.host);
        }
        if connected {
            let _ = raw.send(Raw::Disconnected);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

type Result<T> = std::result::Result<T, String>;

async fn session(socket: &Path, site: &Site, raw: &mpsc::UnboundedSender<Raw>, connected: &mut bool) -> Result<()> {
    let page = page(socket, site.host).await?;
    let stream = connect(socket).await?;
    // A decrypted video arrives base64 in one binding payload, so lift the
    // default 16 MiB frame / 64 MiB message caps well past any attachment.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(256 << 20))
        .max_message_size(Some(256 << 20));
    // A page that's still being created can leave the handshake hanging.
    let handshake = tokio_tungstenite::client_async_with_config(format!("ws://localhost{page}"), stream, Some(config));
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(10), handshake)
        .await
        .map_err(|_| "DevTools handshake timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let mut id = 0u64;
    let mut send = |method: &str, params: serde_json::Value| {
        id += 1;
        WsMessage::text(serde_json::json!({ "id": id, "method": method, "params": params }).to_string())
    };
    // Camera and mic for this site without a prompt nobody would see. The
    // grant lasts as long as this connection, which is as long as Mesh needs
    // it; screen capture still goes through the browser's own picker.
    let origin = format!("https://{}", site.host);
    let mut setup: Vec<WsMessage> = vec![
        send("Browser.grantPermissions", serde_json::json!({ "origin": origin, "permissions": ["videoCapture", "audioCapture"] })),
    ];
    // Bindings are how the page reports; the new-document script reinstalls
    // the agent whenever the site reloads.
    setup.extend(site.bindings.iter().map(|name| send("Runtime.addBinding", serde_json::json!({ "name": name }))));
    setup.push(send("Page.addScriptToEvaluateOnNewDocument", serde_json::json!({ "source": site.agent })));
    setup.push(send("Runtime.evaluate", serde_json::json!({ "expression": site.agent })));
    setup.push(send("Runtime.evaluate", serde_json::json!({ "expression": site.kickoff })));
    for msg in setup {
        ws.send(msg).await.map_err(|e| e.to_string())?;
    }

    tracing::debug!("engine ({}): connected to {page}", site.host);
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    raw.send(Raw::Connected(commands)).map_err(|_| "Mesh went away".to_string())?;
    *connected = true;

    let mut commands_open = true;
    // A browser that's just starting can swap a page out from under this
    // connection without closing it, losing the agent and bindings (or the
    // page goes quiet); check every ten seconds and start over if so.
    let probe = format!("typeof window.__mesh + '/' + typeof window.{}", site.bindings.first().copied().unwrap_or("__mesh"));
    let mut check = tokio::time::interval(Duration::from_secs(10));
    let mut awaiting = false;
    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(incoming) = incoming else { return Err("engine closed".into()) };
                let WsMessage::Text(text) = incoming.map_err(|e| e.to_string())? else { continue };
                if let Some((name, payload)) = binding(&text) {
                    raw.send(Raw::Binding { name, payload }).map_err(|_| "Mesh went away".to_string())?;
                } else if let Some(healthy) = health(&text) {
                    if !healthy {
                        return Err("the agent is gone from the page".into());
                    }
                    awaiting = false;
                }
            }
            _ = check.tick() => {
                if awaiting {
                    return Err("the page stopped answering".into());
                }
                awaiting = true;
                let request = serde_json::json!({ "id": HEALTH_ID, "method": "Runtime.evaluate", "params": { "expression": probe, "returnByValue": true } });
                ws.send(WsMessage::text(request.to_string())).await.map_err(|e| e.to_string())?;
            }
            // Handles may all be dropped (nobody acting); keep streaming state.
            command = command_rx.recv(), if commands_open => {
                let msg = match command {
                    // As a user gesture: pages keep some things (screen capture,
                    // starting call audio) for a real click.
                    Some(Command::Eval(expression)) => {
                        send("Runtime.evaluate", serde_json::json!({ "expression": expression, "userGesture": true }))
                    }
                    // Restores the window even when it's minimized.
                    Some(Command::BringToFront) => send("Page.bringToFront", serde_json::json!({})),
                    None => {
                        commands_open = false;
                        continue;
                    }
                };
                ws.send(msg).await.map_err(|e| e.to_string())?;
            }
        }
    }
}

/// Request id of the health check: far from the counter the rest use, and
/// inside the 32-bit range DevTools accepts.
const HEALTH_ID: u64 = 2_000_000_000;

/// If `text` is the health check's answer: whether the agent and its
/// bindings are there.
fn health(text: &str) -> Option<bool> {
    let msg: serde_json::Value = serde_json::from_str(text).ok()?;
    (msg["id"].as_u64() == Some(HEALTH_ID)).then(|| msg["result"]["result"]["value"].as_str() == Some("object/function"))
}

/// A binding call's (name, payload), if `text` is one.
fn binding(text: &str) -> Option<(String, String)> {
    let msg: serde_json::Value = serde_json::from_str(text).ok()?;
    if msg["method"] != "Runtime.bindingCalled" {
        return None;
    }
    Some((msg["params"]["name"].as_str()?.to_owned(), msg["params"]["payload"].as_str()?.to_owned()))
}

/// A `fetchMedia` answer: the URL, and the bytes (base64 in the payload) or
/// why not.
pub(crate) fn media(payload: &str) -> Option<(String, std::result::Result<Vec<u8>, String>)> {
    use base64::Engine as _;
    let media: serde_json::Value = serde_json::from_str(payload).ok()?;
    let bytes = match media["data"].as_str() {
        Some(data) => base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string()),
        None => Err(media["error"].as_str().unwrap_or("no data").to_owned()),
    };
    Some((media["url"].as_str()?.to_owned(), bytes))
}

/// A sign-in QR code a logged-out site shows, as the page drew it.
#[derive(Debug, Clone, PartialEq)]
pub enum Qr {
    Png(Vec<u8>),
    Svg(Vec<u8>),
}

/// A `{png: base64}` or `{svg: markup}` QR report.
pub(crate) fn qr(payload: &str) -> Option<Qr> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    if let Some(svg) = v["svg"].as_str() {
        return Some(Qr::Svg(svg.as_bytes().to_vec()));
    }
    base64::engine::general_purpose::STANDARD.decode(v["png"].as_str()?).ok().map(Qr::Png)
}

/// One frame of a call's video, decoded, or the end of that video.
#[derive(Debug, Clone)]
pub enum Video {
    Frame {
        /// The track it's from: one per person's camera.
        id: String,
        /// Our own camera (the self view).
        local: bool,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Ended { id: String },
}

/// A `{id, local, jpeg: base64}` or `{id, ended: true}` video report.
pub(crate) fn video(payload: &str) -> Option<Video> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = v["id"].as_str()?.to_owned();
    if v["ended"].as_bool() == Some(true) {
        return Some(Video::Ended { id });
    }
    let jpeg = base64::engine::general_purpose::STANDARD.decode(v["jpeg"].as_str()?).ok()?;
    let frame = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?.into_rgba8();
    Some(Video::Frame {
        id,
        local: v["local"].as_bool().unwrap_or(false),
        width: frame.width(),
        height: frame.height(),
        rgba: frame.into_raw(),
    })
}

/// A `sendFile` result: the chat/channel id and whether it sent (or why not).
/// The agent reports `{chat_id|channel_id, ok, error}`.
pub(crate) fn sent(payload: &str) -> Option<(String, std::result::Result<(), String>)> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = v["chat_id"].as_str().or_else(|| v["channel_id"].as_str())?.to_owned();
    let result = if v["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(v["error"].as_str().unwrap_or("send failed").to_owned())
    };
    Some((id, result))
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

/// Focuses the engine's tab whose URL contains `host`, opening `url` in a new
/// tab when there's none (to scan a QR code, say). Only for a `host:port`
/// engine; otherwise it errors and the caller starts the engine instead.
pub fn show(socket: &Path, host: &str, url: &str) -> Result<()> {
    open(socket, host, url, true)
}

/// Opens `url` in a new engine tab unless one for `host` is already there
/// (which `focus` brings to the front).
pub fn open(socket: &Path, host: &str, url: &str, focus: bool) -> Result<()> {
    let addr = tcp_address(socket).ok_or("the engine isn't host:port")?;
    let targets: Vec<serde_json::Value> = serde_json::from_str(&devtools(addr, "GET", "/json")?).map_err(|e| e.to_string())?;
    let tab = targets
        .iter()
        .find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.contains(host)))
        .and_then(|t| t["id"].as_str());
    match tab {
        Some(id) if focus => devtools(addr, "GET", &format!("/json/activate/{id}")).map(drop),
        Some(_) => Ok(()),
        None => devtools(addr, "PUT", &format!("/json/new?{url}")).map(drop),
    }
}

/// One request to the DevTools HTTP endpoint; the body back.
fn devtools(addr: &str, method: &str, path: &str) -> Result<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    // DevTools may keep the connection open, so stop at Content-Length.
    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
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
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    if !head.starts_with("HTTP/1.1 200") {
        return Err(head.lines().next().unwrap_or("no answer").to_owned());
    }
    Ok(body.to_owned())
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

/// The DevTools websocket path of the tab whose URL contains `host`.
async fn page(socket: &Path, host: &str) -> Result<String> {
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
    page_path(body, host).ok_or_else(|| format!("no {host} tab open"))
}

fn page_path(json: &str, host: &str) -> Option<String> {
    let targets: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let page = targets
        .iter()
        .find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.contains(host)))?;
    let url = page["webSocketDebuggerUrl"].as_str()?;
    // ws://localhost/devtools/page/<id> → /devtools/page/<id>
    Some(url.split_once("://")?.1.split_once('/').map(|(_, p)| format!("/{p}"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sites_tab() {
        let json = r#"[{"type":"service_worker","url":"https://discord.com/sw.js","webSocketDebuggerUrl":"ws://localhost/devtools/x"},
                       {"type":"page","url":"https://discord.com/channels/@me","webSocketDebuggerUrl":"ws://localhost/devtools/page/ABC"},
                       {"type":"page","url":"https://web.whatsapp.com/","webSocketDebuggerUrl":"ws://localhost/devtools/page/WA"}]"#;
        assert_eq!(page_path(json, "discord.com").as_deref(), Some("/devtools/page/ABC"));
        assert_eq!(page_path(json, "web.whatsapp.com").as_deref(), Some("/devtools/page/WA"));
        assert_eq!(page_path("[]", "discord.com"), None);
    }

    #[test]
    fn tells_tcp_from_socket_paths() {
        assert_eq!(tcp_address(Path::new("127.0.0.1:9222")), Some("127.0.0.1:9222"));
        assert_eq!(tcp_address(Path::new("localhost:9222")), Some("localhost:9222"));
        assert_eq!(tcp_address(Path::new("/run/user/1000/discord-engine.sock")), None);
        assert_eq!(tcp_address(Path::new(r"C:\Users\me\engine.sock")), None);
    }

    #[test]
    fn reads_video_reports() {
        use base64::Engine as _;
        let mut jpeg = Vec::new();
        image::RgbImage::new(4, 2)
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .unwrap();
        let payload = serde_json::json!({ "id": "t1", "local": true, "jpeg": base64::engine::general_purpose::STANDARD.encode(&jpeg) });
        let Some(Video::Frame { id, local, width, height, rgba }) = video(&payload.to_string()) else { panic!("no frame") };
        assert_eq!((id.as_str(), local, width, height, rgba.len()), ("t1", true, 4, 2, 4 * 2 * 4));
        assert!(matches!(video(r#"{"id":"t1","ended":true}"#), Some(Video::Ended { id }) if id == "t1"));
        assert!(video(r#"{"id":"t1","jpeg":"bm90IGEganBlZw=="}"#).is_none());
    }

    #[test]
    fn notices_a_lost_agent() {
        let answer = |value: &str| format!(r#"{{"id":{HEALTH_ID},"result":{{"result":{{"type":"string","value":"{value}"}}}}}}"#);
        assert_eq!(health(&answer("object/function")), Some(true));
        assert_eq!(health(&answer("undefined/undefined")), Some(false));
        assert_eq!(health(&answer("object/undefined")), Some(false));
        assert_eq!(health(r#"{"id":3,"result":{"result":{"value":"undefined/undefined"}}}"#), None);
    }

    #[test]
    fn reads_qr_reports() {
        assert_eq!(qr(r#"{"svg":"<svg/>"}"#), Some(Qr::Svg(b"<svg/>".to_vec())));
        assert_eq!(qr(r#"{"png":"iVBO"}"#), Some(Qr::Png(vec![0x89, 0x50, 0x4e])));
        assert_eq!(qr("{}"), None);
    }

    #[test]
    fn reads_binding_calls() {
        let msg = serde_json::json!({ "method": "Runtime.bindingCalled", "params": { "name": "meshEmit", "payload": "{}" } });
        assert_eq!(binding(&msg.to_string()), Some(("meshEmit".into(), "{}".into())));
        assert_eq!(binding(r#"{"id":3,"result":{}}"#), None);
    }
}
