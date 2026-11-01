//! Versions publiées de l'image (releases GitHub) et téléchargement dans un cache local, avec
//! vérification SHA-256. Le cache est indexé par la somme SHA-256 : une image déjà téléchargée
//! sert pour plusieurs clés sans être retéléchargée.

use crate::suivi::{Suivi, controler, taille_lisible};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEPOT: &str = "Toune-Fabien/fabrique-cle";
const IMAGE: &str = "fabrique-cle.img.zst";

#[derive(Clone, Debug)]
pub struct Version {
    /// Étiquette (« v2026.11 ») ou « dernière » si l'API GitHub n'a pas répondu.
    pub etiquette: String,
    /// URL de l'image, ou de ses morceaux dans l'ordre (`.part00`, `.part01`…).
    pub morceaux: Vec<String>,
    pub sha256: String,
    pub bmap: Option<String>,
    /// Taille du téléchargement, si connue.
    pub taille: Option<u64>,
}

/// Image prête à écrire : fichier `.img.zst` vérifié et, si disponible, son `.bmap`.
#[derive(Clone, Debug)]
pub struct ImageLocale {
    pub image: PathBuf,
    pub bmap: Option<PathBuf>,
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        // Magasin de certificats du système : fonctionne derrière les proxys d'école qui
        // déchiffrent le HTTPS avec leur propre autorité installée sur les postes.
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(concat!("fabrique-cle/", env!("VERSION_APPLI")))
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_per_read(Some(Duration::from_secs(120)))
        .build()
        .into()
}

fn texte(url: &str) -> Result<String, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| format!("{url} : {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{url} : {e}"))
}

/// Extrait les versions utilisables de la réponse JSON de l'API GitHub.
pub fn analyser_releases(json: &str) -> Result<Vec<Version>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("réponse GitHub : {e}"))?;
    let mut res = Vec::new();
    for r in v.as_array().ok_or("réponse GitHub inattendue")? {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let etiquette = r["tag_name"].as_str().unwrap_or_default().to_string();
        let mut fichiers: Vec<(String, String, u64)> = r["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        (
                            x["name"].as_str().unwrap_or_default().to_string(),
                            x["browser_download_url"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            x["size"].as_u64().unwrap_or(0),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        fichiers.sort();
        let url = |nom: &str| fichiers.iter().find(|f| f.0 == nom).map(|f| f.1.clone());
        let Some(sha256) = url(&format!("{IMAGE}.sha256")) else {
            continue;
        };
        let entiers: Vec<_> = fichiers.iter().filter(|f| f.0 == IMAGE).collect();
        let parts: Vec<_> = fichiers
            .iter()
            .filter(|f| f.0.starts_with(&format!("{IMAGE}.part")))
            .collect();
        let choisis = if !entiers.is_empty() { entiers } else { parts };
        if choisis.is_empty() {
            continue;
        }
        res.push(Version {
            etiquette,
            taille: Some(choisis.iter().map(|f| f.2).sum()),
            morceaux: choisis.iter().map(|f| f.1.clone()).collect(),
            sha256,
            bmap: url("fabrique-cle.img.bmap"),
        });
    }
    Ok(res)
}

/// Sans l'API (quota de 60 requêtes/heure par adresse IP, vite atteint derrière le NAT d'une
/// école) : dernière version via les liens directs `releases/latest/download/…`.
fn derniere_sans_api() -> Result<Version, String> {
    let base = format!("https://github.com/{DEPOT}/releases/latest/download");
    let existe = |nom: &str| agent().head(&format!("{base}/{nom}")).call().is_ok();
    let sha256 = format!("{base}/{IMAGE}.sha256");
    if !existe(&format!("{IMAGE}.sha256")) {
        return Err("aucune version publiée trouvée".into());
    }
    let morceaux = if existe(IMAGE) {
        vec![format!("{base}/{IMAGE}")]
    } else {
        (0..100)
            .map(|i| format!("{IMAGE}.part{i:02}"))
            .take_while(|n| existe(n))
            .map(|n| format!("{base}/{n}"))
            .collect()
    };
    if morceaux.is_empty() {
        return Err("image introuvable dans la dernière version".into());
    }
    let bmap = existe("fabrique-cle.img.bmap").then(|| format!("{base}/fabrique-cle.img.bmap"));
    Ok(Version {
        etiquette: "dernière".into(),
        morceaux,
        sha256,
        bmap,
        taille: None,
    })
}

/// Versions publiées, la plus récente d'abord.
pub fn lister_versions() -> Result<Vec<Version>, String> {
    let api = format!("https://api.github.com/repos/{DEPOT}/releases?per_page=20");
    match texte(&api).and_then(|j| analyser_releases(&j)) {
        Ok(v) if !v.is_empty() => Ok(v),
        Ok(_) => Err("Aucune image publiée pour l'instant.".into()),
        Err(e_api) => derniere_sans_api()
            .map(|v| vec![v])
            .map_err(|e| format!("{e_api} ; {e}")),
    }
}

pub fn dossier_cache() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fabrique-cle")
}

