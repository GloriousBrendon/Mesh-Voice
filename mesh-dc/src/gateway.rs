//! The Discord user-account gateway: identifies as the web client, keeps the
//! guild/channel/DM/voice state current, and reports it as a Mesh snapshot.
//! It also carries the voice handshake (op 4 out, VOICE_SERVER/STATE in) that
//! `voice` feeds to songbird. Hand-rolled, since the maintained gateway crates
//! are bot-only; the REST calls use the same headers the web client sends.

use std::collections::HashMap;

use flate2::Decompress;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;

/// The web client build Mesh presents as. Bump occasionally; a very stale
/// build number is the main thing that stands out.
pub const CLIENT_BUILD: u64 = 371_591;
const API: &str = "https://discord.com/api/v9";

/// Properties Discord ties to the session; must match what REST sends in the
/// `X-Super-Properties` header (see [`super_properties`]).
fn properties() -> Value {
    json!({
        "os": "Windows",
        "browser": "Chrome",
        "device": "",
        "system_locale": "en-US",
        "browser_user_agent": USER_AGENT,
        "browser_version": "131.0.0.0",
        "os_version": "10",
        "referrer": "",
        "referring_domain": "",
        "referrer_current": "",
        "referring_domain_current": "",
        "release_channel": "stable",
        "client_build_number": CLIENT_BUILD,
        "client_event_source": Value::Null,
    })
}

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Base64 of the super-properties, for the REST `X-Super-Properties` header.
pub fn super_properties() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(properties().to_string())
}

/// What the gateway tells the rest of the helper.
pub enum Event {
    /// The state changed; rebuild and emit the snapshot.
    Snapshot,
    /// A message worth a desktop notification (DM, or a mention of us).
    Notify { channel_id: String, title: String, body: String },
    /// The voice server for a join we asked for: hand to songbird.
    VoiceServer { guild_id: Option<String>, channel_id: String, endpoint: String, token: String, session_id: String },
    /// We left / were removed from voice.
    VoiceLeft,
    /// The token was rejected; the caller signs out.
    InvalidSession,
    /// The channel Mesh has open changed; re-fetch its messages.
    ChannelDirty(String),
}

/// Commands into the gateway task.
pub enum Cmd {
    /// Join a guild voice channel or a DM/group call (`guild_id` None for DMs).
    JoinVoice { guild_id: Option<String>, channel_id: String },
    LeaveVoice,
    /// Update our voice flags in place (mute/deaf/video) via op 4.
    VoiceFlags { self_mute: bool, self_deaf: bool, self_video: bool },
}

/// Everything the snapshot is built from, kept current by dispatch events.
#[derive(Default)]
pub struct State {
    pub me: Option<Value>,
    pub users: HashMap<String, Value>,
    pub guilds: HashMap<String, Value>,
    /// channel id -> channel object (guild channels and DMs).
    pub channels: HashMap<String, Value>,
    /// guild id -> (channel id -> voice state).
    pub voice: HashMap<String, HashMap<String, Value>>,
    /// Private (DM/group) voice states live under this key.
    pub dm_channels: Vec<Value>,
    /// DM/group calls going on: channel id -> who it's ringing (user ids).
    pub calls: HashMap<String, Vec<String>>,
    /// channel id -> (last_message_id, mention_count).
    pub read_state: HashMap<String, (String, u64)>,
    /// ssrc/user speaking right now (user id -> speaking), from the voice side.
    pub speaking: HashMap<String, bool>,
    /// Our live voice channel and its guild, once connected.
    pub voice_channel: Option<String>,
    pub voice_guild: Option<String>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub self_video: bool,
    pub rtc_connected: bool,
    /// The join we're waiting on a VOICE_SERVER_UPDATE for.
    pending_join: Option<(Option<String>, String)>,
    session_id: Option<String>,
    /// The channel Mesh currently shows (for message refreshes).
    pub open_channel: Option<String>,
}

impl State {
    pub fn me_id(&self) -> Option<String> {
        self.me.as_ref()?.get("id")?.as_str().map(str::to_owned)
    }
}

