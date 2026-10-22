//! mesh-wa: WhatsApp for Mesh, natively through whatsapp-rust.
//!
//! `mesh-wa --data <dir>`. Commands arrive on stdin as `{"fn":..,"args":[..]}` lines; reports go
//! out on stdout as `{"binding":..,"payload":"<json string>"}` lines. Logs go to stderr.

mod audio;
mod calls;
mod store;

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::{self, UnboundedSender};
use whatsapp_rust::prelude::*;
use whatsapp_rust::wacore::net::HttpRequest;
use whatsapp_rust::wacore_binary::JidExt as _;
use whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info;
use whatsapp_rust::wacore::types::call::{CallAction, IncomingCall};

use calls::{Call, Update};
use store::{Message, Quote, Store};

// ---------- the line protocol ----------

#[derive(Debug, Deserialize, PartialEq)]
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
fn frame(binding: &str, payload: &impl Serialize) -> String {
    let payload = serde_json::to_string(payload).unwrap_or_else(|_| "null".into());
    json!({ "binding": binding, "payload": payload }).to_string()
}

fn emit(binding: &str, payload: &impl Serialize) {
    let line = frame(binding, payload);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

/// A jid without its device part, as a string.
fn bare(jid: &str) -> String {
    jid.parse::<Jid>().map(|j| j.to_non_ad().to_string()).unwrap_or_else(|_| jid.to_owned())
}

fn qr_svg(code: &str) -> Option<String> {
    use qrcode::render::svg;
    let qr = qrcode::QrCode::new(code.as_bytes()).ok()?;
    Some(
        qr.render::<svg::Color>()
            .min_dimensions(256, 256)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
    )
}

// ---------- startup ----------

fn data_dir() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    dir.unwrap_or_else(|| PathBuf::from("."))
}

const SIGNED_OUT: &str = "signed-out";

/// After a sign-out (marked, since Windows won't delete the open database), start clean.
fn wipe_if_signed_out(dir: &Path) {
    if !dir.join(SIGNED_OUT).exists() {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("whatsapp.db") || name.starts_with("chats.json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_file(dir.join(SIGNED_OUT));
}

fn sign_out_and_exit(dir: &Path) -> ! {
    let _ = std::fs::write(dir.join(SIGNED_OUT), b"");
    let _ = std::fs::remove_file(dir.join("chats.json"));
    std::process::exit(0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).target(env_logger::Target::Stderr).init();
    #[cfg(windows)]
    {
        // A 1 ms timer so the mic drain's sleeps are fine-grained (as voip-cli does).
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn timeBeginPeriod(period: u32) -> u32;
        }
        unsafe { timeBeginPeriod(1) };
    }

    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    wipe_if_signed_out(&dir);

    let backend = SqliteStore::open(&dir.join("whatsapp.db").to_string_lossy()).await.map_err(|e| anyhow::anyhow!("sqlite: {e}"))?;
    let bot = Bot::builder().with_backend(backend).build().await.map_err(|e| anyhow::anyhow!("build: {e}"))?;
    let client = bot.client();
    let (handler, events) = ChannelEventHandler::unbounded();
    let _subscription = client.subscribe_handler(handler);
    let running = bot.spawn();

    // stdin on a plain thread: a blocked tokio stdin read would hold up shutdown.
    let (cmd_tx, mut cmds) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if cmd_tx.send(line).is_err() {
                break;
            }
        }
    });

    let (calls_tx, mut call_updates) = mpsc::unbounded_channel();
    let (internal_tx, mut internal) = mpsc::unbounded_channel();
    let mut app = App {
        client,
        store: Store::load(dir.join("chats.json")),
        dir,
        open: None,
        call: None,
        seq: 0,
        emit_due: false,
        save_due: false,
        asked_groups: HashSet::new(),
        calls_tx,
        internal_tx,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut ticks = 0u64;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => app.on_event(&event).await,
                Err(_) => break,
            },
            line = cmds.recv() => match line {
                Some(line) => match parse_cmd(&line) {
                    Some(cmd) => app.on_command(cmd).await,
                    None => warn!("unreadable command line"),
                },
                None => break, // Mesh closed stdin
            },
            Some(update) = call_updates.recv() => app.on_call(update),
            Some(msg) = internal.recv() => app.on_internal(msg),
            _ = tick.tick() => {
                ticks += 1;
                if app.emit_due {
                    app.emit_due = false;
                    app.emit_snapshot();
                }
                if app.save_due && ticks.is_multiple_of(2) {
                    app.save_due = false;
                    app.store.save();
                }
            }
        }
    }
    app.store.save();
    let _ = tokio::time::timeout(Duration::from_secs(3), running.shutdown()).await;
    std::process::exit(0)
}

