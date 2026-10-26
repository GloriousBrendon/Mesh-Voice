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
    /// The helper exited; it's restarted by itself.
    Disconnected,
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

        let open = r#"{"state":"unlocked","id":"mesh1abc","link":"mesh://id/mesh1abc","username":"Brendon","devices":[{"name":"box","createdAt":5,"thisDevice":true}],"keepUnlocked":false,"keyring":null}"#;
        let Some(Event::Identity(open)) = line_event(&line("meshIdentity", open)) else { panic!("no identity") };
        assert_eq!((open.state, open.username.as_deref(), open.devices[0].this_device), (State::Unlocked, Some("Brendon"), true));

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

        assert!(line_event(&line("meshCodes", r#"{"codes":[]}"#)).is_none());
        assert!(line_event("not json").is_none());
    }
}
