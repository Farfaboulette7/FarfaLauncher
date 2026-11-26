//! Succès RetroAchievements d'un jeu (section « Succès » de la page du jeu, commande `succes`
//! du moteur du Deck). Voir docs/romm-api.md §« Succès RetroAchievements ».
//!
//! Tout vient de RomM, avec les scopes déjà demandés (`roms.read`, `me.read`) :
//! - la liste des succès du jeu : `merged_ra_metadata.achievements` de `GET /api/roms/<id>/simple`
//!   (RomM l'a lue sur RetroAchievements au scan, avec les images des badges) ;
//! - ce que le joueur a débloqué : `ra_progression` de `GET /api/users/me`, rempli seulement si
//!   le joueur a mis son pseudo RetroAchievements dans son profil RomM.
//!
//! RomM ne relit cette progression sur RetroAchievements que par « Rafraîchir » dans le profil
//! RomM, ou par sa tâche planifiée (si l'administrateur l'a activée). Le launcher ne la
//! rafraîchit pas lui-même : la route demande le scope `me.write`, que son jeton n'a pas.
//!
//! La progression des amis vient du gestionnaire (`GET /api/succes/jeux/<raId>`), qui la lit
//! dans RomM avec son jeton administrateur. Un gestionnaire trop ancien répond 404 : on se tait.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::romm::client::{parse_json, RommClient};
use crate::state::AppState;

// ---------- Schémas RomM (seuls les champs utiles, tolérants) ----------