/// Somme SHA-256 d'un fichier, avec progression.
pub fn sha256_fichier(chemin: &Path, suivi: &dyn Suivi) -> Result<String, String> {
    let mut f = File::open(chemin).map_err(|e| format!("{} : {e}", chemin.display()))?;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut lus = 0u64;
    loop {
        controler(suivi)?;
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{} : {e}", chemin.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        lus += n as u64;
        suivi.progres(
            lus as f32 / total as f32,
            &format!("{} vérifiés", taille_lisible(lus)),
        );
    }
    Ok(hex::encode(h.finalize()))
}

/// Première somme hexadécimale de 64 caractères d'un fichier `.sha256`.
pub fn lire_somme(contenu: &str) -> Option<String> {
    contenu
        .split_whitespace()
        .find(|m| m.len() == 64 && m.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase())
}

/// Télécharge (ou retrouve dans le cache) l'image d'une version, vérifiée par SHA-256.
pub fn telecharger(v: &Version, suivi: &dyn Suivi) -> Result<ImageLocale, String> {
    suivi.etape("Préparation du téléchargement");
    let attendu = lire_somme(&texte(&v.sha256)?).ok_or("fichier .sha256 illisible")?;
    let dossier = dossier_cache();
    fs::create_dir_all(&dossier).map_err(|e| format!("{} : {e}", dossier.display()))?;
    let image = dossier.join(format!("{}.img.zst", &attendu[..16]));
    let bmap = dossier.join(format!("{}.img.bmap", &attendu[..16]));

    if let Some(url) = &v.bmap {
        let xml = texte(url)?;
        crate::bmap::analyser(&xml)?;
        fs::write(&bmap, xml).map_err(|e| format!("{} : {e}", bmap.display()))?;
    }
    let bmap = v.bmap.as_ref().map(|_| bmap);

    if image.exists() {
        suivi.etape("Vérification de l'image déjà téléchargée");
        if sha256_fichier(&image, suivi)? == attendu {
            return Ok(ImageLocale { image, bmap });
        }
        let _ = fs::remove_file(&image);
    }

    suivi.etape(&format!("Téléchargement de l'image {}", v.etiquette));
    let partiel = image.with_extension("zst.partiel");
    let a = agent();
    // Par morceaux en parallèle, avec reprise ; sinon d'un bloc.
    match crate::telechargement::telecharger(
        &a,
        &v.morceaux,
        &partiel,
        crate::telechargement::Reglages::default(),
        suivi,
    ) {
        Ok(()) => {}
        Err(e) if e == crate::telechargement::NON_PRIS_EN_CHARGE => {
            let _ = fs::remove_file(format!("{}.etat", partiel.display()));
            telecharger_d_un_bloc(&a, v, &partiel, suivi)?
        }
        Err(e) => return Err(e),
    }
    suivi.etape("Vérification de l'image téléchargée (SHA-256)");
    if sha256_fichier(&partiel, suivi)? != attendu {
        let _ = fs::remove_file(&partiel);
        let _ = fs::remove_file(format!("{}.etat", partiel.display()));
        return Err("Image téléchargée corrompue (SHA-256 différent) : recommencez.".into());
    }
    fs::rename(&partiel, &image).map_err(|e| format!("{} : {e}", image.display()))?;
    nettoyer_cache(&dossier, &attendu[..16]);
    Ok(ImageLocale { image, bmap })
}

