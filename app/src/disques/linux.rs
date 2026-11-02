//! Linux : liste des disques via sysfs, ouverture exclusive du périphérique (root nécessaire :
//! l'interface relance l'application en « assistant » avec pkexec, voir `assistant.rs`).

use super::{Disque, Emplacement, propre};
use crate::diagnostic::materiel::Materiel;
use crate::ecriture::Cible;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

fn lire(p: impl AsRef<Path>) -> String {
    fs::read_to_string(p)
        .map(|s| propre(&s))
        .unwrap_or_default()
}

/// Nom du disque entier (« sdb ») d'un périphérique bloc désigné par son entrée sysfs.
/// Traverse les volumes chiffrés, LVM, RAID (dossier `slaves`) jusqu'aux disques physiques.
fn disques_sous_jacents(sysfs: &Path, acc: &mut HashSet<String>, profondeur: u32) {
    let Ok(reel) = fs::canonicalize(sysfs) else {
        return;
    };
    if profondeur > 16 {
        return;
    }
    let esclaves: Vec<_> = fs::read_dir(reel.join("slaves"))
        .map(|it| it.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    if !esclaves.is_empty() {
        for e in esclaves {
            disques_sous_jacents(&Path::new("/sys/class/block").join(e), acc, profondeur + 1);
        }
    } else if reel.join("partition").exists() {
        if let Some(parent) = reel.parent().and_then(|p| p.file_name()) {
            acc.insert(parent.to_string_lossy().into_owned());
        }
    } else if let Some(nom) = reel.file_name() {
        acc.insert(nom.to_string_lossy().into_owned());
    }
}

/// Entrée sysfs d'un fichier de périphérique (`/dev/…`).
fn sysfs_de(dev: &str) -> Option<PathBuf> {
    let m = fs::metadata(dev).ok()?;
    if !m.file_type().is_block_device() {
        return None;
    }
    let (maj, min) = (libc::major(m.rdev()), libc::minor(m.rdev()));
    Some(PathBuf::from(format!("/sys/dev/block/{maj}:{min}")))
}

/// Disques qui portent le système en cours d'exécution (racine, /boot, /home…, swap).
pub(crate) fn disques_systeme() -> HashSet<String> {
    let mut acc = HashSet::new();
    let importants = [
        "/",
        "/boot",
        "/boot/efi",
        "/efi",
        "/home",
        "/usr",
        "/var",
        "/opt",
        "/srv",
    ];
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    for ligne in mountinfo.lines() {
        let champs: Vec<&str> = ligne.split(' ').collect();
        if champs.len() < 5 || !importants.contains(&champs[4]) {
            continue;
        }
        // Numéro majeur:mineur du montage (sauf btrfs/overlay : 0:N anonyme)…
        if !champs[2].starts_with("0:") {
            disques_sous_jacents(&Path::new("/sys/dev/block").join(champs[2]), &mut acc, 0);
        }
        // …et la source déclarée après « - type source ».
        if let Some(i) = champs.iter().position(|c| *c == "-")
            && let Some(src) = champs.get(i + 2).filter(|s| s.starts_with("/dev/"))
            && let Some(p) = sysfs_de(src)
        {
            disques_sous_jacents(&p, &mut acc, 0);
        }
    }
    for ligne in fs::read_to_string("/proc/swaps")
        .unwrap_or_default()
        .lines()
        .skip(1)
    {
        if let Some(p) = ligne.split_whitespace().next().and_then(sysfs_de) {
            disques_sous_jacents(&p, &mut acc, 0);
        }
    }
    acc
}

/// Numéro de série d'après la base udev (lisible sans droits particuliers).
fn serie_udev(nom: &str) -> String {
    let dev = lire(format!("/sys/block/{nom}/dev"));
    let base = fs::read_to_string(format!("/run/udev/data/b{dev}")).unwrap_or_default();
    for cle in ["E:ID_SERIAL_SHORT=", "E:ID_SCSI_SERIAL="] {
        if let Some(v) = base.lines().find_map(|l| l.strip_prefix(cle)) {
            return propre(v);
        }
    }
    String::new()
}

pub fn lister() -> Result<Vec<Disque>, String> {
    Ok(lister_usb()?
        .into_iter()
        .filter(|(_, systeme)| !systeme)
        .map(|(d, _)| d)
        .collect())
}

/// Disques USB qui portent le système en cours (clé Fabrique-Clé démarrée) : proposés seulement
/// par l'onglet Diagnostic, pour des mesures qui n'effacent rien. Jamais par la fabrication.
pub fn lister_systeme_usb() -> Vec<Disque> {
    lister_usb()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, systeme)| *systeme)
        .map(|(d, _)| d)
        .collect()
}

