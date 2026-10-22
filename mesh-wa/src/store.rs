//! Chats and their recent messages, kept in memory and saved as one JSON file.
//! Field names match Mesh's `mesh-discord/src/whatsapp.rs` structs.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use whatsapp_rust::prelude::*;

const KEEP_PER_CHAT: usize = 200;
const SHOWN: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub author: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub author_id: String,
    pub author: String,
    pub avatar: Option<String>,
    pub content: String,
    pub system: bool,
    pub timestamp_ms: u64,
    // Always null / empty for now; kept so the shape matches Mesh's.
    pub image: Option<serde_json::Value>,
    pub file: Option<serde_json::Value>,
    pub quote: Option<Quote>,
    pub reactions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Chat {
    pub id: String,
    pub name: String,
    pub group: bool,
    pub unread: u64,
    pub last_ms: u64,
    pub muted: bool,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub me: String,
    pub chats: Vec<Chat>,
    pub archived: Vec<Chat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub number: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatData {
    /// Group subject or the name history sync gave the chat; may be empty.
    pub name: String,
    pub unread: u64,
    pub last_ms: u64,
    pub muted: bool,
    pub archived: bool,
    pub messages: Vec<Message>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub me: String,
    pub chats: HashMap<String, ChatData>,
    /// Address-book names, by jid.
    pub contacts: HashMap<String, String>,
    /// Push names (what people call themselves), by jid.
    pub push: HashMap<String, String>,
    /// `<lid>@lid` -> `<number>@s.whatsapp.net`, so one person is one chat.
    pub lid_pn: HashMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl Store {
    pub fn load(path: PathBuf) -> Store {
        let mut store: Store = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        store.path = path;
        store
    }

    /// Write temp + rename, so a crash never leaves half a file.
    pub fn save(&self) {
        let tmp = self.path.with_extension("json.tmp");
        let result = serde_json::to_vec(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = result {
            log::warn!("saving chats failed: {e}");
        }
    }

    /// The jid one person's chat is filed under (their number when we know it).
    pub fn canon(&self, jid: &str) -> String {
        self.lid_pn.get(jid).cloned().unwrap_or_else(|| jid.to_owned())
    }

    pub fn map_lid(&mut self, lid: &str, pn: &str) {
        if lid.ends_with("@lid") && pn.ends_with("@s.whatsapp.net") {
            self.lid_pn.insert(lid.to_owned(), pn.to_owned());
        }
    }

    pub fn chat(&mut self, id: &str) -> &mut ChatData {
        self.chats.entry(id.to_owned()).or_default()
    }

    /// Who `jid` is: address book, then group subject / chat name, then push name, then number.
    pub fn name_of(&self, jid: &str) -> String {
        let jid = self.canon(jid);
        if jid == self.me {
            return "You".into();
        }
        let named = |s: Option<&String>| s.filter(|s| !s.is_empty()).cloned();
        named(self.contacts.get(&jid))
            .or_else(|| named(self.chats.get(&jid).map(|c| &c.name)))
            .or_else(|| named(self.push.get(&jid)))
            .unwrap_or_else(|| number_label(&jid))
    }

    /// Adds a message unless it's already there (by id). Returns whether it was new.
    pub fn add(&mut self, chat_id: &str, msg: Message) -> bool {
        let chat = self.chat(chat_id);
        if chat.messages.iter().any(|m| m.id == msg.id) {
            return false;
        }
        chat.last_ms = chat.last_ms.max(msg.timestamp_ms);
        let at = chat.messages.partition_point(|m| m.timestamp_ms <= msg.timestamp_ms);
        chat.messages.insert(at, msg);
        if chat.messages.len() > KEEP_PER_CHAT {
            let extra = chat.messages.len() - KEEP_PER_CHAT;
            chat.messages.drain(..extra);
        }
        true
    }

    pub fn find(&self, chat_id: &str, msg_id: &str) -> Option<&Message> {
        self.chats.get(chat_id)?.messages.iter().find(|m| m.id == msg_id)
    }

    /// The last messages of a chat, oldest first, with authors named as we know them now.
    pub fn recent(&self, chat_id: &str) -> Vec<Message> {
        let msgs = self.chats.get(chat_id).map(|c| c.messages.as_slice()).unwrap_or_default();
        let mut recent = msgs[msgs.len().saturating_sub(SHOWN)..].to_vec();
        for m in recent.iter_mut().filter(|m| !m.author_id.is_empty()) {
            m.author = self.name_of(&m.author_id);
        }
        recent
    }

    pub fn chat_view(&self, id: &str) -> Chat {
        let data = self.chats.get(id).cloned().unwrap_or_default();
        Chat {
            id: id.to_owned(),
            name: self.name_of(id),
            group: id.ends_with("@g.us"),
            unread: data.unread,
            last_ms: data.last_ms,
            muted: data.muted,
            avatar: None,
        }
    }

    /// Unarchived and archived chats, most recent first.
    pub fn snapshot(&self) -> Snapshot {
        let mut snap = Snapshot { me: self.me.clone(), ..Default::default() };
        for (id, data) in &self.chats {
            let list = if data.archived { &mut snap.archived } else { &mut snap.chats };
            list.push(self.chat_view(id));
        }
        for list in [&mut snap.chats, &mut snap.archived] {
            list.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.name.cmp(&b.name)));
        }
        snap
    }

    /// People whose name or number contains `query`.
    pub fn search(&self, query: &str) -> Vec<Contact> {
        let q = query.trim().to_lowercase();
        let digits: String = q.chars().filter(char::is_ascii_digit).collect();
        let mut ids: Vec<&String> = self.contacts.keys().chain(self.push.keys()).chain(self.chats.keys()).collect();
        ids.sort();
        ids.dedup();
        ids.into_iter()
            .filter(|id| id.ends_with("@s.whatsapp.net") && **id != self.me)
            .map(|id| Contact { id: id.clone(), name: self.name_of(id), number: id.split('@').next().unwrap_or_default().to_owned() })
            .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || (!digits.is_empty() && c.number.contains(&digits)))
            .take(30)
            .collect()
    }
}

