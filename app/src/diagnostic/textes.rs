//! Textes explicatifs de l'onglet Diagnostic, tirés de `ressources/diagnostic/textes.txt`
//! (modifiable sans toucher au code ; format décrit en tête du fichier).

use std::collections::HashMap;
use std::sync::OnceLock;

const SOURCE: &str = include_str!("../../ressources/diagnostic/textes.txt");

fn analyser(source: &'static str) -> HashMap<&'static str, String> {
    let mut textes = HashMap::new();
    let mut cle: Option<&'static str> = None;
    let mut corps = String::new();
    let mut ranger = |cle: Option<&'static str>, corps: &mut String| {
        if let Some(c) = cle {
            textes.insert(c, std::mem::take(corps).trim().to_string());
        }
        corps.clear();
    };
    for ligne in source.lines() {
        let l = ligne.trim_end();
        if l.starts_with('#') {
            continue;
        }
        if let Some(c) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            ranger(cle, &mut corps);
            cle = Some(c.trim());
        } else if l.trim().is_empty() {
            corps.push_str("\n\n");
        } else {
            // Lignes d'un même paragraphe : réunies par une espace (le texte est replié à l'écran).
            if !corps.is_empty() && !corps.ends_with("\n\n") {
                corps.push(' ');
            }
            corps.push_str(l.trim());
        }
    }
    ranger(cle, &mut corps);
    // Paragraphes : au plus une ligne vide entre deux.
    textes
        .into_iter()
        .map(|(c, t)| {
            let paragraphes: Vec<&str> = t
                .split("\n\n")
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect();
            (c, paragraphes.join("\n\n"))
        })
        .collect()
}

/// Texte d'une clé (`"protocole"`, `"protocole.pourquoi"`…) ; vide si absent.
pub fn texte(cle: &str) -> &'static str {
    static TEXTES: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    TEXTES
        .get_or_init(|| analyser(SOURCE))
        .get(cle)
        .map(String::as_str)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyse_du_format() {
        let t = analyser("# commentaire\n[a]\nune\nligne\n\nautre\n\n\n[a.pourquoi]\nparce que\n");
        assert_eq!(t["a"], "une ligne\n\nautre");
        assert_eq!(t["a.pourquoi"], "parce que");
    }

    /// Toute clé utilisée par l'application existe dans le fichier de textes.
    #[test]
    fn tous_les_textes_existent() {
        for c in ["introduction", "certitudes", "categories"] {
            assert!(!texte(c).is_empty(), "texte [{c}] manquant");
        }
        for c in super::super::interpretation::CLES {
            assert!(!texte(c).is_empty(), "texte [{c}] manquant");
            let p = format!("{c}.pourquoi");
            assert!(!texte(&p).is_empty(), "texte [{p}] manquant");
        }
    }
}
