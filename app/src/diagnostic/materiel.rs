//! Ce que la clé et le système disent d'elle, sans rien mesurer : protocole USB, liaison négociée,
//! TRIM, identifiant USB… Lu sous Linux dans sysfs, sous Windows par les IOCTL de stockage et le
//! gestionnaire de périphériques (voir `disques/linux.rs` et `disques/windows.rs`).

use serde::{Deserialize, Serialize};

/// Informations lues (toutes facultatives : chaque système n'expose pas tout).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Materiel {
    /// Identifiant USB du fabricant (VID) et du produit (PID).
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    /// Chaînes du descripteur USB (fabricant, produit), quand la clé les fournit.
    pub fabricant_usb: String,
    pub produit_usb: String,
    /// Pilote de stockage : `uas`/`usb-storage` (Linux), `UASPStor`/`USBSTOR` (Windows).
    pub pilote: Option<String>,
    /// Version de la norme USB annoncée par la clé (`bcdUSB`, ex. « 3.20 »).
    pub version_usb: Option<String>,
    /// Vitesse de la liaison réellement négociée, en Mbit/s (480 = USB 2, 5000 = USB 3 Gen 1…).
    pub vitesse_mbps: Option<u32>,
    /// La clé sait fonctionner en USB 3 (SuperSpeed), quelle que soit la liaison actuelle.
    pub superspeed_possible: Option<bool>,
    /// TRIM (« discard ») pris en charge à travers la liaison USB.
    pub trim: Option<bool>,
    /// Nombre de commandes que le système peut confier en même temps à la clé.
    pub file_commandes: Option<u32>,
    /// Taille maximale d'un transfert, en Kio.
    pub transfert_max_kio: Option<u32>,
    pub secteur_logique: Option<u32>,
    pub secteur_physique: Option<u32>,
    /// La clé se déclare « rotative » (comme un disque dur) : fréquent et sans conséquence.
    pub rotatif_declare: Option<bool>,
    /// Ordonnanceur d'E/S en service (Linux).
    pub ordonnanceur: Option<String>,
}

/// Protocole de stockage USB, déduit du pilote.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Protocole {
    /// USB Attached SCSI : plusieurs commandes en parallèle, TRIM possible.
    Uas,
    /// Bulk-Only Transport (« usb-storage ») : une commande à la fois.
    Bot,
}

impl Materiel {
    pub fn protocole(&self) -> Option<Protocole> {
        let p = self.pilote.as_deref()?.to_ascii_lowercase();
        if p == "uas" || p == "uaspstor" {
            Some(Protocole::Uas)
        } else if p == "usb-storage" || p == "usbstor" {
            Some(Protocole::Bot)
        } else {
            None
        }
    }

    /// La clé sait faire de l'USB 3, mais la liaison est en USB 2 : port, rallonge ou câble en cause.
    pub fn bridee_en_usb2(&self) -> bool {
        let usb3 = self.superspeed_possible.unwrap_or(false)
            || self
                .version_usb
                .as_deref()
                .and_then(|v| v.trim().split('.').next()?.parse::<u32>().ok())
                .is_some_and(|m| m >= 3);
        usb3 && self.vitesse_mbps.is_some_and(|v| v <= 480)
    }
}

/// Nom lisible d'une vitesse de liaison USB.
pub fn nom_vitesse(mbps: u32) -> String {
    match mbps {
        0..=2 => "USB 1 « Low Speed » (1,5 Mbit/s)".into(),
        3..=12 => "USB 1 « Full Speed » (12 Mbit/s)".into(),
        13..=480 => "USB 2 « High Speed » (480 Mbit/s)".into(),
        481..=5000 => "USB 3.2 Gen 1 (5 Gbit/s)".into(),
        5001..=10000 => "USB 3.2 Gen 2 (10 Gbit/s)".into(),
        10001..=20000 => "USB 3.2 Gen 2×2 (20 Gbit/s)".into(),
        _ => format!("USB4 ou plus ({} Gbit/s)", mbps / 1000),
    }
}

/// Rôle de l'entreprise derrière un identifiant de fabricant USB (VID).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    /// Marque de clés ou de SSD : le VID ne dit rien de la puce utilisée.
    Marque,
    /// Fabricant de contrôleurs de clés : la marque a laissé l'identifiant de la puce.
    Controleur,
    /// Pont USB vers SATA ou NVMe : un vrai SSD se cache presque toujours derrière.
    Pont,
    /// Identifiant fantaisiste, typique des clés anonymes ou contrefaites.
    Suspect,
}

pub struct Fabricant {
    pub vid: u16,
    pub role: Role,
    pub nom: &'static str,
}

/// Table `ressources/diagnostic/fabricants-usb.txt` (VID, rôle, nom), lue une fois.
pub fn fabricants() -> &'static [Fabricant] {
    static TABLE: std::sync::OnceLock<Vec<Fabricant>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        analyser_fabricants(include_str!(
            "../../ressources/diagnostic/fabricants-usb.txt"
        ))
    })
}

fn analyser_fabricants(texte: &'static str) -> Vec<Fabricant> {
    texte
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (vid, reste) = l.split_once(char::is_whitespace)?;
            let (role, nom) = reste.trim_start().split_once(char::is_whitespace)?;
            let vid = u16::from_str_radix(vid, 16).ok()?;
            let role = match role {
                "marque" => Role::Marque,
                "controleur" => Role::Controleur,
                "pont" => Role::Pont,
                "suspect" => Role::Suspect,
                _ => return None,
            };
            Some(Fabricant {
                vid,
                role,
                nom: nom.trim(),
            })
        })
        .collect()
}

pub fn fabricant(vid: u16) -> Option<&'static Fabricant> {
    fabricants().iter().find(|f| f.vid == vid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_des_fabricants_lisible() {
        let texte = include_str!("../../ressources/diagnostic/fabricants-usb.txt");
        let lignes = texte
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
            .count();
        assert_eq!(
            fabricants().len(),
            lignes,
            "ligne mal formée dans fabricants-usb.txt"
        );
        assert_eq!(fabricant(0x0781).unwrap().nom, "SanDisk");
        assert_eq!(fabricant(0x152d).unwrap().role, Role::Pont);
    }

    #[test]
    fn protocole_et_bridage() {
        let mut m = Materiel {
            pilote: Some("usb-storage".into()),
            version_usb: Some("3.20".into()),
            vitesse_mbps: Some(480),
            ..Default::default()
        };
        assert_eq!(m.protocole(), Some(Protocole::Bot));
        assert!(m.bridee_en_usb2());
        m.vitesse_mbps = Some(5000);
        assert!(!m.bridee_en_usb2());
        m.pilote = Some("UASPStor".into());
        assert_eq!(m.protocole(), Some(Protocole::Uas));
        let usb2 = Materiel {
            version_usb: Some("2.00".into()),
            vitesse_mbps: Some(480),
            ..Default::default()
        };
        assert!(!usb2.bridee_en_usb2());
    }
}
