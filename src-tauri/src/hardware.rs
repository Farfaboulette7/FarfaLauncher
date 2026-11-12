//! Matériel de l'ordinateur pour les réglages optimisés et les rapports de bug :
//! carte graphique (nom, marque, mémoire vidéo, pilote), processeur, mémoire,
//! système, et un niveau pour l'émulation (`faible`, `moyen`, `fort`).
//!
//! Lu une fois par session (`get`) : la carte graphique ne change pas en cours de route.

use std::sync::OnceLock;

use serde::Serialize;
use sysinfo::System;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    /// `nvidia`, `amd`, `intel`, `apple` ou `autre`.
    pub vendor: String,
    /// Mémoire vidéo dédiée en octets (mémoire unifiée sur les puces Apple), 0 si inconnue.
    pub vram: u64,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    /// `windows`, `linux` ou `macos`.
    pub os: String,
    pub os_version: String,
    pub cpu: Option<String>,
    pub cpu_threads: usize,
    pub ram: u64,
    pub gpu: Option<Gpu>,
    /// Niveau pour l'émulation : `faible`, `moyen` ou `fort`.
    pub level: &'static str,
}

const GO: u64 = 1 << 30;

static HARDWARE: OnceLock<Hardware> = OnceLock::new();

/// Matériel de cet ordinateur (lu au premier appel : peut prendre une fraction de seconde).
pub fn get() -> &'static Hardware {
    HARDWARE.get_or_init(read)
}

fn read() -> Hardware {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    let cpu = sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty());
    let os = crate::language::current_system().to_string();
    let ram = sys.total_memory();
    let gpu = read_gpu(ram);
    let cpu_threads = sys.cpus().len();
    let level = level(cpu_threads, ram, gpu.as_ref());
    let hw = Hardware {
        os,
        os_version: System::long_os_version().or_else(System::name).unwrap_or_default(),
        cpu,
        cpu_threads,
        ram,
        gpu,
        level,
    };
    tracing::info!(
        "matériel : {} ({} fils), {} Go, carte {:?}, niveau {}",
        hw.cpu.as_deref().unwrap_or("?"),
        hw.cpu_threads,
        hw.ram / GO,
        hw.gpu.as_ref().map(|g| format!("{} {} Mo", g.name, g.vram >> 20)),
        hw.level
    );
    hw
}

/// Niveau pour l'émulation, d'après le processeur, la mémoire et la carte graphique.
/// - fort : 12 fils ou plus, 16 Go ou plus, et une carte d'au moins 10 Go (ou une puce
///   Apple Pro/Max/Ultra) ;
/// - faible : moins de 8 fils, moins de 16 Go, une carte de moins de 6 Go, ou une
///   carte Intel (intégrée) ;
/// - moyen sinon.
pub fn level(threads: usize, ram: u64, gpu: Option<&Gpu>) -> &'static str {
    let apple_big = gpu.is_some_and(|g| g.vendor == "apple" && ["pro", "max", "ultra"].iter().any(|w| g.name.to_lowercase().contains(w)));
    let vram = gpu.map(|g| g.vram).unwrap_or(0);
    let weak_gpu = match gpu {
        None => true,
        Some(g) if g.vendor == "intel" => true,
        Some(g) if g.vendor == "apple" => false,
        Some(g) => g.vram > 0 && g.vram < 6 * GO,
    };
    if threads < 8 || ram < 15 * GO || weak_gpu {
        "faible"
    } else if threads >= 12 && ram >= 15 * GO && (vram >= 10 * GO || apple_big) {
        "fort"
    } else {
        "moyen"
    }
}

fn vendor_of(id: u32, name: &str) -> &'static str {
    match id {
        0x10DE => "nvidia",
        0x1002 | 0x1022 => "amd",
        0x8086 => "intel",
        0x106B => "apple",
        _ => {
            let n = name.to_lowercase();
            if n.contains("nvidia") || n.contains("geforce") {
                "nvidia"
            } else if n.contains("radeon") || n.contains("amd") {
                "amd"
            } else if n.contains("intel") {
                "intel"
            } else if n.contains("apple") {
                "apple"
            } else {
                "autre"
            }
        }
    }
}

/// Cartes virtuelles ou logicielles, jamais celle qui affiche le jeu.
fn is_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    ["microsoft basic", "parsec", "virtual", "remote", "llvmpipe", "swiftshader", "vmware", "virtualbox"].iter().any(|w| n.contains(w))
}

/// Parmi les cartes trouvées : la plus grosse mémoire dédiée, hors cartes virtuelles.
fn best(gpus: Vec<Gpu>) -> Option<Gpu> {
    gpus.into_iter().filter(|g| !is_virtual(&g.name)).max_by_key(|g| (g.vendor != "intel", g.vram))
}

