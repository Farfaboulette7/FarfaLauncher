//! Réglages optimisés selon le PC. FarfaRomManager décrit, pour chaque émulateur ou recomp,
//! des règles (`optimiser:`) : conditions sur le matériel (niveau, marque de la carte,
//! mémoire vidéo, système…), jeu visé, et réglage à écrire. Le launcher les pose à chaque
//! lancement, tant que le joueur laisse « Réglages optimisés » activé pour ce jeu.
//!
//! Ordre : celui de la fiche ; la dernière règle qui correspond l'emporte (le général
//! d'abord, le spécifique ensuite).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hardware::Hardware;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    /// `niveau`, `gpu`, `os`, `vram`, `ram`, `fils`.
    pub key: String,
    /// `=`, `<`, `<=`, `>`, `>=`.
    pub op: String,
    /// Texte ou nombre dans le catalogue (`vram<9` : 9).
    #[serde(deserialize_with = "text_or_number")]
    pub value: String,
}

fn text_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizeRule {
    /// Toutes à remplir ; vide : toujours.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// `tous`, un ID de titre Switch ou un romId.
    #[serde(default = "all_games")]
    pub game: String,
    /// `ini`, `kv`, `json`, `env`, `ligne` (`CLÉ=VALEUR`), `eden-jeu` (réglages par jeu
    /// des émulateurs de la famille Eden : `user/config/custom/{titleid}.ini`).
    pub format: String,
    #[serde(default)]
    pub file: Option<String>,
    pub key: String,
    pub value: String,
    /// Ce que voit le joueur.
    #[serde(default)]
    pub label: Option<String>,
}

fn all_games() -> String {
    "tous".into()
}

/// Le jeu lancé, pour choisir les règles qui le visent.
pub struct GameRef<'a> {
    pub rom_id: i64,
    /// ID de titre Switch en majuscules, s'il est connu.
    pub title: Option<&'a str>,
}

/// Réglage retenu pour ce lancement, montré au joueur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub label: String,
    pub key: String,
    pub value: String,
}

impl Condition {
    fn holds(&self, hw: &Hardware) -> bool {
        const GO: f64 = (1u64 << 30) as f64;
        let text = match self.key.as_str() {
            "niveau" => Some(hw.level.to_string()),
            "gpu" => Some(hw.gpu.as_ref().map(|g| g.vendor.clone()).unwrap_or_default()),
            "os" => Some(hw.os.clone()),
            _ => None,
        };
        if let Some(text) = text {
            return self.op == "=" && text.eq_ignore_ascii_case(&self.value);
        }
        let have = match self.key.as_str() {
            "vram" => hw.gpu.as_ref().map(|g| g.vram as f64 / GO).unwrap_or(0.0),
            "ram" => hw.ram as f64 / GO,
            "fils" => hw.cpu_threads as f64,
            _ => return false,
        };
        let Ok(want) = self.value.parse::<f64>() else { return false };
        // La mémoire annoncée est un peu sous la valeur ronde (16 Go vus comme 15,9).
        let have = if self.key == "fils" { have } else { (have * 2.0).round() / 2.0 };
        match self.op.as_str() {
            "=" => have == want,
            "<" => have < want,
            "<=" => have <= want,
            ">" => have > want,
            ">=" => have >= want,
            _ => false,
        }
    }
}

impl OptimizeRule {
    fn matches(&self, hw: &Hardware, game: &GameRef) -> bool {
        let game_ok = self.game.eq_ignore_ascii_case("tous")
            || game.title.is_some_and(|t| t.eq_ignore_ascii_case(&self.game))
            || self.game.parse::<i64>().is_ok_and(|id| id == game.rom_id);
        game_ok && self.conditions.iter().all(|c| c.holds(hw))
    }

    fn label(&self) -> String {
        self.label.clone().unwrap_or_else(|| format!("{} = {}", self.key, self.value))
    }
}

/// Règles qui s'appliquent à ce jeu sur ce PC, dans l'ordre de la fiche.
pub fn matching<'a>(rules: &'a [OptimizeRule], hw: &Hardware, game: &GameRef) -> Vec<&'a OptimizeRule> {
    rules.iter().filter(|r| r.matches(hw, game)).collect()
}

/// Ce que verra le joueur : un réglage par clé (le dernier l'emporte), dans l'ordre. Les
/// règles sans libellé (lignes techniques, comme `clé\default=false`) ne sont pas montrées.
pub fn summary(rules: &[OptimizeRule], hw: &Hardware, game: &GameRef) -> Vec<Applied> {
    let mut out: Vec<(String, Applied)> = Vec::new();
    for r in matching(rules, hw, game).into_iter().filter(|r| r.label.is_some()) {
        let id = format!("{}|{}|{}", r.format, r.file.as_deref().unwrap_or(""), r.key);
        out.retain(|(k, _)| *k != id);
        out.push((id, Applied { label: r.label(), key: r.key.clone(), value: r.value.clone() }));
    }
    out.into_iter().map(|(_, a)| a).collect()
}

