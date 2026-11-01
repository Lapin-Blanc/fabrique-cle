//! Personnalisation de la clé et fichier `config.txt` (contrat avec l'image, voir
//! `image/config-exemple.txt`). Les règles de validation sont celles du service de premier
//! démarrage (`fabrique-cle-premier-demarrage`) : une valeur refusée ici le serait là-bas.

use sha_crypt::{PasswordHasher, ShaCrypt};

/// Dispositions de clavier acceptées par l'image (code, libellé).
pub const CLAVIERS: [(&str, &str); 4] = [
    ("be", "Belge (AZERTY)"),
    ("fr", "Français (AZERTY)"),
    ("ch", "Suisse (QWERTZ)"),
    ("us", "Américain (QWERTY)"),
];

/// Langues acceptées par l'image (locale, libellé).
pub const LANGUES: [(&str, &str); 5] = [
    ("fr_BE.UTF-8", "Français (Belgique)"),
    ("fr_FR.UTF-8", "Français (France)"),
    ("fr_CH.UTF-8", "Français (Suisse)"),
    ("nl_BE.UTF-8", "Nederlands (België)"),
    ("en_US.UTF-8", "English (US)"),
];

/// Longueur minimale du mot de passe (même valeur que l'image).
pub const MOT_DE_PASSE_MIN: usize = 6;

/// Comptes déjà présents dans l'image : l'identifiant de l'utilisateur ne peut pas les reprendre.
const COMPTES_RESERVES: &[&str] = &[
    "root",
    "daemon",
    "bin",
    "sys",
    "sync",
    "games",
    "man",
    "lp",
    "mail",
    "news",
    "uucp",
    "proxy",
    "www-data",
    "backup",
    "list",
    "irc",
    "nobody",
    "messagebus",
    "sddm",
    "polkitd",
    "avahi",
    "rtkit",
    "colord",
    "pulse",
    "geoclue",
    "saned",
    "usbmux",
    "dnsmasq",
    "tss",
    "cups-pk-helper",
    "speech-dispatcher",
    "sshd",
    "admin",
    "sudo",
    "users",
];

#[derive(Clone, Debug)]
pub struct Personnalisation {
    pub identifiant: String,
    pub nom_complet: String,
    pub mot_de_passe: String,
    pub confirmation: String,
    pub machine: String,
    /// Indice dans [`CLAVIERS`].
    pub clavier: usize,
    /// Indice dans [`LANGUES`].
    pub langue: usize,
    pub chiffrer: bool,
}

impl Default for Personnalisation {
    fn default() -> Self {
        Self {
            identifiant: String::new(),
            nom_complet: String::new(),
            mot_de_passe: String::new(),
            confirmation: String::new(),
            machine: String::new(),
            clavier: 0,
            langue: 0,
            chiffrer: true,
        }
    }
}

/// `^[a-z][a-z0-9_-]{0,31}$`
pub fn identifiant_valide(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('a'..='z'))
        && s.len() <= 32
        && c.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
        && !COMPTES_RESERVES.contains(&s)
        && !s.starts_with("systemd-")
        && !s.starts_with('_')
}

/// `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`
pub fn machine_valide(s: &str) -> bool {
    let alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    !s.is_empty()
        && s.len() <= 63
        && s.chars().all(|c| alnum(c) || c == '-')
        && s.chars().next().is_some_and(alnum)
        && s.chars().last().is_some_and(alnum)
}

