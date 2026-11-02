//! Disques proposés à l'utilisateur : uniquement des disques USB (ou amovibles), jamais un disque
//! qui porte le système en cours d'exécution.

use crate::suivi::taille_lisible;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{
    creer_fichier_essai, emplacements, identifier, lister, lister_systeme_usb, ouvrir,
    ouvrir_ecriture, ouvrir_lecture,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    creer_fichier_essai, emplacements, identifier, lister, ouvrir, ouvrir_ecriture, ouvrir_lecture,
};

/// Windows : le disque système n'est jamais une clé USB.
#[cfg(windows)]
pub fn lister_systeme_usb() -> Vec<Disque> {
    Vec::new()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn lister() -> Result<Vec<Disque>, String> {
    Err("système non pris en charge".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn ouvrir(_: &Disque) -> Result<Box<dyn crate::ecriture::Cible>, String> {
    Err("système non pris en charge".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn ouvrir_lecture(_: &Disque) -> Result<(std::fs::File, bool), String> {
    Err("système non pris en charge".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn ouvrir_ecriture(_: &Disque) -> Result<(std::fs::File, bool), String> {
    Err("système non pris en charge".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn lister_systeme_usb() -> Vec<Disque> {
    Vec::new()
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn emplacements(_: &Disque) -> Vec<Emplacement> {
    Vec::new()
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn creer_fichier_essai(
    _: &std::path::Path,
    _: u32,
) -> Result<(Vec<std::fs::File>, bool), String> {
    Err("système non pris en charge".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn identifier(_: &Disque) -> crate::diagnostic::materiel::Materiel {
    Default::default()
}

/// Dossier d'un volume monté de la clé, où l'on peut écrire un fichier d'essai.
#[derive(Clone, Debug, PartialEq)]
pub struct Emplacement {
    pub dossier: std::path::PathBuf,
    /// Espace libre disponible, en octets.
    pub libre: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Disque {
    /// `/dev/sdb` ou `\\.\PhysicalDrive2`.
    pub chemin: String,
    /// Fabricant et modèle.
    pub nom: String,
    /// Numéro de série (peut être vide sur certaines clés bon marché).
    pub serie: String,
    /// Taille en octets.
    pub taille: u64,
}

impl Disque {
    pub fn libelle(&self) -> String {
        let nom = if self.nom.is_empty() {
            "Disque USB"
        } else {
            &self.nom
        };
        let serie = if self.serie.is_empty() {
            String::new()
        } else {
            format!(" — n° {}", self.serie)
        };
        format!(
            "{nom} — {}{serie} ({})",
            taille_lisible(self.taille),
            self.chemin
        )
    }

    /// Comme [`Disque::retrouver`], en acceptant aussi la clé qui porte le système en cours :
    /// réservé aux mesures qui n'effacent rien (lecture, fichier d'essai).
    pub fn retrouver_diagnostic(&self) -> Result<(), String> {
        if lister_systeme_usb().contains(self) {
            Ok(())
        } else {
            self.retrouver()
        }
    }

    /// Le même disque est-il toujours branché, identique (chemin, série et taille) ?
    /// Contrôle refait juste avant d'écrire : la clé a pu être remplacée entre-temps.
    pub fn retrouver(&self) -> Result<(), String> {
        if lister()?.iter().any(|d| d == self) {
            Ok(())
        } else {
            Err(format!(
                "La clé « {} » n'est plus disponible (débranchée ou remplacée). \
                 Rafraîchissez la liste et choisissez-la de nouveau.",
                self.nom
            ))
        }
    }
}

/// Nettoie une chaîne lue du matériel (espaces, caractères nuls).
pub(crate) fn propre(s: &str) -> String {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\0')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