/// Runs the gateway until the token is rejected or `events`/`cmds` close,
/// reconnecting on a dropped socket (resume is skipped: a fresh identify is
/// simplest and the snapshot is rebuilt from READY anyway).
/// ponytail: no RESUME, full re-identify on every drop; add resume if the
/// reconnect rate ever trips Discord's identify limits.
pub async fn run(token: String, state: &tokio::sync::Mutex<State>, events: mpsc::UnboundedSender<Event>, cmds: &mut mpsc::UnboundedReceiver<Cmd>) {
    // Seconds to wait before the next try: doubles on quick failures, so a
    // bug can't turn into an identify storm on the account.
    let mut wait = 5;
    loop {
        let started = std::time::Instant::now();
        match session(&token, state, &events, cmds).await {
            Err(e) if e == "invalid session" => {
                let _ = events.send(Event::InvalidSession);
                return;
            }
            Err(e) => log::warn!("gateway: {e}"),
            Ok(()) => return,
        }
        if events.is_closed() {
            return;
        }
        wait = if started.elapsed().as_secs() > 300 { 5 } else { (wait * 2).min(300) };
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }
}

type R<T> = Result<T, String>;

async fn session(token: &str, state: &tokio::sync::Mutex<State>, events: &mpsc::UnboundedSender<Event>, cmds: &mut mpsc::UnboundedReceiver<Cmd>) -> R<()> {
    let url = "wss://gateway.discord.gg/?v=9&encoding=json&compress=zlib-stream";
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.map_err(|e| e.to_string())?;
    let mut inflate = Decompress::new(true);
    let mut buf: Vec<u8> = Vec::new();

    let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(41_250));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut seq: Option<u64> = None;

    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(incoming) = incoming else { return Err("gateway closed".into()) };
                let msg = incoming.map_err(|e| e.to_string())?;
                let Some(payload) = inflate_payload(&msg, &mut inflate, &mut buf)? else { continue };
                let v: Value = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
                if let Some(s) = v.get("s").and_then(Value::as_u64) {
                    seq = Some(s);
                }
                match v.get("op").and_then(Value::as_u64) {
                    Some(10) => {
                        let interval = v["d"]["heartbeat_interval"].as_f64().unwrap_or(41_250.0);
                        heartbeat = tokio::time::interval(std::time::Duration::from_millis(interval as u64));
                        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        heartbeat.tick().await; // consume the immediate first tick
                        ws.send(Ws::text(identify(token))).await.map_err(|e| e.to_string())?;
                    }
                    Some(1) => {
                        ws.send(Ws::text(json!({ "op": 1, "d": seq }).to_string())).await.map_err(|e| e.to_string())?;
                    }
                    Some(11) => {}
                    // Not a bad token (that's close 4004): identify again after the backoff.
                    Some(9) => return Err("session invalidated".into()),
                    Some(0) => {
                        let t = v["t"].as_str().unwrap_or("");
                        if dispatch(t, &v["d"], state, events).await {
                            let _ = events.send(Event::Snapshot);
                        }
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                ws.send(Ws::text(json!({ "op": 1, "d": seq }).to_string())).await.map_err(|e| e.to_string())?;
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                let frame = command_frame(cmd, state).await;
                ws.send(Ws::text(frame)).await.map_err(|e| e.to_string())?;
            }
        }
    }
}

/// op 4 (voice state update) for a join, leave, or flag change.
async fn command_frame(cmd: Cmd, state: &tokio::sync::Mutex<State>) -> String {
    let mut s = state.lock().await;
    match cmd {
        Cmd::JoinVoice { guild_id, channel_id } => {
            s.pending_join = Some((guild_id.clone(), channel_id.clone()));
            s.self_video = false;
            json!({ "op": 4, "d": { "guild_id": guild_id, "channel_id": channel_id, "self_mute": s.self_mute, "self_deaf": s.self_deaf, "self_video": false } }).to_string()
        }
        Cmd::LeaveVoice => {
            let guild = s.voice_guild.clone();
            s.voice_channel = None;
            s.rtc_connected = false;
            json!({ "op": 4, "d": { "guild_id": guild, "channel_id": Value::Null, "self_mute": s.self_mute, "self_deaf": s.self_deaf, "self_video": false } }).to_string()
        }
        Cmd::VoiceFlags { self_mute, self_deaf, self_video } => {
            s.self_mute = self_mute;
            s.self_deaf = self_deaf;
            s.self_video = self_video;
            let (guild, channel) = (s.voice_guild.clone(), s.voice_channel.clone());
            json!({ "op": 4, "d": { "guild_id": guild, "channel_id": channel, "self_mute": self_mute, "self_deaf": self_deaf, "self_video": self_video } }).to_string()
        }
    }
}