/// Tous les disques USB, avec l'indication « porte le système en cours ».
fn lister_usb() -> Result<Vec<(Disque, bool)>, String> {
    let systeme = disques_systeme();
    let mut disques = Vec::new();
    let entrees = fs::read_dir("/sys/block").map_err(|e| format!("lecture de /sys/block : {e}"))?;
    for e in entrees.flatten() {
        let nom = e.file_name().to_string_lossy().into_owned();
        if [
            "loop", "ram", "zram", "dm-", "md", "sr", "nbd", "fd", "mmcblk", "nvme",
        ]
        .iter()
        .any(|p| nom.starts_with(p))
        {
            continue;
        }
        // Disque relié par USB : le chemin sysfs réel passe par un contrôleur USB.
        let reel = fs::canonicalize(e.path()).unwrap_or_default();
        if !reel.to_string_lossy().contains("/usb") {
            continue;
        }
        let taille = lire(e.path().join("size")).parse::<u64>().unwrap_or(0) * 512;
        if taille == 0 {
            continue; // lecteur de cartes vide
        }
        let nom_affiche = propre(&format!(
            "{} {}",
            lire(e.path().join("device/vendor")),
            lire(e.path().join("device/model"))
        ));
        disques.push((
            Disque {
                chemin: format!("/dev/{nom}"),
                nom: nom_affiche,
                serie: serie_udev(&nom),
                taille,
            },
            systeme.contains(&nom),
        ));
    }
    disques.sort_by(|a, b| a.0.chemin.cmp(&b.0.chemin));
    Ok(disques)
}

/// Partitions du disque (« sdb1 », « sdb2 »…), d'après sysfs.
fn partitions(nom: &str) -> Vec<String> {
    fs::read_dir(format!("/sys/block/{nom}"))
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|p| {
                    p.starts_with(nom)
                        && Path::new(&format!("/sys/block/{nom}/{p}/partition")).exists()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Démonte les partitions de la clé ; refuse si un volume chiffré ou LVM y est ouvert.
fn liberer(nom: &str) -> Result<(), String> {
    let mut tous = partitions(nom);
    tous.push(nom.to_string());
    for p in &tous {
        let base = if p == nom {
            format!("/sys/block/{nom}")
        } else {
            format!("/sys/block/{nom}/{p}")
        };
        let occupe = fs::read_dir(format!("{base}/holders"))
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if occupe {
            return Err(format!(
                "La clé est en cours d'utilisation (volume chiffré ou LVM ouvert sur /dev/{p}). \
                 Fermez-le, ou débranchez puis rebranchez la clé."
            ));
        }
    }
    for p in &tous {
        let dev = format!("/dev/{p}");
        let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
        if swaps
            .lines()
            .any(|l| l.split_whitespace().next() == Some(dev.as_str()))
        {
            let _ = Command::new("swapoff").arg(&dev).status();
        }
        // Un périphérique peut être monté à plusieurs endroits : démonter jusqu'à ce qu'il n'y soit plus.
        for _ in 0..10 {
            let montages = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
            if !montages
                .lines()
                .any(|l| l.split(' ').next() == Some(dev.as_str()))
            {
                break;
            }
            let ok = Command::new("umount")
                .arg(&dev)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "Impossible de démonter {dev} : fermez les fenêtres et programmes qui utilisent la clé."
                ));
            }
        }
    }
    Ok(())
}

pub struct DisqueLinux {
    f: File,
    taille: u64,
}

