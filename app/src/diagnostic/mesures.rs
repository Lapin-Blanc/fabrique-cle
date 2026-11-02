//! Mesures en lecture seule : rien n'est jamais écrit sur la clé. Les lectures contournent le cache
//! du système (`O_DIRECT` sous Linux, `FILE_FLAG_NO_BUFFERING` sous Windows), sinon on mesurerait
//! la mémoire vive du PC et non la clé. Chaque test est borné en durée, pas en volume : une clé
//! lente ne fait pas durer le diagnostic plus longtemps qu'une clé rapide.

use crate::suivi::{Suivi, controler};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Alignement exigé par les E/S directes (taille d'une page, multiple de tout secteur courant).
pub const ALIGNEMENT: usize = 4096;
/// Bloc des tests aléatoires : 4 Kio, la taille d'une page mémoire et d'un bloc ext4.
pub const BLOC_ALEATOIRE: usize = 4096;
/// Bloc du test séquentiel.
pub const BLOC_SEQUENTIEL: usize = 4 << 20;

/// Bornes (en µs) des classes de l'histogramme des latences ; la dernière classe est « au-delà ».
pub const CLASSES_US: [u32; 9] = [100, 200, 500, 1_000, 2_000, 5_000, 10_000, 20_000, 50_000];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Latences {
    pub mediane_us: u32,
    pub p90_us: u32,
    pub p99_us: u32,
    pub max_us: u32,
    /// Nombre d'opérations par classe (`CLASSES_US.len() + 1` classes).
    pub histogramme: Vec<u64>,
}

impl Latences {
    fn depuis(mut l: Vec<u32>) -> Self {
        let mut histogramme = vec![0u64; CLASSES_US.len() + 1];
        for &v in &l {
            let i = CLASSES_US
                .iter()
                .position(|&b| v < b)
                .unwrap_or(CLASSES_US.len());
            histogramme[i] += 1;
        }
        l.sort_unstable();
        let q = |f: f64| -> u32 {
            if l.is_empty() {
                0
            } else {
                l[((l.len() - 1) as f64 * f).round() as usize]
            }
        };
        Self {
            mediane_us: q(0.5),
            p90_us: q(0.9),
            p99_us: q(0.99),
            max_us: l.last().copied().unwrap_or(0),
            histogramme,
        }
    }
}

/// Résultat d'un test de lectures aléatoires de 4 Kio.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Aleatoire {
    /// Nombre de lectures lancées en même temps.
    pub en_parallele: u32,
    pub operations: u64,
    pub duree_s: f64,
    /// Opérations par seconde.
    pub iops: f64,
    pub mo_s: f64,
    pub latences: Latences,
    /// Part des blocs lus entièrement nuls (zones jamais écrites ou effacées).
    pub part_vide: f64,
}

/// Résultat du test de lecture séquentielle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sequentielle {
    pub octets: u64,
    pub duree_s: f64,
    pub mo_s: f64,
    /// Débit par demi-seconde (Mo/s), pour voir s'il est stable.
    pub serie_mo_s: Vec<f32>,
    pub part_vide: f64,
}

/// Ensemble des mesures en lecture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mesures {
    /// Les lectures ont bien contourné le cache du système (sinon : mesures non fiables).
    pub directes: bool,
    pub sequentielle: Sequentielle,
    pub aleatoire_1: Aleatoire,
    pub aleatoire_n: Aleatoire,
}

/// Durées des trois tests.
#[derive(Clone, Copy, Debug)]
pub struct Durees {
    pub sequentielle: Duration,
    pub aleatoire: Duration,
}

impl Default for Durees {
    fn default() -> Self {
        Self {
            sequentielle: Duration::from_secs(8),
            aleatoire: Duration::from_secs(8),
        }
    }
}

/// Nombre de lectures simultanées du second test aléatoire.
pub const EN_PARALLELE: u32 = 8;