// ---------- the app ----------

enum Internal {
    Sent { chat_id: String, text: String, quote: Option<Quote>, result: Result<String, String> },
    GroupNames(Vec<(String, String)>),
}

struct App {
    client: Arc<Client>,
    store: Store,
    dir: PathBuf,
    open: Option<String>,
    call: Option<Call>,
    seq: u64,
    emit_due: bool,
    save_due: bool,
    asked_groups: HashSet<String>,
    calls_tx: UnboundedSender<Update>,
    internal_tx: UnboundedSender<Internal>,
}

impl App {
    fn emit_snapshot(&self) {
        emit("waEmit", &self.store.snapshot());
    }

    fn emit_messages(&self, chat_id: &str) {
        emit("waMessages", &json!({ "chat_id": chat_id, "messages": self.store.recent(chat_id) }));
    }

    fn emit_call(&self) {
        emit("waCall", &self.call.as_ref().map(|c| &c.info));
    }

    fn changed(&mut self) {
        self.emit_due = true;
        self.save_due = true;
    }

    async fn on_event(&mut self, event: &Event) {
        match event {
            Event::PairingQrCode(qr) => {
                if let Some(svg) = qr_svg(&qr.code) {
                    emit("waQr", &json!({ "svg": svg }));
                }
            }
            Event::Connected(_) => {
                if let Some(pn) = self.client.pn() {
                    self.store.me = pn.to_non_ad().to_string();
                    if let Some(lid) = self.client.lid() {
                        let me = self.store.me.clone();
                        self.store.map_lid(&lid.to_non_ad().to_string(), &me);
                    }
                }
                emit("waReady", &json!({}));
                self.emit_snapshot();
                self.fetch_group_names(None);
            }
            Event::LoggedOut(_) => {
                warn!("logged out from the phone");
                sign_out_and_exit(&self.dir);
            }
            Event::Messages(batch) => {
                let live = matches!(batch.origin, BatchOrigin::Live);
                for m in batch.messages.iter() {
                    self.on_message(m, live).await;
                }
            }
            Event::HistorySync(sync) => match sync.get() {
                Some(history) => self.on_history(history),
                None => warn!("couldn't decode a history sync chunk"),
            },
            Event::IncomingCall(call) => self.on_incoming_call(call),
            Event::MissedCall(missed) => self.drop_ringing(&missed.call_id),
            Event::CallEndedElsewhere(ended) => self.drop_ringing(&ended.call_id),
            Event::ContactUpdate(update) => {
                let a = &update.action;
                if let (Some(lid), Some(pn)) = (&a.lid_jid, &a.pn_jid) {
                    self.store.map_lid(lid, pn);
                }
                let name = a.full_name.clone().or_else(|| a.first_name.clone()).unwrap_or_default();
                if !name.is_empty() {
                    let id = self.store.canon(&update.jid.to_non_ad().to_string());
                    self.store.contacts.insert(id, name);
                    self.changed();
                }
            }
            Event::MuteUpdate(update) => {
                let id = self.store.canon(&update.jid.to_string());
                self.store.chat(&id).muted = update.action.muted == Some(true);
                self.changed();
            }
            Event::ArchiveUpdate(update) => {
                let id = self.store.canon(&update.jid.to_string());
                self.store.chat(&id).archived = update.action.archived == Some(true);
                self.changed();
            }
            Event::MarkChatAsReadUpdate(update) => {
                let id = self.store.canon(&update.jid.to_string());
                let chat = self.store.chat(&id);
                match update.action.read {
                    Some(true) => chat.unread = 0,
                    Some(false) => chat.unread = chat.unread.max(1),
                    None => {}
                }
                self.changed();
            }
            _ => {}
        }
    }

