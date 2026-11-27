//! État d'un jeu pour les listes (badges et filtres), comme les cartes de la bibliothèque du
//! launcher. Portage fidèle de `src/api/backend.ts` (`isAvailable`, `emulatorReady`,
//! `effectiveEmulator`, `hasUpdate`) et de `src/screens/Library.tsx` (`playModes`, `isNew`) :
//! une règle changée là-bas doit l'être ici aussi. Sert au moteur du Steam Deck (NieRDeck).

use crate::install::state::InstalledGame;
use crate::manager::EmulatorOffer;
use crate::romm::library::Game;

/// Un jeu est « nouveau » pendant 24 h après son ajout sur le serveur (`NEW_FOR_MS`).
pub const NEW_FOR_SECS: i64 = 24 * 3600;

/// Le jeu a une archive pour ce système, ou c'est un jeu PC qui se lance (`isAvailable`).
pub fn is_available(g: &Game) -> bool {
    g.my_archive.is_some() || g.pc.as_ref().is_some_and(|p| p.runnable)
}

/// Émulateur installable pour ce jeu sur ce système (`emulatorReady`).
pub fn emulator_ready(g: &Game) -> bool {
    // Jeu fourni en archive de données à décompresser pour sa recomp : rien qu'un émulateur
    // puisse lancer.
    let only_data = g.details_loaded
        && g.emulator_rom_bytes == 0
        && !g.rom_files.is_empty()
        && g.rom_files.iter().all(|f| f.file_name.to_lowercase().ends_with(".extract.zip"));
    !only_data && g.emulator.as_ref().is_some_and(|e| e.available && e.config_missing.is_empty())
}

/// Comment ce jeu se joue sur ce système : recomp, émulateur, ou les deux (`playModes`).
pub fn play_modes(g: &Game) -> (bool, bool) {
    if g.pc.is_some() {
        return (false, false);
    }
    (g.recomp && (is_available(g) || !g.details_loaded), emulator_ready(g))
}

fn is_emulator_install(inst: &InstalledGame) -> bool {
    inst.shared.as_ref().is_some_and(|s| s.is_emulator())
}

/// Émulateur avec lequel le jeu est installé, sinon celui proposé (`effectiveEmulator`).
fn effective_emulator<'a>(g: &'a Game, inst: &InstalledGame) -> Option<&'a EmulatorOffer> {
    match inst.shared.as_ref().filter(|_| is_emulator_install(inst)) {
        Some(shared) => g.emulators.iter().find(|e| e.runtime_rom_id == shared.runtime_rom_id).or(g.emulator.as_ref()),
        None => g.emulator.as_ref(),
    }
}

/// Une mise à jour attend ce jeu installé (`hasUpdate`).
pub fn has_update(g: &Game, inst: Option<&InstalledGame>) -> bool {
    let Some(inst) = inst.filter(|i| i.executable.is_some()) else { return false };
    // Installé avec l'émulateur : c'est lui qui est suivi, pas la recomp du jeu.
    let remote = if is_emulator_install(inst) {
        effective_emulator(g, inst).map(|o| if o.game_version_key.is_empty() { o.version_key.as_str() } else { o.game_version_key.as_str() })
    } else {
        g.server_version_raw.as_deref()
    };
    remote.is_some_and(|r| !r.is_empty() && Some(r) != inst.version_raw.as_deref())
}

