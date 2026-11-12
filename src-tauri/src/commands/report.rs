//! Rapports de bug envoyés par le joueur (report.rs) à FarfaRomManager.

use serde::Deserialize;
use tauri::State;

use crate::error::{log_err, AppError, AppResult};
use crate::install::state;
use crate::state::AppState;

/// Fichiers qui seront joints au rapport de ce jeu (montrés avant l'envoi).
#[tauri::command]
pub fn report_files(state: State<'_, AppState>, rom_id: i64) -> AppResult<Vec<String>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let mut names = vec!["rapport.json (PC, versions, jeu, réglages)".to_string(), "installation.json".to_string()];
    names.extend(crate::report::attachments(&state.catalog(), &state.log_dir, &st).into_iter().map(|a| a.name));
    Ok(names)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportReceipt {
    pub id: String,
}

/// Construit le rapport et l'envoie ; rend son identifiant sur le gestionnaire.
#[tauri::command]
pub async fn send_report(state: State<'_, AppState>, rom_id: i64, description: String, after_crash: bool) -> AppResult<String> {
    let description = description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::Internal);
    }
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let catalog = state.catalog();
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::ServerUnreachable)?;
    let client = state.client()?.with_base(&base);
    let hw = crate::hardware::get();
    let title = crate::switch_saves::title_id(&catalog, &state.config_dir, &st);
    let game = crate::optimize::GameRef { rom_id, title: title.as_deref() };
    let attachments = crate::report::attachments(&catalog, &state.log_dir, &st);
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
    let seen_at = state::now() + 120;
    let id = receipt.id.clone();
    state.update_settings(|s| {
        s.reports_seen.insert(id, seen_at);
    })?;
    Ok(receipt.id)
}

/// Rapports envoyés par ce joueur (tous ses PC), avec la réponse après analyse.
#[tauri::command]
pub async fn my_reports(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::ServerUnreachable)?;
    let client = state.client()?.with_base(&base);
    client.get_json("api/reports").await
}

/// Rapports lus : identifiant → `updatedAt` vu.
#[tauri::command]
pub fn reports_seen(state: State<'_, AppState>) -> std::collections::BTreeMap<String, u64> {
    state.settings().reports_seen
}

/// Retient les rapports qui viennent d'être lus.
#[tauri::command]
pub fn mark_reports_seen(state: State<'_, AppState>, seen: std::collections::BTreeMap<String, u64>) -> AppResult<()> {
    state.update_settings(|s| s.reports_seen.extend(seen))?;
    Ok(())
}
