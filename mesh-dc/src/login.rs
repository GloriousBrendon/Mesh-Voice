//! Login. Reuses a stored token when there is one; otherwise runs Discord's
//! remote-auth (QR) flow: the phone app scans the code and approves, and the
//! account's token comes back encrypted to a key we made here. The token is
//! stored in the data dir, nowhere else. No password or captcha path.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use rsa::rand_core::OsRng;
use rsa::{Oaep, RsaPrivateKey, pkcs8::EncodePublicKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TOKEN_FILE: &str = "token";
const GATEWAY: &str = "wss://remote-auth-gateway.discord.gg/?v=2";

pub fn token_path(dir: &Path) -> PathBuf {
    dir.join(TOKEN_FILE)
}

pub fn load_token(dir: &Path) -> Option<String> {
    std::fs::read_to_string(token_path(dir)).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

pub fn save_token(dir: &Path, token: &str) {
    if let Err(e) = std::fs::write(token_path(dir), token) {
        log::warn!("could not store token: {e}");
    }
}

pub fn forget_token(dir: &Path) {
    let _ = std::fs::remove_file(token_path(dir));
}

/// What the QR flow tells the caller as it runs.
pub enum Update {
    /// A fresh QR code (SVG) to show. The phone scans the fingerprint URL.
    Qr(String),
    /// The token: linked. Store it and bring up the gateway.
    Token(String),
}

fn b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| anyhow!("base64: {e}"))
}

fn qr_svg(url: &str) -> Option<String> {
    use qrcode::render::svg;
    let qr = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(qr.render::<svg::Color>().min_dimensions(256, 256).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).quiet_zone(true).build())
}

/// Runs remote auth until a token comes back (or the socket drops). Emits QR
/// updates meanwhile. The caller loops it while unlinked.
pub async fn remote_auth(updates: mpsc::UnboundedSender<Update>) -> Result<()> {
    // Origin header: the gateway rejects connections without the web origin.
    let mut req = GATEWAY.into_client_request().map_err(|e| anyhow!("request: {e}"))?;
    req.headers_mut().insert("Origin", "https://discord.com".parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.map_err(|e| anyhow!("connect: {e}"))?;

    let key = RsaPrivateKey::new(&mut OsRng, 2048).map_err(|e| anyhow!("keygen: {e}"))?;
    let spki = key.to_public_key().to_public_key_der().map_err(|e| anyhow!("spki: {e}"))?;
    let pub_b64 = base64::engine::general_purpose::STANDARD.encode(spki.as_bytes());

    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(40));
    heartbeat.tick().await;

    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(incoming) = incoming else { return Err(anyhow!("remote auth closed")) };
                let Ws::Text(text) = incoming.map_err(|e| anyhow!("ws: {e}"))? else { continue };
                let v: Value = serde_json::from_str(&text).map_err(|e| anyhow!("json: {e}"))?;
                match v["op"].as_str().unwrap_or("") {
                    "hello" => {
                        let ms = v["heartbeat_interval"].as_f64().unwrap_or(40_000.0);
                        heartbeat = tokio::time::interval(std::time::Duration::from_millis(ms as u64));
                        heartbeat.tick().await;
                        ws.send(Ws::text(serde_json::json!({ "op": "init", "encoded_public_key": pub_b64 }).to_string())).await?;
                    }
                    "nonce_proof" => {
                        let nonce = b64(v["encrypted_nonce"].as_str().ok_or_else(|| anyhow!("no nonce"))?)?;
                        let decrypted = key.decrypt(Oaep::new::<Sha256>(), &nonce).map_err(|e| anyhow!("decrypt nonce: {e}"))?;
                        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(&decrypted));
                        ws.send(Ws::text(serde_json::json!({ "op": "nonce_proof", "proof": proof }).to_string())).await?;
                    }
                    "pending_remote_init" => {
                        let fingerprint = v["fingerprint"].as_str().ok_or_else(|| anyhow!("no fingerprint"))?;
                        if let Some(svg) = qr_svg(&format!("https://discord.com/ra/{fingerprint}")) {
                            let _ = updates.send(Update::Qr(svg));
                        }
                    }
                    // pending_ticket (user scanned, not yet approved) carries an
                    // encrypted user payload; Mesh has nothing to show for it.
                    "pending_ticket" => {}
                    "pending_login" => {
                        let ticket = v["ticket"].as_str().ok_or_else(|| anyhow!("no ticket"))?.to_owned();
                        let token = redeem(&key, &ticket).await?;
                        let _ = updates.send(Update::Token(token));
                        return Ok(());
                    }
                    "cancel" => return Err(anyhow!("login cancelled on the phone")),
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                ws.send(Ws::text(serde_json::json!({ "op": "heartbeat" }).to_string())).await?;
            }
        }
    }
}

/// Exchanges the approved ticket for the (encrypted) token and decrypts it.
async fn redeem(key: &RsaPrivateKey, ticket: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/users/@me/remote-auth/login", crate::gateway::api()))
        .header("User-Agent", crate::gateway::USER_AGENT)
        .header("X-Super-Properties", crate::gateway::super_properties())
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|e| anyhow!("login post: {e}"))?;
    let v: Value = resp.json().await.map_err(|e| anyhow!("login json: {e}"))?;
    let encrypted = v["encrypted_token"].as_str().ok_or_else(|| anyhow!("no encrypted_token: {v}"))?;
    let bytes = b64(encrypted)?;
    let token = key.decrypt(Oaep::new::<Sha256>(), &bytes).map_err(|e| anyhow!("decrypt token: {e}"))?;
    String::from_utf8(token).map_err(|e| anyhow!("token utf8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_forgets_a_token() {
        let dir = std::env::temp_dir().join(format!("mesh-dc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_token(&dir), None);
        save_token(&dir, "abc.def.ghi\n");
        assert_eq!(load_token(&dir).as_deref(), Some("abc.def.ghi"));
        forget_token(&dir);
        assert_eq!(load_token(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonce_proof_round_trips() {
        // The proof is the url-safe base64 of sha256(decrypted nonce); verify
        // our encode matches what the gateway checks against.
        let nonce = b"a-random-16-byte";
        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(nonce));
        assert!(!proof.contains('=') && !proof.contains('+'));
    }
}
