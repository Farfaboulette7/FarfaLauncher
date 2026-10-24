//! Connexion à RomM : vérification du serveur, flux d'autorisation d'appareil
//! (principal), échange de code d'appairage et collage de token (secours).
//! Détails et justification : docs/romm-api.md §1.

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::client::{check_status, parse_json, RommClient};
use crate::error::{log_err, AppError, AppResult};

/// Scopes demandés à RomM (docs/romm-api.md §1.5).
pub const REQUIRED_SCOPES: &[&str] = &[
    "me.read",
    "platforms.read",
    "roms.read",
    "roms.user.read",
    "roms.user.write",
    "devices.read",
    "collections.read",
    "collections.write",
    "assets.read",
    "assets.write",
];

pub const TOKEN_PREFIX: &str = "rmm_";

#[derive(Deserialize)]
struct Heartbeat {
    #[serde(rename = "SYSTEM")]
    system: HeartbeatSystem,
}

#[derive(Deserialize)]
struct HeartbeatSystem {
    #[serde(rename = "VERSION")]
    version: String,
}

/// Vérifie qu'un serveur RomM répond (route publique) et renvoie sa version.
pub async fn check_server(client: &RommClient) -> AppResult<String> {
    let resp = client
        .get("api/heartbeat")
        .send()
        .await
        .map_err(|e| log_err(AppError::ServerUnreachable, "heartbeat", e))?;
    if !resp.status().is_success() {
        tracing::warn!("heartbeat: HTTP {}", resp.status());
        return Err(AppError::NotRomm);
    }
    let hb: Heartbeat = resp.json().await.map_err(|e| log_err(AppError::NotRomm, "heartbeat JSON", e))?;
    Ok(hb.system.version)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub role: Option<String>,
}

pub async fn me(client: &RommClient) -> AppResult<Me> {
    client.get_json("api/users/me").await
}

// ---------- Flux device (/api/auth/device) ----------

#[derive(Serialize)]
struct DeviceInitPayload<'a> {
    client_device_identifier: &'a str,
    name: &'a str,
    client: &'a str,
    platform: &'a str,
    client_version: &'a str,
    requested_scopes: &'a [&'a str],
}

#[derive(Debug, Deserialize)]
pub struct DeviceInitResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_path_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn device_init(client: &RommClient, device_id: &str, device_name: &str) -> AppResult<DeviceInitResponse> {
    let payload = DeviceInitPayload {
        client_device_identifier: device_id,
        name: device_name,
        client: "FarfaLauncher",
        platform: std::env::consts::OS,
        client_version: env!("CARGO_PKG_VERSION"),
        requested_scopes: REQUIRED_SCOPES,
    };
    let resp = client.send(client.post("api/auth/device/init", &payload)).await?;
    parse_json(resp).await
}

#[derive(Debug)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Denied,
    Expired,
    Approved { token: String, scopes: Vec<String> },
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    access_token: String,
    #[serde(default)]
    scopes: Vec<String>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    detail: serde_json::Value,
}

pub async fn device_poll(client: &RommClient, device_code: &str) -> AppResult<DevicePoll> {
    let resp = client
        .post("api/auth/device/token", &serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .map_err(|e| log_err(AppError::ServerUnreachable, "device token poll", e))?;
    match resp.status() {
        StatusCode::OK => {
            let t: DeviceTokenResponse = parse_json(resp).await?;
            Ok(DevicePoll::Approved { token: t.access_token, scopes: t.scopes })
        }
        StatusCode::TOO_MANY_REQUESTS => Ok(DevicePoll::SlowDown),
        StatusCode::BAD_REQUEST => {
            let detail = resp
                .json::<ErrorDetail>()
                .await
                .ok()
                .and_then(|d| d.detail.as_str().map(str::to_string))
                .unwrap_or_default();
            Ok(match detail.as_str() {
                "authorization_pending" => DevicePoll::Pending,
                "slow_down" => DevicePoll::SlowDown,
                "access_denied" => DevicePoll::Denied,
                _ => DevicePoll::Expired,
            })
        }
        _ => {
            check_status(resp).await?;
            Err(AppError::Server)
        }
    }
}

// ---------- Secours : code d'appairage (/api/client-tokens/exchange) ----------

#[derive(Deserialize)]
struct ExchangeResponse {
    raw_token: String,
}

pub async fn exchange_pair_code(client: &RommClient, code: &str) -> AppResult<String> {
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    if code.len() != 8 {
        return Err(AppError::PairCodeInvalid);
    }
    let resp = client
        .post("api/client-tokens/exchange", &serde_json::json!({ "code": code }))
        .send()
        .await
        .map_err(|e| log_err(AppError::ServerUnreachable, "échange du code", e))?;
    match resp.status() {
        StatusCode::NOT_FOUND => Err(AppError::PairCodeInvalid),
        StatusCode::FORBIDDEN => Err(AppError::Forbidden),
        _ => {
            let resp = check_status(resp).await?;
            Ok(parse_json::<ExchangeResponse>(resp).await?.raw_token)
        }
    }
}

pub fn validate_token_format(token: &str) -> AppResult<String> {
    let token = token.trim();
    if !token.starts_with(TOKEN_PREFIX) || token.len() < 12 || token.chars().any(char::is_whitespace) {
        return Err(AppError::TokenFormat);
    }
    Ok(token.to_string())
}
