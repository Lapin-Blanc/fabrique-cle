//! Analyse complète : **efface la clé**. Mêmes garde-fous que la fabrication (clé revérifiée,
//! volumes démontés ou verrouillés, refus si un volume chiffré est ouvert), confirmation par
//! modèle, taille et numéro de série dans l'interface.
//!
//! Dans l'ordre :
//! 1. capacité réelle : des marqueurs uniques écrits sur toute la surface, puis relus. Une clé
//!    contrefaite (128 Go annoncés, 16 Go réels) « replie » les adresses : les derniers marqueurs
//!    écrasent les premiers ;
//! 2. écritures aléatoires de 4 Ko, une à la fois puis plusieurs à la fois ;
//! 3. écritures synchronisées (chaque écriture attend d'être en sécurité, comme un fsync) ;
//! 4. longue écriture séquentielle : la courbe du débit montre le cache SLC se remplir ;
//! 5. lectures aléatoires sur les données écrites (mesure réaliste, sans zones vides).
//!
//! À la fin, le début et la fin de la clé sont remis à zéro : elle apparaît vide (sans table de
//! partitions) plutôt que remplie de données de test.

use super::mesures::{
    Aleatoire, BLOC_ALEATOIRE, BLOC_SEQUENTIEL, EN_PARALLELE, Hasard, Mode, Support, TamponAligne,
    aleatoire, graine,
};
use crate::suivi::{Suivi, controler};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Nombre minimal de marqueurs du contrôle de capacité (entre 256 et 513 selon la taille).
pub const MARQUEURS_MIN: u64 = 256;
const MAGIE: &[u8; 16] = b"FABCLE-MARQUEUR\0";

/// Réglages choisis par l'utilisateur.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Reglages {
    /// Durée maximale de la longue écriture séquentielle.
    pub duree_longue_s: u64,
    /// Durée de chaque test d'écriture aléatoire.
    pub duree_aleatoire_s: u64,
}

