//! On-disk cache for avatars and media, so reopening Mesh shows pictures
//! straight away. Keyed by URL minus its query string: signed CDN params
//! (`oh=`, `ex=`…) rotate on their own, but a changed profile picture gets a
//! new path (Discord's avatar hash, WhatsApp's file id), so it misses here and
//! downloads fresh while the old file ages out.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Kind {
    Avatar,
    Media,
}

impl Kind {
    fn dir(self) -> PathBuf {
        crate::data_dir().join("cache").join(match self {
            Kind::Avatar => "avatars",
            Kind::Media => "media",
        })
    }

    /// Avatars keep a week (a backstop for a picture changed under the same
    /// URL); photos and videos just the day.
    fn max_age(self) -> Duration {
        Duration::from_secs(match self {
            Kind::Avatar => 7 * 24 * 3600,
            Kind::Media => 24 * 3600,
        })
    }
}

/// The cache file name for `key`.
fn name(key: &str) -> String {
    let stable = if key.starts_with("http") { key.split('?').next().unwrap_or(key) } else { key };
    // ponytail: std's SipHash isn't promised stable across Rust releases; a
    // toolchain bump just re-downloads everything once.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stable.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn get(kind: Kind, key: &str) -> Option<Vec<u8>> {
    std::fs::read(kind.dir().join(name(key))).ok()
}

pub fn put(kind: Kind, key: &str, bytes: &[u8]) {
    let dir = kind.dir();
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(name(key)), bytes)) {
        tracing::debug!("cache write: {err}");
    }
}

/// A saved attachment for `key`. Attachments keep their filename
/// (`<hash>-<name>`) so the OS knows what opens them.
pub fn file(key: &str) -> Option<PathBuf> {
    let prefix = format!("{}-", name(key));
    std::fs::read_dir(Kind::Media.dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)))
}

pub fn put_file(key: &str, filename: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = Kind::Media.dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}-{filename}", name(key)));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Deletes files past their kind's age. Run once at startup.
pub fn prune() {
    for kind in [Kind::Avatar, Kind::Media] {
        let Ok(entries) = std::fs::read_dir(kind.dir()) else { continue };
        for entry in entries.flatten() {
            let expired = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > kind.max_age());
            if expired {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_rotating_query_but_not_path() {
        let a = "https://pps.whatsapp.net/v/t61/123_n.jpg?oh=aa&oe=1";
        let b = "https://pps.whatsapp.net/v/t61/123_n.jpg?oh=bb&oe=2";
        let changed = "https://pps.whatsapp.net/v/t61/456_n.jpg?oh=aa&oe=1";
        assert_eq!(name(a), name(b));
        assert_ne!(name(a), name(changed));
        // Non-URL keys (wa-msg:, mxc ids) are used whole.
        assert_ne!(name("wa-msg:x?1"), name("wa-msg:x?2"));
    }
}
