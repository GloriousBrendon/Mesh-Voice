//! Matrix client logic shared by every mesh-app frontend.

mod audio;
mod call;
mod session;
mod verification;

pub use call::CallEngine;
pub use session::{Session, SessionStore};

pub use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};

use matrix_sdk::{
    Client, Room,
    config::SyncSettings,
    encryption::verification::Verification,
    room::MessagesOptions,
    ruma::events::{
        AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
        room::message::{MessageType, RoomMessageEventContent},
    },
};
use std::collections::HashMap;
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
}

#[derive(Debug, Clone)]
pub struct TimelineMessage {
    pub sender: OwnedUserId,
    pub sender_name: String,
    pub body: String,
    /// Origin server timestamp in milliseconds since the epoch.
    pub timestamp_ms: u64,
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

    /// Runs the sync loop forever; call this in a background task.
    pub async fn run_sync(&self) -> MeshResult<()> {
        self.client.sync(SyncSettings::default()).await?;
        Ok(())
    }

    /// Joined rooms, most recently active first.
    pub async fn rooms(&self) -> Vec<RoomSummary> {
        let mut summaries = Vec::new();
        for room in self.client.rooms() {
            let is_direct = room.is_direct().await.unwrap_or(false);
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

        let mut display_names: HashMap<OwnedUserId, String> = HashMap::new();
        let mut timeline = Vec::new();

        for event in messages.chunk {
            let Ok(AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
                SyncMessageLikeEvent::Original(message),
            ))) = event.raw().deserialize()
            else {
                continue;
            };
            let Some(body) = plain_body(&message.content.msgtype) else {
                continue;
            };

            if !display_names.contains_key(&message.sender) {
                let name = room
                    .get_member_no_sync(&message.sender)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|member| member.display_name().map(str::to_owned))
                    .unwrap_or_else(|| message.sender.localpart().to_owned());
                display_names.insert(message.sender.clone(), name);
            }

            timeline.push(TimelineMessage {
                sender_name: display_names[&message.sender].clone(),
                sender: message.sender,
                body: body.to_owned(),
                timestamp_ms: message.origin_server_ts.0.into(),
            });
        }

        // `backward` pagination yields newest-first; the UI wants oldest-first.
        timeline.reverse();
        Ok(timeline)
    }

    /// Subscribes to push updates from the sync loop: room changes and
    /// interactive verification progress. Call once per session.
    pub fn subscribe_updates(&self) -> mpsc::UnboundedReceiver<MeshUpdate> {
        let (tx, rx) = mpsc::unbounded_channel();

        verification::register_handlers(&self.client, tx.clone());
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

    pub fn logout_locally(&self) -> MeshResult<()> {
        SessionStore::new(&self.data_dir).clear()
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
