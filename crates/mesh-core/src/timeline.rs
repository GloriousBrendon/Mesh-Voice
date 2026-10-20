//! Turns raw room events into the plain [`TimelineMessage`]s the UI draws:
//! messages, images, reactions (folded onto their target) and membership /
//! room changes as system lines. Also the typing-notification handler.

use crate::{MeshUpdate, Network, user_network};
use matrix_sdk::deserialized_responses::TimelineEvent;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::member::MembershipChange;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::typing::SyncTypingEvent;
use matrix_sdk::ruma::events::{
    AnySyncMessageLikeEvent, AnySyncStateEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
    SyncStateEvent,
};
use matrix_sdk::ruma::{OwnedEventId, OwnedUserId, UserId};
use matrix_sdk::{Client, Room};
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct TimelineMessage {
    /// `None` only for events the server sent without one (never in practice).
    pub event_id: Option<OwnedEventId>,
    pub sender: OwnedUserId,
    pub sender_name: String,
    pub sender_avatar: Option<MediaRef>,
    /// Discord for bridge ghosts.
    pub sender_network: Network,
    /// The text; an image's filename; a system line's description.
    pub body: String,
    pub kind: MessageKind,
    pub reactions: Vec<Reaction>,
    /// Origin server timestamp in milliseconds since the epoch.
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageKind {
    Text,
    /// `/me` action.
    Emote,
    Image(ImageInfo),
    /// A join, leave, rename...: drawn as a small mono line, not a message.
    System,
    /// Encrypted and we don't have the key.
    Undecryptable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub media: MediaRef,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// Bytes.
    pub size: Option<u64>,
}

/// Where to download a piece of media from: Matrix media (pass it to
/// [`crate::MeshClient::media`]) or a web URL the UI fetches itself (a
/// Discord picture, say). Opaque to the UI except for its cache key.
#[derive(Debug, Clone)]
pub struct MediaRef(pub(crate) Source);

#[derive(Debug, Clone)]
pub(crate) enum Source {
    Matrix(MediaSource),
    Web(String),
}

impl MediaRef {
    pub(crate) fn plain(url: matrix_sdk::ruma::OwnedMxcUri) -> Self {
        MediaRef(Source::Matrix(MediaSource::Plain(url)))
    }

    pub fn web(url: String) -> Self {
        MediaRef(Source::Web(url))
    }

    /// The URL, for web media; `None` for Matrix media.
    pub fn web_url(&self) -> Option<&str> {
        match &self.0 {
            Source::Web(url) => Some(url),
            Source::Matrix(_) => None,
        }
    }

    pub(crate) fn matrix(&self) -> crate::MeshResult<MediaSource> {
        match &self.0 {
            Source::Matrix(source) => Ok(source.clone()),
            Source::Web(_) => Err(crate::MeshError::NotMatrixMedia),
        }
    }
}

impl PartialEq for MediaRef {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl MediaRef {
    /// A stable key for caching the downloaded bytes.
    pub fn key(&self) -> String {
        match &self.0 {
            Source::Matrix(MediaSource::Plain(uri)) => uri.to_string(),
            Source::Matrix(MediaSource::Encrypted(file)) => file.url.to_string(),
            Source::Web(url) => url.clone(),
        }
    }
}

/// One reaction pill: an emoji (or text) and how many people used it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub key: String,
    pub count: usize,
    /// Our own reaction event, if we reacted (redact it to take it back).
    pub mine: Option<OwnedEventId>,
}

/// Builds the timeline from a newest-first `chunk`; returns it oldest-first.
pub(crate) async fn build(room: &Room, chunk: Vec<TimelineEvent>, me: Option<&UserId>) -> Vec<TimelineMessage> {
    let mut names: HashMap<OwnedUserId, (String, Option<MediaRef>)> = HashMap::new();
    let mut timeline = Vec::new();
    // target event → (key, sender, reaction event)
    let mut reactions: HashMap<OwnedEventId, Vec<(String, OwnedUserId, OwnedEventId)>> = HashMap::new();

    for event in chunk {
        let Ok(event) = event.raw().deserialize() else {
            continue;
        };
        let (body, kind) = match &event {
            AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
                SyncMessageLikeEvent::Original(message),
            )) => match message_body(&message.content.msgtype) {
                Some(parsed) => parsed,
                None => continue,
            },
            AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Reaction(
                SyncMessageLikeEvent::Original(reaction),
            )) => {
                let annotation = &reaction.content.relates_to;
                reactions.entry(annotation.event_id.clone()).or_default().push((
                    annotation.key.clone(),
                    reaction.sender.clone(),
                    reaction.event_id.clone(),
                ));
                continue;
            }
            // Still encrypted: we lack the key. Show it rather than silently drop it.
            AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomEncrypted(
                SyncMessageLikeEvent::Original(_),
            )) => (
                "🔒 Unable to decrypt this message (restore keys in Settings → Secure Backup)".to_owned(),
                MessageKind::Undecryptable,
            ),
            AnySyncTimelineEvent::State(state) => match system_line(state) {
                Some(line) => (line, MessageKind::System),
                None => continue,
            },
            _ => continue,
        };

        let sender = event.sender().to_owned();
        if !names.contains_key(&sender) {
            names.insert(sender.clone(), name_and_avatar(room, &sender).await);
        }
        timeline.push(TimelineMessage {
            event_id: Some(event.event_id().to_owned()),
            sender_name: names[&sender].0.clone(),
            sender_avatar: names[&sender].1.clone(),
            sender_network: user_network(&sender),
            sender,
            body,
            kind,
            reactions: Vec::new(),
            timestamp_ms: event.origin_server_ts().0.into(),
        });
    }

    for message in &mut timeline {
        if let Some(event_id) = &message.event_id
            && let Some(list) = reactions.remove(event_id)
        {
            message.reactions = fold_reactions(list, me);
        }
    }
    // `backward` pagination yields newest-first; the UI wants oldest-first.
    timeline.reverse();
    timeline
}

