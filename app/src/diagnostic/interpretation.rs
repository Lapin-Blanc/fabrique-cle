//! Des informations lues et mesurées vers des « fiches » lisibles : une par facette technique,
//! dans l'ordre d'importance pour un système sur clé, chacune avec son niveau de certitude.
//! Les seuils sont indicatifs : à recalibrer sur des clés de référence.

use super::materiel::{Materiel, Protocole, Role, fabricant, nom_vitesse};
use super::mesures::{EN_PARALLELE, Mesures};
use crate::disques::Disque;
use crate::suivi::taille_lisible;

/// D'où vient l'information.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Certitude {
    Lu,
    Mesure,
    Deduit,
    NonMesure,
}

impl Certitude {
    pub fn nom(self) -> &'static str {
        match self {
            Certitude::Lu => "lu",
            Certitude::Mesure => "mesuré",
            Certitude::Deduit => "déduit",
            Certitude::NonMesure => "non mesuré",
        }
    }
}

/// Ce que vaut la facette pour un système sur clé.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Appreciation {
    Bon,
    Moyen,
    Faible,
    Neutre,
}

#[derive(Clone, Debug)]
pub struct Fiche {
    /// Clé des textes explicatifs (`textes.txt`).
    pub cle: &'static str,
    pub titre: &'static str,
    pub valeur: String,
    pub certitude: Certitude,
    pub appreciation: Appreciation,
    /// Précisions propres à cette clé (vide si rien à ajouter).
    pub commentaire: String,
}

/// Toutes les clés de textes que peuvent produire les fiches (vérifié par un test).
#[cfg(test)]
pub const CLES: [&str; 13] = [
    "controleur",
    "protocole",
    "parallelisme",
    "lecture_aleatoire",
    "latence",
    "trim",
    "liaison",
    "sequentielle",
    "cache_slc",
    "memoire",
    "capacite",
    "endurance",
    "zones_vides",
];

/// Durée lisible à partir de microsecondes.
pub fn duree_us(us: u32) -> String {
    if us < 1000 {
        format!("{us} µs")
    } else if us < 10_000 {
        format!("{:.1} ms", us as f64 / 1000.0).replace('.', ",")
    } else {
        format!("{} ms", us / 1000)
    }
}

/// Nombre avec une décimale, à la française.
fn dec(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}").replace('.', ",")
    }
}

/// Gain obtenu en lançant plusieurs lectures à la fois.
fn gain_parallele(m: &Mesures) -> f64 {
    if m.aleatoire_1.iops > 0.0 {
        m.aleatoire_n.iops / m.aleatoire_1.iops
    } else {
        0.0
    }
}

fn fiche(
    cle: &'static str,
    titre: &'static str,
    valeur: impl Into<String>,
    certitude: Certitude,
    appreciation: Appreciation,
    commentaire: impl Into<String>,
) -> Fiche {
    Fiche {
        cle,
        titre,
        valeur: valeur.into(),
        certitude,
        appreciation,
        commentaire: commentaire.into(),
    }
}

fn en_attente(cle: &'static str, titre: &'static str) -> Fiche {
    fiche(
        cle,
        titre,
        "pas encore mesuré",
        Certitude::Mesure,
        Appreciation::Neutre,
        "Lancez les mesures (bouton en bas de la fenêtre).",
    )
}

