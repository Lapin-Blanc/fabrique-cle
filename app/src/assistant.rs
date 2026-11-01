//! Linux : l'interface tourne avec les droits de l'utilisateur ; seule l'écriture sur la clé
//! demande root. L'application se relance alors elle-même, sans fenêtre, avec
//! `pkexec <application> --assistant` (fenêtre d'authentification du bureau).
//!
//! Protocole (une ligne par message, champs séparés par des tabulations) :
//! - entrée : une ligne JSON [`Ecriture`], puis l'entrée reste ouverte ; sa fermeture = annulation ;
//! - sortie : `ETAPE\ttexte`, `PROGRES\tfraction\tdétail`, `FINI\tmessage`, `ERREUR\tmessage`.

use crate::suivi::Suivi;
use crate::travail::{Ecriture, ecrire_sur_disque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const OPTION: &str = "--assistant";

fn une_ligne(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

/// Côté interface : lance l'assistant et relaie ses messages au suivi.
pub fn lancer(e: &Ecriture, suivi: &dyn Suivi) -> Result<String, String> {
    // Dans une AppImage, l'exécutable courant est dans un montage FUSE que root ne peut pas lire :
    // relancer l'AppImage elle-même.
    let exe = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or("chemin de l'application introuvable")?;
    suivi.etape("Autorisation administrateur (mot de passe du PC)");
    let mut enfant = Command::new("pkexec")
        .arg(&exe)
        .arg(OPTION)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|x| {
            format!(
                "pkexec introuvable ({x}) : lancez l'application avec « sudo » pour écrire la clé."
            )
        })?;

    let mut entree = enfant.stdin.take().unwrap();
    let json = serde_json::to_string(e).map_err(|x| x.to_string())?;
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

/// Côté root : point d'entrée de `--assistant`.
pub fn servir() -> i32 {
    let mut entree = BufReader::new(std::io::stdin());
    let mut json = String::new();
    if entree.read_line(&mut json).is_err() {
        return 2;
    }
    let e: Ecriture = match serde_json::from_str(&json) {
        Ok(e) => e,
        Err(x) => {
            println!("ERREUR\tdemande illisible : {x}");
            return 2;
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
    // `ecrire_sur_disque` revérifie que le disque est bien une clé USB branchée, hors système.
    match ecrire_sur_disque(&e, &suivi) {
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
