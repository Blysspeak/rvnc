//! rvnc-panel: the 800x360 logical dashboard that fills the phone output `rvnc-phone`.
//!
//! The output is a Hyprland headless monitor of 2400x1080 at scale 3, so every coordinate here is
//! logical pixels of an 800x360 canvas. The phone's camera cutout sits in the top-left corner of
//! the panel, so the first 90x40 px stay empty.
//!
//! Data comes from a JSON file produced outside the stream (a bun script on this PC); this binary
//! never talks to prod itself. Missing, unparsable or stale data turns the affected cards into the
//! dim `нет данных` marker instead of crashing the panel.

use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

/// Feed contract, written by the producer every 30 s:
///
/// ```json
/// {"updated":"2026-10-21T18:00:00.000Z",
///  "prod":{"state":"ok|bad|unknown","hint":"200ms"},
///  "relays":[{"name":"KZ","state":"ok","hint":"240ms"},
///            {"name":"BY","state":"bad","hint":"0/4"}],
///  "lv_ci":{"state":"unknown","hint":"..."},
///  "payments_today":{"sum":1284930,"count":1210,"delta_pct":12},
///  "refunds":{"count":4,"sum":12400},
///  "top_merchant":{"name":"acme","turnover":480000,"share_pct":37}}
/// ```
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Feed {
    /// Part of the documented contract; freshness is measured from the file mtime, not parsed.
    #[allow(dead_code)]
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    prod: Option<Pill>,
    #[serde(default)]
    relays: Vec<Pill>,
    #[serde(default)]
    lv_ci: Option<Pill>,
    #[serde(default)]
    payments_today: Option<PaymentsToday>,
    #[serde(default)]
    refunds: Option<Refunds>,
    #[serde(default)]
    top_merchant: Option<TopMerchant>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Pill {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    hint: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct PaymentsToday {
    #[serde(default)]
    sum: f64,
    #[serde(default)]
    count: f64,
    #[serde(default)]
    delta_pct: f64,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Refunds {
    #[serde(default)]
    count: f64,
    #[serde(default)]
    sum: f64,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct TopMerchant {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    turnover: f64,
    #[serde(default)]
    share_pct: f64,
}

const FEED_DEFAULT: &str = "/tmp/rvnc/feed.json";
const REFRESH: Duration = Duration::from_secs(2);
/// The producer writes every 30 s; three missed cycles make the numbers on screen lies.
const STALE: Duration = Duration::from_secs(90);
const UNKNOWN: &str = "нет данных";

// The 800x360 budget: 344 px of content after the 8 px outer margin.
const HEADER_H: f32 = 30.0;
const PILL_H: f32 = 30.0;
const PILL_GAP: f32 = 6.0;
const MONEY_H: f32 = 112.0;

fn rgb(t: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(t.0, t.1, t.2)
}

const BG: (u8, u8, u8) = (0x2F, 0x2A, 0x58);
const BG_CARD: (u8, u8, u8) = (0x35, 0x30, 0x62);
const BG_DARK: (u8, u8, u8) = (0x25, 0x20, 0x48);
const ACCENT: (u8, u8, u8) = (0x5F, 0x68, 0x9B);
const LAVENDER: (u8, u8, u8) = (0xC1, 0xB6, 0xD1);
const DIM: (u8, u8, u8) = (0x52, 0x48, 0x6D);
const GREEN: (u8, u8, u8) = (0x80, 0xCB, 0xC4);
const RED: (u8, u8, u8) = (0xE7, 0x4C, 0x3C);
const CYAN: (u8, u8, u8) = (0x26, 0xC6, 0xDA);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Ok,
    Down,
    Unknown,
}

fn kind(state: Option<&str>) -> Kind {
    match state {
        Some("ok") => Kind::Ok,
        Some("bad") => Kind::Down,
        _ => Kind::Unknown,
    }
}

fn kind_color(k: Kind) -> egui::Color32 {
    match k {
        Kind::Ok => rgb(GREEN),
        Kind::Down => rgb(RED),
        Kind::Unknown => rgb(DIM),
    }
}

/// "3736852" -> "3 736 852": grouping by a plain space, tabular via monospace.
fn grouped(n: f64) -> String {
    let v = n.round().abs() as i64;
    let digits = v.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    if n < 0.0 {
        format!("-{}", out)
    } else {
        out
    }
}

/// The bundled egui fonts have no U+20BD, so the ruble sign would render as tofu.
fn rub(n: f64) -> String {
    format!("{} руб.", grouped(n))
}

fn pct(n: f64) -> String {
    let s = if (n - n.round()).abs() < 0.05 {
        format!("{:+}", n.round() as i64)
    } else {
        format!("{:+.1}", n).replace('.', ",")
    };
    format!("{}%", s)
}

fn now_age(modified: SystemTime) -> Duration {
    SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
}

fn short_age(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{} с", s)
    } else if s < 3600 {
        format!("{}м", s / 60)
    } else {
        format!("{}ч", s / 3600)
    }
}

/// "4/4" is meaningless on its own: anything shaped like a ratio gets its unit.
fn with_units(hint: &str) -> String {
    if let Some((a, b)) = hint.split_once('/') {
        let numeric = !a.is_empty()
            && !b.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && b.chars().all(|c| c.is_ascii_digit());
        if numeric {
            return format!("{} пробы", hint);
        }
    }
    hint.to_owned()
}

fn hhmm() -> String {
    Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "--:--".to_owned())
}

struct App {
    path: PathBuf,
    feed: Option<Feed>,
    file_age: Option<Duration>,
    last_read: Option<Instant>,
    clock: String,
    clock_at: Option<Instant>,
}

impl App {
    fn new() -> Self {
        let path = std::env::var("RVNC_FEED")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(FEED_DEFAULT));
        let mut app = Self {
            path,
            feed: None,
            file_age: None,
            last_read: None,
            clock: hhmm(),
            clock_at: Some(Instant::now()),
        };
        app.refresh();
        app
    }

    /// Re-read the feed and its mtime. Any error means "no data", never a panic.
    fn refresh(&mut self) {
        self.last_read = Some(Instant::now());
        match fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(modified) => {
                self.file_age = Some(now_age(modified));
                self.feed = fs::read_to_string(&self.path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Feed>(&text).ok());
            }
            Err(_) => {
                self.file_age = None;
                self.feed = None;
            }
        }
    }

    fn fresh(&self) -> bool {
        self.feed.is_some() && self.file_age.map(|a| a < STALE).unwrap_or(false)
    }

    /// A section is usable only when the whole feed is fresh; each card then checks its own field.
    fn feed(&self) -> Option<&Feed> {
        if self.fresh() {
            self.feed.as_ref()
        } else {
            None
        }
    }

    /// The clock only needs minute precision; one `date` per 5 s is cheaper than per frame.
    fn clock_text(&mut self) -> String {
        if self.clock_at.map(|t| t.elapsed() >= Duration::from_secs(5)).unwrap_or(true) {
            self.clock = hhmm();
            self.clock_at = Some(Instant::now());
        }
        self.clock.clone()
    }
}

