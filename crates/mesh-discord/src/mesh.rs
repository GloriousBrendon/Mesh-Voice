//! Mesh's own network, through the `mesh-node` helper (Mesh-Node). It speaks
//! the same JSON lines as `mesh-wa`: `{fn, args}` in on stdin, `{binding,
//! payload}` out on stdout. Commands carry passwords, so they're never logged.

use serde::Deserialize;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// What Mesh hears from `mesh-node`.
#[derive(Debug, Clone)]
pub enum Event {
    /// The helper is up; it reports `Identity` straight after.
    Connected(Node),
    Identity(Identity),
    /// The verdict on a password being typed (`Node::check_password`).
    Password { ok: bool, message: String },
    Error(Error),
    /// The safety number with someone (`Node::safety_number`).
    Safety(Safety),
    /// A command that answers with nothing else went through (`idChangePassword`, `idExport`).
    Done(String),
    /// The node's state; comes by itself whenever it changes.
    Net(Net),
    /// This device's test ticket (`Node::ticket`).
    Ticket(String),
    /// A ping's answer (`Node::ping`).
    Ping(Pong),
    /// This device's live friend codes; comes after making or revoking one,
    /// and when a seed confirms holding them.
    Codes(Codes),
    /// The ID a friend code points to (`Node::resolve`).
    Resolved(Resolved),
    /// Friends, requests both ways, blocks and our status (SPEC-social).
    /// Answers every friend command, and comes by itself when something
    /// changes (a request arrives, a friend's presence, a sync).
    Social(Social),
    /// A test note that opened, from a friend or our own other device (SPEC-e2ee).
    Note(Note),
    /// A test note went out (`Node::note`): the group it went to, if any, and
    /// how many devices it was encrypted for.
    NoteSent { conversation: Option<String>, devices: usize },
    /// A link code to type on the new device, once a seed holds where to
    /// find this one (`Node::link_start`); `expires` is unix seconds.
    Link { code: String, expires: u64 },
    /// The new device typed the code: confirm linking the device with this name.
    LinkConfirm(String),
    /// The device with this name is linked; an `Identity` with it follows.
    LinkDone(String),
    /// On the new device: the identity arrived; ask for its username and a
    /// new password (`Node::link_finish`).
    LinkReceived(String),
    /// The chat list (`Node::dm_list`); comes by itself whenever a chat changes.
    DmChats(Vec<Chat>),
    /// A page of the open chat, newest last (`Node::dm_open`); comes again
    /// while that chat is open when it changes.
    DmMessages(Page),
    /// A new message in a chat that isn't open.
    DmNotify(DmNotify),
    /// Someone is typing in a chat (`id`): their IDs.
    DmTyping {
        id: String,
        who: Vec<String>,
    },
    /// A file is ready to open, decrypted to `path` (`Node::dm_fetch`).
    DmFile(DmFile),
    /// The helper exited; it's restarted by itself.
    Disconnected,
}

/// A `meshDmChats` row: a 1:1 (`id` is the friend's ID) or a group.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Chat {
    pub id: String,
    pub kind: ChatKind,
    /// The friend's name, or the group's.
    pub name: String,
    pub members: Vec<String>,
    /// Milliseconds since the epoch of the newest message; 0 for none.
    pub last: u64,
    pub unread: u64,
    pub mentions: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    Direct,
    Group,
}

/// A `meshDmMessages`: one chat's page, oldest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Page {
    pub id: String,
    pub events: Vec<DmMessage>,
    /// Older messages exist (`Node::dm_open` with `before`).
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessage {
    /// The event ID: a `TimelineMessage`'s `web_id`.
    pub id: String,
    pub from: String,
    pub text: String,
    pub edited: bool,
    pub reply_to: Option<String>,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    /// Arrived well after it was sent (SPEC-dm D2).
    pub late: bool,
    /// Milliseconds since the epoch.
    pub time: u64,
    /// Ours, and taken by another device; false while "sending".
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// A small picture preview, hex-encoded.
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub who: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DmNotify {
    pub id: String,
    pub from: String,
    /// Cut to 100 characters; a file's name if it has no text.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DmFile {
    pub id: String,
    pub message: String,
    pub index: usize,
    pub path: String,
}

/// Where the identity on this install stands. Only `Unlocked` says anything
/// about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    None,
    Locked,
    Unlocked,
}

