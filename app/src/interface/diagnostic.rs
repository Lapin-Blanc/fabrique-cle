//! Onglet « Diagnostic » de la fenêtre : identification de la clé, mesures en lecture seule et
//! explications. La logique est dans `crate::diagnostic` ; ici, seulement l'affichage.

use super::{Appli, Evenement, Nature, Onglet, SuiviFenetre, Travail};
use crate::diagnostic::analyse::{self, Analyse};
use crate::diagnostic::interpretation::{self, Appreciation, Certitude, Fiche, duree_us};
use crate::diagnostic::materiel::{Materiel, nom_vitesse};
use crate::diagnostic::mesures::{CLASSES_US, EN_PARALLELE, Latences, Mesures};
use crate::diagnostic::textes::texte;
use crate::disques;
use crate::theme;
use eframe::egui;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Durées proposées pour la longue écriture séquentielle (secondes, libellé).
const DUREES_LONGUES: [(u64, &str); 3] =
    [(60, "1 minute"), (180, "3 minutes"), (600, "10 minutes")];

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
fn histogramme(ui: &mut egui::Ui, id: &str, l: &Latences) {
    let total: u64 = l.histogramme.iter().sum();
    let max = l.histogramme.iter().copied().max().unwrap_or(0).max(1);
    let barre = theme::accent(ui);
    let fond = ui.visuals().faint_bg_color;
    egui::Grid::new(id)
        .num_columns(3)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            for (libelle, &n) in classes().iter().zip(&l.histogramme) {
                ui.label(egui::RichText::new(libelle).small());
                let largeur = 220.0;
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

/// Courbe du débit pendant la longue écriture : Go écrits en abscisse, Mo/s en ordonnée.
/// La fin du cache SLC, si elle est repérée, est marquée d'un trait vertical.
fn courbe(ui: &mut egui::Ui, a: &Analyse) {
    let points = &a.longue.points;
    if points.len() < 2 {
        ui.label(egui::RichText::new("Écriture trop courte pour tracer une courbe.").weak());
        return;
    }
    let largeur = ui.available_width().min(560.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(largeur, 210.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let texte = ui.visuals().weak_text_color();
    let police = egui::FontId::proportional(11.0);
    let zone = egui::Rect::from_min_max(
        rect.min + egui::vec2(44.0, 18.0),
        rect.max - egui::vec2(12.0, 34.0),
    );
    let max_x = points.last().map(|q| q[0]).unwrap_or(1.0).max(1e-3);
    let max_y = points.iter().map(|q| q[1]).fold(1.0f32, f32::max) * 1.1;
    let vers = |x: f32, y: f32| {
        egui::pos2(
            zone.left() + zone.width() * x / max_x,
            zone.bottom() - zone.height() * y / max_y,
        )
    };
    let grille = egui::Stroke::new(1.0, ui.visuals().faint_bg_color.gamma_multiply(2.0));
    for i in 0..=4 {
        let y = max_y * i as f32 / 4.0;
        let a = vers(0.0, y);
        p.line_segment([a, egui::pos2(zone.right(), a.y)], grille);
        p.text(
            egui::pos2(zone.left() - 6.0, a.y),
            egui::Align2::RIGHT_CENTER,
            format!("{y:.0}"),
            police.clone(),
            texte,
        );
    }
    for i in 0..=4 {
        let x = max_x * i as f32 / 4.0;
        let a = vers(x, 0.0);
        p.text(
            egui::pos2(a.x, zone.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            format!("{x:.1}").replace('.', ","),
            police.clone(),
            texte,
        );
    }
    p.text(
        egui::pos2(rect.left(), rect.top()),
        egui::Align2::LEFT_TOP,
        "Mo/s",
        police.clone(),
        texte,
    );
    p.text(
        egui::pos2(zone.center().x, zone.bottom() + 18.0),
        egui::Align2::CENTER_TOP,
        "Go écrits",
        police.clone(),
        texte,
    );
    if let Some(b) = a.longue.chute_apres {
        let x = vers(b as f32 / 1e9, 0.0).x;
        p.line_segment(
            [egui::pos2(x, zone.top()), egui::pos2(x, zone.bottom())],
            egui::Stroke::new(1.0, ui.visuals().warn_fg_color),
        );
        p.text(
            egui::pos2(x + 4.0, zone.top()),
            egui::Align2::LEFT_TOP,
            "fin du cache",
            police,
            ui.visuals().warn_fg_color,
        );
    }
    // La courbe part de l'origine des abscisses, au débit de la première demi-seconde.
    let trace: Vec<egui::Pos2> = std::iter::once(vers(0.0, points[0][1]))
        .chain(points.iter().map(|q| vers(q[0], q[1])))
        .collect();
    p.add(egui::Shape::line(
        trace,
        egui::Stroke::new(2.0, theme::accent(ui)),
    ));
}

impl Appli {
    /// Lance l'analyse complète de la clé choisie (après confirmation).
    fn demarrer_analyse(&mut self, ctx: &egui::Context) {
        let Some(disque) = self.disque.clone() else {
            return;
        };
        let reglages = self.diag.reglages;
        let (tx, rx) = channel();
        let annuler = Arc::new(AtomicBool::new(false));
        let suivi = SuiviFenetre {
            tx: tx.clone(),
            annuler: annuler.clone(),
            ctx: ctx.clone(),
            dernier: Mutex::new(Instant::now()),
        };
        let d = disque.clone();
        std::thread::spawn(move || {
            let r = analyse::analyser_cle(&d, reglages, &suivi);
            let _ = tx.send(Evenement::Analyse(r.map(Box::new)));
            suivi.ctx.request_repaint();
        });
        self.diag.erreur = None;
        self.travail = Some(Travail {
            nature: Nature::Analyse(disque),
            rx,
            annuler,
            etape: "Démarrage de l'analyse complète".into(),
            fraction: 0.0,
            detail: String::new(),
        });
    }

    /// Fenêtre de confirmation de l'analyse complète (efface la clé).
    pub(super) fn ui_confirmation_analyse(&mut self, ctx: &egui::Context) {
        let Some(d) = self.disque.clone() else {
            self.diag.confirmer_analyse = false;
            return;
        };
        let mut choix = None;
        egui::Modal::new(egui::Id::unique("confirmation-analyse")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading("Effacer la clé pour l'analyser ?");
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    "L'analyse complète écrit sur toute la clé. Tout son contenu sera \
                     définitivement effacé, y compris un système Fabrique-Clé :",
                )
                .wrap(),
            );
            ui.add_space(6.0);
            Self::encadre(ui, ui.visuals().warn_fg_color, &d.libelle());
            ui.add_space(6.0);
            let longue = DUREES_LONGUES
                .iter()
                .find(|x| x.0 == self.diag.reglages.duree_longue_s)
                .map_or("", |x| x.1);
            ui.label(
                egui::RichText::new(format!(
                    "Durée : environ {} de tests, puis la clé est laissée vide.",
                    match self.diag.reglages.duree_longue_s {
                        60 => "2 minutes",
                        180 => "4 minutes",
                        _ => "11 minutes",
                    }
                ))
                .small()
                .weak(),
            )
            .on_hover_text(format!("Dont {longue} d'écriture séquentielle."));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let rouge = ui.visuals().error_fg_color;
                let b = egui::Button::new(
                    egui::RichText::new("Effacer et analyser")
                        .family(theme::gras())
                        .color(egui::Color32::WHITE),
                )
                .fill(rouge)
                .min_size(egui::vec2(170.0, 34.0));
                if ui.add(b).clicked() {
                    choix = Some(true);
                }
                if ui
                    .add(egui::Button::new("Annuler").min_size(egui::vec2(100.0, 34.0)))
                    .clicked()
                {
                    choix = Some(false);
                }
            });
        });
        match choix {
            Some(true) => {
                self.diag.confirmer_analyse = false;
                self.demarrer_analyse(ctx);
            }
            Some(false) => self.diag.confirmer_analyse = false,
            None => {}
        }
    }

    /// Carte de l'analyse complète : présentation, réglage, bouton, résultats.
    fn ui_carte_analyse(&mut self, ui: &mut egui::Ui, occupe: bool, an: Option<&Analyse>) {
        explication(
            ui,
            "Ce que fait l'analyse complète",
            "analyse-presentation",
            "analyse",
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(!occupe, |ui| {
            ui.horizontal(|ui| {
                ui.label("Longue écriture :");
                let actuel = DUREES_LONGUES
                    .iter()
                    .find(|x| x.0 == self.diag.reglages.duree_longue_s)
                    .map_or("", |x| x.1);
                egui::ComboBox::from_id_salt("duree-longue")
                    .selected_text(actuel)
                    .show_ui(ui, |ui| {
                        for (s, nom) in DUREES_LONGUES {
                            ui.selectable_value(&mut self.diag.reglages.duree_longue_s, s, nom);
                        }
                    })
                    .response
                    .on_hover_text(
                        "Plus elle est longue, plus on a de chances de remplir le cache SLC et de \
                         voir le débit réel de la mémoire.",
                    );
            });
            ui.add_space(4.0);
            let rouge = ui.visuals().error_fg_color;
            let b = egui::Button::new(
                egui::RichText::new("Analyser en effaçant la clé…")
                    .family(theme::gras())
                    .color(egui::Color32::WHITE),
            )
            .fill(rouge)
            .min_size(egui::vec2(220.0, 32.0));
            if ui.add(b).clicked() {
                self.diag.confirmer_analyse = true;
            }
        });
        if let Some(a) = an {
            ui.add_space(10.0);
            Self::ui_analyse(ui, a);
        }
    }

    fn ui_analyse(ui: &mut egui::Ui, a: &Analyse) {
        if !a.directes {
            Self::encadre(
                ui,
                ui.visuals().warn_fg_color,
                "Le système a refusé les écritures directes : les mesures sont passées par son \
                 cache et sont probablement trop optimistes.",
            );
            ui.add_space(6.0);
        }
        let c = &a.capacite;
        egui::Grid::new("analyse")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Capacité réelle").weak());
                if c.conforme() {
                    ui.label(format!("conforme ({} marqueurs)", c.marqueurs));
                } else {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!(
                            "NON CONFORME : {} fiables",
                            crate::suivi::taille_lisible(c.fiable)
                        ),
                    );
                }
                ui.end_row();
                ui.label(egui::RichText::new("Écriture aléatoire 4 Ko, une à la fois").weak());
                ui.label(format!(
                    "{} écritures/s ({} Mo/s)",
                    dec(a.ecriture_1.iops),
                    dec(a.ecriture_1.mo_s)
                ));
                ui.end_row();
                ui.label(
                    egui::RichText::new(format!(
                        "Écriture aléatoire 4 Ko, {EN_PARALLELE} à la fois"
                    ))
                    .weak(),
                );
                ui.label(format!(
                    "{} écritures/s ({} Mo/s)",
                    dec(a.ecriture_n.iops),
                    dec(a.ecriture_n.mo_s)
                ));
                ui.end_row();
                ui.label(egui::RichText::new("Écriture synchronisée : médiane / 99 %").weak());
                ui.label(format!(
                    "{} / {}",
                    duree_us(a.synchro.latences.mediane_us),
                    duree_us(a.synchro.latences.p99_us)
                ));
                ui.end_row();
                ui.label(egui::RichText::new("Longue écriture séquentielle").weak());
                ui.label(format!(
                    "{} Mo/s en moyenne ({} écrits)",
                    dec(a.longue.mo_s),
                    crate::suivi::taille_lisible(a.longue.octets)
                ));
                ui.end_row();
                ui.label(egui::RichText::new("Lecture aléatoire sur données écrites").weak());
                ui.label(format!("{} lectures/s", dec(a.lecture_donnees.iops)));
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Débit pendant la longue écriture").family(theme::gras()));
        courbe(ui, a);
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Répartition des temps d'écriture synchronisée (4 Ko)")
                .family(theme::gras()),
        );
        ui.add_space(2.0);
        histogramme(ui, "histogramme-synchro", &a.synchro.latences);
    }

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
        let an = self
            .diag
            .analyses
            .iter()
            .find(|(x, _)| *x == d)
            .map(|(_, a)| a.clone());

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
        Self::carte(ui, 4, "Analyse complète (efface la clé)", |ui| {
            self.ui_carte_analyse(ui, occupe, an.as_ref());
        });
        ui.add_space(10.0);
        Self::carte(ui, 5, "Ce que cela signifie", |ui| {
            let (estimation, appr) = interpretation::estimation(&m, mes.as_ref(), an.as_ref());
            Self::encadre(ui, couleur(ui, appr), &estimation);
            ui.add_space(8.0);
            for f in interpretation::fiches(&m, d.taille, mes.as_ref(), an.as_ref()) {
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
                        .copy_text(interpretation::resume(&d, &m, mes.as_ref(), an.as_ref()));
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
        histogramme(ui, "histogramme-lecture", &a1.latences);
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