fn identify(token: &str) -> String {
    json!({
        "op": 2,
        "d": {
            "token": token,
            // The web client's capability bitfield; stale is harmless, wrong-shaped is not.
            "capabilities": 30717,
            "properties": properties(),
            "presence": { "status": "unknown", "since": 0, "activities": [], "afk": false },
            "compress": false,
            "client_state": { "guild_versions": {} },
        }
    })
    .to_string()
}

/// Decompresses a zlib-stream frame; `None` until a full message (ending in the
/// `00 00 ff ff` sync flush) has arrived.
fn inflate_payload(msg: &Ws, inflate: &mut Decompress, buf: &mut Vec<u8>) -> R<Option<Vec<u8>>> {
    let data = match msg {
        Ws::Binary(b) => b.as_ref(),
        Ws::Text(t) => return Ok(Some(t.as_bytes().to_vec())),
        // 4004: the token is no good (signed out elsewhere, or reset).
        Ws::Close(Some(f)) if u16::from(f.code) == 4004 => return Err("invalid session".into()),
        Ws::Close(f) => return Err(format!("gateway sent close {:?}", f.as_ref().map(|f| u16::from(f.code)))),
        _ => return Ok(None),
    };
    buf.extend_from_slice(data);
    if buf.len() < 4 || buf[buf.len() - 4..] != [0, 0, 0xff, 0xff] {
        return Ok(None);
    }
    // `decompress_vec` only fills spare capacity, so keep going until a pass
    // has eaten all the input and still left room: then nothing's held back.
    let mut out = Vec::with_capacity(buf.len() * 4);
    let start = inflate.total_in();
    loop {
        let consumed = (inflate.total_in() - start) as usize;
        if out.capacity() - out.len() < 4096 {
            out.reserve(out.capacity().max(4096));
        }
        inflate.decompress_vec(&buf[consumed..], &mut out, flate2::FlushDecompress::Sync).map_err(|e| e.to_string())?;
        if (inflate.total_in() - start) as usize == buf.len() && out.len() < out.capacity() {
            break;
        }
    }
    buf.clear();
    Ok(Some(out))
}

