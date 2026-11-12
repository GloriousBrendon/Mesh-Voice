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

/// What the timeline needs from a page: its rows, which messages are ours
/// and which can be edited, each message's small mark ("sending · edited"),
/// and the picture previews to show at once (by media key).
pub(crate) struct Rows {
    pub timeline: Vec<TimelineMessage>,
    pub mine: HashSet<String>,
    pub editable: HashSet<String>,
    pub marks: HashMap<String, String>,
    pub previews: Vec<(String, Vec<u8>)>,
}

/// A message's file, fetched through `mesh-node` (`dmFetch`): `mesh-file:<chat>:<message>:<index>`.
pub(crate) fn file_url(chat: &str, message: &str, index: usize) -> String {
    format!("mesh-file:{chat}:{message}:{index}")
}

/// A page as timeline rows. `name` gives a person's name from their ID. A
/// message takes a row per file, then one for its text, as WhatsApp's do.
pub(crate) fn rows(page: &mesh::Page, me: &str, name: impl Fn(&str) -> String) -> Rows {
    let mut rows = Rows {
        timeline: Vec::new(),
        mine: HashSet::new(),
        editable: HashSet::new(),
        marks: HashMap::new(),
        previews: Vec::new(),
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
        let reactions: Vec<mesh_core::Reaction> = (m.reactions.iter())
            .map(|r| mesh_core::Reaction {
                key: r.emoji.clone(),
                count: r.who.len(),
                mine: None,
                by_me: r.who.iter().any(|w| w == me),
            })
            .collect();
        let row = |body: String, kind| TimelineMessage {
            event_id: None,
            sender: sender.clone(),
            sender_name: name(&m.from),
            sender_avatar: None,
            sender_network: Network::Mesh,
            body,
            kind,
            reactions: Vec::new(),
            timestamp_ms: m.time,
            web_id: Some(m.id.clone()),
            reply_to: None,
            forwarded: false,
        };
        let first = rows.timeline.len();
        for (index, a) in m.attachments.iter().enumerate() {
            let full = file_url(&page.id, &m.id, index);
            let preview = a.preview.as_deref().and_then(unhex).filter(|_| a.mime.starts_with("image/"));
            let kind = match preview {
                // A picture: its preview at once, the full one when opened (PLAN-dm Task 18).
                Some(bytes) => {
                    let key = format!("mesh-preview:{}:{index}", m.id);
                    rows.previews.push((key.clone(), bytes));
                    MessageKind::Image(mesh_core::ImageInfo {
                        media: MediaRef::web(key),
                        width: None,
                        height: None,
                        size: Some(a.size),
                        full: Some(MediaRef::web(full)),
                    })
                }
                None => MessageKind::File(mesh_core::FileInfo {
                    media: MediaRef::web(full),
                    name: a.name.clone(),
                    kind: "File".to_string(),
                    size: Some(a.size),
                    duration: None,
                    waveform: Vec::new(),
                    thumbnail: None,
                }),
            };
            rows.timeline.push(row(a.name.clone(), kind));
        }
        if !m.text.is_empty() || m.attachments.is_empty() {
            rows.timeline.push(row(m.text.clone(), MessageKind::Text));
        }
        // The quote above the first row, the reactions under the last.
        if let Some(first) = rows.timeline.get_mut(first) {
            first.reply_to = quote;
        }
        if let Some(last) = rows.timeline.last_mut() {
            last.reactions = reactions;
        }
        if m.from == me {
            rows.mine.insert(m.id.clone());
            if m.attachments.is_empty() {
                rows.editable.insert(m.id.clone());
            }
        }
        let marks = [(m.from == me && !m.sent, "sending"), (m.edited, "edited"), (m.late, "late")];
        let mark: Vec<&str> = marks.iter().filter(|(on, _)| *on).map(|(_, label)| *label).collect();
        if !mark.is_empty() {
            rows.marks.insert(m.id.clone(), mark.join(" · "));
        }
    }
    rows
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    (digits.chunks(2))
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// What clicking a received file does (PLAN-dm Task 18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    Open,
    ShowInFolder,
}

/// A received file opens in its app only when its name and its first bytes
/// both say the same known-safe type: pictures, PDF, audio, video, plain
/// text. Anything else is shown in its folder, so clicking never runs it
/// (on Windows `start` would run a `.bat`, whatever it holds).
pub(crate) fn on_click(name: &str, head: &[u8]) -> Click {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    let text = || {
        let valid = match std::str::from_utf8(head) {
            Ok(_) => true,
            // The head may end mid-character.
            Err(e) => e.error_len().is_none(),
        };
        valid && !head.iter().any(|b| b.is_ascii_control() && !matches!(b, b'\t' | b'\n' | b'\r'))
    };
    let safe = match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" => image::guess_format(head).is_ok(),
        "pdf" => at(0, b"%PDF-"),
        "mp3" => at(0, b"ID3") || at(0, &[0xff, 0xfb]) || at(0, &[0xff, 0xf3]) || at(0, &[0xff, 0xf2]),
        "ogg" | "oga" | "opus" => at(0, b"OggS"),
        "wav" => at(0, b"RIFF") && at(8, b"WAVE"),
        "flac" => at(0, b"fLaC"),
        "m4a" | "mp4" | "mov" => at(4, b"ftyp"),
        "webm" | "mkv" => at(0, &[0x1a, 0x45, 0xdf, 0xa3]),
        "txt" | "md" | "log" => text(),
        _ => false,
    };
    if safe { Click::Open } else { Click::ShowInFolder }
}

