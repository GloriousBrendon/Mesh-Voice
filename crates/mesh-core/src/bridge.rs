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

/// A room is bridged to Discord if it has an `m.bridge` (or the older
/// `uk.half-shot.bridge`) state event whose `protocol.id` is `discord`.
pub async fn room_network(room: &Room) -> Network {
    for event_type in ["m.bridge", "uk.half-shot.bridge"] {
        let Ok(events) = room.get_state_events(StateEventType::from(event_type)).await else {
            continue;
        };
        let discord = events.iter().any(|raw| {
            let json = match raw {
                RawAnySyncOrStrippedState::Sync(raw) => raw.json(),
                RawAnySyncOrStrippedState::Stripped(raw) => raw.json(),
            };
            serde_json::from_str::<serde_json::Value>(json.get())
                .is_ok_and(|event| is_discord_bridge(&event))
        });
        if discord {
            return Network::Discord;
        }
    }
    Network::Matrix
}

fn is_discord_bridge(event: &serde_json::Value) -> bool {
    event["content"]["protocol"]["id"].as_str() == Some("discord")
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
        let discord = serde_json::json!({
            "type": "m.bridge",
            "content": { "bridgebot": "@discordbot:strange.lab", "protocol": { "id": "discord" } }
        });
        let other = serde_json::json!({ "content": { "protocol": { "id": "telegram" } } });
        assert!(is_discord_bridge(&discord));
        assert!(!is_discord_bridge(&other));
        assert!(!is_discord_bridge(&serde_json::json!({})));
    }
}
