//! Giao diện kiểu Windows 11 (Fluent) dùng chung cho hai app: bảng màu sáng/tối
//! theo hệ thống, font Segoe UI, thẻ bo góc, hàng cài đặt, công tắc, nút accent.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Button, Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily,
    FontId, Frame, Layout, Margin, Response, RichText, Sense, Stroke, StrokeKind, TextStyle, Theme,
    Ui, Visuals, WidgetText,
};

const SEMIBOLD: &str = "semibold";
const CONTROL_RADIUS: u8 = 4;
const CARD_RADIUS: u8 = 8;
const ROW_HEIGHT: f32 = 32.0;
const BUTTON_SIZE: egui::Vec2 = egui::vec2(112.0, 32.0);

/// Bảng màu Fluent cho một chế độ sáng hoặc tối.
pub struct Palette {
    pub background: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    pub control_stroke: Color32,
    pub input: Color32,
    pub rail: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub error: Color32,
}

const LIGHT: Palette = Palette {
    background: Color32::from_rgb(0xF3, 0xF3, 0xF3),
    card: Color32::from_rgb(0xFB, 0xFB, 0xFB),
    card_stroke: Color32::from_rgb(0xE5, 0xE5, 0xE5),
    divider: Color32::from_rgb(0xEA, 0xEA, 0xEA),
    text: Color32::from_rgb(0x1B, 0x1B, 0x1B),
    text_secondary: Color32::from_rgb(0x5D, 0x5D, 0x5D),
    control: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    control_hover: Color32::from_rgb(0xF2, 0xF2, 0xF2),
    control_pressed: Color32::from_rgb(0xEB, 0xEB, 0xEB),
    control_stroke: Color32::from_rgb(0xD6, 0xD6, 0xD6),
    input: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    rail: Color32::from_rgb(0xC8, 0xC8, 0xC8),
    accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
    on_accent: Color32::WHITE,
    success: Color32::from_rgb(0x0F, 0x7B, 0x0F),
    error: Color32::from_rgb(0xC4, 0x2B, 0x1C),
};

const DARK: Palette = Palette {
    background: Color32::from_rgb(0x20, 0x20, 0x20),
    card: Color32::from_rgb(0x2B, 0x2B, 0x2B),
    card_stroke: Color32::from_rgb(0x1D, 0x1D, 0x1D),
    divider: Color32::from_rgb(0x3A, 0x3A, 0x3A),
    text: Color32::WHITE,
    text_secondary: Color32::from_rgb(0xC5, 0xC5, 0xC5),
    control: Color32::from_rgb(0x37, 0x37, 0x37),
    control_hover: Color32::from_rgb(0x3E, 0x3E, 0x3E),
    control_pressed: Color32::from_rgb(0x32, 0x32, 0x32),
    control_stroke: Color32::from_rgb(0x45, 0x45, 0x45),
    input: Color32::from_rgb(0x1F, 0x1F, 0x1F),
    rail: Color32::from_rgb(0x6A, 0x6A, 0x6A),
    accent: Color32::from_rgb(0x60, 0xCD, 0xFF),
    on_accent: Color32::BLACK,
    success: Color32::from_rgb(0x6C, 0xCB, 0x5F),
    error: Color32::from_rgb(0xFF, 0x99, 0xA4),
};

impl Palette {
    pub fn of(visuals: &Visuals) -> &'static Palette {
        if visuals.dark_mode {
            &DARK
        } else {
            &LIGHT
        }
    }
}

/// Cài font và style Windows 11 cho cả chế độ sáng lẫn tối; app tự theo
/// chế độ của hệ thống.
pub fn install(ctx: &Context) {
    install_fonts(ctx);
    for (theme, palette) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.style_mut_of(theme, |style| apply_style(style, palette, theme == Theme::Dark));
    }
}