fn controleur(m: &Materiel, mes: Option<&Mesures>) -> Fiche {
    use Appreciation::*;
    let fab = m.vid.and_then(fabricant);
    let mut commentaire = match (m.vid, m.pid, fab) {
        (Some(v), Some(p), Some(f)) => format!(
            "Identifiant USB {v:04x}:{p:04x} : {} — {}.",
            f.nom,
            match f.role {
                Role::Marque => "une marque, qui ne dit rien de la puce utilisée",
                Role::Controleur => "le fabricant de la puce lui-même",
                Role::Pont => "un pont USB vers un SSD SATA ou NVMe",
                Role::Suspect =>
                    "identifiant fantaisiste, typique des clés anonymes ou contrefaites",
            }
        ),
        (Some(v), Some(p), None) => {
            format!(
                "Identifiant USB {v:04x}:{p:04x} : fabricant absent de la table de l'application."
            )
        }
        _ => "Identifiant USB non disponible.".into(),
    };
    let uas = m.protocole() == Some(Protocole::Uas);
    let bot = m.protocole() == Some(Protocole::Bot);
    let trim = m.trim == Some(true);
    let parallele = mes.map(gain_parallele).is_some_and(|g| g >= 1.8);
    let indices = [
        (uas, "protocole UAS"),
        (trim, "TRIM"),
        (parallele, "lectures en parallèle efficaces"),
    ];
    let pour: Vec<&str> = indices.iter().filter(|i| i.0).map(|i| i.1).collect();
    if !pour.is_empty() {
        commentaire += &format!(" Indices de contrôleur de SSD : {}.", pour.join(", "));
    }
    let (valeur, appr) = match fab.map(|f| f.role) {
        Some(Role::Pont) => ("probablement un SSD derrière un pont USB", Bon),
        Some(Role::Suspect) => ("clé générique, probablement de très bas de gamme", Faible),
        _ if uas && trim => ("probablement de type SSD", Bon),
        _ if uas => ("peut-être de type SSD (UAS, mais sans TRIM)", Moyen),
        _ if bot && parallele => ("contrôleur de clé, plutôt efficace", Moyen),
        _ if bot => ("probablement un contrôleur de clé ordinaire", Faible),
        _ if parallele => ("probablement de type SSD", Bon),
        _ => ("indéterminé", Neutre),
    };
    fiche(
        "controleur",
        "Contrôleur",
        valeur,
        Certitude::Deduit,
        appr,
        commentaire,
    )
}

fn protocole(m: &Materiel) -> Fiche {
    let file = match m.file_commandes {
        Some(n) => format!("File de commandes : {n}. "),
        None => String::new(),
    };
    let pilote = m
        .pilote
        .as_deref()
        .map(|p| format!("Pilote : {p}."))
        .unwrap_or_default();
    let (valeur, appr) = match m.protocole() {
        Some(Protocole::Uas) => ("UAS — plusieurs commandes à la fois", Appreciation::Bon),
        Some(Protocole::Bot) => ("BOT — une seule commande à la fois", Appreciation::Faible),
        None if m.pilote.is_some() => ("pilote inattendu", Appreciation::Neutre),
        None => ("inconnu", Appreciation::Neutre),
    };
    fiche(
        "protocole",
        "Protocole USB",
        valeur,
        Certitude::Lu,
        appr,
        format!("{file}{pilote}"),
    )
}

fn parallelisme(mes: &Mesures) -> Fiche {
    let g = gain_parallele(mes);
    let valeur = format!("× {} avec {EN_PARALLELE} lectures simultanées", dec(g));
    let (appr, commentaire) = if g >= 1.8 {
        (
            Appreciation::Bon,
            "La clé traite réellement plusieurs demandes à la fois.",
        )
    } else if g >= 1.2 {
        (
            Appreciation::Moyen,
            "Gain modeste : la clé ne traite que partiellement les demandes en parallèle.",
        )
    } else {
        (
            Appreciation::Faible,
            "Aucun gain : les demandes font la queue, une à une.",
        )
    };
    fiche(
        "parallelisme",
        "Parallélisme",
        valeur,
        Certitude::Mesure,
        appr,
        format!(
            "{commentaire} ({} lectures/s une à une, {} à {EN_PARALLELE} en même temps.)",
            dec(mes.aleatoire_1.iops),
            dec(mes.aleatoire_n.iops)
        ),
    )
}

