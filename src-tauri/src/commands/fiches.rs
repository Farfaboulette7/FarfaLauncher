//! Écran « Fiches » des administrateurs : lire, vérifier et modifier les fiches de recomps
//! (`recomps/<Nom>.txt`) et d'émulateurs (`emulateurs/<id>.txt`) de FarfaRomManager, sans
//! passer par le partage du NAS.
//!
//! Tout le contrôle est fait par le gestionnaire (`/api/fiches`, voir son `docs/securite.md`) :
//! compte administrateur de RomM, écriture seulement depuis la maison ou le VPN, fiche validée
//! avant d'être écrite, ancienne version gardée, jamais de suppression. Ici, on ne fait que
//! relayer, avec des noms de fichiers contrôlés et encodés dans l'adresse.
//!
//! Pas sur le Steam Deck (module `commands`, interface seulement).

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::{error_chain, parse_json, truncate, RommClient};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FicheInfo {
    /// `recomps` ou `emulateurs`.
    pub dossier: String,
    pub nom: String,
    /// `valide`, `invalide` ou `retiree` (nom en `_<nom>.txt`, ignorée par le gestionnaire).
    pub etat: String,
    #[serde(default)]
    pub erreur: Option<String>,
    #[serde(default)]
    pub ligne: Option<u32>,
    #[serde(default)]
    pub modifiee: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FichesList {
    /// L'adresse vue par le gestionnaire permet d'écrire (réseau local ou VPN).
    pub modification: bool,
    /// Adresse vue par le gestionnaire (aide quand la modification est refusée).
    #[serde(default)]
    pub adresse: String,
    pub fiches: Vec<FicheInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FicheContent {
    pub dossier: String,
    pub nom: String,
    pub texte: String,
    /// SHA-1 du fichier : renvoyé à l'enregistrement, le gestionnaire refuse d'écraser une
    /// fiche changée entre-temps.
    pub empreinte: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FicheVerdict {
    pub valide: bool,
    #[serde(default)]
    pub erreur: Option<String>,
    /// Ligne en cause (1 = première), si le validateur la connaît.
    #[serde(default)]
    pub ligne: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FicheSaved {
    pub nom: String,
    #[serde(default)]
    pub empreinte: Option<String>,
    /// Copie de l'ancienne version sur le NAS (`fiches-historique/…`, sous `data/`).
    #[serde(default)]
    pub historique: Option<String>,
}

#[derive(Deserialize)]
struct Me {
    #[serde(default)]
    admin: bool,
}

/// Client du gestionnaire (même jeton que RomM), refusé hors ligne.
fn manager(state: &AppState) -> AppResult<RommClient> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    Ok(state.online_client()?.with_base(&base))
}

/// `recomps/<nom encodé>` : dossier connu et nom de fichier simple (le gestionnaire revérifie).
fn fiche_path(dossier: &str, nom: &str) -> AppResult<String> {
    if !matches!(dossier, "recomps" | "emulateurs")
        || nom.is_empty()
        || nom.len() > 200
        || !nom.ends_with(".txt")
        || nom.contains("..")
        || nom.chars().any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(AppError::NotAvailable);
    }
    Ok(format!("api/fiches/{dossier}/{}", encode_segment(nom)))
}

/// Encode un morceau d'adresse : lettres, chiffres, `.`, `_` et `-` restent, le reste devient
/// `%XX` (espace, `&`, `'`… des noms de fiches comme « Ratchet & Clank.txt »).
fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Envoie une requête au gestionnaire. Un refus expliqué (`{"detail": "…"}` en 400, 403, 404,
/// 409, 413 ou 422 : fiche invalide, hors de la maison, fiche changée…) devient un message
/// lisible (`AppError::Refused`) ; le reste suit les règles habituelles (`check`).
async fn send(client: &RommClient, req: reqwest::RequestBuilder) -> AppResult<reqwest::Response> {
    let resp = req.send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
    let status = resp.status().as_u16();
    if matches!(status, 400 | 403 | 404 | 409 | 413 | 422) {
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!("fiches : HTTP {status} : {}", truncate(&body, 300));
        return Err(refusal(status, &body));
    }
    client.check(resp).await
}

/// Message du gestionnaire (`detail`), ou l'erreur habituelle s'il n'y en a pas.
fn refusal(status: u16, body: &str) -> AppError {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(str::to_string));
    match (status, detail) {
        (_, Some(message)) if !message.trim().is_empty() => AppError::Refused { message: truncate(message.trim(), 500).to_string() },
        (403, _) => AppError::Forbidden,
        _ => AppError::Server,
    }
}

/// Liste des fiches ; `None` pour un compte qui n'est pas administrateur, ou un gestionnaire
/// trop ancien pour cet écran (404) : le launcher ne montre alors pas l'entrée.
#[tauri::command]
pub async fn fiches_list(state: State<'_, AppState>) -> AppResult<Option<FichesList>> {
    let client = manager(&state)?;
    let me: Me = match client.get_json("api/me").await {
        Ok(me) => me,
        Err(AppError::Server) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !me.admin {
        return Ok(None);
    }
    match send(&client, client.get("api/fiches")).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        // Ancien gestionnaire (route absente) ou compte plus administrateur : pas d'écran.
        Err(AppError::Refused { .. }) | Err(AppError::Forbidden) => Ok(None),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn fiche_read(state: State<'_, AppState>, dossier: String, nom: String) -> AppResult<FicheContent> {
    let client = manager(&state)?;
    let path = fiche_path(&dossier, &nom)?;
    parse_json(send(&client, client.get(&path)).await?).await
}

/// Bouton « Vérifier » : le validateur du gestionnaire, sans rien écrire.
#[tauri::command]
pub async fn fiche_check(state: State<'_, AppState>, dossier: String, nom: String, texte: String) -> AppResult<FicheVerdict> {
    let client = manager(&state)?;
    fiche_path(&dossier, &nom)?;
    let body = serde_json::json!({ "dossier": dossier, "nom": nom, "texte": texte });
    parse_json(send(&client, client.post("api/fiches/verifier", &body)).await?).await
}

/// Enregistre une fiche. `empreinte` : celle de la version ouverte, `None` pour une nouvelle fiche
/// (refusée si le fichier existe déjà).
#[tauri::command]
pub async fn fiche_save(state: State<'_, AppState>, dossier: String, nom: String, texte: String, empreinte: Option<String>) -> AppResult<FicheSaved> {
    let client = manager(&state)?;
    let path = fiche_path(&dossier, &nom)?;
    let body = serde_json::json!({ "texte": texte, "empreinte": empreinte });
    let saved: FicheSaved = parse_json(send(&client, client.put(&path, &body)).await?).await?;
    tracing::info!("fiche enregistrée : {dossier}/{nom}");
    Ok(saved)
}

/// « Retirer » (`_<nom>.txt`, ignorée par le gestionnaire) ou « Remettre ». Jamais de suppression.
#[tauri::command]
pub async fn fiche_rename(state: State<'_, AppState>, dossier: String, nom: String, retirer: bool) -> AppResult<FicheSaved> {
    let client = manager(&state)?;
    let path = format!("{}/{}", fiche_path(&dossier, &nom)?, if retirer { "retirer" } else { "remettre" });
    let done: FicheSaved = parse_json(send(&client, client.post(&path, &serde_json::json!({}))).await?).await?;
    tracing::info!("fiche {} : {dossier}/{nom} → {}", if retirer { "retirée" } else { "remise" }, done.nom);
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_and_encoded() {
        assert_eq!(fiche_path("recomps", "Ratchet & Clank.txt").unwrap(), "api/fiches/recomps/Ratchet%20%26%20Clank.txt");
        assert_eq!(fiche_path("emulateurs", "mgba.txt").unwrap(), "api/fiches/emulateurs/mgba.txt");
        assert_eq!(
            fiche_path("recomps", "The Legend Of Zelda Link's Awakening.txt").unwrap(),
            "api/fiches/recomps/The%20Legend%20Of%20Zelda%20Link%27s%20Awakening.txt"
        );
        for (dossier, nom) in [("data", "a.txt"), ("recomps", "../.env"), ("recomps", "a/b.txt"), ("recomps", "a\\b.txt"), ("recomps", "a.json"), ("recomps", "")] {
            assert!(fiche_path(dossier, nom).is_err(), "{dossier}/{nom}");
        }
    }

    #[test]
    fn manager_refusal_is_shown_as_its_message() {
        let e = refusal(403, r#"{"detail":"Modification possible seulement depuis la maison ou le VPN"}"#);
        assert_eq!(e.to_string(), "Modification possible seulement depuis la maison ou le VPN");
        assert_eq!(e.code(), "refused");
        assert!(matches!(refusal(403, "<html>403</html>"), AppError::Forbidden));
        assert!(matches!(refusal(409, ""), AppError::Server));
    }

    #[test]
    fn reads_the_list_of_the_manager() {
        let l: FichesList = serde_json::from_str(
            r#"{"modification":false,"adresse":"203.0.113.7","fiches":[{"dossier":"recomps","nom":"A.txt","etat":"invalide","erreur":"x (recomps/A.txt, ligne 2)","ligne":2,"modifiee":1}]}"#,
        )
        .unwrap();
        assert!(!l.modification);
        assert_eq!(l.fiches[0].ligne, Some(2));
    }
}
