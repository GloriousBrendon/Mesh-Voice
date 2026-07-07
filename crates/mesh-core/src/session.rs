use matrix_sdk::AuthSession;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use matrix_sdk::{SessionMeta, SessionTokens};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::MeshResult;

/// Everything needed to restore a logged-in client without re-authenticating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub homeserver: String,
    pub user_id: OwnedUserId,
    pub device_id: OwnedDeviceId,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl Session {
    pub fn into_matrix_session(self) -> AuthSession {
        AuthSession::Matrix(MatrixSession {
            meta: SessionMeta {
                user_id: self.user_id,
                device_id: self.device_id,
            },
            tokens: SessionTokens {
                access_token: self.access_token,
                refresh_token: self.refresh_token,
            },
        })
    }
}

impl From<MatrixSession> for Session {
    fn from(session: MatrixSession) -> Self {
        Self {
            // Filled in by callers that know the homeserver; see `SessionStore::save`.
            homeserver: String::new(),
            user_id: session.meta.user_id,
            device_id: session.meta.device_id,
            access_token: session.tokens.access_token,
            refresh_token: session.tokens.refresh_token,
        }
    }
}

/// Reads and writes the single session file kept in a client's data directory.
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            path: data_dir.as_ref().join("session.json"),
        }
    }

    pub fn save(&self, session: &Session) -> MeshResult<()> {
        let json = serde_json::to_vec_pretty(session)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    pub fn load(&self) -> MeshResult<Option<Session>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    pub fn clear(&self) -> MeshResult<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}
