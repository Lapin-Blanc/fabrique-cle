//! Windows : manifeste « requireAdministrator » (écrire sur un disque brut l'exige) et
//! contrôles communs modernes. Sans effet sur les autres systèmes.
fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        use embed_manifest::manifest::ExecutionLevel;
        embed_manifest::embed_manifest(
            embed_manifest::new_manifest("Fabrique-Cle")
                .requested_execution_level(ExecutionLevel::RequireAdministrator),
        )
        .expect("manifeste Windows");
    }
    // Version affichée : l'étiquette de la release (CI) ou celle de Cargo.toml.
    let version = std::env::var("FABRIQUE_CLE_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=VERSION_APPLI={version}");
    println!("cargo:rerun-if-env-changed=FABRIQUE_CLE_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
}