#[derive(Debug, Default, Deserialize)]
struct RawRom {
    #[serde(default)]
    ra_id: Option<i64>,
    #[serde(default)]
    merged_ra_metadata: Option<RawRaMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct RawRaMetadata {
    #[serde(default)]
    achievements: Vec<RawAchievement>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAchievement {
    #[serde(default)]
    ra_id: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    points: Option<i64>,
    /// Nombre de joueurs RetroAchievements qui l'ont (tous modes).
    #[serde(default)]
    num_awarded: Option<i64>,
    /// Nom du badge : c'est lui qui relie un succès à ceux débloqués par le joueur.
    #[serde(default)]
    badge_id: Option<String>,
    /// Image gardée par RomM : `/assets/romm/resources/roms/<plateforme>/<rom>/retroachievements/badges/<badge>.png`.
    #[serde(default)]
    badge_path: Option<String>,
    #[serde(default)]
    badge_path_lock: Option<String>,
    /// `progression`, `win_condition`, `missable`… ou vide.
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMe {
    #[serde(default)]
    ra_username: Option<String>,
    #[serde(default)]
    ra_progression: Option<RawProgression>,
}

#[derive(Debug, Default, Deserialize)]
struct RawProgression {
    #[serde(default)]
    results: Vec<RawGameProgress>,
}

#[derive(Debug, Default, Deserialize)]
struct RawGameProgress {
    #[serde(default)]
    rom_ra_id: Option<i64>,
    #[serde(default)]
    most_recent_awarded_date: Option<String>,
    /// `mastered`, `completed`, `beaten-hardcore`, `beaten-softcore`, ou `null`.
    #[serde(default)]
    highest_award_kind: Option<String>,
    #[serde(default)]
    earned_achievements: Vec<RawEarned>,
}

#[derive(Debug, Default, Deserialize)]
struct RawEarned {
    /// Nom du badge du succès (pas son identifiant), en texte… ou en nombre, par prudence.
    #[serde(default)]
    id: Value,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    date_hardcore: Option<String>,
}

// ---------- Modèle FarfaLauncher ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Achievement {
    pub id: Option<i64>,
    pub title: String,
    pub description: String,
    pub points: i64,
    /// Image du badge sur RomM (URL complète, à passer à `image_path`), en couleur et grisée.
    pub badge_url: Option<String>,
    pub badge_lock_url: Option<String>,
    /// Date du déblocage (RFC 3339, UTC), `None` s'il n'est pas débloqué.
    pub unlocked: Option<String>,
    /// Date du déblocage en mode hardcore, s'il l'a été dans ce mode.
    pub unlocked_hardcore: Option<String>,
    pub kind: Option<String>,
    pub players: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameAchievements {
    /// Identifiant RetroAchievements du jeu (sert à demander les amis au gestionnaire).
    pub ra_id: i64,
    /// Le joueur a mis son pseudo RetroAchievements dans son profil RomM.
    pub linked: bool,
    /// RomM a déjà relu sa progression sur ce jeu (sinon : jamais joué, ou pas encore rafraîchie).
    pub progress_known: bool,
    pub total: usize,
    pub total_points: i64,
    pub earned: usize,
    pub earned_hardcore: usize,
    pub earned_points: i64,
    /// Plus haute récompense RetroAchievements (`mastered`, `beaten-hardcore`…).
    pub award: Option<String>,
    /// Date du dernier succès débloqué (RFC 3339).
    pub last_unlock: Option<String>,
    pub achievements: Vec<Achievement>,
}

/// Date RetroAchievements en RFC 3339. RomM recopie celles de l'API RetroAchievements
/// (`2026-08-30 22:10:38`, en UTC) ; celles déjà en RFC 3339 restent telles quelles.
fn rfc3339(date: &str) -> String {
    let d = date.trim();
    let bytes = d.as_bytes();
    if d.len() == 19 && bytes[10] == b' ' {
        format!("{}T{}Z", &d[..10], &d[11..])
    } else {
        d.to_string()
    }
}

/// Nom de badge lisible, qu'il soit écrit en texte ou en nombre.
fn badge_name(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Image de RomM en URL complète ; seulement un chemin de ses ressources statiques.
fn badge_url(base: &str, path: Option<&str>) -> Option<String> {
    let path = path?;
    (path.starts_with("/assets/") && !path.contains("..")).then(|| format!("{base}{path}"))
}

/// Assemble la section : succès du jeu, et ce que le joueur en a débloqué.
/// `None` si le jeu n'a pas de succès RetroAchievements.
fn combine(rom: RawRom, me: RawMe, base: &str) -> Option<GameAchievements> {
    let ra_id = rom.ra_id.filter(|id| *id > 0)?;
    let list = rom.merged_ra_metadata.unwrap_or_default().achievements;
    if list.is_empty() {
        return None;
    }
    let linked = me.ra_username.as_deref().is_some_and(|u| !u.trim().is_empty());
    let progress = if linked {
        me.ra_progression.unwrap_or_default().results.into_iter().find(|p| p.rom_ra_id == Some(ra_id))
    } else {
        None
    };
    // Succès débloqués, par nom de badge.
    let earned: std::collections::HashMap<String, &RawEarned> = progress
        .as_ref()
        .map(|p| p.earned_achievements.iter().filter_map(|e| Some((badge_name(&e.id)?, e))).collect())
        .unwrap_or_default();

    let achievements: Vec<Achievement> = list
        .into_iter()
        .map(|a| {
            let got = a.badge_id.as_ref().and_then(|b| earned.get(b));
            Achievement {
                id: a.ra_id,
                title: a.title.unwrap_or_default(),
                description: a.description.unwrap_or_default(),
                points: a.points.unwrap_or(0).max(0),
                badge_url: badge_url(base, a.badge_path.as_deref()),
                badge_lock_url: badge_url(base, a.badge_path_lock.as_deref()),
                // Un succès hardcore compte aussi en mode normal, même sans `date`.
                unlocked: got.and_then(|e| e.date.as_deref().or(e.date_hardcore.as_deref())).map(rfc3339),
                unlocked_hardcore: got.and_then(|e| e.date_hardcore.as_deref()).filter(|d| !d.is_empty()).map(rfc3339),
                kind: a.kind.filter(|k| !k.is_empty()),
                players: a.num_awarded,
            }
        })
        .collect();

    let unlocked: Vec<&Achievement> = achievements.iter().filter(|a| a.unlocked.is_some()).collect();
    Some(GameAchievements {
        ra_id,
        linked,
        progress_known: progress.is_some(),
        total: achievements.len(),
        total_points: achievements.iter().map(|a| a.points).sum(),
        earned: unlocked.len(),
        earned_hardcore: unlocked.iter().filter(|a| a.unlocked_hardcore.is_some()).count(),
        earned_points: unlocked.iter().map(|a| a.points).sum(),
        award: progress.as_ref().and_then(|p| p.highest_award_kind.clone()).filter(|k| !k.is_empty()),
        last_unlock: progress.as_ref().and_then(|p| p.most_recent_awarded_date.as_deref()).map(rfc3339),
        achievements,
    })
}

/// Succès d'un jeu et progression du joueur (deux lectures RomM en parallèle).
/// `None` : le jeu n'a pas de succès RetroAchievements.
pub async fn game(state: &AppState, rom_id: i64) -> AppResult<Option<GameAchievements>> {
    let client = state.online_client()?;
    let rom_path = format!("api/roms/{rom_id}/simple");
    let (rom, me) = tokio::join!(client.get_json::<RawRom>(&rom_path), client.get_json::<RawMe>("api/users/me"));
    Ok(combine(rom?, me?, client.base()))
}

/// Progression des autres comptes sur ce jeu, d'après le gestionnaire. `None` : gestionnaire
/// absent ou trop ancien (404).
pub async fn friends(state: &AppState, ra_id: i64) -> AppResult<Option<Value>> {
    let client: RommClient = crate::presence::manager(state)?;
    match client.send(client.get(&format!("api/succes/jeux/{ra_id}"))).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        Err(AppError::Server) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://romm.example.org";

    fn rom() -> RawRom {
        serde_json::from_str(
            r#"{"id":8133,"ra_id":3732,"merged_ra_metadata":{"first_release_date":null,"achievements":[
              {"ra_id":171106,"title":"Bienvenue","description":"Finir la quête 1","points":3,"num_awarded":3711,
               "badge_id":"195541","badge_path":"/assets/romm/resources/roms/9/8133/retroachievements/badges/195541.png",
               "badge_path_lock":"/assets/romm/resources/roms/9/8133/retroachievements/badges/195541_lock.png","type":"progression"},
              {"ra_id":171107,"title":"Deux","description":"","points":10,"badge_id":"195542","badge_path":"/../x.png","type":""},
              {"ra_id":171108,"title":"Trois","description":"","points":25,"badge_id":"195543"}
            ]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn earned_achievements_are_matched_by_badge() {
        let me: RawMe = serde_json::from_str(
            r#"{"id":1,"ra_username":"joueur","ra_progression":{"total":2,"results":[
              {"rom_ra_id":1,"earned_achievements":[{"id":"195543","date":"2026-01-01 00:00:00"}]},
              {"rom_ra_id":3732,"max_possible":3,"num_awarded":2,"most_recent_awarded_date":"2026-09-17T22:30:23+00:00",
               "highest_award_kind":null,"earned_achievements":[
                 {"id":"195541","date":"2026-08-30 22:10:38","date_hardcore":"2026-08-30 22:10:38"},
                 {"id":195542,"date":"2026-08-31 10:00:00"}]}]}}"#,
        )
        .unwrap();
        let g = combine(rom(), me, BASE).unwrap();
        assert_eq!((g.ra_id, g.linked, g.progress_known), (3732, true, true));
        assert_eq!((g.total, g.total_points, g.earned, g.earned_hardcore, g.earned_points), (3, 38, 2, 1, 13));
        assert_eq!(g.award, None);
        assert_eq!(g.last_unlock.as_deref(), Some("2026-09-17T22:30:23+00:00"));
        let a = &g.achievements[0];
        assert_eq!(a.unlocked.as_deref(), Some("2026-08-30T22:10:38Z"));
        assert_eq!(a.unlocked_hardcore.as_deref(), Some("2026-08-30T22:10:38Z"));
        assert_eq!(a.badge_url.as_deref(), Some("https://romm.example.org/assets/romm/resources/roms/9/8133/retroachievements/badges/195541.png"));
        assert_eq!(a.kind.as_deref(), Some("progression"));
        // Badge en nombre : relié quand même ; chemin d'image douteux : pas d'image.
        assert!(g.achievements[1].unlocked.is_some() && g.achievements[1].unlocked_hardcore.is_none());
        assert_eq!((g.achievements[1].badge_url.as_ref(), g.achievements[1].kind.as_ref()), (None, None));
        // Le succès d'un autre jeu au même badge ne compte pas.
        assert!(g.achievements[2].unlocked.is_none());
        assert_eq!(serde_json::to_value(&g).unwrap()["earnedHardcore"], 1);
    }

    #[test]
    fn without_username_or_progress() {
        // Pas de pseudo dans RomM : la liste, sans rien de débloqué (même s'il reste une vieille progression).
        let me: RawMe = serde_json::from_str(r#"{"ra_username":"","ra_progression":{"results":[{"rom_ra_id":3732,"earned_achievements":[{"id":"195541","date":"2026-08-30 22:10:38"}]}]}}"#).unwrap();
        let g = combine(rom(), me, BASE).unwrap();
        assert_eq!((g.linked, g.progress_known, g.earned, g.total), (false, false, 0, 3));
        // Pseudo, mais jeu jamais relu par RomM.
        let me: RawMe = serde_json::from_str(r#"{"ra_username":"joueur","ra_progression":null}"#).unwrap();
        let g = combine(rom(), me, BASE).unwrap();
        assert_eq!((g.linked, g.progress_known, g.earned), (true, false, 0));
    }

    #[test]
    fn no_achievements_no_section() {
        let me = RawMe::default();
        assert!(combine(serde_json::from_str(r#"{"ra_id":null,"merged_ra_metadata":null}"#).unwrap(), RawMe::default(), BASE).is_none());
        assert!(combine(serde_json::from_str(r#"{"ra_id":5,"merged_ra_metadata":{"achievements":[]}}"#).unwrap(), me, BASE).is_none());
        assert!(combine(serde_json::from_str(r#"{"ra_id":5}"#).unwrap(), RawMe::default(), BASE).is_none());
    }

    #[test]
    fn dates() {
        assert_eq!(rfc3339("2026-08-30 22:10:38"), "2026-08-30T22:10:38Z");
        assert_eq!(rfc3339("2026-09-17T22:30:23+00:00"), "2026-09-17T22:30:23+00:00");
    }
}
