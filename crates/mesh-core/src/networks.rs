//! Every network Mesh knows, as data. The Accounts page, network tags and
//! colours, add-friend hints and bridge detection all read from here, so a
//! new network is a new entry (design/DESIGN.md §3.5).

/// How an account on the network signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Mesh's own identity: a username and password that open the keystore.
    Identity,
    Password,
    Qr,
    Phone,
    Browser,
}

/// One screen of the Accounts setup panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    SignIn,
    /// Typing the code the network sent (phone sign-in).
    Code,
    Sync,
    Done,
}

impl Auth {
    pub fn label(self) -> &'static str {
        match self {
            Auth::Identity => "Username + password",
            Auth::Password => "Homeserver sign-in",
            Auth::Qr => "Scan a QR code",
            Auth::Phone => "Phone number + code",
            Auth::Browser => "Browser sign-in",
        }
    }

    /// The setup panel's screens for this sign-in, in order.
    pub fn steps(self) -> &'static [Step] {
        match self {
            Auth::Phone => &[Step::SignIn, Step::Code, Step::Sync, Step::Done],
            _ => &[Step::SignIn, Step::Sync, Step::Done],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Ready,
    Soon,
}

/// Something the step-2 switches can leave out of Mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncItem {
    /// Stored in the config as `<network id>:<key>` when switched off.
    pub key: &'static str,
    pub label: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NetworkSpec {
    pub id: &'static str,
    /// The two-letter tag shown everywhere.
    pub code: &'static str,
    pub name: &'static str,
    /// Tag colour, readable on `surface_alt`.
    pub color: &'static str,
    pub auth: Auth,
    pub availability: Availability,
    /// The mautrix bridge on your homeserver; `None` when there's none.
    pub bridge: Option<&'static str>,
    /// Host of the engine browser tab that runs it, for networks Mesh drives
    /// through their own web client rather than a bridge.
    pub engine: Option<&'static str>,
    /// Where that tab signs in.
    pub login_url: Option<&'static str>,
    /// The network doesn't allow other apps: show the one-line warning.
    pub unofficial: bool,
    /// Phone steps next to the QR code.
    pub how: &'static [&'static str],
    pub browser_note: Option<&'static str>,
    pub sync: &'static [SyncItem],
    /// Localpart prefix of the bridge's ghost users (`discord_` → `@discord_123:hs`).
    pub ghost_prefix: Option<&'static str>,
    /// Localpart prefix of the bridge bot, which creates the bridge's spaces.
    pub bot_prefix: Option<&'static str>,
    /// `protocol.id` values in the bridge's `m.bridge` room state.
    pub protocols: &'static [&'static str],
}

impl NetworkSpec {
    pub fn soon(&self) -> bool {
        self.availability == Availability::Soon
    }

    /// The setup panel footer: what the account goes through.
    pub fn via(&self) -> String {
        match (self.engine, self.bridge) {
            (Some(host), _) => format!("hidden {host} tab"),
            (None, Some(bridge)) => bridge.to_string(),
            (None, None) => "native, no bridge".to_string(),
        }
    }
}

const fn sync(key: &'static str, label: &'static str) -> SyncItem {
    SyncItem { key, label }
}

/// Defaults for an entry; each one below sets what differs.
const BASE: NetworkSpec = NetworkSpec {
    id: "",
    code: "",
    name: "",
    color: "#8892a0",
    auth: Auth::Browser,
    availability: Availability::Soon,
    bridge: None,
    engine: None,
    login_url: None,
    unofficial: false,
    how: &[],
    browser_note: None,
    sync: &[],
    ghost_prefix: None,
    bot_prefix: None,
    protocols: &[],
};

