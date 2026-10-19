//! Friends, Matrix-style. Matrix has no friend list: a friend is someone you
//! share an encrypted DM with, and pending requests are DM invites.

use crate::{MeshClient, MeshResult, Network, user_network};
use matrix_sdk::RoomMemberships;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};

/// A DM invite waiting on someone.
#[derive(Debug, Clone)]
pub struct PendingContact {
    pub room_id: OwnedRoomId,
    pub user_id: OwnedUserId,
    pub network: Network,
    /// They invited us (accept / ignore); otherwise we invited them (cancel).
    pub incoming: bool,
}

impl MeshClient {
    /// Opens a DM with `user_id`: the existing one if there is one, else a new
    /// encrypted room flagged `is_direct` (which also updates `m.direct`, so
    /// the contact follows you to every device).
    pub async fn start_dm(&self, user_id: &OwnedUserId) -> MeshResult<OwnedRoomId> {
        if let Some(room) = self.client.get_dm_room(user_id) {
            return Ok(room.room_id().to_owned());
        }
        Ok(self.client.create_dm(user_id).await?.room_id().to_owned())
    }

    /// Someone's display name, or `None` if the profile can't be read
    /// (unknown user, private profile, homeserver down).
    pub async fn display_name_of(&self, user_id: &OwnedUserId) -> Option<String> {
        let profile = self.client.account().fetch_user_profile_of(user_id).await.ok()?;
        profile.get("displayname")?.as_str().map(str::to_owned)
    }

    /// DM invites both ways: incoming invites to direct rooms, and DMs we made
    /// that the other person hasn't joined yet.
    pub async fn pending_contacts(&self) -> Vec<PendingContact> {
        let mut pending = Vec::new();
        for room in self.client.invited_rooms() {
            if !room.is_direct().await.unwrap_or(false) {
                continue;
            }
            if let Ok(invite) = room.invite_details().await {
                pending.push(PendingContact {
                    room_id: room.room_id().to_owned(),
                    network: user_network(&invite.inviter_id),
                    user_id: invite.inviter_id,
                    incoming: true,
                });
            }
        }
        for room in self.client.joined_rooms() {
            if room.joined_members_count() != 1 || !room.is_direct().await.unwrap_or(false) {
                continue;
            }
            let invited = room.members(RoomMemberships::INVITE).await.unwrap_or_default();
            for member in invited {
                pending.push(PendingContact {
                    room_id: room.room_id().to_owned(),
                    user_id: member.user_id().to_owned(),
                    network: user_network(member.user_id()),
                    incoming: false,
                });
            }
        }
        pending
    }

    pub async fn accept_invite(&self, room_id: &OwnedRoomId) -> MeshResult<()> {
        if let Some(room) = self.client.get_room(room_id) {
            room.join().await?;
        }
        Ok(())
    }

    /// Declines an invite, or leaves a room.
    pub async fn leave_room(&self, room_id: &OwnedRoomId) -> MeshResult<()> {
        if let Some(room) = self.client.get_room(room_id) {
            room.leave().await?;
        }
        Ok(())
    }

    /// Withdraws our DM invite to `user_id` and leaves the empty room.
    pub async fn cancel_invite(&self, room_id: &OwnedRoomId, user_id: &OwnedUserId) -> MeshResult<()> {
        if let Some(room) = self.client.get_room(room_id) {
            room.kick_user(user_id, None).await?;
            room.leave().await?;
        }
        Ok(())
    }
}