/// Pose les réglages et rend les variables d'environnement à donner au programme.
pub fn apply(rules: &[OptimizeRule], hw: &Hardware, game: &GameRef, exe_dir: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for rule in matching(rules, hw, game) {
        if rule.format == "env" {
            env.retain(|(k, _)| *k != rule.key);
            env.push((rule.key.clone(), rule.value.replace("{exedir}", &exe_dir.to_string_lossy())));
            tracing::info!("réglage optimisé : {} = {} (environnement)", rule.key, rule.value);
            continue;
        }
        match write(rule, game, exe_dir) {
            Ok(path) => tracing::info!("réglage optimisé : {} = {} dans {}", rule.key, rule.value, path.display()),
            Err(e) => tracing::warn!("réglage optimisé non posé ({} {}) : {e}", rule.format, rule.key),
        }
    }
    env
}

fn write(rule: &OptimizeRule, game: &GameRef, exe_dir: &Path) -> Result<PathBuf, String> {
    let mut file = rule.file.clone().ok_or("fichier manquant")?;
    if file.contains("{titleid}") {
        file = file.replace("{titleid}", game.title.ok_or("ID de titre du jeu inconnu")?);
    }
    // Chemin relatif : dans le dossier de l'émulateur.
    let file = if file.starts_with('{') { file } else { format!("{{exedir}}/{file}") };
    let path = crate::language::expand(&file, exe_dir).ok_or("dossier introuvable")?;
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let new = match rule.format.as_str() {
        "ini" => crate::language::set_ini(&old, &rule.key, &rule.value),
        "kv" => crate::language::set_kv(&old, &rule.key, &rule.value),
        "json" => crate::language::set_json(&old, &rule.key, &rule.value)?,
        "ligne" => set_line(&old, &rule.key, &rule.value),
        "eden-jeu" => set_eden_game(&old, &rule.key, &rule.value),
        other => return Err(format!("format inconnu : {other}")),
    };
    if new == old {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, new).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Fichier de lignes `CLÉ=VALEUR` (`nx2_switches.txt`) : remplacée ou ajoutée.
fn set_line(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let entry = format!("{key}={value}");
    match lines.iter_mut().find(|l| l.split('=').next().map(str::trim) == Some(key)) {
        Some(line) => *line = entry,
        None => lines.push(entry),
    }
    crate::language::join_lines(lines, text)
}

/// Réglage par jeu de la famille Eden (`section.clé`) : la valeur, marquée comme propre au
/// jeu (`clé\use_global=false`) et non par défaut (`clé\default=false`).
fn set_eden_game(text: &str, key: &str, value: &str) -> String {
    let (section, name) = key.rsplit_once('.').unwrap_or(("Renderer", key));
    let mut out = crate::language::set_ini(text, &format!("{section}.{name}\\use_global"), "false");
    out = crate::language::set_ini(&out, &format!("{section}.{name}\\default"), "false");
    crate::language::set_ini(&out, &format!("{section}.{name}"), value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::Gpu;

    const GO: u64 = 1 << 30;

    fn pc(level: &'static str, vram_go: u64, os: &str) -> Hardware {
        Hardware {
            os: os.into(),
            os_version: String::new(),
            cpu: None,
            cpu_threads: 8,
            ram: 32 * GO - GO / 4,
            gpu: Some(Gpu { name: "RTX".into(), vendor: "nvidia".into(), vram: vram_go * GO - GO / 8, driver: None }),
            level,
        }
    }

    fn rule(conds: &[(&str, &str, &str)], game: &str, format: &str, file: Option<&str>, key: &str, value: &str) -> OptimizeRule {
        OptimizeRule {
            conditions: conds.iter().map(|(k, o, v)| Condition { key: k.to_string(), op: o.to_string(), value: v.to_string() }).collect(),
            game: game.into(),
            format: format.into(),
            file: file.map(str::to_string),
            key: key.into(),
            value: value.into(),
            label: None,
        }
    }

    #[test]
    fn catalog_numbers_are_read_as_text() {
        let rule: OptimizeRule = serde_json::from_str(
            r#"{"conditions":[{"key":"vram","op":"<","value":9},{"key":"niveau","op":"=","value":"moyen"}],"game":"tous","format":"ini","file":"{exedir}/mkw.ini","key":"graphics.TEX_QUALITY","value":"half","label":null}"#,
        )
        .unwrap();
        assert_eq!(rule.conditions[0].value, "9");
        assert!(rule.matches(&pc("moyen", 8, "windows"), &GameRef { rom_id: 1, title: None }));
        assert!(!rule.matches(&pc("moyen", 16, "windows"), &GameRef { rom_id: 1, title: None }));
    }

    #[test]
    fn conditions_and_games() {
        let hw = pc("moyen", 8, "windows");
        let game = GameRef { rom_id: 42, title: Some("040074A01BF12000") };
        assert!(rule(&[], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(rule(&[("niveau", "=", "moyen"), ("gpu", "=", "nvidia")], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(!rule(&[("niveau", "=", "faible")], "tous", "env", None, "A", "1").matches(&hw, &game));
        // 8 Go annoncés un peu en dessous : comptés 8.
        assert!(rule(&[("vram", ">=", "8")], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(!rule(&[("vram", "<", "8")], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(rule(&[("ram", ">=", "32"), ("fils", "<", "12")], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(!rule(&[("os", "=", "macos")], "tous", "env", None, "A", "1").matches(&hw, &game));
        assert!(rule(&[], "040074a01bf12000", "env", None, "A", "1").matches(&hw, &game));
        assert!(rule(&[], "42", "env", None, "A", "1").matches(&hw, &game));
        assert!(!rule(&[], "0400C3F00006E000", "env", None, "A", "1").matches(&hw, &game));
        assert!(!rule(&[("inconnu", "=", "x")], "tous", "env", None, "A", "1").matches(&hw, &game));
    }

    #[test]
    fn last_rule_wins_and_files_are_written() {
        let dir = std::env::temp_dir().join(format!("farfa-optimize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("user/config")).unwrap();
        std::fs::write(dir.join("user/config/qt-config.ini"), "[Renderer]\nresolution_setup=2\nbackend=1\n").unwrap();
        let hw = pc("faible", 4, "windows");
        let game = GameRef { rom_id: 1, title: Some("040074A01BF12000") };
        let rules = vec![
            rule(&[], "tous", "ini", Some("user/config/qt-config.ini"), "Renderer.resolution_setup", "3"),
            rule(&[("niveau", "=", "faible")], "tous", "ini", Some("user/config/qt-config.ini"), "Renderer.resolution_setup", "2"),
            rule(&[], "040074A01BF12000", "eden-jeu", Some("user/config/custom/{titleid}.ini"), "Renderer.gpu_fence_behavior", "0"),
            rule(&[], "040074A01BF12000", "ligne", Some("{exedir}/nx2_switches.txt"), "EDEN_NX2_FRAMES_AHEAD", "1"),
            rule(&[], "040074A01BF12000", "env", None, "EDEN_NX2_X", "{exedir}/y"),
            rule(&[], "0400C3F00006E000", "env", None, "AUTRE_JEU", "1"),
        ];
        let env = apply(&rules, &hw, &game, &dir);
        assert_eq!(env, vec![("EDEN_NX2_X".to_string(), format!("{}/y", dir.to_string_lossy()))]);
        let qt = std::fs::read_to_string(dir.join("user/config/qt-config.ini")).unwrap();
        assert!(qt.contains("resolution_setup=2") && qt.contains("backend=1"), "{qt}");
        let custom = std::fs::read_to_string(dir.join("user/config/custom/040074A01BF12000.ini")).unwrap();
        assert!(custom.contains("[Renderer]"));
        assert!(custom.contains("gpu_fence_behavior\\use_global=false"));
        assert!(custom.contains("gpu_fence_behavior\\default=false"));
        assert!(custom.contains("gpu_fence_behavior=0"));
        assert_eq!(std::fs::read_to_string(dir.join("nx2_switches.txt")).unwrap().trim(), "EDEN_NX2_FRAMES_AHEAD=1");
        // Deuxième lancement : rien ne bouge, pas de doublon.
        apply(&rules, &hw, &game, &dir);
        assert_eq!(std::fs::read_to_string(dir.join("nx2_switches.txt")).unwrap().trim(), "EDEN_NX2_FRAMES_AHEAD=1");
        assert_eq!(std::fs::read_to_string(dir.join("user/config/custom/040074A01BF12000.ini")).unwrap(), custom);
        // Ce que voit le joueur : un réglage par clé, le dernier ; sans libellé, rien.
        assert!(summary(&rules, &hw, &game).is_empty());
        let rules: Vec<OptimizeRule> = rules.into_iter().map(|r| OptimizeRule { label: Some(format!("{} à {}", r.key, r.value)), ..r }).collect();
        let shown = summary(&rules, &hw, &game);
        assert_eq!(shown.iter().filter(|a| a.key == "Renderer.resolution_setup").count(), 1);
        assert_eq!(shown[0].value, "2");
        assert_eq!(shown.len(), 4);
        // Jeu sans ID connu : la règle par jeu ne s'écrit nulle part.
        let unknown = GameRef { rom_id: 1, title: None };
        let _ = apply(&[rule(&[], "tous", "eden-jeu", Some("user/config/custom/{titleid}.ini"), "Renderer.x", "1")], &hw, &unknown, &dir);
        assert!(!dir.join("user/config/custom/{titleid}.ini").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
