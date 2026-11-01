//! Écriture de l'image sur la cible (clé USB, ou fichier pour les tests), puis vérification par
//! relecture. L'image est lue en flux (décompression zstd à la volée) : rien n'est décompressé
//! sur le disque du PC.

use crate::bmap::Bmap;
use crate::suivi::{Suivi, controler, taille_lisible};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Taille des écritures (multiple de 4096 : exigé par Windows pour un disque brut).
const MORCEAU: usize = 4 << 20;

/// Ce sur quoi on écrit : un disque ouvert en accès brut, ou un simple fichier.
pub trait Cible: Read + Write + Seek + Send {
    /// Taille utilisable en octets.
    fn taille(&mut self) -> io::Result<u64>;
    /// Vider les tampons jusqu'au support physique.
    fn synchroniser(&mut self) -> io::Result<()>;
    /// Oublier le cache de lecture, pour que la vérification relise vraiment la clé.
    fn oublier_cache(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Fin du travail (relecture de la table de partitions par le système…).
    fn terminer(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Cible for File {
    fn taille(&mut self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
    fn synchroniser(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

/// Lecteur qui compte les octets lus (avancement d'après le fichier compressé).
struct Compteur<R> {
    interne: R,
    lus: Arc<AtomicU64>,
}

impl<R: Read> Read for Compteur<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.interne.read(buf)?;
        self.lus.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn err(contexte: &str) -> impl Fn(io::Error) -> String + '_ {
    move |e| format!("{contexte} : {e}")
}

/// Image source : `.img.zst` (décompressée en flux) ou `.img` brute.
pub struct Source {
    lecteur: Box<dyn Read + Send>,
    lus: Arc<AtomicU64>,
    taille_fichier: u64,
}

impl Source {
    pub fn ouvrir(chemin: &Path) -> Result<Self, String> {
        let f = File::open(chemin).map_err(err(&format!("ouverture de {}", chemin.display())))?;
        let taille_fichier = f.metadata().map(|m| m.len()).unwrap_or(0);
        let lus = Arc::new(AtomicU64::new(0));
        let compteur = Compteur {
            interne: f,
            lus: lus.clone(),
        };
        let compresse = chemin
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zst"));
        let lecteur: Box<dyn Read + Send> = if compresse {
            Box::new(zstd::Decoder::new(compteur).map_err(err("décompression"))?)
        } else {
            Box::new(io::BufReader::with_capacity(1 << 20, compteur))
        };
        Ok(Self {
            lecteur,
            lus,
            taille_fichier,
        })
    }

    /// Fraction du fichier source déjà lue.
    fn fraction(&self) -> f32 {
        if self.taille_fichier == 0 {
            return 0.0;
        }
        (self.lus.load(Ordering::Relaxed) as f64 / self.taille_fichier as f64).min(1.0) as f32
    }
}

/// Copie `n` octets de la source vers la cible, à la position courante de la cible.
fn copier(
    source: &mut Source,
    n: u64,
    cible: &mut dyn Cible,
    tampon: &mut [u8],
    ecrits: &mut u64,
    suivi: &dyn Suivi,
) -> Result<(), String> {
    let mut reste = n;
    while reste > 0 {
        controler(suivi)?;
        let t = (reste as usize).min(tampon.len());
        source
            .lecteur
            .read_exact(&mut tampon[..t])
            .map_err(|e| match e.kind() {
                io::ErrorKind::UnexpectedEof => {
                    "image tronquée (téléchargement incomplet ?)".to_string()
                }
                _ => format!("lecture de l'image : {e}"),
            })?;
        cible
            .write_all(&tampon[..t])
            .map_err(err("écriture sur la clé"))?;
        reste -= t as u64;
        *ecrits += t as u64;
        suivi.progres(
            source.fraction(),
            &format!("{} écrits", taille_lisible(*ecrits)),
        );
    }
    Ok(())
}

/// Lit et jette `n` octets de la source (zones vides de l'image, non écrites).
fn sauter(source: &mut Source, n: u64) -> Result<(), String> {
    let lus = io::copy(&mut (&mut source.lecteur).take(n), &mut io::sink())
        .map_err(err("lecture de l'image"))?;
    if lus != n {
        return Err("image tronquée (téléchargement incomplet ?)".into());
    }
    Ok(())
}

/// Écrit l'image sur la cible. Avec un bmap, seuls les blocs utilisés sont écrits.
/// Retourne le nombre d'octets écrits.
pub fn ecrire_image(
    mut source: Source,
    bmap: Option<&Bmap>,
    cible: &mut dyn Cible,
    suivi: &dyn Suivi,
) -> Result<u64, String> {
    let taille_cible = cible.taille().map_err(err("taille de la clé"))?;
    if let Some(b) = bmap
        && b.taille_image > taille_cible
    {
        return Err(format!(
            "Clé trop petite : l'image demande {}, la clé fait {}.",
            taille_lisible(b.taille_image),
            taille_lisible(taille_cible)
        ));
    }
    let mut tampon = vec![0u8; MORCEAU];
    let mut ecrits = 0u64;
    cible
        .seek(SeekFrom::Start(0))
        .map_err(err("positionnement"))?;

    match bmap {
        Some(b) => {
            let mut position = 0u64; // octets de l'image déjà consommés
            for p in &b.plages {
                let (debut, longueur) = b.octets(p);
                sauter(&mut source, debut - position)?;
                cible
                    .seek(SeekFrom::Start(debut))
                    .map_err(err("positionnement"))?;
                copier(
                    &mut source,
                    longueur,
                    cible,
                    &mut tampon,
                    &mut ecrits,
                    suivi,
                )?;
                position = debut + longueur;
            }
        }
        None => loop {
            controler(suivi)?;
            let mut t = 0;
            // Remplir le tampon entièrement (sauf en fin d'image) : écritures alignées.
            while t < tampon.len() {
                match source.lecteur.read(&mut tampon[t..]) {
                    Ok(0) => break,
                    Ok(n) => t += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(format!("lecture de l'image : {e}")),
                }
            }
            if t == 0 {
                break;
            }
            if ecrits + t as u64 > taille_cible {
                return Err("Clé trop petite pour cette image.".into());
            }
            cible
                .write_all(&tampon[..t])
                .map_err(err("écriture sur la clé"))?;
            ecrits += t as u64;
            suivi.progres(
                source.fraction(),
                &format!("{} écrits", taille_lisible(ecrits)),
            );
        },
    }

    // Effacer la fin de la clé : une ancienne table de partitions de secours (GPT) qui y
    // traînerait pourrait tromper certains firmwares. Le premier démarrage en écrit une neuve.
    let image = bmap.map(|b| b.taille_image).unwrap_or(ecrits);
    let mio = 1u64 << 20;
    if taille_cible >= image + mio {
        let debut = (taille_cible - mio) & !4095;
        cible
            .seek(SeekFrom::Start(debut))
            .map_err(err("positionnement"))?;
        let zeros = vec![0u8; (taille_cible - debut) as usize & !511];
        cible
            .write_all(&zeros)
            .map_err(err("effacement de la fin de la clé"))?;
    }

    cible
        .synchroniser()
        .map_err(err("synchronisation de la clé"))?;
    suivi.progres(1.0, &format!("{} écrits", taille_lisible(ecrits)));
    Ok(ecrits)
}

/// Relit les plages écrites et compare leur SHA-256 à celles du bmap.
/// Retourne `false` si le bmap ne contient pas de sommes (vérification impossible).
pub fn verifier(bmap: &Bmap, cible: &mut dyn Cible, suivi: &dyn Suivi) -> Result<bool, String> {
    if bmap.plages.iter().any(|p| p.sha256.is_none()) {
        return Ok(false);
    }
    cible.oublier_cache().map_err(err("vidage du cache"))?;
    let total = bmap.octets_utiles().max(1);
    let mut lus = 0u64;
    let mut tampon = vec![0u8; MORCEAU];
    for p in &bmap.plages {
        let (debut, longueur) = bmap.octets(p);
        cible
            .seek(SeekFrom::Start(debut))
            .map_err(err("positionnement"))?;
        let mut h = Sha256::new();
        let mut reste = longueur;
        while reste > 0 {
            controler(suivi)?;
            let t = (reste as usize).min(tampon.len());
            cible
                .read_exact(&mut tampon[..t])
                .map_err(err("relecture de la clé"))?;
            h.update(&tampon[..t]);
            reste -= t as u64;
            lus += t as u64;
            suivi.progres(
                lus as f32 / total as f32,
                &format!("{} vérifiés", taille_lisible(lus)),
            );
        }
        if hex::encode(h.finalize()) != *p.sha256.as_ref().unwrap() {
            return Err(format!(
                "La clé ne relit pas ce qui y a été écrit (blocs {}-{}). \
                 Elle est peut-être défectueuse ou mal branchée : essayez un autre port ou une autre clé.",
                p.debut, p.fin
            ));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bmap::Plage;
    use crate::suivi::Muet;

    fn image_test() -> Vec<u8> {
        // 16 blocs de 4096 ; blocs 0, 5-6 et 15 non nuls.
        let mut img = vec![0u8; 16 * 4096];
        for b in [0usize, 5, 6, 15] {
            for (i, o) in img[b * 4096..(b + 1) * 4096].iter_mut().enumerate() {
                *o = (b * 31 + i % 251) as u8 | 1;
            }
        }
        img
    }

    fn bmap_de(img: &[u8], plages: &[(u64, u64)]) -> Bmap {
        Bmap {
            taille_image: img.len() as u64,
            taille_bloc: 4096,
            plages: plages
                .iter()
                .map(|&(d, f)| Plage {
                    debut: d,
                    fin: f,
                    sha256: Some(hex::encode(Sha256::digest(
                        &img[d as usize * 4096..(f as usize + 1) * 4096],
                    ))),
                })
                .collect(),
        }
    }

    #[test]
    fn ecriture_zstd_avec_bmap_et_verification() {
        let dossier = std::env::temp_dir().join(format!("fabcle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dossier).unwrap();
        let img = image_test();
        let zst = dossier.join("t.img.zst");
        std::fs::write(&zst, zstd::encode_all(&img[..], 3).unwrap()).unwrap();
        let bmap = bmap_de(&img, &[(0, 0), (5, 6), (15, 15)]);

        // Cible plus grande que l'image et remplie de « vieilles données ».
        let chemin_cible = dossier.join("cle.bin");
        std::fs::write(&chemin_cible, vec![0xAAu8; 64 * 4096 + (1 << 20)]).unwrap();
        let mut cible = File::options()
            .read(true)
            .write(true)
            .open(&chemin_cible)
            .unwrap();

        let n = ecrire_image(
            Source::ouvrir(&zst).unwrap(),
            Some(&bmap),
            &mut cible,
            &Muet,
        )
        .unwrap();
        assert_eq!(n, 4 * 4096);
        assert!(verifier(&bmap, &mut cible, &Muet).unwrap());

        let cle = std::fs::read(&chemin_cible).unwrap();
        for b in [0usize, 5, 6, 15] {
            assert_eq!(cle[b * 4096..(b + 1) * 4096], img[b * 4096..(b + 1) * 4096]);
        }
        assert!(
            cle[4096..2 * 4096].iter().all(|&o| o == 0xAA),
            "bloc vide non écrit"
        );
        assert!(
            cle[cle.len() - (1 << 20)..].iter().all(|&o| o == 0),
            "fin de clé effacée"
        );

        // Une corruption est détectée.
        cible.seek(SeekFrom::Start(5 * 4096 + 10)).unwrap();
        cible.write_all(&[0]).unwrap();
        assert!(verifier(&bmap, &mut cible, &Muet).is_err());
        std::fs::remove_dir_all(&dossier).ok();
    }

    #[test]
    fn ecriture_brute_sans_bmap_et_cle_trop_petite() {
        let dossier = std::env::temp_dir().join(format!("fabcle-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dossier).unwrap();
        let img = image_test();
        let brut = dossier.join("t.img");
        std::fs::write(&brut, &img).unwrap();

        let chemin_cible = dossier.join("cle.bin");
        std::fs::write(&chemin_cible, vec![0xAAu8; img.len()]).unwrap();
        let mut cible = File::options()
            .read(true)
            .write(true)
            .open(&chemin_cible)
            .unwrap();
        ecrire_image(Source::ouvrir(&brut).unwrap(), None, &mut cible, &Muet).unwrap();
        assert_eq!(std::fs::read(&chemin_cible).unwrap(), img);

        let petite = dossier.join("petite.bin");
        std::fs::write(&petite, vec![0u8; 4096]).unwrap();
        let mut cible = File::options()
            .read(true)
            .write(true)
            .open(&petite)
            .unwrap();
        let bmap = bmap_de(&img, &[(0, 0)]);
        let e = ecrire_image(
            Source::ouvrir(&brut).unwrap(),
            Some(&bmap),
            &mut cible,
            &Muet,
        );
        assert!(e.unwrap_err().contains("trop petite"));
        std::fs::remove_dir_all(&dossier).ok();
    }
}
