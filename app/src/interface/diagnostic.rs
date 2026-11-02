//! Onglet « Diagnostic » de la fenêtre : identification de la clé, mesures en lecture seule et
//! explications. La logique est dans `crate::diagnostic` ; ici, seulement l'affichage.

use super::{Appli, Onglet};
use crate::diagnostic::interpretation::{self, Appreciation, Certitude, Fiche, duree_us};
use crate::diagnostic::materiel::{Materiel, nom_vitesse};
use crate::diagnostic::mesures::{CLASSES_US, EN_PARALLELE, Latences, Mesures};
use crate::diagnostic::textes::texte;
use crate::disques;
use crate::theme;
use eframe::egui;

/// Couleur d'une appréciation.
fn couleur(ui: &egui::Ui, a: Appreciation) -> egui::Color32 {
    match a {
        Appreciation::Bon => theme::vert(ui),
        Appreciation::Moyen => ui.visuals().warn_fg_color,
        Appreciation::Faible => ui.visuals().error_fg_color,
        Appreciation::Neutre => ui.visuals().weak_text_color(),
    }
}

/// Petite étiquette arrondie (« lu », « mesuré »…).
fn etiquette(ui: &mut egui::Ui, c: Certitude) {
    let teinte = match c {
        Certitude::Lu => theme::accent(ui),
        Certitude::Mesure => theme::vert(ui),
        Certitude::Deduit => ui.visuals().warn_fg_color,
        Certitude::NonMesure => ui.visuals().weak_text_color(),
    };
    egui::Frame::new()
        .fill(theme::teinte(teinte))
        .stroke(egui::Stroke::new(1.0, teinte.gamma_multiply(0.5)))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(c.nom()).small().color(teinte));
        })
        .response
        .on_hover_text(match c {
            Certitude::Lu => "Déclaré par la clé ou par le système.",
            Certitude::Mesure => "Obtenu en chronométrant de vraies lectures.",
            Certitude::Deduit => "Interprétation des autres informations, pas une certitude.",
            Certitude::NonMesure => "Demande des écritures, donc l'effacement de la clé.",
        });
}

/// Texte de plusieurs paragraphes, replié à la largeur disponible.
fn paragraphes(ui: &mut egui::Ui, t: &str) {
    for (i, p) in t.split("\n\n").enumerate() {
        if i > 0 {
            ui.add_space(4.0);
        }
        ui.add(egui::Label::new(p).wrap());
    }
}

/// Section repliable contenant un texte explicatif.
fn explication(ui: &mut egui::Ui, titre: &str, id: &str, cle: &str) {
    egui::CollapsingHeader::new(titre)
        .id_salt(id)
        .default_open(false)
        .show(ui, |ui| paragraphes(ui, texte(cle)));
}

fn oui_non(v: Option<bool>) -> String {
    match v {
        Some(true) => "oui".into(),
        Some(false) => "non".into(),
        None => "—".into(),
    }
}

/// Libellés des classes de l'histogramme (voir `CLASSES_US`).
fn classes() -> Vec<String> {
    let mut l = Vec::new();
    let mut bas = 0u32;
    for &haut in &CLASSES_US {
        l.push(if bas == 0 {
            format!("< {}", duree_us(haut))
        } else {
            format!("{} – {}", duree_us(bas), duree_us(haut))
        });
        bas = haut;
    }
    l.push(format!("≥ {}", duree_us(bas)));
    l
}

/// Histogramme horizontal des latences.
fn histogramme(ui: &mut egui::Ui, l: &Latences) {
    let total: u64 = l.histogramme.iter().sum();
    let max = l.histogramme.iter().copied().max().unwrap_or(0).max(1);
    let barre = theme::accent(ui);
    let fond = ui.visuals().faint_bg_color;
    egui::Grid::new("histogramme")
        .num_columns(3)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            for (libelle, &n) in classes().iter().zip(&l.histogramme) {
                ui.label(egui::RichText::new(libelle).small());
                let largeur = (ui.available_width() - 50.0).clamp(80.0, 320.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(largeur, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, fond);
                if n > 0 {
                    let mut plein = rect;
                    plein.set_width((rect.width() * n as f32 / max as f32).max(2.0));
                    ui.painter().rect_filled(plein, 3.0, barre);
                }
                let part = if total == 0 {
                    0.0
                } else {
                    n as f64 * 100.0 / total as f64
                };
                ui.label(
                    egui::RichText::new(if n == 0 {
                        "—".to_string()
                    } else if part < 1.0 {
                        "< 1 %".to_string()
                    } else {
                        format!("{part:.0} %")
                    })
                    .small()
                    .weak(),
                );
                ui.end_row();
            }
        });
}

/// Nombre à la française (une décimale sous 100).
fn dec(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}").replace('.', ",")
    }
}

