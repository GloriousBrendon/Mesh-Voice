//! Matrix client logic shared by every mesh-app frontend.

mod audio;
mod bridge;
mod call;
mod session;
mod spaces;
mod timeline;
mod verification;

pub use bridge::{Network, user_network};
pub use call::CallEngine;
pub use session::{Session, SessionStore};
pub use spaces::{SpaceGroup, group_by_space};
pub use timeline::{ImageInfo, MediaRef, MessageKind, Reaction, TimelineMessage};

pub use matrix_sdk::ruma::{OwnedEventId, OwnedRoomId, OwnedUserId};

use matrix_sdk::{
    Client, Room,
    config::SyncSettings,
    encryption::verification::Verification,
    room::MessagesOptions,
    ruma::events::room::message::{MessageType, RoomMessageEventContent},
};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum MeshError {
    #[error("matrix sdk error: {0}")]
    Sdk(#[from] matrix_sdk::Error),
    #[error("client build error: {0}")]
    ClientBuild(#[from] matrix_sdk::ClientBuildError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("(de)serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("no saved session found")]
    NoSession,
}

pub type MeshResult<T> = Result<T, MeshError>;

/// A single logged-in Matrix session plus a background sync loop.
pub struct MeshClient {
    client: Client,
    data_dir: PathBuf,
    call: Arc<CallEngine>,
}

impl std::fmt::Debug for MeshClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshClient")
            .field("user_id", &self.client.user_id())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct RoomSummary {
    pub id: OwnedRoomId,
    pub name: String,
    pub topic: Option<String>,
    pub unread_notifications: u64,
    pub is_direct: bool,
    /// Origin timestamp of the latest event, in milliseconds since the epoch.
    pub last_activity_ms: Option<u64>,
    /// Unread messages that mention us.
    pub mentions: u64,
    /// An `m.space`; its rooms are in `children`.
    pub is_space: bool,
    /// For spaces: the rooms listed by its `m.space.child` events.
    pub children: Vec<OwnedRoomId>,
    /// Discord if a bridge says this room is a Discord portal.
    pub network: Network,
}

/// One emoji of a SAS short-auth string, e.g. ("🐶", "Dog").
#[derive(Debug, Clone)]
pub struct SasEmoji {
    pub symbol: String,
    pub description: String,
}

/// A joined member of a room, for the member list.
#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub user_id: OwnedUserId,
    pub display_name: String,
    /// True for the logged-in user.
    pub is_me: bool,
    pub network: Network,
}

/// The logged-in user's own identity, for the user panel.
#[derive(Debug, Clone)]
pub struct MyProfile {
    pub user_id: OwnedUserId,
    pub display_name: String,
}

/// State of Secure Backup (server-side room-key backup + recovery) for the
/// account, so the UI can show the right controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupStatus {
    /// Backup and recovery are set up and this session has the secrets.
    Enabled,
    /// No backup / secret storage configured.
    Disabled,
    /// A backup exists but this session is missing the recovery secrets —
    /// the user should restore with their recovery key.
    Incomplete,
    /// Not determined yet (still syncing).
    Unknown,
}

/// Push updates delivered on the channel returned by
/// [`MeshClient::subscribe_updates`], driven by the background sync loop.
#[derive(Debug, Clone)]
pub enum MeshUpdate {
    /// Sync delivered changes (new events, state, receipts...) to these rooms.
    RoomsChanged(Vec<OwnedRoomId>),
    /// Another device or user asked to verify with us.
    VerificationRequested {
        user_id: OwnedUserId,
        flow_id: String,
    },
    /// The SAS emojis are ready to be compared on both devices.
    VerificationEmojis {
        flow_id: String,
        emojis: Vec<SasEmoji>,
    },
    VerificationDone {
        flow_id: String,
    },
    VerificationCancelled {
        flow_id: String,
        reason: String,
    },
    /// An incoming 1:1 voice call is ringing.
    IncomingCall {
        call_id: String,
        room_id: OwnedRoomId,
        caller: OwnedUserId,
    },
    /// A call we placed or accepted is negotiating media.
    CallConnecting {
        call_id: String,
    },
    /// Media is flowing — the call is live.
    CallConnected {
        call_id: String,
    },
    /// The call ended (hangup, rejection, or lost connection).
    CallEnded {
        call_id: String,
        reason: String,
    },
    /// Who is typing in a room now (display names, never us); empty = nobody.
    Typing {
        room_id: OwnedRoomId,
        names: Vec<String>,
    },
}

