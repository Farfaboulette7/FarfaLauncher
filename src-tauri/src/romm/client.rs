//! Client HTTP RomM (voir docs/romm-api.md).

use std::net::IpAddr;
use std::time::Duration;

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::error::{log_err, AppError, AppResult};

const USER_AGENT: &str = concat!("FarfaLauncher/", env!("CARGO_PKG_VERSION"));
const API_TIMEOUT: Duration = Duration::from_secs(45);
/// Téléchargements (`get_stream`, sans délai global) : temps maximum sans rien recevoir, ni
/// en-têtes ni données. Un serveur muet ne bloque donc jamais un lancement (L6).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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
            .build()
            .map_err(|e| log_err(AppError::Internal, "création du client HTTP", e))?;
        Ok(Self { base: base.to_string(), token, http })
    }

    /// Même token, autre serveur (gestionnaire, musiques et bruitages).
    /// Le token n'est envoyé qu'à une adresse du même domaine que RomM (ou la même machine) :
    /// une adresse saisie à la main ailleurs ne le reçoit pas (L16).
    pub fn with_base(&self, base: &str) -> Self {
        let token = if same_site(&self.base, base) {
            self.token.clone()
        } else {
            tracing::warn!("adresse hors du domaine de RomM : le token RomM ne lui est pas envoyé");
            None
        };
        Self { base: base.to_string(), token, http: self.http.clone() }
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

    /// Requête GET avec délai par défaut (`API_TIMEOUT`) ; à surcharger avec
    /// `.timeout()` pour les appels longs. Les téléchargements de fichiers
    /// utilisent `get_stream` (pas de délai global).
    pub fn get(&self, path: &str) -> RequestBuilder {
        self.authed(self.http.get(self.url(path)).timeout(API_TIMEOUT))
    }

    pub fn get_stream(&self, path: &str) -> RequestBuilder {
        self.authed(self.http.get(self.url(path)))
    }

    pub fn post<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RequestBuilder {
        self.authed(self.http.post(self.url(path)).json(body).timeout(API_TIMEOUT))
    }

    pub fn patch<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RequestBuilder {
        self.authed(self.http.patch(self.url(path)).json(body).timeout(API_TIMEOUT))
    }

    pub fn put<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RequestBuilder {
        self.authed(self.http.put(self.url(path)).json(body).timeout(API_TIMEOUT))
    }

    pub fn delete<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RequestBuilder {
        self.authed(self.http.delete(self.url(path)).json(body).timeout(API_TIMEOUT))
    }

    /// POST de formulaire (`application/x-www-form-urlencoded`).
    pub fn post_form<B: Serialize + ?Sized>(&self, path: &str, form: &B) -> RequestBuilder {
        self.authed(self.http.post(self.url(path)).form(form).timeout(API_TIMEOUT))
    }

    pub fn post_multipart(&self, path: &str, form: reqwest::multipart::Form) -> RequestBuilder {
        self.authed(self.http.post(self.url(path)).multipart(form))
    }

    /// PUT d'un fichier entier, sans délai global : à fixer selon l'envoi.
    pub fn put_bytes(&self, path: &str, body: Vec<u8>) -> RequestBuilder {
        self.authed(self.http.put(self.url(path)).header(reqwest::header::CONTENT_TYPE, "application/zip").body(body))
    }

    pub async fn send(&self, req: RequestBuilder) -> AppResult<Response> {
        let resp = req.send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
        check_status(resp).await
    }

    /// Comme `send`, pour une requête `get_stream` : abandonne si le serveur ne répond pas
    /// (en-têtes) dans les `IDLE_TIMEOUT`. Le corps se lit ensuite avec `read_body` ou avec
    /// son propre délai par morceau.
    pub async fn send_stream(&self, req: RequestBuilder) -> AppResult<Response> {
        send_within(self.send(req), IDLE_TIMEOUT).await
    }

    /// Lecture JSON ; une erreur serveur passagère (5xx) est retentée une fois.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        let resp = match self.send(self.get(path)).await {
            Err(AppError::Server) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tracing::info!("nouvel essai après une erreur serveur");
                self.send(self.get(path)).await?
            }
            other => other?,
        };
        parse_json(resp).await
    }
}

/// Même machine (même hôte, port ignoré) ou même domaine enregistrable
/// (`romm.exemple.fr` et `manager.exemple.fr`, voir `assets::derive_subdomain`).
fn same_site(a: &str, b: &str) -> bool {
    let host = |u: &str| Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    let (Some(a), Some(b)) = (host(a), host(b)) else { return false };
    if a == b {
        return true;
    }
    match (psl::domain_str(&a), psl::domain_str(&b)) {
        (Some(da), Some(db)) => da == db,
        _ => false,
    }
}

/// Attend une réponse au plus `limit`, sinon serveur injoignable.
async fn send_within(fut: impl std::future::Future<Output = AppResult<Response>>, limit: Duration) -> AppResult<Response> {
    match tokio::time::timeout(limit, fut).await {
        Ok(result) => result,
        Err(_) => Err(log_err(AppError::ServerUnreachable, "requête HTTP", format!("aucune réponse depuis {} s", limit.as_secs()))),
    }
}

/// Lit tout le corps d'une réponse (sauvegarde…), en abandonnant après `IDLE_TIMEOUT` sans
/// recevoir de données. Un gros fichier lent mais qui avance n'est jamais coupé.
pub async fn read_body(resp: Response, what: &str) -> AppResult<Vec<u8>> {
    read_body_within(resp, what, IDLE_TIMEOUT).await
}