pub fn ouvrir(d: &Disque) -> Result<Box<dyn Cible>, String> {
    d.retrouver()?;
    let nom = d
        .chemin
        .strip_prefix("/dev/")
        .ok_or("chemin de disque inattendu")?;
    liberer(nom)?;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_EXCL | libc::O_CLOEXEC)
        .open(&d.chemin)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::EBUSY) => format!("{} est encore utilisé par le système (EBUSY).", d.chemin),
            Some(libc::EACCES) | Some(libc::EPERM) => {
                format!(
                    "Accès refusé à {} : droits administrateur nécessaires.",
                    d.chemin
                )
            }
            _ => format!("ouverture de {} : {e}", d.chemin),
        })?;
    Ok(Box::new(DisqueLinux {
        f,
        taille: d.taille,
    }))
}

/// Ouvre la clé en lecture seule, en contournant le cache (`O_DIRECT`) pour le diagnostic.
/// Ni démontage ni accès exclusif : une clé en service peut être examinée sans risque.
/// Retourne aussi `false` si `O_DIRECT` est refusé (lectures alors moins fiables).
pub fn ouvrir_lecture(d: &Disque) -> Result<(File, bool), String> {
    let ouvrir = |drapeaux| {
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(drapeaux | libc::O_CLOEXEC)
            .open(&d.chemin)
    };
    match ouvrir(libc::O_DIRECT) {
        Ok(f) => Ok((f, true)),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => ouvrir(0)
            .map(|f| (f, false))
            .map_err(|e| format!("ouverture de {} : {e}", d.chemin)),
        Err(e) => Err(match e.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EPERM) => format!(
                "Accès refusé à {} : droits administrateur nécessaires.",
                d.chemin
            ),
            _ => format!("ouverture de {} : {e}", d.chemin),
        }),
    }
}

/// Descripteur supplémentaire en lecture-écriture directe (`O_DIRECT`), pour l'analyse complète.
/// À n'appeler qu'après [`ouvrir`], qui a revérifié, démonté et réservé la clé (`O_EXCL`) : les
/// ouvertures sans `O_EXCL` restent permises au même processus.
pub fn ouvrir_ecriture(d: &Disque) -> Result<(File, bool), String> {
    let ouvrir = |drapeaux| {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(drapeaux | libc::O_CLOEXEC)
            .open(&d.chemin)
    };
    match ouvrir(libc::O_DIRECT) {
        Ok(f) => Ok((f, true)),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => ouvrir(0)
            .map(|f| (f, false))
            .map_err(|e| format!("ouverture de {} : {e}", d.chemin)),
        Err(e) => Err(format!("ouverture de {} : {e}", d.chemin)),
    }
}

/// Point de montage tel qu'écrit dans /proc/self/mountinfo (espaces codés « \040 »…).
fn decoder_montage(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8)
        {
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Espace disponible pour un utilisateur ordinaire dans `dossier`, et identifiant du système de
/// fichiers qui le porte.
fn libre_et_volume(dossier: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dossier.as_os_str().as_bytes()).ok()?;
    // SAFETY : chaîne C valide, structure de sortie possédée.
    let mut v: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut v) } != 0 {
        return None;
    }
    let dev = fs::metadata(dossier).ok()?.dev();
    Some((v.f_bavail as u64 * v.f_frsize as u64, dev))
}