impl MeshClient {
    /// Log in with a password and persist the resulting session under `data_dir`.
    pub async fn login_with_password(
        homeserver_url: &str,
        user: &str,
        password: &str,
        data_dir: PathBuf,
    ) -> MeshResult<Self> {
        std::fs::create_dir_all(&data_dir)?;

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(data_dir.join("store"), None)
            .build()
            .await?;

        client
            .matrix_auth()
            .login_username(user, password)
            .initial_device_display_name("Mesh")
            .send()
            .await?;

        if let Some(session) = client.matrix_auth().session() {
            let mut session = Session::from(session);
            session.homeserver = homeserver_url.to_string();
            SessionStore::new(&data_dir).save(&session)?;
        }

        let call = CallEngine::new(client.clone());
        Ok(Self {
            client,
            data_dir,
            call,
        })
    }

    /// Restore a previously-persisted session without touching the network.
    pub async fn restore(data_dir: PathBuf) -> MeshResult<Self> {
        let session = SessionStore::new(&data_dir)
            .load()?
            .ok_or(MeshError::NoSession)?;

        let client = Client::builder()
            .homeserver_url(&session.homeserver)
            .sqlite_store(data_dir.join("store"), None)
            .build()
            .await?;

        client.restore_session(session.into_matrix_session()).await?;

        let call = CallEngine::new(client.clone());
        Ok(Self {
            client,
            data_dir,
            call,
        })
    }

    pub fn has_saved_session(data_dir: &PathBuf) -> bool {
        matches!(SessionStore::new(data_dir).load(), Ok(Some(_)))
    }

