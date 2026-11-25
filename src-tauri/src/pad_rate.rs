//! Fréquence des manettes (page « Contrôles ») : combien de fois par seconde le PC reçoit
//! l'état de chaque manette. Une manette branchée en USB est souvent lue à 125 Hz (environ
//! 8 ms entre deux états) tant que le port n'est pas « débridé » à 1000 Hz.
//!
//! Windows : chaque rapport HID d'une manette arrive en `WM_INPUT` (Raw Input, comme les
//! outils de mesure de souris). Un fil de mesure tourne tant que la page le demande
//! ([`recent`], appelée toutes les 5 s) : il s'arrête seul 12 s après la dernière demande,
//! et aussitôt qu'une partie commence. Seuls les états des 5 dernières secondes comptent :
//! une manette qu'on ne bouge pas n'a pas de nouvelle mesure.
//! Ailleurs : pas de mesure (`None`).

use serde::Serialize;

/// Résultat pour une manette.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PadRate {
    pub vendor: u16,
    pub product: u16,
    /// États reçus par seconde pendant que la manette envoyait.
    pub hz: u32,
    /// Reliée en Bluetooth (sa fréquence ne dépend pas du port USB).
    pub bluetooth: bool,
}

/// Écart au-delà duquel la manette est considérée au repos (pas compté dans la fréquence).
const IDLE_GAP_S: f64 = 0.05;
/// En dessous, pas assez d'états pour une fréquence fiable.
const MIN_REPORTS: u32 = 20;
/// États pris en compte : ceux des dernières secondes.
pub const WINDOW_S: f64 = 5.0;

/// Fréquence d'après les instants d'arrivée (secondes) : états reçus divisés par le temps
/// passé à en recevoir, sans les pauses (stick lâché). Robuste aux paquets livrés groupés.
pub fn rate_of(times: &[f64]) -> Option<u32> {
    if times.len() < MIN_REPORTS as usize {
        return None;
    }
    let (mut n, mut busy) = (0u32, 0f64);
    for w in times.windows(2) {
        let gap = w[1] - w[0];
        if gap < IDLE_GAP_S {
            n += 1;
            busy += gap;
        }
    }
    (n >= MIN_REPORTS && busy > 0.0).then(|| (n as f64 / busy).round() as u32)
}

