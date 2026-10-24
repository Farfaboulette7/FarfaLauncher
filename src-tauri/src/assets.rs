//! Serveur des musiques et bruitages (conteneur `farfa-assets`, sous-domaine dédié).
//! Il ne sert ses fichiers qu'aux requêtes portant le token RomM de l'utilisateur.

use url::{Host, Url};

use crate::settings::Settings;

const SUBDOMAIN: &str = "assets";

/// Adresse déduite de celle du serveur RomM, sans jamais sortir de son domaine
/// (le token y est envoyé) :
/// - `https://romm.poupounas.synology.me` → `https://assets.poupounas.synology.me` ;
/// - `https://poupounas.synology.me` → `https://assets.poupounas.synology.me`
///   (et non `assets.synology.me`, qui appartiendrait à quelqu'un d'autre).
///
/// Rien pour une IP ou `localhost` : l'adresse doit alors être saisie.
pub fn derive_url(server_url: &str) -> Option<String> {
    let mut url = Url::parse(server_url).ok()?;
    let host = match url.host()? {
        Host::Domain(d) => d.to_ascii_lowercase(),
        _ => return None,
    };
    let registrable = psl::domain_str(&host)?;
    let derived = if host == registrable {
        format!("{SUBDOMAIN}.{host}")
    } else {
        // Sous-domaine : on remplace son premier niveau.
        let (_, parent) = host.split_once('.')?;
        format!("{SUBDOMAIN}.{parent}")
    };
    url.set_host(Some(&derived)).ok()?;
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// Adresse effective : celle saisie dans les paramètres, sinon celle déduite.
pub fn base_url(settings: &Settings) -> Option<String> {
    settings
        .assets_url
        .clone()
        .or_else(|| settings.server_url.as_deref().and_then(derive_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_subdomain() {
        assert_eq!(
            derive_url("https://romm.poupounas.synology.me").as_deref(),
            Some("https://assets.poupounas.synology.me")
        );
        assert_eq!(derive_url("https://romm.example.org:8443").as_deref(), Some("https://assets.example.org:8443"));
    }

    #[test]
    fn never_leaves_the_owner_domain() {
        assert_eq!(
            derive_url("https://poupounas.synology.me").as_deref(),
            Some("https://assets.poupounas.synology.me")
        );
        assert_eq!(derive_url("https://example.org").as_deref(), Some("https://assets.example.org"));
        assert_eq!(derive_url("https://romm.farfa.duckdns.org").as_deref(), Some("https://assets.farfa.duckdns.org"));
    }

    #[test]
    fn nothing_for_ip_or_localhost() {
        assert_eq!(derive_url("http://192.168.1.20:8080"), None);
        assert_eq!(derive_url("http://localhost:3000"), None);
    }

    #[test]
    fn setting_wins_over_derivation() {
        let mut s = Settings { server_url: Some("https://romm.example.org".into()), ..Settings::default() };
        assert_eq!(base_url(&s).as_deref(), Some("https://assets.example.org"));
        s.assets_url = Some("https://sons.example.org".into());
        assert_eq!(base_url(&s).as_deref(), Some("https://sons.example.org"));
    }
}
