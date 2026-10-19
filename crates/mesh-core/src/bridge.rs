//! Spotting Discord through a mautrix-discord bridge, from Matrix data only:
//! Mesh never talks to Discord itself.

use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::StateEventType;

/// Which network a room or person is really on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Network {
    #[default]
    Matrix,
    /// Bridged in from Discord.
    Discord,
}

impl Network {
    /// The short tag the UI shows: `mx` or `dc`.
    pub fn tag(self) -> &'static str {
        match self {
            Network::Matrix => "mx",
            Network::Discord => "dc",
        }
    }
}

/// Discord ghosts live in the bridge's user namespace; mautrix-discord's
/// default is `@discord_<id>:<hs>`. ponytail: a custom `username_template`
/// in the bridge config isn't read; make the prefix configurable if needed.
pub fn user_network(user_id: &UserId) -> Network {
    if user_id.localpart().starts_with("discord_") {
        Network::Discord
    } else {
        Network::Matrix
    }
}

/// What mautrix-discord's bridge state says about a portal room.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgedIds {
    /// The Discord channel (or DM channel) id.
    pub channel_id: String,
    /// The Discord server (guild) id; `None` for DMs.
    pub guild_id: Option<String>,
}

/// A room is a Discord portal if it has an `m.bridge` (or the older
/// `uk.half-shot.bridge`) state event for the Discord protocol.
pub async fn bridged_ids(room: &Room) -> Option<BridgedIds> {
    for event_type in ["m.bridge", "uk.half-shot.bridge"] {
        let Ok(events) = room.get_state_events(StateEventType::from(event_type)).await else {
            continue;
        };
        for raw in &events {
            let json = match raw {
                RawAnySyncOrStrippedState::Sync(raw) => raw.json(),
                RawAnySyncOrStrippedState::Stripped(raw) => raw.json(),
            };
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(json.get())
                && let Some(ids) = discord_bridge_ids(&event)
            {
                return Some(ids);
            }
        }
    }
    None
}

/// Rooms the bridge made itself (its "Discord" and "Direct Messages" spaces)
/// carry no bridge state; their creator, the bridge bot (`@discordbot:…` by
/// default), gives them away.
pub fn made_by_bridge(room: &Room) -> bool {
    room.creators()
        .unwrap_or_default()
        .iter()
        .any(|creator| creator.localpart().starts_with("discord"))
}

/// mautrix-discord says `protocol.id = "discordgo"`; other bridges `"discord"`.
fn discord_bridge_ids(event: &serde_json::Value) -> Option<BridgedIds> {
    let content = &event["content"];
    if !matches!(content["protocol"]["id"].as_str(), Some("discord" | "discordgo")) {
        return None;
    }
    Some(BridgedIds {
        channel_id: content["channel"]["id"].as_str().unwrap_or_default().to_owned(),
        guild_id: content["network"]["id"].as_str().map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghosts_are_discord() {
        let ghost = <&UserId>::try_from("@discord_123456789:strange.lab").unwrap();
        let human = <&UserId>::try_from("@strange:strange.lab").unwrap();
        let bot = <&UserId>::try_from("@discordbot:strange.lab").unwrap();
        assert_eq!(user_network(ghost), Network::Discord);
        assert_eq!(user_network(human), Network::Matrix);
        assert_eq!(user_network(bot), Network::Matrix);
    }

    #[test]
    fn bridge_event_protocol() {
        let portal = serde_json::json!({
            "type": "m.bridge",
            "content": {
                "bridgebot": "@discordbot:strange.lab",
                "protocol": { "id": "discordgo" },
                "channel": { "id": "111" },
                "network": { "id": "222" }
            }
        });
        let dm = serde_json::json!({ "content": { "protocol": { "id": "discord" }, "channel": { "id": "333" } } });
        let other = serde_json::json!({ "content": { "protocol": { "id": "telegram" } } });
        assert_eq!(
            discord_bridge_ids(&portal),
            Some(BridgedIds { channel_id: "111".into(), guild_id: Some("222".into()) })
        );
        assert_eq!(discord_bridge_ids(&dm).and_then(|ids| ids.guild_id), None);
        assert!(discord_bridge_ids(&other).is_none());
        assert!(discord_bridge_ids(&serde_json::json!({})).is_none());
    }
}