fn lecture_aleatoire(mes: &Mesures) -> Fiche {
    let a = &mes.aleatoire_1;
    let appr = if a.iops >= 3000.0 {
        Appreciation::Bon
    } else if a.iops >= 1000.0 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    };
    fiche(
        "lecture_aleatoire",
        "Lecture aléatoire 4 Ko",
        format!("{} lectures/s ({} Mo/s)", dec(a.iops), dec(a.mo_s)),
        Certitude::Mesure,
        appr,
        format!(
            "{} lectures en {} s, une à la fois.",
            a.operations,
            dec(a.duree_s)
        ),
    )
}

fn latence(mes: &Mesures) -> Fiche {
    let l = &mes.aleatoire_1.latences;
    let appr = if l.p99_us <= 2_000 {
        Appreciation::Bon
    } else if l.p99_us <= 20_000 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    };
    fiche(
        "latence",
        "Latence des lectures",
        format!(
            "médiane {}, 99 % sous {}",
            duree_us(l.mediane_us),
            duree_us(l.p99_us)
        ),
        Certitude::Mesure,
        appr,
        format!("Lecture la plus lente : {}.", duree_us(l.max_us)),
    )
}

fn trim(m: &Materiel) -> Fiche {
    let (valeur, appr) = match m.trim {
        Some(true) => ("pris en charge", Appreciation::Bon),
        Some(false) => ("non pris en charge", Appreciation::Faible),
        None => ("inconnu", Appreciation::Neutre),
    };
    fiche("trim", "TRIM", valeur, Certitude::Lu, appr, "")
}

fn liaison(m: &Materiel, mes: Option<&Mesures>) -> Fiche {
    let norme = m
        .version_usb
        .as_deref()
        .map(|v| format!("La clé se déclare conforme à la norme USB {v}."))
        .unwrap_or_default();
    match m.vitesse_mbps {
        Some(v) if m.bridee_en_usb2() => fiche(
            "liaison",
            "Liaison USB",
            nom_vitesse(v),
            Certitude::Lu,
            Appreciation::Faible,
            format!(
                "{norme} Elle est bridée en USB 2 : essayez un autre port (souvent bleu), sans rallonge ni concentrateur."
            ),
        ),
        Some(v) => fiche(
            "liaison",
            "Liaison USB",
            nom_vitesse(v),
            Certitude::Lu,
            if v >= 5000 {
                Appreciation::Bon
            } else {
                Appreciation::Faible
            },
            norme,
        ),
        None => match mes {
            Some(x) if x.sequentielle.mo_s > 0.0 && x.sequentielle.mo_s <= 42.0 => fiche(
                "liaison",
                "Liaison USB",
                "peut-être limitée à l'USB 2",
                Certitude::Deduit,
                Appreciation::Moyen,
                format!(
                    "{norme} Vitesse non lue ; le débit séquentiel ne dépasse pas le plafond de l'USB 2 (≈ 40 Mo/s)."
                ),
            ),
            _ => fiche(
                "liaison",
                "Liaison USB",
                "inconnue",
                Certitude::Lu,
                Appreciation::Neutre,
                norme,
            ),
        },
    }
}