/// Applies a dispatch event to the state; returns whether the snapshot changed.
async fn dispatch(t: &str, d: &Value, state: &tokio::sync::Mutex<State>, events: &mpsc::UnboundedSender<Event>) -> bool {
    let mut s = state.lock().await;
    match t {
        "READY" => {
            s.me = d.get("user").cloned();
            s.session_id = d.get("session_id").and_then(Value::as_str).map(str::to_owned);
            if let Some(users) = d.get("users").and_then(Value::as_array) {
                for u in users {
                    if let Some(id) = u.get("id").and_then(Value::as_str) {
                        s.users.insert(id.to_owned(), u.clone());
                    }
                }
            }
            for g in d.get("guilds").and_then(Value::as_array).into_iter().flatten() {
                index_guild(&mut s, g);
            }
            // A re-identify (reconnect without resume) sends READY again with the full list.
            s.dm_channels.clear();
            s.calls.clear();
            for ch in d.get("private_channels").and_then(Value::as_array).into_iter().flatten() {
                if let Some(id) = ch.get("id").and_then(Value::as_str) {
                    s.channels.insert(id.to_owned(), ch.clone());
                }
                s.dm_channels.push(ch.clone());
            }
            if let Some(rs) = d.get("read_state") {
                let entries = rs.get("entries").and_then(Value::as_array).or_else(|| rs.as_array());
                for e in entries.into_iter().flatten() {
                    if let Some(id) = e.get("id").and_then(Value::as_str) {
                        let last = e.get("last_message_id").and_then(Value::as_str).unwrap_or("0").to_owned();
                        let mentions = e.get("mention_count").and_then(Value::as_u64).unwrap_or(0);
                        s.read_state.insert(id.to_owned(), (last, mentions));
                    }
                }
            }
            true
        }
        "READY_SUPPLEMENTAL" => {
            // Per-guild voice states that READY leaves out.
            for g in d.get("guilds").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(gid), Some(vs)) = (g.get("id").and_then(Value::as_str), g.get("voice_states").and_then(Value::as_array)) {
                    let map = s.voice.entry(gid.to_owned()).or_default();
                    for v in vs {
                        if let Some(uid) = v.get("user_id").and_then(Value::as_str) {
                            map.insert(uid.to_owned(), v.clone());
                        }
                    }
                }
            }
            true
        }
        "GUILD_CREATE" => {
            index_guild(&mut s, d);
            true
        }
        "GUILD_DELETE" => {
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                s.guilds.remove(id);
                s.voice.remove(id);
            }
            true
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                s.channels.insert(id.to_owned(), d.clone());
            }
            true
        }
        "CHANNEL_DELETE" => {
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                s.channels.remove(id);
            }
            true
        }
        // DM and group calls (a user account's gateway only): who's being rung.
        "CALL_CREATE" | "CALL_UPDATE" => {
            if let Some(id) = d.get("channel_id").and_then(Value::as_str) {
                let ringing = d.get("ringing").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                s.calls.insert(id.to_owned(), ringing);
            }
            true
        }
        "CALL_DELETE" => {
            if let Some(id) = d.get("channel_id").and_then(Value::as_str) {
                s.calls.remove(id);
            }
            true
        }
        "MESSAGE_CREATE" => {
            let changed = bump_dm_order(&mut s, d);
            notify(&s, d, events);
            dirty_if_open(&s, d, events);
            changed
        }
        "MESSAGE_UPDATE" | "MESSAGE_DELETE" => {
            dirty_if_open(&s, d, events);
            false
        }
        "MESSAGE_ACK" => {
            if let (Some(id), Some(last)) = (d.get("channel_id").and_then(Value::as_str), d.get("message_id").and_then(Value::as_str)) {
                let entry = s.read_state.entry(id.to_owned()).or_default();
                entry.0 = last.to_owned();
                entry.1 = d.get("mention_count").and_then(Value::as_u64).unwrap_or(0);
            }
            true
        }
        "VOICE_STATE_UPDATE" => {
            let me = s.me_id();
            let is_me = d.get("user_id").and_then(Value::as_str) == me.as_deref();
            let left = is_me && d.get("channel_id").and_then(Value::as_str).is_none();
            apply_voice_state(&mut s, d);
            if left {
                let _ = events.send(Event::VoiceLeft);
            }
            true
        }
        "VOICE_SERVER_UPDATE" => {
            let endpoint = d.get("endpoint").and_then(Value::as_str).unwrap_or_default().to_owned();
            let token = d.get("token").and_then(Value::as_str).unwrap_or_default().to_owned();
            let guild_id = d.get("guild_id").and_then(Value::as_str).map(str::to_owned);
            let (session_id, pending) = (s.session_id.clone(), s.pending_join.clone());
            if let (Some(session_id), Some((_, channel_id)), false) = (session_id, pending, endpoint.is_empty()) {
                s.voice_channel = Some(channel_id.clone());
                s.voice_guild = guild_id.clone();
                let _ = events.send(Event::VoiceServer { guild_id, channel_id, endpoint, token, session_id });
            }
            s.pending_join = None;
            true
        }
        _ => false,
    }
}