/// Téléchargement simple, d'un seul flux (serveur sans requêtes `Range`).
fn telecharger_d_un_bloc(
    a: &ureq::Agent,
    v: &Version,
    partiel: &Path,
    suivi: &dyn Suivi,
) -> Result<(), String> {
    let mut sortie = File::create(partiel).map_err(|e| format!("{} : {e}", partiel.display()))?;
    let mut recus = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    for url in &v.morceaux {
        let mut rep = a
            .get(url)
            .call()
            .map_err(|e| format!("téléchargement : {e}"))?;
        let mut lecteur = rep.body_mut().as_reader();
        loop {
            if suivi.annule() {
                return Err(crate::suivi::ANNULE.into());
            }
            let n = lecteur
                .read(&mut buf)
                .map_err(|e| format!("téléchargement interrompu : {e}"))?;
            if n == 0 {
                break;
            }
            sortie
                .write_all(&buf[..n])
                .map_err(|e| format!("écriture dans le cache : {e}"))?;
            recus += n as u64;
            let (fraction, detail) = match v.taille {
                Some(t) if t > 0 => (
                    recus as f32 / t as f32,
                    format!("{} / {}", taille_lisible(recus), taille_lisible(t)),
                ),
                _ => (0.0, taille_lisible(recus)),
            };
            suivi.progres(fraction, &detail);
        }
    }
    sortie
        .sync_all()
        .map_err(|e| format!("écriture dans le cache : {e}"))
}

/// Ne garder dans le cache que l'image qui vient d'être téléchargée (une image = ~2 Go).
fn nettoyer_cache(dossier: &Path, garder: &str) {
    let Ok(it) = fs::read_dir(dossier) else {
        return;
    };
    for e in it.flatten() {
        let nom = e.file_name().to_string_lossy().into_owned();
        if !nom.starts_with(garder) && nom.contains(".img.") {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Image choisie sur le disque : `.bmap` et `.sha256` cherchés à côté (facultatifs).
pub fn image_locale(chemin: &Path, suivi: &dyn Suivi) -> Result<ImageLocale, String> {
    if !chemin.is_file() {
        return Err(format!("Fichier introuvable : {}", chemin.display()));
    }
    let nom = chemin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let racine = nom.strip_suffix(".zst").unwrap_or(&nom);
    let racine = racine.strip_suffix(".img").unwrap_or(racine);
    let voisin = |suffixe: &str| chemin.with_file_name(format!("{racine}{suffixe}"));
    let bmap = Some(voisin(".img.bmap")).filter(|p| p.is_file());
    if let Some(b) = &bmap {
        let xml = fs::read_to_string(b).map_err(|e| format!("{} : {e}", b.display()))?;
        crate::bmap::analyser(&xml)?;
    }
    let somme = chemin.with_file_name(format!("{nom}.sha256"));
    if let Ok(c) = fs::read_to_string(&somme) {
        suivi.etape("Vérification de l'image (SHA-256)");
        let attendu = lire_somme(&c).ok_or("fichier .sha256 illisible")?;
        if sha256_fichier(chemin, suivi)? != attendu {
            return Err("L'image ne correspond pas à son fichier .sha256 (corrompue ?).".into());
        }
    }
    Ok(ImageLocale {
        image: chemin.to_path_buf(),
        bmap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_entieres_ou_en_morceaux() {
        let json = r#"[
          {"tag_name":"v2026.12","draft":false,"assets":[
            {"name":"fabrique-cle.img.zst.part01","browser_download_url":"https://x/p1","size":5},
            {"name":"fabrique-cle.img.zst.part00","browser_download_url":"https://x/p0","size":10},
            {"name":"fabrique-cle.img.zst.sha256","browser_download_url":"https://x/s","size":1},
            {"name":"fabrique-cle.img.bmap","browser_download_url":"https://x/b","size":1}]},
          {"tag_name":"v2026.11","draft":false,"assets":[
            {"name":"fabrique-cle.img.zst","browser_download_url":"https://y/i","size":7},
            {"name":"fabrique-cle.img.zst.sha256","browser_download_url":"https://y/s","size":1}]},
          {"tag_name":"brouillon","draft":true,"assets":[]},
          {"tag_name":"v-app-seule","draft":false,"assets":[
            {"name":"fabrique-cle.exe","browser_download_url":"https://z/e","size":1}]}
        ]"#;
        let v = analyser_releases(json).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].morceaux, vec!["https://x/p0", "https://x/p1"]);
        assert_eq!(v[0].taille, Some(15));
        assert_eq!(v[0].bmap.as_deref(), Some("https://x/b"));
        assert_eq!(v[1].morceaux, vec!["https://y/i"]);
        assert_eq!(v[1].bmap, None);
    }

    #[test]
    fn somme() {
        let s = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855  fabrique-cle.img.zst\n";
        assert_eq!(
            lire_somme(s).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(lire_somme("rien").is_none());
    }
}
