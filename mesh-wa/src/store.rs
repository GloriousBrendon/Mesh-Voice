//! Chats and their recent messages, kept in memory and saved as one JSON file.
//! Field names match Mesh's `mesh-discord/src/whatsapp.rs` structs.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use whatsapp_rust::buffa::{Message as _, MessageField};
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
    /// Mesh's `Image` (photo, sticker, GIF).
    pub image: Option<Value>,
    /// Mesh's `FileMeta` (video, voice note, audio, document).
    pub file: Option<Value>,
    pub quote: Option<Quote>,
    /// Forwarded from another chat (WhatsApp's "Forwarded" label).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forwarded: bool,
    /// Filled from `reacts` when shown.
    pub reactions: Vec<Value>,
    /// The media part of the message, protobuf as base64, for downloading or forwarding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// Reactions: sender jid -> emoji.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reacts: BTreeMap<String, String>,
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
    /// Profile picture URLs, by jid. Not saved: they expire, so each run asks again.
    #[serde(skip)]
    pub avatars: HashMap<String, String>,
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

    /// A message and its chat, by message id alone (Mesh's media keys carry only that).
    // ponytail: scans every chat; an id -> chat index if that ever shows up in a profile.
    pub fn find_any(&self, msg_id: &str) -> Option<(&str, &Message)> {
        self.chats.iter().find_map(|(id, c)| c.messages.iter().find(|m| m.id == msg_id).map(|m| (id.as_str(), m)))
    }

    /// Sets (or, with an empty emoji, takes back) `who`'s reaction. Returns whether the message is here.
    pub fn react(&mut self, chat_id: &str, msg_id: &str, who: &str, emoji: &str) -> bool {
        let Some(m) = self.chats.get_mut(chat_id).and_then(|c| c.messages.iter_mut().find(|m| m.id == msg_id)) else {
            return false;
        };
        if emoji.is_empty() {
            m.reacts.remove(who);
        } else {
            m.reacts.insert(who.to_owned(), emoji.to_owned());
        }
        true
    }

    /// The last messages of a chat, oldest first, with authors named as we know them now.
    pub fn recent(&self, chat_id: &str) -> Vec<Message> {
        let msgs = self.chats.get(chat_id).map(|c| c.messages.as_slice()).unwrap_or_default();
        let mut recent = msgs[msgs.len().saturating_sub(SHOWN)..].to_vec();
        for m in recent.iter_mut() {
            if !m.author_id.is_empty() {
                m.author = self.name_of(&m.author_id);
                m.avatar = self.avatars.get(&m.author_id).cloned();
            }
            m.reactions = pills(&m.reacts, &self.me);
            m.media = None;
            m.reacts.clear();
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
            avatar: self.avatars.get(id).cloned(),
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
        // A national number's leading 0 stands in for the country code.
        let digits: String = q.chars().filter(char::is_ascii_digit).collect();
        let digits = digits.strip_prefix('0').unwrap_or(&digits);
        let mut ids: Vec<&String> = self.contacts.keys().chain(self.push.keys()).chain(self.chats.keys()).collect();
        ids.sort();
        ids.dedup();
        ids.into_iter()
            .filter(|id| id.ends_with("@s.whatsapp.net") && **id != self.me)
            .map(|id| Contact { id: id.clone(), name: self.name_of(id), number: id.split('@').next().unwrap_or_default().to_owned() })
            .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || (digits.len() >= 3 && c.number.contains(digits)))
            .take(30)
            .collect()
    }
}

/// `+27821234567` for a phone jid, the bare user part otherwise.
pub fn number_label(jid: &str) -> String {
    let user = jid.split('@').next().unwrap_or(jid);
    if jid.ends_with("@s.whatsapp.net") { format!("+{user}") } else { user.to_owned() }
}

/// Reaction pills, one per emoji: `{key, count, mine}`.
fn pills(reacts: &BTreeMap<String, String>, me: &str) -> Vec<Value> {
    let mut pills: Vec<(&str, u64, bool)> = Vec::new();
    for (who, emoji) in reacts {
        match pills.iter_mut().find(|p| p.0 == emoji) {
            Some(p) => {
                p.1 += 1;
                p.2 |= who == me;
            }
            None => pills.push((emoji, 1, who == me)),
        }
    }
    pills.into_iter().map(|(key, count, mine)| json!({ "key": key, "count": count, "mine": mine })).collect()
}

/// What Mesh shows for a message.
#[derive(Debug, Default, PartialEq)]
pub struct Body {
    /// The text, caption, or a "[Location]"-style label.
    pub content: String,
    pub image: Option<Value>,
    pub file: Option<Value>,
    /// See [`Message::media`].
    pub media: Option<String>,
    /// "Photo", "Voice note"…, for quoting or notifying a captionless one.
    pub kind: &'static str,
}