fn sequentielle(mes: &Mesures) -> Fiche {
    let s = &mes.sequentielle;
    let appr = if s.mo_s >= 100.0 {
        Appreciation::Bon
    } else if s.mo_s >= 40.0 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    };
    let (min, max) = s
        .serie_mo_s
        .iter()
        .fold((f32::MAX, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
    let variation = if s.serie_mo_s.len() >= 2 {
        format!(
            " Débit par demi-seconde : de {} à {} Mo/s.",
            dec(min as f64),
            dec(max as f64)
        )
    } else {
        String::new()
    };
    fiche(
        "sequentielle",
        "Lecture séquentielle",
        format!("{} Mo/s", dec(s.mo_s)),
        Certitude::Mesure,
        appr,
        format!(
            "{} lus en {} s.{variation}",
            taille_lisible(s.octets),
            dec(s.duree_s)
        ),
    )
}

fn non_mesure(cle: &'static str, titre: &'static str) -> Fiche {
    fiche(
        cle,
        titre,
        "demande l'analyse complète (à venir)",
        Certitude::NonMesure,
        Appreciation::Neutre,
        "",
    )
}

/// Les fiches, dans l'ordre d'importance pour un système sur clé.
pub fn fiches(m: &Materiel, mes: Option<&Mesures>) -> Vec<Fiche> {
    let mut f = vec![controleur(m, mes), protocole(m)];
    match mes {
        Some(x) => {
            f.push(parallelisme(x));
            f.push(lecture_aleatoire(x));
            f.push(latence(x));
        }
        None => {
            f.push(en_attente("parallelisme", "Parallélisme"));
            f.push(en_attente("lecture_aleatoire", "Lecture aléatoire 4 Ko"));
            f.push(en_attente("latence", "Latence des lectures"));
        }
    }
    f.push(trim(m));
    f.push(liaison(m, mes));
    f.push(match mes {
        Some(x) => sequentielle(x),
        None => en_attente("sequentielle", "Lecture séquentielle"),
    });
    if let Some(x) = mes {
        let vide = x.sequentielle.part_vide.max(x.aleatoire_1.part_vide);
        if vide >= 0.3 {
            f.push(fiche(
                "zones_vides",
                "Zones vides",
                format!(
                    "{} % des blocs lus ne contenaient que des zéros",
                    (vide * 100.0).round()
                ),
                Certitude::Mesure,
                Appreciation::Neutre,
                "Les mesures en lecture sont probablement optimistes.",
            ));
        }
    }
    f.push(non_mesure("cache_slc", "Cache SLC et débit soutenu"));
    f.push(non_mesure("memoire", "Type de mémoire flash"));
    f.push(non_mesure("capacite", "Capacité réelle"));
    f.push(fiche(
        "endurance",
        "Endurance",
        "illisible par USB",
        Certitude::NonMesure,
        Appreciation::Neutre,
        "",
    ));
    f
}

/// Estimation de la catégorie (A à D, voir le texte « categories »), forcément prudente :
/// la frontière se joue sur l'écriture, que ce diagnostic ne mesure pas.
pub fn estimation(m: &Materiel, mes: Option<&Mesures>) -> (String, Appreciation) {
    let uas = m.protocole() == Some(Protocole::Uas);
    let bot = m.protocole() == Some(Protocole::Bot);
    let trim = m.trim == Some(true);
    let parallele = mes.map(gain_parallele);
    let pont = m
        .vid
        .and_then(fabricant)
        .is_some_and(|f| f.role == Role::Pont);
    let (texte, appr) = if pont || (uas && trim && parallele.is_none_or(|g| g >= 1.8)) {
        (
            "Plutôt catégorie C ou D : contrôleur de type SSD. Un bon choix pour un système.",
            Appreciation::Bon,
        )
    } else if uas {
        (
            "Entre B et C : la clé a l'UAS, mais pas tous les signes d'un contrôleur de SSD.",
            Appreciation::Moyen,
        )
    } else if bot || parallele.is_some_and(|g| g < 1.2) {
        (
            "Plutôt catégorie A ou B : clé ordinaire. Le système fonctionnera grâce aux réglages \
             de l'image, mais restera lent en écriture.",
            Appreciation::Faible,
        )
    } else {
        (
            "Indéterminée : pas assez d'informations.",
            Appreciation::Neutre,
        )
    };
    let mut texte = texte.to_string();
    texte += " Estimation à confirmer par l'analyse complète, qui mesure l'écriture.";
    if m.bridee_en_usb2() {
        texte += " Attention : la liaison actuelle est en USB 2.";
    }
    (texte, appr)
}

/// Résumé en texte brut, à copier (comparaison de modèles, support).
pub fn resume(d: &Disque, m: &Materiel, mes: Option<&Mesures>) -> String {
    let mut s = format!("Diagnostic Fabrique-Clé ({})\n", env!("VERSION_APPLI"));
    s += &format!("Clé : {}\n", d.libelle());
    if !m.fabricant_usb.is_empty() || !m.produit_usb.is_empty() {
        s += &format!("Descripteur USB : {} {}\n", m.fabricant_usb, m.produit_usb);
    }
    s += &format!("Estimation : {}\n\n", estimation(m, mes).0);
    for f in fiches(m, mes) {
        s += &format!("- {} : {} [{}]", f.titre, f.valeur, f.certitude.nom());
        if !f.commentaire.is_empty() {
            s += &format!(" — {}", f.commentaire);
        }
        s.push('\n');
    }
    if let Some(x) = mes
        && !x.directes
    {
        s += "\nAttention : lectures passées par le cache du système, mesures peu fiables.\n";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::mesures::{Aleatoire, Latences};

    fn mesures(iops_1: f64, iops_n: f64, p99: u32) -> Mesures {
        let a = |iops, en_parallele| Aleatoire {
            en_parallele,
            operations: 1000,
            duree_s: 8.0,
            iops,
            mo_s: iops * 4096.0 / 1e6,
            latences: Latences {
                mediane_us: 400,
                p90_us: 600,
                p99_us: p99,
                max_us: p99 * 2,
                histogramme: vec![0; 10],
            },
            part_vide: 0.0,
        };
        Mesures {
            directes: true,
            sequentielle: Default::default(),
            aleatoire_1: a(iops_1, 1),
            aleatoire_n: a(iops_n, EN_PARALLELE),
        }
    }

    #[test]
    fn cle_ordinaire_en_bot() {
        // Profil de la SanDisk 3.2 Gen 1 de l'essai du 2026-11-01 (usb-storage, sans TRIM).
        let m = Materiel {
            vid: Some(0x0781),
            pid: Some(0x5581),
            pilote: Some("usb-storage".into()),
            trim: Some(false),
            file_commandes: Some(1),
            vitesse_mbps: Some(5000),
            version_usb: Some("3.20".into()),
            ..Default::default()
        };
        let x = mesures(2000.0, 2050.0, 3000);
        let f = fiches(&m, Some(&x));
        let par = |c: &str| f.iter().find(|f| f.cle == c).unwrap();
        assert_eq!(par("controleur").appreciation, Appreciation::Faible);
        assert_eq!(par("protocole").appreciation, Appreciation::Faible);
        assert_eq!(par("parallelisme").appreciation, Appreciation::Faible);
        assert_eq!(par("liaison").appreciation, Appreciation::Bon);
        assert_eq!(estimation(&m, Some(&x)).1, Appreciation::Faible);
        // Chaque fiche renvoie à un texte connu.
        assert!(f.iter().all(|f| CLES.contains(&f.cle)));
    }

    #[test]
    fn ssd_en_uas() {
        let m = Materiel {
            vid: Some(0x152d),
            pilote: Some("uas".into()),
            trim: Some(true),
            vitesse_mbps: Some(10000),
            ..Default::default()
        };
        let x = mesures(4000.0, 16000.0, 900);
        assert_eq!(estimation(&m, Some(&x)).1, Appreciation::Bon);
        let f = fiches(&m, Some(&x));
        assert_eq!(f[0].appreciation, Appreciation::Bon);
        assert_eq!(f[2].appreciation, Appreciation::Bon);
    }

    #[test]
    fn sans_mesures_ni_informations() {
        let m = Materiel::default();
        let f = fiches(&m, None);
        assert!(f.iter().any(|f| f.valeur == "pas encore mesuré"));
        assert_eq!(estimation(&m, None).1, Appreciation::Neutre);
    }

    #[test]
    fn durees_lisibles() {
        assert_eq!(duree_us(250), "250 µs");
        assert_eq!(duree_us(2500), "2,5 ms");
        assert_eq!(duree_us(25_000), "25 ms");
    }
}
