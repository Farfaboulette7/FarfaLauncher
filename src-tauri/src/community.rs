//! Rapports de bug, avis et signalements d'un jeu, demandes du joueur : le cœur partagé entre le
//! launcher (`commands/report.rs`, `commands/community.rs`) et le moteur du Steam Deck (`deck.rs`).
//!
//! Un avis est une note RomM publique (`/api/roms/<id>/notes`), une par joueur et par jeu, de titre
//! `Avis`. RomM ne laisse lire à un compte que ses propres notes de jeu (`rating`, `difficulty`,
//! `completion`) : elles voyagent donc aussi dans les étiquettes de l'avis (`note:8`,
//! `difficulte:4`, `completion:100`), et sont posées dans RomM pour le joueur.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{log_err, AppError, AppResult};
use crate::install::state;
use crate::romm::client::{parse_json, RommClient};
use crate::state::AppState;

/// Titre des avis écrits par le launcher.
const REVIEW_TITLE: &str = "Avis";

// ---------------------------------------------------------------- rapports de bug

/// Fichiers qui seront joints au rapport de ce jeu (montrés avant l'envoi).
pub fn report_files(state: &AppState, rom_id: i64) -> AppResult<Vec<String>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let mut names = vec!["rapport.json (PC, versions, jeu, réglages)".to_string(), "installation.json".to_string()];
    names.extend(crate::report::attachments(&state.catalog(), &state.config_dir, &state.log_dir, &st).into_iter().map(|a| a.name));
    Ok(names)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportReceipt {
    id: String,
}

/// Client du gestionnaire (même jeton que RomM).
fn manager(state: &AppState) -> AppResult<RommClient> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::ServerUnreachable)?;
    Ok(state.client()?.with_base(&base))
}

/// Construit le rapport et l'envoie ; rend son identifiant sur le gestionnaire.
pub async fn send_report(state: &AppState, rom_id: i64, description: &str, after_crash: bool) -> AppResult<String> {
    let description = description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::Internal);
    }
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let catalog = state.catalog();
    let client = manager(state)?;
    let hw = crate::hardware::get();
    let title = crate::switch_saves::title_id(&catalog, &state.config_dir, &st);
    let game = crate::optimize::GameRef { rom_id, title: title.as_deref() };
    let attachments = crate::report::attachments(&catalog, &state.config_dir, &state.log_dir, &st);
    let info = crate::report::ReportInfo {
        launcher: env!("CARGO_PKG_VERSION"),
        created_at: state::now(),
        description: &description,
        after_crash,
        os: format!("{} ({})", hw.os_version, hw.os),
        hardware: hw,
        game: st.name.clone(),
        rom_id,
        runner: crate::report::runner(&catalog, &st),
        optimizations: crate::optimize::summary(&catalog.optimize_for_install(&st), hw, &game),
        optimizations_enabled: !state.settings().optimize_off.contains(&rom_id),
        files: attachments.iter().map(|a| a.name.clone()).collect(),
    };
    let body = crate::report::build(&info, &st, &attachments).map_err(|e| log_err(AppError::Internal, "construction du rapport", e))?;
    tracing::info!("rapport de bug pour « {} » : {} fichier(s), {} Ko", st.name, attachments.len() + 2, body.len() >> 10);
    let resp = client
        .send(client.put_bytes(&format!("api/reports?rom={rom_id}"), body).timeout(std::time::Duration::from_secs(120)))
        .await?;
    let receipt: ReportReceipt = resp.json().await.map_err(|e| log_err(AppError::Server, "réponse au rapport", e))?;
    tracing::info!("rapport envoyé : {}", receipt.id);
    // Envoyé d'ici : pas une nouvelle à signaler (la réponse, plus tard, en sera une).
    mark_seen_now(state, &receipt.id)?;
    Ok(receipt.id)
}

/// Le rapport `id` vient d'être écrit d'ici : vu (la prochaine nouvelle sera la réponse).
fn mark_seen_now(state: &AppState, id: &str) -> AppResult<()> {
    let seen_at = state::now() + 120;
    state.update_settings(|s| {
        s.reports_seen.insert(id.to_string(), seen_at);
    })?;
    Ok(())
}

/// Rapports envoyés par ce joueur (tous ses appareils), avec la réponse après analyse.
pub async fn my_reports(state: &AppState) -> AppResult<Value> {
    manager(state)?.get_json("api/reports").await
}

/// Réponse du joueur à l'analyse d'un de ses rapports (le support lui répondra dans le fil).
pub async fn reply_report(state: &AppState, id: &str, text: &str) -> AppResult<Value> {
    let text = text.trim();
    if text.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(AppError::Internal);
    }
    let client = manager(state)?;
    let resp = client.send(client.post(&format!("api/reports/{id}/reply"), &json!({ "texte": text }))).await?;
    let thread: Value = resp.json().await.map_err(|e| log_err(AppError::Server, "réponse au message", e))?;
    tracing::info!("réponse envoyée au rapport {id}");
    mark_seen_now(state, id)?;
    Ok(thread)
}

/// Le support (le PC qui analyse les rapports) est-il allumé ? `{ "enLigne": bool, "vuA": secondes|null }`.
pub async fn support_status(state: &AppState) -> AppResult<Value> {
    manager(state)?.get_json("api/reports/support").await
}