    async fn on_message(&mut self, m: &InboundMessage, live: bool) {
        let src = &m.info.source;
        let chat = src.chat.to_non_ad();
        if chat.is_status_broadcast() || chat.is_newsletter() || chat.is_broadcast_list() {
            return;
        }
        let chat_s = chat.to_string();
        let sender = src.sender.to_non_ad().to_string();
        for alt in [&src.sender_alt, &src.recipient_alt].into_iter().flatten() {
            let alt = alt.to_non_ad().to_string();
            for (a, b) in [(&chat_s, &alt), (&alt, &chat_s), (&sender, &alt), (&alt, &sender)] {
                self.store.map_lid(a, b);
            }
        }
        if chat.is_lid()
            && !self.store.lid_pn.contains_key(&chat_s)
            && let Ok(Some(entry)) = self.client.get_lid_pn_entry(&chat).await
        {
            self.store.map_lid(&format!("{}@lid", entry.lid), &format!("{}@s.whatsapp.net", entry.phone_number));
        }
        let chat_id = self.store.canon(&chat_s);
        let from_me = src.is_from_me;
        let author_id = if from_me { self.store.me.clone() } else { self.store.canon(&sender) };
        if !from_me && !m.info.push_name.is_empty() {
            self.store.push.insert(author_id.clone(), m.info.push_name.to_string());
        }
        let Some(content) = store::content(&m.message) else {
            debug!("skipping a message with nothing to show");
            return;
        };
        let quote = store::quoted(&m.message).map(|(who, text)| Quote { author: if who.is_empty() { String::new() } else { self.store.name_of(&bare(&who)) }, text });
        let msg = Message {
            id: m.info.id.to_string(),
            author: self.store.name_of(&author_id),
            author_id,
            content: content.clone(),
            timestamp_ms: m.info.timestamp.timestamp_millis().max(0) as u64,
            quote,
            ..Default::default()
        };
        let author = msg.author.clone();
        if !self.store.add(&chat_id, msg) {
            return;
        }
        let is_open = self.open.as_deref() == Some(chat_id.as_str());
        let data = self.store.chat(&chat_id);
        if from_me || is_open {
            data.unread = 0;
        } else {
            data.unread += 1;
        }
        let muted = data.muted;
        self.changed();
        if is_open {
            self.emit_messages(&chat_id);
        }
        if src.is_group {
            self.fetch_group_names(Some(&chat_id));
        }
        if live && !m.info.is_offline && !from_me && !muted {
            let title = if src.is_group { format!("{author} in {}", self.store.name_of(&chat_id)) } else { author };
            emit("waNotify", &json!({ "chat_id": chat_id, "title": title, "body": content }));
        }
    }

