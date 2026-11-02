//! Windows : disques physiques `\\.\PhysicalDriveN` reliés en USB, hors disque système.
//! Avant l'écriture, chaque volume de la clé est verrouillé puis démonté (sinon Windows refuse
//! d'écrire dans les secteurs d'un volume monté). L'application demande les droits administrateur
//! (manifeste, voir `build.rs`).

use super::{Disque, propre};
use crate::diagnostic::materiel::Materiel;
use crate::ecriture::Cible;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DRP_ADDRESS, CM_DRP_SERVICE, CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW,
    CM_Get_Parent, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::Usb::{
    GUID_DEVINTERFACE_USB_HUB, IOCTL_USB_GET_DESCRIPTOR_FROM_NODE_CONNECTION,
    IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX, IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2,
};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW,
    FindNextVolumeW, FindVolumeClose, FlushFileBuffers, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GUID_DEVINTERFACE_DISK, IOCTL_DISK_GET_LENGTH_INFO,
    IOCTL_DISK_UPDATE_PROPERTIES, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::core::GUID;

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

// ————————————————————————————————————————————————————————————————————————————————
// Diagnostic : lecture seule et identification du matériel.
// ————————————————————————————————————————————————————————————————————————————————

/// Ouvre la clé sans cache système (`FILE_FLAG_NO_BUFFERING`), en lecture seule ou en
/// lecture-écriture.
fn ouvrir_directe(d: &Disque, acces: u32) -> Result<(File, bool), String> {
    let w = large(&d.chemin);
    // SAFETY : chaîne terminée par un zéro, paramètres valides.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            acces,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(format!(
            "Accès refusé à {} : lancez l'application en administrateur.",
            d.chemin
        ));
    }
    // SAFETY : handle valide que nous possédons désormais.
    Ok((unsafe { File::from_raw_handle(h as _) }, true))
}

/// Lecture seule, pour le diagnostic. Ni verrouillage ni démontage : une clé en service peut être
/// examinée sans risque.
pub fn ouvrir_lecture(d: &Disque) -> Result<(File, bool), String> {
    ouvrir_directe(d, GENERIC_READ)
}

/// Descripteur supplémentaire en lecture-écriture directe, pour l'analyse complète. À n'appeler
/// qu'après [`ouvrir`], dont les verrous sur les volumes de la clé autorisent l'écriture.
pub fn ouvrir_ecriture(d: &Disque) -> Result<(File, bool), String> {
    ouvrir_directe(d, GENERIC_READ | GENERIC_WRITE)
}

// Identifiants STORAGE_PROPERTY_ID.
const PROP_ADAPTATEUR: u32 = 1;
const PROP_ALIGNEMENT: u32 = 6;
const PROP_PENALITE_DEPLACEMENT: u32 = 7;
const PROP_TRIM: u32 = 8;

/// IOCTL_STORAGE_QUERY_PROPERTY (requête standard) ; nombre d'octets reçus.
fn propriete(h: &OwnedHandle, id: u32, sortie: &mut [u8]) -> Option<usize> {
    let mut requete = [0u8; 12]; // STORAGE_PROPERTY_QUERY
    requete[..4].copy_from_slice(&id.to_le_bytes());
    ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &requete, sortie)
}

