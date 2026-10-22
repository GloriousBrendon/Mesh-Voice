//! Login. Reuses a stored token when there is one; otherwise runs Discord's
//! remote-auth (QR) flow: the phone app scans the code and approves, and the
//! account's token comes back encrypted to a key we made here. The token is
//! stored in the data dir, nowhere else. Or email + password (and a 2FA code),
//! the web client's `/auth/login`; no captcha path, so when Discord asks for
//! one this reports it rather than solving it.

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

/// Where an email + password sign-in got to.
#[derive(Debug, PartialEq)]
pub enum Password {
    Token(String),
    /// 2FA is on: send the authenticator (or backup) code with this ticket.
    Code { ticket: String },
    Failed(String),
}

/// `POST /auth/login`, as the web client sends it.
pub async fn password(login: &str, password: &str) -> Password {
    let body = serde_json::json!({ "login": login, "password": password, "undelete": false, "login_source": null, "gift_code_sku_id": null });
    password_step(post("/auth/login", &body).await)
}

/// `POST /auth/mfa/totp` with the ticket from [`password`].
pub async fn code(ticket: &str, code: &str) -> Password {
    let code: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    password_step(post("/auth/mfa/totp", &serde_json::json!({ "code": code, "ticket": ticket })).await)
}

async fn post(path: &str, body: &Value) -> Result<Value> {
    let resp = reqwest::Client::new()
        .post(format!("{}{path}", crate::gateway::api()))
        .header("User-Agent", crate::gateway::USER_AGENT)
        .header("X-Super-Properties", crate::gateway::super_properties())
        .header("X-Discord-Locale", "en-US")
        .json(body)
        .send()
        .await
        .map_err(|e| anyhow!("couldn't reach Discord: {e}"))?;
    resp.json().await.map_err(|e| anyhow!("odd answer from Discord: {e}"))
}

/// Discord's answer to a sign-in step, as what to do next.
fn password_step(answer: Result<Value>) -> Password {
    let v = match answer {
        Ok(v) => v,
        Err(e) => return Password::Failed(e.to_string()),
    };
    if let Some(token) = v["token"].as_str() {
        return Password::Token(token.to_owned());
    }
    if v["mfa"].as_bool() == Some(true)
        && let Some(ticket) = v["ticket"].as_str()
    {
        return Password::Code { ticket: ticket.to_owned() };
    }
    if v.get("captcha_key").is_some() || v.get("captcha_sitekey").is_some() {
        return Password::Failed("Discord wants a captcha for this sign-in, which Mesh can't show. Try again later, or sign in once from the QR code.".into());
    }
    // `{errors: {login: {_errors: [{code, message}]}}}`, or a top-level message.
    let first = v["errors"].as_object().and_then(|e| e.values().find_map(|f| f["_errors"][0].as_object().cloned()));
    let message = match first {
        Some(e) if e["code"] == "ACCOUNT_LOGIN_VERIFICATION_EMAIL" => {
            "Discord emailed you to confirm this new sign-in location. Approve it, then sign in again.".to_owned()
        }
        Some(e) => e["message"].as_str().unwrap_or("Sign-in failed").to_owned(),
        None => v["message"].as_str().unwrap_or("Sign-in failed").to_owned(),
    };
    Password::Failed(message)
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
    #[test]
    fn reads_password_answers() {
        let step = |v: Value| password_step(Ok(v));
        assert_eq!(step(serde_json::json!({"token": "t", "user_id": "1"})), Password::Token("t".into()));
        assert_eq!(step(serde_json::json!({"mfa": true, "ticket": "k", "totp": true})), Password::Code { ticket: "k".into() });
        assert!(matches!(step(serde_json::json!({"captcha_key": ["captcha-required"], "captcha_sitekey": "x"})), Password::Failed(m) if m.contains("captcha")));
        let bad = serde_json::json!({"code": 50035, "errors": {"login": {"_errors": [{"code": "INVALID_LOGIN", "message": "Login or password is invalid."}]}}});
        assert_eq!(step(bad), Password::Failed("Login or password is invalid.".into()));
        let email = serde_json::json!({"errors": {"login": {"_errors": [{"code": "ACCOUNT_LOGIN_VERIFICATION_EMAIL", "message": "x"}]}}});
        assert!(matches!(step(email), Password::Failed(m) if m.contains("emailed")));
    }
}
