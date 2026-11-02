//! Base communautaire `usb.ids` (linux-usb.org, celle de `lsusb`) : nom du fabricant et du produit
//! pour un identifiant USB, hors ligne. Fichier embarqué et compressé
//! (`ressources/diagnostic/usb.ids.zst`, licence GPL v2+ ou BSD 3 clauses, voir le LISEZMOI).

use std::collections::HashMap;
use std::sync::OnceLock;

const DONNEES: &[u8] = include_bytes!("../../ressources/diagnostic/usb.ids.zst");

struct Base {
    version: String,
    fabricants: HashMap<u16, (String, HashMap<u16, String>)>,
}

fn analyser(texte: &str) -> Base {
    let mut base = Base {
        version: String::new(),
        fabricants: HashMap::new(),
    };
    let mut courant: Option<u16> = None;
    for ligne in texte.lines() {
        if let Some(v) = ligne.strip_prefix("# Version:") {
            base.version = v.trim().to_string();
        }
        if ligne.starts_with('#') || ligne.trim().is_empty() {
            continue;
        }
        let (produit, l) = match ligne.strip_prefix('\t') {
            Some(l) => (true, l),
            None => (false, ligne),
        };
        if l.starts_with('\t') {
            continue; // interface (non gardée)
        }
        let Some((id, nom)) = l.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(id) = u16::from_str_radix(id, 16) else {
            courant = None; // autre section (classes…)
            continue;
        };
        let nom = nom.trim().to_string();
        if produit {
            if let Some(v) = courant
                && let Some(f) = base.fabricants.get_mut(&v)
            {
                f.1.insert(id, nom);
            }
        } else {
            courant = Some(id);
            base.fabricants.insert(id, (nom, HashMap::new()));
        }
    }
    base
}

fn base() -> &'static Base {
    static BASE: OnceLock<Base> = OnceLock::new();
    BASE.get_or_init(|| {
        let texte = zstd::decode_all(DONNEES).unwrap_or_default();
        analyser(&String::from_utf8_lossy(&texte))
    })
}

/// Noms du fabricant et du produit, s'ils figurent dans la base.
pub fn noms(vid: u16, pid: Option<u16>) -> (Option<&'static str>, Option<&'static str>) {
    match base().fabricants.get(&vid) {
        Some((f, produits)) => (
            Some(f.as_str()),
            pid.and_then(|p| produits.get(&p)).map(String::as_str),
        ),
        None => (None, None),
    }
}

/// Version de la base embarquée (ex. « 2026.06.26 »).
pub fn version() -> &'static str {
    &base().version
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_usb_ids() {
        let b = analyser(
            "# Version: 2026.01.01\n0781  SanDisk Corp.\n\t5581  Ultra\n\t\t00  interface\n\
             2109  VIA Labs, Inc.\n\t0715  VL817 SATA Adaptor\nC 00  classe\n\t01  sous-classe\n",
        );
        assert_eq!(b.version, "2026.01.01");
        assert_eq!(b.fabricants[&0x0781].1[&0x5581], "Ultra");
        assert_eq!(b.fabricants[&0x2109].1.len(), 1);
    }

    #[test]
    fn base_embarquee() {
        assert!(!version().is_empty());
        assert_eq!(
            noms(0x0781, Some(0x5581)),
            (Some("SanDisk Corp."), Some("Ultra"))
        );
        assert_eq!(noms(0x090c, Some(0xffff)).1, None);
        assert!(noms(0x090c, None).0.unwrap().starts_with("Silicon Motion"));
    }
}
