//! Fenêtre principale, dans l'esprit de Rufus : une seule fenêtre, de haut en bas
//! (clé, image, personnalisation, état, bouton).

use crate::config::{CLAVIERS, LANGUES, MOT_DE_PASSE_MIN, Personnalisation};
use crate::disques::{self, Disque};
use crate::releases::{self, Version};
use crate::suivi::{ANNULE, Suivi};
use crate::travail::{self, Ecriture};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Evenement {
    Etape(String),
    Progres(f32, String),
    Fin(Result<String, String>),
}

/// Suivi relié à la fenêtre par un canal.
struct SuiviFenetre {
    tx: Sender<Evenement>,
    annuler: Arc<AtomicBool>,
    ctx: egui::Context,
    dernier: Mutex<Instant>,
}

impl Suivi for SuiviFenetre {
    fn etape(&self, texte: &str) {
        let _ = self.tx.send(Evenement::Etape(texte.to_string()));
        self.ctx.request_repaint();
    }
    fn progres(&self, fraction: f32, detail: &str) {
        let mut d = self.dernier.lock().unwrap();
        if d.elapsed() >= Duration::from_millis(50) || fraction >= 1.0 {
            *d = Instant::now();
            let _ = self
                .tx
                .send(Evenement::Progres(fraction, detail.to_string()));
            self.ctx.request_repaint();
        }
    }
    fn annule(&self) -> bool {
        self.annuler.load(Ordering::Relaxed)
    }
}

struct Travail {
    rx: Receiver<Evenement>,
    annuler: Arc<AtomicBool>,
    etape: String,
    fraction: f32,
    detail: String,
}

enum EtatVersions {
    Chargement(Receiver<Result<Vec<Version>, String>>),
    Pret(Vec<Version>),
    Erreur(String),
}

#[derive(PartialEq, Clone, Copy)]
enum ModeImage {
    Publiee,
    Locale,
}

pub struct Appli {
    disques: Vec<Disque>,
    erreur_disques: Option<String>,
    disque: Option<Disque>,
    derniere_detection: Instant,

    versions: EtatVersions,
    version: usize,
    mode: ModeImage,
    chemin_local: String,

    perso: Personnalisation,
    verifier: bool,

    confirmation: bool,
    travail: Option<Travail>,
    resultat: Option<Result<String, String>>,
}

fn charger_versions(ctx: &egui::Context) -> EtatVersions {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(releases::lister_versions());
        ctx.request_repaint();
    });
    EtatVersions::Chargement(rx)
}