pub const NETWORKS: &[NetworkSpec] = &[
    // Mesh's own network, first; the rest are add-ons (PLAN-identity P1).
    NetworkSpec {
        id: "mesh",
        code: "me",
        name: "Mesh",
        color: "#e6c07b",
        auth: Auth::Identity,
        availability: Availability::Ready,
        ..BASE
    },
    NetworkSpec {
        id: "mx",
        code: "mx",
        name: "Matrix",
        color: "#8fd6cc",
        auth: Auth::Password,
        availability: Availability::Ready,
        // ponytail: no Matrix switches until Mesh can hide Matrix DMs/rooms.
        ..BASE
    },
    NetworkSpec {
        id: "dc",
        code: "dc",
        name: "Discord",
        color: "#9aa6f5",
        auth: Auth::Qr,
        availability: Availability::Ready,
        // Text can also come through mautrix-discord as Matrix rooms.
        bridge: Some("mautrix-discord"),
        engine: Some("discord.com"),
        login_url: Some("https://discord.com/login"),
        unofficial: true,
        how: &["Open Discord on your phone", "Settings → Scan QR code", "Point it at this code"],
        sync: &[sync("dms", "Direct messages"), sync("servers", "Servers")],
        ghost_prefix: Some("discord_"),
        bot_prefix: Some("discordbot"),
        protocols: &["discord", "discordgo"],
        ..BASE
    },
    NetworkSpec {
        id: "wa",
        code: "wa",
        name: "WhatsApp",
        color: "#9fd88b",
        auth: Auth::Qr,
        availability: Availability::Ready,
        engine: Some("web.whatsapp.com"),
        login_url: Some("https://web.whatsapp.com/"),
        unofficial: true,
        how: &[
            "Open WhatsApp on your phone",
            "Settings → Linked devices → Link a device",
            "Point it at this code",
        ],
        sync: &[sync("chats", "Chats"), sync("groups", "Groups")],
        ghost_prefix: Some("whatsapp_"),
        bot_prefix: Some("whatsappbot"),
        protocols: &["whatsapp"],
        ..BASE
    },
    NetworkSpec {
        id: "tm",
        code: "tm",
        name: "Teams",
        color: "#a9b4ff",
        browser_note: Some("Uses your work or school sign-in. Your organisation may need to allow it."),
        sync: &[sync("chats", "Chats"), sync("channels", "Teams & channels")],
        ..BASE
    },
    NetworkSpec {
        id: "ig",
        code: "ig",
        name: "Instagram",
        color: "#eaa3c7",
        bridge: Some("mautrix-meta"),
        unofficial: true,
        browser_note: Some("Log in on Instagram’s own page. Mesh never sees your password."),
        sync: &[sync("dms", "Direct messages"), sync("groups", "Group chats")],
        ..BASE
    },
    NetworkSpec {
        id: "fm",
        code: "fm",
        name: "Messenger",
        color: "#8ab8f5",
        bridge: Some("mautrix-meta"),
        unofficial: true,
        browser_note: Some("Log in on Facebook’s own page. Mesh never sees your password."),
        sync: &[sync("chats", "Chats"), sync("groups", "Group chats")],
        ..BASE
    },
    NetworkSpec {
        id: "tg",
        code: "tg",
        name: "Telegram",
        color: "#7fc3ef",
        auth: Auth::Phone,
        bridge: Some("mautrix-telegram"),
        sync: &[sync("chats", "Chats"), sync("groups", "Groups"), sync("channels", "Channels")],
        ghost_prefix: Some("telegram_"),
        bot_prefix: Some("telegrambot"),
        protocols: &["telegram"],
        ..BASE
    },
    NetworkSpec {
        id: "sg",
        code: "sg",
        name: "Signal",
        color: "#c9b3f0",
        auth: Auth::Qr,
        bridge: Some("mautrix-signal"),
        how: &["Open Signal on your phone", "Settings → Linked devices → +", "Point it at this code"],
        sync: &[sync("chats", "Chats"), sync("groups", "Groups")],
        ghost_prefix: Some("signal_"),
        bot_prefix: Some("signalbot"),
        protocols: &["signal"],
        ..BASE
    },
];

/// The entry with this id (`"wa"`).
pub fn spec(id: &str) -> Option<&'static NetworkSpec> {
    NETWORKS.iter().find(|n| n.id == id)
}

/// The network whose bridge ghosts have this localpart, if any.
pub fn ghost_network(localpart: &str) -> Option<&'static NetworkSpec> {
    NETWORKS
        .iter()
        .find(|n| n.ghost_prefix.is_some_and(|prefix| localpart.starts_with(prefix)))
}

/// The network whose bridge bot this localpart is, if any.
pub fn bot_network(localpart: &str) -> Option<&'static NetworkSpec> {
    NETWORKS
        .iter()
        .find(|n| n.bot_prefix.is_some_and(|prefix| localpart.starts_with(prefix)))
}

/// The network an `m.bridge` event's `protocol.id` belongs to.
pub fn protocol_network(protocol: &str) -> Option<&'static NetworkSpec> {
    NETWORKS.iter().find(|n| n.protocols.contains(&protocol))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookups() {
        assert_eq!(NETWORKS.len(), 9);
        assert_eq!(spec("wa").map(|n| n.name), Some("WhatsApp"));
        assert_eq!(spec("nope"), None);
        let ids: std::collections::HashSet<_> = NETWORKS.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), NETWORKS.len(), "ids are unique");
        for n in NETWORKS {
            assert_eq!(n.code.len(), 2, "{}", n.id);
            assert!(n.color.starts_with('#') && n.color.len() == 7, "{}", n.id);
            assert!(matches!(n.id, "mx" | "mesh") || !n.sync.is_empty(), "{} has nothing to sync", n.id);
            assert!(n.auth != Auth::Qr || !n.how.is_empty(), "{} QR needs phone steps", n.id);
        }
        let ready: Vec<_> = NETWORKS.iter().filter(|n| !n.soon()).map(|n| n.id).collect();
        assert_eq!(ready, ["mesh", "mx", "dc", "wa"]);
        assert_eq!(protocol_network("discordgo").map(|n| n.id), Some("dc"));
        assert_eq!(protocol_network("irc"), None);
        assert_eq!(spec("mx").map(NetworkSpec::via).as_deref(), Some("native, no bridge"));
        assert_eq!(spec("wa").map(NetworkSpec::via).as_deref(), Some("hidden web.whatsapp.com tab"));
        assert_eq!(spec("tg").map(NetworkSpec::via).as_deref(), Some("mautrix-telegram"));
    }

    #[test]
    fn ghost_prefixes() {
        assert_eq!(ghost_network("discord_123456789").map(|n| n.id), Some("dc"));
        assert_eq!(ghost_network("whatsapp_27821234567").map(|n| n.id), Some("wa"));
        assert_eq!(ghost_network("telegram_42").map(|n| n.id), Some("tg"));
        // Bots and people aren't ghosts.
        assert_eq!(ghost_network("discordbot"), None);
        assert_eq!(ghost_network("strange"), None);
        assert_eq!(bot_network("discordbot").map(|n| n.id), Some("dc"));
        assert_eq!(bot_network("strange"), None);
    }

    #[test]
    fn step_order_per_auth() {
        use Step::*;
        assert_eq!(Auth::Identity.steps(), [SignIn, Sync, Done]);
        assert_eq!(Auth::Password.steps(), [SignIn, Sync, Done]);
        assert_eq!(Auth::Qr.steps(), [SignIn, Sync, Done]);
        assert_eq!(Auth::Browser.steps(), [SignIn, Sync, Done]);
        assert_eq!(Auth::Phone.steps(), [SignIn, Code, Sync, Done]);
    }
}
