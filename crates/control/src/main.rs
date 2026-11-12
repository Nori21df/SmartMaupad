//! SmartMaupad Control: cửa sổ quản lý. Bấm Bắt đầu để mở ô bắt chuột và
//! gửi chuột + bàn phím sang máy target.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod hotkey;
mod session;

use std::time::Duration;

use eframe::egui;
use serde::{Deserialize, Serialize};
use smartmaupad_shared::config;
use smartmaupad_shared::protocol::DEFAULT_PORT;
use smartmaupad_ui as ui;

use hotkey::Hotkey;
use session::{Event, Params, Session};

const CONFIG_FILE: &str = "control.toml";
const SENSITIVITY_RANGE: std::ops::RangeInclusive<f32> = 0.2..=3.0;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Config {
    host: String,
    port: u16,
    /// Mật khẩu đã mã hoá DPAPI, dạng hex.
    password: String,
    sensitivity: f32,
    hotkey: Hotkey,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: DEFAULT_PORT,
            password: String::new(),
            sensitivity: 1.0,
            hotkey: Hotkey::default(),
        }
    }
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Connecting,
    Active,
}

struct App {
    host: String,
    port: String,
    password: String,
    sensitivity: f32,
    hotkey: Hotkey,
    /// Đang chờ người dùng nhấn tổ hợp phím tắt mới.
    recording_hotkey: bool,
    phase: Phase,
    session: Option<Session>,
    rtt: Option<Duration>,
    /// Kết quả của lần chạy gần nhất: `Ok` là thông báo, `Err` là lỗi.
    last_result: Option<Result<String, String>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::install(&cc.egui_ctx);
        let cfg: Config = config::load(CONFIG_FILE);
        Self {
            host: cfg.host,
            port: cfg.port.to_string(),
            password: config::unprotect(&cfg.password).unwrap_or_default(),
            sensitivity: cfg.sensitivity.clamp(*SENSITIVITY_RANGE.start(), *SENSITIVITY_RANGE.end()),
            hotkey: if cfg.hotkey.is_valid() { cfg.hotkey } else { Hotkey::default() },
            recording_hotkey: false,
            phase: Phase::Idle,
            session: None,
            rtt: None,
            last_result: None,
        }
    }

    fn save_config(&mut self) {
        let cfg = Config {
            host: self.host.trim().to_owned(),
            port: self.port.trim().parse().unwrap_or(DEFAULT_PORT),
            password: config::protect(&self.password),
            sensitivity: self.sensitivity,
            hotkey: self.hotkey,
        };
        if let Err(e) = config::save(CONFIG_FILE, &cfg) {
            self.last_result = Some(Err(format!("Không lưu được cấu hình: {e}")));
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let host = self.host.trim().to_owned();
        if host.is_empty() {
            self.last_result = Some(Err("Hãy nhập IP của target.".to_owned()));
            return;
        }
        let port = match self.port.trim().parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                self.last_result = Some(Err("Cổng phải là số từ 1 đến 65535.".to_owned()));
                return;
            }
        };
        if self.password.is_empty() {
            self.last_result = Some(Err("Hãy nhập mật khẩu của target.".to_owned()));
            return;
        }

        self.last_result = None;
        self.save_config();
        self.rtt = None;
        self.recording_hotkey = false;
        self.phase = Phase::Connecting;
        let params = Params {
            host,
            port,
            password: self.password.clone(),
            sensitivity: self.sensitivity,
            hotkey: self.hotkey,
        };
        self.session = Some(Session::start(params, ctx.clone()));
    }

    fn poll_session(&mut self, ctx: &egui::Context) {
        let Some(session) = &self.session else {
            return;
        };
        let mut ended = None;
        for event in session.events.try_iter() {
            match event {
                Event::Connected => self.phase = Phase::Active,
                Event::Rtt(rtt) => self.rtt = Some(rtt),
                Event::Ended(result) => ended = Some(result),
            }
        }
        if let Some(result) = ended {
            self.session = None;
            self.phase = Phase::Idle;
            self.last_result = Some(result);
            // Ô bắt chuột vừa đóng: đưa cửa sổ quản lý trở lại trước mặt.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Khi đang ghi phím tắt, lấy tổ hợp hợp lệ đầu tiên người dùng nhấn.
    fn record_hotkey(&mut self, ctx: &egui::Context) {
        if !self.recording_hotkey {
            return;
        }
        let pressed = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => {
                    Some((*key, *modifiers))
                }
                _ => None,
            })
        });
        let Some((key, modifiers)) = pressed else {
            return;
        };
        if key == egui::Key::Escape && modifiers.is_none() {
            self.recording_hotkey = false;
        } else if let Some(hotkey) = Hotkey::from_egui(key, modifiers) {
            self.hotkey = hotkey;
            self.recording_hotkey = false;
            self.save_config();
        }
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        ui::row(ui, "IP target", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.host)
                    .hint_text("100.x.y.z")
                    .desired_width(190.0),
            );
        });
        ui.separator();
        ui::row(ui, "Cổng", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.port).desired_width(90.0));
        });
        ui.separator();
        ui::row(ui, "Mật khẩu", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.password).password(true).desired_width(190.0),
            );
        });
        ui.separator();
        ui::row(ui, "Độ nhạy chuột", |ui| {
            ui.add(egui::Slider::new(&mut self.sensitivity, SENSITIVITY_RANGE).fixed_decimals(1));
        });
        ui.separator();
        ui::row(ui, "Phím tắt dừng", |ui| {
            let text = if self.recording_hotkey {
                "Nhấn tổ hợp phím… (Esc để huỷ)".to_owned()
            } else {
                self.hotkey.to_string()
            };
            if ui
                .button(text)
                .on_hover_text("Bấm rồi nhấn Ctrl/Alt/Shift + chữ, số hoặc F1–F24")
                .clicked()
            {
                self.recording_hotkey = !self.recording_hotkey;
            }
        });
    }

    fn status_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = ui::Palette::of(ui.visuals());
        match self.phase {
            Phase::Idle => {
                let label = egui::RichText::new("Chưa kết nối").color(palette.text_secondary);
                ui::row(ui, label, |ui| {
                    if ui::primary_button(ui, "Bắt đầu").clicked() {
                        self.start(ctx);
                    }
                });
            }
            Phase::Connecting => {
                ui::row(ui, "Đang kết nối…", |ui| {
                    if ui::button(ui, "Huỷ").clicked() {
                        if let Some(session) = &self.session {
                            session.cancel();
                        }
                    }
                    ui.spinner();
                });
            }
            Phase::Active => {
                let ping = match self.rtt {
                    Some(rtt) => format!(" — ping {:.0} ms", rtt.as_secs_f64() * 1000.0),
                    None => String::new(),
                };
                let label =
                    egui::RichText::new(format!("Đang điều khiển{ping}")).color(palette.success);
                ui::row(ui, label, |_| {});
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_session(ctx);
        self.record_hotkey(ctx);

        ui::page(ctx, "Control", |ui| {
            let palette = ui::Palette::of(ui.visuals());

            ui::card(ui, |ui| {
                ui.add_enabled_ui(self.phase == Phase::Idle, |ui| self.form(ui));
            });
            ui.add_space(4.0);
            ui::card(ui, |ui| self.status_card(ui, ctx));

            match &self.last_result {
                Some(Ok(message)) => {
                    ui.label(message);
                }
                Some(Err(error)) => {
                    ui.colored_label(palette.error, error);
                }
                None => {}
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "Khi đang điều khiển, chuột bị khoá trong ô 200px. \
                     Nhấn {} trên bàn phím để dừng.",
                    self.hotkey
                ))
                .small()
                .color(palette.text_secondary),
            );
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 500.0])
            .with_min_inner_size([400.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SmartMaupad Control",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