/// A `meshIdentity` report.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub state: State,
    /// `mesh1…`, while unlocked.
    #[serde(default)]
    pub id: Option<String>,
    /// `mesh://id/…`, while unlocked.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub keep_unlocked: bool,
    /// Why "keep unlocked" can't be used on this machine, if it can't.
    #[serde(default)]
    pub keyring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    /// Seconds since the epoch.
    pub created_at: u64,
    pub this_device: bool,
    /// Hex; what `Node::remove_device` takes.
    #[serde(default)]
    pub key: String,
}

/// A `meshSafety`: the 60-digit number two people compare, for the other ID.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Safety {
    /// The other person's ID, as parsed.
    pub id: String,
    /// 12 groups of 5 digits.
    pub number: String,
    /// What the QR code holds.
    pub qr: String,
}

/// Whether this device is on the Mesh network (SPEC-transport "Helper protocol").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetState {
    /// Locked: no node.
    Off,
    /// Running, no relay reached yet.
    Connecting,
    Online,
}

/// A `meshNet` report. Never holds an IP address.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Net {
    pub state: NetState,
    /// The home relay's URL, once online.
    pub relay: Option<String>,
    /// At least one connection is on a direct path.
    pub direct: bool,
    pub connections: usize,
    /// This device's address records are on a seed: contacts can find it.
    #[serde(default)]
    pub published: bool,
    /// Those records carry this device's e2ee keys: secure messages ready.
    #[serde(default)]
    pub keys: bool,
}

/// A `meshNote`: a test note (SPEC-e2ee "Client side").
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Note {
    /// The sender's ID: ours when it's from our own other device.
    pub from: String,
    /// The sending device's name, if known.
    pub device: Option<String>,
    /// In a 1:1, the other person (our own devices need it).
    pub with: Option<String>,
    /// A group's members.
    pub group: Option<Vec<String>>,
    /// A group's ID, to reply to it.
    pub conversation: Option<String>,
    pub text: String,
    /// Seconds since the epoch.
    pub at: u64,
    /// `live` or `waited`.
    pub path: String,
}

/// A `meshPing`: who answered, by which path, and how fast.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Pong {
    /// Their `mesh1…` ID.
    pub id: String,
    /// Their device's name.
    pub name: String,
    /// `direct` or `relay`.
    pub path: String,
    pub ms: f64,
}

/// A `meshCodes` report.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Codes {
    pub codes: Vec<Code>,
    /// The code just made, if this report answers `create_code`.
    pub created: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Code {
    /// `K7QM-3XPA`.
    pub code: String,
    /// `mesh://code/…`.
    pub link: String,
    pub created_at: u64,
    /// Seconds since the epoch; `None` never expires.
    pub expires_at: Option<u64>,
    /// `None` is unlimited.
    pub max_uses: Option<u32>,
    /// A seed holds it, so others can look it up.
    #[serde(default)]
    pub held: bool,
}

/// A `meshSocial` report (SPEC-social).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Social {
    /// The command it answers; `None` when it came by itself.
    #[serde(rename = "fn")]
    pub answers: Option<String>,
    pub friends: Vec<Friend>,
    /// Requests to us, waiting for Accept or Decline.
    pub incoming: Vec<Request>,
    /// People we asked, or added by ID, who haven't connected yet.
    pub outgoing: Vec<Known>,
    pub blocked: Vec<Known>,
    /// `online`, `idle`, `dnd` or `invisible`.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Friend {
    /// Their `mesh1…` ID.
    pub id: String,
    /// The name they gave themselves: a hint only.
    pub name: String,
    /// Our name for them; empty if none.
    pub nick: String,
    pub verified: bool,
    /// `online`, `idle`, `dnd` or `offline`.
    pub presence: String,
    /// When one of their devices last answered a dial (seconds since the epoch).
    pub seen: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Request {
    pub id: String,
    pub name: String,
    pub note: String,
    /// When they sent it.
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Known {
    pub id: String,
    pub nick: String,
    pub at: u64,
}

/// A `meshResolved`: the code and the ID it points to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Resolved {
    pub code: String,
    pub id: String,
    pub link: String,
}

/// A `meshError`: the command that failed, a stable kind and a message to show.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Error {
    #[serde(rename = "fn")]
    pub func: String,
    pub kind: String,
    pub message: String,
}

