use eframe::egui;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const PORT: u16 = 8800;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([380.0, 480.0])
            .with_resizable(false)
            .with_title("rVNC"),
        ..Default::default()
    };

    eframe::run_native("rVNC", options, Box::new(|cc| {
        setup_theme(&cc.egui_ctx);
        Ok(Box::new(App::new()))
    }))
}

fn c(r: u8, g: u8, b: u8) -> egui::Color32 { egui::Color32::from_rgb(r, g, b) }

const BG: (u8,u8,u8) = (0x2F, 0x2A, 0x58);
const BG_CARD: (u8,u8,u8) = (0x35, 0x30, 0x62);
const BG_DARK: (u8,u8,u8) = (0x25, 0x20, 0x48);
const ACCENT: (u8,u8,u8) = (0x5F, 0x68, 0x9B);
const PURPLE: (u8,u8,u8) = (0xB0, 0x5F, 0xA8);
const LAVENDER: (u8,u8,u8) = (0xC1, 0xB6, 0xD1);
const DIM: (u8,u8,u8) = (0x52, 0x48, 0x6D);
const GREEN: (u8,u8,u8) = (0x80, 0xCB, 0xC4);
const RED: (u8,u8,u8) = (0xE7, 0x4C, 0x3C);
const CYAN: (u8,u8,u8) = (0x26, 0xC6, 0xDA);

fn ct(t: (u8,u8,u8)) -> egui::Color32 { c(t.0, t.1, t.2) }

fn setup_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.visuals.dark_mode = true;
    style.visuals.panel_fill = ct(BG);
    style.visuals.window_fill = ct(BG);
    style.visuals.extreme_bg_color = ct(BG_CARD);
    style.visuals.faint_bg_color = ct(BG_CARD);

    style.visuals.widgets.noninteractive.bg_fill = ct(BG_CARD);
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, ct(LAVENDER));
    style.visuals.widgets.inactive.bg_fill = ct(BG_CARD);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, ct(LAVENDER));
    style.visuals.widgets.hovered.bg_fill = ct(ACCENT);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    style.visuals.widgets.active.bg_fill = ct(PURPLE);
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    style.visuals.selection.bg_fill = ct(ACCENT);

    let r = egui::Rounding::same(8.0);
    style.visuals.widgets.noninteractive.rounding = r;
    style.visuals.widgets.inactive.rounding = r;
    style.visuals.widgets.hovered.rounding = r;
    style.visuals.widgets.active.rounding = r;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(16.0, 8.0);

    ctx.set_style(style);
}

#[derive(Clone)]
struct LogEntry {
    time: String,
    icon: &'static str,
    color: (u8,u8,u8),
    msg: String,
}

#[derive(Clone)]
struct State {
    streaming: bool,
    phone_connected: bool,
    phone_res: String,
    fps: u32,
    quality: u32,
    desktop: u32,
    mirror: bool,
    log: Vec<LogEntry>,
}

struct App {
    state: Arc<Mutex<State>>,
    open_cmd: String,
}

impl App {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(State {
            streaming: false,
            phone_connected: false,
            phone_res: "—".into(),
            fps: 60,
            quality: 18,
            desktop: 7,
            mirror: false,
            log: vec![LogEntry {
                time: now(), icon: "◆", color: ACCENT, msg: "rVNC готов к работе".into()
            }],
        }));

        let s = state.clone();
        thread::spawn(move || loop {
            poll_status(&s);
            thread::sleep(Duration::from_secs(2));
        });

        Self { state, open_cmd: "firefox".into() }
    }
}

fn now() -> String {
    let output = Command::new("date").arg("+%H:%M:%S").output().ok();
    output.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or("--:--:--".into())
}

fn poll_status(state: &Arc<Mutex<State>>) {
    let phone = Command::new("adb").args(["devices"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .any(|l| l.contains("device") && !l.contains("List")))
        .unwrap_or(false);

    let res = if phone {
        Command::new("adb").args(["shell", "wm", "size"]).output().ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).split(':').nth(1)
                .map(|s| s.trim().to_string()))
            .unwrap_or("—".into())
    } else { "—".into() };

    let streaming = std::fs::read_to_string("/tmp/rvnc/ffmpeg").is_ok()
        && Command::new("pgrep").args(["-f", "ffmpeg.*x11grab"]).output()
            .map(|o| o.status.success()).unwrap_or(false);

    let mut s = state.lock().unwrap();
    // Log connection/disconnection events
    if phone != s.phone_connected {
        let entry = if phone {
            LogEntry { time: now(), icon: "●", color: GREEN, msg: "Телефон подключён".into() }
        } else {
            LogEntry { time: now(), icon: "●", color: RED, msg: "Телефон отключён".into() }
        };
        s.log.push(entry);
    }
    s.phone_connected = phone;
    s.phone_res = res;
    s.streaming = streaming;
}