/// Tampon aligné, comme l'exigent les E/S directes (un `Vec<u8>` ne l'est pas forcément).
pub struct TamponAligne {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

// SAFETY : le tampon est une simple zone mémoire possédée, sans partage.
unsafe impl Send for TamponAligne {}

impl TamponAligne {
    pub fn new(taille: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(taille.max(1), ALIGNEMENT).unwrap();
        // SAFETY : taille non nulle ; zone initialisée à zéro.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Self { ptr, layout }
    }
}

impl std::ops::Deref for TamponAligne {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY : zone allouée de cette taille, vivante tant que `self`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.layout.size()) }
    }
}
impl std::ops::DerefMut for TamponAligne {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY : idem, accès exclusif par `&mut self`.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}
impl Drop for TamponAligne {
    fn drop(&mut self) {
        // SAFETY : même pointeur et même disposition qu'à l'allocation.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

/// Lecture complète à une position donnée, sans déplacer de curseur partagé.
fn lire_a(f: &File, buf: &mut [u8], pos: u64) -> io::Result<()> {
    let mut lu = 0;
    while lu < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(f, &mut buf[lu..], pos + lu as u64)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(f, &mut buf[lu..], pos + lu as u64)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fin du disque",
            ));
        }
        lu += n;
    }
    Ok(())
}

/// Générateur pseudo-aléatoire minimal (xorshift64*) : suffisant pour tirer des positions.
struct Hasard(u64);
impl Hasard {
    fn new(graine: u64) -> Self {
        Self(graine | 1)
    }
    fn suivant(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

fn graine() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        return t.as_nanos() as u64;
    }
    u64::from_le_bytes(b)
}

fn ms_lisible(us: u32) -> String {
    if us < 1000 {
        format!("{us} µs")
    } else {
        format!("{:.1} ms", us as f64 / 1000.0).replace('.', ",")
    }
}

/// Lit le début de la clé par blocs de 4 Mio, pendant `duree` au plus.
fn sequentielle(
    f: &File,
    taille: u64,
    duree: Duration,
    suivi: &dyn Suivi,
) -> Result<Sequentielle, String> {
    let mut buf = TamponAligne::new(BLOC_SEQUENTIEL);
    let debut = Instant::now();
    let (mut pos, mut vides, mut blocs) = (0u64, 0u64, 0u64);
    let mut serie = Vec::new();
    let (mut tranche, mut octets_tranche) = (Instant::now(), 0u64);
    while debut.elapsed() < duree && pos + BLOC_SEQUENTIEL as u64 <= taille {
        controler(suivi)?;
        lire_a(f, &mut buf, pos).map_err(|e| format!("lecture à {pos} : {e}"))?;
        pos += BLOC_SEQUENTIEL as u64;
        octets_tranche += BLOC_SEQUENTIEL as u64;
        blocs += 1;
        if buf.iter().all(|&o| o == 0) {
            vides += 1;
        }
        if tranche.elapsed() >= Duration::from_millis(500) {
            serie.push((octets_tranche as f64 / 1e6 / tranche.elapsed().as_secs_f64()) as f32);
            tranche = Instant::now();
            octets_tranche = 0;
        }
        let s = debut.elapsed().as_secs_f64();
        suivi.progres(
            (s / duree.as_secs_f64()).min(1.0) as f32,
            &format!("{:.0} Mo/s", pos as f64 / 1e6 / s.max(1e-6)),
        );
    }
    let duree_s = debut.elapsed().as_secs_f64().max(1e-6);
    Ok(Sequentielle {
        octets: pos,
        duree_s,
        mo_s: pos as f64 / 1e6 / duree_s,
        serie_mo_s: serie,
        part_vide: if blocs == 0 {
            0.0
        } else {
            vides as f64 / blocs as f64
        },
    })
}