/// Commands into `mesh-node`. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Node {
    commands: mpsc::UnboundedSender<String>,
}

impl Node {
    /// Makes the identity; answers with `Identity` (unlocked) or `Error`.
    pub fn create(&self, username: &str, password: &str) {
        self.call("idCreate", &[username, password]);
    }
    /// Opens the identity; a wrong username or password answers with the
    /// same `Error` either way.
    pub fn unlock(&self, username: &str, password: &str) {
        self.call("idUnlock", &[username, password]);
    }
    pub fn lock(&self) {
        self.call("idLock", &[]);
    }
    /// Turns "keep unlocked on this device" on or off; answers with `Identity`.
    pub fn keep_unlocked(&self, on: bool) {
        self.call("idKeepUnlocked", &[if on { "true" } else { "false" }]);
    }
    /// Answers with `Done` or `Error`.
    pub fn change_password(&self, username: &str, old: &str, new: &str) {
        self.call("idChangePassword", &[username, old, new]);
    }
    /// Writes an encrypted backup that opens with this username and password;
    /// answers with `Done` or `Error`.
    pub fn export(&self, path: &str, username: &str, password: &str) {
        self.call("idExport", &[path, username, password]);
    }
    /// Restores a backup as this install's identity (a new device), with a new
    /// password; answers with `Identity` (unlocked) or `Error`.
    pub fn import(&self, path: &str, username: &str, backup_password: &str, new_password: &str) {
        self.call("idImport", &[path, username, backup_password, new_password]);
    }
    /// The safety number with a pasted ID or `mesh://` link; answers with
    /// `Safety`, or an `Error` naming what's wrong with the text.
    pub fn safety_number(&self, other: &str) {
        self.call("safetyNumber", &[other]);
    }
    /// Asks whether a new password would be accepted; answers with `Password`.
    pub fn check_password(&self, username: &str, password: &str) {
        self.call("idCheckPassword", &[username, password]);
    }

    /// Makes a friend code; `expiry` is seconds, `never` or empty (7 days),
    /// `uses` a count, `unlimited` or empty (1). Answers with `Codes`.
    pub fn create_code(&self, expiry: &str, uses: &str) {
        self.call("codeCreate", &[expiry, uses]);
    }
    /// Answers with `Codes`.
    pub fn revoke_code(&self, code: &str) {
        self.call("codeRevoke", &[code]);
    }
    /// Looks a friend code (or `mesh://code/` link) up on the seeds; answers
    /// with `Resolved` or an `Error` (`not-found`, `slow-down`, `offline`, or
    /// what's wrong with the text).
    pub fn resolve(&self, code: &str) {
        self.call("codeResolve", &[code]);
    }
    /// Asks for the node's state; answers with `Net`.
    pub fn net_status(&self) {
        self.call("netStatus", &[]);
    }
    /// This device's `mesh://node/…` test ticket; answers with `Ticket`, or an
    /// `Error` (`offline`) before a relay is reached.
    pub fn ticket(&self) {
        self.call("netTicket", &[]);
    }
    /// Pings the device a ticket names; answers with `Ping` or an `Error`
    /// (`bad-ticket`, `unreachable`, `timeout`, `bad-cert`, `key-mismatch`, `offline`).
    pub fn ping(&self, ticket: &str) {
        self.call("netPing", &[ticket]);
    }

