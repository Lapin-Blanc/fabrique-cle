//! Windows : disques physiques `\\.\PhysicalDriveN` reliés en USB, hors disque système.
//! Avant l'écriture, chaque volume de la clé est verrouillé puis démonté (sinon Windows refuse
//! d'écrire dans les secteurs d'un volume monté). L'application demande les droits administrateur
//! (manifeste, voir `build.rs`).

use super::{Disque, propre};
use crate::ecriture::Cible;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW,
    FindVolumeClose, FlushFileBuffers, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_DISK_GET_LENGTH_INFO,
    IOCTL_DISK_UPDATE_PROPERTIES, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

const BUS_USB: u32 = 7;

fn large(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ouvre un périphérique ; `None` si impossible.
fn ouvrir_handle(chemin: &str, acces: u32) -> Option<OwnedHandle> {
    let w = large(chemin);
    // SAFETY : chaîne terminée par un zéro, paramètres valides.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            acces,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        None
    } else {
        // SAFETY : handle valide que nous possédons désormais.
        Some(unsafe { OwnedHandle::from_raw_handle(h as _) })
    }
}

/// DeviceIoControl ; retourne le nombre d'octets reçus.
fn ioctl(h: &OwnedHandle, code: u32, entree: &[u8], sortie: &mut [u8]) -> Option<usize> {
    let mut recus = 0u32;
    // SAFETY : tampons valides de la longueur indiquée.
    let ok = unsafe {
        DeviceIoControl(
            h.as_raw_handle() as HANDLE,
            code,
            if entree.is_empty() {
                null()
            } else {
                entree.as_ptr().cast()
            },
            entree.len() as u32,
            if sortie.is_empty() {
                null_mut()
            } else {
                sortie.as_mut_ptr().cast()
            },
            sortie.len() as u32,
            &mut recus,
            null_mut(),
        )
    };
    (ok != 0).then_some(recus as usize)
}

fn u32_le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

/// Chaîne ASCII terminée par un zéro à l'offset `o` d'un descripteur.
fn chaine(b: &[u8], o: usize) -> String {
    if o == 0 || o >= b.len() {
        return String::new();
    }
    let fin = b[o..]
        .iter()
        .position(|&c| c == 0)
        .map_or(b.len(), |p| o + p);
    propre(&String::from_utf8_lossy(&b[o..fin]))
}

/// Numéro du disque physique qui porte un volume (`\\.\C:`).
fn numero_disque(h: &OwnedHandle) -> Option<u32> {
    let mut b = [0u8; 12]; // STORAGE_DEVICE_NUMBER
    ioctl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut b)?;
    Some(u32_le(&b, 4))
}

/// Disques physiques sur lesquels s'étend un volume.
fn disques_du_volume(h: &OwnedHandle) -> Vec<u32> {
    let mut b = vec![0u8; 8 + 24 * 32]; // VOLUME_DISK_EXTENTS + DISK_EXTENT[32]
    if ioctl(h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut b).is_none() {
        return Vec::new();
    }
    let n = (u32_le(&b, 0) as usize).min(32);
    (0..n).map(|i| u32_le(&b, 8 + 24 * i)).collect()
}

/// Numéro du disque qui contient Windows.
fn disque_systeme() -> Option<u32> {
    let mut buf = [0u16; 260];
    // SAFETY : tampon de 260 caractères.
    let n = unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let dossier = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    let lettre = dossier.get(..2)?; // « C: »
    let h = ouvrir_handle(&format!("\\\\.\\{lettre}"), 0)?;
    numero_disque(&h)
}

/// Tous les volumes (`\\?\Volume{…}\`) du système.
fn volumes() -> Vec<String> {
    let mut res = Vec::new();
    let mut buf = [0u16; 64];
    // SAFETY : tampon de 64 caractères (un nom de volume en fait 49).
    let f = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if f == INVALID_HANDLE_VALUE {
        return res;
    }
    loop {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        res.push(String::from_utf16_lossy(&buf[..n]));
        // SAFETY : handle de recherche valide.
        if unsafe { FindNextVolumeW(f, buf.as_mut_ptr(), buf.len() as u32) } == 0 {
            break;
        }
    }
    // SAFETY : handle de recherche valide.
    unsafe { FindVolumeClose(f) };
    res
}

