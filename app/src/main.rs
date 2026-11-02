//! Fabrique-Clé : fabrique une clé USB Debian 13 + KDE Plasma amorçable et personnalisée.
//! Voir `docs/architecture.md` (§ 4) pour la conception.

// Pas de console noire sous Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod assistant;
mod bmap;
mod config;
mod diagnostic;
mod disques;
mod ecriture;
mod esp;
mod interface;
mod releases;
mod suivi;
mod telechargement;
mod theme;
mod travail;

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    match std::env::args().nth(1).as_deref() {
        Some(assistant::OPTION) => std::process::exit(assistant::servir()),
        Some(assistant::OPTION_DIAGNOSTIC) => std::process::exit(assistant::servir_diagnostic()),
        _ => {}
    }

    let mut viewport = eframe::egui::ViewportBuilder::default();
    if let Some((rgba, width, height)) = theme::icone() {
        viewport = viewport.with_icon(eframe::egui::IconData {
            rgba,
            width,
            height,
        });
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title("Fabrique-Clé")
            .with_app_id("fabrique-cle")
            .with_inner_size([640.0, 820.0])
            .with_min_inner_size([540.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Fabrique-Clé",
        options,
        Box::new(|cc| Ok(Box::new(interface::Appli::new(cc)))),
    )
}