    /// Sends a friend request through someone's live friend code (it uses
    /// one of the code's uses). Answers with `Social` once a seed has it, or
    /// an `Error` (`not-found`, `self`, `blocked`, `already-friend`,
    /// `slow-down`, `offline`, `bad-note`).
    pub fn request(&self, code: &str, note: &str) {
        self.call("friendRequest", &[code, note]);
    }
    /// Adds someone by ID: they're a friend once they've added yours too.
    /// Answers with `Social`, or an `Error` (`self`, `already-contact`,
    /// `blocked`, `bad-name`, or what's wrong with the ID).
    pub fn add_friend(&self, id: &str, nick: &str) {
        self.call("friendAdd", &[id, nick]);
    }
    /// Each answers with `Social`, or an `Error`.
    pub fn accept(&self, id: &str) {
        self.call("friendAccept", &[id]);
    }
    /// Nothing tells them.
    pub fn decline(&self, id: &str) {
        self.call("friendDecline", &[id]);
    }
    /// Removes a friend, or cancels a request we sent.
    pub fn remove_friend(&self, id: &str) {
        self.call("friendRemove", &[id]);
    }
    pub fn rename_friend(&self, id: &str, nick: &str) {
        self.call("friendRename", &[id, nick]);
    }
    pub fn verify(&self, id: &str, verified: bool) {
        self.call("friendVerify", &[id, if verified { "true" } else { "false" }]);
    }
    pub fn block(&self, id: &str) {
        self.call("block", &[id]);
    }
    pub fn unblock(&self, id: &str) {
        self.call("unblock", &[id]);
    }
    pub fn social(&self) {
        self.call("socialList", &[]);
    }
    /// `online`, `idle`, `dnd` or `invisible`, for the whole account.
    pub fn set_status(&self, status: &str) {
        self.call("statusSet", &[status]);
    }
    /// Pings a friend by ID; answers with `Ping` (then `Social`), or an
    /// `Error` (`not-a-contact`, `unreachable`, `revoked-device`, `offline`).
    pub fn ping_id(&self, id: &str) {
        self.call("netPingId", &[id]);
    }

    /// Sends a test note to one friend's ID, or to several (space-separated)
    /// as a test group; `group` reuses one. Answers with `NoteSent`, or an
    /// `Error` (`not-a-friend`, `no-keys`, `too-long`, `locked`).
    pub fn note(&self, ids: &str, text: &str, group: Option<&str>) {
        match group {
            Some(group) => self.call("e2eeNote", &[ids, text, group]),
            None => self.call("e2eeNote", &[ids, text]),
        }
    }
    /// Shows a link code for a new device; answers with `Link`, then
    /// `LinkConfirm` once the new device has typed it.
    pub fn link_start(&self) {
        self.call("linkStart", &[]);
    }
    /// On a device with no identity: joins the identity showing `code`,
    /// naming this device; answers with `LinkReceived` or `Error`.
    pub fn link_join(&self, code: &str, device_name: &str) {
        self.call("linkJoin", &[code, device_name]);
    }
    /// Yes or no to the device `LinkConfirm` named; yes answers with
    /// `LinkDone`, no with `Done`.
    pub fn link_confirm(&self, yes: bool) {
        self.call("linkConfirm", &[if yes { "true" } else { "false" }]);
    }
    /// Stops a link on either side; answers with `Done`.
    pub fn link_cancel(&self) {
        self.call("linkCancel", &[]);
    }
    /// On the new device: saves the identity under its username and this
    /// device's own password; answers with `Identity` (unlocked) or `Error`.
    pub fn link_finish(&self, username: &str, password: &str) {
        self.call("linkFinish", &[username, password]);
    }
    /// Removes one of our other linked devices by its key; answers with `Identity`.
    pub fn remove_device(&self, key: &str) {
        self.call("deviceRemove", &[key]);
    }

