//! Apparence : police Inter, couleurs (thème clair et sombre, suivant le système), espacements,
//! icône. egui dessine lui-même ses éléments : sans ce réglage, la fenêtre a l'allure brute
//! d'un outil de développement.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Theme, Vec2, Visuals,
};
use std::sync::Arc;

/// Famille de la graisse « semi-gras » (titres, boutons principaux).
pub fn gras() -> FontFamily {
    FontFamily::Name("gras".into())
}

/// Couleur d'accent (bleu de l'icône), selon le thème.
pub fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x4a, 0x86, 0xd4)
    } else {
        Color32::from_rgb(0x1d, 0x4f, 0x91)
    }
}

/// Fond des cartes (sections).
pub fn fond_carte(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x27, 0x2b, 0x33)
    } else {
        Color32::WHITE
    }
}

pub fn bord_carte(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x36, 0x3c, 0x47)
    } else {
        Color32::from_rgb(0xdd, 0xe2, 0xea)
    }
}

pub fn vert(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x5c, 0xc2, 0x7a)
    } else {
        Color32::from_rgb(0x1f, 0x7a, 0x3e)
    }
}

/// Fond teinté (messages de réussite ou d'erreur).
pub fn teinte(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 28)
}

fn polices(ctx: &egui::Context) {
    let mut p = FontDefinitions::default();
    p.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../ressources/Inter-Regular.otf"
        ))),
    );
    p.font_data.insert(
        "inter-gras".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../ressources/Inter-SemiBold.otf"
        ))),
    );
    // Inter d'abord ; les polices d'egui restent en secours (symboles).
    let secours = p
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    p.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    let mut g = vec!["inter-gras".to_string()];
    g.extend(secours);
    p.families.insert(gras(), g);
    ctx.set_fonts(p);
}

fn couleurs(v: &mut Visuals) {
    let sombre = v.dark_mode;
    let c = |r, g, b| Color32::from_rgb(r, g, b);
    let (fond, champ, bouton, survol, actif, texte, bord, acc) = if sombre {
        (
            c(0x1e, 0x21, 0x27),
            c(0x1a, 0x1d, 0x22),
            c(0x33, 0x39, 0x44),
            c(0x3c, 0x43, 0x50),
            c(0x46, 0x4e, 0x5c),
            c(0xe6, 0xe9, 0xee),
            c(0x45, 0x4c, 0x58),
            c(0x4a, 0x86, 0xd4),
        )
    } else {
        (
            c(0xf3, 0xf5, 0xf8),
            c(0xff, 0xff, 0xff),
            c(0xe9, 0xed, 0xf2),
            c(0xdf, 0xe5, 0xed),
            c(0xd3, 0xdb, 0xe6),
            c(0x1f, 0x29, 0x33),
            c(0xc9, 0xd1, 0xdc),
            c(0x1d, 0x4f, 0x91),
        )
    };
    let r = CornerRadius::same(6);
    v.panel_fill = fond;
    v.window_fill = if sombre {
        c(0x27, 0x2b, 0x33)
    } else {
        Color32::WHITE
    };
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = champ;
    v.text_edit_bg_color = Some(champ);
    v.faint_bg_color = fond;
    v.hyperlink_color = acc;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 90);
    v.selection.stroke = Stroke::new(1.0, acc);

    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, texte);
    w.noninteractive.bg_stroke = Stroke::new(1.0, bord);
    w.noninteractive.corner_radius = r;
    for (etat, fill) in [
        (&mut w.inactive, bouton),
        (&mut w.hovered, survol),
        (&mut w.active, actif),
        (&mut w.open, survol),
    ] {
        etat.weak_bg_fill = fill;
        etat.bg_fill = fill;
        etat.corner_radius = r;
        etat.fg_stroke = Stroke::new(1.0, texte);
        etat.expansion = 0.0;
    }
    w.inactive.bg_stroke = Stroke::new(1.0, bord);
    w.hovered.bg_stroke = Stroke::new(1.0, acc);
    w.active.bg_stroke = Stroke::new(1.5, acc);
    w.open.bg_stroke = Stroke::new(1.0, acc);
}

/// Applique le thème (police, couleurs des deux thèmes, tailles, espacements).
/// Clair ou sombre selon le système ; `FABRIQUE_CLE_THEME=clair|sombre` pour forcer.
pub fn appliquer(ctx: &egui::Context) {
    polices(ctx);
    match std::env::var("FABRIQUE_CLE_THEME").as_deref() {
        Ok("clair") => ctx.set_theme(egui::ThemePreference::Light),
        Ok("sombre") => ctx.set_theme(egui::ThemePreference::Dark),
        _ => ctx.set_theme(egui::ThemePreference::System),
    }
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |s| {
            s.visuals = theme.default_visuals();
            couleurs(&mut s.visuals);
            s.text_styles = [
                (TextStyle::Heading, FontId::new(22.0, gras())),
                (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(14.5, FontFamily::Proportional),
                ),
                (
                    TextStyle::Small,
                    FontId::new(12.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(13.0, FontFamily::Monospace),
                ),
            ]
            .into();
            let sp = &mut s.spacing;
            sp.item_spacing = Vec2::new(8.0, 9.0);
            sp.button_padding = Vec2::new(12.0, 6.0);
            sp.interact_size = Vec2::new(40.0, 30.0);
            sp.combo_height = 300.0;
            sp.menu_margin = Margin::same(6);
            sp.window_margin = Margin::same(18);
        });
    }
}

/// Icône de l'application (PNG embarqué), en RGBA.
pub fn icone() -> Option<(Vec<u8>, u32, u32)> {
    let d = png::Decoder::new(std::io::Cursor::new(
        &include_bytes!("../paquet/fabrique-cle.png")[..],
    ));
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some((buf, info.width, info.height))
}