/// Ajouté sur le serveur il y a moins de 24 h (`isNew`, sans les jeux déjà ouverts, que
/// l'interface retient elle-même).
pub fn is_new(created_at: &str, now: i64) -> bool {
    crate::save_sync::parse_server_date(created_at).is_some_and(|t| now - t < NEW_FOR_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(json: &str) -> Game {
        let mut base: serde_json::Value = serde_json::from_str(
            r#"{"id":1,"platformId":1,"name":"J","folderName":"j","summary":null,"coverUrl":null,"box3dUrl":null,"boxFaces":null,
            "screenshotUrls":[],"videoUrl":null,"genres":[],"developers":[],"releaseYear":null,"manuals":[],"archives":[],
            "myArchive":null,"romFiles":[],"emulatorRomBytes":0,"mods":[],"pc":null,"runtime":null,"variant":null,"players":null,
            "console":"n64","recomp":false,"emulator":null,"emulators":[],"recomps":[],"recompId":null,"versionFile":null,
            "serverVersion":null,"serverVersionRaw":null,"detailsLoaded":true,"createdAt":"","updatedAt":""}"#,
        )
        .unwrap();
        let extra: serde_json::Value = serde_json::from_str(json).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    const OFFER: &str = r#"{"id":"e","name":"E","version":"1","versionKey":"k1","runtimeRomId":-5,"available":true,"size":1,
        "systems":[],"repo":null,"configMissing":[],"gameVersionKey":"","packsSize":0,"advice":null,"note":null}"#;

    #[test]
    fn modes_follow_the_launcher() {
        // Recomp sans fiche lue : comptée comme jouable (la carte dit « Recomp »).
        assert_eq!(play_modes(&game(r#"{"recomp":true,"detailsLoaded":false}"#)), (true, false));
        // Recomp lue sans archive pour ce système : pas jouable.
        assert_eq!(play_modes(&game(r#"{"recomp":true}"#)), (false, false));
        // Émulateur disponible.
        assert_eq!(play_modes(&game(&format!(r#"{{"emulator":{OFFER}}}"#))), (false, true));
        // Émulateur dont la configuration manque : pas prêt.
        let missing = OFFER.replace(r#""configMissing":[]"#, r#""configMissing":["bios"]"#);
        assert_eq!(play_modes(&game(&format!(r#"{{"emulator":{missing}}}"#))), (false, false));
        // Données à décompresser pour la recomp : rien pour l'émulateur.
        let data = format!(r#"{{"emulator":{OFFER},"romFiles":[{{"id":1,"fileName":"d.extract.zip","relPath":"d.extract.zip","size":3,"sha1":null,"md5":null}}]}}"#);
        assert_eq!(play_modes(&game(&data)), (false, false));
    }

    #[test]
    fn new_for_a_day() {
        let added = "2027-01-15T07:00:00Z";
        let t = crate::save_sync::parse_server_date(added).unwrap();
        assert!(is_new(added, t + 3600));
        assert!(!is_new(added, t + NEW_FOR_SECS + 1));
        assert!(!is_new("", t));
    }

    fn installed(json: serde_json::Value) -> InstalledGame {
        let mut base = serde_json::json!({
            "romId": 1, "name": "J", "version": null, "target": "Windows", "archiveName": "", "executable": "j.exe",
            "gameDir": "/jeux/j", "installedAt": 0, "updatedAt": 0
        });
        for (k, v) in json.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn update_like_the_launcher() {
        let g = game(r#"{"recomp":true,"serverVersionRaw":"v2"}"#);
        assert!(!has_update(&g, None));
        assert!(!has_update(&g, Some(&installed(serde_json::json!({ "versionRaw": "v2" })))));
        assert!(has_update(&g, Some(&installed(serde_json::json!({ "versionRaw": "v1" })))));
        // Installation inachevée (pas d'exécutable) : pas de mise à jour à proposer.
        assert!(!has_update(&g, Some(&installed(serde_json::json!({ "versionRaw": "v1", "executable": null })))));
        // Installé avec l'émulateur : c'est sa version qui compte, pas celle de la recomp.
        let emu = game(&format!(r#"{{"recomp":true,"serverVersionRaw":"v2","emulator":{OFFER},"emulators":[{OFFER}]}}"#));
        let shared = serde_json::json!({ "runtimeRomId": -5, "game": "e", "runtimeName": "E", "dataDir": "", "saveDir": "", "kind": "emulator" });
        assert!(!has_update(&emu, Some(&installed(serde_json::json!({ "versionRaw": "k1", "shared": shared.clone() })))));
        assert!(has_update(&emu, Some(&installed(serde_json::json!({ "versionRaw": "k0", "shared": shared })))));
    }
}