/// Nom de machine proposé à partir de l'identifiant (« cle-lucas »).
pub fn machine_par_defaut(identifiant: &str) -> String {
    let base: String = identifiant
        .chars()
        .map(|c| if c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    let s = format!("cle-{base}");
    let s: String = s.chars().take(63).collect();
    s.trim_end_matches('-').to_string()
}

impl Personnalisation {
    /// Machine effective : la valeur saisie, sinon celle proposée d'après l'identifiant.
    pub fn machine_effective(&self) -> String {
        if self.machine.trim().is_empty() {
            machine_par_defaut(self.identifiant.trim())
        } else {
            self.machine.trim().to_string()
        }
    }

    /// Liste des problèmes à corriger avant de pouvoir écrire la clé (vide = tout va bien).
    pub fn erreurs(&self) -> Vec<String> {
        let mut e = Vec::new();
        let id = self.identifiant.trim();
        if id.is_empty() {
            e.push("Identifiant : obligatoire.".into());
        } else if !identifiant_valide(id) {
            e.push(
                "Identifiant : une lettre minuscule puis minuscules, chiffres, - ou _ \
                 (32 caractères au plus, pas un nom réservé comme « root »)."
                    .into(),
            );
        }
        if self
            .nom_complet
            .chars()
            .any(|c| c.is_control() || c == ':' || c == ',')
        {
            e.push("Nom complet : pas de « : », de « , » ni de caractère de contrôle.".into());
        }
        if !machine_valide(&self.machine_effective()) {
            e.push(
                "Nom de la machine : minuscules, chiffres et -, sans - au début ni à la fin \
                 (63 caractères au plus)."
                    .into(),
            );
        }
        if !self.mot_de_passe.is_empty() || !self.confirmation.is_empty() {
            if self.mot_de_passe.chars().count() < MOT_DE_PASSE_MIN {
                e.push(format!(
                    "Mot de passe : au moins {MOT_DE_PASSE_MIN} caractères (ou laisser vide : \
                     il sera demandé au premier démarrage)."
                ));
            } else if self.mot_de_passe != self.confirmation {
                e.push("Mot de passe : les deux saisies diffèrent.".into());
            }
        }
        e
    }

    /// Contenu de `config.txt`. Le mot de passe est haché ici (SHA-512 crypt) : il n'est jamais
    /// écrit en clair sur la clé.
    pub fn config_txt(&self) -> Result<String, String> {
        let erreurs = self.erreurs();
        if !erreurs.is_empty() {
            return Err(erreurs.join("\n"));
        }
        let mut s = format!(
            "# Fabrique-Clé {} — lu au premier démarrage, puis effacé.\n",
            env!("VERSION_APPLI")
        );
        s += &format!("identifiant={}\n", self.identifiant.trim());
        let nom = self.nom_complet.trim();
        if !nom.is_empty() {
            s += &format!("nom_complet={nom}\n");
        }
        if !self.mot_de_passe.is_empty() {
            s += &format!("mot_de_passe_hache={}\n", hacher(&self.mot_de_passe)?);
        }
        s += &format!("machine={}\n", self.machine_effective());
        s += &format!("clavier={}\n", CLAVIERS[self.clavier].0);
        s += &format!("langue={}\n", LANGUES[self.langue].0);
        s += &format!("chiffrer={}\n", if self.chiffrer { "oui" } else { "non" });
        Ok(s)
    }
}

/// Hachage SHA-512 crypt (`$6$…`), sel aléatoire, accepté par `useradd -p` / `chpasswd -e`.
///
/// Le sel fait 12 octets, soit 16 caractères une fois encodé : la glibc tronque tout sel plus
/// long à 16 caractères, et le hachage stocké ne correspondrait alors plus jamais (connexion
/// impossible). Vérifié avec `crypt(3)` de la glibc.
pub fn hacher(mot_de_passe: &str) -> Result<String, String> {
    let mut sel = [0u8; 12];
    getrandom::fill(&mut sel).map_err(|e| format!("générateur aléatoire : {e}"))?;
    ShaCrypt::SHA512
        .hash_password_with_salt(mot_de_passe.as_bytes(), &sel)
        .map(|h| h.to_string())
        .map_err(|e| format!("hachage du mot de passe : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha_crypt::PasswordVerifier;

    fn exemple() -> Personnalisation {
        Personnalisation {
            identifiant: "lucas".into(),
            nom_complet: "Lucas Dupont".into(),
            mot_de_passe: "secret123".into(),
            confirmation: "secret123".into(),
            ..Default::default()
        }
    }

    #[test]
    fn identifiants() {
        for ok in [
            "lucas",
            "l",
            "eleve_2-b",
            "a234567890123456789012345678901b",
        ] {
            assert!(identifiant_valide(ok), "{ok}");
        }
        for ko in [
            "",
            "Lucas",
            "2lucas",
            "-x",
            "lu cas",
            "élève",
            "root",
            "sddm",
            "systemd-x",
            "a2345678901234567890123456789012b",
        ] {
            assert!(!identifiant_valide(ko), "{ko}");
        }
    }

    #[test]
    fn machines() {
        assert!(machine_valide("cle-lucas"));
        assert!(machine_valide("a"));
        assert!(!machine_valide("-a"));
        assert!(!machine_valide("a-"));
        assert!(!machine_valide("Cle"));
        assert!(!machine_valide(&"a".repeat(64)));
        assert_eq!(machine_par_defaut("eleve_2"), "cle-eleve-2");
    }

    #[test]
    fn contenu_config() {
        let c = exemple().config_txt().unwrap();
        assert!(c.contains("\nidentifiant=lucas\n"));
        assert!(c.contains("\nnom_complet=Lucas Dupont\n"));
        assert!(c.contains("\nmachine=cle-lucas\n"));
        assert!(c.contains("\nclavier=be\n"));
        assert!(c.contains("\nlangue=fr_BE.UTF-8\n"));
        assert!(c.contains("\nchiffrer=oui\n"));
        assert!(!c.contains("secret123"));
        assert!(!c.contains('\r'));
    }

    #[test]
    fn hachage_accepte_par_l_image_et_verifiable() {
        let c = exemple().config_txt().unwrap();
        let h = c
            .lines()
            .find_map(|l| l.strip_prefix("mot_de_passe_hache="))
            .unwrap();
        // Même contrôle que l'image : ^\$(6|y|5)\$[./A-Za-z0-9\$=,]+$
        assert!(h.starts_with("$6$"));
        assert!(
            h[3..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "./$=,".contains(c)),
            "{h}"
        );
        // Sel de 16 caractères au plus (au-delà, la glibc le tronque et la connexion échoue).
        let sel = h
            .split('$')
            .filter(|c| !c.is_empty() && !c.starts_with("rounds="))
            .nth(1)
            .unwrap();
        assert_eq!(sel.len(), 16, "{h}");
        let h = sha_crypt::PasswordHash::new(h).unwrap();
        assert!(ShaCrypt::SHA512.verify_password(b"secret123", &h).is_ok());
        assert!(ShaCrypt::SHA512.verify_password(b"autre", &h).is_err());
    }

    #[test]
    fn mot_de_passe_facultatif_mais_controle() {
        let mut p = exemple();
        p.mot_de_passe.clear();
        p.confirmation.clear();
        assert!(!p.config_txt().unwrap().contains("mot_de_passe_hache"));
        p.mot_de_passe = "abc".into();
        p.confirmation = "abc".into();
        assert_eq!(p.erreurs().len(), 1);
        p.mot_de_passe = "abcdefg".into();
        assert_eq!(p.erreurs().len(), 1);
        p.nom_complet = "Dupont, Lucas".into();
        assert_eq!(p.erreurs().len(), 2);
    }
}