#[cfg(windows)]
fn read_gpu(_ram: u64) -> Option<Gpu> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut gpus = Vec::new();
    for i in 0..16 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]).trim().to_string();
        // Version du pilote : a.b.c.d sur 4 × 16 bits.
        let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok().map(|v| {
            let v = v as u64;
            format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF)
        });
        gpus.push(Gpu { vendor: vendor_of(desc.VendorId, &name).into(), name, vram: desc.DedicatedVideoMemory as u64, driver });
    }
    best(gpus)
}

#[cfg(target_os = "linux")]
fn read_gpu(_ram: u64) -> Option<Gpu> {
    use std::fs;
    let mut gpus = Vec::new();
    let Ok(cards) = fs::read_dir("/sys/class/drm") else { return None };
    for card in cards.filter_map(Result::ok) {
        let name = card.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = card.path().join("device");
        let read = |f: &str| fs::read_to_string(dev.join(f)).ok().map(|s| s.trim().to_string());
        let vendor_id = read("vendor").and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
        let slot = read("uevent").and_then(|u| u.lines().find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(str::to_string)));
        let label = slot
            .as_deref()
            .and_then(|s| std::process::Command::new("lspci").args(["-s", s]).output().ok())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .and_then(|l| l.split_once(": ").map(|(_, n)| n.to_string()))
            .unwrap_or_else(|| format!("carte {vendor_id:04x}"));
        let mut vram = read("mem_info_vram_total").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if vram == 0 && vendor_id == 0x10DE {
            vram = std::process::Command::new("nvidia-smi")
                .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().and_then(|l| l.trim().parse::<u64>().ok()))
                .map(|mib| mib << 20)
                .unwrap_or(0);
        }
        let driver = fs::read_link(dev.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        gpus.push(Gpu { vendor: vendor_of(vendor_id, &label).into(), name: label, vram, driver });
    }
    best(gpus)
}

#[cfg(target_os = "macos")]
fn read_gpu(ram: u64) -> Option<Gpu> {
    let sysctl = |k: &str| {
        std::process::Command::new("sysctl").args(["-n", k]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
    };
    let chip = sysctl("machdep.cpu.brand_string").unwrap_or_default();
    if chip.to_lowercase().contains("apple") {
        // Mémoire unifiée : la carte graphique peut en utiliser environ les trois quarts.
        return Some(Gpu { name: chip, vendor: "apple".into(), vram: ram / 4 * 3, driver: None });
    }
    Some(Gpu { name: sysctl("hw.model").unwrap_or_else(|| "Mac Intel".into()), vendor: "autre".into(), vram: 0, driver: None })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn read_gpu(_ram: u64) -> Option<Gpu> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(vendor: &str, name: &str, vram_go: u64) -> Gpu {
        Gpu { name: name.into(), vendor: vendor.into(), vram: vram_go * GO, driver: None }
    }

    #[test]
    fn levels_of_typical_machines() {
        // i7-9700K, 32 Go, RTX 2080 8 Go.
        assert_eq!(level(8, 32 * GO, Some(&gpu("nvidia", "RTX 2080", 8))), "moyen");
        // Ryzen 9, 32 Go, RTX 4080 16 Go.
        assert_eq!(level(32, 32 * GO, Some(&gpu("nvidia", "RTX 4080", 16))), "fort");
        // Portable, iGPU Intel.
        assert_eq!(level(8, 16 * GO, Some(&gpu("intel", "Iris Xe", 0))), "faible");
        // Steam Deck : 8 fils, 16 Go (un peu moins vus par le système), APU AMD 1 Go dédié.
        assert_eq!(level(8, 15 * GO + GO / 2, Some(&gpu("amd", "Van Gogh", 1))), "faible");
        // 8 Go de RAM.
        assert_eq!(level(12, 8 * GO, Some(&gpu("nvidia", "RTX 3060", 12))), "faible");
        // Mac M2 Pro 32 Go, M1 16 Go.
        assert_eq!(level(12, 32 * GO, Some(&gpu("apple", "Apple M2 Pro", 24))), "fort");
        assert_eq!(level(8, 16 * GO, Some(&gpu("apple", "Apple M1", 12))), "moyen");
    }

    /// Matériel réel de ce PC : `cargo test hardware::tests::this_pc -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn this_pc() {
        println!("{:#?}", get());
    }

    #[test]
    fn best_card_skips_virtual_and_prefers_dedicated() {
        let picked = best(vec![gpu("autre", "Parsec Virtual Display Adapter", 0), gpu("intel", "UHD 630", 0), gpu("nvidia", "NVIDIA GeForce RTX 2080", 8)]);
        assert_eq!(picked.unwrap().name, "NVIDIA GeForce RTX 2080");
        assert_eq!(best(vec![gpu("intel", "UHD 630", 0)]).unwrap().vendor, "intel");
    }
}
