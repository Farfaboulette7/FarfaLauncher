//! Section « Soutenir le projet » des paramètres : infos de don lues à distance.
//!
//! Rien n'est codé en dur dans le launcher : à chaque ouverture des paramètres, l'interface
//! (`components/SupportSection.tsx`) appelle `support_info`, qui lit un petit fichier JSON
//! servi par FarfaRomManager :
//!
//! - sur le NAS : `Z:/FarfaRomManager/assets/soutien.json` ;
//! - adresse : `<adresse du gestionnaire>/soutien.json` (route `GET /soutien.json`, réservée aux
//!   comptes connectés, comme les musiques : le numéro TWINT n'est pas public).
//!
//! Pour le modifier ou le désactiver : `docs/runbook.md` du launcher, « Modifier les infos de
//! don » (le pas à pas complet est dans le runbook du gestionnaire, section 21).
//!
//! Règle d'or : **cette section ne gêne jamais**. Fichier absent, gestionnaire injoignable ou
//! trop lent, JSON invalide, `enabled` différent de `true`, aucun lien utilisable : la commande
//! rend `None` et la section est cachée, sans message d'erreur (seulement une ligne dans le
//! journal, qui commence par « soutien : »).
//!
//! Pas sur le Steam Deck : ce module fait partie de `commands` (interface du launcher). Le
//! moteur du Deck (NieRDeck) n'a pas d'écran de paramètres.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::error::LockOrRecover as _;
use crate::romm::client::{error_chain, truncate};
use crate::state::AppState;

/// Chemin du fichier de don, **relatif à l'adresse de FarfaRomManager**.
///
/// C'est la « constante configurable » de l'URL : l'adresse complète est
/// `<gestionnaire>/<SUPPORT_PATH>`, par exemple `https://manager.exemple.fr/soutien.json`.
/// L'adresse du gestionnaire n'est pas écrite ici, car le dépôt est public : elle est déduite
/// de celle de RomM (`romm.x` → `manager.x`), ou réglée à la main dans Paramètres → Émulateurs
/// (voir `manager::base_url`). Pour servir le fichier ailleurs sur le gestionnaire, changer ce
/// chemin (et la route `/soutien.json` de FarfaRomManager, `app/farfa_manager/api.py`).
pub const SUPPORT_PATH: &str = "soutien.json";

/// Délai maximal pour lire le fichier. Court exprès : l'écran des paramètres s'affiche tout de
/// suite de toute façon (la lecture se fait en arrière-plan), et au-delà de ce délai la section
/// reste simplement cachée.
const SUPPORT_TIMEOUT: Duration = Duration::from_secs(3);

/// Garde-fous contre un fichier démesuré (faute de frappe, copier-coller raté) : au-delà, le
/// texte est coupé ou les lignes en trop ignorées. Largement assez pour l'usage prévu.
const MAX_LINKS: usize = 10;
const MAX_MESSAGE: usize = 500;
const MAX_LABEL: usize = 60;
const MAX_VALUE: usize = 500;

/// Liens `url` de la dernière lecture réussie. `open_url` (commands/mod.rs) n'ouvre que des
/// adresses connues (faille SL12) : celles-ci y sont ajoutées, et seulement elles. Remplacée à
/// chaque lecture, vidée si le fichier disparaît ou est désactivé.
static SUPPORT_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Ce que l'interface affiche. Les types et champs inconnus du fichier ont déjà été écartés.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportInfo {
    /// Phrase en haut de la section (peut être vide).
    pub message: String,
    /// Lignes à afficher, dans l'ordre du fichier.
    pub links: Vec<SupportLink>,
}

/// Une ligne de la section.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SupportLink {
    /// `"type": "text"` : valeur affichée avec un bouton « Copier » (numéro TWINT, IBAN…).
    Text { label: String, value: String },
    /// `"type": "url"` : bouton qui ouvre le lien dans le navigateur ; QR code si `show_qr`.
    #[serde(rename_all = "camelCase")]
    Url { label: String, value: String, show_qr: bool },
}