/// Opens `path` in its app, or shows it in its folder, by [`on_click`].
fn open_received(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Read;
    let mut head = [0u8; 512];
    let n = std::fs::File::open(path)?.read(&mut head)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match on_click(&name, &head[..n]) {
        Click::Open => open_url(&path.to_string_lossy()),
        Click::ShowInFolder => {
            #[cfg(windows)]
            let shown = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
            #[cfg(not(windows))]
            let shown = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
            shown.map(|_| ())
        }
    }
}

/// Previews go in the message itself; mesh-node takes up to 16 KB (MAX_PREVIEW).
const PREVIEW_BYTES: usize = 16 * 1024;

/// A picture's preview: a small JPEG, shrunk until it fits.
fn preview_of(data: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(data).ok()?;
    [320, 200, 120].into_iter().find_map(|side| {
        let mut jpeg = Vec::new();
        let small = image::DynamicImage::ImageRgb8(picture.thumbnail(side, side).to_rgb8());
        small.write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).ok()?;
        (jpeg.len() <= PREVIEW_BYTES).then_some(jpeg)
    })
}

/// A file to send, as mesh-node takes it: a path, and a preview's path for a
/// picture. Both go in mesh-node's `files/`, which it empties at unlock and
/// lock; a picked file is sent from where it is.
pub(crate) fn staged(attachment: &Attachment) -> std::io::Result<(PathBuf, Option<PathBuf>)> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = data_dir().join("node").join("files").join(format!("sending-{nanos}"));
    let preview = match attachment.mime.starts_with("image/").then(|| preview_of(&attachment.data)).flatten() {
        Some(jpeg) => {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("preview.jpg"), jpeg)?;
            Some(dir.join("preview.jpg"))
        }
        None => None,
    };
    let path = match &attachment.path {
        Some(path) => path.clone(),
        None => {
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(safe_filename(&attachment.filename));
            std::fs::write(&path, &attachment.data)?;
            path
        }
    };
    Ok((path, preview))
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
            let Rows {
                timeline,
                mine,
                editable,
                marks,
                previews,
            } = rows(&page, &me, |id| state.mesh.name_of(id));
            // Seen while looking at it: read up to the newest.
            let unread = state.mesh.chats.iter().any(|c| c.id == page.id && c.unread > 0);
            if state.window_focused
                && unread
                && let (Some(node), Some(last)) = (&state.mesh.node, page.events.last())
            {
                node.dm_read(&page.id, &last.id);
            }
            if let Screen::LoggedIn(session) = &mut state.screen {
                // In memory only: Mesh's pictures never go to the media cache.
                for (key, bytes) in previews {
                    if !session.images.contains_key(&key) {
                        show_image(&mut session.images, &mut session.animations, &key, bytes);
                    }
                }
                (session.timeline, session.mesh_mine, session.mesh_editable, session.web_marks) = (timeline, mine, editable, marks);
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
        mesh::Event::DmFile(file) => {
            let Screen::LoggedIn(session) = &mut state.screen else { return Task::none() };
            let key = file_url(&file.id, &file.message, file.index);
            let path = std::path::Path::new(&file.path);
            if matches!(session.images.get(&key), Some(None)) {
                // A picture being viewed: shown from memory, never opened.
                match std::fs::read(path) {
                    Ok(bytes) => show_image(&mut session.images, &mut session.animations, &key, bytes),
                    Err(_) => {
                        session.images.remove(&key);
                        session.status = Some("Couldn't read that picture.".to_string());
                    }
                }
            } else {
                session.status = None;
                if let Err(err) = open_received(path) {
                    session.status = Some(format!("Couldn't open that file: {err}"));
                }
            }
        }
        _ => {}
    }
    Task::none()
}