/// Chemin d'un périphérique relié en Bluetooth (classique ou basse consommation).
pub fn is_bluetooth_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("bthenum") || p.contains("bthledevice") || p.contains("00001124-0000-1000-8000-00805f9b34fb") || p.contains("00001812-0000-1000-8000-00805f9b34fb")
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::Input::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use crate::error::LockOrRecover;

    /// Le fil s'arrête quand la page ne demande plus rien depuis ce délai.
    const IDLE_STOP: Duration = Duration::from_secs(12);

    struct Shared {
        started: Instant,
        /// Instants d'arrivée des états, par périphérique (secondes depuis `started`).
        times: Mutex<HashMap<usize, VecDeque<f64>>>,
        last_ask: Mutex<Instant>,
        running: AtomicBool,
    }

    static SHARED: Mutex<Option<Arc<Shared>>> = Mutex::new(None);

    /// Fréquence des manettes bougées pendant les 5 dernières secondes ; démarre la mesure
    /// au besoin (la première réponse est donc vide).
    pub fn recent() -> Option<Vec<PadRate>> {
        if crate::gamepad::in_game() {
            return Some(Vec::new());
        }
        let shared = {
            let mut slot = SHARED.lock_or_recover();
            match slot.as_ref().filter(|s| s.running.load(Ordering::SeqCst)) {
                Some(s) => s.clone(),
                None => {
                    let s = Arc::new(Shared { started: Instant::now(), times: Mutex::default(), last_ask: Mutex::new(Instant::now()), running: AtomicBool::new(true) });
                    let thread = s.clone();
                    if std::thread::Builder::new().name("frequence-manettes".into()).spawn(move || run(thread)).is_err() {
                        return None;
                    }
                    *slot = Some(s.clone());
                    s
                }
            }
        };
        *shared.last_ask.lock_or_recover() = Instant::now();
        let now = shared.started.elapsed().as_secs_f64();
        let times = shared.times.lock_or_recover();
        let mut out: Vec<PadRate> = times
            .iter()
            .filter_map(|(&device, t)| {
                let recent: Vec<f64> = t.iter().copied().filter(|&x| x >= now - WINDOW_S).collect();
                let hz = rate_of(&recent)?;
                let (vendor, product, bluetooth) = describe(device);
                Some(PadRate { vendor, product, hz, bluetooth })
            })
            .collect();
        out.sort_by_key(|r| (r.vendor, r.product));
        Some(out)
    }

    /// Fabricant, produit et liaison Bluetooth d'un périphérique Raw Input.
    fn describe(device: usize) -> (u16, u16, bool) {
        let mut info: RID_DEVICE_INFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<RID_DEVICE_INFO>() as u32;
        let mut len = info.cbSize;
        // SAFETY : `info` est assez grand ; le système n'écrit que `len` octets.
        let ok = unsafe { GetRawInputDeviceInfoW(device as _, RIDI_DEVICEINFO, (&mut info as *mut RID_DEVICE_INFO).cast(), &mut len) };
        let (vendor, product) = if ok != u32::MAX && info.dwType == RIM_TYPEHID {
            let hid = unsafe { info.Anonymous.hid };
            (hid.dwVendorId as u16, hid.dwProductId as u16)
        } else {
            (0, 0)
        };
        let mut name = vec![0u16; 512];
        let mut chars = name.len() as u32;
        // SAFETY : `name` contient `chars` caractères.
        let got = unsafe { GetRawInputDeviceInfoW(device as _, RIDI_DEVICENAME, name.as_mut_ptr().cast(), &mut chars) };
        let path = if got != u32::MAX { String::from_utf16_lossy(&name[..(got as usize).min(name.len())]) } else { String::new() };
        (vendor, product, is_bluetooth_path(&path))
    }

    fn run(shared: Arc<Shared>) {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY : fenêtre « message seulement », créée, lue et détruite par ce seul fil.
        let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
        if hwnd.is_null() {
            tracing::warn!("fréquence des manettes : fenêtre non créée ({})", std::io::Error::last_os_error());
            shared.running.store(false, Ordering::SeqCst);
            return;
        }
        // Page « Generic Desktop » : joystick (4), manette (5), contrôleur multi-axes (8).
        let usages = [4u16, 5, 8];
        let size = std::mem::size_of::<RAWINPUTDEVICE>() as u32;
        let devices: Vec<RAWINPUTDEVICE> = usages.iter().map(|&u| RAWINPUTDEVICE { usUsagePage: 1, usUsage: u, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd }).collect();
        if unsafe { RegisterRawInputDevices(devices.as_ptr(), devices.len() as u32, size) } == 0 {
            tracing::warn!("fréquence des manettes : Raw Input refusé ({})", std::io::Error::last_os_error());
            unsafe { DestroyWindow(hwnd) };
            shared.running.store(false, Ordering::SeqCst);
            return;
        }
        tracing::info!("fréquence des manettes : mesure démarrée");
        let mut buf = vec![0u8; 4096];
        let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
        while shared.last_ask.lock_or_recover().elapsed() < IDLE_STOP && !crate::gamepad::in_game() {
            // Réveil dès qu'un état arrive : l'heure relevée est celle de l'arrivée.
            unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 200, QS_RAWINPUT) };
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            let mut times = shared.times.lock_or_recover();
            while unsafe { PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) } != 0 {
                if msg.message != WM_INPUT {
                    continue;
                }
                let now = shared.started.elapsed().as_secs_f64();
                let mut len = buf.len() as u32;
                let got = unsafe { GetRawInputData(msg.lParam as HRAWINPUT, RID_INPUT, buf.as_mut_ptr().cast(), &mut len, header) };
                if got == u32::MAX || (got as usize) < std::mem::size_of::<RAWINPUTHEADER>() {
                    continue;
                }
                // SAFETY : GetRawInputData vient d'écrire un RAWINPUT complet dans `buf`.
                let raw = unsafe { &*(buf.as_ptr() as *const RAWINPUT) };
                if raw.header.dwType != RIM_TYPEHID {
                    continue;
                }
                // Un message peut porter plusieurs états (dwCount) : chacun compte.
                let count = unsafe { raw.data.hid.dwCount }.max(1);
                let list = times.entry(raw.header.hDevice as usize).or_default();
                for _ in 0..count {
                    list.push_back(now);
                }
                while list.front().is_some_and(|&t| t < now - WINDOW_S * 2.0) {
                    list.pop_front();
                }
            }
        }
        let remove: Vec<RAWINPUTDEVICE> = usages.iter().map(|&u| RAWINPUTDEVICE { usUsagePage: 1, usUsage: u, dwFlags: RIDEV_REMOVE, hwndTarget: std::ptr::null_mut() }).collect();
        unsafe {
            RegisterRawInputDevices(remove.as_ptr(), remove.len() as u32, size);
            DestroyWindow(hwnd);
        }
        shared.running.store(false, Ordering::SeqCst);
        tracing::info!("fréquence des manettes : mesure arrêtée");
    }
}

#[cfg(windows)]
pub use imp::recent;

#[cfg(not(windows))]
pub fn recent() -> Option<Vec<PadRate>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_ignores_pauses_and_bursts() {
        // 1000 Hz pendant 0,5 s, une pause d'une seconde, puis encore 0,5 s.
        let mut t: Vec<f64> = (0..500).map(|i| i as f64 / 1000.0).collect();
        t.extend((0..500).map(|i| 1.5 + i as f64 / 1000.0));
        assert_eq!(rate_of(&t), Some(1000));
        // 125 Hz.
        let t: Vec<f64> = (0..200).map(|i| i as f64 * 0.008).collect();
        assert_eq!(rate_of(&t), Some(125));
        // Paquets livrés par deux : la moyenne reste juste.
        let t: Vec<f64> = (0..400).map(|i| (i / 2) as f64 * 0.002).collect();
        assert!((990..=1010).contains(&rate_of(&t).unwrap()));
        // Trop peu d'états.
        assert_eq!(rate_of(&[0.0, 0.01, 0.02]), None);
    }

    #[test]
    fn bluetooth_paths() {
        assert!(is_bluetooth_path(r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009#9&1a"));
        assert!(is_bluetooth_path(r"\\?\HID#BTHLEDevice#{00001812-0000-1000-8000-00805f9b34fb}"));
        assert!(!is_bluetooth_path(r"\\?\HID#VID_057E&PID_2009&MI_00#8&2b"));
    }
}
