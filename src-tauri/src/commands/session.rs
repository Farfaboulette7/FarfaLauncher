//! Commandes de connexion / déconnexion. Le token ne quitte jamais le côté
//! Rust : l'interface ne reçoit que le nom d'utilisateur et l'état.

use serde::Serialize;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::romm::auth::{self, DevicePoll};
use crate::romm::client::{normalize_server_url, RommClient};
use crate::secrets;
use crate::state::{AppState, PendingDevice};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub server_url: Option<String>,
    pub connected: bool,
    /// Connecté mais serveur injoignable : on affiche le cache.
    pub offline: bool,
    pub username: Option<String>,
    /// Raison d'une déconnexion forcée (token révoqué, compte supprimé…).
    pub notice: Option<String>,
}

impl SessionInfo {
    fn disconnected(server_url: Option<String>, notice: Option<String>) -> Self {
        Self { server_url, connected: false, offline: false, username: None, notice }
    }
}

/// Au démarrage : reprend la session à partir du token du trousseau.
#[tauri::command]
pub async fn session_status(state: State<'_, AppState>) -> AppResult<SessionInfo> {
    let Some(server_url) = state.settings().server_url else {
        return Ok(SessionInfo::disconnected(None, None));
    };
    let Some(token) = secrets::load_token(&server_url)? else {
        return Ok(SessionInfo::disconnected(Some(server_url), None));
    };
    let client = RommClient::new(&server_url, Some(token))?;
    match auth::me(&client).await {
        Ok(me) => {
            state.set_client(Some(client));
            Ok(SessionInfo { server_url: Some(server_url), connected: true, offline: false, username: Some(me.username), notice: None })
        }
        Err(e @ (AppError::AuthInvalid | AppError::Forbidden)) => {
            tracing::warn!("token refusé au démarrage ({}), suppression", e.code());
            secrets::delete_token(&server_url)?;
            state.set_client(None);
            Ok(SessionInfo::disconnected(Some(server_url), Some(AppError::AuthInvalid.to_string())))
        }
        Err(AppError::ServerUnreachable | AppError::Server) => {
            state.set_client(Some(client));
            Ok(SessionInfo { server_url: Some(server_url), connected: true, offline: true, username: None, notice: None })
        }
        Err(e) => Err(e),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub server_url: String,
    pub version: String,
}

/// Enregistre l'adresse du serveur après avoir vérifié que c'est bien RomM.
#[tauri::command]
pub async fn set_server(state: State<'_, AppState>, url: String) -> AppResult<ServerInfo> {
    let server_url = normalize_server_url(&url)?;
    let version = auth::check_server(&RommClient::new(&server_url, None)?).await?;
    tracing::info!("serveur RomM {version} configuré");
    state.update_settings(|s| s.server_url = Some(server_url.clone()))?;
    state.set_client(None);
    Ok(ServerInfo { server_url, version })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthStart {
    pub user_code: String,
    pub verification_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[tauri::command]
pub async fn start_device_auth(state: State<'_, AppState>) -> AppResult<DeviceAuthStart> {
    let server_url = state.server_url()?;
    let client = RommClient::new(&server_url, None)?;
    let name = format!("FarfaLauncher — {}", device_name());
    let init = auth::device_init(&client, &state.settings().device_id, &name).await?;
    *state.pending_device.lock().unwrap() = Some(PendingDevice { server_url: server_url.clone(), device_code: init.device_code });
    Ok(DeviceAuthStart {
        user_code: init.user_code,
        verification_url: client.url(&init.verification_path_complete),
        interval: init.interval,
        expires_in: init.expires_in,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum DeviceAuthPoll {
    Pending,
    SlowDown,
    Denied,
    Expired,
    Approved { session: SessionInfo },
}

#[tauri::command]
pub async fn poll_device_auth(state: State<'_, AppState>) -> AppResult<DeviceAuthPoll> {
    let (server_url, device_code) = match state.pending_device.lock().unwrap().as_ref() {
        Some(p) => (p.server_url.clone(), p.device_code.clone()),
        None => return Ok(DeviceAuthPoll::Expired),
    };
    let client = RommClient::new(&server_url, None)?;
    Ok(match auth::device_poll(&client, &device_code).await? {
        DevicePoll::Pending => DeviceAuthPoll::Pending,
        DevicePoll::SlowDown => DeviceAuthPoll::SlowDown,
        DevicePoll::Denied => {
            state.pending_device.lock().unwrap().take();
            DeviceAuthPoll::Denied
        }
        DevicePoll::Expired => {
            state.pending_device.lock().unwrap().take();
            DeviceAuthPoll::Expired
        }
        DevicePoll::Approved { token, scopes } => {
            state.pending_device.lock().unwrap().take();
            let missing: Vec<_> = auth::REQUIRED_SCOPES.iter().filter(|s| !scopes.iter().any(|g| g == *s)).collect();
            if !missing.is_empty() {
                tracing::warn!("scopes non accordés: {missing:?}");
            }
            DeviceAuthPoll::Approved { session: finish_login(&state, &server_url, token).await? }
        }
    })
}

#[tauri::command]
pub fn cancel_device_auth(state: State<'_, AppState>) {
    state.pending_device.lock().unwrap().take();
}

#[tauri::command]
pub async fn login_with_pair_code(state: State<'_, AppState>, code: String) -> AppResult<SessionInfo> {
    let server_url = state.server_url()?;
    let token = auth::exchange_pair_code(&RommClient::new(&server_url, None)?, &code).await?;
    finish_login(&state, &server_url, token).await
}

#[tauri::command]
pub async fn login_with_token(state: State<'_, AppState>, token: String) -> AppResult<SessionInfo> {
    let server_url = state.server_url()?;
    let token = auth::validate_token_format(&token)?;
    finish_login(&state, &server_url, token).await
}

/// Valide le token auprès du serveur puis le range dans le trousseau.
async fn finish_login(state: &AppState, server_url: &str, token: String) -> AppResult<SessionInfo> {
    let client = RommClient::new(server_url, Some(token.clone()))?;
    let me = auth::me(&client).await.map_err(|e| match e {
        AppError::AuthInvalid => AppError::TokenRejected,
        other => other,
    })?;
    secrets::store_token(server_url, &token)?;
    state.set_client(Some(client));
    tracing::info!("connecté en tant que {}", me.username);
    Ok(SessionInfo { server_url: Some(server_url.to_string()), connected: true, offline: false, username: Some(me.username), notice: None })
}

/// Oublie le token local. (Il reste révocable côté RomM : Paramètres →
/// Appareils / Client API Tokens.)
#[tauri::command]
pub fn logout(state: State<'_, AppState>) -> AppResult<()> {
    if let Some(server_url) = state.settings().server_url {
        secrets::delete_token(&server_url)?;
    }
    state.set_client(None);
    tracing::info!("déconnexion");
    Ok(())
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Ordinateur".to_string())
}