/// Lit les infos de don. Ne renvoie **jamais** d'erreur : `None` = section cachée.
#[tauri::command]
pub async fn support_info(state: State<'_, AppState>) -> Result<Option<SupportInfo>, ()> {
    let info = fetch(&state).await;
    // Mémorise les liens à ouvrir (ou les oublie si la section est cachée).
    let urls = info
        .iter()
        .flat_map(|i| i.links.iter())
        .filter_map(|l| match l {
            SupportLink::Url { value, .. } => Some(value.clone()),
            SupportLink::Text { .. } => None,
        })
        .collect();
    *SUPPORT_URLS.lock_or_recover() = urls;
    Ok(info)
}

/// `true` si `url` est un lien `url` de la section, tel que lu à la dernière ouverture.
pub fn is_support_link(url: &str) -> bool {
    SUPPORT_URLS.lock_or_recover().iter().any(|u| u == url)
}

/// Requête au gestionnaire. Chaque échec est noté dans le journal puis donne `None`.
async fn fetch(state: &AppState) -> Option<SupportInfo> {
    // Pas d'adresse de gestionnaire (aucun serveur configuré) ou mode hors ligne : rien à lire.
    let base = crate::manager::base_url(&state.settings())?;
    let client = state.online_client().ok()?.with_base(&base);
    let resp = match client.get(SUPPORT_PATH).timeout(SUPPORT_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::info!("soutien : gestionnaire injoignable ou trop lent : {}", error_chain(&e));
            return None;
        }
    };
    let status = resp.status().as_u16();
    if status != 200 {
        // 404 : fichier absent (ou gestionnaire d'avant la route) ; 401 : jeton refusé.
        tracing::info!("soutien : HTTP {status}, section cachée");
        return None;
    }
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            tracing::info!("soutien : lecture interrompue : {}", error_chain(&e));
            return None;
        }
    };
    let info = parse(&body);
    if info.is_none() {
        tracing::info!("soutien : fichier désactivé, invalide ou sans lien utilisable : {}", truncate(&body, 200));
    }
    info
}

/// Texte du fichier → infos à afficher. Séparé pour être testé sans réseau.
///
/// Règles (les mêmes que dans le runbook) :
/// - `enabled` doit valoir exactement `true` ;
/// - `message` : texte facultatif ;
/// - chaque élément de `links` a un `label` et une `value` non vides, et un `type` connu
///   (`text` ou `url`) ; un élément d'un **type inconnu est ignoré**, pas le fichier entier
///   (on pourra ajouter des types plus tard sans casser les anciens launchers) ;
/// - un lien `url` doit commencer par `https://`, sans espace ni caractère de contrôle (il est
///   passé tel quel au système) ; sinon il est ignoré ;
/// - s'il ne reste aucun lien, la section est cachée.
pub fn parse(body: &str) -> Option<SupportInfo> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("enabled")?.as_bool()? != true {
        return None;
    }
    let message = clip(v.get("message").and_then(|m| m.as_str()).unwrap_or(""), MAX_MESSAGE);
    let links: Vec<SupportLink> = v
        .get("links")?
        .as_array()?
        .iter()
        .filter_map(parse_link)
        .take(MAX_LINKS)
        .collect();
    if links.is_empty() {
        return None;
    }
    Some(SupportInfo { message, links })
}

/// Un élément de `links` → une ligne, ou `None` s'il faut l'ignorer.
fn parse_link(l: &serde_json::Value) -> Option<SupportLink> {
    let label = clip(l.get("label")?.as_str()?, MAX_LABEL);
    let value = l.get("value")?.as_str()?.trim().to_string();
    if label.is_empty() || value.is_empty() || value.chars().count() > MAX_VALUE {
        return None;
    }
    match l.get("type")?.as_str()? {
        "text" => Some(SupportLink::Text { label, value }),
        "url" => {
            let safe = value.starts_with("https://")
                && value.len() > "https://".len()
                && !value.chars().any(|c| c.is_whitespace() || c.is_control());
            if !safe {
                return None;
            }
            // `showQr` absent ou pas un booléen : pas de QR code.
            let show_qr = l.get("showQr").and_then(|q| q.as_bool()).unwrap_or(false);
            Some(SupportLink::Url { label, value, show_qr })
        }
        // Type inconnu de cette version : ignoré sans bruit.
        _ => None,
    }
}

