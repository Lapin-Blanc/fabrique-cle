//! Dépôt de `fabrique-cle/config.txt` dans la partition EFI (FAT32) de la clé, directement sur le
//! disque brut : Windows n'attribue pas de lettre de lecteur à une partition EFI, et sous Linux on
//! évite ainsi de monter quoi que ce soit.

use std::io::{self, Read, Seek, SeekFrom, Write};

const SECTEUR: u64 = 512;
/// GUID de type « EFI System Partition » tel qu'il est stocké dans la GPT (ordre mixte).
const TYPE_ESP: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

fn u32_le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

/// Lit un bloc aligné de 4096 octets contenant `position` (lectures alignées pour Windows).
fn lire_aligne<D: Read + Seek + ?Sized>(
    d: &mut D,
    position: u64,
    longueur: usize,
) -> io::Result<Vec<u8>> {
    let debut = position & !4095;
    let fin = (position + longueur as u64 + 4095) & !4095;
    let mut buf = vec![0u8; (fin - debut) as usize];
    d.seek(SeekFrom::Start(debut))?;
    d.read_exact(&mut buf)?;
    let i = (position - debut) as usize;
    Ok(buf[i..i + longueur].to_vec())
}

/// Position et taille (octets) de la partition EFI, d'après la GPT principale.
pub fn trouver_esp<D: Read + Seek + ?Sized>(d: &mut D) -> Result<(u64, u64), String> {
    let e = |e: io::Error| format!("lecture de la table de partitions : {e}");
    let entete = lire_aligne(d, SECTEUR, 92).map_err(e)?;
    if &entete[0..8] != b"EFI PART" {
        return Err("table de partitions GPT introuvable sur la clé".into());
    }
    let lba_entrees = u64_le(&entete, 72);
    let nombre = u32_le(&entete, 80).min(128) as usize;
    let taille_entree = u32_le(&entete, 84) as usize;
    if !(128..=4096).contains(&taille_entree) {
        return Err("table de partitions GPT invalide".into());
    }
    let entrees = lire_aligne(d, lba_entrees * SECTEUR, nombre * taille_entree).map_err(e)?;
    for k in 0..nombre {
        let ent = &entrees[k * taille_entree..(k + 1) * taille_entree];
        if ent[0..16] == TYPE_ESP {
            let (premier, dernier) = (u64_le(ent, 32), u64_le(ent, 40));
            if dernier < premier {
                break;
            }
            return Ok((premier * SECTEUR, (dernier - premier + 1) * SECTEUR));
        }
    }
    Err("partition EFI introuvable sur la clé".into())
}

/// Fenêtre sur une partie du disque, avec lectures et écritures toujours alignées sur 4096 octets
/// (exigence de Windows pour un disque brut ; fatfs, lui, lit et écrit octet par octet).
pub struct Fenetre<'a, D: Read + Write + Seek + ?Sized> {
    disque: &'a mut D,
    debut: u64,
    taille: u64,
    position: u64,
    bloc: Vec<u8>,
    /// Numéro du bloc en cache (relatif à la fenêtre) et s'il a été modifié.
    en_cache: Option<(u64, bool)>,
}

const BLOC: u64 = 4096;

impl<'a, D: Read + Write + Seek + ?Sized> Fenetre<'a, D> {
    pub fn new(disque: &'a mut D, debut: u64, taille: u64) -> Self {
        assert!(
            debut.is_multiple_of(BLOC) && taille.is_multiple_of(BLOC),
            "partition non alignée sur 4096"
        );
        Self {
            disque,
            debut,
            taille,
            position: 0,
            bloc: vec![0; BLOC as usize],
            en_cache: None,
        }
    }

    fn vider(&mut self) -> io::Result<()> {
        if let Some((n, true)) = self.en_cache {
            self.disque.seek(SeekFrom::Start(self.debut + n * BLOC))?;
            self.disque.write_all(&self.bloc)?;
            self.en_cache = Some((n, false));
        }
        Ok(())
    }

    fn charger(&mut self, n: u64) -> io::Result<()> {
        if self.en_cache.map(|(c, _)| c) == Some(n) {
            return Ok(());
        }
        self.vider()?;
        self.disque.seek(SeekFrom::Start(self.debut + n * BLOC))?;
        self.disque.read_exact(&mut self.bloc)?;
        self.en_cache = Some((n, false));
        Ok(())
    }
}

impl<D: Read + Write + Seek + ?Sized> Read for Fenetre<'_, D> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.taille || buf.is_empty() {
            return Ok(0);
        }
        self.charger(self.position / BLOC)?;
        let i = (self.position % BLOC) as usize;
        let n = buf
            .len()
            .min(BLOC as usize - i)
            .min((self.taille - self.position) as usize);
        buf[..n].copy_from_slice(&self.bloc[i..i + n]);
        self.position += n as u64;
        Ok(n)
    }
}