fn setup_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = rgb(BG);
    style.visuals.window_fill = rgb(BG);
    style.visuals.extreme_bg_color = rgb(BG_CARD);
    style.spacing.item_spacing = egui::vec2(0.0, 0.0);
    ctx.set_style(style);
}

fn font(size: f32) -> egui::FontId {
    egui::FontId::proportional(size)
}

fn mono(size: f32) -> egui::FontId {
    egui::FontId::monospace(size)
}

fn text_left(p: &egui::Painter, pos: egui::Pos2, text: &str, f: egui::FontId, c: egui::Color32) {
    p.text(pos, egui::Align2::LEFT_CENTER, text, f, c);
}

fn text_right(p: &egui::Painter, pos: egui::Pos2, text: &str, f: egui::FontId, c: egui::Color32) {
    p.text(pos, egui::Align2::RIGHT_CENTER, text, f, c);
}

/// Largest prefix that fits `max_w`, so a merchant name never spills into the neighbour card.
fn fitted(p: &egui::Painter, text: &str, f: &egui::FontId, max_w: f32) -> String {
    if p.layout_no_wrap(text.to_owned(), f.clone(), egui::Color32::WHITE)
        .size()
        .x
        <= max_w
    {
        return text.to_owned();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut probe = out.clone();
        probe.push(ch);
        probe.push('…');
        if p.layout_no_wrap(probe, f.clone(), egui::Color32::WHITE).size().x > max_w {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn card(ui: &mut egui::Ui, size: egui::Vec2, fill: egui::Color32) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::Rounding::same(10.0), fill);
    rect
}

fn label(p: &egui::Painter, rect: egui::Rect, text: &str) {
    p.text(
        rect.left_top() + egui::vec2(14.0, 10.0),
        egui::Align2::LEFT_TOP,
        text,
        font(11.0),
        rgb(LAVENDER),
    );
}

fn unknown_marker(p: &egui::Painter, rect: egui::Rect, size: f32) {
    p.text(
        egui::pos2(rect.right() - 14.0, rect.top() + 10.0 + size * 1.6),
        egui::Align2::RIGHT_TOP,
        UNKNOWN,
        font(size),
        rgb(DIM),
    );
}

fn pill(p: &egui::Painter, rect: egui::Rect, name: &str, state: Option<&str>, hint: Option<&str>) {
    p.rect_filled(rect, egui::Rounding::same(8.0), rgb(BG_CARD));
    let k = kind(state);
    let cy = rect.center().y;
    p.circle_filled(egui::pos2(rect.left() + 14.0, cy), 4.0, kind_color(k));
    text_left(p, egui::pos2(rect.left() + 26.0, cy), name, font(12.0), rgb(LAVENDER));
    let (value, color) = match k {
        Kind::Unknown => (UNKNOWN.to_owned(), rgb(DIM)),
        _ => (with_units(hint.unwrap_or("—")), kind_color(k)),
    };
    text_right(p, egui::pos2(rect.right() - 10.0, cy), &value, mono(12.0), color);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.last_read.map(|t| t.elapsed() >= REFRESH).unwrap_or(true) {
            self.refresh();
        }

        let clock = self.clock_text();
        let fresh = self.fresh();
        let file_age = self.file_age;
        let feed = self.feed().cloned();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(rgb(BG))
                    .inner_margin(egui::Margin::symmetric(8.0, 8.0)),
            )
            .show(ctx, |ui| {
                let w = ui.available_width();
                let p = ui.painter().clone();

                // === Header: the cutout owns the first 90x40, so text starts past it. ===
                let header = ui
                    .allocate_exact_size(egui::vec2(w, HEADER_H), egui::Sense::hover())
                    .0;
                let hy = header.center().y;
                let x0 = header.left() + 84.0;
                p.text(
                    egui::pos2(x0, hy),
                    egui::Align2::LEFT_CENTER,
                    "X-HUB",
                    font(16.0),
                    rgb(ACCENT),
                );
                let name_w = p
                    .layout_no_wrap("X-HUB".to_owned(), font(16.0), rgb(ACCENT))
                    .size()
                    .x;
                p.text(
                    egui::pos2(x0 + name_w + 8.0, hy),
                    egui::Align2::LEFT_CENTER,
                    format!("· {}", clock),
                    mono(14.0),
                    rgb(LAVENDER),
                );
                let (age_text, age_color) = match file_age {
                    Some(a) if fresh => (format!("feed {}", short_age(a)), rgb(DIM)),
                    Some(a) => (format!("feed {} · устарел", short_age(a)), rgb(RED)),
                    None => ("feed нет".to_owned(), rgb(RED)),
                };
                text_right(
                    &p,
                    egui::pos2(header.right(), hy),
                    &age_text,
                    mono(11.0),
                    age_color,
                );

                ui.add_space(2.0);

                // === Status strip: four pills, two rows of two. ===
                let pill_w = (w - 8.0) / 2.0;
                let kz = feed
                    .as_ref()
                    .and_then(|f| f.relays.iter().find(|r| r.name.as_deref() == Some("KZ")));
                let by = feed
                    .as_ref()
                    .and_then(|f| f.relays.iter().find(|r| r.name.as_deref() == Some("BY")));
                let prod = feed.as_ref().and_then(|f| f.prod.as_ref());
                let lv_ci = feed.as_ref().and_then(|f| f.lv_ci.as_ref());

                let rows = [("PROD", prod), ("KZ relay", kz), ("BY relay", by), ("LV CI", lv_ci)];
                for row in 0..2 {
                    let y = ui.cursor().top();
                    for col in 0..2 {
                        let idx = row * 2 + col;
                        let r = egui::Rect::from_min_size(
                            egui::pos2(header.left() + col as f32 * (pill_w + 8.0), y),
                            egui::vec2(pill_w, PILL_H),
                        );
                        let (name, src) = rows[idx];
                        let (state, hint) = match src {
                            Some(s) => (s.state.as_deref(), s.hint.as_deref()),
                            None => (None, None),
                        };
                        pill(&p, r, name, state, hint);
                    }
                    ui.add_space(PILL_H);
                    if row == 0 {
                        ui.add_space(PILL_GAP);
                    }
                }

                ui.add_space(6.0);

                // === Money block. Numbers right-align to the same edge as the cards below. ===
                let money = card(ui, egui::vec2(w, MONEY_H), rgb(BG_CARD));
                label(&p, money, "ПЛАТЕЖИ СЕГОДНЯ");
                match feed.as_ref().and_then(|f| f.payments_today.as_ref()) {
                    Some(m) => {
                        p.text(
                            egui::pos2(money.right() - 14.0, money.top() + 26.0),
                            egui::Align2::RIGHT_TOP,
                            rub(m.sum),
                            mono(40.0),
                            rgb(CYAN),
                        );
                        let delta_color = if m.delta_pct >= 0.0 { rgb(GREEN) } else { rgb(RED) };
                        let line_y = money.bottom() - 16.0;
                        // Count and «к вчера» in lavender, only the delta in green/red. Widths are
                        // measured from the laid-out text, not guessed from character counts.
                        let parts = [
                            (format!("{} шт · ", grouped(m.count)), rgb(LAVENDER)),
                            (pct(m.delta_pct), delta_color),
                            (" к вчера".to_owned(), rgb(LAVENDER)),
                        ];
                        let widths: Vec<f32> = parts
                            .iter()
                            .map(|(t, c)| p.layout_no_wrap(t.clone(), mono(12.0), *c).size().x)
                            .collect();
                        let total: f32 = widths.iter().sum();
                        let mut x = money.right() - 14.0 - total;
                        for ((text, color), width) in parts.iter().zip(widths) {
                            text_left(&p, egui::pos2(x, line_y), text, mono(12.0), *color);
                            x += width;
                        }
                    }
                    None => unknown_marker(&p, money, 18.0),
                }

                ui.add_space(6.0);

                // === Bottom row: refunds and the top merchant take the remaining height. ===
                let bh = ui.available_height().max(0.0);
                let bw = (w - 8.0) / 2.0;
                let left = card(ui, egui::vec2(bw, bh), rgb(BG_CARD));
                let right = egui::Rect::from_min_size(
                    egui::pos2(left.right() + 8.0, left.top()),
                    egui::vec2(bw, bh),
                );
                p.rect_filled(right, egui::Rounding::same(10.0), rgb(BG_DARK));

                label(&p, left, "ВОЗВРАТЫ");
                match feed.as_ref().and_then(|f| f.refunds.as_ref()) {
                    Some(r) => {
                        p.text(
                            egui::pos2(left.right() - 14.0, left.top() + 22.0),
                            egui::Align2::RIGHT_TOP,
                            grouped(r.count),
                            mono(28.0),
                            rgb(LAVENDER),
                        );
                        p.text(
                            egui::pos2(left.right() - 14.0, left.top() + 58.0),
                            egui::Align2::RIGHT_TOP,
                            rub(r.sum),
                            mono(13.0),
                            rgb(LAVENDER),
                        );
                        p.text(
                            egui::pos2(left.right() - 14.0, left.top() + 80.0),
                            egui::Align2::RIGHT_TOP,
                            "висят",
                            font(11.0),
                            rgb(DIM),
                        );
                    }
                    None => unknown_marker(&p, left, 16.0),
                }

                label(&p, right, "ТОП МЕРЧ");
                match feed.as_ref().and_then(|f| f.top_merchant.as_ref()) {
                    Some(t) => {
                        let name = t.name.as_deref().unwrap_or("—");
                        let shown = fitted(&p, name, &font(16.0), bw - 28.0);
                        p.text(
                            right.left_top() + egui::vec2(14.0, 22.0),
                            egui::Align2::LEFT_TOP,
                            shown,
                            font(16.0),
                            rgb(LAVENDER),
                        );
                        p.text(
                            egui::pos2(right.right() - 14.0, right.top() + 50.0),
                            egui::Align2::RIGHT_TOP,
                            rub(t.turnover),
                            mono(14.0),
                            rgb(CYAN),
                        );
                        p.text(
                            egui::pos2(right.right() - 14.0, right.top() + 72.0),
                            egui::Align2::RIGHT_TOP,
                            format!("{:.0}% оборота", t.share_pct),
                            font(11.0),
                            rgb(DIM),
                        );
                    }
                    None => unknown_marker(&p, right, 16.0),
                }
            });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 360.0])
            .with_min_inner_size([800.0, 360.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_app_id("rvnc-panel")
            .with_title("rvnc-panel"),
        ..Default::default()
    };

    eframe::run_native(
        "rvnc-panel",
        options,
        Box::new(|cc| {
            setup_theme(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
