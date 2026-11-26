//! Onglet « Ajouter un jeu » de l'écran Fiches (administrateurs) : choisir un fichier ou un dossier
//! de Download Station et une console, pour que le jeu apparaisse dans RomM sans passer par le NAS.
//!
//! Le launcher ne fait que relayer vers FarfaRomManager (`/api/ajouts`, voir son
//! `docs/securite.md`, SG19) :
//! - la liste de Download Station est publiée par le PC du propriétaire (pas par le gestionnaire,
//!   qui ne voit pas ce dossier) ;
//! - une demande est un petit fichier sur le NAS ; le PC du propriétaire fait le lien physique et
//!   le scan RomM à son prochain passage (3 min), puis écrit le résultat (« fait » ou « échec ») ;
//! - compte administrateur de RomM, demande seulement depuis la maison ou le VPN (comme les fiches).
//!
//! Pas sur le Steam Deck (module `commands`, interface seulement).

use serde::{Deserialize, Serialize};
use tauri::State;

use super::fiches::{manager, refusal};
use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::{error_chain, parse_json, truncate, RommClient};
use crate::state::AppState;

/// Un fichier ou un dossier de Download Station (premier ou deuxième niveau : « Dossier/fichier »).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DsElement {
    pub chemin: String,
    /// `fichier` ou `dossier`.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub taille: u64,
    /// Date de modification (secondes Unix).
    #[serde(default)]
    pub date: i64,
    /// Fichiers qui peuvent être des ROM (les films, .nfo, images… ne comptent pas).
    #[serde(default)]
    pub fichiers: u32,
    /// Combien de ces fichiers sont déjà liés dans RomM (même inode).
    #[serde(default)]
    pub lies: u32,
    /// Dossiers de console de RomM où ils sont liés.
    #[serde(default)]
    pub consoles: Vec<String>,
}