fn inscriptible(dossier: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dossier.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY : chaîne C valide.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Volumes de la clé montés en lecture-écriture, avec un dossier où l'utilisateur peut écrire
/// (la racine du volume, sinon son dossier personnel ou /var/tmp s'ils sont sur ce volume : cas
/// d'une clé Fabrique-Clé démarrée, dont le système est chiffré).
pub fn emplacements(d: &Disque) -> Vec<Emplacement> {
    let Some(nom) = d.chemin.strip_prefix("/dev/") else {
        return Vec::new();
    };
    let mut res: Vec<Emplacement> = Vec::new();
    let mut vus = HashSet::new();
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    for ligne in mountinfo.lines() {
        let champs: Vec<&str> = ligne.split(' ').collect();
        if champs.len() < 6 || champs[2].starts_with("0:") {
            continue;
        }
        if champs[5].split(',').any(|o| o == "ro") {
            continue;
        }
        let mut acc = HashSet::new();
        disques_sous_jacents(&Path::new("/sys/dev/block").join(champs[2]), &mut acc, 0);
        if !acc.contains(nom) || !vus.insert(champs[2].to_string()) {
            continue;
        }
        let point = PathBuf::from(decoder_montage(champs[4]));
        let Some((_, volume)) = libre_et_volume(&point) else {
            continue;
        };
        let mut candidats = vec![point.clone()];
        if let Some(h) = std::env::var_os("HOME") {
            candidats.push(PathBuf::from(h));
        }
        candidats.push(PathBuf::from("/var/tmp"));
        if let Some((dossier, libre)) = candidats.into_iter().find_map(|c| {
            let (libre, v) = libre_et_volume(&c)?;
            (v == volume && inscriptible(&c)).then_some((c, libre))
        }) {
            res.push(Emplacement { dossier, libre });
        }
    }
    res
}

/// Crée un fichier d'essai dans `dossier` et l'ouvre `n` fois en lecture-écriture directe.
/// Le fichier est retiré du dossier aussitôt ouvert : il disparaît à la fermeture du dernier
/// descripteur, même si l'application s'arrête brutalement. Aucun autre fichier n'est touché.
pub fn creer_fichier_essai(dossier: &Path, n: u32) -> Result<(Vec<File>, bool), String> {
    let mut alea = [0u8; 8];
    let _ = getrandom::fill(&mut alea);
    let chemin = dossier.join(format!(
        ".fabrique-cle-essai-{}-{}.tmp",
        std::process::id(),
        hex::encode(alea)
    ));
    let ouvrir = |creer: bool, direct: bool| {
        let mut o = fs::OpenOptions::new();
        o.read(true).write(true);
        if creer {
            o.create_new(true).mode(0o600);
        }
        o.custom_flags(if direct { libc::O_DIRECT } else { 0 } | libc::O_CLOEXEC)
            .open(&chemin)
    };
    let erreur = |e: io::Error| format!("fichier d'essai dans {} : {e}", dossier.display());
    let (premier, directes) = match ouvrir(true, true) {
        Ok(f) => (f, true),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
            // Système de fichiers sans E/S directes : sans elles (mesures moins fiables).
            let _ = fs::remove_file(&chemin);
            (ouvrir(true, false).map_err(erreur)?, false)
        }
        Err(e) => return Err(erreur(e)),
    };
    let mut fichiers = vec![premier];
    let suite = (1..n).try_for_each(|_| {
        fichiers.push(ouvrir(false, directes)?);
        Ok::<(), io::Error>(())
    });
    let retrait = fs::remove_file(&chemin);
    suite.map_err(erreur)?;
    retrait.map_err(erreur)?;
    Ok((fichiers, directes))
}

/// Ce que sysfs dit de la clé (lisible sans droits particuliers).
pub fn identifier(d: &Disque) -> Materiel {
    let mut m = Materiel::default();
    let Some(nom) = d.chemin.strip_prefix("/dev/") else {
        return m;
    };
    let base = Path::new("/sys/block").join(nom);
    let q = |f: &str| lire(base.join("queue").join(f));
    let nombre = |s: String| s.parse::<u64>().ok();
    m.trim = nombre(q("discard_max_bytes")).map(|v| v > 0);
    m.transfert_max_kio = nombre(q("max_hw_sectors_kb")).map(|v| v as u32);
    m.secteur_logique = nombre(q("logical_block_size")).map(|v| v as u32);
    m.secteur_physique = nombre(q("physical_block_size")).map(|v| v as u32);
    m.rotatif_declare = nombre(q("rotational")).map(|v| v == 1);
    // « mq-deadline [bfq] none » : l'ordonnanceur actif est entre crochets.
    m.ordonnanceur = q("scheduler")
        .split_whitespace()
        .find_map(|s| s.strip_prefix('[')?.strip_suffix(']').map(String::from));
    // File de commandes du périphérique SCSI : 1 en usb-storage, davantage en uas.
    m.file_commandes = nombre(lire(base.join("device/queue_depth"))).map(|v| v as u32);

    // Remonter l'arborescence : interface USB (pilote), puis périphérique USB (VID, vitesse…).
    // Ex. …/usb2/2-1/2-1:1.0/host3/target3:0:0/3:0:0:0/block/sdb
    let reel = fs::canonicalize(&base).unwrap_or_default();
    for anc in reel.ancestors() {
        if m.pilote.is_none() && anc.join("bInterfaceClass").exists() {
            m.pilote = fs::read_link(anc.join("driver"))
                .ok()
                .and_then(|l| Some(l.file_name()?.to_string_lossy().into_owned()));
        }
        if anc.join("idVendor").exists() {
            let hexa = |f: &str| u16::from_str_radix(&lire(anc.join(f)), 16).ok();
            m.vid = hexa("idVendor");
            m.pid = hexa("idProduct");
            m.fabricant_usb = lire(anc.join("manufacturer"));
            m.produit_usb = lire(anc.join("product"));
            let v = lire(anc.join("version"));
            m.version_usb = (!v.is_empty()).then_some(v);
            m.vitesse_mbps = lire(anc.join("speed"))
                .parse::<f64>()
                .ok()
                .map(|v| v as u32);
            m.superspeed_possible = m
                .version_usb
                .as_deref()
                .and_then(|v| v.split('.').next()?.parse::<u32>().ok())
                .map(|maj| maj >= 3);
            break;
        }
    }
    m
}

