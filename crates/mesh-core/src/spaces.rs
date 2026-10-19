//! Groups the flat room list into Home plus one group per `m.space`.

use crate::{Network, RoomSummary};
use matrix_sdk::ruma::OwnedRoomId;
use std::collections::HashSet;

/// One entry of the space rail.
#[derive(Debug, Clone)]
pub struct SpaceGroup {
    /// `None` is Home: DMs plus rooms that aren't in any joined space.
    pub space: Option<RoomSummary>,
    /// Home keeps recency order; a space's rooms are sorted by name, so its
    /// channel list doesn't jump around.
    pub rooms: Vec<RoomSummary>,
}

impl SpaceGroup {
    pub fn id(&self) -> Option<&OwnedRoomId> {
        self.space.as_ref().map(|s| &s.id)
    }

    pub fn name(&self) -> &str {
        self.space.as_ref().map_or("Home", |s| s.name.as_str())
    }

    pub fn mentions(&self) -> u64 {
        self.rooms.iter().map(|r| r.mentions).sum()
    }

    /// A space is Discord if it, or any of its rooms, is bridged (guild
    /// spaces don't always carry the bridge event themselves). Home is Matrix.
    pub fn network(&self) -> Network {
        match &self.space {
            Some(space)
                if space.network == Network::Discord
                    || self.rooms.iter().any(|r| r.network == Network::Discord) =>
            {
                Network::Discord
            }
            _ => Network::Matrix,
        }
    }
}

/// Home first, then spaces sorted by name (so `Alt+n` keys stay stable). A
/// room listed by several spaces appears in each; sub-spaces get their own
/// group instead of appearing as channels.
pub fn group_by_space(rooms: &[RoomSummary]) -> Vec<SpaceGroup> {
    let mut spaces: Vec<&RoomSummary> = rooms.iter().filter(|r| r.is_space).collect();
    spaces.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let in_a_space: HashSet<&OwnedRoomId> = spaces.iter().flat_map(|s| &s.children).collect();
    let home = SpaceGroup {
        space: None,
        rooms: rooms
            .iter()
            .filter(|r| !r.is_space && (r.is_direct || !in_a_space.contains(&r.id)))
            .cloned()
            .collect(),
    };

    let groups = spaces.into_iter().map(|space| {
        let mut children: Vec<RoomSummary> = rooms
            .iter()
            .filter(|r| !r.is_space && !r.is_direct && space.children.contains(&r.id))
            .cloned()
            .collect();
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        SpaceGroup { space: Some(space.clone()), rooms: children }
    });
    std::iter::once(home).chain(groups).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, name: &str, direct: bool, children: &[&str]) -> RoomSummary {
        RoomSummary {
            id: OwnedRoomId::try_from(format!("!{id}:x.org")).unwrap(),
            name: name.into(),
            topic: None,
            unread_notifications: 0,
            is_direct: direct,
            last_activity_ms: None,
            mentions: 1,
            is_space: !children.is_empty(),
            children: children
                .iter()
                .map(|c| OwnedRoomId::try_from(format!("!{c}:x.org")).unwrap())
                .collect(),
            network: if name == "beta" { Network::Discord } else { Network::Matrix },
        }
    }

    #[test]
    fn groups_rooms_under_spaces() {
        let rooms = [
            room("dm", "alice", true, &[]),
            room("lone", "lonely", false, &[]),
            room("b", "beta", false, &[]),
            room("a", "Alpha", false, &[]),
            room("s2", "zeta space", false, &["a"]),
            room("s1", "Arch", false, &["b", "a", "dm", "gone"]),
        ];
        let groups = group_by_space(&rooms);
        let names = |g: &SpaceGroup| g.rooms.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].name(), "Home");
        assert_eq!(names(&groups[0]), ["alice", "lonely"]);
        assert_eq!(groups[1].name(), "Arch");
        assert_eq!(names(&groups[1]), ["Alpha", "beta"]);
        assert_eq!(groups[1].mentions(), 2);
        assert_eq!(names(&groups[2]), ["Alpha"]);
        assert_eq!(groups[0].network(), Network::Matrix);
        assert_eq!(groups[1].network(), Network::Discord);
        assert_eq!(groups[2].network(), Network::Matrix);
    }
}