    fn on_history(&mut self, history: &wa::HistorySync) {
        let s = &mut self.store;
        for m in &history.phone_number_to_lid_mappings {
            if let (Some(pn), Some(lid)) = (&m.pn_jid, &m.lid_jid) {
                s.map_lid(lid, pn);
            }
        }
        for c in &history.inline_contacts {
            if let (Some(pn), Some(lid)) = (&c.pn_jid, &c.lid_jid) {
                s.map_lid(lid, pn);
            }
            let name = c.full_name.clone().or_else(|| c.first_name.clone()).unwrap_or_default();
            if let Some(jid) = c.pn_jid.as_ref().or(c.lid_jid.as_ref()).filter(|_| !name.is_empty()) {
                let id = s.canon(jid);
                s.contacts.insert(id, name);
            }
        }
        for p in &history.pushnames {
            if let (Some(id), Some(name)) = (&p.id, &p.pushname) {
                let id = s.canon(id);
                s.push.insert(id, name.clone());
            }
        }
        let now = now_ms();
        for conv in &history.conversations {
            if let (Some(lid), Some(pn)) = (&conv.lid_jid, &conv.pn_jid) {
                s.map_lid(lid, pn);
            }
            let id = s.canon(&conv.id);
            if id.ends_with("@broadcast") || id.ends_with("@newsletter") {
                continue;
            }
            let group = id.ends_with("@g.us");
            let me = s.me.clone();
            let mut msgs = Vec::new();
            for hm in &conv.messages {
                let Some(wmi) = hm.message.as_option() else { continue };
                let Some(key) = wmi.key.as_option() else { continue };
                let (Some(msg_id), Some(body)) = (key.id.clone(), wmi.message.as_option()) else { continue };
                let Some(content) = store::content(body) else { continue };
                let from_me = key.from_me == Some(true);
                let author_id = if from_me {
                    me.clone()
                } else if group {
                    s.canon(&bare(wmi.participant.as_deref().or(key.participant.as_deref()).unwrap_or_default()))
                } else {
                    id.clone()
                };
                if let Some(name) = wmi.push_name.as_ref().filter(|n| !from_me && !n.is_empty()) {
                    s.push.insert(author_id.clone(), name.clone());
                }
                let quote = store::quoted(body).map(|(who, text)| Quote { author: if who.is_empty() { String::new() } else { s.name_of(&bare(&who)) }, text });
                msgs.push(Message {
                    id: msg_id,
                    author: s.name_of(&author_id),
                    author_id,
                    content,
                    timestamp_ms: wmi.message_timestamp.unwrap_or_default() * 1000,
                    quote,
                    ..Default::default()
                });
            }
            for msg in msgs {
                s.add(&id, msg);
            }
            let chat = s.chat(&id);
            if let Some(name) = conv.name.as_ref().or(conv.display_name.as_ref()).filter(|n| !n.is_empty()) {
                chat.name = name.clone();
            }
            chat.unread = conv.unread_count.unwrap_or_default() as u64;
            chat.archived = conv.archived == Some(true);
            chat.muted = conv.mute_end_time.is_some_and(|t| if t < 100_000_000_000 { t * 1000 } else { t } > now);
            let last = conv.conversation_timestamp.or(conv.last_msg_timestamp).unwrap_or_default() * 1000;
            chat.last_ms = chat.last_ms.max(last);
        }
        self.changed();
    }

