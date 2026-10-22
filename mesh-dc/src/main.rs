//! mesh-dc: Discord for Mesh, natively (no browser engine).
//!
//! `mesh-dc --data <dir>`. Commands arrive on stdin as `{"fn":..,"args":[..]}`
//! lines; reports go out on stdout as `{"binding":..,"payload":"<json string>"}`
//! lines, the same bindings `agent.js` emits. Logs go to stderr.
//!
//! Login is remote-auth (QR) or a stored token; the gateway keeps the snapshot
//! current; songbird does voice (RTP/Opus, GCM, DAVE). Video/screen-share are
//! not implemented here (see the report); those commands are accepted and
//! no-op so Mesh never stalls on them.

mod audio;
mod gateway;
mod login;
mod rest;
mod voice;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use gateway::State;
use rest::Rest;
use voice::Voice;

#[derive(Debug, Deserialize)]
struct Cmd {
    #[serde(rename = "fn")]
    func: String,
    #[serde(default)]
    args: Vec<String>,
}

fn parse_cmd(line: &str) -> Option<Cmd> {
    serde_json::from_str(line.trim()).ok()
}

/// One stdout line: the payload travels as a JSON string inside the frame.
fn emit(binding: &str, payload: &Value) {
    let line = json!({ "binding": binding, "payload": serde_json::to_string(payload).unwrap_or_else(|_| "null".into()) }).to_string();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn data_dir() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).target(env_logger::Target::Stderr).init();
    #[cfg(windows)]
    {
        // 1 ms timer resolution, so the audio drain's sleeps stay fine-grained.
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn timeBeginPeriod(period: u32) -> u32;
        }
        unsafe { timeBeginPeriod(1) };
    }

    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;

    // stdin on a plain thread: a blocked async read would hold up shutdown.
    let (cmd_tx, mut cmds) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if cmd_tx.send(line).is_err() {
                break;
            }
        }
    });

    loop {
        let token = match login::load_token(&dir) {
            Some(t) => t,
            None => {
                if !show_qr_until_linked(&dir, &mut cmds).await {
                    return Ok(()); // stdin closed: Mesh is gone
                }
                // Couldn't store it (logged): link again rather than crash.
                let Some(t) = login::load_token(&dir) else { continue };
                t
            }
        };
        match run_session(&dir, token, &mut cmds).await {
            SessionEnd::StdinClosed => return Ok(()),
            SessionEnd::SignedOut => {
                login::forget_token(&dir);
                // Loop back to the QR flow.
            }
        }
    }
}

enum SessionEnd {
    StdinClosed,
    SignedOut,
}