impl Appli {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut a = Self {
            disques: Vec::new(),
            erreur_disques: None,
            disque: None,
            derniere_detection: Instant::now(),
            versions: charger_versions(&cc.egui_ctx),
            version: 0,
            mode: ModeImage::Publiee,
            chemin_local: String::new(),
            perso: Personnalisation::default(),
            verifier: true,
            confirmation: false,
            travail: None,
            resultat: None,
        };
        a.detecter();
        a
    }

    /// Relit la liste des clés ; garde la sélection si la même clé est toujours là.
    fn detecter(&mut self) {
        self.derniere_detection = Instant::now();
        match disques::lister() {
            Ok(l) => {
                self.erreur_disques = None;
                if let Some(d) = &self.disque
                    && !l.contains(d)
                {
                    self.disque = None;
                }
                if self.disque.is_none() && l.len() == 1 {
                    self.disque = Some(l[0].clone());
                }
                self.disques = l;
            }
            Err(e) => {
                self.erreur_disques = Some(e);
                self.disques.clear();
                self.disque = None;
            }
        }
    }

    fn version_choisie(&self) -> Option<&Version> {
        match &self.versions {
            EtatVersions::Pret(v) => v.get(self.version),
            _ => None,
        }
    }

    /// Ce qui empêche de démarrer (vide = prêt).
    fn obstacles(&self) -> Vec<String> {
        let mut o = Vec::new();
        if self.disque.is_none() {
            o.push("Choisissez une clé USB.".to_string());
        }
        match self.mode {
            ModeImage::Publiee if self.version_choisie().is_none() => {
                o.push("Aucune version de l'image disponible (connexion Internet ?).".into())
            }
            ModeImage::Locale if self.chemin_local.trim().is_empty() => {
                o.push("Indiquez le fichier image (.img.zst ou .img).".into())
            }
            _ => {}
        }
        o.extend(self.perso.erreurs());
        o
    }

    fn demarrer(&mut self, ctx: &egui::Context) {
        let Some(disque) = self.disque.clone() else {
            return;
        };
        let config = match self.perso.config_txt() {
            Ok(c) => c,
            Err(e) => {
                self.resultat = Some(Err(e));
                return;
            }
        };
        let version = self.version_choisie().cloned();
        let local =
            (self.mode == ModeImage::Locale).then(|| PathBuf::from(self.chemin_local.trim()));
        let verifier = self.verifier;

        let (tx, rx) = channel();
        let annuler = Arc::new(AtomicBool::new(false));
        let suivi = SuiviFenetre {
            tx: tx.clone(),
            annuler: annuler.clone(),
            ctx: ctx.clone(),
            dernier: Mutex::new(Instant::now()),
        };
        std::thread::spawn(move || {
            let r = (|| {
                let image = match (&local, &version) {
                    (Some(p), _) => releases::image_locale(p, &suivi)?,
                    (None, Some(v)) => releases::telecharger(v, &suivi)?,
                    _ => return Err("aucune image choisie".to_string()),
                };
                let e = Ecriture {
                    disque,
                    image: image.image,
                    bmap: image.bmap,
                    config,
                    verifier,
                };
                travail::ecrire(&e, &suivi)
            })();
            let _ = tx.send(Evenement::Fin(r));
            suivi.ctx.request_repaint();
        });
        self.resultat = None;
        self.travail = Some(Travail {
            rx,
            annuler,
            etape: "Démarrage".into(),
            fraction: 0.0,
            detail: String::new(),
        });
    }

    fn recevoir(&mut self) {
        let mut fin = None;
        if let Some(t) = &mut self.travail {
            while let Ok(ev) = t.rx.try_recv() {
                match ev {
                    Evenement::Etape(e) => {
                        t.etape = e;
                        t.fraction = 0.0;
                        t.detail.clear();
                    }
                    Evenement::Progres(f, d) => {
                        t.fraction = f;
                        t.detail = d;
                    }
                    Evenement::Fin(r) => fin = Some(r),
                }
            }
        }
        if let Some(r) = fin {
            self.travail = None;
            self.resultat = Some(r);
            self.detecter();
        }
    }

    fn section(ui: &mut egui::Ui, titre: &str) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(titre).strong().size(15.0));
        ui.separator();
    }

    fn ui_cle(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "Clé USB");
        ui.horizontal(|ui| {
            let texte = self
                .disque
                .as_ref()
                .map(|d| d.libelle())
                .unwrap_or_else(|| {
                    if self.disques.is_empty() {
                        "Aucune clé USB détectée".into()
                    } else {
                        "Choisir…".into()
                    }
                });
            egui::ComboBox::from_id_salt("disque")
                .width(ui.available_width() - 90.0)
                .selected_text(texte)
                .show_ui(ui, |ui| {
                    for d in &self.disques {
                        ui.selectable_value(&mut self.disque, Some(d.clone()), d.libelle());
                    }
                });
            if ui
                .button("Actualiser")
                .on_hover_text("Rechercher de nouveau les clés USB")
                .clicked()
            {
                self.detecter();
            }
        });
        if let Some(e) = &self.erreur_disques {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if let Some(d) = &self.disque
            && d.taille < 15_000_000_000
        {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Clé de moins de 16 Go : l'image risque de ne pas tenir.",
            );
        }
    }

    fn ui_image(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "Image");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.mode, ModeImage::Publiee, "Version publiée :");
            let mut recharger = false;
            let mut erreur = None;
            match &self.versions {
                EtatVersions::Chargement(_) => {
                    ui.spinner();
                    ui.label("recherche des versions…");
                }
                EtatVersions::Erreur(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, "indisponible");
                    recharger = ui.button("Réessayer").clicked();
                    erreur = Some(e.clone());
                }
                EtatVersions::Pret(v) => {
                    let texte = |x: &Version| match x.taille {
                        Some(t) => format!("{} ({})", x.etiquette, crate::suivi::taille_lisible(t)),
                        None => x.etiquette.clone(),
                    };
                    egui::ComboBox::from_id_salt("version")
                        .selected_text(v.get(self.version).map(texte).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (i, x) in v.iter().enumerate() {
                                ui.selectable_value(&mut self.version, i, texte(x));
                            }
                        });
                }
            }
            if recharger {
                self.versions = charger_versions(ui.ctx());
            }
            erreur
        })
        .inner
        .map(|e| ui.label(egui::RichText::new(e).small().weak()));
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.mode, ModeImage::Locale, "Fichier :");
            // Bouton à droite, champ dans l'espace restant.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Sous Windows, l'application tourne en administrateur : Windows interdit alors le
                // glisser-déposer depuis l'Explorateur. Le bouton est le moyen principal.
                if ui.button("Parcourir…").clicked() {
                    let mut dialogue = rfd::FileDialog::new()
                        .set_title("Choisir l'image Fabrique-Clé")
                        .add_filter("Image Fabrique-Clé (.img.zst, .img)", &["zst", "img"])
                        .add_filter("Tous les fichiers", &["*"]);
                    let actuel = PathBuf::from(self.chemin_local.trim());
                    if let Some(dossier) = actuel.parent().filter(|d| d.is_dir()) {
                        dialogue = dialogue.set_directory(dossier);
                    }
                    if let Some(p) = dialogue.pick_file() {
                        self.chemin_local = p.display().to_string();
                        self.mode = ModeImage::Locale;
                    }
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.chemin_local)
                        .hint_text("chemin d'un fichier .img.zst")
                        .desired_width(ui.available_width()),
                );
                if r.changed() {
                    self.mode = ModeImage::Locale;
                }
                r.on_hover_text(
                    "Image téléchargée à part (.img.zst ou .img) ; .bmap et .sha256 lus à côté s'ils existent.",
                );
            });
        });
    }

    fn ui_perso(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "Personnalisation");
        let p = &mut self.perso;
        egui::Grid::new("perso").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Identifiant :");
            ui.add(egui::TextEdit::singleline(&mut p.identifiant).hint_text("ex. lucas").desired_width(220.0));
            ui.end_row();

            ui.label("Nom complet :");
            ui.add(egui::TextEdit::singleline(&mut p.nom_complet).hint_text("ex. Lucas Dupont").desired_width(220.0));
            ui.end_row();

            ui.label("Mot de passe :");
            ui.add(egui::TextEdit::singleline(&mut p.mot_de_passe).password(true).desired_width(220.0))
                .on_hover_text(format!(
                    "Au moins {MOT_DE_PASSE_MIN} caractères. Laisser vide : il sera demandé au premier démarrage."
                ));
            ui.end_row();

            ui.label("Confirmation :");
            ui.add(egui::TextEdit::singleline(&mut p.confirmation).password(true).desired_width(220.0));
            ui.end_row();

            ui.label("Nom de la machine :");
            let proposition = crate::config::machine_par_defaut(p.identifiant.trim());
            ui.add(egui::TextEdit::singleline(&mut p.machine).hint_text(proposition).desired_width(220.0));
            ui.end_row();

            ui.label("Clavier :");
            egui::ComboBox::from_id_salt("clavier")
                .selected_text(CLAVIERS[p.clavier].1)
                .show_ui(ui, |ui| {
                    for (i, c) in CLAVIERS.iter().enumerate() {
                        ui.selectable_value(&mut p.clavier, i, c.1);
                    }
                });
            ui.end_row();

            ui.label("Langue :");
            egui::ComboBox::from_id_salt("langue")
                .selected_text(LANGUES[p.langue].1)
                .show_ui(ui, |ui| {
                    for (i, l) in LANGUES.iter().enumerate() {
                        ui.selectable_value(&mut p.langue, i, l.1);
                    }
                });
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.checkbox(&mut p.chiffrer, "Chiffrer la clé (recommandé)").on_hover_text(
            "Une phrase secrète sera choisie au premier démarrage et demandée à chaque démarrage. \
             Sans elle, les données de la clé sont perdues : notez-la !",
        );
        ui.checkbox(
            &mut self.verifier,
            "Vérifier la clé après l'écriture (relecture)",
        );
    }

    fn ui_etat(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "État");
        if let Some(t) = &self.travail {
            ui.label(&t.etape);
            ui.add(
                egui::ProgressBar::new(t.fraction)
                    .show_percentage()
                    .text(&t.detail)
                    .animate(true),
            );
        } else if let Some(r) = &self.resultat {
            match r {
                Ok(m) => ui.colored_label(egui::Color32::from_rgb(0x2e, 0x9e, 0x4f), m.as_str()),
                Err(m) if m == ANNULE => ui.label(m),
                Err(m) => ui.colored_label(ui.visuals().error_fg_color, m.as_str()),
            };
        } else {
            let o = self.obstacles();
            if o.is_empty() {
                ui.label("Prêt.");
            } else {
                for x in o {
                    ui.label(format!("• {x}"));
                }
            }
        }
    }

    fn ui_confirmation(&mut self, ctx: &egui::Context) {
        let Some(d) = self.disque.clone() else {
            self.confirmation = false;
            return;
        };
        let mut choix = None;
        egui::Modal::new(egui::Id::unique("confirmation")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading("Effacer la clé ?");
            ui.add_space(6.0);
            ui.label("Toutes les données de ce disque seront définitivement effacées :");
            ui.add_space(4.0);
            ui.label(egui::RichText::new(d.libelle()).strong());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Effacer et écrire").clicked() {
                    choix = Some(true);
                }
                if ui.button("Annuler").clicked() {
                    choix = Some(false);
                }
            });
        });
        match choix {
            Some(true) => {
                self.confirmation = false;
                self.demarrer(ctx);
            }
            Some(false) => self.confirmation = false,
            None => {}
        }
    }
}

