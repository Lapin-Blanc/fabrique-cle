//! Écriture complète d'une clé : image, vérification, `config.txt`.
//! Sous Linux sans droits root, ce travail est confié à l'assistant (`assistant.rs`) via pkexec (ou run0).

use crate::disques::{self, Disque};
use crate::ecriture::{Source, ecrire_image, verifier};
use crate::esp;
use crate::suivi::{Suivi, taille_lisible};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Tout ce qu'il faut pour écrire une clé (transmis tel quel à l'assistant Linux, en JSON).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ecriture {
    pub disque: Disque,
    pub image: PathBuf,
    pub bmap: Option<PathBuf>,
    /// Contenu de `config.txt` (mot de passe déjà haché).
    pub config: String,
    pub verifier: bool,
}

/// Écrit la clé dans ce processus (droits administrateur nécessaires).
pub fn ecrire_sur_disque(e: &Ecriture, suivi: &dyn Suivi) -> Result<String, String> {
    let bmap = match &e.bmap {
        Some(p) => Some(crate::bmap::analyser(
            &std::fs::read_to_string(p).map_err(|x| format!("{} : {x}", p.display()))?,
        )?),
        None => None,
    };
    suivi.etape("Préparation de la clé (démontage)");
    let source = Source::ouvrir(&e.image)?;
    let mut cible = disques::ouvrir(&e.disque)?;

    suivi.etape("Écriture de l'image sur la clé");
    let ecrits = ecrire_image(source, bmap.as_ref(), cible.as_mut(), suivi)?;

    let mut verifiee = false;
    if let (true, Some(b)) = (e.verifier, &bmap) {
        suivi.etape("Vérification de la clé (relecture)");
        verifiee = verifier(b, cible.as_mut(), suivi)?;
    }

    suivi.etape("Personnalisation (config.txt)");
    esp::deposer_config(cible.as_mut(), &e.config)?;
    cible
        .synchroniser()
        .map_err(|x| format!("synchronisation : {x}"))?;
    cible
        .oublier_cache()
        .map_err(|x| format!("vidage du cache : {x}"))?;
    if esp::lire_config(cible.as_mut())? != e.config {
        return Err(
            "config.txt relu différent de ce qui a été écrit : la clé semble défectueuse.".into(),
        );
    }
    cible
        .terminer()
        .map_err(|x| format!("fin d'écriture : {x}"))?;
    suivi.progres(1.0, "");

    Ok(format!(
        "Clé prête : {} écrits{}. Vous pouvez la retirer et démarrer un PC dessus \
         (menu de démarrage : F9, F12, F11 ou Échap selon le fabricant).",
        taille_lisible(ecrits),
        if verifiee { " et vérifiés" } else { "" }
    ))
}

/// Écrit la clé, en passant par l'assistant privilégié si nécessaire.
pub fn ecrire(e: &Ecriture, suivi: &dyn Suivi) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY : geteuid n'a pas d'effet de bord.
        if unsafe { libc::geteuid() } != 0 {
            return crate::assistant::lancer(e, suivi);
        }
    }
    ecrire_sur_disque(e, suivi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suivi::Muet;
    use sha2::{Digest, Sha256};

    /// Parcours complet sur des fichiers : image GPT + FAT compressée, bmap, écriture,
    /// vérification, dépôt de config.txt — tout sauf l'accès à un vrai disque.
    #[test]
    fn parcours_complet_sur_fichier() {
        let dossier = std::env::temp_dir().join(format!("fabcle-parcours-{}", std::process::id()));
        std::fs::create_dir_all(&dossier).unwrap();
        let img = esp::disque_de_test().into_inner();
        let zst = dossier.join("fabrique-cle.img.zst");
        std::fs::write(&zst, zstd::encode_all(&img[..], 3).unwrap()).unwrap();

        // bmap : tous les blocs non nuls.
        let bloc = 4096usize;
        let mut plages = String::new();
        for (i, b) in img.chunks(bloc).enumerate() {
            if b.iter().any(|&o| o != 0) {
                plages += &format!(
                    "<Range chksum=\"{}\"> {i} </Range>\n",
                    hex::encode(Sha256::digest(b))
                );
            }
        }
        let xml = format!(
            "<bmap version=\"2.0\"><ImageSize> {} </ImageSize><BlockSize> {bloc} </BlockSize>\
             <ChecksumType> sha256 </ChecksumType><BlockMap>{plages}</BlockMap></bmap>",
            img.len()
        );
        let bmap = crate::bmap::analyser(&xml).unwrap();

        let cle = dossier.join("cle.bin");
        std::fs::write(&cle, vec![0x55u8; img.len() * 2]).unwrap();
        let mut f = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&cle)
            .unwrap();
        ecrire_image(Source::ouvrir(&zst).unwrap(), Some(&bmap), &mut f, &Muet).unwrap();
        assert!(verifier(&bmap, &mut f, &Muet).unwrap());
        esp::deposer_config(&mut f, "identifiant=lucas\n").unwrap();
        assert_eq!(esp::lire_config(&mut f).unwrap(), "identifiant=lucas\n");
        std::fs::remove_dir_all(&dossier).ok();
    }
}

/// Essai sur une vraie image (construite par `construire-image.sh` ou à la main) écrite dans un
/// fichier : `FABCLE_IMAGE=x.img.zst FABCLE_BMAP=x.img.bmap FABCLE_CIBLE=cle.bin cargo test -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn image_reelle_vers_fichier() {
    use crate::suivi::Muet;
    let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} manquant"));
    let bmap =
        crate::bmap::analyser(&std::fs::read_to_string(var("FABCLE_BMAP")).unwrap()).unwrap();
    let mut f = std::fs::File::options()
        .read(true)
        .write(true)
        .open(var("FABCLE_CIBLE"))
        .unwrap();
    ecrire_image(
        Source::ouvrir(std::path::Path::new(&var("FABCLE_IMAGE"))).unwrap(),
        Some(&bmap),
        &mut f,
        &Muet,
    )
    .unwrap();
    assert!(verifier(&bmap, &mut f, &Muet).unwrap());
    let p = crate::config::Personnalisation {
        identifiant: "lucas".into(),
        nom_complet: "Lucas Dupont-Hélène".into(),
        mot_de_passe: "secret123".into(),
        confirmation: "secret123".into(),
        ..Default::default()
    };
    esp::deposer_config(&mut f, &p.config_txt().unwrap()).unwrap();
}
