//! WhatsApp for Mesh, driven through WhatsApp Web running hidden in the
//! engine browser (its own tab, next to Discord's). `whatsapp-agent.js` reads
//! WhatsApp's own collections and calls its own actions; see
//! design/WHATSAPP-NATIVE.md.

use crate::engine;
use serde::Deserialize;
use std::path::PathBuf;
use tokio::sync::mpsc;

const AGENT: &str = include_str!("whatsapp-agent.js");

/// What Mesh hears from WhatsApp Web.
#[derive(Debug, Clone)]
pub enum Event {
    Connected(Engine),
    Snapshot(Snapshot),
    /// The open chat's latest messages (after `Engine::open_chat`, and again
    /// whenever they change).
    Messages(Messages),
    /// A new message from someone else in an unmuted chat.
    Notify(Notify),
    /// A picture asked for with `Engine::fetch_media`: its bytes, or why not.
    Media { url: String, bytes: Result<Vec<u8>, String> },
    /// How an `Engine::send_file` went.
    Sent { chat_id: String, result: Result<(), String> },
    /// A media file decrypted for `Engine::save_media`: its bytes (to write
    /// out and open) with a suggested filename, or why not.
    File { id: String, filename: String, bytes: Result<Vec<u8>, String> },
    /// How an `Engine::forward` went.
    Forwarded(Result<(), String>),
    /// A GIF's frames (JPEGs), asked for with `Engine::fetch_media(&gif_url(id))`.
    Frames { url: String, delay_ms: u64, frames: Result<Vec<Vec<u8>>, String> },
    /// A voice note as 48 kHz mono samples, from `Engine::voice_pcm`.
    Pcm { id: String, samples: Result<Vec<i16>, String> },
    /// Contacts matching an `Engine::search_contacts` query.
    Contacts { query: String, contacts: Vec<Contact> },
    /// The chat `Engine::start_chat` opened, or why not.
    ChatStarted(Result<Chat, String>),
    /// How an archive / unarchive / delete went.
    ChatAction { chat_id: String, result: Result<(), String> },
    /// Not linked: the page's current QR code to scan (a new one every so often).
    Qr(engine::Qr),
    /// The active call changed (`None`: no call), or a call action failed.
    Call(Result<Option<Call>, String>),
    /// A video call frame (their camera or ours), or its end.
    Video(engine::Video),
    /// Lost the tab (closed, not logged in yet); reconnects on its own.
    Disconnected,
}

/// The call WhatsApp has going (one at a time).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Call {
    pub id: String,
    /// The other person's chat id.
    pub peer: String,
    pub name: String,
    /// `incoming`, `outgoing`, `connecting`, `active` or `ended`.
    pub phase: String,
    /// A video call (offered or answered with cameras).
    pub video: bool,
    pub group: bool,
    pub muted: bool,
    /// Our camera is on.
    pub camera: bool,
    /// We're sharing our screen.
    #[serde(default)]
    pub screen: bool,
}

fn flag(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

/// A `waCall` report: the call, `null`, or `{error}` from a call action.
fn call_event(payload: &str) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    if v.is_null() {
        return Some(Event::Call(Ok(None)));
    }
    if let Some(error) = v["error"].as_str() {
        return Some(Event::Call(Err(error.to_owned())));
    }
    serde_json::from_value(v).ok().map(|call| Event::Call(Ok(Some(call))))
}

/// Commands into WhatsApp Web. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::UnboundedSender<engine::Command>,
}

