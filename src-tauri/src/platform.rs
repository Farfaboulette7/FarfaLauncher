//! Détection du système et choix de l'archive RECOMP correspondante.
//!
//! Convention serveur : `RECOMP/<nom>_<Plateforme>.zip` avec
//! Plateforme ∈ {Windows, Linux, Linux_Arch, SteamOS, macOS_ARM, macOS_Intel}.
//! `Linux_Arch` : build compilée sur Arch Linux, qui demande une glibc et des
//! bibliothèques récentes : proposée seulement sur les systèmes de la famille Arch
//! (hors SteamOS, figé sur une base plus ancienne).
//! Les noms de releases d'origine sont aussi reconnus (« Jeu-win32-x86_64.zip »,
//! « Jeu-x86_64.AppImage », « Jeu-macos-arm64.zip », « Jeu-Mac.zip »…), la
//! convention restant prioritaire quand les deux existent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Windows,
    Linux,
    #[serde(rename = "Linux_Arch")]
    LinuxArch,
    SteamOS,
    #[serde(rename = "macOS_ARM")]
    MacArm,
    #[serde(rename = "macOS_Intel")]
    MacIntel,
}

pub const ALL_TARGETS: [Target; 6] = [Target::Windows, Target::Linux, Target::LinuxArch, Target::SteamOS, Target::MacArm, Target::MacIntel];

impl Target {
    /// Suffixe officiel dans le nom d'archive.
    pub fn suffix(self) -> &'static str {
        match self {
            Target::Windows => "Windows",
            Target::Linux => "Linux",
            Target::LinuxArch => "Linux_Arch",
            Target::SteamOS => "SteamOS",
            Target::MacArm => "macOS_ARM",
            Target::MacIntel => "macOS_Intel",
        }
    }

    /// Archives acceptables pour ce système, par ordre de préférence.
    pub fn candidates(self) -> &'static [Target] {
        match self {
            Target::SteamOS => &[Target::SteamOS, Target::Linux],
            Target::Windows => &[Target::Windows],
            Target::Linux => &[Target::Linux],
            Target::LinuxArch => &[Target::LinuxArch, Target::Linux],
            Target::MacArm => &[Target::MacArm],
            Target::MacIntel => &[Target::MacIntel],
        }
    }
}

/// Système sur lequel tourne le launcher.
pub fn detect() -> Target {
    if cfg!(target_os = "windows") {
        Target::Windows
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") || running_under_rosetta() {
            Target::MacArm
        } else {
            Target::MacIntel
        }
    } else {
        let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        if os_release_is_steamos(&os_release) {
            Target::SteamOS
        } else if os_release_is_arch(&os_release) {
            Target::LinuxArch
        } else {
            Target::Linux
        }
    }
}

fn os_release_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=').map(|v| v.trim().trim_matches('"')))
}

/// Arch Linux et ses dérivées à jour (CachyOS, EndeavourOS…) ; SteamOS est traité à part.
fn os_release_is_arch(content: &str) -> bool {
    let id = os_release_value(content, "ID").unwrap_or_default();
    let like = os_release_value(content, "ID_LIKE").unwrap_or_default();
    std::iter::once(id).chain(like.split_whitespace()).any(|v| v.eq_ignore_ascii_case("arch"))
}

fn os_release_is_steamos(content: &str) -> bool {
    content.lines().any(|l| {
        l.strip_prefix("ID=")
            .map(|v| v.trim().trim_matches('"').eq_ignore_ascii_case("steamos"))
            .unwrap_or(false)
    })
}

fn running_under_rosetta() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .args(["-n", "sysctl.proc_translated"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Format d'une archive RECOMP utilisable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    Zip,
    /// Exécutable autonome Linux : copié tel quel, pas d'extraction.
    AppImage,
    TarGz,
}

/// Une plateforme reconnue dans un nom d'archive, avec un score de préférence
/// (plus haut = meilleur) pour départager plusieurs archives du même système.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveMatch {
    pub target: Target,
    pub kind: ArchiveKind,
    pub score: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Linux,
    LinuxArch,
    SteamOS,
    Mac,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arch {
    X64,
    Arm64,
    /// Non précisé, ou « universal » pour macOS.
    Any,
}

/// Minuscules, sans espaces, `_`, `-` ni `.` : « macOS ARM » == « macos_arm ».
fn squash(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, ' ' | '_' | '-' | '.')).flat_map(char::to_lowercase).collect()
}

fn split_kind(file_name: &str) -> Option<(&str, ArchiveKind)> {
    let lower = file_name.to_ascii_lowercase();
    for (ext, kind) in [
        (".tar.gz", ArchiveKind::TarGz),
        (".tgz", ArchiveKind::TarGz),
        (".appimage", ArchiveKind::AppImage),
        (".zip", ArchiveKind::Zip),
    ] {
        if lower.ends_with(ext) {
            return Some((&file_name[..file_name.len() - ext.len()], kind));
        }
    }
    None
}