fn install_fonts(ctx: &Context) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    let load = |file: &str| std::fs::read(format!("{windir}\\Fonts\\{file}")).ok();

    let mut fonts = FontDefinitions::default();
    // Font mặc định của egui thiếu nhiều dấu tiếng Việt; Segoe UI thì đủ.
    if let Some(bytes) = load("segoeui.ttf") {
        fonts.font_data.insert("segoe".to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".to_owned());
        fonts.families.entry(FontFamily::Monospace).or_default().push("segoe".to_owned());
    }
    // Family semibold phải luôn tồn tại vì style tiêu đề tham chiếu tới nó.
    let mut semibold = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bytes) = load("seguisb.ttf") {
        fonts.font_data.insert("segoe-semibold".to_owned(), Arc::new(FontData::from_owned(bytes)));
        semibold.insert(0, "segoe-semibold".to_owned());
    }
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    ctx.set_fonts(fonts);
}

fn apply_style(style: &mut egui::Style, p: &Palette, dark: bool) {
    style.text_styles = [
        (TextStyle::Heading, FontId::new(24.0, FontFamily::Name(SEMIBOLD.into()))),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(8.0, 6.0);
    spacing.button_padding = egui::vec2(12.0, 5.0);
    spacing.interact_size = egui::vec2(40.0, ROW_HEIGHT);
    spacing.slider_width = 150.0;
    spacing.slider_rail_height = 4.0;

    let mut visuals = if dark { Visuals::dark() } else { Visuals::light() };
    visuals.panel_fill = p.background;
    visuals.window_fill = p.card;
    visuals.window_stroke = Stroke::new(1.0, p.card_stroke);
    visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(CARD_RADIUS);
    visuals.extreme_bg_color = p.input;
    visuals.faint_bg_color = p.card;
    visuals.selection.bg_fill = p.accent;
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.slider_trailing_fill = true;

    let radius = CornerRadius::same(CONTROL_RADIUS);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = p.card;
    widgets.noninteractive.weak_bg_fill = p.card;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    widgets.noninteractive.corner_radius = radius;
    for (state, fill, text) in [
        (&mut widgets.inactive, p.control, p.text),
        (&mut widgets.hovered, p.control_hover, p.text),
        (&mut widgets.active, p.control_pressed, p.text_secondary),
        (&mut widgets.open, p.control_hover, p.text),
    ] {
        // bg_fill là màu ray thanh trượt; nút bấm dùng weak_bg_fill.
        state.bg_fill = p.rail;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, p.control_stroke);
        state.fg_stroke = Stroke::new(1.0, text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    style.visuals = visuals;
}

fn semibold(text: &str) -> RichText {
    RichText::new(text).family(FontFamily::Name(SEMIBOLD.into()))
}

/// Khung trang: nền Windows 11, lề rộng và tiêu đề lớn.
pub fn page(ctx: &Context, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(&ctx.style().visuals);
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.background).inner_margin(Margin::same(20)))
        .show(ctx, |ui| {
            ui.heading(title);
            ui.add_space(10.0);
            add_contents(ui);
        });
}

/// Tiêu đề nhỏ phía trên một nhóm thẻ.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(semibold(title));
}

/// Thẻ bo góc chiếm hết chiều ngang, như một nhóm trong Settings của Windows 11.
pub fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.visuals());
    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.card_stroke))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// Một hàng cài đặt: nhãn bên trái, control canh phải.
pub fn row(ui: &mut Ui, label: impl Into<WidgetText>, add_control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT);
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), add_control);
    });
}

/// Nút chính tô màu accent.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let palette = Palette::of(ui.visuals());
    ui.add(
        Button::new(RichText::new(text).color(palette.on_accent))
            .fill(palette.accent)
            .stroke(Stroke::NONE)
            .min_size(BUTTON_SIZE),
    )
}

/// Nút thường, cùng kích thước với nút chính.
pub fn button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Button::new(text).min_size(BUTTON_SIZE))
}

/// Công tắc bật/tắt kiểu Windows 11.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let palette = Palette::of(ui.visuals());
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = rect.height() / 2.0;
        let (fill, stroke, knob) = if *on {
            (palette.accent, Stroke::NONE, palette.on_accent)
        } else {
            (palette.input, Stroke::new(1.0, palette.text_secondary), palette.text_secondary)
        };
        ui.painter().rect(rect, radius, fill, stroke, StrokeKind::Inside);
        let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob_radius = if response.hovered() { 7.0 } else { 6.0 };
        ui.painter().circle_filled(egui::pos2(knob_x, rect.center().y), knob_radius, knob);
    }
    response
}
