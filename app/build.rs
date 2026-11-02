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
    // Hors release, l'identifiant du commit compilé (CI : GITHUB_SHA) : on sait ainsi de quelle
    // version provient un résumé de diagnostic ou une capture.
    let version = match std::env::var("GITHUB_SHA") {
        Ok(sha) if sha.len() >= 7 && !version.starts_with('v') => {
            format!("{version} ({})", &sha[..7])
        }
        _ => version,
    };
    println!("cargo:rustc-env=VERSION_APPLI={version}");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=FABRIQUE_CLE_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
}
