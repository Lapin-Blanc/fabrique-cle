//! Téléchargement par morceaux, en parallèle (comme aria2c) et avec reprise après coupure.
//!
//! Le fichier logique est la concaténation des URL données (image entière ou `.part00`, `.part01`…).
//! Il est découpé en morceaux de quelques Mio, demandés en parallèle par requêtes HTTP `Range` et
//! écrits directement à leur place dans le fichier de destination. Chaque morceau terminé est noté
//! dans un fichier d'état (`….etat`) : après une coupure (Wi-Fi, mise en veille, fermeture), seuls
//! les morceaux manquants sont redemandés. L'intégrité finale est vérifiée par SHA-256 par l'appelant.

use crate::suivi::{ANNULE, Suivi, taille_lisible};
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Réglages (modifiables pour les tests).
#[derive(Clone, Copy)]
pub struct Reglages {
    pub connexions: usize,
    pub taille_morceau: u64,
    pub essais: u32,
}

impl Default for Reglages {
    fn default() -> Self {
        Self {
            connexions: 8,
            taille_morceau: 8 << 20,
            essais: 4,
        }
    }
}

/// Le téléchargement par morceaux n'est pas possible (serveur, proxy) : télécharger d'un bloc.
pub const NON_PRIS_EN_CHARGE: &str = "téléchargement par morceaux non pris en charge";

#[derive(Clone, Copy, Debug, PartialEq)]
struct Morceau {
    source: usize,
    debut_source: u64,
    debut: u64,
    longueur: u64,
}

/// Écriture à une position donnée, sans déplacer de curseur partagé entre fils.
fn ecrire_a(f: &File, buf: &[u8], position: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.write_all_at(buf, position)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut fait = 0;
        while fait < buf.len() {
            let n = f.seek_write(&buf[fait..], position + fait as u64)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "écriture nulle"));
            }
            fait += n;
        }
        Ok(())
    }
}

/// Taille d'une source, et preuve que le serveur accepte les requêtes `Range`
/// (réponse 206 avec `Content-Range: bytes 0-0/TOTAL`). `None` si non pris en charge.
fn sonder(agent: &ureq::Agent, url: &str) -> Result<Option<u64>, String> {
    let rep = agent
        .get(url)
        .header("Range", "bytes=0-0")
        .call()
        .map_err(|e| format!("{url} : {e}"))?;
    if rep.status() != 206 {
        return Ok(None);
    }
    Ok(rep
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit('/').next())
        .and_then(|t| t.trim().parse().ok()))
}

fn decouper(tailles: &[u64], taille_morceau: u64) -> Vec<Morceau> {
    let mut res = Vec::new();
    let mut global = 0;
    for (source, &taille) in tailles.iter().enumerate() {
        let mut d = 0;
        while d < taille {
            let longueur = taille_morceau.min(taille - d);
            res.push(Morceau {
                source,
                debut_source: d,
                debut: global + d,
                longueur,
            });
            d += longueur;
        }
        global += taille;
    }
    res
}

