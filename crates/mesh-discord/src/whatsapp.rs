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
    /// Lost the tab (closed, not logged in yet); reconnects on its own.
    Disconnected,
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
    /// Sends through WhatsApp Web's own send action.
    pub fn send_message(&self, chat_id: &str, text: &str) {
        self.call("send", &[chat_id, text]);
    }
    /// A profile picture URL, or [`image_url`] for a message's photo
    /// (decrypted and shrunk in the page); answers with `Event::Media`.
    pub fn fetch_media(&self, url: &str) {
        self.call("fetchMedia", &[url]);
    }

    fn call(&self, function: &str, args: &[&str]) {
        let _ = self.commands.send(engine::call(function, args));
    }
}

/// The media key `Engine::fetch_media` takes for a message's photo.
pub fn image_url(message_id: &str) -> String {
    format!("wa-msg:{message_id}")
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    /// Our own id.
    pub me: String,
    /// Unarchived chats, most recent first.
    pub chats: Vec<Chat>,
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
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Image {
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// A sticker (fetched as a small transparent PNG) rather than a photo.
    #[serde(default)]
    pub sticker: bool,
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
        bindings: &["waEmit", "waMessages", "waNotify", "waMedia"],
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
        _ => None,
    }
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
    }
}
