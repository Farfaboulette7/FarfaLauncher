fn main() {
    // macOS : SDL3, compilé en statique, teste la version du système avec `@available`,
    // qui appelle `__isPlatformVersionAtLeast` de la bibliothèque d'exécution de clang.
    // Rust ne la lie pas d'office (édition de liens sans bibliothèques par défaut).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        if let Ok(out) = std::process::Command::new("clang").arg("--print-resource-dir").output() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() {
                println!("cargo:rustc-link-search=native={dir}/lib/darwin");
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            }
        }
    }
    // Le programme sans interface (Steam Deck) n'a pas de fenêtre Tauri.
    #[cfg(feature = "gui")]
    tauri_build::build()
}
