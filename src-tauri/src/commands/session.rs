//! Commandes de connexion / déconnexion. Le token ne quitte jamais le côté
//! Rust : l'interface ne reçoit que le nom d'utilisateur et l'état.

use serde::Serialize;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::romm::auth::{self, DevicePoll};
use crate::romm::client::{normalize_server_url, RommClient};
use crate::secrets;
use crate::state::{AppState, PendingDevice};
use crate::error::LockOrRecover;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub server_url: Option<String>,
    pub connected: bool,
    /// Connecté mais serveur injoignable : on affiche le cache.
    pub offline: bool,
    pub username: Option<String>,
    /// Image de profil du compte RomM (servie par `image_path`), s'il en a une.
    pub avatar_url: Option<String>,
    /// Raison d'une déconnexion forcée (token révoqué, compte supprimé…).
    pub notice: Option<String>,
}

impl SessionInfo {
    fn disconnected(server_url: Option<String>, notice: Option<String>) -> Self {
        Self { server_url, connected: false, offline: false, username: None, avatar_url: None, notice }
    }
}

/// Message de l'écran de connexion après un 403 au démarrage.
const FORBIDDEN_NOTICE: &str =
    "Le serveur refuse l'accès à votre compte (erreur 403). Reconnectez-vous ; si cela continue, le serveur ou son proxy bloque l'accès.";

