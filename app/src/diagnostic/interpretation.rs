//! Des informations lues et mesurées vers des « fiches » lisibles : une par facette technique,
//! dans l'ordre d'importance pour un système sur clé, chacune avec son niveau de certitude.
//! Les seuils sont indicatifs : à recalibrer sur des clés de référence.

use super::analyse::Analyse;
use super::materiel::{Materiel, Protocole, Role, identification, nom_vitesse};
use super::mesures::{Aleatoire, EN_PARALLELE, Mesures, Sequentielle};
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
pub const CLES: [&str; 16] = [
    "regularite",
    "controleur",
    "ecriture_aleatoire",
    "ecriture_synchro",
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
    } else if us < 1_000_000 {
        format!("{} ms", us / 1000)
    } else {
        format!("{:.1} s", us as f64 / 1e6).replace('.', ",")
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

fn controleur(m: &Materiel, mes: Option<&Mesures>, an: Option<&Analyse>) -> Fiche {
    use Appreciation::*;
    let fab = m.vid.and_then(|v| identification(v, m.pid));
    let mut commentaire = match (m.vid, m.pid, fab) {
        (Some(v), Some(p), Some(f)) => format!(
            "Identifiant USB {v:04x}:{p:04x} : {} — {}.",
            f.nom,
            match f.role {
                Role::Marque => "une marque, qui ne dit rien de la puce utilisée",
                Role::Controleur => "le fabricant de la puce lui-même",
                Role::Pont => "un pont USB vers un SSD SATA ou NVMe",
                Role::Ssd => "un contrôleur de SSD qui parle USB directement, sans pont",
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
    if let Some(v) = m.vid {
        let (f, p) = super::usbids::noms(v, m.pid);
        if let Some(f) = f {
            commentaire += &format!(
                " Base usb.ids : {f}{}.",
                p.map(|p| format!(" — {p}")).unwrap_or_default()
            );
        }
    }
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
    // Mesures d'écriture disponibles : elles priment sur les indices, car c'est sur l'écriture
    // aléatoire que les contrôleurs se distinguent vraiment.
    if let Some(a) = an {
        let w = ecriture_4k(a);
        let (valeur, appr) = if w >= 10.0 {
            ("se comporte comme un contrôleur de SSD", Bon)
        } else if w >= 2.0 {
            (
                "intermédiaire, meilleur qu'un contrôleur de clé ordinaire",
                Moyen,
            )
        } else {
            ("se comporte comme un contrôleur de clé ordinaire", Faible)
        };
        commentaire += &format!(" Écriture aléatoire 4 Ko mesurée : {} Mo/s.", dec(w));
        return fiche(
            "controleur",
            "Contrôleur",
            valeur,
            Certitude::Deduit,
            appr,
            commentaire,
        );
    }
    let (valeur, appr) = match fab.map(|f| f.role) {
        Some(Role::Pont) => ("probablement un SSD derrière un pont USB", Bon),
        Some(Role::Ssd) => ("contrôleur de SSD portable", Bon),
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

/// Temps de réponse très irréguliers : la moyenne dépasse de loin la médiane (quelques opérations
/// très lentes dominent le total) et les pires cas atteignent des dizaines de millisecondes.
pub fn irreguliere(a: &Aleatoire) -> bool {
    if a.operations == 0 {
        return false;
    }
    let moyenne_us = a.duree_s * 1e6 * a.en_parallele as f64 / a.operations as f64;
    moyenne_us >= 3.0 * a.latences.mediane_us.max(1) as f64 && a.latences.p99_us >= 20_000
}

/// Débit séquentiel qui varie de plus d'un facteur dix d'une demi-seconde à l'autre.
fn sequentielle_irreguliere(s: &Sequentielle) -> bool {
    let (min, max) = s
        .serie_mo_s
        .iter()
        .fold((f32::MAX, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
    s.serie_mo_s.len() >= 4 && min * 10.0 < max
}

fn lectures_irregulieres(mes: &Mesures) -> bool {
    irreguliere(&mes.aleatoire_1)
        || irreguliere(&mes.aleatoire_n)
        || sequentielle_irreguliere(&mes.sequentielle)
}

fn regularite(mes: &Mesures) -> Fiche {
    let a = &mes.aleatoire_1;
    let moyenne_us = (a.duree_s * 1e6 / a.operations.max(1) as f64) as u32;
    let mut commentaire = format!(
        "La moitié des lectures aboutit en moins de {}, mais la moyenne est de {} : quelques \
         lectures très lentes (jusqu'à {}) pèsent plus que toutes les autres.",
        duree_us(a.latences.mediane_us),
        duree_us(moyenne_us),
        duree_us(a.latences.max_us)
    );
    if sequentielle_irreguliere(&mes.sequentielle) {
        let (min, max) = mes
            .sequentielle
            .serie_mo_s
            .iter()
            .fold((f32::MAX, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
        commentaire += &format!(
            " En lecture séquentielle, le débit varie de {} à {} Mo/s.",
            dec(min as f64),
            dec(max as f64)
        );
    }
    commentaire += " Débits et parallélisme en sont faussés : refaites la mesure après avoir \
                    fermé les fenêtres et programmes qui utilisent la clé, une ou deux minutes \
                    après l'avoir branchée.";
    fiche(
        "regularite",
        "Régularité",
        "temps de réponse très irréguliers",
        Certitude::Mesure,
        Appreciation::Faible,
        commentaire,
    )
}

fn parallelisme(mes: &Mesures) -> Fiche {
    let g = gain_parallele(mes);
    let valeur = format!("× {} avec {EN_PARALLELE} lectures simultanées", dec(g));
    let (appr, commentaire) = if irreguliere(&mes.aleatoire_1) || irreguliere(&mes.aleatoire_n) {
        (
            Appreciation::Neutre,
            "Non significatif : les interruptions (voir « Régularité ») limitent davantage que \
             le protocole ou le contrôleur.",
        )
    } else if g < 0.8 {
        (
            Appreciation::Faible,
            "Perte : avec plusieurs demandes à la fois, la clé ralentit. Pont USB dont la file \
             de commandes fonctionne mal, ou interruption pendant ce test : refaites la mesure \
             pour trancher.",
        )
    } else if g >= 1.8 {
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
            "{commentaire} ({} lectures/s une à une, {} à {EN_PARALLELE} en même temps ; à \
             {EN_PARALLELE} : médiane {}, 99 % sous {}, la plus lente {}.)",
            dec(mes.aleatoire_1.iops),
            dec(mes.aleatoire_n.iops),
            duree_us(mes.aleatoire_n.latences.mediane_us),
            duree_us(mes.aleatoire_n.latences.p99_us),
            duree_us(mes.aleatoire_n.latences.max_us)
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
    match m.trim {
        Some(true) => fiche(
            "trim",
            "TRIM",
            "pris en charge",
            Certitude::Lu,
            Appreciation::Bon,
            "",
        ),
        Some(false) => fiche(
            "trim",
            "TRIM",
            "non pris en charge",
            Certitude::Lu,
            Appreciation::Faible,
            "",
        ),
        // Windows ne répond pas toujours (clés USBSTOR) ; en BOT, TRIM est de toute façon
        // impossible : le protocole ne transmet pas cette commande.
        None if m.protocole() == Some(Protocole::Bot) => fiche(
            "trim",
            "TRIM",
            "non pris en charge",
            Certitude::Deduit,
            Appreciation::Faible,
            "Le protocole BOT ne transmet pas la commande TRIM.",
        ),
        None => fiche(
            "trim",
            "TRIM",
            "inconnu",
            Certitude::Lu,
            Appreciation::Neutre,
            "",
        ),
    }
}

/// Débit utile maximal d'une liaison USB (Mo/s), une fois retirés les surcoûts du protocole.
fn plafond_liaison(mbps: u32) -> f64 {
    match mbps {
        0..=480 => 42.0,
        481..=5000 => 450.0,
        5001..=10000 => 1000.0,
        _ => 2000.0,
    }
}

/// Plus haut débit mesuré (lecture séquentielle, écriture longue), en Mo/s.
fn debit_max(mes: Option<&Mesures>, an: Option<&Analyse>) -> f64 {
    let lu = mes.map_or(0.0, |x| x.sequentielle.mo_s);
    let ecrit = an.map_or(0.0, |a| a.longue.debit_debut.max(a.longue.mo_s));
    lu.max(ecrit)
}

/// Le débit `v` (Mo/s) atteint presque le plafond de la liaison : c'est elle qui limite.
fn plafonne(m: &Materiel, v: f64) -> bool {
    m.vitesse_mbps
        .is_some_and(|l| l > 480 && v >= 0.75 * plafond_liaison(l))
}

/// La puce identifiée (table de l'application) sait faire l'USB 3.2 Gen 2 (10 Gbit/s).
fn puce_gen2(m: &Materiel) -> bool {
    m.vid
        .and_then(|v| identification(v, m.pid))
        .is_some_and(|f| f.nom.contains("Gen 2"))
}

fn liaison(m: &Materiel, mes: Option<&Mesures>, an: Option<&Analyse>) -> Fiche {
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
        Some(v) => {
            let mut appr = if v >= 5000 {
                Appreciation::Bon
            } else {
                Appreciation::Faible
            };
            let mut commentaire = norme;
            let max = debit_max(mes, an);
            if plafonne(m, max) {
                commentaire += &format!(
                    " Les débits mesurés (jusqu'à {} Mo/s) atteignent presque le plafond de cette \
                     liaison (≈ {} Mo/s) : c'est elle qui les limite, pas la mémoire.",
                    dec(max),
                    dec(plafond_liaison(v))
                );
                if v <= 5000 && puce_gen2(m) {
                    appr = Appreciation::Moyen;
                    commentaire += " La puce sait faire l'USB 3.2 Gen 2 (10 Gbit/s) : un port \
                                    10 Gbit/s (marqué « SS10 », souvent rouge) et le câble \
                                    d'origine iraient plus vite.";
                }
            }
            fiche(
                "liaison",
                "Liaison USB",
                nom_vitesse(v),
                Certitude::Lu,
                appr,
                commentaire,
            )
        }
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
        "demande l'analyse complète (efface la clé)",
        Certitude::NonMesure,
        Appreciation::Neutre,
        "",
    )
}

/// Meilleur débit d'écriture aléatoire 4 Ko (une à la fois ou plusieurs à la fois), en Mo/s.
fn ecriture_4k(a: &Analyse) -> f64 {
    a.ecriture_1.mo_s.max(a.ecriture_n.mo_s)
}

fn ecriture_aleatoire(a: &Analyse) -> Fiche {
    let w = ecriture_4k(a);
    let pauses = if irreguliere(&a.ecriture_1) || irreguliere(&a.ecriture_n) {
        " Écritures très irrégulières : le contrôleur s'interrompt pour réorganiser sa mémoire, \
         ce que l'on ressent comme des gels du bureau."
    } else {
        ""
    };
    let appr = if w >= 10.0 {
        Appreciation::Bon
    } else if w >= 0.5 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    };
    fiche(
        "ecriture_aleatoire",
        "Écriture aléatoire 4 Ko",
        format!(
            "{} écritures/s ({} Mo/s)",
            dec(a.ecriture_1.iops.max(a.ecriture_n.iops)),
            dec(w)
        ),
        Certitude::Mesure,
        appr,
        format!(
            "Une à la fois : {} écritures/s ; {EN_PARALLELE} à la fois : {} écritures/s. \
             Écriture la plus lente : {}.{pauses}",
            dec(a.ecriture_1.iops),
            dec(a.ecriture_n.iops),
            duree_us(
                a.ecriture_1
                    .latences
                    .max_us
                    .max(a.ecriture_n.latences.max_us)
            )
        ),
    )
}

fn ecriture_synchro(a: &Analyse) -> Fiche {
    let l = &a.synchro.latences;
    let appr = if l.mediane_us <= 1_000 {
        Appreciation::Bon
    } else if l.mediane_us <= 5_000 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    };
    fiche(
        "ecriture_synchro",
        "Écritures synchronisées",
        format!(
            "médiane {}, 99 % sous {}",
            duree_us(l.mediane_us),
            duree_us(l.p99_us)
        ),
        Certitude::Mesure,
        appr,
        format!(
            "{} écritures synchronisées par seconde ; la plus lente : {}.",
            dec(a.synchro.iops),
            duree_us(l.max_us)
        ),
    )
}

fn capacite(a: &Analyse, taille: u64) -> Fiche {
    let c = &a.capacite;
    if c.conforme() {
        fiche(
            "capacite",
            "Capacité réelle",
            format!("conforme : {} marqueurs relus intacts", c.marqueurs),
            Certitude::Mesure,
            Appreciation::Bon,
            "Contrôle par échantillon : il détecte les contrefaçons courantes, pas un défaut isolé.",
        )
    } else {
        let mut commentaire = format!(
            "{} marqueurs sur {} relus intacts.",
            c.corrects, c.marqueurs
        );
        if c.repliement {
            commentaire += " Des marqueurs en ont écrasé d'autres : la clé replie ses adresses, \
                             c'est une contrefaçon.";
        }
        if c.erreurs > 0 {
            commentaire += &format!(" {} erreurs de lecture ou d'écriture.", c.erreurs);
        }
        commentaire += " À ne pas utiliser : les données écrites au-delà seraient perdues.";
        fiche(
            "capacite",
            "Capacité réelle",
            format!(
                "NON CONFORME : seuls {} fiables sur {} annoncés",
                taille_lisible(c.fiable),
                taille_lisible(taille)
            ),
            Certitude::Mesure,
            Appreciation::Faible,
            commentaire,
        )
    }
}

/// Débit d'écriture soutenu : après la chute du cache, ou moyen faute de chute.
fn debit_soutenu(a: &Analyse) -> f64 {
    if a.longue.chute_apres.is_some() {
        a.longue.debit_fin
    } else {
        a.longue.mo_s
    }
}

fn appr_soutenu(v: f64) -> Appreciation {
    if v >= 60.0 {
        Appreciation::Bon
    } else if v >= 20.0 {
        Appreciation::Moyen
    } else {
        Appreciation::Faible
    }
}

fn cache_slc(a: &Analyse, m: &Materiel) -> Fiche {
    let l = &a.longue;
    match l.chute_apres {
        Some(b) => fiche(
            "cache_slc",
            "Cache SLC et débit soutenu",
            format!(
                "≈ {} à {} Mo/s, puis {} Mo/s",
                taille_lisible(b),
                dec(l.debit_debut),
                dec(l.debit_fin)
            ),
            Certitude::Mesure,
            appr_soutenu(l.debit_fin),
            format!(
                "{} écrits en {} s ({} Mo/s en moyenne).",
                taille_lisible(l.octets),
                dec(l.duree_s),
                dec(l.mo_s)
            ),
        ),
        None => fiche(
            "cache_slc",
            "Cache SLC et débit soutenu",
            format!("pas de chute sur {} écrits", taille_lisible(l.octets)),
            Certitude::Mesure,
            appr_soutenu(l.mo_s),
            format!(
                "{} Mo/s en moyenne (début {}, fin {}). Le cache est plus grand que ce qui a été \
                 écrit, ou la clé n'en a pas : une écriture plus longue le dirait.{}",
                dec(l.mo_s),
                dec(l.debit_debut),
                dec(l.debit_fin),
                if plafonne(m, l.mo_s) {
                    " Débit plafonné par la liaison USB (voir « Liaison USB »)."
                } else {
                    ""
                }
            ),
        ),
    }
}

fn memoire(a: &Analyse, m: &Materiel) -> Fiche {
    let v = debit_soutenu(a);
    // Pas de chute à un débit de SSD : seul le cache a été mesuré (et peut-être seulement la
    // liaison USB). Le débit après le cache, qui trahit le type de mémoire, reste inconnu.
    if a.longue.chute_apres.is_none() && v >= 200.0 {
        return fiche(
            "memoire",
            "Type de mémoire flash",
            "non déterminé : le cache SLC n'a pas été rempli",
            Certitude::Deduit,
            Appreciation::Neutre,
            format!(
                "{} écrits à {} Mo/s sans ralentir : le débit après le cache, qui trahit le type de \
                 mémoire, n'a pas été atteint. Une écriture plus longue (réglage du Diagnostic) le \
                 dirait.{}",
                taille_lisible(a.longue.octets),
                dec(v),
                if plafonne(m, v) {
                    " Ce débit est d'ailleurs celui de la liaison USB, pas de la mémoire."
                } else {
                    ""
                }
            ),
        );
    }
    // Cache SLC rapide (débit de SSD) qui retombe bas : c'est le profil d'un SSD QLC (ou TLC
    // sans DRAM), pas d'un SSD TLC ordinaire qui garde 150 Mo/s ou plus après le cache.
    let ssd_cache_lent = a.longue.chute_apres.is_some() && a.longue.debit_debut >= 200.0;
    let valeur = if v >= 100.0 {
        "écriture soutenue rapide : mémoire de SSD (TLC ou meilleure)"
    } else if ssd_cache_lent {
        "SSD à grand cache SLC mais lent ensuite : probablement QLC, ou TLC sans DRAM"
    } else if v >= 30.0 {
        "probablement TLC"
    } else {
        "lente une fois le cache plein : probablement QLC, ou TLC derrière un contrôleur simple"
    };
    fiche(
        "memoire",
        "Type de mémoire flash",
        valeur,
        Certitude::Deduit,
        appr_soutenu(v),
        if ssd_cache_lent {
            format!(
                "Déduit du débit soutenu : {} Mo/s après le cache SLC ({} Mo/s avant).",
                dec(v),
                dec(a.longue.debit_debut)
            )
        } else {
            format!("Déduit du débit soutenu : {} Mo/s.", dec(v))
        },
    )
}

/// Les fiches, dans l'ordre d'importance pour un système sur clé.
pub fn fiches(
    m: &Materiel,
    taille: u64,
    mes: Option<&Mesures>,
    an: Option<&Analyse>,
) -> Vec<Fiche> {
    let mut f = vec![controleur(m, mes, an)];
    match an {
        Some(a) => {
            f.push(ecriture_aleatoire(a));
            f.push(ecriture_synchro(a));
            f.push(if a.fichier.is_some() {
                non_mesure("capacite", "Capacité réelle")
            } else {
                capacite(a, taille)
            });
        }
        None => {
            f.push(non_mesure("ecriture_aleatoire", "Écriture aléatoire 4 Ko"));
            f.push(non_mesure("ecriture_synchro", "Écritures synchronisées"));
            f.push(non_mesure("capacite", "Capacité réelle"));
        }
    }
    f.push(protocole(m));
    match mes {
        Some(x) => {
            f.push(parallelisme(x));
            let mut l = lecture_aleatoire(x);
            if let Some(a) = an {
                l.commentaire += &format!(
                    " Sur des données réellement écrites (analyse complète) : {} lectures/s.",
                    dec(a.lecture_donnees.iops)
                );
            }
            f.push(l);
            f.push(latence(x));
            if lectures_irregulieres(x) {
                f.push(regularite(x));
            }
        }
        None => {
            f.push(en_attente("parallelisme", "Parallélisme"));
            f.push(en_attente("lecture_aleatoire", "Lecture aléatoire 4 Ko"));
            f.push(en_attente("latence", "Latence des lectures"));
        }
    }
    f.push(trim(m));
    f.push(liaison(m, mes, an));
    match an {
        Some(a) => {
            f.push(cache_slc(a, m));
            f.push(memoire(a, m));
        }
        None => {
            f.push(non_mesure("cache_slc", "Cache SLC et débit soutenu"));
            f.push(non_mesure("memoire", "Type de mémoire flash"));
        }
    }
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

/// Catégorie (A à D, voir le texte « categories »). Mesurée si l'analyse complète a eu lieu
/// (écriture aléatoire), sinon estimée prudemment à partir des indices en lecture.
pub fn estimation(
    m: &Materiel,
    mes: Option<&Mesures>,
    an: Option<&Analyse>,
) -> (String, Appreciation) {
    let mut texte = String::new();
    let appr;
    if let Some(a) = an {
        let w = ecriture_4k(a);
        let (t, ap) = if a.fichier.is_none() && !a.capacite.conforme() {
            (
                "Clé contrefaite ou défectueuse : sa capacité réelle ne correspond pas à \
                 l'annonce. À ne pas utiliser.",
                Appreciation::Faible,
            )
        } else if w < 0.1 {
            (
                "Catégorie A : clé basique. Inutilisable pour un système.",
                Appreciation::Faible,
            )
        } else if w < 2.0 {
            (
                "Catégorie B : clé « rapide » grand public. Le système fonctionnera grâce aux \
                 réglages de l'image, mais le bureau pourra se figer pendant les écritures.",
                Appreciation::Moyen,
            )
        } else if w < 10.0 {
            (
                "Entre les catégories B et C : nettement meilleure qu'une clé ordinaire.",
                Appreciation::Moyen,
            )
        } else if w < 30.0 {
            (
                "Catégorie C : contrôleur de type SSD. Un bon choix pour un système.",
                Appreciation::Bon,
            )
        } else {
            (
                "Catégorie D : performances de SSD. Excellent pour un système.",
                Appreciation::Bon,
            )
        };
        texte += t;
        texte += &format!(" (Écriture aléatoire 4 Ko mesurée : {} Mo/s", dec(w));
        texte += match &a.fichier {
            Some(d) => format!(", dans un fichier d'essai sur {d}.)"),
            None => ".)".into(),
        }
        .as_str();
        appr = ap;
    } else {
        let uas = m.protocole() == Some(Protocole::Uas);
        let bot = m.protocole() == Some(Protocole::Bot);
        let trim = m.trim == Some(true);
        let parallele = mes.map(gain_parallele);
        // Pont USB vers SSD, ou contrôleur de SSD USB natif (modèle ou fabricant connu).
        let pont = m
            .vid
            .and_then(|v| identification(v, m.pid))
            .is_some_and(|f| matches!(f.role, Role::Pont | Role::Ssd));
        let (t, ap) = if pont || (uas && trim && parallele.is_none_or(|g| g >= 1.8)) {
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
                "Plutôt catégorie A ou B : clé ordinaire. Le système fonctionnera grâce aux \
                 réglages de l'image, mais restera lent en écriture.",
                Appreciation::Faible,
            )
        } else {
            (
                "Indéterminée : pas assez d'informations.",
                Appreciation::Neutre,
            )
        };
        texte += t;
        texte += " Estimation à confirmer par l'analyse complète, qui mesure l'écriture.";
        appr = ap;
    }
    if m.bridee_en_usb2() {
        texte += " Attention : la liaison actuelle est en USB 2.";
    }
    (texte, appr)
}

/// Recherche sur le web à partir de l'identifiant le plus précis : puce connue de notre table,
/// sinon produit de la base usb.ids, sinon identifiant USB, sinon modèle déclaré.
pub fn requete_web(d: &Disque, m: &Materiel) -> Option<String> {
    if let Some(v) = m.vid {
        if let Some(f) = identification(v, m.pid).filter(|f| f.pid.is_some()) {
            let puce = f.nom.split(',').next().unwrap_or(f.nom).trim();
            let puce = puce.replace(" (probablement)", "");
            return Some(format!("{puce} specifications"));
        }
        let (f, p) = super::usbids::noms(v, m.pid);
        if let (Some(f), Some(p)) = (f, p) {
            return Some(format!("{f} {p} USB controller"));
        }
        if let Some(p) = m.pid {
            return Some(format!("USB {v:04x}:{p:04x} controller"));
        }
    }
    (!d.nom.is_empty()).then(|| format!("{} specifications", d.nom))
}

/// Adresse de la recherche (DuckDuckGo : sans compte ni pistage publicitaire).
pub fn adresse_recherche(requete: &str) -> String {
    let mut q = String::new();
    for o in requete.bytes() {
        match o {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                q.push(o as char)
            }
            b' ' => q.push('+'),
            _ => q += &format!("%{o:02X}"),
        }
    }
    format!("https://duckduckgo.com/?q={q}")
}

/// Résumé en texte brut, à copier (comparaison de modèles, support).
pub fn resume(d: &Disque, m: &Materiel, mes: Option<&Mesures>, an: Option<&Analyse>) -> String {
    let mut s = format!("Diagnostic Fabrique-Clé ({})\n", env!("VERSION_APPLI"));
    s += &format!("Clé : {}\n", d.libelle());
    if !m.fabricant_usb.is_empty() || !m.produit_usb.is_empty() {
        s += &format!("Descripteur USB : {} {}\n", m.fabricant_usb, m.produit_usb);
    }
    s += &format!("Catégorie : {}\n\n", estimation(m, mes, an).0);
    for f in fiches(m, d.taille, mes, an) {
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
    if let Some(a) = an {
        if let Some(d) = &a.fichier {
            s += &format!("\nMesures d'écriture : fichier d'essai sur {d} (sans rien effacer).\n");
        }
        if !a.directes {
            s += "\nAttention : écritures passées par le cache du système, mesures peu fiables.\n";
        }
        if !a.longue.points.is_empty() {
            s += "\nÉcriture longue (Go écrits ; Mo/s ; un point sur 4) :\n";
            for p in a.longue.points.iter().step_by(4) {
                s += &format!("{:.2} ; {:.0}\n", p[0], p[1]);
            }
        }
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
        let f = fiches(&m, 32_000_000_000, Some(&x), None);
        let par = |c: &str| f.iter().find(|f| f.cle == c).unwrap();
        assert_eq!(par("controleur").appreciation, Appreciation::Faible);
        assert_eq!(par("protocole").appreciation, Appreciation::Faible);
        assert_eq!(par("parallelisme").appreciation, Appreciation::Faible);
        assert_eq!(par("liaison").appreciation, Appreciation::Bon);
        assert_eq!(estimation(&m, Some(&x), None).1, Appreciation::Faible);
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
        assert_eq!(estimation(&m, Some(&x), None).1, Appreciation::Bon);
        let f = fiches(&m, 32_000_000_000, Some(&x), None);
        let par = |c: &str| f.iter().find(|f| f.cle == c).unwrap();
        assert_eq!(par("controleur").appreciation, Appreciation::Bon);
        assert_eq!(par("parallelisme").appreciation, Appreciation::Bon);
    }

    /// Cas réel : SSD NVMe WD SN350 en boîtier RTL9210, sous Windows (2026-11-02).
    #[test]
    fn lectures_irregulieres_signalees() {
        let m = Materiel {
            vid: Some(0x0bda),
            pid: Some(0x9210),
            pilote: Some("UASPStor".into()),
            trim: Some(true),
            vitesse_mbps: Some(10000),
            ..Default::default()
        };
        let mut x = mesures(364.0, 446.0, 52_000);
        x.aleatoire_1.operations = 2925;
        x.aleatoire_1.duree_s = 8.0;
        x.aleatoire_1.latences.mediane_us = 237;
        x.sequentielle.serie_mo_s = vec![478.0, 8.3, 120.0, 30.0];
        let f = fiches(&m, 1_000_000_000_000, Some(&x), None);
        let par = |c: &str| f.iter().find(|f| f.cle == c).cloned();
        assert_eq!(
            par("regularite").unwrap().appreciation,
            Appreciation::Faible
        );
        assert_eq!(
            par("parallelisme").unwrap().appreciation,
            Appreciation::Neutre
        );
        // Profil régulier (clé ordinaire) : pas de fiche « Régularité ».
        let mut y = mesures(2000.0, 2050.0, 3000);
        y.aleatoire_1.operations = 16_000;
        let f = fiches(&m, 32_000_000_000, Some(&y), None);
        assert!(f.iter().all(|f| f.cle != "regularite"));
    }

    fn analyse(ecriture_iops: f64, synchro_us: u32, conforme: bool) -> Analyse {
        let x = mesures(ecriture_iops, ecriture_iops, synchro_us);
        let mut a = Analyse {
            directes: true,
            ecriture_1: x.aleatoire_1.clone(),
            ecriture_n: x.aleatoire_n.clone(),
            synchro: x.aleatoire_1.clone(),
            lecture_donnees: x.aleatoire_1,
            ..Default::default()
        };
        a.synchro.latences.mediane_us = synchro_us;
        a.capacite.marqueurs = 300;
        a.capacite.corrects = if conforme { 300 } else { 40 };
        a.capacite.repliement = !conforme;
        a.longue.octets = 3_000_000_000;
        a.longue.mo_s = 25.0;
        a.longue.chute_apres = Some(1_000_000_000);
        a.longue.debit_debut = 80.0;
        a.longue.debit_fin = 12.0;
        a
    }

    #[test]
    fn classement_par_l_analyse_complete() {
        let m = Materiel::default();
        // Clé ordinaire : ~100 écritures 4 Ko par seconde (0,4 Mo/s), 10 ms par écriture sûre.
        let a = analyse(100.0, 10_000, true);
        let (t, appr) = estimation(&m, None, Some(&a));
        assert!(t.starts_with("Catégorie B"), "{t}");
        assert_eq!(appr, Appreciation::Moyen);
        let f = fiches(&m, 32_000_000_000, None, Some(&a));
        let par = |c: &str| f.iter().find(|f| f.cle == c).unwrap().clone();
        assert_eq!(par("ecriture_synchro").appreciation, Appreciation::Faible);
        assert_eq!(par("capacite").appreciation, Appreciation::Bon);
        assert_eq!(par("memoire").appreciation, Appreciation::Faible);
        assert!(par("cache_slc").valeur.contains("puis 12"));
        assert!(par("memoire").valeur.contains("QLC"));
        // SSD USB à grand cache SLC (360 Mo/s) puis 40 Mo/s : QLC plutôt que TLC.
        let mut b = a.clone();
        b.longue.debit_debut = 360.0;
        b.longue.debit_fin = 40.0;
        let f = fiches(&m, 32_000_000_000, None, Some(&b));
        let v = f.iter().find(|f| f.cle == "memoire").unwrap();
        assert!(v.valeur.starts_with("SSD à grand cache"), "{}", v.valeur);
        assert_eq!(v.appreciation, Appreciation::Moyen);
        // Sans chute à 354 Mo/s (SSK SM2320 sur 63,8 Go) : seul le cache a été mesuré.
        b.longue.chute_apres = None;
        b.longue.mo_s = 354.0;
        b.longue.debit_debut = 355.0;
        b.longue.debit_fin = 355.0;
        let f = fiches(&m, 32_000_000_000, None, Some(&b));
        let v = f.iter().find(|f| f.cle == "memoire").unwrap();
        assert!(v.valeur.starts_with("non déterminé"), "{}", v.valeur);
        assert_eq!(v.appreciation, Appreciation::Neutre);
        // Même mesure, liaison 5 Gbit/s et puce Gen 2 : débits plafonnés par la liaison.
        let ssk = Materiel {
            vid: Some(0x090c),
            pid: Some(0x2320),
            vitesse_mbps: Some(5000),
            ..Default::default()
        };
        let f = fiches(&ssk, 1_000_000_000_000, None, Some(&b));
        let par = |c: &str| f.iter().find(|f| f.cle == c).unwrap().clone();
        let l = par("liaison");
        assert!(l.commentaire.contains("plafond"), "{}", l.commentaire);
        assert!(l.commentaire.contains("Gen 2"), "{}", l.commentaire);
        assert_eq!(l.appreciation, Appreciation::Moyen);
        assert!(par("cache_slc").commentaire.contains("plafonné"));
        assert!(par("memoire").commentaire.contains("liaison USB"));
        // En 10 Gbit/s, 354 Mo/s ne touchent pas le plafond.
        let ssk10 = Materiel {
            vitesse_mbps: Some(10000),
            ..ssk
        };
        let f = fiches(&ssk10, 1_000_000_000_000, None, Some(&b));
        let l = f.iter().find(|f| f.cle == "liaison").unwrap();
        assert!(!l.commentaire.contains("plafond"), "{}", l.commentaire);
        assert_eq!(l.appreciation, Appreciation::Bon);
        // Clé lente sans cache (25 Mo/s constants) : le débit moyen reste le débit soutenu.
        b.longue.mo_s = 25.0;
        b.longue.debit_debut = 25.0;
        let f = fiches(&m, 32_000_000_000, None, Some(&b));
        let v = f.iter().find(|f| f.cle == "memoire").unwrap();
        assert!(v.valeur.contains("QLC"), "{}", v.valeur);
        // SSD : 10 000 écritures par seconde (41 Mo/s).
        let a = analyse(10_000.0, 300, true);
        assert!(estimation(&m, None, Some(&a)).0.starts_with("Catégorie D"));
        // Contrefaçon : quoi qu'il en soit des débits.
        let a = analyse(10_000.0, 300, false);
        let (t, appr) = estimation(&m, None, Some(&a));
        assert!(t.starts_with("Clé contrefaite"), "{t}");
        assert_eq!(appr, Appreciation::Faible);
        let f = fiches(&m, 64_000_000_000, None, Some(&a));
        assert!(f.iter().all(|f| CLES.contains(&f.cle)));
    }

    #[test]
    fn sans_mesures_ni_informations() {
        let m = Materiel::default();
        let f = fiches(&m, 32_000_000_000, None, None);
        assert!(f.iter().any(|f| f.valeur == "pas encore mesuré"));
        assert_eq!(estimation(&m, None, None).1, Appreciation::Neutre);
    }

    #[test]
    fn recherche_web() {
        let d = Disque {
            chemin: "/dev/sdb".into(),
            nom: "SSK Portable SSD".into(),
            serie: String::new(),
            taille: 1,
        };
        let m = Materiel {
            vid: Some(0x090c),
            pid: Some(0x2320),
            ..Default::default()
        };
        assert_eq!(
            requete_web(&d, &m).unwrap(),
            "Silicon Motion SM2320 specifications"
        );
        let m = Materiel {
            vid: Some(0x2109),
            pid: Some(0x0715),
            ..Default::default()
        };
        assert_eq!(
            requete_web(&d, &m).unwrap(),
            "VIA Labs VL715 specifications"
        );
        let m = Materiel {
            vid: Some(0x0781),
            pid: Some(0x5581),
            ..Default::default()
        };
        assert_eq!(
            requete_web(&d, &m).unwrap(),
            "SanDisk Corp. Ultra USB controller"
        );
        assert_eq!(
            adresse_recherche("SM2320 & co"),
            "https://duckduckgo.com/?q=SM2320+%26+co"
        );
    }

    #[test]
    fn parallelisme_en_perte() {
        let x = mesures(5693.0, 1609.0, 531);
        let f = parallelisme(&x);
        assert_eq!(f.appreciation, Appreciation::Faible);
        assert!(f.commentaire.starts_with("Perte"));
    }

    #[test]
    fn durees_lisibles() {
        assert_eq!(duree_us(250), "250 µs");
        assert_eq!(duree_us(2500), "2,5 ms");
        assert_eq!(duree_us(25_000), "25 ms");
        assert_eq!(duree_us(2_598_000), "2,6 s");
    }
}
