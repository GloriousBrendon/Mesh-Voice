//! mesh-wa: WhatsApp for Mesh, natively through whatsapp-rust.
//!
//! `mesh-wa --data <dir>`. Commands arrive on stdin as `{"fn":..,"args":[..]}` lines; reports go
//! out on stdout as `{"binding":..,"payload":"<json string>"}` lines. Logs go to stderr.

mod audio;
mod calls;
mod media;
mod store;

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::{self, UnboundedSender};
use whatsapp_rust::buffa::MessageField;
use whatsapp_rust::download::{Downloadable, MediaType};
use whatsapp_rust::media::{DocumentOptions, ImageOptions, VideoOptions, document_message, image_message, video_message};
use whatsapp_rust::prelude::*;
use whatsapp_rust::{ProfilePictureRequest, ProfilePictureTarget, ProfilePictureType, UploadOptions};
use whatsapp_rust::wacore::net::HttpRequest;
use whatsapp_rust::wacore_binary::JidExt as _;
use whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info;
use whatsapp_rust::voip::VideoUpgradeToken;
use whatsapp_rust::wacore::types::call::{CallAction, IncomingCall, VideoState};

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
        messages_due: false,
        asked_groups: HashSet::new(),
        asked_pics: HashSet::new(),
        calls_tx,
        internal_tx,
        screen_id: String::new(),
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
                if std::mem::take(&mut app.messages_due) && let Some(id) = app.open.clone() {
                    app.emit_messages(&id);
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
    /// A message we sent (text, file or forward), or why not; `binding` is the report Mesh waits on.
    Sent { chat_id: String, message: Box<wa::Message>, quote: Option<Quote>, binding: &'static str, result: Result<String, String> },
    GroupNames(Vec<(String, String)>),
    /// A profile picture URL, by jid.
    Avatar(String, String),
    /// The chat `startChat` found, or why not.
    ChatStarted(Result<String, String>),
    /// How an archive / unarchive / delete went.
    ChatAction { chat_id: String, action: ChatAction, result: Result<(), String> },
}

#[derive(Clone, Copy, PartialEq)]
enum ChatAction {
    Archive,
    Unarchive,
    Delete,
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
    /// The open chat's messages changed (pictures arrived); send them again on the next tick.
    messages_due: bool,
    asked_groups: HashSet<String>,
    asked_pics: HashSet<String>,
    calls_tx: UnboundedSender<Update>,
    internal_tx: UnboundedSender<Internal>,
    /// The screen or window `callScreen` last named.
    screen_id: String,
}

/// How many chats get their profile pictures looked up, from the top of the list.
const PICTURED: usize = 40;

impl App {
    fn emit_snapshot(&mut self) {
        let snap = self.store.snapshot();
        let ids: Vec<String> = snap.chats.iter().take(PICTURED).map(|c| c.id.clone()).collect();
        emit("waEmit", &snap);
        self.ask_avatars(ids);
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

    fn is_open(&self, chat_id: &str) -> bool {
        self.open.as_deref() == Some(chat_id)
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
        // A reaction lands on its message.
        if let Some(r) = m.message.get_base_message().reaction_message.as_option() {
            let target = r.key.as_option().and_then(|k| k.id.as_deref()).unwrap_or_default();
            if self.store.react(&chat_id, target, &author_id, r.text.as_deref().unwrap_or_default()) {
                self.save_due = true;
                if self.is_open(&chat_id) {
                    self.emit_messages(&chat_id);
                }
            }
            return;
        }
        let Some(body) = store::body(&m.message) else {
            debug!("skipping a message with nothing to show");
            return;
        };
        let notice = body.line();
        let quote = store::quoted(&m.message).map(|(who, text)| Quote { author: if who.is_empty() { String::new() } else { self.store.name_of(&bare(&who)) }, text });
        let msg = Message {
            id: m.info.id.to_string(),
            author: self.store.name_of(&author_id),
            author_id,
            content: body.content,
            timestamp_ms: m.info.timestamp.timestamp_millis().max(0) as u64,
            image: body.image,
            file: body.file,
            media: body.media,
            quote,
            ..Default::default()
        };
        let author = msg.author.clone();
        if !self.store.add(&chat_id, msg) {
            return;
        }
        let is_open = self.is_open(&chat_id);
        let data = self.store.chat(&chat_id);
        if from_me {
            data.unread = 0;
        } else {
            data.unread += 1;
        }
        let muted = data.muted;
        self.changed();
        if is_open {
            self.emit_messages(&chat_id);
            // On screen in Mesh, so seen the moment it lands (as WhatsApp Web does when focused).
            self.mark_read(&chat_id, false);
        }
        if src.is_group {
            self.fetch_group_names(Some(&chat_id));
        }
        if live && !m.info.is_offline && !from_me && !muted {
            let title = if src.is_group { format!("{author} in {}", self.store.name_of(&chat_id)) } else { author };
            emit("waNotify", &json!({ "chat_id": chat_id, "title": title, "body": notice }));
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
            // Who sent a message (or a reaction), from its key.
            let who = |s: &Store, from_me: bool, participant: Option<&str>| {
                if from_me {
                    me.clone()
                } else if group {
                    s.canon(&bare(participant.unwrap_or_default()))
                } else {
                    id.clone()
                }
            };
            let mut msgs = Vec::new();
            for hm in &conv.messages {
                let Some(wmi) = hm.message.as_option() else { continue };
                let Some(key) = wmi.key.as_option() else { continue };
                let (Some(msg_id), Some(body)) = (key.id.clone(), wmi.message.as_option()) else { continue };
                let Some(shown) = store::body(body) else { continue };
                let from_me = key.from_me == Some(true);
                let author_id = who(s, from_me, wmi.participant.as_deref().or(key.participant.as_deref()));
                if let Some(name) = wmi.push_name.as_ref().filter(|n| !from_me && !n.is_empty()) {
                    s.push.insert(author_id.clone(), name.clone());
                }
                let quote = store::quoted(body).map(|(who, text)| Quote { author: if who.is_empty() { String::new() } else { s.name_of(&bare(&who)) }, text });
                let reacts = wmi
                    .reactions
                    .iter()
                    .filter_map(|r| {
                        let k = r.key.as_option()?;
                        let emoji = r.text.clone().filter(|t| !t.is_empty())?;
                        Some((who(s, k.from_me == Some(true), k.participant.as_deref()), emoji))
                    })
                    .collect();
                msgs.push(Message {
                    id: msg_id,
                    author: s.name_of(&author_id),
                    author_id,
                    content: shown.content,
                    timestamp_ms: wmi.message_timestamp.unwrap_or_default() * 1000,
                    image: shown.image,
                    file: shown.file,
                    media: shown.media,
                    quote,
                    reacts,
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

    /// Profile pictures (small ones), once a run each: they come back as `Internal::Avatar`.
    // ponytail: one lookup at a time per batch; WhatsApp rate-limits these (429s just get logged).
    fn ask_avatars(&mut self, ids: Vec<String>) {
        let ids: Vec<String> = ids.into_iter().filter(|id| !id.is_empty() && self.asked_pics.insert(id.clone())).collect();
        if ids.is_empty() {
            return;
        }
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            for id in ids {
                let Ok(jid) = id.parse::<Jid>() else { continue };
                let target = if jid.is_group() { ProfilePictureTarget::Group(&jid) } else { ProfilePictureTarget::Contact(&jid) };
                match client.pictures().lookup(ProfilePictureRequest::new(target, ProfilePictureType::Preview)).await {
                    Ok(found) => {
                        if let Some(pic) = found.into_found() {
                            let _ = tx.send(Internal::Avatar(id, pic.url));
                        }
                    }
                    Err(e) => debug!("no picture for {id}: {e}"),
                }
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
            Internal::Avatar(id, url) => {
                self.store.avatars.insert(id, url);
                self.emit_due = true;
                self.messages_due = self.open.is_some();
            }
            Internal::Sent { chat_id, message, quote, binding, result } => match result {
                Ok(id) => {
                    let body = store::body(&message).unwrap_or_default();
                    let msg = Message {
                        id,
                        author_id: self.store.me.clone(),
                        author: "You".into(),
                        content: body.content,
                        timestamp_ms: now_ms(),
                        image: body.image,
                        file: body.file,
                        media: body.media,
                        quote,
                        ..Default::default()
                    };
                    self.store.add(&chat_id, msg);
                    self.store.chat(&chat_id).unread = 0;
                    self.save_due = true;
                    emit(binding, &json!({ "chat_id": chat_id, "ok": true }));
                    if self.is_open(&chat_id) {
                        self.emit_messages(&chat_id);
                    }
                    self.emit_snapshot();
                }
                Err(error) => emit(binding, &json!({ "chat_id": chat_id, "ok": false, "error": error })),
            },
            Internal::ChatStarted(Ok(id)) => {
                let chat = self.store.chat(&id);
                if chat.last_ms == 0 {
                    chat.last_ms = now_ms();
                }
                self.changed();
                emit("waChatStarted", &json!({ "chat": self.store.chat_view(&id) }));
            }
            Internal::ChatStarted(Err(error)) => emit("waChatStarted", &json!({ "error": error })),
            Internal::ChatAction { chat_id, action, result } => {
                if result.is_ok() {
                    match action {
                        ChatAction::Archive | ChatAction::Unarchive => self.store.chat(&chat_id).archived = action == ChatAction::Archive,
                        ChatAction::Delete => {
                            self.store.chats.remove(&chat_id);
                        }
                    }
                    self.changed();
                }
                emit("waChatAction", &json!({ "chat_id": chat_id, "ok": result.is_ok(), "error": result.err() }));
            }
        }
    }

    // ---------- commands ----------

    async fn on_command(&mut self, cmd: Cmd) {
        let arg = |i: usize| cmd.args.get(i).cloned().unwrap_or_default();
        match cmd.func.as_str() {
            "open" => {
                let id = arg(0);
                if id.is_empty() {
                    self.open = None;
                    return;
                }
                self.emit_messages(&id);
                self.open = Some(id.clone());
                // Opening a chat marks it read, here and on the phone, as WhatsApp Web does.
                self.mark_read(&id, true);
                let mut pics = vec![id.clone()];
                pics.extend(self.store.recent(&id).into_iter().map(|m| m.author_id).filter(|a| *a != self.store.me));
                pics.dedup();
                self.ask_avatars(pics);
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
            "fetchMedia" => self.fetch_media(arg(0)),
            "saveMedia" => self.save_media(arg(0)),
            "voicePcm" => self.voice_pcm(arg(0)),
            "sendFile" => self.send_file(arg(0), arg(1), arg(2), &arg(3), arg(4)),
            "forward" => self.forward(&arg(0), arg(1)),
            "react" => self.react(&arg(0), &arg(1)),
            "archive" => self.chat_action(arg(0), ChatAction::Archive),
            "unarchive" => self.chat_action(arg(0), ChatAction::Unarchive),
            "deleteChat" => self.chat_action(arg(0), ChatAction::Delete),
            "callStart" => self.call_start(&arg(0), arg(1) == "true"),
            "callAccept" => self.call_accept(arg(0) == "true"),
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
            "callCamera" => self.call_video(Some(arg(0) == "true"), None),
            "callScreen" => self.call_video(None, Some((arg(0) == "true", arg(1)))),
            "listSources" => list_sources(),
            other => warn!("unknown command {other}"),
        }
    }

    /// Sends read receipts for the chat's unread messages from others; `sync` also marks the chat
    /// read on the phone and other devices.
    // ponytail: receipts go to the jids we file people under (their number), not the LID a
    // message may have come from; WhatsApp maps them, but that's untested.
    fn mark_read(&mut self, chat_id: &str, sync: bool) {
        let me = self.store.me.clone();
        let Some(chat) = self.store.chats.get_mut(chat_id) else { return };
        let unread = std::mem::take(&mut chat.unread) as usize;
        if unread == 0 {
            return;
        }
        self.changed();
        let Ok(jid) = chat_id.parse::<Jid>() else { return };
        let Some(chat) = self.store.chats.get(chat_id) else { return };
        let mut by_sender: HashMap<String, Vec<String>> = HashMap::new();
        for m in chat.messages.iter().rev().filter(|m| m.author_id != me && !m.system).take(unread.min(50)) {
            by_sender.entry(m.author_id.clone()).or_default().push(m.id.clone());
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            for (sender, ids) in by_sender {
                let sender = sender.parse::<Jid>().ok().filter(|_| jid.is_group());
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                if let Err(e) = client.mark_as_read(&jid, sender.as_ref(), &ids).await {
                    warn!("read receipts failed: {e}");
                }
            }
            if sync && let Err(e) = client.chat_actions().mark_chat_as_read(&jid, true, None).await {
                warn!("marking the chat read failed: {e}");
            }
        });
    }

    fn send(&mut self, chat_id: String, text: String, reply_to: String) {
        let jid = match chat_id.parse::<Jid>() {
            Ok(jid) => jid,
            Err(e) => return emit("waSent", &json!({ "chat_id": chat_id, "ok": false, "error": e.to_string() })),
        };
        let quoted = self.store.find(&chat_id, &reply_to).cloned();
        let (message, quote) = match quoted.as_ref().and_then(|q| Some((q, q.author_id.parse::<Jid>().ok()?))) {
            Some((q, author)) => {
                let shown = if q.content.is_empty() { q.file.as_ref().and_then(|f| f["kind"].as_str()).unwrap_or("Photo").to_owned() } else { q.content.clone() };
                let ctx = build_quote_context_with_info(q.id.clone(), &author, &jid, &jid, &wa::Message::text(shown.clone()));
                (wa::Message::text_with_context(text, ctx), Some(Quote { author: self.store.name_of(&q.author_id), text: shown }))
            }
            None => (wa::Message::text(text), None),
        };
        self.send_built(chat_id, message, quote, "waSent");
    }

    /// Sends `message`; reports through `binding` once it's out (see `Internal::Sent`).
    fn send_built(&self, chat_id: String, message: wa::Message, quote: Option<Quote>, binding: &'static str) {
        let jid = match chat_id.parse::<Jid>() {
            Ok(jid) => jid,
            Err(e) => return emit(binding, &json!({ "chat_id": chat_id, "ok": false, "error": e.to_string() })),
        };
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let result = client.send_message(jid, message.clone()).await.map(|r| r.message_id.to_string()).map_err(|e| e.to_string());
            let _ = tx.send(Internal::Sent { chat_id, message: Box::new(message), quote, binding, result });
        });
    }

    /// Uploads a file and sends it: images as photos, videos as videos, the rest as documents.
    fn send_file(&self, chat_id: String, filename: String, mimetype: String, data: &str, caption: String) {
        let bytes = match B64.decode(data) {
            Ok(bytes) => bytes,
            Err(e) => return emit("waSent", &json!({ "chat_id": chat_id, "ok": false, "error": e.to_string() })),
        };
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let built = async {
                let caption = Some(caption).filter(|c| !c.is_empty());
                let kind = if mimetype.starts_with("image/") {
                    MediaType::Image
                } else if mimetype.starts_with("video/") {
                    MediaType::Video
                } else {
                    MediaType::Document
                };
                let preview = match kind {
                    MediaType::Image => {
                        let bytes = bytes.clone();
                        tokio::task::spawn_blocking(move || media::preview(&bytes)).await.ok().flatten()
                    }
                    _ => None,
                };
                let upload = client.upload(bytes, kind, UploadOptions::new()).await.map_err(|e| format!("upload failed: {e}"))?;
                Ok::<_, String>(match kind {
                    MediaType::Image => {
                        let mut opts = ImageOptions::default().with_mimetype(&mimetype);
                        opts.caption = caption;
                        if let Some((thumb, _, _)) = &preview {
                            opts = opts.with_jpeg_thumbnail(thumb.clone());
                        }
                        let mut msg = image_message(upload, opts);
                        if let (Some(img), Some((_, w, h))) = (msg.image_message.as_option_mut(), preview) {
                            (img.width, img.height) = (Some(w), Some(h));
                        }
                        msg
                    }
                    MediaType::Video => {
                        let mut opts = VideoOptions::default().with_mimetype(&mimetype);
                        opts.caption = caption;
                        video_message(upload, opts)
                    }
                    _ => {
                        let mut opts = DocumentOptions::default().with_mimetype(&mimetype).with_file_name(&filename).with_title(&filename);
                        opts.caption = caption;
                        document_message(upload, opts)
                    }
                })
            }
            .await;
            match built {
                Ok(message) => {
                    let jid = match chat_id.parse::<Jid>() {
                        Ok(jid) => jid,
                        Err(e) => return emit("waSent", &json!({ "chat_id": chat_id, "ok": false, "error": e.to_string() })),
                    };
                    let result = client.send_message(jid, message.clone()).await.map(|r| r.message_id.to_string()).map_err(|e| e.to_string());
                    let _ = tx.send(Internal::Sent { chat_id, message: Box::new(message), quote: None, binding: "waSent", result });
                }
                Err(error) => emit("waSent", &json!({ "chat_id": chat_id, "ok": false, "error": error })),
            }
        });
    }

    /// Forwards a message, keeping its caption; media goes as it is on WhatsApp's servers.
    fn forward(&self, msg_id: &str, to: String) {
        let Some((_, m)) = self.store.find_any(msg_id) else {
            return emit("waForwarded", &json!({ "chat_id": to, "ok": false, "error": "couldn't find that message" }));
        };
        let mut ctx = wa::ContextInfo::default();
        (ctx.is_forwarded, ctx.forwarding_score) = (Some(true), Some(1));
        let message = match store::media(m) {
            Some(mut media) => {
                let ctx = || MessageField::some(ctx.clone());
                if let Some(x) = media.image_message.as_option_mut() {
                    x.context_info = ctx();
                } else if let Some(x) = media.video_message.as_option_mut() {
                    x.context_info = ctx();
                } else if let Some(x) = media.audio_message.as_option_mut() {
                    x.context_info = ctx();
                } else if let Some(x) = media.document_message.as_option_mut() {
                    x.context_info = ctx();
                } else if let Some(x) = media.sticker_message.as_option_mut() {
                    x.context_info = ctx();
                }
                media
            }
            None => wa::Message::text_with_context(m.content.clone(), ctx),
        };
        self.send_built(to, message, None, "waForwarded");
    }

    /// Reacts to a message (an empty emoji takes ours back).
    // ponytail: a group sender's key carries the jid we file them under, which may not be the
    // LID the message came from.
    fn react(&mut self, msg_id: &str, emoji: &str) {
        let Some((chat_id, m)) = self.store.find_any(msg_id) else { return warn!("no message {msg_id} to react to") };
        let (chat_id, author_id, me) = (chat_id.to_owned(), m.author_id.clone(), self.store.me.clone());
        let Ok(chat) = chat_id.parse::<Jid>() else { return };
        let mut key = wa::MessageKey::default();
        (key.remote_jid, key.from_me, key.id) = (Some(chat_id.clone()), Some(author_id == me), Some(msg_id.to_owned()));
        if chat.is_group() && author_id != me {
            key.participant = Some(author_id);
        }
        let (client, emoji_s) = (self.client.clone(), emoji.to_owned());
        tokio::spawn(async move {
            if let Err(e) = client.send_reaction_raw(chat, key, &emoji_s).await {
                warn!("reaction failed: {e}");
            }
        });
        self.store.react(&chat_id, msg_id, &me, emoji);
        self.save_due = true;
        if self.is_open(&chat_id) {
            self.emit_messages(&chat_id);
        }
    }

    fn chat_action(&mut self, chat_id: String, action: ChatAction) {
        let jid = match chat_id.parse::<Jid>() {
            Ok(jid) => jid,
            Err(e) => return emit("waChatAction", &json!({ "chat_id": chat_id, "ok": false, "error": e.to_string() })),
        };
        if action == ChatAction::Delete && jid.is_group() {
            return emit("waChatAction", &json!({ "chat_id": chat_id, "ok": false, "error": "WhatsApp wouldn't delete it (leave the group first)" }));
        }
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let actions = client.chat_actions();
            let result = match action {
                ChatAction::Archive => actions.archive_chat(&jid, None).await,
                ChatAction::Unarchive => actions.unarchive_chat(&jid, None).await,
                ChatAction::Delete => actions.delete_chat(&jid, false, None).await,
            };
            let _ = tx.send(Internal::ChatAction { chat_id, action, result: result.map_err(|e| e.to_string()) });
        });
    }

    /// Opens (or makes) the chat with a jid or a phone number, checking the number is on WhatsApp.
    fn start_chat(&mut self, who: &str) {
        let who = who.trim();
        if who.contains('@') || self.store.chats.contains_key(who) {
            let id = self.store.canon(who);
            return self.on_internal(Internal::ChatStarted(Ok(id)));
        }
        let Some(digits) = phone_digits(who, &self.store.me) else {
            return emit("waChatStarted", &json!({ "error": "that doesn't look like a phone number" }));
        };
        let (client, tx) = (self.client.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let found = client.contacts().is_on_whatsapp(&[Jid::pn(&digits)]).await;
            let result = match found {
                Ok(found) => found.into_iter().find(|r| r.is_registered).map(|r| r.jid.to_non_ad().to_string()).ok_or_else(|| format!("+{digits} isn't on WhatsApp")),
                Err(e) => Err(format!("couldn't check +{digits}: {e}")),
            };
            let _ = tx.send(Internal::ChatStarted(result));
        });
    }

    // ---------- media ----------

    /// A message's media part, as kept in the store.
    fn media_of(&self, id: &str) -> Result<wa::Message, String> {
        self.store.find_any(id).and_then(|(_, m)| store::media(m)).ok_or_else(|| "no such message".into())
    }

    /// `wa-msg:<id>` (a message's picture, 480 px), `wa-big:<id>` (1600 px, full screen),
    /// `wa-gif:<id>` (a GIF's frames, through `waFrames`), or a URL (profile pictures).
    fn fetch_media(&self, url: String) {
        let client = self.client.clone();
        if url.starts_with("http://") || url.starts_with("https://") {
            tokio::spawn(async move {
                let payload = match client.http_client().execute(HttpRequest::get(url.clone())).await {
                    Ok(r) if (200..300).contains(&r.status_code) => json!({ "url": url, "data": B64.encode(&r.body) }),
                    Ok(r) => json!({ "url": url, "error": format!("HTTP {}", r.status_code) }),
                    Err(e) => json!({ "url": url, "error": e.to_string() }),
                };
                emit("waMedia", &payload);
            });
            return;
        }
        let (scheme, id) = url.split_once(':').unwrap_or_default();
        let media = self.media_of(id);
        let scheme = scheme.to_owned();
        tokio::spawn(async move {
            let bytes = match media {
                Ok(m) => download(&client, &m).await.map(|b| (b, m.sticker_message.is_set())),
                Err(e) => Err(e),
            };
            let payload = match scheme.as_str() {
                "wa-gif" => {
                    let frames = match bytes {
                        Ok((mp4, _)) => blocking(move || media::gif_frames(&mp4)).await,
                        Err(e) => Err(e),
                    };
                    let payload = match frames {
                        Ok(frames) => json!({ "url": url, "delay_ms": 1000 / media::GIF_FPS, "frames": frames.iter().map(|f| B64.encode(f)).collect::<Vec<_>>() }),
                        Err(error) => json!({ "url": url, "error": error }),
                    };
                    return emit("waFrames", &payload);
                }
                "wa-msg" | "wa-big" => {
                    let width = if scheme == "wa-big" { 1600 } else { 480 };
                    let shrunk = match bytes {
                        Ok((pic, sticker)) => blocking(move || media::shrink(&pic, sticker, width)).await,
                        Err(e) => Err(e),
                    };
                    match shrunk {
                        Ok(data) => json!({ "url": url, "data": B64.encode(data) }),
                        Err(error) => json!({ "url": url, "error": error }),
                    }
                }
                _ => json!({ "url": url, "error": "unknown media key" }),
            };
            emit("waMedia", &payload);
        });
    }

    /// A message's attachment in full, for Mesh to open (`waFile`).
    fn save_media(&self, id: String) {
        let filename = self.store.find_any(&id).and_then(|(_, m)| m.file.as_ref()?["name"].as_str().map(str::to_owned));
        let filename = filename.unwrap_or_else(|| "file".into());
        let (media, client) = (self.media_of(&id), self.client.clone());
        tokio::spawn(async move {
            let bytes = match media {
                Ok(m) => download(&client, &m).await,
                Err(e) => Err(e),
            };
            emit("waFile", &match bytes {
                Ok(data) => json!({ "id": id, "filename": filename, "data": B64.encode(data) }),
                Err(error) => json!({ "id": id, "error": error }),
            });
        });
    }

    /// A voice note as 48 kHz mono 16-bit PCM, little-endian, for Mesh to play (`waPcm`).
    fn voice_pcm(&self, id: String) {
        let (media, client) = (self.media_of(&id), self.client.clone());
        tokio::spawn(async move {
            let pcm = match media {
                Ok(m) => match download(&client, &m).await {
                    Ok(ogg) => blocking(move || media::voice_pcm(&ogg)).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            emit("waPcm", &match pcm {
                Ok(samples) => json!({ "id": id, "data": B64.encode(samples.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>()) }),
                Err(error) => json!({ "id": id, "error": error }),
            });
        });
    }

    // ---------- calls ----------

    fn on_incoming_call(&mut self, call: &IncomingCall) {
        match &call.action {
            CallAction::Offer { call_id, group_jid, is_video, .. } => {
                if call.offline {
                    info!("ignoring a stale call offer");
                    return;
                }
                if self.call.is_some() {
                    info!("already in a call; leaving {call_id} to ring out");
                    return;
                }
                // A group call shows as its group's.
                let peer = match group_jid {
                    Some(group) => group.to_non_ad().to_string(),
                    None => self.store.canon(&call.from.to_non_ad().to_string()),
                };
                let mut name = self.store.name_of(&peer);
                if group_jid.is_none()
                    && name.starts_with(|c: char| c == '+' || c.is_ascii_digit())
                    && let Some(notify) = call.notify.clone().filter(|n| !n.is_empty())
                {
                    name = notify;
                }
                self.seq += 1;
                let mut info = calls::info(call_id, &peer, name, "incoming");
                (info.video, info.group) = (*is_video, group_jid.is_some());
                self.call = Some(calls::call(self.seq, info, Some(Box::new(call.clone()))));
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

    fn call_start(&mut self, chat_id: &str, video: bool) {
        if self.call.is_some() {
            return emit("waCall", &json!({ "error": "already in a call" }));
        }
        let peer = match chat_id.parse::<Jid>() {
            Ok(jid) => jid,
            Err(e) => return emit("waCall", &json!({ "error": e.to_string() })),
        };
        self.seq += 1;
        let name = self.store.name_of(chat_id);
        let mut info = calls::info("", chat_id, name, "outgoing");
        (info.video, info.group) = (video, peer.is_group());
        let mut call = calls::call(self.seq, info, None);
        let ends = video.then(|| (call.video.attach(), call.video.sink.clone()));
        let keyframe = call.video.keyframe.clone();
        self.call = Some(call);
        if video {
            self.call_video(Some(true), None);
        } else {
            self.emit_call();
        }
        calls::dial(self.client.clone(), peer, ends, keyframe, self.seq, self.calls_tx.clone());
    }

    fn call_accept(&mut self, video: bool) {
        let Some(call) = self.call.as_mut() else {
            return emit("waCall", &json!({ "error": "no call to answer" }));
        };
        let Some(incoming) = call.incoming.take() else { return };
        call.info.phase = "connecting".into();
        call.info.video &= video;
        let ends = call.info.video.then(|| (call.video.attach(), call.video.sink.clone()));
        calls::answer(self.client.clone(), incoming, ends, call.video.keyframe.clone(), call.seq, self.calls_tx.clone());
        if call.info.video {
            self.call_video(Some(true), None);
        } else {
            self.emit_call();
        }
    }

    /// Our camera or screen share on or off. Sharing the screen takes over from the camera (a
    /// call carries one video of ours), and the camera comes back when sharing stops.
    fn call_video(&mut self, camera: Option<bool>, screen: Option<(bool, String)>) {
        let Some(call) = self.call.as_mut() else {
            return emit("waCall", &json!({ "error": "no call" }));
        };
        if let Some(on) = camera {
            call.info.camera = on;
        }
        if let Some((on, id)) = screen {
            call.info.screen = on;
            self.screen_id = id;
        }
        let want = if call.info.screen {
            Some(calls::Source::Screen(self.screen_id.clone()))
        } else if call.info.camera {
            Some(calls::Source::Camera)
        } else {
            None
        };
        if let Err(e) = call.set_video(want, &self.calls_tx) {
            let what = if call.info.screen { "screen share" } else { "camera" };
            emit("waCall", &json!({ "error": format!("{what}: {e}") }));
            if call.info.screen {
                call.info.screen = false;
                // Back to the camera if it was on, else nothing.
                let fallback = call.info.camera.then_some(calls::Source::Camera);
                if call.set_video(fallback, &self.calls_tx).is_err() {
                    call.info.camera = false;
                }
            } else {
                call.info.camera = false;
            }
        }
        self.emit_call();
    }

    /// Their video changed (1:1 only: a group participant's doesn't change the call).
    fn on_peer_video(&mut self, state: VideoState, token: Option<VideoUpgradeToken>) {
        let Some(call) = self.call.as_mut().filter(|c| !c.info.group) else { return };
        match state {
            _ if state.is_upgrade_request() => match token {
                Some(token) => call.accept_peer_video(token, &self.calls_tx),
                None => return,
            },
            VideoState::UpgradeAccept | VideoState::Enabled => call.info.video = true,
            VideoState::Stopped => return emit("waVideo", &json!({ "id": calls::PEER_ID, "ended": true })),
            // They turned down our ask to make it a video call.
            VideoState::UpgradeReject | VideoState::UpgradeRejectByTimeout if !call.info.video => {
                call.video.stop();
                (call.info.camera, call.info.screen) = (false, false);
                emit("waCall", &json!({ "error": "they didn't switch to video" }));
            }
            _ => return,
        }
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
            Update::PeerVideo(seq, state, token) if Some(seq) == current => self.on_peer_video(state, token),
            Update::VideoFailed(seq, error) if Some(seq) == current => {
                if let Some(call) = self.call.as_mut() {
                    call.video.stop();
                    (call.info.camera, call.info.screen) = (false, false);
                }
                emit("waCall", &json!({ "error": format!("video: {error}") }));
                self.emit_call();
            }
            _ => {}
        }
    }
}

/// International digits for what someone typed; a national number (0…) takes our own country
/// code. ponytail: assumes it's as long as ours; type +code otherwise.
fn phone_digits(input: &str, me: &str) -> Option<String> {
    let mut digits: String = input.chars().filter(char::is_ascii_digit).collect();
    if let Some(national) = digits.strip_prefix('0').filter(|_| !input.trim_start().starts_with('+')) {
        let mine = me.split('@').next().unwrap_or_default();
        let code = &mine[..(mine.len() + 1).saturating_sub(digits.len()).min(mine.len())];
        digits = format!("{code}{national}");
    }
    (digits.len() >= 8).then_some(digits)
}

/// Runs CPU work (decoding, resizing) off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// Downloads and decrypts a message's media. Old media expires from WhatsApp's servers (it then
/// lives only on the phone), which shows up as an error here.
async fn download(client: &Client, m: &wa::Message) -> Result<Vec<u8>, String> {
    let item: &dyn Downloadable = if let Some(x) = m.image_message.as_option() {
        x
    } else if let Some(x) = m.video_message.as_option() {
        x
    } else if let Some(x) = m.audio_message.as_option() {
        x
    } else if let Some(x) = m.document_message.as_option() {
        x
    } else if let Some(x) = m.sticker_message.as_option() {
        x
    } else {
        return Err("nothing to download".into());
    };
    client.download(item, Default::default()).await.map_err(|e| format!("download failed: {e}"))
}

/// What Mesh can show in a call (`waSources`): cameras, then screens and windows with thumbnails.
fn list_sources() {
    tokio::task::spawn_blocking(|| {
        let cameras: Vec<_> = mesh_media::list_cameras().into_iter().map(|c| json!({ "id": c.id, "name": c.name })).collect();
        let sources: Vec<_> = mesh_media::list_sources()
            .into_iter()
            .map(|s| json!({ "id": s.id, "name": s.name, "kind": s.kind.as_str(), "thumbnail": B64.encode(&s.thumbnail_jpeg) }))
            .collect();
        emit("waSources", &json!({ "cameras": cameras, "sources": sources }));
    });
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

    #[test]
    fn reads_phone_numbers() {
        let me = "27821234567@s.whatsapp.net";
        assert_eq!(phone_digits("082 765 4321", me).as_deref(), Some("27827654321"));
        assert_eq!(phone_digits("+44 7700 900123", me).as_deref(), Some("447700900123"));
        assert_eq!(phone_digits("12345", me), None);
    }
}