// Numéros d'ioctl de <linux/fs.h>.
const BLKRRPART: libc::c_ulong = 0x125f;
const BLKFLSBUF: libc::c_ulong = 0x1261;

impl Read for DisqueLinux {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.f.read(buf)
    }
}
impl Write for DisqueLinux {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.f.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}
impl Seek for DisqueLinux {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.f.seek(pos)
    }
}

impl Cible for DisqueLinux {
    fn taille(&mut self) -> io::Result<u64> {
        Ok(self.taille)
    }
    fn synchroniser(&mut self) -> io::Result<()> {
        self.f.sync_all()
    }
    fn oublier_cache(&mut self) -> io::Result<()> {
        self.f.sync_all()?;
        let fd = self.f.as_raw_fd();
        // SAFETY : appels système sur un descripteur valide, sans pointeur.
        unsafe {
            libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED);
            libc::ioctl(fd, BLKFLSBUF as _, 0);
        }
        Ok(())
    }
    fn terminer(&mut self) -> io::Result<()> {
        self.f.sync_all()?;
        // Faire relire la nouvelle table de partitions au noyau (sans gravité si refusé).
        // SAFETY : ioctl sans argument sur un descripteur valide.
        unsafe {
            libc::ioctl(self.f.as_raw_fd(), BLKRRPART as _, 0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn montage_decode() {
        assert_eq!(super::decoder_montage("/media/a\\040b"), "/media/a b");
        assert_eq!(super::decoder_montage("/"), "/");
    }

    /// Le fichier d'essai n'apparaît jamais dans le dossier, et disparaît à la fermeture.
    #[test]
    fn fichier_essai_invisible_et_efface() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let dossier = std::env::temp_dir().join(format!("fabcle-essai-{}", std::process::id()));
        std::fs::create_dir_all(&dossier).unwrap();
        let (mut f, _) = super::creer_fichier_essai(&dossier, 3).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(std::fs::read_dir(&dossier).unwrap().count(), 0);
        // Les descripteurs partagent bien le même fichier.
        let bloc = crate::diagnostic::mesures::TamponAligne::new(4096);
        let _ = f[0].write_all(&bloc);
        let mut lu = vec![1u8; 1];
        f[2].seek(SeekFrom::Start(0)).unwrap();
        f[2].read_exact(&mut lu).ok();
        drop(f);
        assert_eq!(std::fs::read_dir(&dossier).unwrap().count(), 0);
        std::fs::remove_dir_all(&dossier).ok();
    }

    #[test]
    fn le_disque_systeme_n_est_jamais_propose() {
        let systeme = super::disques_systeme();
        let liste = super::lister().unwrap();
        for d in &liste {
            assert!(!systeme.contains(d.chemin.trim_start_matches("/dev/")));
        }
    }
}
