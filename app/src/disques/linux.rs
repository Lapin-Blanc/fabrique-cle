//! Linux : liste des disques via sysfs, ouverture exclusive du périphérique (root nécessaire :
//! l'interface relance l'application en « assistant » avec pkexec, voir `assistant.rs`).

use super::{Disque, propre};
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
            || systeme.contains(&nom)
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
        disques.push(Disque {
            chemin: format!("/dev/{nom}"),
            nom: nom_affiche,
            serie: serie_udev(&nom),
            taille,
        });
    }
    disques.sort_by(|a, b| a.chemin.cmp(&b.chemin));
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
    fn le_disque_systeme_n_est_jamais_propose() {
        let systeme = super::disques_systeme();
        let liste = super::lister().unwrap();
        for d in &liste {
            assert!(!systeme.contains(d.chemin.trim_start_matches("/dev/")));
        }
    }
}
