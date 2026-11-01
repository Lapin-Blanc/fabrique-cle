//! Fabrique-Clé : fabrique une clé USB Debian 13 + KDE Plasma amorçable et personnalisée.
//! Voir `docs/architecture.md` (§ 4) pour la conception.

// Pas de console noire sous Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod assistant;
mod bmap;
mod config;
mod disques;
mod ecriture;
mod esp;
mod interface;
mod releases;
mod suivi;
mod travail;

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some(assistant::OPTION) {
        std::process::exit(assistant::servir());
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Fabrique-Clé")
            .with_app_id("fabrique-cle")
            .with_inner_size([620.0, 720.0])
            .with_min_inner_size([520.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Fabrique-Clé",
        options,
        Box::new(|cc| Ok(Box::new(interface::Appli::new(cc)))),
    )
}
