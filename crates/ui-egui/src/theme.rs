//! Design tokens (light and dark), interface fonts and the egui style.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

/// Syngle's accent, as the app's chrome draws it (SyngleCode's `--app-accent` in light): the
/// Project tab and the logo.
pub const BRAND: Color32 = Color32::from_rgb(0x34, 0x64, 0xB7);
pub const BRAND_DARK: Color32 = Color32::from_rgb(0x2C, 0x4B, 0x86);
/// Contextual tab group colour (Feature Layer, Labeling, Data): the diverging ramp's teal.
pub const CONTEXT: Color32 = Color32::from_rgb(0x00, 0x7A, 0x97);

/// Every colour the interface uses; widgets never hard-code colours.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub header: Color32,
    pub ribbon: Color32,
    pub tab_strip: Color32,
    pub canvas: Color32,
    pub pane: Color32,
    pub pane_header: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub checked: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub input: Color32,
    pub input_border: Color32,
    pub group_label: Color32,
    pub status_bar: Color32,
    pub selection: Color32,
    pub menu: Color32,
    pub shadow: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
    pub context: Color32,
    pub context_tint: Color32,
}

/// The token values are Syngle's (`@syngle/ui`'s colour roles, and the Graphite dark restatements
/// `apps/web/src/index.css` makes for SyngleCode), so the canvas-only view's own chrome — the
/// scale bar, north arrow, zoom buttons and attribution — is drawn in the same palette as the
/// React surface around it (#71). `--app-accent` is the app's accent, not PE's darker navy.
impl Tokens {
    pub fn light() -> Self {
        Tokens {
            dark: false,
            header: Color32::from_rgb(0xF1, 0xEE, 0xE7),
            ribbon: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            tab_strip: Color32::from_rgb(0xF1, 0xEE, 0xE7),
            canvas: Color32::from_rgb(0xF6, 0xF4, 0xEF),
            pane: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            pane_header: Color32::from_rgb(0xF1, 0xEE, 0xE7),
            border: Color32::from_rgb(0xE5, 0xE1, 0xD7),
            border_strong: Color32::from_rgb(0xD6, 0xD1, 0xC4),
            text: Color32::from_rgb(0x1B, 0x1A, 0x17),
            text_dim: Color32::from_rgb(0x4A, 0x47, 0x3F),
            text_disabled: Color32::from_rgb(0x84, 0x7E, 0x70),
            icon: Color32::from_rgb(0x4A, 0x47, 0x3F),
            hover: Color32::from_rgb(0xF2, 0xEF, 0xE8),
            pressed: Color32::from_rgb(0xEA, 0xE6, 0xDD),
            checked: Color32::from_rgb(0xE8, 0xF1, 0xFF),
            accent: Color32::from_rgb(0x34, 0x64, 0xB7),
            accent_text: Color32::from_rgb(0x2C, 0x4B, 0x86),
            on_accent: Color32::WHITE,
            input: Color32::WHITE,
            input_border: Color32::from_rgb(0xD6, 0xD1, 0xC4),
            group_label: Color32::from_rgb(0x84, 0x7E, 0x70),
            status_bar: Color32::from_rgb(0xF1, 0xEE, 0xE7),
            selection: Color32::from_rgb(0xE8, 0xF1, 0xFF),
            menu: Color32::WHITE,
            shadow: Color32::from_black_alpha(28),
            green: Color32::from_rgb(0x48, 0x8C, 0x58),
            red: Color32::from_rgb(0xAD, 0x4A, 0x4E),
            amber: Color32::from_rgb(0x8A, 0x61, 0x00),
            context: CONTEXT,
            context_tint: Color32::from_rgb(0xE6, 0xF2, 0xF4),
        }
    }