impl Engine {
    /// Shows a chat in Mesh: its messages stream back as `Event::Messages`
    /// until another chat is opened or it's closed.
    pub fn open_chat(&self, chat_id: &str) {
        self.call("open", &[chat_id]);
    }
    pub fn close_chat(&self) {
        self.call("open", &[]);
    }
    /// Unlinks this browser from the phone; the tab reloads to a new QR code.
    pub fn sign_out(&self) {
        self.call("signOut", &[]);
    }
    /// Calls a chat (1:1), with our camera on if `video`.
    pub fn call_start(&self, chat_id: &str, video: bool) {
        self.call("callStart", &[chat_id, flag(video)]);
    }
    /// Answers the ringing call, with our camera on if `video`.
    pub fn call_accept(&self, video: bool) {
        self.call("callAccept", &[flag(video)]);
    }
    pub fn call_decline(&self) {
        self.call("callDecline", &[]);
    }
    pub fn call_end(&self) {
        self.call("callEnd", &[]);
    }
    pub fn call_mute(&self, muted: bool) {
        self.call("callMute", &[flag(muted)]);
    }
    pub fn call_camera(&self, on: bool) {
        self.call("callCamera", &[flag(on)]);
    }
    /// Shares our screen (or a window) in the call, or stops. Sharing brings
    /// WhatsApp's window up, since the browser asks what to share there.
    pub fn call_screen(&self, on: bool) {
        if on {
            let _ = self.commands.send(engine::Command::BringToFront);
        }
        self.call("callScreen", &[flag(on)]);
    }
    /// Sends through WhatsApp Web's own send action; `reply_to` (a message
    /// id) quotes that message.
    pub fn send_message(&self, chat_id: &str, text: &str, reply_to: Option<&str>) {
        self.call("send", &[chat_id, text, reply_to.unwrap_or_default()]);
    }
    /// Decodes a voice note for playing in Mesh; answers with `Event::Pcm`.
    pub fn voice_pcm(&self, message_id: &str) {
        self.call("voicePcm", &[message_id]);
    }
    /// Searches address-book contacts by name or number; answers with
    /// `Event::Contacts`.
    pub fn search_contacts(&self, query: &str) {
        self.call("searchContacts", &[query]);
    }
    /// Opens (or makes) the chat with a contact id or a phone number; answers
    /// with `Event::ChatStarted`.
    pub fn start_chat(&self, contact_or_number: &str) {
        self.call("startChat", &[contact_or_number]);
    }
    /// Archives or unarchives a chat; answers with `Event::ChatAction`.
    pub fn set_archived(&self, chat_id: &str, archived: bool) {
        self.call(if archived { "archive" } else { "unarchive" }, &[chat_id]);
    }
    /// Deletes a chat (for us); answers with `Event::ChatAction`.
    pub fn delete_chat(&self, chat_id: &str) {
        self.call("deleteChat", &[chat_id]);
    }
    /// Reacts to a message; an empty `emoji` takes our reaction back.
    pub fn react(&self, message_id: &str, emoji: &str) {
        self.call("react", &[message_id, emoji]);
    }
    /// Forwards a message to another chat; answers with `Event::Forwarded`.
    pub fn forward(&self, message_id: &str, to_chat_id: &str) {
        self.call("forward", &[message_id, to_chat_id]);
    }
    /// Sends a file (image as a photo, else a document) with an optional
    /// caption, through WhatsApp Web's own attach flow; answers with
    /// `Event::Sent`. `data` is the raw bytes (base64-encoded for the page).
    pub fn send_file(&self, chat_id: &str, filename: &str, content_type: &str, data: &[u8], caption: &str) {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.call("sendFile", &[chat_id, filename, content_type, &b64, caption]);
    }
    /// A profile picture URL, or [`image_url`] for a message's photo
    /// (decrypted and shrunk in the page); answers with `Event::Media`.
    pub fn fetch_media(&self, url: &str) {
        self.call("fetchMedia", &[url]);
    }
    /// Decrypts a message's attachment (video, voice note, document…) in full;
    /// answers with `Event::File`. `id` is the message id from [`file_url`].
    pub fn save_media(&self, id: &str) {
        self.call("saveMedia", &[id]);
    }

    fn call(&self, function: &str, args: &[&str]) {
        let _ = self.commands.send(engine::call(function, args));
    }
}

/// The media key `Engine::fetch_media` takes for a message's photo.
pub fn image_url(message_id: &str) -> String {
    format!("wa-msg:{message_id}")
}

/// The media key for a bigger copy of a message's photo (full screen).
pub fn big_url(message_id: &str) -> String {
    format!("wa-big:{message_id}")
}

/// The media key for a GIF's frames (they answer as `Event::Frames`).
pub fn gif_url(message_id: &str) -> String {
    format!("wa-gif:{message_id}")
}