impl<D: Read + Write + Seek + ?Sized> Write for Fenetre<'_, D> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.position >= self.taille {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "fin de la partition",
            ));
        }
        self.charger(self.position / BLOC)?;
        let i = (self.position % BLOC) as usize;
        let n = buf
            .len()
            .min(BLOC as usize - i)
            .min((self.taille - self.position) as usize);
        self.bloc[i..i + n].copy_from_slice(&buf[..n]);
        if let Some((_, modifie)) = self.en_cache.as_mut() {
            *modifie = true;
        }
        self.position += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.vider()?;
        self.disque.flush()
    }
}

impl<D: Read + Write + Seek + ?Sized> Seek for Fenetre<'_, D> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let p = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.position as i128 + d as i128,
            SeekFrom::End(d) => self.taille as i128 + d as i128,
        };
        if p < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "position négative",
            ));
        }
        self.position = p as u64;
        Ok(self.position)
    }
}

impl<D: Read + Write + Seek + ?Sized> Drop for Fenetre<'_, D> {
    fn drop(&mut self) {
        let _ = self.vider();
    }
}

/// Écrit `fabrique-cle/config.txt` (remplacé s'il existe) dans la partition EFI de la clé.
pub fn deposer_config<D: Read + Write + Seek + ?Sized>(
    disque: &mut D,
    contenu: &str,
) -> Result<(), String> {
    let (debut, taille) = trouver_esp(disque)?;
    let e = |e: io::Error| format!("écriture de config.txt dans la partition EFI : {e}");
    let mut fenetre = Fenetre::new(disque, debut, taille);
    {
        let fs = fatfs::FileSystem::new(&mut fenetre, fatfs::FsOptions::new()).map_err(e)?;
        {
            let dossier = fs.root_dir().create_dir("fabrique-cle").map_err(e)?;
            let mut f = dossier.create_file("config.txt").map_err(e)?;
            f.truncate().map_err(e)?;
            f.write_all(contenu.as_bytes()).map_err(e)?;
            f.flush().map_err(e)?;
        }
        fs.unmount().map_err(e)?;
    }
    fenetre.flush().map_err(e)?;
    Ok(())
}

/// Relit `fabrique-cle/config.txt` (contrôle après écriture).
pub fn lire_config<D: Read + Write + Seek + ?Sized>(disque: &mut D) -> Result<String, String> {
    let (debut, taille) = trouver_esp(disque)?;
    let e = |e: io::Error| format!("relecture de config.txt : {e}");
    let mut fenetre = Fenetre::new(disque, debut, taille);
    let fs = fatfs::FileSystem::new(&mut fenetre, fatfs::FsOptions::new()).map_err(e)?;
    let mut s = String::new();
    fs.root_dir()
        .open_file("fabrique-cle/config.txt")
        .map_err(e)?
        .read_to_string(&mut s)
        .map_err(e)?;
    Ok(s)
}

/// Construit en mémoire un petit disque GPT avec une partition EFI FAT32 (pour les tests).
#[cfg(test)]
pub fn disque_de_test() -> io::Cursor<Vec<u8>> {
    let mio = 1u64 << 20;
    let mut d = vec![0u8; (40 * mio) as usize];
    d[512..520].copy_from_slice(b"EFI PART");
    d[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
    d[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
    d[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
    // Entrée 0 : partition « Linux » quelconque ; entrée 1 : l'ESP de 1 Mio à 35 Mio.
    let e1 = 1024 + 128;
    d[1024] = 0xAF;
    d[e1..e1 + 16].copy_from_slice(&TYPE_ESP);
    d[e1 + 32..e1 + 40].copy_from_slice(&(mio / 512).to_le_bytes());
    d[e1 + 40..e1 + 48].copy_from_slice(&(35 * mio / 512 - 1).to_le_bytes());
    let mut c = io::Cursor::new(d);
    {
        let mut f = Fenetre::new(&mut c, mio, 34 * mio);
        fatfs::format_volume(
            &mut f,
            fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
        )
        .unwrap();
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esp_trouvee() {
        let mut d = disque_de_test();
        assert_eq!(trouver_esp(&mut d).unwrap(), (1 << 20, 34 << 20));
        let mut vide = io::Cursor::new(vec![0u8; 1 << 20]);
        assert!(trouver_esp(&mut vide).is_err());
    }

    #[test]
    fn depot_puis_remplacement() {
        let mut d = disque_de_test();
        deposer_config(&mut d, "identifiant=lucas\nnom_complet=Élève Ünïcode\n").unwrap();
        assert_eq!(
            lire_config(&mut d).unwrap(),
            "identifiant=lucas\nnom_complet=Élève Ünïcode\n"
        );
        // Un second dépôt plus court remplace entièrement le premier.
        deposer_config(&mut d, "identifiant=emma\n").unwrap();
        assert_eq!(lire_config(&mut d).unwrap(), "identifiant=emma\n");
        // Rien n'a débordé hors de la partition.
        let v = d.into_inner();
        assert!(v[35 << 20..].iter().all(|&o| o == 0));
    }
}