/// Plateformes couvertes par une archive d'après son nom. Une archive macOS
/// « universal » (ou sans architecture) couvre les deux Mac.
pub fn archive_matches(file_name: &str) -> Vec<ArchiveMatch> {
    let Some((stem, kind)) = split_kind(file_name) else {
        return Vec::new();
    };

    // 1. Convention officielle `<nom>_<Plateforme>.zip` : prioritaire.
    if kind == ArchiveKind::Zip {
        let s = squash(stem);
        let mut targets = ALL_TARGETS;
        targets.sort_by_key(|t| std::cmp::Reverse(t.suffix().len()));
        if let Some(t) = targets.into_iter().find(|t| s.ends_with(&squash(t.suffix()))) {
            // « macOS » seul ne doit pas être pris pour macOS_ARM/Intel ici :
            // les suffixes sont complets, donc pas d'ambiguïté.
            return vec![ArchiveMatch { target: t, kind, score: 100 }];
        }
    }

    // 2. Noms de releases d'origine : système + architecture.
    let lower = stem.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));

    // Plateformes non PC : ignorées.
    if has(&["android", "ios", "ps4", "ps5", "switch", "xbox", "uwp", "portmaster", "sbc", "web", "wasm"]) {
        return Vec::new();
    }

    let os = if has(&["steamos", "steamdeck", "deck"]) {
        Os::SteamOS
    } else if has(&["arch", "archlinux"]) {
        Os::LinuxArch
    } else if has(&["windows", "win", "win32", "win64", "w64", "mingw"]) {
        Os::Windows
    } else if has(&["macos", "mac", "osx", "darwin", "apple"]) {
        Os::Mac
    } else if has(&["linux"]) || kind == ArchiveKind::AppImage {
        Os::Linux
    } else {
        return Vec::new();
    };

    let squashed = squash(&lower);
    let arch = if has(&["arm64", "aarch64", "arm"]) || squashed.contains("arm64") || squashed.contains("aarch64") {
        Arch::Arm64
    } else if has(&["x64", "amd64", "win64", "x86"]) || squashed.contains("x8664") {
        // `x86` seul (32 bits) reste accepté : les PC x64 l'exécutent.
        Arch::X64
    } else {
        Arch::Any
    };
    let explicit = if arch == Arch::Any { 0 } else { 10 };

    match (os, arch) {
        (Os::Windows | Os::Linux | Os::LinuxArch | Os::SteamOS, Arch::Arm64) => Vec::new(),
        (Os::Windows, _) if kind == ArchiveKind::Zip => vec![ArchiveMatch { target: Target::Windows, kind, score: 50 + explicit }],
        (Os::Windows, _) => Vec::new(),
        (Os::Linux | Os::LinuxArch | Os::SteamOS, _) => {
            // AppImage > tar.gz > zip (un AppImage est directement lançable).
            let pref = match kind {
                ArchiveKind::AppImage => 20,
                ArchiveKind::TarGz => 10,
                ArchiveKind::Zip => 0,
            };
            let target = match os {
                Os::SteamOS => Target::SteamOS,
                Os::LinuxArch => Target::LinuxArch,
                _ => Target::Linux,
            };
            vec![ArchiveMatch { target, kind, score: 40 + explicit + pref }]
        }
        (Os::Mac, _) if kind != ArchiveKind::Zip => Vec::new(),
        (Os::Mac, Arch::Arm64) => vec![ArchiveMatch { target: Target::MacArm, kind, score: 60 }],
        (Os::Mac, Arch::X64) => vec![ArchiveMatch { target: Target::MacIntel, kind, score: 60 }],
        (Os::Mac, Arch::Any) => vec![
            ArchiveMatch { target: Target::MacArm, kind, score: 50 },
            ArchiveMatch { target: Target::MacIntel, kind, score: 50 },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(name: &str) -> Vec<Target> {
        archive_matches(name).into_iter().map(|m| m.target).collect()
    }

    #[test]
    fn matches_official_names() {
        assert_eq!(targets("sm64coopdx_Windows.zip"), [Target::Windows]);
        assert_eq!(targets("sm64coopdx_Linux.zip"), [Target::Linux]);
        assert_eq!(targets("sm64coopdx_SteamOS.zip"), [Target::SteamOS]);
        assert_eq!(targets("MHGURecomp_Linux_Arch.zip"), [Target::LinuxArch]);
        assert_eq!(targets("sm64coopdx_macOS_ARM.zip"), [Target::MacArm]);
        assert_eq!(targets("sm64coopdx_macOS_Intel.zip"), [Target::MacIntel]);
        assert_eq!(archive_matches("sm64coopdx_Windows.zip")[0].score, 100);
    }

    #[test]
    fn official_tolerant_to_case_spaces_underscores() {
        assert_eq!(targets("Game - windows.ZIP"), [Target::Windows]);
        assert_eq!(targets("game_MacOS ARM.zip"), [Target::MacArm]);
        assert_eq!(targets("game_macos-intel.zip"), [Target::MacIntel]);
        assert_eq!(targets("game_steam_os.zip"), [Target::SteamOS]);
        assert_eq!(targets("Windows.zip"), [Target::Windows]);
        assert_eq!(targets("Linux.zip"), [Target::Linux]);
    }

    #[test]
    fn release_names_seen_on_server() {
        assert_eq!(targets("Dusklight-v2.0.3-win32-x86_64.zip"), [Target::Windows]);
        assert!(targets("Dusklight-v2.0.3-win32-arm64.zip").is_empty());
        assert_eq!(targets("Dusklight-v2.0.3-macos-arm64.zip"), [Target::MacArm]);
        assert_eq!(targets("Dusklight-v2.0.3-macos-x86_64.zip"), [Target::MacIntel]);
        assert_eq!(targets("Dusklight-v2.0.3-linux-x86_64.AppImage"), [Target::Linux]);
        assert!(targets("Dusklight-v2.0.3-linux-arm64.AppImage").is_empty());
        assert_eq!(targets("Melee-Windows-x86_64.zip"), [Target::Windows]);
        assert_eq!(targets("Melee-x86_64.AppImage"), [Target::Linux]);
        assert!(targets("Melee-aarch64.AppImage").is_empty());
        assert_eq!(targets("melee-linux-x86_64.tar.gz"), [Target::Linux]);
        assert_eq!(targets("Paperboat-Mulberry-Delta-Win64.zip"), [Target::Windows]);
        assert_eq!(targets("Paperboat-Mulberry-Delta-Mac.zip"), [Target::MacArm, Target::MacIntel]);
        assert_eq!(targets("Paperboat-Mulberry-Delta-Linux.zip"), [Target::Linux]);
        assert_eq!(targets("gen1recomp-0.3.95-windows.zip"), [Target::Windows]);
        assert_eq!(targets("gen1recomp-0.3.95-macos.zip"), [Target::MacArm, Target::MacIntel]);
        assert_eq!(targets("ArchLinux-MHGURecomp.0.10.0.zip"), [Target::LinuxArch]);
    }

    #[test]
    fn rejects_non_pc_and_unknown() {
        for name in [
            "Melee-Android-arm64.apk",
            "Melee-iOS-arm64.ipa",
            "gen1recomp-0.3.95-linux.flatpak",
            "gen1recomp-0.3.95-ps4.pkg",
            "gen1recomp-0.3.95-switch.zip",
            "gen1recomp-0.3.95-xbox-uwp.zip",
            "gen1recomp-0.3.95-sbc-portmaster.zip",
            "gen1recomp-0.3.95-rg34xxsp-stockos64-mod.zip",
            "gen1recomp-0.3.95.love",
            "sha256sums.txt",
            "game_Windows.7z",
            "latest.yml",
        ] {
            assert!(targets(name).is_empty(), "{name} aurait dû être ignoré");
        }
    }

    #[test]
    fn preferences() {
        let appimage = archive_matches("Melee-x86_64.AppImage")[0].score;
        let targz = archive_matches("melee-linux-x86_64.tar.gz")[0].score;
        assert!(appimage > targz);
        let official = archive_matches("x_Linux.zip")[0].score;
        assert!(official > appimage);
    }

    #[test]
    fn steamos_falls_back_to_linux() {
        assert_eq!(Target::SteamOS.candidates(), &[Target::SteamOS, Target::Linux]);
    }

    #[test]
    fn detects_steamos_os_release() {
        assert!(os_release_is_steamos("NAME=\"SteamOS\"\nID=steamos\nID_LIKE=arch\n"));
        assert!(os_release_is_steamos("ID=\"steamos\""));
        assert!(!os_release_is_steamos("ID=arch\nID_LIKE=steamos"));
    }

    #[test]
    fn arch_builds_only_on_arch() {
        assert!(os_release_is_arch("ID=arch\n"));
        assert!(os_release_is_arch("ID=cachyos\nID_LIKE=\"arch\""));
        assert!(!os_release_is_arch("ID=bazzite\nID_LIKE=\"fedora\""));
        assert!(!os_release_is_arch("ID=archlike\n"));
        assert_eq!(Target::Linux.candidates(), &[Target::Linux]);
        assert!(!Target::SteamOS.candidates().contains(&Target::LinuxArch));
    }
}
