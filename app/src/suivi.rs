//! Suivi d'un travail long : étapes, progression, annulation.
//! Implémenté par l'interface (canal vers la fenêtre) et par l'assistant Linux (lignes sur stdout).

pub trait Suivi: Sync {
    /// Début d'une nouvelle étape (« Téléchargement », « Écriture »…).
    fn etape(&self, texte: &str);
    /// Avancement de l'étape en cours, entre 0 et 1, avec un détail lisible.
    fn progres(&self, fraction: f32, detail: &str);
    /// Vrai si l'utilisateur a demandé l'arrêt.
    fn annule(&self) -> bool;
}

pub const ANNULE: &str = "Opération annulée.";

pub fn controler(s: &dyn Suivi) -> Result<(), String> {
    if s.annule() {
        Err(ANNULE.into())
    } else {
        Ok(())
    }
}

/// Taille lisible, en unités décimales comme les fabricants de clés (« 31,0 Go »).
pub fn taille_lisible(octets: u64) -> String {
    let o = octets as f64;
    let (v, u) = if o >= 1e12 {
        (o / 1e12, "To")
    } else if o >= 1e9 {
        (o / 1e9, "Go")
    } else if o >= 1e6 {
        (o / 1e6, "Mo")
    } else if o >= 1e3 {
        (o / 1e3, "ko")
    } else {
        return format!("{octets} o");
    };
    format!("{v:.1} {u}").replace('.', ",")
}

/// Suivi silencieux (tests).
#[cfg(test)]
pub struct Muet;
#[cfg(test)]
impl Suivi for Muet {
    fn etape(&self, _: &str) {}
    fn progres(&self, _: f32, _: &str) {}
    fn annule(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tailles() {
        assert_eq!(super::taille_lisible(31_042_043_904), "31,0 Go");
        assert_eq!(super::taille_lisible(512), "512 o");
    }
}