/// Lectures de 4 Kio à des positions tirées au hasard sur toute la clé, `n` à la fois
/// (un fil d'exécution et un descripteur par lecture simultanée).
fn aleatoire(
    fichiers: &[File],
    taille: u64,
    duree: Duration,
    suivi: &dyn Suivi,
) -> Result<Aleatoire, String> {
    let blocs = taille / BLOC_ALEATOIRE as u64;
    if blocs == 0 {
        return Err("disque trop petit".into());
    }
    let arret = AtomicBool::new(false);
    let debut = Instant::now();
    let resultats: Vec<Result<(Vec<u32>, u64), String>> = std::thread::scope(|s| {
        let fils: Vec<_> = fichiers
            .iter()
            .map(|f| {
                let arret = &arret;
                s.spawn(move || {
                    let mut h = Hasard::new(graine());
                    let mut buf = TamponAligne::new(BLOC_ALEATOIRE);
                    let (mut lat, mut vides) = (Vec::new(), 0u64);
                    while !arret.load(Ordering::Relaxed) {
                        let pos = (h.suivant() % blocs) * BLOC_ALEATOIRE as u64;
                        let t = Instant::now();
                        lire_a(f, &mut buf, pos).map_err(|e| format!("lecture à {pos} : {e}"))?;
                        lat.push(t.elapsed().as_micros().min(u32::MAX as u128) as u32);
                        if buf.iter().all(|&o| o == 0) {
                            vides += 1;
                        }
                    }
                    Ok((lat, vides))
                })
            })
            .collect();
        // Le fil principal surveille la durée, la progression et l'annulation.
        while debut.elapsed() < duree && !suivi.annule() && !fils.iter().any(|f| f.is_finished()) {
            std::thread::sleep(Duration::from_millis(100));
            suivi.progres(
                (debut.elapsed().as_secs_f64() / duree.as_secs_f64()).min(1.0) as f32,
                "",
            );
        }
        arret.store(true, Ordering::Relaxed);
        fils.into_iter()
            .map(|f| {
                f.join()
                    .unwrap_or_else(|_| Err("fil de mesure interrompu".into()))
            })
            .collect()
    });
    controler(suivi)?;
    let duree_s = debut.elapsed().as_secs_f64().max(1e-6);
    let (mut toutes, mut vides) = (Vec::new(), 0u64);
    for r in resultats {
        let (l, v) = r?;
        toutes.extend(l);
        vides += v;
    }
    let operations = toutes.len() as u64;
    let iops = operations as f64 / duree_s;
    Ok(Aleatoire {
        en_parallele: fichiers.len() as u32,
        operations,
        duree_s,
        iops,
        mo_s: iops * BLOC_ALEATOIRE as f64 / 1e6,
        latences: Latences::depuis(toutes),
        part_vide: if operations == 0 {
            0.0
        } else {
            vides as f64 / operations as f64
        },
    })
}

/// Lance les trois tests. `ouvrir` fournit un nouveau descripteur en lecture (et dit si les
/// lectures contournent le cache) ; il est appelé une fois par lecture simultanée.
pub fn mesurer(
    ouvrir: &dyn Fn() -> Result<(File, bool), String>,
    taille: u64,
    durees: Durees,
    suivi: &dyn Suivi,
) -> Result<Mesures, String> {
    let (f, directes) = ouvrir()?;

    suivi.etape("Lecture séquentielle (gros blocs à la suite)");
    let sequentielle = sequentielle(&f, taille, durees.sequentielle, suivi)?;

    suivi.etape("Lectures aléatoires de 4 Ko, une à la fois");
    let aleatoire_1 = aleatoire(std::slice::from_ref(&f), taille, durees.aleatoire, suivi)?;
    suivi.progres(
        1.0,
        &format!(
            "{:.0} lectures/s, médiane {}",
            aleatoire_1.iops,
            ms_lisible(aleatoire_1.latences.mediane_us)
        ),
    );

    suivi.etape(&format!(
        "Lectures aléatoires de 4 Ko, {EN_PARALLELE} en même temps"
    ));
    let mut fichiers = vec![f];
    for _ in 1..EN_PARALLELE {
        fichiers.push(ouvrir()?.0);
    }
    let aleatoire_n = aleatoire(&fichiers, taille, durees.aleatoire, suivi)?;
    suivi.progres(1.0, "");

    Ok(Mesures {
        directes,
        sequentielle,
        aleatoire_1,
        aleatoire_n,
    })
}

