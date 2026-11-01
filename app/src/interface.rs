//! Fenêtre principale, dans l'esprit de Rufus : une seule fenêtre, de haut en bas
//! (clé, image, personnalisation, état, bouton).

use crate::config::{CLAVIERS, LANGUES, MOT_DE_PASSE_MIN, Personnalisation};
use crate::disques::{self, Disque};
use crate::releases::{self, Version};
use crate::suivi::{ANNULE, Suivi};
use crate::theme;
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

    icone: Option<egui::TextureHandle>,
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
        theme::appliquer(&cc.egui_ctx);
        let icone = theme::icone().map(|(rgba, l, h)| {
            cc.egui_ctx.load_texture(
                "icone",
                egui::ColorImage::from_rgba_unmultiplied([l as usize, h as usize], &rgba),
                egui::TextureOptions::LINEAR,
            )
        });
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
            icone,
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

    /// Carte numérotée (« 1  Clé USB ») contenant une étape du formulaire.
    fn carte<R>(
        ui: &mut egui::Ui,
        numero: u8,
        titre: &str,
        contenu: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let acc = theme::accent(ui);
        egui::Frame::new()
            .fill(theme::fond_carte(ui))
            .stroke(egui::Stroke::new(1.0, theme::bord_carte(ui)))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Pastille numérotée
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 12.0, acc);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        numero.to_string(),
                        egui::FontId::new(13.0, theme::gras()),
                        egui::Color32::WHITE,
                    );
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(titre).family(theme::gras()).size(16.0));
                });
                ui.add_space(6.0);
                contenu(ui)
            })
            .inner
    }

    /// Message encadré, teinté de la couleur donnée.
    fn encadre(ui: &mut egui::Ui, couleur: egui::Color32, texte: &str) {
        egui::Frame::new()
            .fill(theme::teinte(couleur))
            .stroke(egui::Stroke::new(1.0, couleur.gamma_multiply(0.6)))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(texte).color(couleur));
            });
    }

    fn ui_entete(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(t) = &self.icone {
                ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(44.0, 44.0)));
                ui.add_space(6.0);
            }
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.heading("Fabrique-Clé");
                ui.label(
                    egui::RichText::new("Votre clé USB Debian + KDE Plasma, prête à emporter")
                        .weak(),
                );
            });
        });
    }

    fn ui_cle(&mut self, ui: &mut egui::Ui) {
        Self::carte(ui, 1, "Clé USB", |ui| {
            ui.horizontal(|ui| {
                let texte = self
                    .disque
                    .as_ref()
                    .map(|d| d.libelle())
                    .unwrap_or_else(|| {
                        if self.disques.is_empty() {
                            "Aucune clé USB détectée — branchez-en une".into()
                        } else {
                            "Choisir la clé…".into()
                        }
                    });
                let largeur = ui.available_width() - 110.0;
                egui::ComboBox::from_id_salt("disque")
                    .width(largeur)
                    .selected_text(texte)
                    .show_ui(ui, |ui| {
                        for d in &self.disques {
                            ui.selectable_value(&mut self.disque, Some(d.clone()), d.libelle());
                        }
                    });
                if ui
                    .add_sized([100.0, 30.0], egui::Button::new("Actualiser"))
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
            } else {
                ui.label(
                    egui::RichText::new("Tout son contenu sera effacé. 16 Go minimum ; un SSD USB est bien plus rapide.")
                        .small()
                        .weak(),
                );
            }
        });
    }

    fn ui_image(&mut self, ui: &mut egui::Ui) {
        Self::carte(ui, 2, "Système à installer", |ui| {
            let erreur = ui
                .horizontal(|ui| {
                    ui.radio_value(&mut self.mode, ModeImage::Publiee, "Version publiée");
                    let mut recharger = false;
                    let mut erreur = None;
                    match &self.versions {
                        EtatVersions::Chargement(_) => {
                            ui.spinner();
                            ui.label(egui::RichText::new("recherche des versions…").weak());
                        }
                        EtatVersions::Erreur(e) => {
                            ui.colored_label(ui.visuals().error_fg_color, "indisponible");
                            recharger = ui.button("Réessayer").clicked();
                            erreur = Some(e.clone());
                        }
                        EtatVersions::Pret(v) => {
                            let texte = |x: &Version| match x.taille {
                                Some(t) => format!(
                                    "{}  ·  {}",
                                    x.etiquette,
                                    crate::suivi::taille_lisible(t)
                                ),
                                None => x.etiquette.clone(),
                            };
                            let r = egui::ComboBox::from_id_salt("version")
                                .width(ui.available_width())
                                .selected_text(v.get(self.version).map(texte).unwrap_or_default())
                                .show_ui(ui, |ui| {
                                    for (i, x) in v.iter().enumerate() {
                                        ui.selectable_value(&mut self.version, i, texte(x));
                                    }
                                });
                            if r.response.clicked() {
                                self.mode = ModeImage::Publiee;
                            }
                        }
                    }
                    if recharger {
                        self.versions = charger_versions(ui.ctx());
                    }
                    erreur
                })
                .inner;
            if let Some(e) = erreur {
                ui.label(egui::RichText::new(e).small().weak());
            }
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.mode, ModeImage::Locale, "Fichier");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Sous Windows, l'application tourne en administrateur : Windows interdit alors
                    // le glisser-déposer depuis l'Explorateur. Le bouton est le moyen principal.
                    if ui.add_sized([100.0, 30.0], egui::Button::new("Parcourir…")).clicked() {
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
                            .hint_text("image .img.zst téléchargée à part")
                            .desired_width(ui.available_width())
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                    if r.changed() {
                        self.mode = ModeImage::Locale;
                    }
                    r.on_hover_text(
                        "Image téléchargée à part (.img.zst ou .img) ; .bmap et .sha256 lus à côté s'ils existent.",
                    );
                });
            });
        });
    }

    fn ui_perso(&mut self, ui: &mut egui::Ui) {
        let verifier = &mut self.verifier;
        let p = &mut self.perso;
        Self::carte(ui, 3, "Votre compte", |ui| {
            let champ = |ui: &mut egui::Ui, texte: &mut String, indice: &str, secret: bool| {
                ui.add(
                    egui::TextEdit::singleline(texte)
                        .hint_text(indice)
                        .password(secret)
                        .desired_width(ui.available_width())
                        .margin(egui::Margin::symmetric(8, 6)),
                )
            };
            egui::Grid::new("perso")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .min_col_width(130.0)
                .show(ui, |ui| {
                    ui.label("Identifiant");
                    champ(ui, &mut p.identifiant, "ex. lucas", false)
                        .on_hover_text("Nom de connexion : minuscules, chiffres, - ou _");
                    ui.end_row();

                    ui.label("Nom complet");
                    champ(ui, &mut p.nom_complet, "ex. Lucas Dupont", false);
                    ui.end_row();

                    ui.label("Mot de passe");
                    champ(ui, &mut p.mot_de_passe, "vide : demandé au premier démarrage", true)
                        .on_hover_text(format!(
                            "Au moins {MOT_DE_PASSE_MIN} caractères. Laisser vide : il sera demandé au premier démarrage."
                        ));
                    ui.end_row();

                    ui.label("Confirmation");
                    champ(ui, &mut p.confirmation, "", true);
                    ui.end_row();

                    ui.label("Nom de la machine");
                    let proposition = if p.identifiant.trim().is_empty() {
                        "ex. cle-lucas".to_string()
                    } else {
                        crate::config::machine_par_defaut(p.identifiant.trim())
                    };
                    champ(ui, &mut p.machine, &proposition, false);
                    ui.end_row();

                    ui.label("Clavier");
                    egui::ComboBox::from_id_salt("clavier")
                        .width(ui.available_width())
                        .selected_text(CLAVIERS[p.clavier].1)
                        .show_ui(ui, |ui| {
                            for (i, c) in CLAVIERS.iter().enumerate() {
                                ui.selectable_value(&mut p.clavier, i, c.1);
                            }
                        });
                    ui.end_row();

                    ui.label("Langue");
                    egui::ComboBox::from_id_salt("langue")
                        .width(ui.available_width())
                        .selected_text(LANGUES[p.langue].1)
                        .show_ui(ui, |ui| {
                            for (i, l) in LANGUES.iter().enumerate() {
                                ui.selectable_value(&mut p.langue, i, l.1);
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.checkbox(&mut p.chiffrer, "Chiffrer la clé (recommandé)");
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Une phrase secrète sera choisie au premier démarrage, puis demandée \
                             à chaque démarrage. Notez-la !",
                        )
                        .small()
                        .weak(),
                    )
                    .wrap(),
                );
            });
            ui.checkbox(verifier, "Vérifier la clé après l'écriture (relecture)");
        });
    }

    /// Zone d'état : progression, résultat, ou ce qu'il reste à remplir.
    fn ui_etat(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = &self.travail {
            ui.label(egui::RichText::new(&t.etape).family(theme::gras()));
            ui.add(
                egui::ProgressBar::new(t.fraction)
                    .text(&t.detail)
                    .fill(theme::accent(ui))
                    .desired_height(24.0)
                    .corner_radius(6)
                    .animate(true),
            );
        } else if let Some(r) = &self.resultat {
            match r {
                Ok(m) => Self::encadre(ui, theme::vert(ui), &format!("✔  {m}")),
                Err(m) if m == ANNULE => Self::encadre(ui, ui.visuals().weak_text_color(), m),
                Err(m) => Self::encadre(ui, ui.visuals().error_fg_color, m),
            }
        } else if let Some(x) = self.obstacles().first() {
            ui.label(egui::RichText::new(format!("Pour continuer : {x}")).weak());
        }
    }

    fn ui_confirmation(&mut self, ctx: &egui::Context) {
        let Some(d) = self.disque.clone() else {
            self.confirmation = false;
            return;
        };
        let mut choix = None;
        egui::Modal::new(egui::Id::unique("confirmation")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading("Effacer la clé ?");
            ui.add_space(8.0);
            ui.label("Toutes les données de ce disque seront définitivement effacées :");
            ui.add_space(6.0);
            Self::encadre(ui, ui.visuals().warn_fg_color, &d.libelle());
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let rouge = ui.visuals().error_fg_color;
                let b = egui::Button::new(
                    egui::RichText::new("Effacer et écrire")
                        .family(theme::gras())
                        .color(egui::Color32::WHITE),
                )
                .fill(rouge)
                .min_size(egui::vec2(150.0, 34.0));
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
                self.confirmation = false;
                self.demarrer(ctx);
            }
            Some(false) => self.confirmation = false,
            None => {}
        }
    }

    /// Barre du bas, toujours visible : bouton principal et liens.
    fn ui_pied(&mut self, ui: &mut egui::Ui) {
        self.ui_etat(ui);
        ui.add_space(4.0);
        let largeur = ui.available_width();
        if let Some(t) = &self.travail {
            if ui
                .add(egui::Button::new("Annuler").min_size(egui::vec2(largeur, 40.0)))
                .clicked()
            {
                t.annuler.store(true, Ordering::Relaxed);
            }
        } else {
            let pret = self.obstacles().is_empty();
            let b = egui::Button::new(
                egui::RichText::new("Fabriquer la clé")
                    .family(theme::gras())
                    .size(16.0)
                    .color(egui::Color32::WHITE),
            )
            .fill(theme::accent(ui))
            .corner_radius(8);
            let r = ui
                .add_enabled_ui(pret, |ui| ui.add_sized([largeur, 42.0], b))
                .inner;
            if r.clicked() {
                self.resultat = None;
                self.confirmation = true;
            }
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(env!("VERSION_APPLI")).small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.hyperlink_to(
                    egui::RichText::new("Aide et sources").small(),
                    format!("https://github.com/{}", releases::DEPOT),
                );
            });
        });
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

        let fond = ui.visuals().panel_fill;
        let marges = egui::Frame::new().fill(fond).inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 10,
            bottom: 14,
        });
        egui::Panel::bottom("pied")
            .frame(marges)
            .show_separator_line(true)
            .show(ui, |ui| self.ui_pied(ui));

        let occupe = self.travail.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(fond))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 20,
                            right: 20,
                            top: 18,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            self.ui_entete(ui);
                            ui.add_space(12.0);
                            ui.add_enabled_ui(!occupe, |ui| {
                                self.ui_cle(ui);
                                ui.add_space(10.0);
                                self.ui_image(ui);
                                ui.add_space(10.0);
                                self.ui_perso(ui);
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