impl Appli {
    /// Barre d'onglets sous l'en-tête.
    pub(super) fn ui_onglets(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (o, nom, aide) in [
                (
                    Onglet::Fabriquer,
                    "Fabriquer une clé",
                    "Écrire le système sur une clé USB",
                ),
                (
                    Onglet::Diagnostic,
                    "Diagnostic de la clé",
                    "Examiner une clé : ce qui compte pour y faire tourner un système (lecture seule)",
                ),
            ] {
                let choisi = self.onglet == o;
                let mut texte = egui::RichText::new(nom).family(theme::gras());
                if choisi {
                    texte = texte.color(egui::Color32::WHITE);
                }
                let mut b = egui::Button::new(texte)
                    .corner_radius(8)
                    .min_size(egui::vec2(150.0, 30.0));
                if choisi {
                    b = b.fill(theme::accent(ui));
                }
                if ui.add(b).on_hover_text(aide).clicked() {
                    self.onglet = o;
                }
            }
        });
    }

    /// Contenu de l'onglet Diagnostic.
    pub(super) fn ui_diagnostic(&mut self, ui: &mut egui::Ui, occupe: bool) {
        Self::carte(ui, 1, "Clé à examiner", |ui| {
            ui.add_enabled_ui(!occupe, |ui| self.choix_disque(ui));
            ui.label(
                egui::RichText::new(
                    "Rien n'est écrit sur la clé : une clé déjà fabriquée peut être examinée.",
                )
                .small()
                .weak(),
            );
            ui.add_space(4.0);
            explication(ui, "À quoi sert ce diagnostic ?", "intro", "introduction");
            explication(
                ui,
                "Lu, mesuré, déduit : d'où viennent les informations",
                "certitudes",
                "certitudes",
            );
            explication(
                ui,
                "Les quatre familles de clés",
                "categories",
                "categories",
            );
        });
        let Some(d) = self.disque.clone() else {
            return;
        };
        // Identification relue quand la clé choisie change (lecture instantanée, sans droits).
        if self.diag.materiel.as_ref().map(|(x, _)| x) != Some(&d) {
            self.diag.materiel = Some((d.clone(), disques::identifier(&d)));
        }
        let m = self
            .diag
            .materiel
            .as_ref()
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        let mes = self
            .diag
            .mesures
            .iter()
            .find(|(x, _)| *x == d)
            .map(|(_, m)| m.clone());

        ui.add_space(10.0);
        Self::carte(ui, 2, "Ce que la clé et le système déclarent", |ui| {
            Self::ui_materiel(ui, &d, &m);
        });
        ui.add_space(10.0);
        Self::carte(ui, 3, "Mesures en lecture", |ui| match &mes {
            Some(x) => Self::ui_mesures(ui, x),
            None => {
                ui.add(
                    egui::Label::new(format!(
                        "Trois tests d'environ 8 secondes chacun : lecture séquentielle, lectures \
                         aléatoires de 4 Ko une à la fois, puis {EN_PARALLELE} à la fois. Rien n'est \
                         écrit. Lancez-les avec le bouton en bas de la fenêtre."
                    ))
                    .wrap(),
                );
                #[cfg(target_os = "linux")]
                ui.label(
                    egui::RichText::new(
                        "Lire la clé directement demande le mot de passe administrateur.",
                    )
                    .small()
                    .weak(),
                );
            }
        });
        ui.add_space(10.0);
        Self::carte(ui, 4, "Ce que cela signifie", |ui| {
            let (estimation, appr) = interpretation::estimation(&m, mes.as_ref());
            Self::encadre(ui, couleur(ui, appr), &estimation);
            ui.add_space(8.0);
            for f in interpretation::fiches(&m, mes.as_ref()) {
                Self::ui_fiche(ui, &f);
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                if ui
                    .button("Copier le résumé")
                    .on_hover_text("Résumé en texte, pour comparer des clés ou demander de l'aide")
                    .clicked()
                {
                    ui.ctx()
                        .copy_text(interpretation::resume(&d, &m, mes.as_ref()));
                }
            });
        });
    }

    fn ui_materiel(ui: &mut egui::Ui, d: &disques::Disque, m: &Materiel) {
        let mut lignes: Vec<(&str, String)> = vec![
            (
                "Modèle",
                if d.nom.is_empty() {
                    "—".into()
                } else {
                    d.nom.clone()
                },
            ),
            ("Capacité annoncée", crate::suivi::taille_lisible(d.taille)),
        ];
        let descripteur = format!("{} {}", m.fabricant_usb, m.produit_usb);
        if !descripteur.trim().is_empty() {
            lignes.push(("Descripteur USB", descripteur.trim().to_string()));
        }
        if let (Some(v), Some(p)) = (m.vid, m.pid) {
            lignes.push(("Identifiant USB (VID:PID)", format!("{v:04x}:{p:04x}")));
        }
        lignes.push(("Pilote de stockage", m.pilote.clone().unwrap_or("—".into())));
        lignes.push((
            "Norme USB de la clé",
            m.version_usb.clone().unwrap_or("—".into()),
        ));
        lignes.push((
            "Liaison négociée",
            m.vitesse_mbps.map(nom_vitesse).unwrap_or("—".into()),
        ));
        lignes.push(("TRIM", oui_non(m.trim)));
        if let Some(n) = m.file_commandes {
            lignes.push(("Commandes simultanées", n.to_string()));
        }
        if let Some(t) = m.transfert_max_kio {
            lignes.push(("Transfert maximal", format!("{t} Kio")));
        }
        if let (Some(l), Some(p)) = (m.secteur_logique, m.secteur_physique) {
            lignes.push(("Secteurs (logique / physique)", format!("{l} / {p} octets")));
        }
        if let Some(r) = m.rotatif_declare {
            lignes.push(("Se déclare « rotative »", oui_non(Some(r))));
        }
        if let Some(o) = &m.ordonnanceur {
            lignes.push(("Ordonnanceur d'E/S (ce PC)", o.clone()));
        }
        egui::Grid::new("materiel")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .striped(true)
            .show(ui, |ui| {
                for (k, v) in lignes {
                    ui.label(egui::RichText::new(k).weak());
                    ui.label(v);
                    ui.end_row();
                }
            });
        if m.bridee_en_usb2() {
            ui.add_space(6.0);
            Self::encadre(
                ui,
                ui.visuals().warn_fg_color,
                "Clé USB 3 reliée en USB 2 : branchez-la sur un autre port (souvent bleu), sans \
                 rallonge ni concentrateur.",
            );
        }
        if m.rotatif_declare == Some(true) {
            ui.label(
                egui::RichText::new(
                    "Beaucoup de clés se déclarent « rotatives » comme un disque dur : c'est sans \
                     conséquence, et cela montre qu'on ne peut pas tout croire de ce qu'elles disent.",
                )
                .small()
                .weak(),
            );
        }
    }

    fn ui_mesures(ui: &mut egui::Ui, x: &Mesures) {
        if !x.directes {
            Self::encadre(
                ui,
                ui.visuals().warn_fg_color,
                "Le système a refusé les lectures directes : les mesures sont passées par son \
                 cache et sont probablement trop optimistes.",
            );
            ui.add_space(6.0);
        }
        let a1 = &x.aleatoire_1;
        let an = &x.aleatoire_n;
        egui::Grid::new("mesures")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Lecture séquentielle (blocs de 4 Mo)").weak());
                ui.label(format!("{} Mo/s", dec(x.sequentielle.mo_s)));
                ui.end_row();
                ui.label(egui::RichText::new("Aléatoire 4 Ko, une à la fois").weak());
                ui.label(format!(
                    "{} lectures/s ({} Mo/s)",
                    dec(a1.iops),
                    dec(a1.mo_s)
                ));
                ui.end_row();
                ui.label(
                    egui::RichText::new(format!("Aléatoire 4 Ko, {EN_PARALLELE} à la fois")).weak(),
                );
                ui.label(format!(
                    "{} lectures/s ({} Mo/s)",
                    dec(an.iops),
                    dec(an.mo_s)
                ));
                ui.end_row();
                ui.label(egui::RichText::new("Latence médiane / 99 %").weak())
                    .on_hover_text("La moitié des lectures est plus rapide que la médiane ; 99 % le sont plus que le second chiffre.");
                ui.label(format!(
                    "{} / {}",
                    duree_us(a1.latences.mediane_us),
                    duree_us(a1.latences.p99_us)
                ));
                ui.end_row();
                ui.label(egui::RichText::new("Lecture la plus lente").weak());
                ui.label(duree_us(a1.latences.max_us));
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Répartition des temps de lecture (4 Ko, une à la fois)")
                .family(theme::gras()),
        );
        ui.add_space(2.0);
        histogramme(ui, &a1.latences);
    }

    fn ui_fiche(ui: &mut egui::Ui, f: &Fiche) {
        let c = couleur(ui, f.appreciation);
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, theme::bord_carte(ui)))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, c);
                    ui.label(egui::RichText::new(f.titre).family(theme::gras()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        etiquette(ui, f.certitude);
                    });
                });
                ui.add(
                    egui::Label::new(egui::RichText::new(&f.valeur).color(
                        if f.appreciation == Appreciation::Neutre {
                            ui.visuals().text_color()
                        } else {
                            c
                        },
                    ))
                    .wrap(),
                );
                if !f.commentaire.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&f.commentaire).small().weak()).wrap(),
                    );
                }
                egui::CollapsingHeader::new("En savoir plus")
                    .id_salt(f.cle)
                    .show(ui, |ui| {
                        paragraphes(ui, texte(f.cle));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Pourquoi c'est important ici")
                                .family(theme::gras()),
                        );
                        paragraphes(ui, texte(&format!("{}.pourquoi", f.cle)));
                    });
            });
    }
}