fn decrire(n: u32, systeme: Option<u32>) -> Option<Disque> {
    if Some(n) == systeme {
        return None;
    }
    let chemin = format!("\\\\.\\PhysicalDrive{n}");
    let h = ouvrir_handle(&chemin, GENERIC_READ)?;
    let requete = [0u8; 12]; // StorageDeviceProperty, PropertyStandardQuery
    let mut d = vec![0u8; 1024];
    ioctl(&h, IOCTL_STORAGE_QUERY_PROPERTY, &requete, &mut d)?;
    let amovible = d[10] != 0;
    let bus = u32_le(&d, 28);
    if bus != BUS_USB && !amovible {
        return None;
    }
    let nom = propre(&format!(
        "{} {}",
        chaine(&d, u32_le(&d, 12) as usize),
        chaine(&d, u32_le(&d, 16) as usize)
    ));
    let serie = chaine(&d, u32_le(&d, 24) as usize);
    let mut l = [0u8; 8];
    ioctl(&h, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut l)?;
    let taille = u64::from_le_bytes(l);
    if taille == 0 {
        return None;
    }
    Some(Disque {
        chemin,
        nom,
        serie,
        taille,
    })
}

pub fn lister() -> Result<Vec<Disque>, String> {
    let systeme = disque_systeme();
    if systeme.is_none() {
        // Sans savoir où est Windows, ne rien proposer plutôt que risquer le disque système.
        return Err(
            "Disque système non identifié : lancez l'application en administrateur.".into(),
        );
    }
    Ok((0..64).filter_map(|n| decrire(n, systeme)).collect())
}

pub struct DisqueWindows {
    f: File,
    taille: u64,
    /// Volumes de la clé, verrouillés et démontés pendant toute l'écriture.
    _volumes: Vec<OwnedHandle>,
}

pub fn ouvrir(d: &Disque) -> Result<Box<dyn Cible>, String> {
    d.retrouver()?;
    let n: u32 = d
        .chemin
        .strip_prefix("\\\\.\\PhysicalDrive")
        .and_then(|s| s.parse().ok())
        .ok_or("chemin de disque inattendu")?;

    let mut verrous = Vec::new();
    for v in volumes() {
        let chemin = v.trim_end_matches('\\');
        let Some(h) = ouvrir_handle(chemin, GENERIC_READ | GENERIC_WRITE) else {
            continue;
        };
        if !disques_du_volume(&h).contains(&n) {
            continue;
        }
        let mut verrouille = false;
        for _ in 0..20 {
            if ioctl(&h, FSCTL_LOCK_VOLUME, &[], &mut []).is_some() {
                verrouille = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if !verrouille {
            return Err(
                "Impossible de verrouiller un volume de la clé : fermez l'Explorateur et les \
                 programmes qui utilisent la clé, puis recommencez."
                    .into(),
            );
        }
        ioctl(&h, FSCTL_DISMOUNT_VOLUME, &[], &mut []);
        verrous.push(h);
    }

    let h = ouvrir_handle(&d.chemin, GENERIC_READ | GENERIC_WRITE).ok_or_else(|| {
        format!(
            "Accès refusé à {} : lancez l'application en administrateur.",
            d.chemin
        )
    })?;
    // SAFETY : conversion d'un handle possédé en fichier (même propriétaire).
    let f =
        unsafe { File::from_raw_handle(std::os::windows::io::IntoRawHandle::into_raw_handle(h)) };
    Ok(Box::new(DisqueWindows {
        f,
        taille: d.taille,
        _volumes: verrous,
    }))
}

impl Read for DisqueWindows {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.f.read(buf)
    }
}
impl Write for DisqueWindows {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.f.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}
impl Seek for DisqueWindows {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.f.seek(pos)
    }
}

impl Cible for DisqueWindows {
    fn taille(&mut self) -> io::Result<u64> {
        Ok(self.taille)
    }
    fn synchroniser(&mut self) -> io::Result<()> {
        // SAFETY : handle valide.
        if unsafe { FlushFileBuffers(self.f.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn terminer(&mut self) -> io::Result<()> {
        self.synchroniser()?;
        // Faire relire la nouvelle table de partitions par Windows.
        // SAFETY : handle valide, aucun tampon.
        unsafe {
            DeviceIoControl(
                self.f.as_raw_handle() as HANDLE,
                IOCTL_DISK_UPDATE_PROPERTIES,
                null(),
                0,
                null_mut(),
                0,
                &mut 0,
                null_mut(),
            )
        };
        Ok(())
    }
}