    /// Runs the sync loop forever; call this in a background task. Network
    /// errors (homeserver restart, Wi-Fi drop) are retried; it only returns
    /// once the access token is rejected, i.e. after logout.
    pub async fn run_sync(&self) -> MeshResult<()> {
        use matrix_sdk::ruma::api::error::ErrorKind;
        loop {
            // `sync` resumes from the stored sync token, so retrying loses nothing.
            let Err(err) = self.client.sync(SyncSettings::default()).await else {
                return Ok(());
            };
            if matches!(err.client_api_error_kind(), Some(ErrorKind::UnknownToken { .. })) {
                return Err(err.into());
            }
            tracing::warn!("sync failed, retrying in 5s: {err}");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Joined rooms, most recently active first.
    pub async fn rooms(&self) -> Vec<RoomSummary> {
        let mut summaries = Vec::new();
        for room in self.client.rooms() {
            let is_direct = room.is_direct().await.unwrap_or(false);
            let is_space = room.is_space();
            let children = if is_space { space_children(&room).await } else { Vec::new() };
            summaries.push(RoomSummary {
                id: room.room_id().to_owned(),
                name: room
                    .cached_display_name()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| room.room_id().to_string()),
                topic: room.topic(),
                unread_notifications: room.num_unread_notifications(),
                is_direct,
                last_activity_ms: room.latest_event_timestamp().map(|ts| ts.0.into()),
                mentions: room.num_unread_mentions(),
                is_space,
                children,
                network: bridge::room_network(&room).await,
            });
        }
        summaries.sort_by(|a, b| {
            b.last_activity_ms
                .cmp(&a.last_activity_ms)
                .then_with(|| a.name.cmp(&b.name))
        });
        summaries
    }

    /// Joined members of a room, for the member list (self last).
    pub async fn room_members(&self, room_id: &OwnedRoomId) -> Vec<MemberInfo> {
        let Some(room) = self.client.get_room(room_id) else {
            return Vec::new();
        };
        let me = self.client.user_id();
        let members = room
            .members(matrix_sdk::RoomMemberships::JOIN)
            .await
            .unwrap_or_default();
        let mut infos: Vec<MemberInfo> = members
            .into_iter()
            .map(|m| MemberInfo {
                display_name: m
                    .display_name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| m.user_id().localpart().to_owned()),
                is_me: Some(m.user_id()) == me,
                network: user_network(m.user_id()),
                user_id: m.user_id().to_owned(),
            })
            .collect();
        infos.sort_by(|a, b| {
            a.is_me
                .cmp(&b.is_me)
                .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()))
        });
        infos
    }

    /// The logged-in user's identity (display name falls back to the localpart).
    pub async fn my_profile(&self) -> MyProfile {
        let user_id = self
            .client
            .user_id()
            .map(|u| u.to_owned())
            .expect("logged-in client has a user id");
        let display_name = match self.client.account().get_display_name().await {
            Ok(Some(name)) if !name.is_empty() => name,
            _ => user_id.localpart().to_owned(),
        };
        MyProfile {
            user_id,
            display_name,
        }
    }

    /// The most recent `limit` messages of a room, oldest first.
    pub async fn timeline(
        &self,
        room_id: &OwnedRoomId,
        limit: u16,
    ) -> MeshResult<Vec<TimelineMessage>> {
        let Some(room) = self.client.get_room(room_id) else {
            return Ok(Vec::new());
        };

        let mut options = MessagesOptions::backward();
        options.limit = limit.into();
        let messages = room.messages(options).await?;

        Ok(timeline::build(&room, messages.chunk, self.client.user_id()).await)
    }

    /// Subscribes to push updates from the sync loop: room changes and
    /// interactive verification progress. Call once per session.
    pub fn subscribe_updates(&self) -> mpsc::UnboundedReceiver<MeshUpdate> {
        let (tx, rx) = mpsc::unbounded_channel();

        verification::register_handlers(&self.client, tx.clone());
        timeline::register_typing_handler(&self.client, tx.clone());
        self.call.set_update_sender(tx.clone());
        self.call.register_handlers();

        let mut room_updates = self.client.subscribe_to_all_room_updates();
        tokio::spawn(async move {
            loop {
                match room_updates.recv().await {
                    Ok(updates) => {
                        let ids: Vec<_> = updates.iter_all_room_ids().cloned().collect();
                        if tx.send(MeshUpdate::RoomsChanged(ids)).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        rx
    }

    /// Accept an incoming verification request; progress arrives as
    /// [`MeshUpdate`] events.
    pub async fn accept_verification(
        &self,
        user_id: &OwnedUserId,
        flow_id: &str,
    ) -> MeshResult<()> {
        if let Some(request) = self
            .client
            .encryption()
            .get_verification_request(user_id, flow_id)
            .await
        {
            request.accept().await?;
        }
        Ok(())
    }

    /// Confirm that the SAS emojis match on both devices.
    pub async fn confirm_verification(
        &self,
        user_id: &OwnedUserId,
        flow_id: &str,
    ) -> MeshResult<()> {
        if let Some(Verification::SasV1(sas)) = self
            .client
            .encryption()
            .get_verification(user_id, flow_id)
            .await
        {
            sas.confirm().await?;
        }
        Ok(())
    }

    /// Cancel a verification flow at whatever stage it is in.
    pub async fn cancel_verification(
        &self,
        user_id: &OwnedUserId,
        flow_id: &str,
    ) -> MeshResult<()> {
        if let Some(Verification::SasV1(sas)) = self
            .client
            .encryption()
            .get_verification(user_id, flow_id)
            .await
        {
            sas.cancel().await?;
        } else if let Some(request) = self
            .client
            .encryption()
            .get_verification_request(user_id, flow_id)
            .await
        {
            request.cancel().await?;
        }
        Ok(())
    }

    pub fn room(&self, room_id: &OwnedRoomId) -> Option<Room> {
        self.client.get_room(room_id)
    }

    pub async fn send_message(&self, room_id: &OwnedRoomId, body: &str) -> MeshResult<()> {
        if let Some(room) = self.client.get_room(room_id) {
            room.send(RoomMessageEventContent::text_markdown(body)).await?;
        }
        Ok(())
    }

    /// Reacts to `event_id` with `key` (an emoji, usually).
    pub async fn react(&self, room_id: &OwnedRoomId, event_id: &OwnedEventId, key: &str) -> MeshResult<()> {
        use matrix_sdk::ruma::events::{reaction::ReactionEventContent, relation::Annotation};
        if let Some(room) = self.client.get_room(room_id) {
            room.send(ReactionEventContent::new(Annotation::new(event_id.clone(), key.to_owned())))
                .await?;
        }
        Ok(())
    }

    /// Takes back one of our own events (e.g. a reaction).
    pub async fn redact(&self, room_id: &OwnedRoomId, event_id: &OwnedEventId) -> MeshResult<()> {
        if let Some(room) = self.client.get_room(room_id) {
            room.redact(event_id, None, None)
                .await
                .map_err(matrix_sdk::Error::from)?;
        }
        Ok(())
    }

    /// Downloads (and decrypts, for encrypted rooms) a piece of media. Cached
    /// in the local store, so a second call is cheap.
    pub async fn media(&self, media: &MediaRef) -> MeshResult<Vec<u8>> {
        use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
        let request = MediaRequestParameters { source: media.0.clone(), format: MediaFormat::File };
        Ok(self.client.media().get_media_content(&request, true).await?)
    }

    /// Logs out: invalidates the access token on the server (which also stops
    /// the sync loop), then deletes the saved session and the local store so
    /// the next login starts clean instead of tripping over this device's state.
    pub async fn logout(&self) -> MeshResult<()> {
        if let Err(err) = self.client.matrix_auth().logout().await {
            tracing::warn!("server logout failed, clearing locally anyway: {err}");
        }
        SessionStore::new(&self.data_dir).clear()?;
        // ponytail: on Windows the open sqlite files may refuse removal; the next
        // login then reuses a stale store. Fix by closing the client first if it bites.
        let store = self.data_dir.join("store");
        if store.exists()
            && let Err(err) = std::fs::remove_dir_all(&store)
        {
            tracing::warn!("could not remove store {}: {err}", store.display());
        }
        Ok(())
    }

    /// Places a 1:1 voice call to the other member of `room_id`.
    pub async fn place_call(&self, room_id: &OwnedRoomId) -> Result<(), String> {
        self.call.place_call(room_id).await
    }

    /// Accepts the currently-ringing incoming call.
    pub async fn accept_call(&self) -> Result<(), String> {
        self.call.accept_call().await
    }

    /// Hangs up or rejects the current call.
    pub async fn hangup(&self) -> Result<(), String> {
        self.call.hangup().await
    }

    /// Mutes or unmutes the local microphone for the current call.
    pub async fn set_call_muted(&self, muted: bool) {
        self.call.set_muted(muted).await
    }

    /// Available microphone device names.
    pub fn audio_input_devices(&self) -> Vec<String> {
        audio::input_device_names()
    }

    /// Available speaker device names.
    pub fn audio_output_devices(&self) -> Vec<String> {
        audio::output_device_names()
    }

    /// Selects the mic / speaker for future calls (`None` = system default).
    pub fn set_audio_devices(&self, input: Option<String>, output: Option<String>) {
        self.call.set_audio_devices(input, output);
    }

    /// Deafens or undeafens: silences the remote audio (and mutes the mic too,
    /// as clients like Discord do).
    pub async fn set_call_deafened(&self, deafened: bool) {
        self.call.set_deafened(deafened).await
    }

    /// Current Secure Backup state.
    pub fn backup_status(&self) -> BackupStatus {
        use matrix_sdk::encryption::recovery::RecoveryState;
        match self.client.encryption().recovery().state() {
            RecoveryState::Enabled => BackupStatus::Enabled,
            RecoveryState::Disabled => BackupStatus::Disabled,
            RecoveryState::Incomplete => BackupStatus::Incomplete,
            RecoveryState::Unknown => BackupStatus::Unknown,
        }
    }

    /// Sets up Secure Backup and continuous room-key upload, returning the
    /// recovery key to show the user once. If a stale backup already exists
    /// that we can't connect to, it is reset and a fresh key is minted.
    pub async fn enable_secure_backup(&self) -> Result<String, String> {
        let recovery = self.client.encryption().recovery();
        match recovery.enable().await {
            Ok(key) => Ok(key),
            Err(_) => recovery.reset_key().await.map_err(|e| e.to_string()),
        }
    }

    /// Turns Secure Backup on when the account has none at all, so room keys
    /// are backed up going forward. Returns the new recovery key (for the UI
    /// to show once), or `None` when nothing was changed. An existing backup
    /// this session can't open yet (`Incomplete`, normal right after a fresh
    /// login) is left alone: the user restores it with their recovery key.
    /// Never resets here — that would orphan the keys in the old backup.
    pub async fn ensure_secure_backup(&self) -> Result<Option<String>, String> {
        use matrix_sdk::encryption::recovery::RecoveryState;
        // Let initial sync populate the recovery/cross-signing state first.
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        match self.client.encryption().recovery().state() {
            RecoveryState::Disabled => self
                .client
                .encryption()
                .recovery()
                .enable()
                .await
                .map(Some)
                .map_err(|e| e.to_string()),
            _ => Ok(None),
        }
    }

    /// Restores room keys from the existing backup using the recovery key.
    pub async fn restore_secure_backup(&self, recovery_key: &str) -> Result<(), String> {
        self.client
            .encryption()
            .recovery()
            .recover_and_fix_backup(recovery_key.trim())
            .await
            .map_err(|e| e.to_string())
    }
}

/// Extracts a plain-text body from a message event, if present.
pub fn plain_body(msgtype: &MessageType) -> Option<&str> {
    match msgtype {
        MessageType::Text(text) => Some(text.body.as_str()),
        MessageType::Notice(notice) => Some(notice.body.as_str()),
        MessageType::Emote(emote) => Some(emote.body.as_str()),
        _ => None,
    }
}

/// The rooms a space lists through its `m.space.child` state events. A child
/// event with an empty `via` is a removed child and is skipped.
async fn space_children(room: &Room) -> Vec<OwnedRoomId> {
    use matrix_sdk::deserialized_responses::SyncOrStrippedState;
    use matrix_sdk::ruma::events::{SyncStateEvent, space::child::SpaceChildEventContent};
    let events = match room.get_state_events_static::<SpaceChildEventContent>().await {
        Ok(events) => events,
        Err(err) => {
            tracing::warn!("could not read children of space {}: {err}", room.room_id());
            return Vec::new();
        }
    };
    events
        .into_iter()
        .filter_map(|raw| match raw.deserialize().ok()? {
            SyncOrStrippedState::Sync(SyncStateEvent::Original(ev)) => {
                (!ev.content.via.is_empty()).then_some(ev.state_key)
            }
            SyncOrStrippedState::Stripped(ev) => {
                ev.content.via.is_some_and(|via| !via.is_empty()).then_some(ev.state_key)
            }
            _ => None,
        })
        .collect()
}