/// Chemins des interfaces présentes d'une classe donnée, avec leur nœud (DevInst).
fn interfaces(classe: &GUID) -> Vec<(String, u32)> {
    let mut res = Vec::new();
    // SAFETY : GUID valide, pas d'énumérateur ni de fenêtre.
    let ens = unsafe {
        SetupDiGetClassDevsW(
            classe,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if ens == INVALID_HANDLE_VALUE as HDEVINFO {
        return res;
    }
    for i in 0..256 {
        // SAFETY : structures C remplies de zéros puis dimensionnées.
        let mut itf: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        itf.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        // SAFETY : ensemble valide, structure dimensionnée.
        if unsafe { SetupDiEnumDeviceInterfaces(ens, null(), classe, i, &mut itf) } == 0 {
            break;
        }
        let mut requis = 0u32;
        // SAFETY : premier appel pour connaître la taille nécessaire.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(ens, &itf, null_mut(), 0, &mut requis, null_mut())
        };
        if requis < 8 {
            continue;
        }
        // Tampon de u32 : alignement suffisant pour SP_DEVICE_INTERFACE_DETAIL_DATA_W.
        let mut tampon = vec![0u32; (requis as usize).div_ceil(4)];
        let detail = tampon
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY : tampon d'au moins `requis` octets ; cbSize = taille de l'en-tête.
        unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
        // SAFETY : structures C remplies de zéros puis dimensionnées.
        let mut info: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
        // SAFETY : tampon et structures valides, de la taille annoncée.
        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(ens, &itf, detail, requis, null_mut(), &mut info)
        };
        if ok == 0 {
            continue;
        }
        // Le chemin (UTF-16 terminé par un zéro) commence juste après cbSize.
        let octets: &[u8] =
            // SAFETY : vue en octets du tampon possédé.
            unsafe { std::slice::from_raw_parts(tampon.as_ptr().cast(), tampon.len() * 4) };
        let utf16: Vec<u16> = octets[4..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        res.push((String::from_utf16_lossy(&utf16), info.DevInst));
    }
    // SAFETY : ensemble obtenu ci-dessus.
    unsafe { SetupDiDestroyDeviceInfoList(ens) };
    res
}

/// Identifiant d'instance d'un nœud (« USB\VID_0781&PID_5581\4C53… »).
fn id_noeud(noeud: u32) -> Option<String> {
    let mut b = [0u16; 512];
    // SAFETY : tampon de la longueur indiquée.
    if unsafe { CM_Get_Device_IDW(noeud, b.as_mut_ptr(), b.len() as u32, 0) } != CR_SUCCESS {
        return None;
    }
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    Some(String::from_utf16_lossy(&b[..n]))
}

fn parent(noeud: u32) -> Option<u32> {
    let mut p = 0u32;
    // SAFETY : pointeur de sortie valide.
    (unsafe { CM_Get_Parent(&mut p, noeud, 0) } == CR_SUCCESS).then_some(p)
}

/// Propriété de registre d'un nœud (CM_DRP_*), brute.
fn propriete_noeud(noeud: u32, prop: u32) -> Option<Vec<u8>> {
    let mut b = vec![0u8; 512];
    let mut longueur = b.len() as u32;
    // SAFETY : tampon de la longueur indiquée.
    let r = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            noeud,
            prop,
            null_mut(),
            b.as_mut_ptr().cast(),
            &mut longueur,
            0,
        )
    };
    (r == CR_SUCCESS).then(|| {
        b.truncate(longueur as usize);
        b
    })
}

fn chaine_utf16(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    propre(&String::from_utf16_lossy(&u))
}

/// Champ hexadécimal « VID_0781 » d'un identifiant d'instance USB.
fn champ_hexa(id: &str, cle: &str) -> Option<u16> {
    let i = id.find(cle)? + cle.len();
    u16::from_str_radix(id.get(i..i + 4)?, 16).ok()
}

/// Chaîne d'un descripteur USB (fabricant, produit…), demandée au concentrateur ; `indice` vient
/// du descripteur de périphérique (0 : pas de chaîne). Langue : anglais américain (0x0409), la
/// seule que fournissent presque toutes les clés.
fn chaine_usb(h: &OwnedHandle, port: u32, indice: u8) -> Option<String> {
    if indice == 0 {
        return None;
    }
    // USB_DESCRIPTOR_REQUEST (aligné sur 1 octet) : port (4), paquet SETUP (8), puis les données.
    let mut b = vec![0u8; 12 + 255];
    b[..4].copy_from_slice(&port.to_le_bytes());
    b[4] = 0x80; // périphérique vers hôte
    b[5] = 6; // GET_DESCRIPTOR
    b[6..8].copy_from_slice(&((3u16 << 8) | indice as u16).to_le_bytes()); // descripteur de chaîne
    b[8..10].copy_from_slice(&0x0409u16.to_le_bytes());
    b[10..12].copy_from_slice(&255u16.to_le_bytes());
    let entree = b.clone();
    let n = ioctl(
        h,
        IOCTL_USB_GET_DESCRIPTOR_FROM_NODE_CONNECTION,
        &entree,
        &mut b,
    )?;
    // Descripteur : longueur, type (3 = chaîne), puis le texte en UTF-16LE.
    let d = &b[12..n.min(b.len())];
    if d.len() < 4 || d[1] != 3 {
        return None;
    }
    let texte = chaine_utf16(&d[2..(d[0] as usize).min(d.len())]);
    (!texte.is_empty()).then_some(texte)
}