/// Texte nettoyé (espaces autour retirés) et coupé à `max` caractères.
fn clip(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le contenu initial du fichier sur le NAS.
    const INITIAL: &str = r#"{
      "enabled": true,
      "message": "FarfaLauncher est gratuit et le restera. Si tu veux quand même m'offrir une bière :",
      "links": [
        { "label": "TWINT", "type": "text", "value": "07x xxx xx xx" },
        { "label": "PayPal", "type": "url", "value": "https://www.paypal.me/Farfaboulette", "showQr": true }
      ]
    }"#;

    #[test]
    fn affichage_normal() {
        let info = parse(INITIAL).unwrap();
        assert!(info.message.starts_with("FarfaLauncher est gratuit"));
        assert_eq!(
            info.links,
            vec![
                SupportLink::Text { label: "TWINT".into(), value: "07x xxx xx xx".into() },
                SupportLink::Url { label: "PayPal".into(), value: "https://www.paypal.me/Farfaboulette".into(), show_qr: true },
            ]
        );
        // Ce que reçoit l'interface (noms des champs de SupportSection.tsx).
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["links"][0]["type"], "text");
        assert_eq!(json["links"][1]["type"], "url");
        assert_eq!(json["links"][1]["showQr"], true);
    }

    #[test]
    fn json_invalide_ou_desactive() {
        for bad in [
            "",
            "<html>404</html>",
            "{ \"enabled\": true, ",           // coupé
            "[1, 2]",                          // pas un objet
            r#"{"enabled": false, "links": [{"label":"A","type":"text","value":"1"}]}"#,
            r#"{"enabled": "true", "links": [{"label":"A","type":"text","value":"1"}]}"#, // pas un booléen
            r#"{"links": [{"label":"A","type":"text","value":"1"}]}"#,                    // enabled absent
            r#"{"enabled": true}"#,                                                       // pas de links
            r#"{"enabled": true, "links": []}"#,                                          // aucun lien
            r#"{"enabled": true, "links": "x"}"#,
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn types_inconnus_et_liens_douteux_ignores() {
        let info = parse(
            r#"{"enabled": true, "links": [
                {"label": "Crypto", "type": "wallet", "value": "abc"},
                {"label": "Local", "type": "url", "value": "file:///C:/Windows/calc.exe"},
                {"label": "Http", "type": "url", "value": "http://exemple.fr"},
                {"label": "Espace", "type": "url", "value": "https://a.fr/b c"},
                {"label": "", "type": "text", "value": "1"},
                {"label": "Vide", "type": "text", "value": "  "},
                {"label": "Ok", "type": "url", "value": "https://ko-fi.com/x", "showQr": "oui"},
                {"label": "IBAN", "type": "text", "value": "CH00 0000"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(info.message, "");
        assert_eq!(
            info.links,
            vec![
                // showQr qui n'est pas un booléen : pas de QR.
                SupportLink::Url { label: "Ok".into(), value: "https://ko-fi.com/x".into(), show_qr: false },
                SupportLink::Text { label: "IBAN".into(), value: "CH00 0000".into() },
            ]
        );
        // Seulement des types inconnus : section cachée.
        assert_eq!(parse(r#"{"enabled": true, "links": [{"label": "X", "type": "nouveau", "value": "1"}]}"#), None);
    }

    #[test]
    fn liens_memorises_pour_open_url() {
        *SUPPORT_URLS.lock_or_recover() = vec!["https://www.paypal.me/Farfaboulette".into()];
        assert!(is_support_link("https://www.paypal.me/Farfaboulette"));
        assert!(!is_support_link("https://www.paypal.me/Autre"));
        SUPPORT_URLS.lock_or_recover().clear();
        assert!(!is_support_link("https://www.paypal.me/Farfaboulette"));
    }
}