/// `+27821234567` for a phone jid, the bare user part otherwise.
pub fn number_label(jid: &str) -> String {
    let user = jid.split('@').next().unwrap_or(jid);
    if jid.ends_with("@s.whatsapp.net") { format!("+{user}") } else { user.to_owned() }
}

/// What to show for a message, or `None` for ones that aren't shown (reactions, receipts,
/// protocol messages, key distribution).
pub fn content(msg: &wa::Message) -> Option<String> {
    let base = msg.get_base_message();
    if let Some(text) = base.text_content() {
        return Some(text.to_owned());
    }
    let caption = |label: &str, caption: Option<&str>| match caption.filter(|c| !c.is_empty()) {
        Some(c) => format!("{label} {c}"),
        None => label.to_owned(),
    };
    if let Some(m) = base.image_message.as_option() {
        return Some(caption("[Photo]", m.caption.as_deref()));
    }
    if let Some(m) = base.video_message.as_option().or(base.ptv_message.as_option()) {
        return Some(caption("[Video]", m.caption.as_deref()));
    }
    if let Some(m) = base.audio_message.as_option() {
        return Some(if m.ptt == Some(true) { "[Voice message]" } else { "[Audio]" }.into());
    }
    if let Some(m) = base.document_message.as_option() {
        let name = m.file_name.as_deref().or(m.title.as_deref()).unwrap_or_default();
        let label = format!("[Document] {name}");
        return Some(match m.caption.as_deref().filter(|c| !c.is_empty()) {
            Some(c) => format!("{label}\n{c}"),
            None => label.trim_end().to_owned(),
        });
    }
    if base.sticker_message.is_set() {
        return Some("[Sticker]".into());
    }
    if base.location_message.is_set() || base.live_location_message.is_set() {
        return Some("[Location]".into());
    }
    if let Some(m) = base.contact_message.as_option() {
        return Some(caption("[Contact]", m.display_name.as_deref()));
    }
    if base.contacts_array_message.is_set() {
        return Some("[Contacts]".into());
    }
    if let Some(m) = base.poll_creation_message.as_option().or(base.poll_creation_message_v3.as_option()) {
        return Some(caption("[Poll]", m.name.as_deref()));
    }
    None
}

/// The quoted message's author jid and text, if `msg` is a reply.
pub fn quoted(msg: &wa::Message) -> Option<(String, String)> {
    let ctx = msg.get_base_message().context_info()?;
    ctx.stanza_id.as_ref()?;
    let text = ctx.quoted_message.as_option().and_then(content).unwrap_or_default();
    Some((ctx.participant.clone().unwrap_or_default(), text))
}