impl Default for Reglages {
    fn default() -> Self {
        Self {
            duree_longue_s: 180,
            duree_aleatoire_s: 10,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Capacite {
    pub marqueurs: u32,
    pub corrects: u32,
    /// Octets dont on est sûr : jusqu'au premier marqueur perdu (la capacité annoncée si aucun).
    pub fiable: u64,
    /// Un marqueur a été retrouvé à la place d'un autre : adresses « repliées » (contrefaçon).
    pub repliement: bool,
    /// Erreurs d'entrée-sortie pendant le contrôle.
    pub erreurs: u32,
}

impl Capacite {
    pub fn conforme(&self) -> bool {
        self.corrects == self.marqueurs && self.erreurs == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EcritureLongue {
    pub octets: u64,
    pub duree_s: f64,
    pub mo_s: f64,
    /// Points de la courbe : (Go écrits, Mo/s sur la dernière demi-seconde).
    pub points: Vec<[f32; 2]>,
    /// Débit des premières secondes et des dernières (Mo/s, médianes).
    pub debit_debut: f64,
    pub debit_fin: f64,
    /// Octets écrits avant que le débit ne chute durablement de moitié (fin du cache SLC).
    pub chute_apres: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analyse {
    /// E/S directes obtenues (sinon : chiffres faussés par le cache du système).
    pub directes: bool,
    pub capacite: Capacite,
    pub ecriture_1: Aleatoire,
    pub ecriture_n: Aleatoire,
    pub synchro: Aleatoire,
    pub longue: EcritureLongue,
    pub lecture_donnees: Aleatoire,
}

/// Contenu d'un marqueur : en-tête (magie, session, position, rang) puis octets pseudo-aléatoires
/// propres à cette position : un marqueur ne peut pas être confondu avec un autre.
fn marqueur(buf: &mut [u8], session: u64, pos: u64, rang: u32) {
    Hasard::new(session ^ pos.rotate_left(17)).remplir(buf);
    buf[..16].copy_from_slice(MAGIE);
    buf[16..24].copy_from_slice(&session.to_le_bytes());
    buf[24..32].copy_from_slice(&pos.to_le_bytes());
    buf[32..36].copy_from_slice(&rang.to_le_bytes());
}

/// Positions des marqueurs : une grille dont le pas est une puissance de deux, sur toute la
/// capacité annoncée, plus le tout dernier bloc.
///
/// Pourquoi une puissance de deux : une clé contrefaite replie les adresses sur sa capacité
/// réelle, presque toujours une puissance de deux (l'adresse est tronquée de ses bits de poids
/// fort). Avec ce pas, un marqueur écrit au-delà de la capacité réelle retombe exactement sur un
/// marqueur déjà écrit et l'écrase : la relecture le révèle. Avec un pas quelconque, il tomberait
/// entre deux marqueurs et passerait inaperçu. C'est un échantillonnage : une vérification
/// exhaustive (f3, H2testw) écrit toute la surface et prend des heures.
fn positions(taille: u64) -> Vec<u64> {
    let bloc = BLOC_ALEATOIRE as u64;
    let derniere = (taille / bloc).saturating_sub(1) * bloc;
    let pas = (taille / MARQUEURS_MIN).max(bloc);
    let pas = 1u64 << (63 - pas.leading_zeros()); // puissance de deux inférieure
    let mut p: Vec<u64> = (0..=derniere / pas).map(|k| k * pas).collect();
    if p.last() != Some(&derniere) {
        p.push(derniere);
    }
    p
}

/// Contrôle de capacité réelle.
pub fn capacite(s: &impl Support, taille: u64, suivi: &dyn Suivi) -> Result<Capacite, String> {
    let session = graine();
    let pos = positions(taille);
    let mut buf = TamponAligne::new(BLOC_ALEATOIRE);
    let mut erreurs = 0;
    let mut ecrits = vec![false; pos.len()];
    // Écriture dans l'ordre croissant : en cas de repliement, les marqueurs hauts écrasent les bas.
    for (i, &p) in pos.iter().enumerate() {
        controler(suivi)?;
        marqueur(&mut buf, session, p, i as u32);
        match s.ecrire_a(&buf, p) {
            Ok(()) => ecrits[i] = true,
            Err(_) => erreurs += 1,
        }
        suivi.progres(i as f32 / pos.len() as f32 * 0.5, "écriture des marqueurs");
    }
    if erreurs as usize == pos.len() {
        return Err("aucun marqueur n'a pu être écrit : la clé refuse l'écriture.".into());
    }
    s.synchroniser()
        .map_err(|e| format!("synchronisation : {e}"))?;

    let mut attendu = TamponAligne::new(BLOC_ALEATOIRE);
    let (mut corrects, mut fiable, mut repliement) = (0u32, None, false);
    for (i, &p) in pos.iter().enumerate() {
        controler(suivi)?;
        let bon = ecrits[i]
            && match s.lire_a(&mut buf, p) {
                Ok(()) => {
                    marqueur(&mut attendu, session, p, i as u32);
                    if buf[..] == attendu[..] {
                        true
                    } else {
                        // Marqueur de cette session, mais d'une autre position : repliement.
                        if &buf[..16] == MAGIE
                            && buf[16..24] == session.to_le_bytes()
                            && buf[24..32] != p.to_le_bytes()
                        {
                            repliement = true;
                        }
                        false
                    }
                }
                Err(_) => {
                    erreurs += 1;
                    false
                }
            };
        if bon {
            corrects += 1;
        } else if fiable.is_none() {
            fiable = Some(p);
        }
        suivi.progres(
            0.5 + i as f32 / pos.len() as f32 * 0.5,
            "relecture des marqueurs",
        );
    }
    Ok(Capacite {
        marqueurs: pos.len() as u32,
        corrects,
        fiable: fiable.unwrap_or(taille),
        repliement,
        erreurs,
    })
}

fn mediane(v: &[f32]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut t = v.to_vec();
    t.sort_by(f32::total_cmp);
    t[t.len() / 2] as f64
}

/// Repère la fin du cache SLC dans une série de débits : premier point à partir duquel la moyenne
/// glissante passe sous la moitié du débit initial et la suite reste basse.
fn chute(debits: &[f32], debut: f64) -> Option<usize> {
    const FENETRE: usize = 4;
    if debits.len() < 12 || debut <= 0.0 {
        return None;
    }
    (6..=debits.len() - FENETRE).find(|&i| {
        if (debits[i] as f64) >= debut * 0.5 {
            return false;
        }
        let glissante = debits[i..i + FENETRE].iter().sum::<f32>() as f64 / FENETRE as f64;
        let reste = &debits[i..];
        glissante < debut * 0.5 && mediane(reste) < debut * 0.6
    })
}

/// Longue écriture séquentielle par blocs de 4 Mio, dans `zone`, pendant `duree` au plus.
pub fn ecriture_longue(
    s: &impl Support,
    zone: (u64, u64),
    duree: Duration,
    suivi: &dyn Suivi,
) -> Result<EcritureLongue, String> {
    let bloc = BLOC_SEQUENTIEL as u64;
    let mut buf = TamponAligne::new(BLOC_SEQUENTIEL);
    let mut h = Hasard::new(graine());
    h.remplir(&mut buf);
    let debut = Instant::now();
    let mut pos = zone.0.div_ceil(bloc) * bloc;
    let depart = pos;
    let (mut points, mut debits) = (Vec::new(), Vec::new());
    let (mut tranche, mut octets_tranche) = (Instant::now(), 0u64);
    while debut.elapsed() < duree && pos + bloc <= zone.1 {
        controler(suivi)?;
        // Contenu différent pour chaque bloc (ni compression ni déduplication possibles).
        for i in 0..16 {
            let o = i * (BLOC_SEQUENTIEL / 16);
            buf[o..o + 8].copy_from_slice(&h.suivant().to_le_bytes());
        }
        s.ecrire_a(&buf, pos)
            .map_err(|e| format!("écriture à {pos} : {e}"))?;
        pos += bloc;
        octets_tranche += bloc;
        if tranche.elapsed() >= Duration::from_millis(500) {
            let d = (octets_tranche as f64 / 1e6 / tranche.elapsed().as_secs_f64()) as f32;
            debits.push(d);
            points.push([((pos - depart) as f64 / 1e9) as f32, d]);
            tranche = Instant::now();
            octets_tranche = 0;
        }
        let e = debut.elapsed().as_secs_f64();
        suivi.progres(
            (e / duree.as_secs_f64()).min(1.0) as f32,
            &format!(
                "{} écrits — {:.0} Mo/s",
                crate::suivi::taille_lisible(pos - depart),
                debits.last().copied().unwrap_or(0.0)
            ),
        );
    }
    s.synchroniser()
        .map_err(|e| format!("synchronisation : {e}"))?;
    let duree_s = debut.elapsed().as_secs_f64().max(1e-6);
    let octets = pos - depart;
    let debit_debut = mediane(&debits[..debits.len().min(6)]);
    let debit_fin = mediane(&debits[debits.len().saturating_sub(6)..]);
    let chute_apres = chute(&debits, debit_debut).map(|i| (points[i][0] as f64 * 1e9) as u64);
    Ok(EcritureLongue {
        octets,
        duree_s,
        mo_s: octets as f64 / 1e6 / duree_s,
        points,
        debit_debut,
        debit_fin,
        chute_apres,
    })
}

/// Enchaîne les tests sur des supports déjà ouverts (`EN_PARALLELE` descripteurs).
pub fn analyser<S: Support>(
    supports: &[S],
    directes: bool,
    taille: u64,
    reglages: Reglages,
    suivi: &dyn Suivi,
) -> Result<Analyse, String> {
    let s = &supports[0];
    let un = std::slice::from_ref(s);
    let duree = Duration::from_secs(reglages.duree_aleatoire_s);

    suivi.etape("1/5 — Capacité réelle : marqueurs sur toute la surface");
    let capacite = capacite(s, taille, suivi)?;
    // Sur une clé contrefaite, les tests suivants restent dans la partie réelle.
    let zone = (
        0,
        capacite.fiable.max(BLOC_SEQUENTIEL as u64 * 4).min(taille),
    );

    suivi.etape("2/5 — Écritures aléatoires de 4 Ko, une à la fois");
    let ecriture_1 = aleatoire(un, zone, duree, Mode::Ecriture, suivi)?;
    suivi.etape(&format!(
        "2/5 — Écritures aléatoires de 4 Ko, {EN_PARALLELE} à la fois"
    ));
    let ecriture_n = aleatoire(supports, zone, duree, Mode::Ecriture, suivi)?;

    suivi.etape("3/5 — Écritures synchronisées (comme un journal ou une base de données)");
    let synchro = aleatoire(un, zone, duree, Mode::EcritureSynchro, suivi)?;

    suivi.etape("4/5 — Longue écriture séquentielle (recherche du cache SLC)");
    let longue = ecriture_longue(s, zone, Duration::from_secs(reglages.duree_longue_s), suivi)?;

    suivi.etape("5/5 — Lectures aléatoires sur les données écrites");
    let ecrite = (0, longue.octets.max(BLOC_SEQUENTIEL as u64));
    let lecture_donnees = aleatoire(
        un,
        ecrite,
        duree.min(Duration::from_secs(6)),
        Mode::Lecture,
        suivi,
    )?;

    Ok(Analyse {
        directes,
        capacite,
        ecriture_1,
        ecriture_n,
        synchro,
        longue,
        lecture_donnees,
    })
}

/// Remet à zéro le premier et le dernier Mio : la clé apparaît vide, sans table de partitions.
fn blanchir(s: &impl Support, taille: u64) -> Result<(), String> {
    let mio = 1u64 << 20;
    let zeros = TamponAligne::new(mio as usize);
    s.ecrire_a(&zeros, 0)
        .map_err(|e| format!("remise à zéro du début : {e}"))?;
    if taille >= 2 * mio {
        s.ecrire_a(&zeros, (taille / mio - 1) * mio)
            .map_err(|e| format!("remise à zéro de la fin : {e}"))?;
    }
    s.synchroniser()
        .map_err(|e| format!("synchronisation : {e}"))
}

/// Analyse une clé de la liste (droits administrateur nécessaires). **Efface la clé.**
pub fn analyser_disque(
    d: &crate::disques::Disque,
    reglages: Reglages,
    suivi: &dyn Suivi,
) -> Result<Analyse, String> {
    suivi.etape("Préparation de la clé (démontage)");
    // Mêmes garde-fous que la fabrication : clé revérifiée, démontée ou verrouillée, et gardée
    // ainsi jusqu'à la fin (la variable `cible` vit jusqu'au bout de la fonction).
    let mut cible = crate::disques::ouvrir(d)?;
    let mut supports = Vec::new();
    let mut directes = true;
    for _ in 0..EN_PARALLELE {
        let (f, dir) = crate::disques::ouvrir_ecriture(d)?;
        directes &= dir;
        supports.push(f);
    }
    let r = analyser(&supports, directes, d.taille, reglages, suivi);
    // Même après une annulation ou une erreur, laisser une clé vide et propre.
    let fin = blanchir(&supports[0], d.taille);
    drop(supports);
    cible
        .terminer()
        .map_err(|e| format!("fin de l'analyse : {e}"))?;
    let a = r?;
    fin?;
    suivi.progres(1.0, "");
    Ok(a)
}

/// Analyse, en passant par l'assistant privilégié si nécessaire (Linux sans droits root).
pub fn analyser_cle(
    d: &crate::disques::Disque,
    reglages: Reglages,
    suivi: &dyn Suivi,
) -> Result<Analyse, String> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY : geteuid n'a pas d'effet de bord.
        if unsafe { libc::geteuid() } != 0 {
            return crate::assistant::analyser(d, reglages, suivi);
        }
    }
    analyser_disque(d, reglages, suivi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suivi::Muet;
    use std::io;
    use std::sync::Mutex;

    /// Clé simulée en mémoire. `reelle` < `annoncee` : clé contrefaite, dont les adresses sont
    /// repliées modulo la capacité réelle (comportement le plus courant de ces clés).
    struct Cle {
        donnees: Mutex<Vec<u8>>,
        reelle: u64,
    }
    impl Cle {
        fn new(reelle: u64) -> Self {
            Self {
                donnees: Mutex::new(vec![0; reelle as usize]),
                reelle,
            }
        }
    }
    impl Support for Cle {
        fn lire_a(&self, buf: &mut [u8], pos: u64) -> io::Result<()> {
            let d = self.donnees.lock().unwrap();
            for (i, o) in buf.iter_mut().enumerate() {
                *o = d[((pos + i as u64) % self.reelle) as usize];
            }
            Ok(())
        }
        fn ecrire_a(&self, buf: &[u8], pos: u64) -> io::Result<()> {
            let mut d = self.donnees.lock().unwrap();
            for (i, o) in buf.iter().enumerate() {
                d[((pos + i as u64) % self.reelle) as usize] = *o;
            }
            Ok(())
        }
        fn synchroniser(&self) -> io::Result<()> {
            Ok(())
        }
    }

    const MIO: u64 = 1 << 20;

    /// Contrefaçon dont la capacité réelle n'est pas une puissance de deux mais un multiple du pas :
    /// détectée aussi. (Un repliement sur une capacité quelconque peut échapper à l'échantillon.)
    #[test]
    fn contrefacon_multiple_du_pas() {
        let cle = Cle::new(24 * MIO);
        let c = capacite(&cle, 128 * MIO, &Muet).unwrap();
        assert!(c.repliement && !c.conforme());
        assert!(c.fiable <= 24 * MIO);
    }

    #[test]
    fn capacite_conforme() {
        let cle = Cle::new(32 * MIO);
        let c = capacite(&cle, 32 * MIO, &Muet).unwrap();
        assert!(c.conforme());
        assert_eq!(c.fiable, 32 * MIO);
        assert!(!c.repliement);
    }

    #[test]
    fn cle_contrefaite_detectee() {
        // 64 Mio annoncés, 16 Mio réels.
        let cle = Cle::new(16 * MIO);
        let c = capacite(&cle, 64 * MIO, &Muet).unwrap();
        assert!(!c.conforme());
        assert!(c.repliement);
        // Le premier marqueur perdu est le tout premier (écrasé par le repliement), ou se trouve
        // au plus à la capacité réelle : jamais au-delà.
        assert!(c.fiable <= 16 * MIO, "fiable = {}", c.fiable);
        assert!(c.corrects < c.marqueurs / 2);
    }

    #[test]
    fn positions_couvrent_toute_la_cle() {
        let p = positions(64 * MIO);
        assert!((256..=520).contains(&p.len()), "{} marqueurs", p.len());
        // Taille qui n'est pas une puissance de deux (clé de « 32 Go » réelle).
        let q = positions(31_042_043_904);
        assert!((256..=520).contains(&q.len()), "{} marqueurs", q.len());
        assert_eq!(*q.last().unwrap(), 31_042_043_904 / 4096 * 4096 - 4096);
        assert_eq!(p[0], 0);
        assert_eq!(*p.last().unwrap(), 64 * MIO - BLOC_ALEATOIRE as u64);
        assert!(p.windows(2).all(|w| w[0] < w[1]));
        assert!(p.iter().all(|x| x % BLOC_ALEATOIRE as u64 == 0));
    }

    #[test]
    fn chute_du_cache_slc() {
        // 10 s à 300 Mo/s, puis 20 s à 40 Mo/s (points toutes les 0,5 s).
        let mut d = vec![300.0f32; 20];
        d.extend(vec![40.0; 40]);
        assert_eq!(chute(&d, 300.0), Some(20));
        // Débit stable : pas de chute.
        assert_eq!(chute(&[100.0; 60], 100.0), None);
        // Un creux passager n'est pas une chute.
        let mut d = vec![200.0f32; 40];
        d[15..19].copy_from_slice(&[50.0; 4]);
        assert_eq!(chute(&d, 200.0), None);
    }

    /// Parcours complet sur une clé simulée : chaque étape produit des chiffres cohérents.
    #[test]
    fn analyse_complete_sur_cle_simulee() {
        let supports: Vec<Cle> = (0..EN_PARALLELE).map(|_| Cle::new(64 * MIO)).collect();
        let r = Reglages {
            duree_longue_s: 1,
            duree_aleatoire_s: 1,
        };
        // Les supports simulés ne partagent pas leurs données : seul le premier sert aux
        // marqueurs et à l'écriture longue, ce qui suffit à vérifier l'enchaînement.
        let a = analyser(&supports, false, 64 * MIO, r, &Muet).unwrap();
        assert!(a.capacite.conforme());
        assert!(a.ecriture_1.operations > 0 && a.ecriture_n.operations > 0);
        assert!(a.synchro.operations > 0);
        assert!(a.longue.octets > 0);
        assert_eq!(a.lecture_donnees.part_vide, 0.0);
        blanchir(&supports[0], 64 * MIO).unwrap();
        let mut b = vec![1u8; 4096];
        supports[0].lire_a(&mut b, 0).unwrap();
        assert!(b.iter().all(|&o| o == 0));
    }
}