/// Groups raw reactions into pills in first-seen order, counting each
/// person once per key.
fn fold_reactions(list: Vec<(String, OwnedUserId, OwnedEventId)>, me: Option<&UserId>) -> Vec<Reaction> {
    let mut pills: Vec<(Reaction, Vec<OwnedUserId>)> = Vec::new();
    // The chunk is newest-first; fold oldest-first so pill order is stable.
    for (key, sender, event_id) in list.into_iter().rev() {
        let index = match pills.iter().position(|(r, _)| r.key == key) {
            Some(index) => index,
            None => {
                pills.push((Reaction { key, count: 0, mine: None }, Vec::new()));
                pills.len() - 1
            }
        };
        let (pill, senders) = &mut pills[index];
        if senders.contains(&sender) {
            continue;
        }
        if me == Some(sender.as_ref()) {
            pill.mine = Some(event_id);
        }
        senders.push(sender);
        pill.count += 1;
    }
    pills.into_iter().map(|(pill, _)| pill).collect()
}

async fn name_and_avatar(room: &Room, user: &UserId) -> (String, Option<MediaRef>) {
    let member = room.get_member_no_sync(user).await.ok().flatten();
    let name = member
        .as_ref()
        .and_then(|m| m.display_name().map(str::to_owned))
        .unwrap_or_else(|| user.localpart().to_owned());
    let avatar = member.and_then(|m| m.avatar_url().map(|u| MediaRef::plain(u.to_owned())));
    (name, avatar)
}

async fn display_name(room: &Room, user: &UserId) -> String {
    room.get_member_no_sync(user)
        .await
        .ok()
        .flatten()
        .and_then(|member| member.display_name().map(str::to_owned))
        .unwrap_or_else(|| user.localpart().to_owned())
}

/// The text and kind of a message, or `None` for types we don't show.
fn message_body(msgtype: &MessageType) -> Option<(String, MessageKind)> {
    Some(match msgtype {
        MessageType::Text(text) => (text.body.clone(), MessageKind::Text),
        MessageType::Notice(notice) => (notice.body.clone(), MessageKind::Text),
        MessageType::Emote(emote) => (emote.body.clone(), MessageKind::Emote),
        MessageType::Image(image) => {
            let info = image.info.as_deref();
            let image_info = ImageInfo {
                media: MediaRef(Source::Matrix(image.source.clone())),
                width: info.and_then(|i| i.width).map(u64::from),
                height: info.and_then(|i| i.height).map(u64::from),
                size: info.and_then(|i| i.size).map(u64::from),
            };
            let name = image.filename.clone().unwrap_or_else(|| image.body.clone());
            (name, MessageKind::Image(image_info))
        }
        MessageType::File(file) => (format!("📎 {}", file.filename.as_deref().unwrap_or(&file.body)), MessageKind::Text),
        MessageType::Video(video) => (format!("🎞 {}", video.body), MessageKind::Text),
        MessageType::Audio(audio) => (format!("🔉 {}", audio.body), MessageKind::Text),
        _ => return None,
    })
}