/// Fichier d'état : une ligne d'en-tête (tailles des sources et taille des morceaux), puis le
/// numéro de chaque morceau terminé. Un en-tête différent = on repart de zéro.
fn lire_etat(chemin: &Path, entete: &str) -> HashSet<usize> {
    let Ok(f) = File::open(chemin) else {
        return HashSet::new();
    };
    let mut lignes = io::BufReader::new(f).lines();
    if lignes.next().and_then(|l| l.ok()).as_deref() != Some(entete) {
        return HashSet::new();
    }
    lignes
        .map_while(|l| l.ok())
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Télécharge un morceau ; retourne une erreur si la connexion échoue ou si la réponse n'a pas
/// exactement la longueur attendue.
fn un_morceau(
    agent: &ureq::Agent,
    url: &str,
    m: &Morceau,
    f: &File,
    recus: &AtomicU64,
    arret: &AtomicBool,
) -> Result<(), (String, u64)> {
    let mut fait = 0u64;
    let r = (|| {
        let fin = m.debut_source + m.longueur - 1;
        let mut rep = agent
            .get(url)
            .header("Range", &format!("bytes={}-{fin}", m.debut_source))
            .call()
            .map_err(|e| e.to_string())?;
        if rep.status() != 206 {
            return Err(format!("réponse {} au lieu de 206", rep.status()));
        }
        let mut lecteur = rep.body_mut().as_reader();
        let mut buf = vec![0u8; 256 << 10];
        while fait < m.longueur {
            if arret.load(Ordering::Relaxed) {
                return Err(ANNULE.to_string());
            }
            let voulu = buf.len().min((m.longueur - fait) as usize);
            let n = lecteur.read(&mut buf[..voulu]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("connexion fermée avant la fin du morceau".into());
            }
            ecrire_a(f, &buf[..n], m.debut + fait)
                .map_err(|e| format!("écriture dans le cache : {e}"))?;
            fait += n as u64;
            recus.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(())
    })();
    r.map_err(|e| (e, fait))
}

/// Télécharge la concaténation de `urls` dans `destination`, en reprenant un téléchargement
/// interrompu s'il en existe un. Erreur [`NON_PRIS_EN_CHARGE`] si le serveur ne sert pas de morceaux.
pub fn telecharger(
    agent: &ureq::Agent,
    urls: &[String],
    destination: &Path,
    reglages: Reglages,
    suivi: &dyn Suivi,
) -> Result<(), String> {
    let mut tailles = Vec::new();
    for u in urls {
        // Sonde en échec ou sans réponse 206 : l'appelant se rabat sur un téléchargement simple.
        match sonder(agent, u) {
            Ok(Some(t)) => tailles.push(t),
            _ => return Err(NON_PRIS_EN_CHARGE.into()),
        }
    }
    let total: u64 = tailles.iter().sum();
    let morceaux = decouper(&tailles, reglages.taille_morceau);
    let entete = format!("fabrique-cle 1 {} {:?}", reglages.taille_morceau, tailles);
    let chemin_etat = PathBuf::from(format!("{}.etat", destination.display()));

    let mut faits = lire_etat(&chemin_etat, &entete);
    let reprise = !faits.is_empty()
        && fs::metadata(destination)
            .map(|m| m.len() == total)
            .unwrap_or(false);
    if !reprise {
        faits.clear();
        let mut e =
            File::create(&chemin_etat).map_err(|e| format!("{} : {e}", chemin_etat.display()))?;
        writeln!(e, "{entete}").map_err(|e| format!("{} : {e}", chemin_etat.display()))?;
    }
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(destination)
        .map_err(|e| format!("{} : {e}", destination.display()))?;
    f.set_len(total)
        .map_err(|e| format!("{} : {e}", destination.display()))?;

    let deja: u64 = morceaux
        .iter()
        .enumerate()
        .filter(|(i, _)| faits.contains(i))
        .map(|(_, m)| m.longueur)
        .sum();
    let file: Mutex<VecDeque<(usize, Morceau)>> = Mutex::new(
        morceaux
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| !faits.contains(i))
            .collect(),
    );
    let etat = Mutex::new(
        OpenOptions::new()
            .append(true)
            .open(&chemin_etat)
            .map_err(|e| format!("{} : {e}", chemin_etat.display()))?,
    );
    let recus = AtomicU64::new(deja);
    let arret = AtomicBool::new(false);
    let erreur: Mutex<Option<String>> = Mutex::new(None);
    let reussis = AtomicUsize::new(0);
    if reprise {
        suivi.etape(&format!(
            "Reprise du téléchargement ({} déjà reçus)",
            taille_lisible(deja)
        ));
    }

    let actifs = AtomicUsize::new(reglages.connexions.max(1));
    std::thread::scope(|s| {
        for _ in 0..reglages.connexions.max(1) {
            s.spawn(|| {
                // Quoi qu'il arrive, signaler la fin de cette connexion au fil principal.
                struct Fin<'a>(&'a AtomicUsize);
                impl Drop for Fin<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _fin = Fin(&actifs);
                loop {
                    if arret.load(Ordering::Relaxed) {
                        return;
                    }
                    let Some((i, m)) = file.lock().unwrap().pop_front() else {
                        return;
                    };
                    let mut essai = 0;
                    loop {
                        match un_morceau(agent, &urls[m.source], &m, &f, &recus, &arret) {
                            Ok(()) => {
                                let mut e = etat.lock().unwrap();
                                let _ = writeln!(e, "{i}");
                                reussis.fetch_add(1, Ordering::Relaxed);
                                break;
                            }
                            Err((x, fait)) => {
                                // Les octets de l'essai raté seront redemandés : ne plus les compter.
                                recus.fetch_sub(fait, Ordering::Relaxed);
                                essai += 1;
                                if x == ANNULE || essai >= reglages.essais {
                                    erreur.lock().unwrap().get_or_insert(x);
                                    arret.store(true, Ordering::Relaxed);
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(500 * essai as u64));
                            }
                        }
                    }
                }
            });
        }
        // Fil principal : progression et annulation.
        let debut = Instant::now();
        while actifs.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(200));
            if suivi.annule() {
                arret.store(true, Ordering::Relaxed);
            }
            let r = recus.load(Ordering::Relaxed).min(total);
            let ecoule = debut.elapsed().as_secs_f64().max(0.5);
            let debit = (r.saturating_sub(deja)) as f64 / ecoule;
            suivi.progres(
                r as f32 / total.max(1) as f32,
                &format!(
                    "{} / {}  ·  {}/s",
                    taille_lisible(r),
                    taille_lisible(total),
                    taille_lisible(debit as u64)
                ),
            );
        }
    });

    if let Some(e) = erreur.into_inner().unwrap() {
        // Aucun morceau obtenu : les requêtes par morceaux ne passent pas (proxy, pare-feu…) ;
        // l'appelant se rabat sur un téléchargement simple.
        if e != ANNULE && reussis.load(Ordering::Relaxed) == 0 && !reprise {
            return Err(NON_PRIS_EN_CHARGE.into());
        }
        return Err(if e == ANNULE {
            ANNULE.into()
        } else {
            format!("Téléchargement interrompu ({e}). Relancez : il reprendra où il s'est arrêté.")
        });
    }
    f.sync_all()
        .map_err(|e| format!("écriture dans le cache : {e}"))?;
    let _ = fs::remove_file(&chemin_etat);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suivi::Muet;
    use std::io::{BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// Mini-serveur HTTP : sert `donnees` (avec ou sans `Range`) ; coupe la connexion au milieu
    /// des réponses dont le numéro est dans `pannes`. Compte les requêtes reçues.
    fn serveur(
        donnees: Arc<Vec<u8>>,
        range: bool,
        pannes: Vec<usize>,
    ) -> (String, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let adresse = format!("http://{}/fichier", l.local_addr().unwrap());
        let compte = Arc::new(AtomicUsize::new(0));
        let c = compte.clone();
        std::thread::spawn(move || {
            for flux in l.incoming().flatten() {
                let (donnees, pannes, c) = (donnees.clone(), pannes.clone(), c.clone());
                std::thread::spawn(move || {
                    let mut lecteur = BufReader::new(flux.try_clone().unwrap());
                    let mut flux = flux;
                    loop {
                        let mut plage = None;
                        let mut ligne = String::new();
                        if lecteur.read_line(&mut ligne).unwrap_or(0) == 0 {
                            return;
                        }
                        loop {
                            let mut h = String::new();
                            if lecteur.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                                break;
                            }
                            if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                                let (a, b) = v.trim().split_once('-').unwrap();
                                plage = Some((
                                    a.parse::<usize>().unwrap(),
                                    b.parse::<usize>().unwrap(),
                                ));
                            }
                        }
                        let n = c.fetch_add(1, Ordering::SeqCst);
                        let (code, corps, entete) = match plage.filter(|_| range) {
                            Some((a, b)) => (
                                "206 Partial Content",
                                &donnees[a..=b],
                                format!("Content-Range: bytes {a}-{b}/{}\r\n", donnees.len()),
                            ),
                            None => ("200 OK", &donnees[..], String::new()),
                        };
                        let _ = write!(
                            flux,
                            "HTTP/1.1 {code}\r\nContent-Length: {}\r\n{entete}\r\n",
                            corps.len()
                        );
                        if pannes.contains(&n) {
                            let _ = flux.write_all(&corps[..corps.len() / 2]);
                            return; // coupure au milieu de la réponse
                        }
                        let _ = flux.write_all(corps);
                    }
                });
            }
        });
        (adresse, compte)
    }

    fn donnees(n: usize) -> Arc<Vec<u8>> {
        Arc::new((0..n).map(|i| (i * 7 + i / 251) as u8).collect())
    }

    fn reglages() -> Reglages {
        Reglages {
            connexions: 4,
            taille_morceau: 64 << 10,
            essais: 3,
        }
    }

    fn dossier(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fabcle-dl-{nom}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn decoupage_par_source() {
        let m = decouper(&[10, 25], 10);
        assert_eq!(m.len(), 4);
        assert_eq!(
            m[1],
            Morceau {
                source: 1,
                debut_source: 0,
                debut: 10,
                longueur: 10
            }
        );
        assert_eq!(
            m[3],
            Morceau {
                source: 1,
                debut_source: 20,
                debut: 30,
                longueur: 5
            }
        );
    }

    #[test]
    fn parallele_avec_pannes_puis_identique() {
        let d = donnees(1_000_000 + 123);
        // Coupures au milieu de plusieurs réponses : les morceaux sont redemandés.
        let (url, _) = serveur(d.clone(), true, vec![3, 7, 8]);
        let dest = dossier("pannes").join("img");
        telecharger(
            &ureq::Agent::new_with_defaults(),
            &[url],
            &dest,
            reglages(),
            &Muet,
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), *d);
        assert!(!PathBuf::from(format!("{}.etat", dest.display())).exists());
    }

    #[test]
    fn plusieurs_sources_concatenees() {
        let (a, b) = (donnees(300_000), Arc::new(vec![9u8; 200_001]));
        let (ua, _) = serveur(a.clone(), true, vec![]);
        let (ub, _) = serveur(b.clone(), true, vec![]);
        let dest = dossier("sources").join("img");
        telecharger(
            &ureq::Agent::new_with_defaults(),
            &[ua, ub],
            &dest,
            reglages(),
            &Muet,
        )
        .unwrap();
        let mut attendu = (*a).clone();
        attendu.extend_from_slice(&b);
        assert_eq!(fs::read(&dest).unwrap(), attendu);
    }

    #[test]
    fn reprise_apres_echec() {
        let d = donnees(1_000_000);
        let dest = dossier("reprise").join("img");
        // 1er essai : un seul essai par morceau et des pannes → échec, avec une partie reçue.
        let (url, _) = serveur(d.clone(), true, vec![6]);
        let r = Reglages {
            essais: 1,
            connexions: 1,
            ..reglages()
        };
        assert!(telecharger(&ureq::Agent::new_with_defaults(), &[url], &dest, r, &Muet).is_err());
        let etat = PathBuf::from(format!("{}.etat", dest.display()));
        assert!(etat.exists());
        // 2e essai sur un serveur sain : seuls les morceaux manquants sont redemandés.
        let (url, compte) = serveur(d.clone(), true, vec![]);
        telecharger(
            &ureq::Agent::new_with_defaults(),
            &[url],
            &dest,
            reglages(),
            &Muet,
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), *d);
        let total_morceaux = 1_000_000usize.div_ceil(64 << 10);
        let requetes = compte.load(Ordering::SeqCst);
        assert!(
            requetes < total_morceaux,
            "{requetes} requêtes pour {total_morceaux} morceaux"
        );
    }

    #[test]
    fn morceaux_tous_en_echec_repli() {
        // La sonde passe, puis chaque morceau est coupé (proxy capricieux) : repli demandé.
        let (url, _) = serveur(donnees(500_000), true, (1..1000).collect());
        let dest = dossier("repli").join("img");
        let e = telecharger(&ureq::Agent::new_with_defaults(), &[url], &dest, reglages(), &Muet);
        assert_eq!(e.unwrap_err(), NON_PRIS_EN_CHARGE);
    }

    #[test]
    fn serveur_sans_morceaux() {
        let (url, _) = serveur(donnees(1000), false, vec![]);
        let dest = dossier("sans").join("img");
        let e = telecharger(
            &ureq::Agent::new_with_defaults(),
            &[url],
            &dest,
            reglages(),
            &Muet,
        );
        assert_eq!(e.unwrap_err(), NON_PRIS_EN_CHARGE);
    }
}