/// Runs the QR flow, emitting `meshQr`, until a token is stored. Returns false
/// if stdin closes first (Mesh quit).
async fn show_qr_until_linked(dir: &Path, cmds: &mut mpsc::UnboundedReceiver<String>) -> bool {
    loop {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let auth = tokio::spawn(login::remote_auth(tx));
        loop {
            tokio::select! {
                update = rx.recv() => match update {
                    Some(login::Update::Qr(svg)) => emit("meshQr", &json!({ "svg": svg })),
                    Some(login::Update::Token(token)) => {
                        login::save_token(dir, &token);
                        auth.abort();
                        return true;
                    }
                    None => break, // auth task ended (error / socket drop)
                },
                line = cmds.recv() => match line {
                    None => { auth.abort(); return false; }
                    // Only signOut matters while logged out; nothing else applies.
                    Some(_) => {}
                },
            }
        }
        let _ = auth.await;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Connected session: gateway + commands + voice, until signed out or stdin
/// closes.
async fn run_session(dir: &Path, token: String, cmds: &mut mpsc::UnboundedReceiver<String>) -> SessionEnd {
    let _ = dir;
    let state = Arc::new(Mutex::new(State::default()));
    let rest = Rest::new(token.clone());
    let (events_tx, mut events) = mpsc::unbounded_channel::<gateway::Event>();
    let (gw_cmd_tx, gw_cmd_rx) = mpsc::unbounded_channel::<gateway::Cmd>();
    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<()>();

    let gw_state = state.clone();
    let gw = tokio::spawn(async move {
        let mut rx = gw_cmd_rx;
        gateway::run(token, &gw_state, events_tx, &mut rx).await;
    });

    let mut voice: Option<Voice> = None;
    let mut ready_sent = false;
    let mut snapshot_due = false;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(150));

    let result = loop {
        tokio::select! {
            ev = events.recv() => match ev {
                None => break SessionEnd::SignedOut, // gateway stopped
                Some(ev) => match ev {
                    gateway::Event::Snapshot => {
                        if !ready_sent && state.lock().await.me.is_some() {
                            ready_sent = true;
                            emit("dcReady", &json!({}));
                        }
                        snapshot_due = true;
                    }
                    gateway::Event::Notify { channel_id, title, body } => {
                        emit("meshNotify", &json!({ "channel_id": channel_id, "title": title, "body": body }));
                    }
                    gateway::Event::ChannelDirty(cid) => {
                        if let Ok(payload) = rest.messages(&cid, &state).await {
                            emit("meshMessages", &payload);
                        }
                    }
                    gateway::Event::VoiceServer { guild_id, channel_id, endpoint, token, session_id } => {
                        if let Some(v) = voice.take() {
                            v.leave().await;
                        }
                        match Voice::connect(state.clone(), guild_id, channel_id, endpoint, token, session_id, notify_tx.clone()).await {
                            Ok(v) => voice = Some(v),
                            Err(e) => log::warn!("voice: {e}"),
                        }
                        snapshot_due = true;
                    }
                    gateway::Event::VoiceLeft => {
                        if let Some(v) = voice.take() {
                            v.leave().await;
                        }
                        snapshot_due = true;
                    }
                    gateway::Event::InvalidSession => break SessionEnd::SignedOut,
                },
            },
            _ = notify_rx.recv() => { snapshot_due = true; }
            line = cmds.recv() => match line {
                None => break SessionEnd::StdinClosed,
                Some(line) => {
                    let Some(cmd) = parse_cmd(&line) else { continue };
                    if handle(&cmd, &state, &rest, &gw_cmd_tx, &mut voice).await {
                        rest.logout().await;
                        break SessionEnd::SignedOut;
                    }
                    snapshot_due = true;
                }
            },
            _ = tick.tick() => {
                if snapshot_due {
                    snapshot_due = false;
                    let payload = rest::snapshot(&*state.lock().await);
                    emit("meshEmit", &payload);
                }
            }
        }
    };

    if let Some(v) = voice.take() {
        v.leave().await;
    }
    gw.abort();
    result
}

/// Runs one command. Returns true for signOut (the caller tears down).
async fn handle(cmd: &Cmd, state: &Arc<Mutex<State>>, rest: &Rest, gw: &mpsc::UnboundedSender<gateway::Cmd>, voice: &mut Option<Voice>) -> bool {
    let arg = |i: usize| cmd.args.get(i).map(String::as_str);
    match cmd.func.as_str() {
        "join" => {
            if let Some(cid) = arg(0) {
                let guild = state.lock().await.channels.get(cid).and_then(|c| c.get("guild_id")).and_then(Value::as_str).map(str::to_owned);
                let _ = gw.send(gateway::Cmd::JoinVoice { guild_id: guild, channel_id: cid.to_owned() });
            }
        }
        "leave" => {
            let _ = gw.send(gateway::Cmd::LeaveVoice);
            if let Some(v) = voice.take() {
                v.leave().await;
            }
        }
        "toggleMute" => {
            let (m, d, vid) = {
                let mut s = state.lock().await;
                s.self_mute = !s.self_mute;
                (s.self_mute, s.self_deaf, s.self_video)
            };
            if let Some(v) = voice.as_mut() {
                v.set_mute(m);
            }
            let _ = gw.send(gateway::Cmd::VoiceFlags { self_mute: m, self_deaf: d, self_video: vid });
        }
        "toggleDeaf" => {
            let (m, d, vid) = {
                let mut s = state.lock().await;
                s.self_deaf = !s.self_deaf;
                if s.self_deaf {
                    s.self_mute = true; // deafen implies mute, as the client does
                }
                (s.self_mute, s.self_deaf, s.self_video)
            };
            if let Some(v) = voice.as_mut() {
                v.set_mute(m);
            }
            let _ = gw.send(gateway::Cmd::VoiceFlags { self_mute: m, self_deaf: d, self_video: vid });
        }
        // Video and screen share aren't implemented natively; accept and ignore
        // so Mesh's buttons don't hang. See the handoff report.
        "setVideo" | "watchStream" | "stopWatching" | "shareScreen" => {
            log::info!("{} is not supported by mesh-dc (no native video yet)", cmd.func);
        }
        "open" => match arg(0) {
            Some(cid) => {
                state.lock().await.open_channel = Some(cid.to_owned());
                match rest.messages(cid, state).await {
                    Ok(payload) => emit("meshMessages", &payload),
                    Err(e) => log::warn!("open {cid}: {e}"),
                }
            }
            None => state.lock().await.open_channel = None,
        },
        "send" => {
            if let (Some(cid), Some(content)) = (arg(0), arg(1)) {
                if let Err(e) = rest.send(cid, content).await {
                    log::warn!("send: {e}");
                }
            }
        }
        "fetchMedia" => {
            if let Some(url) = arg(0) {
                let payload = match rest.fetch_media(url).await {
                    Ok(bytes) => {
                        use base64::Engine as _;
                        json!({ "url": url, "data": base64::engine::general_purpose::STANDARD.encode(bytes) })
                    }
                    Err(e) => json!({ "url": url, "error": e }),
                };
                emit("meshMedia", &payload);
            }
        }
        "acceptInvite" => {
            if let Some(link) = arg(0) {
                emit("meshInvite", &json!({ "state": "waiting" }));
                let payload = match rest.accept_invite(link).await {
                    Ok(guild) => json!({ "state": "joined", "guild": guild }),
                    Err(e) => json!({ "state": "failed", "error": e }),
                };
                emit("meshInvite", &payload);
            }
        }
        "signOut" => return true,
        other => log::warn!("unknown command: {other}"),
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_line_protocol() {
        let c = parse_cmd(r#"{"fn":"send","args":["5","hi"]}"#).unwrap();
        assert_eq!(c.func, "send");
        assert_eq!(c.args, vec!["5", "hi"]);
        assert_eq!(parse_cmd(r#"{"fn":"leave"}"#).unwrap().args.len(), 0);
        assert!(parse_cmd("not json").is_none());
    }

    #[test]
    fn frames_payload_as_a_json_string() {
        // The frame carries the payload as a *string*, matching mesh-wa.
        let payload = json!({ "a": 1 });
        let line = json!({ "binding": "meshEmit", "payload": serde_json::to_string(&payload).unwrap() }).to_string();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["binding"], "meshEmit");
        assert!(v["payload"].is_string());
        let inner: Value = serde_json::from_str(v["payload"].as_str().unwrap()).unwrap();
        assert_eq!(inner["a"], 1);
    }
}