/// Mesure une clé USB de la liste (droits administrateur nécessaires).
pub fn mesurer_disque(d: &crate::disques::Disque, suivi: &dyn Suivi) -> Result<Mesures, String> {
    d.retrouver()?;
    suivi.etape("Ouverture de la clé en lecture seule");
    mesurer(
        &|| crate::disques::ouvrir_lecture(d),
        d.taille,
        Durees::default(),
        suivi,
    )
}

/// Mesure, en passant par l'assistant privilégié si nécessaire (Linux sans droits root).
pub fn mesurer_cle(d: &crate::disques::Disque, suivi: &dyn Suivi) -> Result<Mesures, String> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY : geteuid n'a pas d'effet de bord.
        if unsafe { libc::geteuid() } != 0 {
            return crate::assistant::diagnostiquer(d, suivi);
        }
    }
    mesurer_disque(d, suivi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suivi::Muet;

    #[test]
    fn latences_et_histogramme() {
        let l = Latences::depuis(vec![50, 150, 150, 300, 800, 60_000]);
        assert_eq!(l.histogramme.iter().sum::<u64>(), 6);
        assert_eq!(l.histogramme[0], 1); // < 100 µs
        assert_eq!(l.histogramme[1], 2); // 100–200 µs
        assert_eq!(*l.histogramme.last().unwrap(), 1); // ≥ 50 ms
        assert_eq!(l.max_us, 60_000);
        assert_eq!(l.mediane_us, 300);
    }

    #[test]
    fn tampon_aligne() {
        let t = TamponAligne::new(BLOC_ALEATOIRE);
        assert_eq!(t.as_ptr() as usize % ALIGNEMENT, 0);
        assert_eq!(t.len(), BLOC_ALEATOIRE);
    }

    /// Parcours complet sur un fichier ordinaire (sans E/S directes) : vérifie l'enchaînement,
    /// les comptes et la détection des zones vides, pas les chiffres.
    #[test]
    fn mesures_sur_fichier() {
        let chemin =
            std::env::temp_dir().join(format!("fabcle-diagnostic-{}.bin", std::process::id()));
        // 16 Mio : moitié de zéros, moitié de données.
        let mut contenu = vec![0u8; 16 << 20];
        for (i, o) in contenu[8 << 20..].iter_mut().enumerate() {
            *o = (i % 251) as u8 + 1;
        }
        std::fs::write(&chemin, &contenu).unwrap();
        let ouvrir = || {
            File::open(&chemin)
                .map(|f| (f, false))
                .map_err(|e| e.to_string())
        };
        let durees = Durees {
            sequentielle: Duration::from_millis(300),
            aleatoire: Duration::from_millis(300),
        };
        let m = mesurer(&ouvrir, contenu.len() as u64, durees, &Muet).unwrap();
        assert!(!m.directes);
        assert!(m.sequentielle.octets > 0);
        assert!(m.aleatoire_1.operations > 0);
        assert_eq!(m.aleatoire_1.en_parallele, 1);
        assert_eq!(m.aleatoire_n.en_parallele, EN_PARALLELE);
        let p = m.aleatoire_n.part_vide;
        assert!((0.35..0.65).contains(&p), "part vide {p}");
        // Rien n'a été écrit.
        assert_eq!(std::fs::read(&chemin).unwrap(), contenu);
        std::fs::remove_file(&chemin).ok();
    }
}