    /// Group subjects: all of them after connecting, or one group we have no name for.
    fn fetch_group_names(&mut self, only: Option<&str>) {
        if let Some(id) = only
            && (self.store.contacts.contains_key(id) || !self.asked_groups.insert(id.to_owned()))
        {
            return;
        }
        let (client, tx, only) = (self.client.clone(), self.internal_tx.clone(), only.map(str::to_owned));
        tokio::spawn(async move {
            let names = match only {
                Some(id) => match id.parse::<Jid>() {
                    Ok(jid) => client.groups().fetch_metadata(&jid).await.map(|m| vec![(id, m.subject.unwrap_or_default())]).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
                None => client
                    .groups()
                    .list_participating()
                    .await
                    .map(|groups| groups.into_iter().map(|g| (g.id.to_string(), g.subject.unwrap_or_default())).collect())
                    .map_err(|e| e.to_string()),
            };
            match names {
                Ok(names) => {
                    let _ = tx.send(Internal::GroupNames(names));
                }
                Err(e) => warn!("couldn't fetch group names: {e}"),
            }
        });
    }

    fn on_internal(&mut self, msg: Internal) {
        match msg {
            Internal::GroupNames(names) => {
                for (id, subject) in names.into_iter().filter(|(_, s)| !s.is_empty()) {
                    self.store.contacts.insert(id, subject);
                }
                self.changed();
            }
            Internal::Sent { chat_id, text, quote, result } => match result {
                Ok(id) => {
                    let msg = Message { id, author_id: self.store.me.clone(), author: "You".into(), content: text, timestamp_ms: now_ms(), quote, ..Default::default() };
                    self.store.add(&chat_id, msg);
                    self.store.chat(&chat_id).unread = 0;
                    self.save_due = true;
                    emit("waSent", &json!({ "chat_id": chat_id, "ok": true }));
                    if self.open.as_deref() == Some(chat_id.as_str()) {
                        self.emit_messages(&chat_id);
                    }
                    self.emit_snapshot();
                }
                Err(error) => emit("waSent", &json!({ "chat_id": chat_id, "error": error })),
            },
        }
    }

    // ---------- commands ----------

    async fn on_command(&mut self, cmd: Cmd) {
        let arg = |i: usize| cmd.args.get(i).cloned().unwrap_or_default();
        let not_yet = "not in native mode yet";
        match cmd.func.as_str() {
            "open" => {
                let id = arg(0);
                if id.is_empty() {
                    self.open = None;
                    return;
                }
                if let Some(chat) = self.store.chats.get_mut(&id) {
                    chat.unread = 0;
                    self.changed();
                }
                self.emit_messages(&id);
                self.open = Some(id);
            }
            "send" => self.send(arg(0), arg(1), arg(2)),
            "signOut" => {
                let report = self.client.logout().await;
                info!("logged out: {:?}", report.deregistration);
                sign_out_and_exit(&self.dir);
            }
            "searchContacts" => {
                let q = arg(0);
                emit("waContacts", &json!({ "query": q, "contacts": self.store.search(&q) }));
            }
            "startChat" => self.start_chat(&arg(0)),
            "fetchMedia" => {
                let url = arg(0);
                if url.starts_with("http://") || url.starts_with("https://") {
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        use base64::Engine as _;
                        let payload = match client.http_client().execute(HttpRequest::get(url.clone())).await {
                            Ok(r) if (200..300).contains(&r.status_code) => json!({ "url": url, "data": base64::engine::general_purpose::STANDARD.encode(&r.body) }),
                            Ok(r) => json!({ "url": url, "error": format!("HTTP {}", r.status_code) }),
                            Err(e) => json!({ "url": url, "error": e.to_string() }),
                        };
                        emit("waMedia", &payload);
                    });
                } else {
                    emit("waMedia", &json!({ "url": url, "error": not_yet }));
                }
            }
            "saveMedia" => emit("waFile", &json!({ "id": arg(0), "error": not_yet })),
            "sendFile" => emit("waSent", &json!({ "chat_id": arg(0), "error": not_yet })),
            "forward" => emit("waForwarded", &json!({ "chat_id": "", "error": not_yet })),
            "voicePcm" => emit("waPcm", &json!({ "id": arg(0), "error": not_yet })),
            "archive" | "unarchive" | "deleteChat" => emit("waChatAction", &json!({ "chat_id": arg(0), "error": not_yet })),
            "react" => info!("reactions aren't in native mode yet"),
            "callStart" => self.call_start(&arg(0)),
            "callAccept" => self.call_accept(),
            "callDecline" => {
                if let Some(incoming) = self.call.as_mut().and_then(|c| c.incoming.take()) {
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        if let Err(e) = client.voip().reject(&incoming).await {
                            warn!("reject failed: {e}");
                        }
                    });
                }
                self.end_call();
            }
            "callEnd" => {
                if let Some(call) = self.call.take() {
                    hang_up(self.client.clone(), call);
                }
                self.emit_call();
            }
            "callMute" => {
                let muted = arg(0) == "true";
                if let Some(call) = self.call.as_mut() {
                    call.info.muted = muted;
                    if let Some(handle) = call.handle.clone() {
                        tokio::spawn(async move {
                            if let Err(e) = handle.set_muted(muted).await {
                                warn!("mute failed: {e}");
                            }
                        });
                    }
                }
                self.emit_call();
            }
            "callCamera" | "callScreen" => emit("waCall", &json!({ "error": "video isn't in native WhatsApp yet" })),
            other => warn!("unknown command {other}"),
        }
    }

    fn send(&mut self, chat_id: String, text: String, reply_to: String) {
        let jid = match chat_id.parse::<Jid>() {
            Ok(jid) => jid,
            Err(e) => return emit("waSent", &json!({ "chat_id": chat_id, "error": e.to_string() })),
        };
        let quoted = self.store.find(&chat_id, &reply_to).cloned();
        let (message, quote) = match quoted.as_ref().and_then(|q| Some((q, q.author_id.parse::<Jid>().ok()?))) {
            Some((q, author)) => {
                let ctx = build_quote_context_with_info(q.id.clone(), &author, &jid, &jid, &wa::Message::text(q.content.clone()));
                (wa::Message::text_with_context(text.clone(), ctx), Some(Quote { author: self.store.name_of(&q.author_id), text: q.content.clone() }))
            }
            None => (wa::Message::text(text.clone()), None),
        };
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let result = client.send_message(jid, message).await.map(|r| r.message_id.to_string()).map_err(|e| e.to_string());
            let _ = tx.send(Internal::Sent { chat_id, text, quote, result });
        });
    }

    fn start_chat(&mut self, who: &str) {
        let who = who.trim();
        let digits = who.strip_prefix('+').unwrap_or(who);
        let id = if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            Jid::pn(digits).to_string()
        } else if self.store.chats.contains_key(who) || who.contains('@') {
            self.store.canon(who)
        } else {
            return emit("waChatStarted", &json!({ "error": format!("{who} isn't a phone number") }));
        };
        let chat = self.store.chat(&id);
        if chat.last_ms == 0 {
            chat.last_ms = now_ms();
        }
        self.changed();
        emit("waChatStarted", &json!({ "chat": self.store.chat_view(&id) }));
    }

    // ---------- calls ----------

    fn on_incoming_call(&mut self, call: &IncomingCall) {
        match &call.action {
            CallAction::Offer { call_id, group_jid, .. } => {
                if call.offline || group_jid.is_some() {
                    info!("ignoring a {} call offer", if call.offline { "stale" } else { "group" });
                    return;
                }
                if self.call.is_some() {
                    info!("already in a call; leaving {call_id} to ring out");
                    return;
                }
                let peer = self.store.canon(&call.from.to_non_ad().to_string());
                let mut name = self.store.name_of(&peer);
                if name.starts_with(|c: char| c == '+' || c.is_ascii_digit())
                    && let Some(notify) = call.notify.clone().filter(|n| !n.is_empty())
                {
                    name = notify;
                }
                self.seq += 1;
                self.call = Some(Call { seq: self.seq, info: calls::info(call_id, &peer, name, "incoming"), incoming: Some(Box::new(call.clone())), handle: None });
                self.emit_call();
            }
            CallAction::Terminate { call_id, .. } => self.drop_ringing(call_id),
            _ => {}
        }
    }

    /// The ringing (not yet answered) call `call_id` went away.
    fn drop_ringing(&mut self, call_id: &str) {
        if self.call.as_ref().is_some_and(|c| c.info.id == call_id && c.handle.is_none()) {
            self.end_call();
        }
    }

    fn end_call(&mut self) {
        self.call = None;
        self.emit_call();
    }

    fn call_start(&mut self, chat_id: &str) {
        if self.call.is_some() {
            return emit("waCall", &json!({ "error": "already in a call" }));
        }
        let peer = match chat_id.parse::<Jid>() {
            Ok(jid) if !jid.is_group() => jid,
            Ok(_) => return emit("waCall", &json!({ "error": "group calls aren't in native WhatsApp yet" })),
            Err(e) => return emit("waCall", &json!({ "error": e.to_string() })),
        };
        self.seq += 1;
        let name = self.store.name_of(chat_id);
        self.call = Some(Call { seq: self.seq, info: calls::info("", chat_id, name, "outgoing"), incoming: None, handle: None });
        self.emit_call();
        calls::dial(self.client.clone(), peer, self.seq, self.calls_tx.clone());
    }

    fn call_accept(&mut self) {
        let Some(call) = self.call.as_mut() else {
            return emit("waCall", &json!({ "error": "no call to answer" }));
        };
        let Some(incoming) = call.incoming.take() else { return };
        call.info.phase = "connecting".into();
        calls::answer(self.client.clone(), incoming, call.seq, self.calls_tx.clone());
        self.emit_call();
    }

    fn on_call(&mut self, update: Update) {
        let current = self.call.as_ref().map(|c| c.seq);
        match update {
            Update::Started(seq, Ok(handle)) if Some(seq) == current => {
                if let Some(call) = self.call.as_mut() {
                    call.info.id = handle.call_id().to_owned();
                    call.handle = Some(handle);
                }
                self.emit_call();
            }
            Update::Started(_, Ok(handle)) => {
                // Hung up while it was still starting.
                tokio::spawn(async move { handle.terminate().await });
            }
            Update::Started(seq, Err(error)) if Some(seq) == current => {
                emit("waCall", &json!({ "error": format!("call failed: {error}") }));
                self.end_call();
            }
            Update::Phase(seq, phase) if Some(seq) == current => {
                if let Some(call) = self.call.as_mut().filter(|c| c.info.phase != phase) {
                    call.info.phase = phase.into();
                    self.emit_call();
                }
            }
            Update::Ended(seq) if Some(seq) == current => self.end_call(),
            _ => {}
        }
    }
}