/// Dossier de console de `RomM/roms` (son `fs_slug`), avec le nom de la plateforme RomM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AjoutConsole {
    pub dossier: String,
    #[serde(default)]
    pub nom: String,
    #[serde(default)]
    pub id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStation {
    /// Heure à laquelle le PC a fait cette liste (secondes Unix).
    #[serde(default)]
    pub genere_a: i64,
    #[serde(default)]
    pub consoles: Vec<AjoutConsole>,
    #[serde(default)]
    pub elements: Vec<DsElement>,
    /// Éléments masqués (films, séries, dossiers vides…).
    #[serde(default)]
    pub masques: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AjoutDemande {
    pub id: String,
    #[serde(default)]
    pub username: String,
    pub source: String,
    pub console: String,
    #[serde(default)]
    pub sous_dossier: String,
    /// Chemin dans `RomM/roms` : `<console>/[<sous-dossier>/]<nom>`.
    #[serde(default)]
    pub cible: String,
    /// `en attente`, `en cours`, `fait` ou `échec`.
    pub status: String,
    #[serde(default)]
    pub raison: String,
    #[serde(default)]
    pub liens: Option<u32>,
    #[serde(default)]
    pub ignores: Option<u32>,
    /// Scan RomM après le lien : `lancé`, `à relancer`, `échec : …`.
    #[serde(default)]
    pub scan: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AjoutsList {
    /// L'adresse vue par le gestionnaire permet de demander (réseau local de la maison ou VPN).
    pub modification: bool,
    #[serde(default)]
    pub adresse: String,
    /// Le PC du propriétaire a donné signe de vie depuis moins de 8 min.
    #[serde(default)]
    pub pc_en_ligne: bool,
    #[serde(default)]
    pub pc_vu_a: Option<i64>,
    /// `None` : le PC n'a pas encore publié la liste.
    #[serde(default)]
    pub liste: Option<DownloadStation>,
    #[serde(default)]
    pub demandes: Vec<AjoutDemande>,
}

/// Suivi des demandes, sans la liste (léger, rechargé pendant l'attente).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AjoutsSuivi {
    #[serde(default)]
    pub pc_en_ligne: bool,
    #[serde(default)]
    pub pc_vu_a: Option<i64>,
    #[serde(default)]
    pub demandes: Vec<AjoutDemande>,
}

/// Un nom simple : pas vide, ni `.` ni `..`, sans `/`, `\` ni caractère de contrôle, 255 octets au
/// plus. Le gestionnaire, le PC et le NAS refont ce contrôle.
fn simple_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && s.len() <= 255 && !s.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

/// Demande bien formée : source à un ou deux niveaux de Download Station, console et sous-dossier simples.
fn check_request(source: &str, console: &str, sous_dossier: &str) -> AppResult<()> {
    let parts: Vec<&str> = source.split('/').collect();
    let ok = (1..=2).contains(&parts.len())
        && parts.iter().all(|p| simple_name(p))
        && simple_name(console)
        && (sous_dossier.is_empty() || simple_name(sous_dossier));
    if ok {
        Ok(())
    } else {
        Err(AppError::Refused { message: "Demande invalide (chemin, console ou sous-dossier).".to_string() })
    }
}

/// Réponse du gestionnaire : `Ok(None)` pour une route absente (gestionnaire pas encore déployé)
/// ou un compte qui n'est plus administrateur ; un refus expliqué devient un message lisible.
async fn send(client: &RommClient, req: reqwest::RequestBuilder) -> AppResult<Option<reqwest::Response>> {
    let resp = req.send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
    let status = resp.status().as_u16();
    if matches!(status, 400 | 403 | 404 | 409 | 413 | 422 | 429) {
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!("ajouts : HTTP {status} : {}", truncate(&body, 300));
        return match refusal(status, &body) {
            AppError::Forbidden => Ok(None),
            // « Not Found » de FastAPI : route absente, ancien gestionnaire.
            AppError::Refused { message } if status == 404 && message.starts_with("Fiche introuvable") => Ok(None),
            e => Err(e),
        };
    }
    client.check(resp).await.map(Some)
}

/// Liste de Download Station, consoles et demandes récentes. `None` : gestionnaire trop ancien
/// (l'onglet l'explique, sans erreur) ou compte qui n'est pas administrateur.
#[tauri::command]
pub async fn ajouts_list(state: State<'_, AppState>) -> AppResult<Option<AjoutsList>> {
    let client = manager(&state)?;
    match send(&client, client.get("api/ajouts").timeout(std::time::Duration::from_secs(60))).await? {
        Some(resp) => parse_json(resp).await.map(Some),
        None => Ok(None),
    }
}

/// Suivi léger des demandes (pendant l'attente du PC).
#[tauri::command]
pub async fn ajouts_suivi(state: State<'_, AppState>) -> AppResult<Option<AjoutsSuivi>> {
    let client = manager(&state)?;
    match send(&client, client.get("api/ajouts/demandes")).await? {
        Some(resp) => parse_json(resp).await.map(Some),
        None => Ok(None),
    }
}

/// « Ajouter à RomM » : dépose la demande ; le PC du propriétaire la traite à son prochain passage.
#[tauri::command]
pub async fn ajout_create(state: State<'_, AppState>, source: String, console: String, sous_dossier: String) -> AppResult<AjoutDemande> {
    check_request(&source, &console, sous_dossier.trim())?;
    let client = manager(&state)?;
    let body = serde_json::json!({ "source": source, "console": console, "sousDossier": sous_dossier.trim() });
    match send(&client, client.post("api/ajouts/demandes", &body)).await? {
        Some(resp) => {
            let d: AjoutDemande = parse_json(resp).await?;
            tracing::info!("ajout à RomM demandé : {} → {}", d.source, d.cible);
            Ok(d)
        }
        None => Err(AppError::Refused { message: "Le gestionnaire n'accepte pas encore les ajouts (à déployer).".to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_checked() {
        assert!(check_request("Jeu $(rm -rf ~); `x` 'a'", "switch", "").is_ok());
        assert!(check_request("Collection/jeu.nds", "nds", "").is_ok());
        assert!(check_request("jeu.iso", "ordinateur", "Mon jeu").is_ok());
        for (source, console, sous) in [
            ("../x", "switch", ""),
            ("a/../b", "switch", ""),
            ("/volume1/x", "switch", ""),
            ("a/b/c", "switch", ""),
            ("a\\b", "switch", ""),
            ("", "switch", ""),
            ("a\nb", "switch", ""),
            ("a", "..", ""),
            ("a", "switch/x", ""),
            ("a", "switch", "x/y"),
            ("a", "switch", ".."),
        ] {
            assert!(check_request(source, console, sous).is_err(), "{source} | {console} | {sous}");
        }
    }

    #[test]
    fn reads_the_answer_of_the_manager() {
        let l: AjoutsList = serde_json::from_str(
            r#"{"modification":true,"adresse":"192.168.1.5","pcEnLigne":false,"pcVuA":null,
                "liste":{"format":1,"genereA":5,"consoles":[{"dossier":"switch","nom":"Nintendo Switch","id":20},{"dossier":"neuve","nom":"neuve","id":null}],
                         "elements":[{"chemin":"A/b.nsp","type":"fichier","taille":3,"date":1,"fichiers":1,"lies":1,"consoles":["switch"]}],"masques":2},
                "demandes":[{"id":"20261126-100000-1-1","account":1,"username":"farfa","adresse":"x","source":"A","type":"dossier","console":"switch",
                             "sousDossier":"","cible":"switch/A","status":"fait","raison":"","liens":2,"ignores":0,"scan":"lancé","scanTache":"t","createdAt":1,"updatedAt":2}]}"#,
        )
        .unwrap();
        let liste = l.liste.unwrap();
        assert_eq!(liste.elements[0].kind, "fichier");
        assert_eq!(liste.consoles[1].id, None);
        assert_eq!(l.demandes[0].liens, Some(2));
        // Liste pas encore publiée, demande « en attente » sans résultat : lue quand même.
        let l: AjoutsList = serde_json::from_str(
            r#"{"modification":false,"adresse":"203.0.113.7","pcEnLigne":true,"pcVuA":5,"liste":null,
                "demandes":[{"id":"x","source":"A","console":"switch","status":"en attente"}]}"#,
        )
        .unwrap();
        assert!(l.liste.is_none() && l.demandes[0].scan.is_none());
    }
}