/// Essai réel (réseau) : un morceau du vrai fichier de la release, à travers la redirection
/// GitHub → CDN. `cargo test morceau_reel -- --ignored`
#[cfg(test)]
#[test]
#[ignore]
fn morceau_reel_github() {
    let url = "https://github.com/Toune-Fabien/fabrique-cle/releases/download/v2026.11/fabrique-cle.img.zst";
    let a = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent();
    let total = sonder(&a, url).unwrap().expect("Range non pris en charge");
    assert!(total > 1_000_000_000, "taille {total}");
    let dest = std::env::temp_dir().join(format!("fabcle-reel-{}", std::process::id()));
    let f = File::create(&dest).unwrap();
    f.set_len(3 << 20).unwrap();
    let m = Morceau {
        source: 0,
        debut_source: 1 << 20,
        debut: 0,
        longueur: 2 << 20,
    };
    un_morceau(&a, url, &m, &f, &AtomicU64::new(0), &AtomicBool::new(false)).unwrap();
    let recu = fs::read(&dest).unwrap();
    let attendu = std::process::Command::new("curl")
        .args(["-sSL", "-r", &format!("{}-{}", 1 << 20, (3 << 20) - 1), url])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(recu[..2 << 20], attendu[..]);
    let _ = fs::remove_file(&dest);
}