/// A short description of a membership, name or topic change.
fn system_line(state: &AnySyncStateEvent) -> Option<String> {
    match state {
        AnySyncStateEvent::RoomMember(SyncStateEvent::Original(member)) => {
            let who = member
                .content
                .displayname
                .clone()
                .unwrap_or_else(|| member.state_key.localpart().to_owned());
            let what = match member.membership_change() {
                MembershipChange::Joined | MembershipChange::InvitationAccepted => "joined",
                MembershipChange::Left => "left",
                MembershipChange::Invited => "was invited",
                MembershipChange::Kicked => "was removed",
                MembershipChange::Banned | MembershipChange::KickedAndBanned => "was banned",
                MembershipChange::Unbanned => "was unbanned",
                MembershipChange::InvitationRejected => "declined the invite",
                _ => return None,
            };
            Some(format!("{who} {what}"))
        }
        AnySyncStateEvent::RoomName(SyncStateEvent::Original(name)) => {
            Some(format!("room renamed to {}", name.content.name))
        }
        AnySyncStateEvent::RoomTopic(SyncStateEvent::Original(_)) => Some("topic changed".to_owned()),
        _ => None,
    }
}

/// Forwards typing notifications (other people only) as [`MeshUpdate::Typing`].
pub(crate) fn register_typing_handler(client: &Client, tx: mpsc::UnboundedSender<MeshUpdate>) {
    client.add_event_handler(move |event: SyncTypingEvent, room: Room, client: Client| {
        let tx = tx.clone();
        async move {
            let me = client.user_id();
            let mut names = Vec::new();
            for user in event.content.user_ids.iter().filter(|u| Some(u.as_ref()) != me) {
                names.push(display_name(&room, user).await);
            }
            let _ = tx.send(MeshUpdate::Typing {
                room_id: room.room_id().to_owned(),
                names,
            });
        }
    });
}

/// Forwards messages the account's push rules say should notify as
/// [`MeshUpdate::Notify`]. Invites (stripped events) and our own messages
/// are skipped.
pub(crate) async fn register_notification_handler(client: &Client, tx: mpsc::UnboundedSender<MeshUpdate>) {
    use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedTimelineEvent;
    use matrix_sdk::sync::Notification;
    client
        .register_notification_handler(move |notification: Notification, room: Room, client: Client| {
            let tx = tx.clone();
            async move {
                if !notification.actions.iter().any(|a| a.should_notify()) {
                    return;
                }
                let RawAnySyncOrStrippedTimelineEvent::Sync(raw) = &notification.event else { return };
                let Ok(event) = raw.deserialize() else { return };
                if Some(event.sender()) == client.user_id() {
                    return;
                }
                let body = match &event {
                    AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
                        SyncMessageLikeEvent::Original(message),
                    )) => message.content.body().to_owned(),
                    AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomEncrypted(_)) => {
                        "Encrypted message".to_owned()
                    }
                    _ => return,
                };
                let sender = display_name(&room, event.sender()).await;
                let room_name = room.cached_display_name().map(|n| n.to_string()).unwrap_or_default();
                let title = if room.is_direct().await.unwrap_or(false) || room_name == sender {
                    sender
                } else {
                    format!("{sender} in {room_name}")
                };
                let _ = tx.send(MeshUpdate::Notify { room_id: room.room_id().to_owned(), title, body });
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> OwnedUserId {
        OwnedUserId::try_from(s).unwrap()
    }
    fn eid(s: &str) -> OwnedEventId {
        OwnedEventId::try_from(s).unwrap()
    }

    #[test]
    fn reactions_fold_per_key_and_person() {
        let me = uid("@me:x.org");
        // Newest first, as the chunk delivers them.
        let list = vec![
            ("👍".into(), me.clone(), eid("$4")),
            ("🔥".into(), uid("@a:x.org"), eid("$3")),
            ("👍".into(), uid("@a:x.org"), eid("$2")),
            ("👍".into(), uid("@a:x.org"), eid("$1")),
        ];
        let pills = fold_reactions(list, Some(&me));
        assert_eq!(pills.len(), 2);
        assert_eq!(pills[0], Reaction { key: "👍".into(), count: 2, mine: Some(eid("$4")) });
        assert_eq!(pills[1], Reaction { key: "🔥".into(), count: 1, mine: None });
    }
}
