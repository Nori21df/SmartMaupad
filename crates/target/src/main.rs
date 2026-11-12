//! SmartMaupad Target: nhận chuột và bàn phím từ app control rồi bơm vào máy này.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod inject;
mod server;

use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui;
use serde::{Deserialize, Serialize};
use smartmaupad_shared::config;
use smartmaupad_shared::protocol::DEFAULT_PORT;

use server::{Server, Shared};

const CONFIG_FILE: &str = "target.toml";

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Config {
    port: u16,
    /// Mật khẩu đã mã hoá DPAPI, dạng hex.
    password: String,
    log_only: bool,
    autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, password: String::new(), log_only: false, autostart: false }
    }
}

struct LocalIp {
    ip: IpAddr,
    interface: String,
    tailscale: bool,
}

fn local_ips() -> Vec<LocalIp> {
    let mut ips: Vec<LocalIp> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(v4) if !v4.is_link_local() => {
                let [a, b, ..] = v4.octets();
                Some(LocalIp {
                    ip: IpAddr::V4(v4),
                    interface: iface.name,
                    // Dải CGNAT 100.64.0.0/10 mà Tailscale cấp cho các máy trong tailnet.
                    tailscale: a == 100 && (b & 0xC0) == 64,
                })
            }
            _ => None,
        })
        .collect();
    ips.sort_by_key(|ip| !ip.tailscale);
    ips
}

fn install_fonts(ctx: &egui::Context) {
    // Font mặc định của egui thiếu nhiều dấu tiếng Việt; dùng Segoe UI của Windows.
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    let Ok(bytes) = std::fs::read(format!("{windir}\\Fonts\\segoeui.ttf")) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("segoe".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "segoe".to_owned());
    fonts.families.entry(egui::FontFamily::Monospace).or_default().push("segoe".to_owned());
    ctx.set_fonts(fonts);
}

struct App {
    port: String,
    password: String,
    log_only: bool,
    autostart: bool,
    shared: Arc<Shared>,
    server: Option<Server>,
    error: Option<String>,
    ips: Vec<LocalIp>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let cfg: Config = config::load(CONFIG_FILE);
        let mut app = Self {
            port: cfg.port.to_string(),
            password: config::unprotect(&cfg.password).unwrap_or_default(),
            log_only: cfg.log_only,
            autostart: cfg.autostart,
            shared: Shared::new(cc.egui_ctx.clone(), cfg.log_only),
            server: None,
            error: None,
            ips: local_ips(),
        };
        if app.autostart && !app.password.is_empty() {
            app.start();
        }
        app
    }

    fn save_config(&self) {
        let cfg = Config {
            port: self.port.trim().parse().unwrap_or(DEFAULT_PORT),
            password: config::protect(&self.password),
            log_only: self.log_only,
            autostart: self.autostart,
        };
        if let Err(e) = config::save(CONFIG_FILE, &cfg) {
            self.shared.log(format!("không lưu được cấu hình: {e}"));
        }
    }

    fn start(&mut self) {
        self.error = None;
        let port = match self.port.trim().parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                self.error = Some("Cổng phải là số từ 1 đến 65535.".to_owned());
                return;
            }
        };
        if self.password.is_empty() {
            self.error = Some("Hãy đặt mật khẩu trước khi bật nghe.".to_owned());
            return;
        }
        match Server::start(port, self.password.clone(), self.shared.clone()) {
            Ok(server) => {
                self.server = Some(server);
                self.shared.log(format!("đang nghe trên cổng {port}"));
                self.save_config();
            }
            Err(e) => self.error = Some(format!("Không mở được cổng {port}: {e}")),
        }
    }

    fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            server.stop();
            self.shared.log("đã tắt nghe");
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("SmartMaupad — Target");
            ui.add_space(6.0);

            let running = self.server.is_some();
            egui::Grid::new("form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Cổng");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut self.port).desired_width(80.0),
                );
                ui.end_row();

                ui.label("Mật khẩu");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(200.0),
                );
                ui.end_row();
            });

            if ui.checkbox(&mut self.log_only, "Chỉ ghi log, không bơm input").changed() {
                self.shared.log_only.store(self.log_only, Ordering::Relaxed);
                self.save_config();
            }
            if ui.checkbox(&mut self.autostart, "Tự bật nghe khi mở app").changed() {
                self.save_config();
            }
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if running {
                    if ui.button("Tắt nghe").clicked() {
                        self.stop();
                    }
                    match self.shared.peer() {
                        Some(peer) => ui.colored_label(
                            egui::Color32::from_rgb(60, 180, 90),
                            format!("Đang được điều khiển bởi {}", peer.ip()),
                        ),
                        None => ui.label("Đang chờ control kết nối…"),
                    };
                } else {
                    if ui.button("Bật nghe").clicked() {
                        self.start();
                    }
                    ui.label("Đang tắt");
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("IP của máy này");
                if ui.small_button("Làm mới").clicked() {
                    self.ips = local_ips();
                }
            });
            if self.ips.is_empty() {
                ui.label("Không tìm thấy card mạng nào đang hoạt động.");
            }
            for ip in &self.ips {
                let note = if ip.tailscale { "Tailscale" } else { ip.interface.as_str() };
                ui.horizontal(|ui| {
                    ui.monospace(ip.ip.to_string());
                    ui.weak(note);
                });
            }

            ui.separator();
            ui.strong("Nhật ký");
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(
                ui,
                |ui| {
                    for line in self.shared.log_lines() {
                        ui.monospace(line);
                    }
                },
            );
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 520.0])
            .with_min_inner_size([360.0, 380.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SmartMaupad Target",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
