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
use smartmaupad_ui as ui;

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
        ui::install(&cc.egui_ctx);
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
        ui::page(ctx, "Target", |ui| {
            let palette = ui::Palette::of(ui.visuals());
            let running = self.server.is_some();

            ui::card(ui, |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    ui::row(ui, "Cổng", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.port).desired_width(90.0));
                    });
                    ui.separator();
                    ui::row(ui, "Mật khẩu", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.password)
                                .password(true)
                                .desired_width(190.0),
                        );
                    });
                });
            });
            ui.add_space(4.0);

            ui::card(ui, |ui| {
                let mut changed = false;
                ui::row(ui, "Chỉ ghi log, không bơm input", |ui| {
                    changed |= ui::toggle(ui, &mut self.log_only).changed();
                });
                ui.separator();
                ui::row(ui, "Tự bật nghe khi mở app", |ui| {
                    changed |= ui::toggle(ui, &mut self.autostart).changed();
                });
                if changed {
                    self.shared.log_only.store(self.log_only, Ordering::Relaxed);
                    self.save_config();
                }
            });
            ui.add_space(4.0);

            ui::card(ui, |ui| {
                let status = match (running, self.shared.peer()) {
                    (false, _) => egui::RichText::new("Đang tắt").color(palette.text_secondary),
                    (true, None) => egui::RichText::new("Đang chờ control kết nối…"),
                    (true, Some(peer)) => {
                        egui::RichText::new(format!("Đang được điều khiển bởi {}", peer.ip()))
                            .color(palette.success)
                    }
                };
                ui::row(ui, status, |ui| {
                    if running {
                        if ui::button(ui, "Tắt nghe").clicked() {
                            self.stop();
                        }
                    } else if ui::primary_button(ui, "Bật nghe").clicked() {
                        self.start();
                    }
                });
            });
            if let Some(error) = &self.error {
                ui.colored_label(palette.error, error);
            }

            ui::section(ui, "IP của máy này");
            ui::card(ui, |ui| {
                if self.ips.is_empty() {
                    ui::row(ui, "Không tìm thấy card mạng nào đang hoạt động.", |_| {});
                }
                for (index, ip) in self.ips.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    let (note, color) = if ip.tailscale {
                        ("Tailscale", palette.accent)
                    } else {
                        (ip.interface.as_str(), palette.text_secondary)
                    };
                    ui::row(ui, egui::RichText::new(ip.ip.to_string()).monospace(), |ui| {
                        ui.colored_label(color, note);
                    });
                }
            });
            if ui.link("Làm mới danh sách IP").clicked() {
                self.ips = local_ips();
            }

            ui::section(ui, "Nhật ký");
            ui::card(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.shared.log_lines() {
                            ui.label(egui::RichText::new(line).small());
                        }
                    });
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 680.0])
            .with_min_inner_size([400.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SmartMaupad Target",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