    /// The chat list; answers with `DmChats`.
    pub fn dm_list(&self) {
        self.call("dmList", &[]);
    }
    /// Opens a chat (a friend's ID or a group's ID): its newest page, or the
    /// one before `before`; answers with `DmMessages`.
    pub fn dm_open(&self, chat: &str, before: Option<&str>) {
        self.call("dmOpen", &[chat, before.unwrap_or("")]);
    }
    /// Sends a message, maybe as a reply; answers with `DmMessages`, or an
    /// `Error` (`not-a-friend`, `too-long`, `not-member`, `locked`).
    pub fn dm_send(&self, chat: &str, text: &str, reply_to: Option<&str>) {
        self.call("dmSend", &[chat, text, reply_to.unwrap_or("")]);
    }
    /// Edits one of our messages; answers with `DmMessages` or `Error` (`not-author`).
    pub fn dm_edit(&self, message: &str, text: &str) {
        self.call("dmEdit", &[message, text]);
    }
    /// Deletes a message here, or for everyone (ours only); answers with `DmMessages`.
    pub fn dm_delete(&self, message: &str, everyone: bool) {
        self.call("dmDelete", &[message, if everyone { "true" } else { "false" }]);
    }
    /// Adds or takes back a reaction; answers with `DmMessages`.
    pub fn dm_react(&self, message: &str, emoji: &str, on: bool) {
        self.call("dmReact", &[message, emoji, if on { "true" } else { "false" }]);
    }
    /// Marks a chat read up to a message; answers with `DmChats`.
    pub fn dm_read(&self, chat: &str, up_to: &str) {
        self.call("dmRead", &[chat, up_to]);
    }
    /// Says we're typing in a chat; answers with nothing.
    pub fn dm_typing(&self, chat: &str) {
        self.call("dmTyping", &[chat]);
    }
    /// Makes a group with these friends; answers with `DmChats`, or an
    /// `Error` (`not-a-friend`, `too-many`).
    pub fn dm_group_create(&self, ids: &[String], name: &str) {
        self.call("dmGroupCreate", &[&ids.join(","), name]);
    }
    pub fn dm_group_add(&self, group: &str, id: &str) {
        self.call("dmGroupAdd", &[group, id]);
    }
    /// Only the group's creator can; answers with `DmChats` or `Error` (`not-creator`).
    pub fn dm_group_remove(&self, group: &str, id: &str) {
        self.call("dmGroupRemove", &[group, id]);
    }
    pub fn dm_group_leave(&self, group: &str) {
        self.call("dmGroupLeave", &[group]);
    }
    pub fn dm_group_rename(&self, group: &str, name: &str) {
        self.call("dmGroupRename", &[group, name]);
    }
    /// Sends the file at `path`, with an optional picture preview file;
    /// answers with `DmMessages` or `Error` (`too-long` past 100 MB).
    pub fn dm_send_file(&self, chat: &str, path: &str, preview: Option<&str>) {
        self.call("dmSendFile", &[chat, path, preview.unwrap_or("")]);
    }
    /// Gets a message's file (`index` among its attachments); answers with
    /// `DmFile`, or `Error` (`unavailable`: nobody connected has it).
    pub fn dm_fetch(&self, chat: &str, message: &str, index: usize) {
        self.call("dmFetch", &[chat, message, &index.to_string()]);
    }

    fn call(&self, function: &str, args: &[&str]) {
        let _ = self.commands.send(serde_json::json!({ "fn": function, "args": args }).to_string() + "\n");
    }
}