impl eframe::App for Appli {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.recevoir();
        if let EtatVersions::Chargement(rx) = &self.versions
            && let Ok(r) = rx.try_recv()
        {
            self.versions = match r {
                Ok(v) => EtatVersions::Pret(v),
                Err(e) => EtatVersions::Erreur(e),
            };
            self.version = 0;
        }
        // Fichier glissé sur la fenêtre : image locale.
        let deposes: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = deposes.into_iter().next() {
            self.mode = ModeImage::Locale;
            self.chemin_local = p.display().to_string();
        }
        // Détection automatique des clés branchées / retirées.
        if self.travail.is_none() && self.derniere_detection.elapsed() > Duration::from_secs(2) {
            self.detecter();
        }
        ctx.request_repaint_after(Duration::from_secs(2));

        let occupe = self.travail.is_some();
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!occupe, |ui| {
                    self.ui_cle(ui);
                    self.ui_image(ui);
                    self.ui_perso(ui);
                });
                self.ui_etat(ui);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if let Some(t) = &self.travail {
                        if ui.button("Annuler").clicked() {
                            t.annuler.store(true, Ordering::Relaxed);
                        }
                    } else {
                        let pret = self.obstacles().is_empty();
                        let b = egui::Button::new(
                            egui::RichText::new("  Démarrer  ").strong().size(16.0),
                        );
                        if ui.add_enabled(pret, b).clicked() {
                            self.resultat = None;
                            self.confirmation = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.hyperlink_to(
                            "Aide et sources",
                            format!("https://github.com/{}", releases::DEPOT),
                        );
                        ui.label(egui::RichText::new(env!("VERSION_APPLI")).weak());
                    });
                });
            });
        });
        if self.confirmation {
            self.ui_confirmation(&ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(t) = &self.travail {
            t.annuler.store(true, Ordering::Relaxed);
        }
    }
}
