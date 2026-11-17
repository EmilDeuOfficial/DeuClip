//! Central design: dark grays, green accent. No default egui blues.

use eframe::egui::{
    self, vec2, Button, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Shape,
    Stroke, StrokeKind, TextStyle, Visuals,
};

pub const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);
pub const CARD: Color32 = Color32::from_rgb(0x25, 0x25, 0x25);
pub const INPUT: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
pub const HOVER_BG: Color32 = Color32::from_rgb(0x38, 0x38, 0x38);
pub const ACCENT: Color32 = Color32::from_rgb(0x22, 0xC5, 0x5E);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x16, 0xA3, 0x4A);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x0B, 0x1F, 0x12);
pub const TEXT: Color32 = Color32::from_rgb(0xE5, 0xE5, 0xE5);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
pub const ERROR: Color32 = Color32::from_rgb(0xC8, 0x6B, 0x66);

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    let radius = CornerRadius::same(6);

    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = INPUT;
    v.faint_bg_color = CARD;
    v.text_edit_bg_color = Some(INPUT);
    v.code_bg_color = INPUT;
    v.hyperlink_color = ACCENT;
    v.error_fg_color = ERROR;
    v.warn_fg_color = TEXT_DIM;
    v.weak_text_color = Some(TEXT_DIM);
    v.slider_trailing_fill = true;

    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.5, ACCENT);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);

    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.weak_bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = radius;

    v.widgets.inactive.bg_fill = INPUT;
    v.widgets.inactive.weak_bg_fill = INPUT;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = radius;

    v.widgets.hovered.bg_fill = HOVER_BG;
    v.widgets.hovered.weak_bg_fill = HOVER_BG;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    v.widgets.hovered.corner_radius = radius;

    v.widgets.active.bg_fill = ACCENT_HOVER;
    v.widgets.active.weak_bg_fill = ACCENT_HOVER;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.5, TEXT);
    v.widgets.active.corner_radius = radius;

    v.widgets.open.bg_fill = INPUT;
    v.widgets.open.weak_bg_fill = INPUT;
    v.widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.corner_radius = radius;

    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v
}

/// Uses Segoe UI from the Windows fonts folder. If the file is missing, the egui default font stays.
fn install_font(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let path = std::path::PathBuf::from(windir).join("Fonts").join("segoeui.ttf");
    let Ok(bytes) = std::fs::read(&path) else {
        log::warn!("Segoe UI not found, using the default font");
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("segoe".to_string(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        list.insert(0, "segoe".to_string());
    }
    ctx.set_fonts(fonts);
}

/// Applies the complete design to the egui context.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    install_font(ctx);
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        // Labels cannot be selected with the mouse (text fields still can).
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = vec2(10.0, 10.0);
        style.spacing.button_padding = vec2(14.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.combo_width = 220.0;
        style.text_styles.insert(TextStyle::Body, FontId::proportional(14.5));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(14.5));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(12.5));
    });
}

pub fn clear_color() -> [f32; 4] {
    BG.to_normalized_gamma_f32()
}

/// Window icon: green dot on a dark, rounded square.
pub fn app_icon() -> egui::IconData {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32, y as f32);
            // Rounded square.
            let dx = (fx - c).abs() - (c - 10.0);
            let dy = (fy - c).abs() - (c - 10.0);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - 10.0;
            let mut px = [0u8; 4];
            if outside <= 0.0 {
                px = [0x1C, 0x1C, 0x1C, 0xFF];
                let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
                if d <= 16.0 {
                    px = [0x22, 0xC5, 0x5E, 0xFF];
                } else if d <= 20.0 {
                    px = [0x16, 0xA3, 0x4A, 0xFF];
                }
            }
            let i = ((y * N + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    egui::IconData {
        rgba,
        width: N,
        height: N,
    }
}

/// Card with a title.
pub fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(15.5).color(TEXT));
            ui.add_space(2.0);
            add(ui)
        })
        .inner
}

/// Green primary button.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0, ON_ACCENT);
        }
        w.inactive.weak_bg_fill = ACCENT;
        w.inactive.bg_fill = ACCENT;
        w.hovered.weak_bg_fill = ACCENT_HOVER;
        w.hovered.bg_fill = ACCENT_HOVER;
        w.active.weak_bg_fill = ACCENT_HOVER;
        w.active.bg_fill = ACCENT_HOVER;
        ui.add(Button::new(RichText::new(text).color(ON_ACCENT).strong()))
    })
    .inner
}

/// Entry in a dropdown. The selected entry has a green background.
pub fn option_row(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let color = if selected { ON_ACCENT } else { TEXT };
    let mut button = Button::new(RichText::new(text).color(color)).min_size(vec2(
        ui.available_width().max(160.0),
        26.0,
    ));
    if selected {
        button = button.fill(ACCENT);
    } else {
        button = button.frame_when_inactive(false);
    }
    ui.add(button)
}

/// Dropdown with a green selection. Returns true when the value changed.
pub fn dropdown<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    options: &[(T, String)],
    value: &mut T,
) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| "Unbekannt".to_string());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                if option_row(ui, v == value, label).clicked() {
                    *value = v.clone();
                    changed = true;
                    ui.close();
                }
            }
        });
    changed
}

/// Checkbox in the accent design.
pub fn checkbox(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let (rect, mut resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        let label_resp = ui.add(egui::Label::new(RichText::new(label).color(TEXT)).sense(Sense::click()));
        if resp.clicked() || label_resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        let hovered = resp.hovered() || label_resp.hovered();
        let fill = if *on { ACCENT } else { INPUT };
        let stroke = if *on || hovered { ACCENT } else { BORDER };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        if *on {
            let p = |fx: f32, fy: f32| rect.min + vec2(rect.width() * fx, rect.height() * fy);
            painter.add(Shape::line(
                vec![p(0.24, 0.54), p(0.43, 0.72), p(0.77, 0.30)],
                Stroke::new(2.0, ON_ACCENT),
            ));
        }
        resp
    })
    .inner
}

/// Status dot.
pub fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}