fn log_info(state: &Arc<Mutex<State>>, msg: &str) {
    let mut s = state.lock().unwrap();
    s.log.push(LogEntry { time: now(), icon: "→", color: ACCENT, msg: msg.into() });
    if s.log.len() > 50 { s.log.remove(0); }
}

fn log_ok(state: &Arc<Mutex<State>>, msg: &str) {
    let mut s = state.lock().unwrap();
    s.log.push(LogEntry { time: now(), icon: "✓", color: GREEN, msg: msg.into() });
    if s.log.len() > 50 { s.log.remove(0); }
}

fn log_err(state: &Arc<Mutex<State>>, msg: &str) {
    let mut s = state.lock().unwrap();
    s.log.push(LogEntry { time: now(), icon: "✗", color: RED, msg: msg.into() });
    if s.log.len() > 50 { s.log.remove(0); }
}

fn do_start(state: &Arc<Mutex<State>>) {
    let s = state.lock().unwrap().clone();

    if !s.phone_connected {
        log_err(state, "Телефон не подключён");
        return;
    }

    log_info(state, "Запуск стрима...");

    let mut args = vec![
        format!("--fps={}", s.fps),
        format!("--quality={}", s.quality),
        format!("--port={}", PORT),
    ];
    if s.mirror { args.push("--mirror".into()); }

    let result = Command::new("rvnc")
        .args(&args)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn();

    match result {
        Ok(_) => {
            if !s.mirror {
                thread::sleep(Duration::from_secs(3));
                let _ = Command::new("bash").args(["-c", &format!(
                    "wid=$(xdotool search --name Xephyr 2>/dev/null | head -1); \
                     [ -n \"$wid\" ] && bspc node $wid -d '^{}'", s.desktop
                )]).output();
                log_ok(state, &format!("Стрим → десктоп {} ({}fps q{})", s.desktop, s.fps, s.quality));
            } else {
                thread::sleep(Duration::from_secs(2));
                log_ok(state, &format!("Зеркало запущено ({}fps q{})", s.fps, s.quality));
            }
        }
        Err(e) => log_err(state, &format!("Ошибка запуска: {}", e)),
    }
}

fn do_stop(state: &Arc<Mutex<State>>) {
    log_info(state, "Остановка...");
    let _ = Command::new("rvnc").arg("stop").output();
    log_ok(state, "Стрим остановлен");
}

fn do_open(state: &Arc<Mutex<State>>, cmd: &str) {
    log_info(state, &format!("Открытие {}...", cmd));
    let result = Command::new("rvnc").args(["open", cmd]).output();
    match result {
        Ok(o) if o.status.success() => log_ok(state, &format!("{} открыт на телефоне", cmd)),
        _ => log_err(state, &format!("Не удалось открыть {}", cmd)),
    }
}