/// Vitesse et version USB, demandées au concentrateur (hub) sur lequel la clé est branchée.
fn liaison_usb(m: &mut Materiel, noeud_usb: u32) {
    let Some(port) = propriete_noeud(noeud_usb, CM_DRP_ADDRESS)
        .filter(|b| b.len() >= 4)
        .map(|b| u32_le(&b, 0))
    else {
        return;
    };
    let Some(hub) = parent(noeud_usb) else {
        return;
    };
    let Some((chemin, _)) = interfaces(&GUID_DEVINTERFACE_USB_HUB)
        .into_iter()
        .find(|(_, n)| *n == hub)
    else {
        return;
    };
    let Some(h) = ouvrir_handle(&chemin, GENERIC_WRITE).or_else(|| ouvrir_handle(&chemin, 0))
    else {
        return;
    };
    // USB_NODE_CONNECTION_INFORMATION_EX (aligné sur 1 octet) : port (4), descripteur USB (18),
    // configuration (1), vitesse (1)… ; bcdUSB à l'octet 6.
    let mut b = vec![0u8; 512];
    b[..4].copy_from_slice(&port.to_le_bytes());
    let entree = b.clone();
    if ioctl(
        &h,
        IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX,
        &entree,
        &mut b,
    )
    .is_some_and(|n| n >= 24)
    {
        let bcd = u16::from_le_bytes([b[6], b[7]]);
        // iManufacturer et iProduct : octets 14 et 15 du descripteur, qui commence à l'octet 4.
        let (i_fab, i_prod) = (b[18], b[19]);
        m.fabricant_usb = chaine_usb(&h, port, i_fab).unwrap_or_default();
        m.produit_usb = chaine_usb(&h, port, i_prod).unwrap_or_default();
        m.version_usb = Some(format!("{:x}.{:02x}", bcd >> 8, bcd & 0xff));
        m.vitesse_mbps = match b[23] {
            0 => Some(1),
            1 => Some(12),
            2 => Some(480),
            _ => Some(5000),
        };
    }
    // USB_NODE_CONNECTION_INFORMATION_EX_V2 : port, taille, protocoles demandés, indicateurs.
    let mut v2 = [0u8; 16];
    v2[..4].copy_from_slice(&port.to_le_bytes());
    v2[4..8].copy_from_slice(&16u32.to_le_bytes());
    v2[8..12].copy_from_slice(&7u32.to_le_bytes()); // USB 1.1, 2.0 et 3.0
    let entree = v2;
    if ioctl(
        &h,
        IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2,
        &entree,
        &mut v2,
    )
    .is_some_and(|n| n >= 16)
    {
        let drapeaux = u32_le(&v2, 12);
        m.superspeed_possible = Some(drapeaux & 0b1010 != 0);
        if drapeaux & 0b0100 != 0 {
            m.vitesse_mbps = Some(10000); // SuperSpeedPlus en service
        } else if drapeaux & 0b0001 != 0 {
            m.vitesse_mbps = Some(5000);
        }
    }
}

/// Ce que Windows dit de la clé : propriétés de stockage, pilote, identifiant et liaison USB.
pub fn identifier(d: &Disque) -> Materiel {
    let mut m = Materiel::default();
    let Some(n) = d
        .chemin
        .strip_prefix("\\\\.\\PhysicalDrive")
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return m;
    };
    if let Some(h) = ouvrir_handle(&d.chemin, 0) {
        let mut b = [0u8; 64];
        if propriete(&h, PROP_TRIM, &mut b).is_some_and(|r| r >= 9) {
            m.trim = Some(b[8] != 0);
        }
        let mut b = [0u8; 64];
        if propriete(&h, PROP_ALIGNEMENT, &mut b).is_some_and(|r| r >= 24) {
            m.secteur_logique = Some(u32_le(&b, 16));
            m.secteur_physique = Some(u32_le(&b, 20));
        }
        let mut b = [0u8; 64];
        if propriete(&h, PROP_PENALITE_DEPLACEMENT, &mut b).is_some_and(|r| r >= 9) {
            m.rotatif_declare = Some(b[8] != 0);
        }
        let mut b = [0u8; 64];
        if propriete(&h, PROP_ADAPTATEUR, &mut b).is_some_and(|r| r >= 12) {
            m.transfert_max_kio = Some(u32_le(&b, 8) / 1024);
        }
    }
    // Nœud du disque dans le gestionnaire de périphériques, puis remontée jusqu'au nœud USB
    // (« USB\VID_…&PID_… ») dont le service est le pilote de stockage (USBSTOR ou UASPStor).
    let Some(mut noeud) = interfaces(&GUID_DEVINTERFACE_DISK)
        .into_iter()
        .find(|(chemin, _)| ouvrir_handle(chemin, 0).and_then(|h| numero_disque(&h)) == Some(n))
        .map(|(_, noeud)| noeud)
    else {
        return m;
    };
    for _ in 0..8 {
        let Some(id) = id_noeud(noeud) else { break };
        let id = id.to_ascii_uppercase();
        if id.starts_with("USB\\VID_") {
            m.vid = champ_hexa(&id, "VID_");
            m.pid = champ_hexa(&id, "PID_");
            m.pilote = propriete_noeud(noeud, CM_DRP_SERVICE)
                .map(|b| chaine_utf16(&b))
                .filter(|s| !s.is_empty());
            liaison_usb(&mut m, noeud);
            break;
        }
        let Some(p) = parent(noeud) else { break };
        noeud = p;
    }
    // USBSTOR traite une commande à la fois ; UASPStor en accepte plusieurs.
    m.file_commandes = match m.protocole() {
        Some(crate::diagnostic::materiel::Protocole::Bot) => Some(1),
        _ => None,
    };
    m
}