/// Puts a guild and its channels into the index (channels carry `guild_id`).
fn index_guild(s: &mut State, g: &Value) {
    let Some(gid) = g.get("id").and_then(Value::as_str) else { return };
    let mut slim = g.clone();
    if let Some(o) = slim.as_object_mut() {
        o.remove("channels");
        // A user account's READY keeps name, icon and the rest under `properties`.
        if let Some(props) = o.remove("properties").and_then(|p| p.as_object().cloned()) {
            for (k, v) in props {
                o.entry(k).or_insert(v);
            }
        }
    }
    s.guilds.insert(gid.to_owned(), slim);
    for ch in g.get("channels").and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = ch.get("id").and_then(Value::as_str) {
            let mut ch = ch.clone();
            ch.as_object_mut().map(|o| o.insert("guild_id".into(), json!(gid)));
            s.channels.insert(id.to_owned(), ch);
        }
    }
    let map = s.voice.entry(gid.to_owned()).or_default();
    for v in g.get("voice_states").and_then(Value::as_array).into_iter().flatten() {
        if let Some(uid) = v.get("user_id").and_then(Value::as_str) {
            map.insert(uid.to_owned(), v.clone());
        }
    }
    for m in g.get("members").and_then(Value::as_array).into_iter().flatten() {
        if let Some(u) = m.get("user") {
            if let Some(id) = u.get("id").and_then(Value::as_str) {
                s.users.insert(id.to_owned(), u.clone());
            }
        }
    }
}

fn apply_voice_state(s: &mut State, d: &Value) {
    let Some(uid) = d.get("user_id").and_then(Value::as_str).map(str::to_owned) else { return };
    let channel = d.get("channel_id").and_then(Value::as_str).map(str::to_owned);
    let guild = d.get("guild_id").and_then(Value::as_str).map(str::to_owned);
    if let Some(u) = d.get("member").and_then(|m| m.get("user")) {
        s.users.insert(uid.clone(), u.clone());
    }
    let key = guild.clone().unwrap_or_else(|| "@me".into());
    let map = s.voice.entry(key).or_default();
    match &channel {
        Some(_) => {
            map.insert(uid.clone(), d.clone());
        }
        None => {
            map.remove(&uid);
        }
    }
    // Our own state: track where we are and our flags.
    if Some(&uid) == s.me_id().as_ref() {
        s.voice_channel = channel.clone();
        s.voice_guild = guild;
        s.self_mute = d.get("self_mute").and_then(Value::as_bool).unwrap_or(s.self_mute);
        s.self_deaf = d.get("self_deaf").and_then(Value::as_bool).unwrap_or(s.self_deaf);
        s.self_video = d.get("self_video").and_then(Value::as_bool).unwrap_or(false);
        if channel.is_none() {
            s.rtc_connected = false;
        }
    }
}

/// Moves a DM to the front and marks it unread on a new message; returns
/// whether anything the snapshot shows changed.
fn bump_dm_order(s: &mut State, d: &Value) -> bool {
    let Some(cid) = d.get("channel_id").and_then(Value::as_str) else { return false };
    let me = s.me_id();
    let from_me = d.get("author").and_then(|a| a.get("id")).and_then(Value::as_str) == me.as_deref();
    if let Some(mid) = d.get("id").and_then(Value::as_str) {
        let entry = s.read_state.entry(cid.to_owned()).or_default();
        if from_me {
            entry.0 = mid.to_owned(); // our own message counts as read
        }
    }
    if let Some(ch) = s.channels.get_mut(cid) {
        if let (Some(obj), Some(mid)) = (ch.as_object_mut(), d.get("id").and_then(Value::as_str)) {
            obj.insert("last_message_id".into(), json!(mid));
        }
    }
    true
}

/// If the message touches the open channel, tell main to re-fetch it.
fn dirty_if_open(s: &State, d: &Value, events: &mpsc::UnboundedSender<Event>) {
    if let (Some(open), Some(cid)) = (&s.open_channel, d.get("channel_id").and_then(Value::as_str)) {
        if open == cid {
            let _ = events.send(Event::ChannelDirty(cid.to_owned()));
        }
    }
}

