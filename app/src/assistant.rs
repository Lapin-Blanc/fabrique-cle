//! Linux : l'interface tourne avec les droits de l'utilisateur ; seule l'écriture sur la clé
//! demande root. L'application se relance alors elle-même, sans fenêtre, avec
//! `pkexec <application> --assistant` (fenêtre d'authentification du bureau). Sans pkexec (paquet
//! séparé sous Debian, souvent absent), `run0` de systemd (≥ 256), qui passe aussi par polkit.
//!
//! Trois usages : `--assistant` (écrire une clé), `--diagnostic` (mesures en lecture seule de
//! l'onglet Diagnostic, qui exigent aussi root pour lire le périphérique brut) et `--analyse`
//! (analyse complète, qui efface la clé).
//!
//! Protocole (une ligne par message, champs séparés par des tabulations) :
//! - entrée : une ligne JSON ([`Ecriture`], [`Disque`] ou demande d'analyse), puis l'entrée reste ouverte ;
//!   sa fermeture = annulation ;
//! - sortie : `ETAPE\ttexte`, `PROGRES\tfraction\tdétail`, `FINI\tmessage`, `ERREUR\tmessage`
//!   (pour le diagnostic et l'analyse, le message de `FINI` est le JSON des résultats).

use crate::diagnostic::analyse::{Analyse, Reglages, analyser_disque};
use crate::diagnostic::mesures::{Mesures, mesurer_disque};
use crate::disques::Disque;
use crate::suivi::Suivi;
use crate::travail::{Ecriture, ecrire_sur_disque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const OPTION: &str = "--assistant";
pub const OPTION_DIAGNOSTIC: &str = "--diagnostic";
pub const OPTION_ANALYSE: &str = "--analyse";

/// Demande d'analyse complète transmise à l'assistant.
#[derive(serde::Serialize, serde::Deserialize)]
struct DemandeAnalyse {
    disque: Disque,
    reglages: Reglages,
}

fn une_ligne(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

/// Côté interface : écriture d'une clé par l'assistant.
pub fn lancer(e: &Ecriture, suivi: &dyn Suivi) -> Result<String, String> {
    let json = serde_json::to_string(e).map_err(|x| x.to_string())?;
    executer(OPTION, &json, suivi)
}

/// Côté interface : mesures du diagnostic par l'assistant.
pub fn diagnostiquer(d: &Disque, suivi: &dyn Suivi) -> Result<Mesures, String> {
    let json = serde_json::to_string(d).map_err(|x| x.to_string())?;
    let r = executer(OPTION_DIAGNOSTIC, &json, suivi)?;
    serde_json::from_str(&r).map_err(|x| format!("réponse de l'assistant illisible : {x}"))
}

/// Programme qui relance l'application en root après authentification graphique (polkit) :
/// pkexec, sinon run0 (systemd ≥ 256, sans terminal : entrée et sortie passent par des tubes).
fn elevateur() -> Option<&'static str> {
    let chemins = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    ["pkexec", "run0"]
        .into_iter()
        .find(|p| std::env::split_paths(&chemins).any(|d| d.join(p).is_file()))
}

/// Côté interface : analyse complète par l'assistant (**efface la clé**).
pub fn analyser(d: &Disque, reglages: Reglages, suivi: &dyn Suivi) -> Result<Analyse, String> {
    let demande = DemandeAnalyse {
        disque: d.clone(),
        reglages,
    };
    let json = serde_json::to_string(&demande).map_err(|x| x.to_string())?;
    let r = executer(OPTION_ANALYSE, &json, suivi)?;
    serde_json::from_str(&r).map_err(|x| format!("réponse de l'assistant illisible : {x}"))
}

/// Lance l'assistant avec `option` et la demande `json`, et relaie ses messages au suivi.
fn executer(option: &str, json: &str, suivi: &dyn Suivi) -> Result<String, String> {
    // Dans une AppImage, l'exécutable courant est dans un montage FUSE que root ne peut pas lire :
    // relancer l'AppImage elle-même.
    let exe = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or("chemin de l'application introuvable")?;
    let elevateur = elevateur().ok_or(
        "Ni pkexec ni run0 ne sont installés : impossible d'obtenir les droits administrateur. \
         Installez le paquet « pkexec » (sudo apt install pkexec), ou lancez l'application avec sudo.",
    )?;
    suivi.etape("Autorisation administrateur (mot de passe du PC)");
    let mut enfant = Command::new(elevateur)
        .arg(&exe)
        .arg(option)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|x| format!("lancement de {elevateur} : {x}"))?;

    let mut entree = enfant.stdin.take().unwrap();
    writeln!(entree, "{json}").map_err(|x| format!("assistant : {x}"))?;

    // Surveillance de l'annulation : fermer l'entrée de l'assistant l'arrête proprement.
    let fini = Arc::new(AtomicBool::new(false));
    let annuler = Arc::new(AtomicBool::new(false));
    let garde = {
        let (fini, annuler) = (fini.clone(), annuler.clone());
        let entree = Mutex::new(Some(entree));
        std::thread::spawn(move || {
            while !fini.load(Ordering::Relaxed) {
                if annuler.load(Ordering::Relaxed) {
                    entree.lock().unwrap().take();
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };

    let mut resultat: Option<Result<String, String>> = None;
    let sortie = BufReader::new(enfant.stdout.take().unwrap());
    // Les lignes arrivent en continu ; l'annulation est relayée entre deux lignes.
    for ligne in sortie.lines() {
        let Ok(ligne) = ligne else { break };
        if suivi.annule() {
            annuler.store(true, Ordering::Relaxed);
        }
        let champs: Vec<&str> = ligne.splitn(3, '\t').collect();
        match champs.as_slice() {
            ["ETAPE", t] => suivi.etape(t),
            ["PROGRES", f, d] => suivi.progres(f.parse().unwrap_or(0.0), d),
            ["FINI", m] => resultat = Some(Ok(m.to_string())),
            ["ERREUR", m] => resultat = Some(Err(m.to_string())),
            _ => {}
        }
    }
    fini.store(true, Ordering::Relaxed);
    let _ = garde.join();
    let statut = enfant.wait().map_err(|x| format!("assistant : {x}"))?;
    let mut erreurs = String::new();
    if let Some(mut s) = enfant.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut s, &mut erreurs);
    }
    match (resultat, statut.code()) {
        (Some(r), _) => r,
        (None, Some(126)) | (None, Some(127)) => Err(
            "Autorisation administrateur refusée ou annulée : la clé n'a pas été modifiée.".into(),
        ),
        (None, _) => Err(format!(
            "L'assistant s'est arrêté sans résultat. {}",
            une_ligne(erreurs.trim())
        )),
    }
}

/// Suivi côté assistant : messages sur stdout, annulation quand l'entrée se ferme.
struct SuiviSortie {
    annule: Arc<AtomicBool>,
    dernier: Mutex<Instant>,
}

impl Suivi for SuiviSortie {
    fn etape(&self, texte: &str) {
        println!("ETAPE\t{}", une_ligne(texte));
    }
    fn progres(&self, fraction: f32, detail: &str) {
        // Au plus 10 messages par seconde.
        let mut d = self.dernier.lock().unwrap();
        if d.elapsed() >= Duration::from_millis(100) || fraction >= 1.0 {
            *d = Instant::now();
            println!("PROGRES\t{fraction}\t{}", une_ligne(detail));
        }
    }
    fn annule(&self) -> bool {
        self.annule.load(Ordering::Relaxed)
    }
}

/// Côté root : lit la demande, puis surveille la fermeture de l'entrée (annulation).
fn demande<T: serde::de::DeserializeOwned>() -> Result<(T, SuiviSortie), i32> {
    let mut entree = BufReader::new(std::io::stdin());
    let mut json = String::new();
    if entree.read_line(&mut json).is_err() {
        return Err(2);
    }
    let t: T = match serde_json::from_str(&json) {
        Ok(t) => t,
        Err(x) => {
            println!("ERREUR\tdemande illisible : {x}");
            return Err(2);
        }
    };
    let annule = Arc::new(AtomicBool::new(false));
    {
        let annule = annule.clone();
        std::thread::spawn(move || {
            // Fin de l'entrée (interface fermée ou bouton Annuler) : arrêter.
            let mut reste = String::new();
            while entree.read_line(&mut reste).map(|n| n > 0).unwrap_or(false) {
                reste.clear();
            }
            annule.store(true, Ordering::Relaxed);
        });
    }
    let suivi = SuiviSortie {
        annule,
        dernier: Mutex::new(Instant::now()),
    };
    Ok((t, suivi))
}

fn repondre(r: Result<String, String>) -> i32 {
    match r {
        Ok(m) => {
            println!("FINI\t{}", une_ligne(&m));
            0
        }
        Err(m) => {
            println!("ERREUR\t{}", une_ligne(&m));
            1
        }
    }
}

/// Côté root : point d'entrée de `--assistant`.
pub fn servir() -> i32 {
    let (e, suivi) = match demande::<Ecriture>() {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `ecrire_sur_disque` revérifie que le disque est bien une clé USB branchée, hors système.
    repondre(ecrire_sur_disque(&e, &suivi))
}

/// Côté root : point d'entrée de `--diagnostic` (lecture seule).
pub fn servir_diagnostic() -> i32 {
    let (d, suivi) = match demande::<Disque>() {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `mesurer_disque` revérifie que le disque est bien une clé USB branchée, hors système.
    repondre(
        mesurer_disque(&d, &suivi)
            .and_then(|m| serde_json::to_string(&m).map_err(|x| x.to_string())),
    )
}

/// Côté root : point d'entrée de `--analyse` (**efface la clé**).
pub fn servir_analyse() -> i32 {
    let (a, suivi) = match demande::<DemandeAnalyse>() {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `analyser_disque` passe par `disques::ouvrir` : mêmes vérifications que pour l'écriture.
    repondre(
        analyser_disque(&a.disque, a.reglages, &suivi)
            .and_then(|m| serde_json::to_string(&m).map_err(|x| x.to_string())),
    )
}