/// Runs `mesh-node --data <data>` for as long as `events` is open, restarting
/// it when it exits (the same backoff as `whatsapp::run_native`).
pub async fn run(exe: PathBuf, data: PathBuf, events: mpsc::UnboundedSender<Event>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    while !events.is_closed() {
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("--data").arg(&data).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("could not start {}: {err}", exe.display());
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        };
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { return };
        let (commands, mut command_rx) = mpsc::unbounded_channel::<String>();
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        if events.send(Event::Connected(Node { commands })).is_err() {
            return;
        }
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let Some(event) = line_event(&line) else { continue };
                    if events.send(event).is_err() {
                        return;
                    }
                }
                Some(command) = command_rx.recv() => {
                    if stdin.write_all(command.as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = child.kill().await;
        if events.send(Event::Disconnected).is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// A helper stdout line as an event; `None` for the ones Mesh doesn't use yet.
fn line_event(line: &str) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = v["payload"].as_str()?;
    match v["binding"].as_str()? {
        "meshIdentity" => serde_json::from_str(payload).ok().map(Event::Identity),
        "meshPassword" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::Password { ok: v["ok"].as_bool()?, message: v["message"].as_str().unwrap_or_default().to_owned() })
        }
        "meshError" => serde_json::from_str(payload).ok().map(Event::Error),
        "meshSafety" => serde_json::from_str(payload).ok().map(Event::Safety),
        "meshDone" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::Done(v["fn"].as_str()?.to_owned()))
        }
        "meshNet" => serde_json::from_str(payload).ok().map(Event::Net),
        "meshTicket" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::Ticket(v["ticket"].as_str()?.to_owned()))
        }
        "meshPing" => serde_json::from_str(payload).ok().map(Event::Ping),
        "meshCodes" => serde_json::from_str(payload).ok().map(Event::Codes),
        "meshResolved" => serde_json::from_str(payload).ok().map(Event::Resolved),
        "meshSocial" => serde_json::from_str(payload).ok().map(Event::Social),
        "meshNote" => serde_json::from_str(payload).ok().map(Event::Note),
        "meshLink" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::Link { code: v["code"].as_str()?.to_owned(), expires: v["expires"].as_u64()? })
        }
        binding @ ("meshLinkConfirm" | "meshLinkDone") => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            let name = v["name"].as_str()?.to_owned();
            Some(if binding == "meshLinkDone" { Event::LinkDone(name) } else { Event::LinkConfirm(name) })
        }
        "meshLinkReceived" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::LinkReceived(v["username"].as_str()?.to_owned()))
        }
        "meshDmChats" => {
            #[derive(Deserialize)]
            struct Chats {
                chats: Vec<Chat>,
            }
            serde_json::from_str::<Chats>(payload).ok().map(|c| Event::DmChats(c.chats))
        }
        "meshDmMessages" => serde_json::from_str(payload).ok().map(Event::DmMessages),
        "meshDmNotify" => serde_json::from_str(payload).ok().map(Event::DmNotify),
        "meshDmTyping" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            let who = v["who"].as_array()?.iter().filter_map(|w| w.as_str().map(str::to_owned)).collect();
            Some(Event::DmTyping {
                id: v["id"].as_str()?.to_owned(),
                who,
            })
        }
        "meshDmFile" => serde_json::from_str(payload).ok().map(Event::DmFile),
        "meshNoteSent" => {
            let v: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(Event::NoteSent { conversation: v["conversation"].as_str().map(str::to_owned), devices: v["devices"].as_u64()? as usize })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(binding: &str, payload: &str) -> String {
        serde_json::json!({ "binding": binding, "payload": payload }).to_string()
    }

    #[test]
    fn reads_helper_lines() {
        let Some(Event::Identity(locked)) = line_event(&line("meshIdentity", r#"{"state":"locked","keyring":"no Secret Service"}"#)) else {
            panic!("no identity")
        };
        assert_eq!((locked.state, locked.id, locked.keyring.as_deref()), (State::Locked, None, Some("no Secret Service")));

        let open = r#"{"state":"unlocked","id":"mesh1abc","link":"mesh://id/mesh1abc","username":"Brendon","devices":[{"name":"box","key":"ab12","createdAt":5,"thisDevice":true}],"keepUnlocked":false,"keyring":null}"#;
        let Some(Event::Identity(open)) = line_event(&line("meshIdentity", open)) else { panic!("no identity") };
        assert_eq!((open.state, open.username.as_deref(), open.devices[0].this_device, open.devices[0].key.as_str()), (State::Unlocked, Some("Brendon"), true, "ab12"));

        let Some(Event::Password { ok, message }) = line_event(&line("meshPassword", r#"{"ok":false,"message":"Add another word."}"#)) else {
            panic!("no verdict")
        };
        assert_eq!((ok, message.as_str()), (false, "Add another word."));

        let Some(Event::Error(e)) = line_event(&line("meshError", r#"{"fn":"idUnlock","kind":"unlock","message":"can't unlock"}"#)) else {
            panic!("no error")
        };
        assert_eq!((e.func.as_str(), e.kind.as_str()), ("idUnlock", "unlock"));

        let Some(Event::Done(func)) = line_event(&line("meshDone", r#"{"fn":"idExport"}"#)) else { panic!("no done") };
        assert_eq!(func, "idExport");

        let Some(Event::Safety(n)) = line_event(&line("meshSafety", r#"{"id":"mesh1abc","number":"12345 67890","qr":"q"}"#)) else {
            panic!("no safety number")
        };
        assert_eq!(n.number, "12345 67890");

        let Some(Event::Net(net)) = line_event(&line("meshNet", r#"{"state":"online","relay":"https://aps1-1.relay.n0.iroh.link./","direct":true,"connections":1}"#)) else {
            panic!("no net")
        };
        assert_eq!((net.state, net.direct, net.connections), (NetState::Online, true, 1));
        let Some(Event::Net(off)) = line_event(&line("meshNet", r#"{"state":"off","relay":null,"direct":false,"connections":0}"#)) else { panic!("no net") };
        assert_eq!((off.state, off.relay), (NetState::Off, None));

        let Some(Event::Ticket(t)) = line_event(&line("meshTicket", r#"{"ticket":"mesh://node/ab?relay=x"}"#)) else { panic!("no ticket") };
        assert_eq!(t, "mesh://node/ab?relay=x");

        let Some(Event::Ping(pong)) = line_event(&line("meshPing", r#"{"id":"mesh1abc","name":"box","path":"direct","ms":0.7}"#)) else {
            panic!("no pong")
        };
        assert_eq!((pong.id.as_str(), pong.path.as_str(), pong.ms), ("mesh1abc", "direct", 0.7));

        let codes = r#"{"codes":[{"code":"K7QM-3XPA","link":"mesh://code/K7QM-3XPA","createdAt":5,"expiresAt":null,"maxUses":1,"held":true}],"created":"K7QM-3XPA"}"#;
        let Some(Event::Codes(c)) = line_event(&line("meshCodes", codes)) else { panic!("no codes") };
        assert_eq!((c.codes[0].held, c.codes[0].expires_at, c.created.as_deref()), (true, None, Some("K7QM-3XPA")));

        let Some(Event::Resolved(r)) = line_event(&line("meshResolved", r#"{"code":"K7QM-3XPA","id":"mesh1abc","link":"mesh://id/mesh1abc"}"#)) else {
            panic!("no resolve")
        };
        assert_eq!(r.id, "mesh1abc");

        let social = r#"{"fn":"friendAccept","friends":[{"id":"mesh1abc","name":"alex","nick":"Alex","verified":true,"presence":"dnd","seen":9}],
            "incoming":[{"id":"mesh1def","name":"sam","note":"hi","at":5}],"outgoing":[{"id":"mesh1ghi","nick":"","at":6}],"blocked":[],"status":"invisible"}"#;
        let Some(Event::Social(s)) = line_event(&line("meshSocial", social)) else { panic!("no social") };
        assert_eq!((s.answers.as_deref(), s.friends[0].presence.as_str(), s.friends[0].seen), (Some("friendAccept"), "dnd", Some(9)));
        assert_eq!((s.incoming[0].note.as_str(), s.outgoing[0].at, s.blocked.len(), s.status.as_str()), ("hi", 6, 0, "invisible"));
        let unasked = r#"{"fn":null,"friends":[],"incoming":[],"outgoing":[],"blocked":[],"status":"online"}"#;
        let Some(Event::Social(s)) = line_event(&line("meshSocial", unasked)) else { panic!("no social") };
        assert_eq!(s.answers, None);
        let Some(Event::Net(published)) = line_event(&line("meshNet", r#"{"state":"online","relay":null,"direct":false,"connections":0,"published":true}"#)) else {
            panic!("no net")
        };
        assert!(published.published && !off.published && !published.keys);
        let Some(Event::Net(keys)) = line_event(&line("meshNet", r#"{"state":"online","relay":null,"direct":false,"connections":0,"published":true,"keys":true}"#)) else {
            panic!("no net")
        };
        assert!(keys.keys);

        let note = r#"{"from":"mesh1abc","device":"laptop","with":"mesh1def","group":null,"conversation":null,"text":"hi","at":7,"path":"waited"}"#;
        let Some(Event::Note(n)) = line_event(&line("meshNote", note)) else { panic!("no note") };
        assert_eq!((n.from.as_str(), n.with.as_deref(), n.group, n.path.as_str(), n.at), ("mesh1abc", Some("mesh1def"), None, "waited", 7));
        let group = r#"{"from":"mesh1abc","device":null,"with":null,"group":["mesh1abc","mesh1def"],"conversation":"ab12","text":"hey","at":8,"path":"live"}"#;
        let Some(Event::Note(g)) = line_event(&line("meshNote", group)) else { panic!("no note") };
        assert_eq!((g.group.map(|g| g.len()), g.conversation.as_deref(), g.device), (Some(2), Some("ab12"), None));
        let Some(Event::NoteSent { conversation, devices }) = line_event(&line("meshNoteSent", r#"{"fn":"e2eeNote","conversation":null,"devices":3}"#)) else {
            panic!("no sent")
        };
        assert_eq!((conversation, devices), (None, 3));

        let Some(Event::Link { code, expires }) = line_event(&line("meshLink", r#"{"code":"K7QM-3XPA","expires":99}"#)) else { panic!("no link") };
        assert_eq!((code.as_str(), expires), ("K7QM-3XPA", 99));
        let Some(Event::LinkConfirm(name)) = line_event(&line("meshLinkConfirm", r#"{"name":"Kitchen laptop"}"#)) else { panic!("no ask") };
        assert_eq!(name, "Kitchen laptop");
        let Some(Event::LinkDone(name)) = line_event(&line("meshLinkDone", r#"{"name":"Kitchen laptop"}"#)) else { panic!("no done") };
        assert_eq!(name, "Kitchen laptop");
        let Some(Event::LinkReceived(username)) = line_event(&line("meshLinkReceived", r#"{"username":"alice"}"#)) else { panic!("no identity") };
        assert_eq!(username, "alice");

        assert!(line_event(&line("meshParsed", r#"{"kind":"id"}"#)).is_none());
        assert!(line_event("not json").is_none());
    }

    /// The dm events, as the helper writes them (SPEC-dm *Client side*).
    #[test]
    fn dm_events() {
        let chats = r#"{"chats":[{"id":"mesh1bob","kind":"direct","name":"bob","members":["mesh1bob"],"last":1794428700000,"unread":2,"mentions":0},{"id":"ab12","kind":"group","name":"hike","members":["mesh1a","mesh1b","mesh1c"],"last":0,"unread":0,"mentions":0}]}"#;
        let Some(Event::DmChats(chats)) = line_event(&line("meshDmChats", chats)) else {
            panic!("no chats")
        };
        assert_eq!(
            (chats.len(), chats[0].kind, chats[0].unread, chats[0].last),
            (2, ChatKind::Direct, 2, 1794428700000)
        );
        assert_eq!((chats[1].kind, chats[1].name.as_str(), chats[1].members.len()), (ChatKind::Group, "hike", 3));

        let page = r#"{"id":"mesh1bob","more":true,"events":[{"id":"57dd","from":"mesh1me","text":"hi","edited":true,"replyTo":"11aa","attachments":[{"name":"trail.jpg","mime":"image/jpeg","size":300000,"preview":"ffd8"}],"reactions":[{"emoji":"🎉","who":["mesh1bob"]}],"late":false,"time":1794428384000,"sent":true}]}"#;
        let Some(Event::DmMessages(page)) = line_event(&line("meshDmMessages", page)) else {
            panic!("no page")
        };
        assert_eq!((page.id.as_str(), page.more, page.events.len()), ("mesh1bob", true, 1));
        let m = &page.events[0];
        assert_eq!(
            (m.id.as_str(), m.from.as_str(), m.text.as_str(), m.edited, m.reply_to.as_deref(), m.sent, m.time),
            ("57dd", "mesh1me", "hi", true, Some("11aa"), true, 1794428384000)
        );
        assert_eq!(
            (m.attachments[0].name.as_str(), m.attachments[0].size, m.attachments[0].preview.as_deref()),
            ("trail.jpg", 300000, Some("ffd8"))
        );
        assert_eq!((m.reactions[0].emoji.as_str(), m.reactions[0].who.len()), ("🎉", 1));
        let bare = r#"{"id":"ab12","more":false,"events":[{"id":"01","from":"mesh1b","text":"","edited":false,"replyTo":null,"attachments":[{"name":"a","mime":"text/plain","size":1,"preview":null}],"reactions":[],"late":true,"time":5,"sent":false}]}"#;
        let Some(Event::DmMessages(bare)) = line_event(&line("meshDmMessages", bare)) else {
            panic!("no page")
        };
        assert_eq!(
            (
                bare.events[0].reply_to.as_deref(),
                bare.events[0].late,
                bare.events[0].attachments[0].preview.as_deref()
            ),
            (None, true, None)
        );

        let Some(Event::DmNotify(n)) = line_event(&line("meshDmNotify", r#"{"id":"ab12","from":"mesh1b","text":"hello"}"#)) else {
            panic!("no notify")
        };
        assert_eq!((n.id.as_str(), n.from.as_str(), n.text.as_str()), ("ab12", "mesh1b", "hello"));
        let Some(Event::DmTyping { id, who }) = line_event(&line("meshDmTyping", r#"{"id":"mesh1bob","who":["mesh1bob"]}"#)) else {
            panic!("no typing")
        };
        assert_eq!((id.as_str(), who), ("mesh1bob", vec!["mesh1bob".to_owned()]));
        let Some(Event::DmFile(f)) = line_event(&line(
            "meshDmFile",
            r#"{"id":"ab12","message":"57dd","index":0,"path":"/data/files/57dd-0/map.png"}"#,
        )) else {
            panic!("no file")
        };
        assert_eq!((f.message.as_str(), f.index, f.path.as_str()), ("57dd", 0, "/data/files/57dd-0/map.png"));
    }
}