/// The key for a downloadable attachment: `Engine::save_media` takes the id
/// back out of this (strip the `wa-file:` prefix).
pub fn file_url(message_id: &str) -> String {
    format!("wa-file:{message_id}")
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    /// Our own id.
    pub me: String,
    /// Unarchived chats, most recent first.
    pub chats: Vec<Chat>,
    /// Archived chats, most recent first.
    #[serde(default)]
    pub archived: Vec<Chat>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    /// International, digits only.
    pub number: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Chat {
    pub id: String,
    pub name: String,
    pub group: bool,
    pub unread: u64,
    /// Last activity, ms since the epoch.
    pub last_ms: u64,
    pub muted: bool,
    /// Profile picture URL (fetch it with `Engine::fetch_media`).
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Messages {
    pub chat_id: String,
    /// Oldest first.
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    pub id: String,
    pub author_id: String,
    pub author: String,
    pub avatar: Option<String>,
    /// The text (or caption, or a "[Voice message]"-style label); for a
    /// system message, what happened.
    pub content: String,
    pub system: bool,
    pub timestamp_ms: u64,
    /// Set for a photo; fetch it with `Engine::fetch_media(&image_url(id))`.
    pub image: Option<Image>,
    /// Set for a downloadable attachment (video, voice note, document…);
    /// open it with `Engine::save_media(id)`.
    #[serde(default)]
    pub file: Option<FileMeta>,
    /// What this message replies to.
    #[serde(default)]
    pub quote: Option<Quote>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Quote {
    /// "You" for our own messages.
    pub author: String,
    /// A line of its text, or its kind ("Photo", "Voice note"…).
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reaction {
    /// The emoji.
    pub key: String,
    pub count: u64,
    /// One of them is ours.
    pub mine: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileMeta {
    /// Suggested filename, with extension.
    pub name: String,
    /// "Video", "Voice note", "Audio", "Document".
    pub kind: String,
    pub size: Option<u64>,
    /// Seconds, for audio and video.
    #[serde(default)]
    pub duration: Option<f64>,
    /// A voice note's loudness over time, 0–100 each.
    #[serde(default)]
    pub waveform: Vec<u8>,
    /// A video's small JPEG preview, base64 (see [`FileMeta::thumbnail`]).
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default)]
    pub width: Option<u64>,
    #[serde(default)]
    pub height: Option<u64>,
}

impl FileMeta {
    /// A video's preview JPEG.
    pub fn thumbnail(&self) -> Option<Vec<u8>> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(self.thumb.as_deref()?).ok()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Image {
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// A sticker (fetched as a small transparent PNG) rather than a photo.
    #[serde(default)]
    pub sticker: bool,
    /// A GIF: fetch its frames with `Engine::fetch_media(&gif_url(id))`.
    #[serde(default)]
    pub gif: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Notify {
    pub chat_id: String,
    /// "Alice", or "Alice in Family".
    pub title: String,
    pub body: String,
}

/// Keeps a connection to the WhatsApp Web tab at `socket` for as long as
/// `events` is open (see [`crate::run`] for the address forms).
pub async fn run(socket: PathBuf, events: mpsc::UnboundedSender<Event>) {
    let site = engine::Site {
        host: "web.whatsapp.com",
        agent: AGENT,
        bindings: &["waEmit", "waMessages", "waNotify", "waMedia", "waSent", "waFile", "waForwarded", "waFrames", "waPcm", "waContacts", "waChatStarted", "waChatAction", "waQr", "waCall", "waVideo"],
        kickoff: "window.__mesh && window.__mesh.ready() && window.waEmit(window.__mesh.snapshot())",
    };
    let (tx, mut raw) = mpsc::unbounded_channel();
    tokio::spawn(engine::run(socket, site, tx));
    while let Some(raw) = raw.recv().await {
        let event = match raw {
            engine::Raw::Connected(commands) => Event::Connected(Engine { commands }),
            engine::Raw::Binding { name, payload } => match binding_event(&name, &payload) {
                Some(event) => event,
                None => continue,
            },
            engine::Raw::Disconnected => Event::Disconnected,
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

fn binding_event(name: &str, payload: &str) -> Option<Event> {
    match name {
        "waEmit" => serde_json::from_str(payload).ok().map(Event::Snapshot),
        "waMessages" => serde_json::from_str(payload).ok().map(Event::Messages),
        "waNotify" => serde_json::from_str(payload).ok().map(Event::Notify),
        "waMedia" => engine::media(payload).map(|(url, bytes)| Event::Media { url, bytes }),
        "waSent" => engine::sent(payload).map(|(chat_id, result)| Event::Sent { chat_id, result }),
        "waFile" => file(payload),
        "waFrames" => frames(payload),
        "waPcm" => pcm(payload),
        "waContacts" => {
            #[derive(Deserialize)]
            struct Found {
                query: String,
                contacts: Vec<Contact>,
            }
            serde_json::from_str::<Found>(payload).ok().map(|f| Event::Contacts { query: f.query, contacts: f.contacts })
        }
        "waChatStarted" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::ChatStarted(match serde_json::from_value::<Chat>(v["chat"].clone()) {
                Ok(chat) => Ok(chat),
                Err(_) => Err(v["error"].as_str().unwrap_or("couldn't open that chat").to_owned()),
            }))
        }
        "waChatAction" => engine::sent(payload).map(|(chat_id, result)| Event::ChatAction { chat_id, result }),
        "waForwarded" => engine::sent(payload).map(|(_, result)| Event::Forwarded(result)),
        "waQr" => engine::qr(payload).map(Event::Qr),
        "waCall" => call_event(payload),
        "waVideo" => engine::video(payload).map(Event::Video),
        _ => None,
    }
}

/// A GIF's frames: `{url, delay_ms, frames: [base64 JPEG]}` or `{url, error}`.
fn frames(payload: &str) -> Option<Event> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let url = v["url"].as_str()?.to_owned();
    let frames = match v["frames"].as_array() {
        Some(list) => list
            .iter()
            .map(|f| base64::engine::general_purpose::STANDARD.decode(f.as_str().unwrap_or_default()).map_err(|e| e.to_string()))
            .collect(),
        None => Err(v["error"].as_str().unwrap_or("no frames").to_owned()),
    };
    Some(Event::Frames { url, delay_ms: v["delay_ms"].as_u64().unwrap_or(83), frames })
}

/// A voice note's samples: little-endian `i16`s, base64 in `data`.
fn pcm(payload: &str) -> Option<Event> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = v["id"].as_str()?.to_owned();
    let samples = match v["data"].as_str() {
        Some(data) => base64::engine::general_purpose::STANDARD
            .decode(data)
            .map(|bytes| bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
            .map_err(|e| e.to_string()),
        None => Err(v["error"].as_str().unwrap_or("couldn't decode it").to_owned()),
    };
    Some(Event::Pcm { id, samples })
}

/// A `saveMedia` result: the message id, a suggested filename and the bytes
/// (base64 in the payload), or why not.
fn file(payload: &str) -> Option<Event> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = v["id"].as_str()?.to_owned();
    let filename = v["filename"].as_str().unwrap_or("file").to_owned();
    let bytes = match v["data"].as_str() {
        Some(data) => base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string()),
        None => Err(v["error"].as_str().unwrap_or("download failed").to_owned()),
    };
    Some(Event::File { id, filename, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agent_reports() {
        let snapshot = r#"{"me":"1@c.us","chats":[{"id":"2@g.us","name":"Family","group":true,"unread":3,"last_ms":1792491279000,"muted":false,"avatar":null}]}"#;
        let Some(Event::Snapshot(s)) = binding_event("waEmit", snapshot) else { panic!("no snapshot") };
        assert_eq!((s.chats[0].name.as_str(), s.chats[0].unread, s.chats[0].group), ("Family", 3, true));

        let messages = r#"{"chat_id":"2@g.us","messages":[{"id":"m1","author_id":"3@c.us","author":"Alice","avatar":null,"content":"hi","system":false,"timestamp_ms":1,"image":{"width":960,"height":1280}}]}"#;
        let Some(Event::Messages(m)) = binding_event("waMessages", messages) else { panic!("no messages") };
        assert_eq!(m.messages[0].image.as_ref().and_then(|i| i.width), Some(960));

        let Some(Event::Media { url, bytes }) = binding_event("waMedia", r#"{"url":"wa-msg:m1","data":"aGk="}"#) else {
            panic!("no media")
        };
        assert_eq!((url, bytes), (image_url("m1"), Ok(b"hi".to_vec())));
        assert!(binding_event("meshEmit", "{}").is_none());

        // 1, -2 as little-endian i16s.
        let Some(Event::Pcm { samples, .. }) = binding_event("waPcm", r#"{"id":"m1","data":"AQD+/w=="}"#) else { panic!("no pcm") };
        assert_eq!(samples, Ok(vec![1, -2]));
        let Some(Event::Frames { frames, delay_ms, .. }) =
            binding_event("waFrames", r#"{"url":"wa-gif:m1","delay_ms":83,"frames":["aGk=","aGk="]}"#)
        else {
            panic!("no frames")
        };
        assert_eq!((delay_ms, frames.map(|f| f.len())), (83, Ok(2)));

        let Some(Event::ChatStarted(Err(why))) = binding_event("waChatStarted", r#"{"error":"+1 isn't on WhatsApp"}"#) else {
            panic!("no chat error")
        };
        assert_eq!(why, "+1 isn't on WhatsApp");
        let started = r#"{"chat":{"id":"2@c.us","name":"Bo","group":false,"unread":0,"last_ms":0,"muted":false,"avatar":null}}"#;
        let Some(Event::ChatStarted(Ok(chat))) = binding_event("waChatStarted", started) else { panic!("no chat") };
        assert_eq!(chat.name, "Bo");
    }

    #[test]
    fn reads_call_reports() {
        let call = r#"{"id":"C1","peer":"27821234567@c.us","name":"Ruan","phase":"incoming","video":true,"group":false,"muted":false,"camera":false}"#;
        let Some(Event::Call(Ok(Some(c)))) = binding_event("waCall", call) else { panic!("no call") };
        assert_eq!((c.name.as_str(), c.phase.as_str(), c.video), ("Ruan", "incoming", true));
        assert!(matches!(binding_event("waCall", "null"), Some(Event::Call(Ok(None)))));
        assert!(matches!(binding_event("waCall", r#"{"error":"couldn't answer: no"}"#), Some(Event::Call(Err(e))) if e.contains("answer")));
    }
}