/// A dm command's error, for the chat's status line (`fn` starts with `dm`).
/// A fetch that failed leaves its pictures to be asked for again.
pub(crate) fn failed(state: &mut App, error: &mesh::Error) {
    if let Screen::LoggedIn(session) = &mut state.screen {
        if error.func == "dmFetch" {
            session.images.retain(|key, image| !(key.starts_with("mesh-file:") && image.is_none()));
        }
        session.status = Some(error.message.clone());
    }
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

    /// A page maps onto timeline rows: names, replies, reactions, a picture
    /// (its preview first, the full one through dmFetch), a file, and the
    /// "sending", "edited" and "late" marks on the right rows. A message with
    /// files can't be edited, whatever its text.
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
        let mut picture = message("03", "mesh1me", "📎 looks like text");
        picture.attachments = vec![mesh::Attachment {
            name: "map.png".into(),
            mime: "image/png".into(),
            size: 2048,
            preview: Some("ffd8".into()),
        }];
        let mut file = message("04", "mesh1bob", "");
        file.late = true;
        file.attachments = vec![mesh::Attachment {
            name: "notes.pdf".into(),
            mime: "application/pdf".into(),
            size: 9,
            preview: None,
        }];
        let page = mesh::Page {
            id: "mesh1bob".into(),
            more: false,
            events: vec![message("01", "mesh1bob", "where are you?"), sent, picture, file],
        };
        let name = |id: &str| if id == "mesh1me" { "me".to_string() } else { "Bob".to_string() };
        let Rows {
            timeline,
            mine,
            editable,
            marks,
            previews,
        } = rows(&page, "mesh1me", name);
        // The picture's message takes two rows: the picture, then its text.
        assert_eq!(timeline.len(), 5);
        assert_eq!((timeline[0].sender_name.as_str(), timeline[0].web_id.as_deref()), ("Bob", Some("01")));
        assert_eq!(
            timeline[1].reply_to,
            Some(mesh_core::Quote {
                author: "Bob".into(),
                text: "where are you?".into()
            })
        );
        assert_eq!((timeline[1].reactions[0].count, timeline[1].reactions[0].by_me), (2, true));
        let MessageKind::Image(info) = &timeline[2].kind else {
            panic!("not a picture: {:?}", timeline[2].kind)
        };
        assert_eq!(info.media.web_url(), Some("mesh-preview:03:0"));
        assert_eq!(info.full.as_ref().and_then(MediaRef::web_url), Some("mesh-file:mesh1bob:03:0"));
        assert_eq!(previews, vec![("mesh-preview:03:0".to_string(), vec![0xff, 0xd8])]);
        assert_eq!(timeline[3].body, "📎 looks like text");
        let MessageKind::File(info) = &timeline[4].kind else { panic!("not a file") };
        assert_eq!(
            (info.name.as_str(), info.size, info.media.web_url()),
            ("notes.pdf", Some(9), Some("mesh-file:mesh1bob:04:0"))
        );
        assert_eq!(timeline[0].sender_network, Network::Mesh);
        assert_eq!(mine, HashSet::from(["02".to_string(), "03".to_string()]));
        assert_eq!(editable, HashSet::from(["02".to_string()]), "03 has a file");
        let marks: HashMap<&str, &str> = marks.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(marks, HashMap::from([("02", "edited"), ("03", "sending"), ("04", "late")]));
    }

    /// Clicking a received file never runs it: it opens only when both its
    /// contents and its name are a known-safe type; else it's shown in its
    /// folder (PLAN-dm Task 18).
    /// A picture's preview fits in a message, however big the picture.
    #[test]
    fn a_preview_fits() {
        let big = image::RgbImage::from_fn(2000, 1500, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let preview = preview_of(&png).unwrap();
        assert!(preview.len() <= PREVIEW_BYTES, "{} bytes", preview.len());
        assert_eq!(image::guess_format(&preview).unwrap(), image::ImageFormat::Jpeg);
        assert!(preview_of(b"not a picture").is_none());
    }

    #[test]
    fn a_received_file_never_runs() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let exe = b"MZ\x90\0\x03\0\0\0";
        for (name, head, click) in [
            ("map.png", &png[..], Click::Open),
            ("map.PNG", &png[..], Click::Open),
            ("notes.txt", &b"meet at the trailhead\n"[..], Click::Open),
            ("trail.pdf", &b"%PDF-1.7\n"[..], Click::Open),
            ("song.mp3", &b"ID3\x04\0"[..], Click::Open),
            ("clip.mp4", &b"\0\0\0\x18ftypmp42"[..], Click::Open),
            // A safe name with other contents, and the other way round.
            ("map.png", &exe[..], Click::ShowInFolder),
            ("notes.txt", &b"MZ\0\0"[..], Click::ShowInFolder),
            ("photo.exe", &png[..], Click::ShowInFolder),
            ("setup.exe", &exe[..], Click::ShowInFolder),
            ("run.bat", &b"echo hi\r\n"[..], Click::ShowInFolder),
            ("run.sh", &b"#!/bin/sh\necho hi\n"[..], Click::ShowInFolder),
            ("trail.lnk", &b"L\0\0\0"[..], Click::ShowInFolder),
            ("notes.txt.exe", &b"hello"[..], Click::ShowInFolder),
            ("README", &b"hello"[..], Click::ShowInFolder),
        ] {
            assert_eq!(on_click(name, head), click, "{name}");
        }
    }
}
