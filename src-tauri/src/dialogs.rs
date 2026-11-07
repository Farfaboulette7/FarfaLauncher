//! Réponses automatiques aux questions qu'un émulateur pose au lancement
//! (ligne `reponse:` du gestionnaire), par exemple « Recommended Ocarina of
//! Time settings » d'Eglantine : la touche du bouton (`Y` pour « &Yes ») est
//! envoyée à la fenêtre dont le titre est exactement celui donné.
//!
//! Windows seulement : sous Linux, aucun moyen commun à X11 et Wayland.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialogAnswer {
    /// Titre exact de la fenêtre.
    pub title: String,
    /// Touche à envoyer : une lettre ou un chiffre.
    pub key: String,
}

/// Surveille les fenêtres jusqu'à `stop` (fin du jeu) ou dix minutes.
pub fn watch(answers: Vec<DialogAnswer>, stop: Arc<AtomicBool>) {
    if answers.is_empty() {
        return;
    }
    #[cfg(windows)]
    std::thread::spawn(move || windows::run(&answers, &stop));
    #[cfg(not(windows))]
    {
        let _ = stop;
        tracing::info!("réponses automatiques ignorées hors Windows : {answers:?}");
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CHAR, WM_KEYDOWN, WM_KEYUP};

    pub fn run(answers: &[DialogAnswer], stop: &AtomicBool) {
        let titles: Vec<(Vec<u16>, char)> = answers
            .iter()
            .filter_map(|a| {
                let key = a.key.chars().next().filter(char::is_ascii_alphanumeric)?;
                Some((a.title.encode_utf16().chain([0]).collect(), key))
            })
            .collect();
        let until = Instant::now() + Duration::from_secs(600);
        while !stop.load(Ordering::Relaxed) && Instant::now() < until {
            for ((title, key), answer) in titles.iter().zip(answers) {
                // SAFETY : titre terminé par un zéro ; la fenêtre peut disparaître, PostMessage échoue alors sans effet.
                let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
                if hwnd.is_null() {
                    continue;
                }
                // Laisse la fenêtre finir de s'afficher avant de répondre.
                std::thread::sleep(Duration::from_millis(300));
                let vk = key.to_ascii_uppercase() as usize;
                unsafe {
                    PostMessageW(hwnd, WM_KEYDOWN, vk, 1);
                    PostMessageW(hwnd, WM_CHAR, key.to_ascii_lowercase() as usize, 1);
                    PostMessageW(hwnd, WM_KEYUP, vk, 0xC000_0001u32 as isize);
                }
                tracing::info!("réponse « {} » envoyée à « {} »", answer.key, answer.title);
                std::thread::sleep(Duration::from_secs(2));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}