impl Body {
    /// The content, or the kind when there's no text.
    pub fn line(&self) -> String {
        if self.content.is_empty() { self.kind.to_owned() } else { self.content.clone() }
    }
}

/// `voice-note.ogg`, `video.mp4`: a name for a media file without one.
fn file_name(kind: &str, mimetype: Option<&str>) -> String {
    let mime = mimetype.unwrap_or_default().split(';').next().unwrap_or_default();
    let ext = mime.split('/').nth(1).map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| "bin".into());
    format!("{}.{ext}", kind.to_lowercase().replace(' ', "-"))
}

/// Mesh's `FileMeta`.
struct FileMeta<'a> {
    name: String,
    kind: &'a str,
    size: Option<u64>,
    seconds: Option<u32>,
    waveform: &'a [u8],
    thumb: Option<&'a [u8]>,
    width: Option<u32>,
    height: Option<u32>,
}

impl FileMeta<'_> {
    fn new(name: String, kind: &str, size: Option<u64>) -> FileMeta<'_> {
        FileMeta { name, kind, size, seconds: None, waveform: &[], thumb: None, width: None, height: None }
    }

    fn json(self) -> Value {
        json!({
            "name": self.name, "kind": self.kind, "size": self.size, "duration": self.seconds.map(f64::from),
            "waveform": self.waveform,
            "thumb": self.thumb.filter(|t| t.starts_with(&[0xff, 0xd8])).map(|t| B64.encode(t)),
            "width": self.width, "height": self.height,
        })
    }
}

/// What to show for a message, or `None` for ones that aren't shown (reactions, receipts,
/// protocol messages, key distribution).
pub fn body(msg: &wa::Message) -> Option<Body> {
    let base = msg.get_base_message();
    if let Some(text) = base.text_content() {
        return Some(Body { content: text.to_owned(), ..Default::default() });
    }
    let text = |s: &Option<String>| s.clone().unwrap_or_default();
    let label = |label: &str, extra: Option<&str>| match extra.filter(|c| !c.is_empty()) {
        Some(c) => format!("[{label}] {c}"),
        None => format!("[{label}]"),
    };
    // The media part kept for later, without thumbnails or the reply context.
    let mut part = wa::Message::default();
    let body = if let Some(m) = base.image_message.as_option() {
        let mut keep = m.clone();
        (keep.jpeg_thumbnail, keep.context_info) = (None, MessageField::none());
        part.image_message = MessageField::some(keep);
        let image = json!({ "width": m.width, "height": m.height, "sticker": false });
        Body { content: text(&m.caption), image: Some(image), kind: "Photo", ..Default::default() }
    } else if let Some(m) = base.sticker_message.as_option() {
        let mut keep = m.clone();
        (keep.png_thumbnail, keep.context_info) = (None, MessageField::none());
        part.sticker_message = MessageField::some(keep);
        let image = json!({ "width": m.width.unwrap_or(512), "height": m.height.unwrap_or(512), "sticker": true });
        Body { image: Some(image), kind: "Sticker", ..Default::default() }
    } else if let Some(m) = base.video_message.as_option().or(base.ptv_message.as_option()) {
        let mut keep = m.clone();
        (keep.jpeg_thumbnail, keep.context_info) = (None, MessageField::none());
        part.video_message = MessageField::some(keep);
        if m.gif_playback == Some(true) {
            // A GIF is a short silent mp4: Mesh loops its frames inline.
            let image = json!({ "width": m.width, "height": m.height, "sticker": false, "gif": true });
            Body { content: text(&m.caption), image: Some(image), kind: "GIF", ..Default::default() }
        } else {
            let file = FileMeta {
                seconds: m.seconds,
                thumb: m.jpeg_thumbnail.as_deref(),
                width: m.width,
                height: m.height,
                ..FileMeta::new(file_name("Video", m.mimetype.as_deref()), "Video", m.file_length)
            };
            Body { content: text(&m.caption), file: Some(file.json()), kind: "Video", ..Default::default() }
        }
    } else if let Some(m) = base.audio_message.as_option() {
        let mut keep = m.clone();
        keep.context_info = MessageField::none();
        part.audio_message = MessageField::some(keep);
        let kind = if m.ptt == Some(true) { "Voice note" } else { "Audio" };
        let file = FileMeta {
            seconds: m.seconds,
            waveform: m.waveform.as_deref().unwrap_or_default(),
            ..FileMeta::new(file_name(kind, m.mimetype.as_deref()), kind, m.file_length)
        };
        Body { file: Some(file.json()), kind, ..Default::default() }
    } else if let Some(m) = base.document_message.as_option() {
        let mut keep = m.clone();
        (keep.jpeg_thumbnail, keep.context_info) = (None, MessageField::none());
        part.document_message = MessageField::some(keep);
        let name = m.file_name.clone().or_else(|| m.title.clone()).filter(|n| !n.is_empty());
        let name = name.unwrap_or_else(|| file_name("Document", m.mimetype.as_deref()));
        let file = FileMeta::new(name, "Document", m.file_length);
        Body { content: text(&m.caption), file: Some(file.json()), kind: "Document", ..Default::default() }
    } else {
        let content = if base.location_message.is_set() || base.live_location_message.is_set() {
            "[Location]".into()
        } else if let Some(m) = base.contact_message.as_option() {
            label("Contact", m.display_name.as_deref())
        } else if base.contacts_array_message.is_set() {
            "[Contacts]".into()
        } else if let Some(m) = base.poll_creation_message.as_option().or(base.poll_creation_message_v3.as_option()) {
            label("Poll", m.name.as_deref())
        } else {
            return None;
        };
        return Some(Body { content, ..Default::default() });
    };
    Some(Body { media: Some(B64.encode(part.encode_to_vec())), ..body })
}