/// Emits a notify for a new DM, or a mention of us in a guild channel (never
/// our own messages), as `agent.js` did.
fn notify(s: &State, d: &Value, events: &mpsc::UnboundedSender<Event>) {
    let Some(me) = s.me_id() else { return };
    let author = d.get("author");
    let author_id = author.and_then(|a| a.get("id")).and_then(Value::as_str).unwrap_or_default();
    if author_id == me || author_id.is_empty() {
        return;
    }
    let Some(cid) = d.get("channel_id").and_then(Value::as_str) else { return };
    let ch = s.channels.get(cid);
    let guild = ch.and_then(|c| c.get("guild_id")).and_then(Value::as_str);
    if guild.is_some() {
        let mentioned = d.get("mentions").and_then(Value::as_array).is_some_and(|ms| ms.iter().any(|m| m.get("id").and_then(Value::as_str) == Some(me.as_str())));
        if !mentioned {
            return;
        }
    }
    let name = author.and_then(|a| a.get("global_name")).and_then(Value::as_str).or_else(|| author.and_then(|a| a.get("username")).and_then(Value::as_str)).unwrap_or("Someone");
    let where_ = match ch.and_then(|c| c.get("name")).and_then(Value::as_str) {
        Some(n) if guild.is_some() => format!(" in #{n}"),
        Some(n) => format!(" in {n}"),
        None => String::new(),
    };
    let body = d.get("content").and_then(Value::as_str).filter(|c| !c.is_empty()).unwrap_or("sent an attachment").to_owned();
    let _ = events.send(Event::Notify { channel_id: cid.to_owned(), title: format!("{name}{where_}"), body });
}

/// The REST base and the headers the web client sends with every call.
pub fn api() -> &'static str {
    API
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_stream_waits_for_the_sync_flush() {
        use flate2::{Compress, Compression, FlushCompress};
        let mut comp = Compress::new(Compression::default(), true);
        let msg = br#"{"op":11}"#;
        // compress_vec only writes into spare capacity, so reserve generously.
        let mut packet = Vec::with_capacity(256);
        comp.compress_vec(msg, &mut packet, FlushCompress::Sync).unwrap();
        assert_eq!(&packet[packet.len() - 4..], &[0, 0, 0xff, 0xff]);
        // Split the packet: the first half must not yield, the whole one must.
        let mut inflate = Decompress::new(true);
        let mut buf = Vec::new();
        // A fragment without the 00 00 ff ff sync flush must not yield yet.
        assert_eq!(inflate_payload(&Ws::binary(packet[..1].to_vec()), &mut inflate, &mut buf).unwrap(), None);
        // The rest completes the frame and decodes to the original message.
        let out = inflate_payload(&Ws::binary(packet[1..].to_vec()), &mut inflate, &mut buf).unwrap();
        assert_eq!(out.as_deref(), Some(&msg[..]));
    }

    #[test]
    fn zlib_stream_inflates_big_messages_whole() {
        // Compresses far more than 4x: the case that used to come out cut short.
        use flate2::{Compress, Compression, FlushCompress};
        let mut comp = Compress::new(Compression::default(), true);
        let mut inflate = Decompress::new(true);
        let mut buf = Vec::new();
        for n in [200_000usize, 50] {
            let msg = format!(r#"{{"op":0,"d":"{}"}}"#, "a".repeat(n));
            let mut packet = Vec::with_capacity(msg.len() + 1024);
            comp.compress_vec(msg.as_bytes(), &mut packet, FlushCompress::Sync).unwrap();
            let out = inflate_payload(&Ws::binary(packet), &mut inflate, &mut buf).unwrap().unwrap();
            assert_eq!(out, msg.as_bytes());
        }
    }

    #[tokio::test]
    async fn repeated_ready_does_not_duplicate_dms() {
        let state = tokio::sync::Mutex::new(State::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let ready = json!({ "private_channels": [{ "id": "500", "type": 1, "recipient_ids": ["2"] }] });
        for _ in 0..3 {
            dispatch("READY", &ready, &state, &tx).await;
        }
        assert_eq!(state.lock().await.dm_channels.len(), 1);
    }

    #[test]
    fn super_properties_is_base64_json() {
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD.decode(super_properties()).unwrap();
        let v: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["client_build_number"], CLIENT_BUILD);
        assert_eq!(v["os"], "Windows");
    }
}
