//! Mesh's own chats (SPEC-dm *Mesh-Voice*) on WhatsApp's path: the chats sit
//! in the sidebar with the DMs, open in the timeline and composer, and their
//! actions go to `mesh-node`. Message text and IDs are never logged.

use super::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// How long "alice is typing…" stays after the last signal (the helper sends
/// one at most every 2 s while someone types).
pub(crate) const TYPING_SHOWN: Duration = Duration::from_secs(5);

/// A chat as the chat pane opens it.
pub(crate) fn web_chat(chat: &mesh::Chat) -> WebChat {
    WebChat {
        network: Network::Mesh,
        id: chat.id.clone(),
        name: chat.name.clone(),
        dm: chat.kind == mesh::ChatKind::Direct,
    }
}

/// Who's typing in `chat` now, by name.
pub(crate) fn typing<'a>(state: &'a App, chat: &str) -> &'a [String] {
    match &state.mesh.typing {
        Some((id, names, at)) if id == chat && at.elapsed() < TYPING_SHOWN => names,
        _ => &[],
    }
}

/// What the timeline needs from a page: its rows, which are ours, and each
/// row's small mark ("sending", "edited").
pub(crate) struct Rows {
    pub timeline: Vec<TimelineMessage>,
    pub mine: HashSet<String>,
    pub marks: HashMap<String, &'static str>,
}

/// A page as timeline rows. `name` gives a person's name from their ID.
pub(crate) fn rows(page: &mesh::Page, me: &str, name: impl Fn(&str) -> String) -> Rows {
    let mut rows = Rows {
        timeline: Vec::new(),
        mine: HashSet::new(),
        marks: HashMap::new(),
    };
    for m in &page.events {
        // ponytail: made-up `@<id>:mesh` user ids, as WhatsApp's `@wa_<number>`; Mesh IDs are lowercase bech32, so valid.
        let Ok(sender) = OwnedUserId::try_from(format!("@{}:mesh", m.from)) else {
            continue;
        };
        let quote = m.reply_to.as_ref().map(|target| match page.events.iter().find(|e| &e.id == target) {
            Some(e) => mesh_core::Quote {
                author: name(&e.from),
                text: e.text.chars().take(200).collect(),
            },
            None => mesh_core::Quote {
                author: String::new(),
                text: "an earlier message".to_string(),
            },
        });
        // ponytail: files show as a line until PLAN-dm Task 18 opens them.
        let files = m.attachments.iter().map(|a| format!("📎 {} · {}", a.name, human_size(a.size)));
        let body = files.chain((!m.text.is_empty()).then(|| m.text.clone())).collect::<Vec<_>>().join("\n");
        let reactions = (m.reactions.iter())
            .map(|r| mesh_core::Reaction {
                key: r.emoji.clone(),
                count: r.who.len(),
                mine: None,
                by_me: r.who.iter().any(|w| w == me),
            })
            .collect();
        rows.timeline.push(TimelineMessage {
            event_id: None,
            sender,
            sender_name: name(&m.from),
            sender_avatar: None,
            sender_network: Network::Mesh,
            body,
            kind: MessageKind::Text,
            reactions,
            timestamp_ms: m.time,
            web_id: Some(m.id.clone()),
            reply_to: quote,
            forwarded: false,
        });
        let ours = m.from == me;
        if ours {
            rows.mine.insert(m.id.clone());
        }
        let mark = match (ours && !m.sent, m.edited) {
            (true, true) => Some("sending · edited"),
            (true, false) => Some("sending"),
            (false, true) => Some("edited"),
            (false, false) => None,
        };
        if let Some(mark) = mark {
            rows.marks.insert(m.id.clone(), mark);
        }
    }
    rows
}