async fn read_body_within(resp: Response, what: &str, idle: Duration) -> AppResult<Vec<u8>> {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    loop {
        match tokio::time::timeout(idle, stream.next()).await {
            Err(_) => return Err(log_err(AppError::Download, what, format!("aucune donnée reçue depuis {} s", idle.as_secs()))),
            Ok(None) => return Ok(body),
            Ok(Some(Err(e))) => return Err(log_err(AppError::Download, what, error_chain(&e))),
            Ok(Some(Ok(chunk))) => body.extend_from_slice(&chunk),
        }
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
    Err(status_error(status, &body))
}

/// Erreur correspondant à un statut HTTP d'échec et au corps de la réponse.
/// Un 401 ne veut dire « token refusé » (`AuthInvalid`, le token est alors oublié) que s'il
/// vient vraiment de RomM, qui répond toujours en JSON (`{"detail": …}`). Un 401 en page HTML
/// vient d'un intermédiaire (proxy, portail) : erreur serveur passagère, le token est gardé.
fn status_error(status: StatusCode, body: &str) -> AppError {
    match status {
        StatusCode::UNAUTHORIZED if serde_json::from_str::<serde_json::Value>(body).is_ok_and(|v| v.get("detail").is_some()) => {
            AppError::AuthInvalid
        }
        StatusCode::UNAUTHORIZED => {
            tracing::warn!("401 qui ne vient pas de RomM (pas de JSON) : proxy ? Le token est gardé");
            AppError::Server
        }
        StatusCode::FORBIDDEN => AppError::Forbidden,
        StatusCode::TOO_MANY_REQUESTS => AppError::RateLimited,
        _ => AppError::Server,
    }
}

pub async fn parse_json<T: DeserializeOwned>(resp: Response) -> AppResult<T> {
    let url = resp.url().path().to_string();
    resp.json::<T>()
        .await
        .map_err(|e| log_err(AppError::Server, &format!("réponse JSON invalide ({url})"), e))
}

/// Message d'erreur avec toutes ses causes (timeout, TLS, DNS…), sans l'URL
/// complète (qui contient l'adresse du serveur).
pub fn error_chain(e: &reqwest::Error) -> String {
    let mut out = String::new();
    if e.is_timeout() {
        out.push_str("[timeout] ");
    }
    if let Some(u) = e.url() {
        out.push_str(u.path());
        out.push_str(": ");
    }
    let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
    out.push_str(&e.without_url_ref());
    while let Some(s) = src {
        out.push_str(" <- ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

trait WithoutUrl {
    fn without_url_ref(&self) -> String;
}

impl WithoutUrl for reqwest::Error {
    fn without_url_ref(&self) -> String {
        let s = self.to_string();
        // « error sending request for url (…) » → on retire la parenthèse.
        match s.find(" for url (") {
            Some(i) => s[..i].to_string(),
            None => s,
        }
    }
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

    // L6 : un serveur (local) qui accepte la connexion mais ne répond jamais ne bloque pas.
    #[tokio::test]
    async fn silent_server_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accepte et garde la connexion ouverte, sans jamais rien envoyer.
        let server = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let client = RommClient::new(&format!("http://{addr}"), None).unwrap();
        let r = send_within(client.send(client.get_stream("api/x")), Duration::from_millis(300)).await;
        assert!(matches!(r, Err(AppError::ServerUnreachable)));
        server.abort();
    }

    // L6 : en-têtes reçus puis plus rien : la lecture du corps abandonne au lieu d'attendre.
    #[tokio::test]
    async fn stalled_body_times_out() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            // Annonce 100 octets, n'en envoie que 10, puis se tait.
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789").await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let client = RommClient::new(&format!("http://{addr}"), None).unwrap();
        let resp = send_within(client.send(client.get_stream("api/x")), Duration::from_secs(5)).await.unwrap();
        let r = read_body_within(resp, "test", Duration::from_millis(300)).await;
        assert!(matches!(r, Err(AppError::Download)));
        server.abort();
    }

    // L16 : le token ne part qu'au même domaine que RomM (ou à la même machine).
    #[test]
    fn token_only_goes_to_the_same_site() {
        assert!(same_site("https://romm.poupounas.synology.me", "https://manager.poupounas.synology.me"));
        assert!(same_site("https://romm.example.org", "https://assets.example.org"));
        assert!(same_site("http://192.168.1.102:8080", "http://192.168.1.102:5000"));
        assert!(!same_site("https://romm.poupounas.synology.me", "https://autre.synology.me"));
        assert!(!same_site("https://romm.example.org", "https://example.com"));
        assert!(!same_site("http://192.168.1.102:8080", "http://192.168.1.50"));
        let client = RommClient::new("https://romm.example.org", Some("rmm_x".into())).unwrap();
        assert!(client.with_base("https://manager.example.org").token.is_some());
        assert!(client.with_base("https://ailleurs.example.com").token.is_none());
    }

    // L4 : seul un 401 en JSON de RomM est un token refusé ; un 401 en page HTML (proxy) et un
    // 403 ne le sont pas (le token est gardé au démarrage).
    #[test]
    fn only_a_romm_401_rejects_the_token() {
        assert!(matches!(status_error(StatusCode::UNAUTHORIZED, r#"{"detail":"Invalid token"}"#), AppError::AuthInvalid));
        assert!(matches!(status_error(StatusCode::UNAUTHORIZED, "<html><body>401 Authorization Required</body></html>"), AppError::Server));
        assert!(matches!(status_error(StatusCode::UNAUTHORIZED, ""), AppError::Server));
        assert!(matches!(status_error(StatusCode::FORBIDDEN, "<html>403 Forbidden</html>"), AppError::Forbidden));
        assert!(matches!(status_error(StatusCode::BAD_GATEWAY, ""), AppError::Server));
    }

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