fn hang_up(client: Arc<Client>, call: Call) {
    tokio::spawn(async move {
        if let Some(handle) = call.handle {
            handle.terminate().await;
        } else if let Some(incoming) = call.incoming {
            let _ = client.voip().reject(&incoming).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_commands_and_frames_reports() {
        let cmd = parse_cmd(r#"{"fn":"send","args":["1@s.whatsapp.net","hi \"there\"",""]}"#).unwrap();
        assert_eq!(cmd, Cmd { func: "send".into(), args: vec!["1@s.whatsapp.net".into(), "hi \"there\"".into(), "".into()] });
        assert_eq!(parse_cmd(r#"{"fn":"open"}"#).unwrap().args, Vec::<String>::new());
        assert!(parse_cmd("not json").is_none());

        // The payload is itself JSON, carried as a string.
        let line = frame("waSent", &json!({ "chat_id": "1@s.whatsapp.net", "ok": true }));
        assert!(!line.contains('\n'));
        let outer: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(outer["binding"], "waSent");
        let inner: serde_json::Value = serde_json::from_str(outer["payload"].as_str().unwrap()).unwrap();
        assert_eq!(inner["ok"], true);
        assert_eq!(frame("waCall", &None::<calls::Info>), r#"{"binding":"waCall","payload":"null"}"#);
    }

    #[test]
    fn snapshot_is_most_recent_first_and_splits_archived() {
        let mut store = Store::default();
        for (id, last, archived) in [("1@s.whatsapp.net", 10, false), ("2@g.us", 30, false), ("3@s.whatsapp.net", 20, true), ("4@s.whatsapp.net", 20, false)] {
            let chat = store.chat(id);
            chat.last_ms = last;
            chat.archived = archived;
        }
        store.contacts.insert("2@g.us".into(), "Family".into());
        let snap = store.snapshot();
        let ids: Vec<&str> = snap.chats.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["2@g.us", "4@s.whatsapp.net", "1@s.whatsapp.net"]);
        assert_eq!((snap.chats[0].name.as_str(), snap.chats[0].group), ("Family", true));
        assert_eq!(snap.chats[1].name, "+4");
        assert_eq!(snap.archived.len(), 1);
    }
}