/// « Ne plus demander pour ce jeu » (`ask = false`) ou réactive la proposition de rapport après
/// un plantage.
pub fn set_crash_prompt(state: &AppState, rom_id: i64, ask: bool) -> AppResult<()> {
    state.update_settings(|s| {
        s.crash_prompt_off.retain(|id| *id != rom_id);
        if !ask {
            s.crash_prompt_off.push(rom_id);
        }
    })?;
    tracing::info!("rapport proposé après un plantage du jeu {rom_id} : {}", if ask { "oui" } else { "non" });
    Ok(())
}

/// Faut-il proposer tout de suite un rapport après ce plantage ?
pub fn ask_report(crash: Option<&crate::launch::Crash>, prompt_off: &[i64], rom_id: i64) -> bool {
    crash.is_some() && !prompt_off.contains(&rom_id)
}

// ---------------------------------------------------------------- avis

/// Avis publics du jeu (et les notes privées du joueur), avec l'ID du joueur.
pub async fn game_reviews(state: &AppState, rom_id: i64) -> AppResult<Value> {
    let client = state.client()?;
    let me = crate::romm::auth::me(&client).await?;
    let notes: Vec<Value> = client.get_json(&format!("api/roms/{rom_id}/notes")).await?;
    let notes: Vec<Value> = notes.into_iter().filter(|n| n["is_public"].as_bool() == Some(true) || n["user_id"].as_i64() == Some(me.id)).collect();
    Ok(json!({ "me": me.id, "notes": notes }))
}

/// Notes du joueur sur le jeu (0 : pas de note).
fn clamp(v: Option<i64>, max: i64) -> Option<i64> {
    v.map(|v| v.clamp(0, max))
}

/// Étiquettes de l'avis : ses notes, lisibles par les autres joueurs.
fn review_tags(rating: Option<i64>, difficulty: Option<i64>, completion: Option<i64>) -> Vec<String> {
    [("note", rating), ("difficulte", difficulty), ("completion", completion)]
        .into_iter()
        .filter_map(|(k, v)| v.filter(|v| *v > 0).map(|v| format!("{k}:{v}")))
        .collect()
}

/// Publie ou met à jour l'avis du joueur, et ses notes dans RomM.
pub async fn save_game_review(
    state: &AppState,
    rom_id: i64,
    note_id: Option<i64>,
    content: &str,
    rating: Option<i64>,
    difficulty: Option<i64>,
    completion: Option<i64>,
) -> AppResult<Value> {
    let client = state.client()?;
    let (rating, difficulty, completion) = (clamp(rating, 10), clamp(difficulty, 10), clamp(completion, 100));
    let body = json!({
        "title": REVIEW_TITLE,
        "content": content.trim(),
        "is_public": true,
        "tags": review_tags(rating, difficulty, completion),
    });
    let req = match note_id {
        Some(id) => client.put(&format!("api/roms/{rom_id}/notes/{id}"), &body),
        None => client.post(&format!("api/roms/{rom_id}/notes"), &body),
    };
    let note: Value = parse_json(client.send(req).await?).await?;
    // Ses notes dans RomM aussi (page du jeu sur RomM) ; l'avis est déjà publié si ça échoue.
    let props = json!({ "rating": rating.unwrap_or(0), "difficulty": difficulty.unwrap_or(0), "completion": completion.unwrap_or(0) });
    if let Err(e) = client.send(client.put(&format!("api/roms/{rom_id}/props"), &props)).await {
        tracing::warn!("notes du jeu non posées dans RomM: {}", e.code());
    }
    Ok(note)
}

pub async fn delete_game_review(state: &AppState, rom_id: i64, note_id: i64) -> AppResult<()> {
    let client = state.client()?;
    client.send(client.delete(&format!("api/roms/{rom_id}/notes/{note_id}"), &json!({}))).await?;
    Ok(())
}

// ---------------------------------------------------------------- signalements et demandes

/// Signalements du jeu, tous joueurs confondus. `None` : gestionnaire sans cette liste.
pub async fn game_reports(state: &AppState, rom_id: i64) -> AppResult<Option<Value>> {
    let client = crate::presence::manager(state)?;
    match client.send(client.get(&format!("api/reports/game/{rom_id}"))).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        Err(AppError::Server) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Envoie une demande de jeu (nom, console, lien et message facultatifs).
pub async fn send_game_request(state: &AppState, name: &str, console: &str, link: &str, message: &str) -> AppResult<Value> {
    if name.trim().is_empty() || console.trim().is_empty() {
        return Err(AppError::Internal);
    }
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    let client = state.online_client()?.with_base(&base);
    let body = json!({ "name": name.trim(), "console": console.trim(), "link": link.trim(), "message": message.trim() });
    let created: Value = parse_json(client.send(client.post("api/requests", &body)).await?).await?;
    tracing::info!("demande de jeu envoyée : {} ({})", name.trim(), created["id"].as_str().unwrap_or("?"));
    Ok(created)
}

/// Demandes de jeux du joueur.
pub async fn my_game_requests(state: &AppState) -> AppResult<Value> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    state.online_client()?.with_base(&base).get_json("api/requests").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Crash;

    #[test]
    fn scores_travel_in_the_tags() {
        assert_eq!(review_tags(Some(8), Some(0), Some(100)), ["note:8", "completion:100"]);
        assert!(review_tags(None, None, None).is_empty());
        assert_eq!(clamp(Some(14), 10), Some(10));
    }

    #[test]
    fn asks_after_a_crash_unless_muted_for_this_game() {
        let crash = Crash { seconds: 3, error: None };
        assert!(ask_report(Some(&crash), &[], 7));
        assert!(ask_report(Some(&crash), &[8], 7));
        assert!(!ask_report(Some(&crash), &[7], 7));
        assert!(!ask_report(None, &[], 7));
    }
}
