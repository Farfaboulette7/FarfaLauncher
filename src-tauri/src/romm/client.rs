//! Client HTTP RomM (voir docs/romm-api.md).

use std::net::IpAddr;
use std::time::Duration;

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::error::{log_err, AppError, AppResult};

const USER_AGENT: &str = concat!("FarfaLauncher/", env!("CARGO_PKG_VERSION"));

/// Normalise l'adresse saisie par l'utilisateur.
/// - ajoute `https://` si aucun schéma n'est donné ;
/// - refuse `http://` sauf sur le réseau local (localhost, IP privées, `.local`) ;
/// - retire la requête, le fragment et le `/` final.
pub fn normalize_server_url(input: &str) -> AppResult<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::InvalidServerUrl);
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|_| AppError::InvalidServerUrl)?;
    let host = url.host_str().ok_or(AppError::InvalidServerUrl)?.to_string();
    match url.scheme() {
        "https" => {}
        "http" if is_local_host(&host) => {}
        "http" => return Err(AppError::InsecureServerUrl),
        _ => return Err(AppError::InvalidServerUrl),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::InvalidServerUrl);
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

#[derive(Clone)]
pub struct RommClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RommClient {
    pub fn new(base: &str, token: Option<String>) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| log_err(AppError::Internal, "création du client HTTP", e))?;
        Ok(Self { base: base.to_string(), token, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    fn authed(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.authed(self.http.get(self.url(path)))
    }

    pub fn post<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RequestBuilder {
        self.authed(self.http.post(self.url(path)).json(body))
    }

    pub async fn send(&self, req: RequestBuilder) -> AppResult<Response> {
        let resp = req.send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", e))?;
        check_status(resp).await
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        let resp = self.send(self.get(path)).await?;
        parse_json(resp).await
    }
}

/// Traduit les statuts d'erreur HTTP en erreurs utilisateur.
pub async fn check_status(resp: Response) -> AppResult<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let url = resp.url().path().to_string();
    let body = resp.text().await.unwrap_or_default();
    tracing::warn!("HTTP {status} sur {url}: {}", truncate(&body, 300));
    Err(match status {
        StatusCode::UNAUTHORIZED => AppError::AuthInvalid,
        StatusCode::FORBIDDEN => AppError::Forbidden,
        StatusCode::TOO_MANY_REQUESTS => AppError::RateLimited,
        _ => AppError::Server,
    })
}

pub async fn parse_json<T: DeserializeOwned>(resp: Response) -> AppResult<T> {
    let url = resp.url().path().to_string();
    resp.json::<T>()
        .await
        .map_err(|e| log_err(AppError::Server, &format!("réponse JSON invalide ({url})"), e))
}

pub fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_https_and_strips_slash() {
        assert_eq!(normalize_server_url("romm.example.org/").unwrap(), "https://romm.example.org");
        assert_eq!(normalize_server_url(" https://romm.example.org:8443/ ").unwrap(), "https://romm.example.org:8443");
    }

    #[test]
    fn normalize_rejects_remote_http() {
        assert!(matches!(normalize_server_url("http://romm.example.org"), Err(AppError::InsecureServerUrl)));
    }

    #[test]
    fn normalize_accepts_lan_http() {
        assert_eq!(normalize_server_url("http://192.168.1.20:8080").unwrap(), "http://192.168.1.20:8080");
        assert_eq!(normalize_server_url("http://localhost:3000").unwrap(), "http://localhost:3000");
        assert_eq!(normalize_server_url("http://nas.local").unwrap(), "http://nas.local");
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_server_url("").is_err());
        assert!(normalize_server_url("ftp://x").is_err());
        assert!(normalize_server_url("https://user:pw@x.org").is_err());
    }
}