fn card(ui: &mut egui::Ui, color: (u8,u8,u8), content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .inner_margin(14.0)
        .rounding(10.0)
        .fill(ct(color))
        .show(ui, content);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));

        let white = egui::Color32::WHITE;

        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_size();
            let s = self.state.lock().unwrap().clone();

            // === Header ===
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("rVNC").size(22.0).color(ct(PURPLE)).strong());
                ui.add_space(6.0);
            });

            // === Status ===
            card(ui, BG_CARD, |ui| {
                ui.columns(2, |cols| {
                    // Phone
                    cols[0].horizontal(|ui| {
                        let (icon, color, text) = if s.phone_connected {
                            ("●", GREEN, "Подключён")
                        } else {
                            ("○", RED, "Нет телефона")
                        };
                        ui.label(egui::RichText::new(icon).color(ct(color)).size(12.0));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(text).color(ct(LAVENDER)).size(12.0));
                            if s.phone_connected {
                                ui.label(egui::RichText::new(&s.phone_res).color(ct(DIM)).size(11.0));
                            }
                        });
                    });

                    // Stream
                    cols[1].horizontal(|ui| {
                        let (icon, color, text) = if s.streaming {
                            ("●", GREEN, "Стримит")
                        } else {
                            ("○", DIM, "Неактивен")
                        };
                        ui.label(egui::RichText::new(icon).color(ct(color)).size(12.0));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(text).color(ct(LAVENDER)).size(12.0));
                            if s.streaming {
                                ui.label(egui::RichText::new(format!("{}fps q{}", s.fps, s.quality)).color(ct(DIM)).size(11.0));
                            }
                        });
                    });
                });
            });

            ui.add_space(6.0);

            // === Settings (only when not streaming) ===
            if !s.streaming {
                card(ui, BG_CARD, |ui| {
                    let mut st = self.state.lock().unwrap();

                    // Desktop
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Десктоп").color(ct(DIM)).size(12.0));
                        ui.add_space(4.0);
                        for d in 5..=10u32 {
                            let sel = st.desktop == d;
                            let btn = egui::Button::new(
                                egui::RichText::new(d.to_string()).size(12.0)
                                    .color(if sel { white } else { ct(LAVENDER) })
                            ).fill(if sel { ct(ACCENT) } else { ct(BG_DARK) })
                             .min_size(egui::vec2(26.0, 26.0));
                            if ui.add(btn).clicked() { st.desktop = d; }
                        }
                    });

                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("FPS").color(ct(DIM)).size(12.0));
                        ui.add(egui::Slider::new(&mut st.fps, 15..=120));
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Качество").color(ct(DIM)).size(12.0));
                        ui.add(egui::Slider::new(&mut st.quality, 1..=40));
                    });
                    ui.checkbox(&mut st.mirror, egui::RichText::new("Зеркало").color(ct(LAVENDER)).size(12.0));
                });

                ui.add_space(6.0);
            }

            // === Main button ===
            ui.vertical_centered(|ui| {
                if !s.streaming {
                    let can = s.phone_connected;
                    let color = if can { ct(GREEN).linear_multiply(0.8) } else { ct(DIM) };
                    let btn = egui::Button::new(
                        egui::RichText::new("▶  Запустить").size(15.0).color(white)
                    ).fill(color).rounding(10.0)
                     .min_size(egui::vec2(ui.available_width() - 28.0, 42.0));

                    if ui.add_enabled(can, btn).clicked() {
                        let state = self.state.clone();
                        thread::spawn(move || do_start(&state));
                    }
                } else {
                    let btn = egui::Button::new(
                        egui::RichText::new("■  Остановить").size(15.0).color(white)
                    ).fill(ct(RED).linear_multiply(0.8)).rounding(10.0)
                     .min_size(egui::vec2(ui.available_width() - 28.0, 42.0));

                    if ui.add(btn).clicked() {
                        let state = self.state.clone();
                        thread::spawn(move || do_stop(&state));
                    }
                }
            });

            // === Open apps (when streaming, not mirror) ===
            if s.streaming && !s.mirror {
                ui.add_space(6.0);
                card(ui, BG_CARD, |ui| {
                    ui.label(egui::RichText::new("Приложения").color(ct(LAVENDER)).size(12.0));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        for (app, icon) in [("firefox", "🌐"), ("brave", "🦁"), ("alacritty", "▸"), ("mpv", "▶")] {
                            let btn = egui::Button::new(
                                egui::RichText::new(format!("{} {}", icon, app)).size(11.0).color(ct(LAVENDER))
                            ).fill(ct(BG_DARK)).min_size(egui::vec2(0.0, 28.0));
                            if ui.add(btn).clicked() {
                                let state = self.state.clone();
                                let app = app.to_string();
                                thread::spawn(move || do_open(&state, &app));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.open_cmd)
                                .hint_text("команда...").desired_width(ui.available_width() - 90.0)
                        );
                        let btn = egui::Button::new(
                            egui::RichText::new("Открыть").size(11.0).color(white)
                        ).fill(ct(ACCENT)).min_size(egui::vec2(70.0, 28.0));

                        if ui.add(btn).clicked() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            let cmd = self.open_cmd.clone();
                            let state = self.state.clone();
                            thread::spawn(move || do_open(&state, &cmd));
                        }
                    });
                });
            }

            // === Log ===
            ui.add_space(6.0);
            card(ui, BG_DARK, |ui| {
                let s = self.state.lock().unwrap();
                egui::ScrollArea::vertical()
                    .max_height(80.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &s.log {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&entry.time)
                                    .color(ct(DIM)).size(10.0).monospace());
                                ui.label(egui::RichText::new(entry.icon)
                                    .color(ct(entry.color)).size(10.0));
                                ui.label(egui::RichText::new(&entry.msg)
                                    .color(ct(LAVENDER)).size(11.0).monospace());
                            });
                        }
                    });
            });
        });
    }
}