/// A message's kept media part back (see [`Message::media`]).
pub fn media(m: &Message) -> Option<wa::Message> {
    let bytes = B64.decode(m.media.as_deref()?).ok()?;
    wa::Message::decode_from_slice(&bytes).ok()
}

/// Whether `msg` was forwarded from another chat.
pub fn forwarded(msg: &wa::Message) -> bool {
    msg.get_base_message().context_info().is_some_and(|c| c.is_forwarded == Some(true))
}

/// The quoted message's author jid and text, if `msg` is a reply.
pub fn quoted(msg: &wa::Message) -> Option<(String, String)> {
    let ctx = msg.get_base_message().context_info()?;
    ctx.stanza_id.as_ref()?;
    let text = ctx.quoted_message.as_option().and_then(body).map(|b| b.line()).unwrap_or_default();
    Some((ctx.participant.clone().unwrap_or_default(), text.chars().take(200).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_messages_carry_meshes_metadata() {
        let mut ptt = wa::message::AudioMessage::default();
        (ptt.ptt, ptt.seconds, ptt.waveform) = (Some(true), Some(7), Some(vec![0, 50, 100]));
        (ptt.mimetype, ptt.direct_path) = (Some("audio/ogg; codecs=opus".into()), Some("/v/t62/x".into()));
        let mut msg = wa::Message::default();
        msg.audio_message = MessageField::some(ptt);
        let b = body(&msg).unwrap();
        assert_eq!((b.content.as_str(), b.kind, b.line().as_str()), ("", "Voice note", "Voice note"));
        let file = b.file.as_ref().unwrap();
        assert_eq!((file["name"].as_str(), file["duration"].as_f64(), file["waveform"][1].as_u64()), (Some("voice-note.ogg"), Some(7.0), Some(50)));
        // The kept part decodes back, ready to download.
        let kept = media(&Message { media: b.media, ..Default::default() }).unwrap();
        assert_eq!(kept.audio_message.as_option().and_then(|a| a.direct_path.as_deref()), Some("/v/t62/x"));

        let mut gif = wa::message::VideoMessage::default();
        (gif.gif_playback, gif.width, gif.caption) = (Some(true), Some(320), Some("lol".into()));
        let mut msg = wa::Message::default();
        msg.video_message = MessageField::some(gif);
        let b = body(&msg).unwrap();
        let image = b.image.as_ref().unwrap();
        assert_eq!((b.content.as_str(), image["gif"].as_bool(), image["width"].as_u64()), ("lol", Some(true), Some(320)));
        assert_eq!(file_name("Document", None), "document.bin");
    }

    #[test]
    fn reactions_group_by_emoji() {
        let mut store = Store { me: "1@s.whatsapp.net".into(), ..Default::default() };
        store.add("2@g.us", Message { id: "m".into(), ..Default::default() });
        for (who, emoji) in [("1@s.whatsapp.net", "👍"), ("3@s.whatsapp.net", "👍"), ("4@s.whatsapp.net", "😂")] {
            assert!(store.react("2@g.us", "m", who, emoji));
        }
        assert!(!store.react("9@g.us", "m", "4@s.whatsapp.net", ""));
        store.react("2@g.us", "m", "4@s.whatsapp.net", "");
        let shown = &store.recent("2@g.us")[0];
        assert_eq!(shown.reactions, vec![json!({ "key": "👍", "count": 2, "mine": true })]);
        assert!(shown.reacts.is_empty() && shown.media.is_none());
        assert_eq!(store.find_any("m").map(|(chat, _)| chat), Some("2@g.us"));
    }
}