/// Au démarrage : reprend la session à partir du token du trousseau.
#[tauri::command]
pub async fn session_status(state: State<'_, AppState>) -> AppResult<SessionInfo> {
    let Some(server_url) = state.settings().server_url else {
        return Ok(SessionInfo::disconnected(None, None));
    };
    let Some(token) = secrets::load_token(&state.config_dir, &server_url)? else {
        return Ok(SessionInfo::disconnected(Some(server_url), None));
    };
    let client = RommClient::new(&server_url, Some(token))?;
    match auth::me(&client).await {
        Ok(me) => {
            state.set_client(Some(client));
            state.set_offline(false);
            let avatar_url = me.avatar_url(&server_url);
            Ok(SessionInfo { server_url: Some(server_url), connected: true, offline: false, username: Some(me.username), avatar_url, notice: None })
        }
        // Seul un 401 de RomM (token révoqué ou expiré) efface le token (L4).
        Err(AppError::AuthInvalid) => {
            tracing::warn!("token refusé au démarrage (auth_invalid), suppression");
            secrets::delete_token(&state.config_dir, &server_url)?;
            state.set_client(None);
            Ok(SessionInfo::disconnected(Some(server_url), Some(AppError::AuthInvalid.to_string())))
        }
        // 403 (droit manquant sur le token, ou page d'erreur d'un proxy) : retour à l'écran de
        // connexion avec une explication, mais le token est gardé. Se reconnecter le remplace ;
        // si c'était un proxy passager, il resservira au prochain démarrage. Ainsi le joueur
        // n'est jamais bloqué « hors ligne » pour toujours (L4).
        Err(AppError::Forbidden) => {
            tracing::warn!("accès refusé (403) au démarrage : écran de connexion, token gardé");
            state.set_client(None);
            Ok(SessionInfo::disconnected(Some(server_url), Some(FORBIDDEN_NOTICE.to_string())))
        }
        Err(AppError::ServerUnreachable | AppError::Server) => {
            state.set_client(Some(client));
            state.set_offline(true);
            Ok(SessionInfo { server_url: Some(server_url), connected: true, offline: true, username: None, avatar_url: None, notice: None })
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
    // Une adresse d'assets saisie pour l'ancien serveur recevrait le token du nouveau.
    state.update_settings(|s| {
        s.server_url = Some(server_url.clone());
        s.assets_url = None;
    })?;
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
    let name = format!("FaRFa project — {}", device_name());
    let init = auth::device_init(&client, &state.settings().device_id, &name).await?;
    *state.pending_device.lock_or_recover() = Some(PendingDevice { server_url: server_url.clone(), device_code: init.device_code });
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
    let (server_url, device_code) = match state.pending_device.lock_or_recover().as_ref() {
        Some(p) => (p.server_url.clone(), p.device_code.clone()),
        None => return Ok(DeviceAuthPoll::Expired),
    };
    let client = RommClient::new(&server_url, None)?;
    Ok(match auth::device_poll(&client, &device_code).await? {
        DevicePoll::Pending => DeviceAuthPoll::Pending,
        DevicePoll::SlowDown => DeviceAuthPoll::SlowDown,
        DevicePoll::Denied => {
            state.pending_device.lock_or_recover().take();
            DeviceAuthPoll::Denied
        }
        DevicePoll::Expired => {
            state.pending_device.lock_or_recover().take();
            DeviceAuthPoll::Expired
        }
        DevicePoll::Approved { token, scopes } => {
            state.pending_device.lock_or_recover().take();
            let missing: Vec<_> = auth::REQUIRED_SCOPES.iter().filter(|s| !scopes.iter().any(|g| g == *s)).collect();
            if !missing.is_empty() {
                tracing::warn!("scopes non accordés: {missing:?}");
            }
            DeviceAuthPoll::Approved { session: finish_login(&state, &server_url, token, false).await? }
        }
    })
}

#[tauri::command]
pub fn cancel_device_auth(state: State<'_, AppState>) {
    state.pending_device.lock_or_recover().take();
}

#[tauri::command]
pub async fn login_with_pair_code(state: State<'_, AppState>, code: String) -> AppResult<SessionInfo> {
    let server_url = state.server_url()?;
    let token = auth::exchange_pair_code(&RommClient::new(&server_url, None)?, &code).await?;
    // Jeton créé à la main dans RomM, scopes choisis par le joueur : vérifiés (SL11).
    finish_login(&state, &server_url, token, true).await
}

#[tauri::command]
pub async fn login_with_token(state: State<'_, AppState>, token: String) -> AppResult<SessionInfo> {
    let server_url = state.server_url()?;
    let token = auth::validate_token_format(&token)?;
    // Jeton collé : il peut avoir plus de droits que le launcher n'en demande (SL11).
    finish_login(&state, &server_url, token, true).await
}

/// Valide le token auprès du serveur puis le range dans le trousseau.
///
/// `check_admin` : jeton collé ou code d'appairage. Un jeton avec les droits d'administration
/// de RomM est refusé avant d'être rangé (faille SL11) : les jeux et émulateurs tournent avec
/// le compte du joueur et peuvent lire le trousseau. Le message s'affiche à la place habituelle
/// des erreurs de l'écran de connexion.
async fn finish_login(state: &AppState, server_url: &str, token: String, check_admin: bool) -> AppResult<SessionInfo> {
    let client = RommClient::new(server_url, Some(token.clone()))?;
    let me = auth::me(&client).await.map_err(|e| match e {
        AppError::AuthInvalid => AppError::TokenRejected,
        other => other,
    })?;
    if check_admin && auth::has_admin_rights(&client, &me).await {
        tracing::warn!("jeton collé refusé : droits d'administration de RomM (compte {})", me.username);
        return Err(AppError::TokenTooPowerful);
    }
    secrets::store_token(&state.config_dir, server_url, &token)?;
    state.set_client(Some(client));
    state.set_offline(false);
    tracing::info!("connecté en tant que {}", me.username);
    let avatar_url = me.avatar_url(server_url);
    Ok(SessionInfo { server_url: Some(server_url.to_string()), connected: true, offline: false, username: Some(me.username), avatar_url, notice: None })
}

/// Délai d'attente du test de connexion : court, il revient toutes les 20 à 60 s.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Test du serveur (heartbeat RomM, sans authentification), répété par l'interface pour
/// passer en mode hors ligne ou en sortir. Rend `true` si le serveur répond.
#[tauri::command]
pub async fn check_connection(state: State<'_, AppState>) -> AppResult<bool> {
    let client = state.client()?;
    let online = match client.send(client.get("api/heartbeat").timeout(CHECK_TIMEOUT)).await {
        Ok(_) => true,
        Err(AppError::ServerUnreachable | AppError::Server) => false,
        Err(e) => return Err(e),
    };
    state.set_offline(!online);
    Ok(online)
}

/// Oublie le token local. (Il reste révocable côté RomM : Paramètres →
/// Appareils / Client API Tokens.)
#[tauri::command]
pub fn logout(state: State<'_, AppState>) -> AppResult<()> {
    if let Some(server_url) = state.settings().server_url {
        secrets::delete_token(&state.config_dir, &server_url)?;
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
