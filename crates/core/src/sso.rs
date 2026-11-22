//! EVE SSO v2 with PKCE (no client secret).

use crate::config;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum SsoError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    /// EVE rejected the request; `error` is the OAuth error code (invalid_grant, invalid_client, …).
    #[error("EVE login rejected the request: {error} {description}")]
    Rejected { status: u16, error: String, description: String },
    #[error("unexpected response: {0}")]
    Invalid(String),
}

impl SsoError {
    /// Whether retrying later can't help (revoked token or wrong application).
    pub fn is_permanent(&self) -> bool {
        match self {
            SsoError::Rejected { error, description, .. } => {
                matches!(error.as_str(), "invalid_grant" | "invalid_client" | "unauthorized_client")
                    || description.contains("does not match the client")
            }
            _ => false,
        }
    }
    pub fn is_client_problem(&self) -> bool {
        match self {
            SsoError::Rejected { error, description, .. } => {
                matches!(error.as_str(), "invalid_client" | "unauthorized_client")
                    || description.contains("does not match the client")
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut st = [0u8; 12];
        rand::rng().fill_bytes(&mut st);
        Pkce { verifier, challenge, state: URL_SAFE_NO_PAD.encode(st) }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

pub fn authorize_url(client_id: &str, callback: &str, scopes: &[&str], pkce: &Pkce) -> String {
    let mut url = url::Url::parse(&format!("{}/authorize", config::SSO_BASE)).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", callback)
        .append_pair("client_id", client_id)
        .append_pair("scope", &scopes.join(" "))
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pkce.state);
    url.to_string()
}

/// Pulls `code` out of the callback address the browser landed on (checking `state` when given).
pub fn code_from_callback(callback_url: &str, expected_state: Option<&str>) -> Result<String, String> {
    let url = url::Url::parse(callback_url.trim()).map_err(|_| "That doesn't look like a web address.".to_string())?;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.to_string()),
            "state" => state = Some(v.to_string()),
            "error" => error = Some(v.to_string()),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(format!("EVE login reported: {e}"));
    }
    if let (Some(exp), Some(got)) = (expected_state, state.as_deref()) {
        if exp != got {
            return Err("This address is from a different login attempt.".into());
        }
    }
    code.ok_or_else(|| "The address has no login code in it.".into())
}

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub expiry: DateTime<Utc>,
    pub refresh_token: String,
}

#[derive(Debug, Clone)]
pub struct TokenClaims {
    pub character_id: i64,
    pub character_name: String,
    pub owner_hash: String,
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Deserialize, Default)]
struct ErrorResponse {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)], old_refresh: &str) -> Result<Tokens, SsoError> {
    let res = http
        .post(format!("{}/token", config::SSO_BASE))
        .header("User-Agent", config::user_agent())
        .form(form)
        .send()
        .await?;
    let status = res.status();
    let body = res.text().await?;
    if !status.is_success() {
        let err: ErrorResponse = serde_json::from_str(&body).unwrap_or_default();
        return Err(SsoError::Rejected { status: status.as_u16(), error: err.error, description: err.error_description });
    }
    let t: TokenResponse = serde_json::from_str(&body).map_err(|e| SsoError::Invalid(e.to_string()))?;
    Ok(Tokens {
        access_token: t.access_token,
        expiry: Utc::now() + Duration::seconds(t.expires_in),
        refresh_token: t.refresh_token.filter(|s| !s.is_empty()).unwrap_or_else(|| old_refresh.to_string()),
    })
}

pub async fn exchange_code(http: &reqwest::Client, client_id: &str, code: &str, pkce: &Pkce) -> Result<Tokens, SsoError> {
    token_request(
        http,
        &[("grant_type", "authorization_code"), ("client_id", client_id), ("code", code), ("code_verifier", &pkce.verifier)],
        "",
    )
    .await
}

pub async fn refresh(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<Tokens, SsoError> {
    token_request(
        http,
        &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh_token)],
        refresh_token,
    )
    .await
}

pub async fn revoke(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<(), SsoError> {
    http.post(config::SSO_REVOKE_URL)
        .header("User-Agent", config::user_agent())
        .form(&[("token_type_hint", "refresh_token"), ("token", refresh_token), ("client_id", client_id)])
        .send()
        .await?;
    Ok(())
}

/// Reads the character and scopes from the access token (a JWT; its signature isn't checked, as the token came
/// straight from EVE over TLS).
pub fn decode_claims(access_token: &str) -> Result<TokenClaims, SsoError> {
    let payload = access_token.split('.').nth(1).ok_or_else(|| SsoError::Invalid("token is not a JWT".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|e| SsoError::Invalid(e.to_string()))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| SsoError::Invalid(e.to_string()))?;
    let sub = v["sub"].as_str().unwrap_or_default();
    let character_id = sub.rsplit(':').next().and_then(|s| s.parse().ok()).ok_or_else(|| SsoError::Invalid("no character id".into()))?;
    let scopes = match &v["scp"] {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a.iter().filter_map(|s| s.as_str().map(String::from)).collect(),
        _ => vec![],
    };
    Ok(TokenClaims {
        character_id,
        character_name: v["name"].as_str().unwrap_or_default().to_string(),
        owner_hash: v["owner"].as_str().unwrap_or_default().to_string(),
        scopes,
    })
}

/// For a loopback callback (`http://127.0.0.1:<port>/…` or `http://localhost:<port>/…`), the port to listen on.
pub fn loopback_port(callback: &str) -> Option<u16> {
    let url = url::Url::parse(callback).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    match url.host_str()? {
        "127.0.0.1" | "localhost" | "[::1]" => url.port(),
        _ => None,
    }
}

/// Waits for the browser to hit the loopback callback and returns the full callback URL it requested.
pub async fn wait_for_loopback(port: u16) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
        if !path.contains("code=") && !path.contains("error=") {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            continue;
        }
        let body = "<!doctype html><meta charset=utf-8><title>PodStack</title><body style=\"font-family:sans-serif;background:#141218;color:#e6e0e9;display:grid;place-items:center;height:100vh;margin:0\"><div><h2>Logged in</h2><p>You can close this tab and return to PodStack.</p></div>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        return Ok(format!("http://127.0.0.1:{port}{path}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let p = Pkce::new();
        assert_eq!(p.challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes())));
        assert_eq!(p.verifier.len(), 43);
    }

    #[test]
    fn parses_callback() {
        let code = code_from_callback("https://localhost/callback?code=abc123&state=xyz", Some("xyz")).unwrap();
        assert_eq!(code, "abc123");
        assert!(code_from_callback("https://localhost/callback?code=a&state=other", Some("xyz")).is_err());
    }

    #[test]
    fn decodes_jwt_claims() {
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"sub":"CHARACTER:EVE:2112625428","name":"Some Pilot","owner":"hash=","scp":["esi-skills.read_skills.v1"]}"#,
        );
        let c = decode_claims(&format!("x.{payload}.y")).unwrap();
        assert_eq!(c.character_id, 2112625428);
        assert_eq!(c.character_name, "Some Pilot");
        assert_eq!(c.scopes, vec!["esi-skills.read_skills.v1"]);
    }

    #[test]
    fn loopback_detection() {
        assert_eq!(loopback_port("http://127.0.0.1:8765/callback"), Some(8765));
        assert_eq!(loopback_port("https://localhost/callback"), None);
    }
}