    pub fn dark() -> Self {
        Tokens {
            dark: true,
            header: Color32::from_rgb(0x24, 0x24, 0x24),
            ribbon: Color32::from_rgb(0x35, 0x35, 0x35),
            tab_strip: Color32::from_rgb(0x24, 0x24, 0x24),
            canvas: Color32::from_rgb(0x2D, 0x2D, 0x2D),
            pane: Color32::from_rgb(0x35, 0x35, 0x35),
            pane_header: Color32::from_rgb(0x24, 0x24, 0x24),
            border: Color32::from_rgb(0x4C, 0x4C, 0x4C),
            border_strong: Color32::from_rgb(0x5D, 0x5D, 0x5D),
            text: Color32::from_rgb(0xF1, 0xEC, 0xE3),
            text_dim: Color32::from_rgb(0xD8, 0xD4, 0xCC),
            text_disabled: Color32::from_rgb(0xB0, 0xAD, 0xA6),
            icon: Color32::from_rgb(0xD8, 0xD4, 0xCC),
            hover: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            pressed: Color32::from_rgb(0x45, 0x45, 0x45),
            checked: Color32::from_rgb(0x29, 0x31, 0x3F),
            accent: Color32::from_rgb(0x8A, 0xAB, 0xE7),
            accent_text: Color32::from_rgb(0x8A, 0xAB, 0xE7),
            on_accent: Color32::from_rgb(0x1B, 0x1A, 0x17),
            input: Color32::from_rgb(0x2D, 0x2D, 0x2D),
            input_border: Color32::from_rgb(0x5D, 0x5D, 0x5D),
            group_label: Color32::from_rgb(0xB0, 0xAD, 0xA6),
            status_bar: Color32::from_rgb(0x24, 0x24, 0x24),
            selection: Color32::from_rgb(0x29, 0x31, 0x3F),
            menu: Color32::from_rgb(0x35, 0x35, 0x35),
            shadow: Color32::from_black_alpha(110),
            green: Color32::from_rgb(0x86, 0xD3, 0x96),
            red: Color32::from_rgb(0xFF, 0x9F, 0x9F),
            amber: Color32::from_rgb(0xC7, 0x9B, 0x4C),
            context: Color32::from_rgb(0x00, 0x97, 0xB3),
            context_tint: Color32::from_rgb(0x1E, 0x3A, 0x3E),
        }
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        if ctx.global_style().visuals.dark_mode { Tokens::dark() } else { Tokens::light() }
    }
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Installs the interface fonts: an installed system sans-serif when available, then `extra`
/// faces (name, bytes, collection index) as fallbacks for scripts it lacks, such as CJK.
pub fn install_fonts(
    ctx: &egui::Context,
    extra: &[(String, Arc<FontData>, Option<rustgis_engine::render::fonts::CjkRegion>)],
    current: Option<rustgis_engine::render::fonts::CjkRegion>,
) {
    let f = rustgis_engine::render::fonts::ui_fonts();
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(f.regular.clone())));
    fonts.font_data.insert("ui-semibold".into(), Arc::new(FontData::from_owned(f.semibold.clone())));
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "ui".into());
        // egui's Hack face has the Georgian letters that Ubuntu Light and Noto Sans lack.
        prop.push("Hack".into());
    }
    let mut semi = vec!["ui-semibold".to_string(), "ui".to_string()];
    if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
        semi.extend(prop.iter().skip(1).cloned());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    for (name, data, region) in extra {
        fonts.font_data.insert(name.clone(), Arc::clone(data));
        let primary = region.is_some() && *region == current;
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into())] {
            if let Some(v) = fonts.families.get_mut(&fam) {
                if primary {
                    // Right after the interface face, so its regional glyph shapes win.
                    v.insert(1.min(v.len()), name.clone());
                } else {
                    v.push(name.clone());
                }
            }
        }
        // Monospace text (the status bar's bearing and pitch, code boxes) falls back to them too,
        // after Hack, so translated or CJK text in it doesn't draw as boxes.
        if let Some(v) = fonts.families.get_mut(&FontFamily::Monospace) {
            v.push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.pane;
    v.window_fill = t.menu;
    v.extreme_bg_color = t.input;
    v.faint_bg_color = t.pane_header;
    v.window_stroke = Stroke::new(1.0, t.border_strong);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    // Slider rails and checkbox wells use this fill; keep it visible on white.
    v.widgets.inactive.bg_fill = if t.dark { Color32::from_rgb(0x3A, 0x42, 0x4D) } else { Color32::from_rgb(0xDD, 0xE3, 0xEA) };
    v.slider_trailing_fill = true;
    v.widgets.inactive.weak_bg_fill = t.input;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.input_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(5);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.corner_radius = CornerRadius::same(5);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.active.corner_radius = CornerRadius::same(5);
    v.widgets.open.bg_fill = t.pressed;
    v.widgets.open.weak_bg_fill = t.pressed;
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow };
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: t.shadow };
    // Pin egui's theme to ours: the app decides when to follow the system (`dark_follows_system`).
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.text_styles.insert(egui::TextStyle::Body, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Button, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Small, regular(10.5));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(12.0));
    });
}

/// egui colour from a map colour.
pub fn c32(c: rustgis_engine::map::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

pub fn rgba(c: Color32) -> rustgis_engine::map::Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    rustgis_engine::map::Rgba::new(r, g, b, a)
}