/// The dm events (the rest go through `mesh_ui::event`).
pub(crate) fn event(state: &mut App, event: mesh::Event) -> Task<Message> {
    let open = |state: &App| match &state.screen {
        Screen::LoggedIn(session) => session.web_chat.as_ref().filter(|c| c.network == Network::Mesh).map(|c| c.id.clone()),
        _ => None,
    };
    match event {
        mesh::Event::DmChats(chats) => {
            state.mesh.chats = chats;
            // After an unlock or a helper restart, the open chat's page again.
            let empty = matches!(&state.screen, Screen::LoggedIn(session) if session.timeline.is_empty());
            if let (Some(id), Some(node), true) = (open(state), &state.mesh.node, empty) {
                node.dm_open(&id, None);
            }
        }
        mesh::Event::DmMessages(page) => {
            if open(state).as_deref() != Some(page.id.as_str()) {
                return Task::none();
            }
            let me = state.mesh.my_id().unwrap_or_default();
            let Rows { timeline, mine, marks } = rows(&page, &me, |id| state.mesh.name_of(id));
            // Seen while looking at it: read up to the newest.
            let unread = state.mesh.chats.iter().any(|c| c.id == page.id && c.unread > 0);
            if state.window_focused
                && unread
                && let (Some(node), Some(last)) = (&state.mesh.node, page.events.last())
            {
                node.dm_read(&page.id, &last.id);
            }
            if let Screen::LoggedIn(session) = &mut state.screen {
                (session.timeline, session.mesh_mine, session.web_marks) = (timeline, mine, marks);
            }
        }
        mesh::Event::DmNotify(n) => {
            if !(state.window_focused && open(state).as_deref() == Some(n.id.as_str())) {
                let from = state.mesh.name_of(&n.from);
                let title = match state.mesh.chats.iter().find(|c| c.id == n.id) {
                    Some(chat) if chat.kind == mesh::ChatKind::Group => format!("{from} in {}", chat.name),
                    _ => from,
                };
                sounds::play(sounds::Sound::Message(Network::Mesh), &state.sounds, speaker(state));
                notify(title, n.text);
            }
        }
        mesh::Event::DmTyping { id, who } => {
            let names = who.iter().map(|w| state.mesh.name_of(w)).collect();
            state.mesh.typing = Some((id, names, Instant::now()));
        }
        // ponytail: opening files comes with PLAN-dm Task 18.
        mesh::Event::DmFile(_) => {}
        _ => {}
    }
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, from: &str, text: &str) -> mesh::DmMessage {
        mesh::DmMessage {
            id: id.into(),
            from: from.into(),
            text: text.into(),
            edited: false,
            reply_to: None,
            attachments: Vec::new(),
            reactions: Vec::new(),
            late: false,
            time: 1_794_428_384_000,
            sent: false,
        }
    }

    /// A page maps onto timeline rows: names, replies, reactions, files, and
    /// the "sending" and "edited" marks on the right rows.
    #[test]
    fn a_page_as_rows() {
        let mut sent = message("02", "mesh1me", "on my way");
        sent.sent = true;
        sent.edited = true;
        sent.reply_to = Some("01".into());
        sent.reactions = vec![mesh::Reaction {
            emoji: "🎉".into(),
            who: vec!["mesh1bob".into(), "mesh1me".into()],
        }];
        let mut file = message("03", "mesh1me", "");
        file.attachments = vec![mesh::Attachment {
            name: "map.png".into(),
            mime: "image/png".into(),
            size: 2048,
            preview: None,
        }];
        let page = mesh::Page {
            id: "mesh1bob".into(),
            more: false,
            events: vec![message("01", "mesh1bob", "where are you?"), sent, file],
        };
        let name = |id: &str| if id == "mesh1me" { "me".to_string() } else { "Bob".to_string() };
        let Rows { timeline, mine, marks } = rows(&page, "mesh1me", name);
        assert_eq!(timeline.len(), 3);
        assert_eq!((timeline[0].sender_name.as_str(), timeline[0].web_id.as_deref()), ("Bob", Some("01")));
        assert_eq!(
            timeline[1].reply_to,
            Some(mesh_core::Quote {
                author: "Bob".into(),
                text: "where are you?".into()
            })
        );
        assert_eq!((timeline[1].reactions[0].count, timeline[1].reactions[0].by_me), (2, true));
        assert!(timeline[2].body.starts_with("📎 map.png"), "{}", timeline[2].body);
        assert_eq!(timeline[0].sender_network, Network::Mesh);
        assert_eq!(mine, HashSet::from(["02".to_string(), "03".to_string()]));
        assert_eq!(marks, HashMap::from([("02".to_string(), "edited"), ("03".to_string(), "sending")]));
    }
}
