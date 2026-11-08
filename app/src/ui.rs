use crate::{icons as ic, keys, net, theme::{self, bold, mix, Palette, MOBILE}, widgets::*};
use eframe::egui::{self, Align2, Color32, CornerRadius, Event, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use proto::{Button, ClientMsg, Input, Pairing, PowerAction, Stats, ThemeColors};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(PartialEq, Clone, Copy)]
enum Tab { Screen, Dashboard, Power, Settings }

enum Conn { Idle, Connecting, Up(String), Down(String) }

#[derive(Clone, Copy)]
enum Card { Cpu, Mem, Net, Sys, CpuHist, NetHist, Disks, Procs, Cores }

const HIST: usize = 120;
const GAP: f32 = 14.0;

struct Settings { quality: u8, max_width: u32, fps: u8, sync_theme: bool }

impl Settings {
    fn load(path: &PathBuf) -> Self {
        let mut s = Self { quality: 60, max_width: 1280, fps: 30, sync_theme: true };
        for l in std::fs::read_to_string(path).unwrap_or_default().lines() {
            if let Some((k, v)) = l.split_once('=') {
                match k { "quality" => s.quality = v.parse().unwrap_or(s.quality), "max_width" => s.max_width = v.parse().unwrap_or(s.max_width),
                    "fps" => s.fps = v.parse().unwrap_or(s.fps), "sync_theme" => s.sync_theme = v != "0", _ => {} }
            }
        }
        s
    }
    fn save(&self, path: &PathBuf) {
        let _ = std::fs::write(path, format!("quality={}\nmax_width={}\nfps={}\nsync_theme={}\n", self.quality, self.max_width, self.fps, self.sync_theme as u8));
    }
}

pub struct App {
    cfg: PathBuf,
    cfg_settings: PathBuf,
    code: String,
    handle: Option<net::Handle>,
    conn: Conn,
    note: Option<String>,
    tex: Option<egui::TextureHandle>,
    stats: Option<Stats>,
    hist: VecDeque<(f32, f32, f32, f32)>, // cpu%, mem%, rx, tx
    tab: Tab,
    palette: Palette,
    host_theme: Option<ThemeColors>,
    local_theme: Option<ThemeColors>,
    theme_rx: Option<mpsc::Receiver<Option<ThemeColors>>>,
    settings: Settings,
    local: Option<Pairing>,
    // input state (touch / remote screen)
    touch_seen: bool,
    drag_mode: bool,
    right_next: bool,
    kb_open: bool,
    sticky: [bool; 4],
    phys: [bool; 3],
    scroll_acc: f32,
    last_move: Option<(Pos2, Instant)>,
    stream_sent: Option<(Tab, u8, u32, u8)>,
    confirm: Option<(PowerAction, Instant)>,
    logo: egui::TextureHandle,
    compact: bool,
    rotate_hint: bool,
    dev_scroll: Option<f32>,
}

fn local_pairing() -> Option<Pairing> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?.join("pc-control");
    let token = std::fs::read_to_string(base.join("token")).ok()?.trim().to_string();
    let cert = std::fs::read(base.join("cert.der")).ok()?;
    Some(Pairing { addr: format!("127.0.0.1:{}", proto::DEFAULT_PORT), token, fingerprint: hex::encode(Sha256::digest(cert)) })
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: PathBuf) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install_fonts(ctx);
        let cfg_settings = cfg.with_file_name("settings.txt");
        let settings = Settings::load(&cfg_settings);
        let dark = ctx.system_theme().map_or(true, |t| t == egui::Theme::Dark);
        let palette = Palette::from_system(dark);
        theme::apply(ctx, &palette);
        let code = std::fs::read_to_string(&cfg).unwrap_or_default().trim().to_string();
        let local = if MOBILE && !cfg!(feature = "mobile-preview") { None } else { local_pairing() };
        let theme_rx = if MOBILE { None } else {
            let (tx, rx) = mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let mut last = None;
                loop {
                    let t = proto::theme::detect(true);
                    if t != last { last = t.clone(); if tx.send(t).is_err() { return; } c.request_repaint(); }
                    std::thread::sleep(Duration::from_secs(4));
                }
            });
            Some(rx)
        };
        let mut app = Self {
            cfg, cfg_settings, code, handle: None, conn: Conn::Idle, note: None, tex: None, stats: None, hist: VecDeque::new(),
            tab: if MOBILE { Tab::Screen } else { Tab::Dashboard }, palette, host_theme: None, local_theme: None, theme_rx, settings, local,
            touch_seen: false, drag_mode: false, right_next: false, kb_open: false, sticky: [false; 4], phys: [false; 3],
            scroll_acc: 0.0, last_move: None, stream_sent: None, confirm: None,
            compact: MOBILE, rotate_hint: true,
            logo: ctx.load_texture("logo", logo_image(), egui::TextureOptions::LINEAR),
            dev_scroll: std::env::var("PCC_SCROLL").ok().and_then(|v| v.parse().ok()), // dev aid for screenshots
        };
        match std::env::var("PCC_TAB").as_deref() { // dev aid: start on a given tab
            Ok("power") => app.tab = Tab::Power,
            Ok("settings") => app.tab = Tab::Settings,
            Ok("dashboard") => app.tab = Tab::Dashboard,
            Ok("screen") => app.tab = Tab::Screen,
            _ => {}
        }
        if std::env::var_os("PCC_ONBOARD").is_some() { return app; } // dev aid: show the pairing screen
        if std::env::var_os("PCC_CONFIRM").is_some() { app.confirm = Some((PowerAction::Reboot, Instant::now() + Duration::from_secs(3600))); }
        if Pairing::parse(&app.code).is_some() { app.connect(ctx); } else if app.local.is_some() { app.connect_local(ctx); }
        app
    }

    fn connect_with(&mut self, ctx: &egui::Context, p: Pairing) {
        self.handle = Some(net::spawn(p, ctx.clone()));
        self.conn = Conn::Connecting;
        self.stream_sent = None;
        self.note = None;
    }
    fn connect(&mut self, ctx: &egui::Context) {
        match Pairing::parse(&self.code) {
            Some(p) => { let _ = std::fs::write(&self.cfg, &self.code); self.connect_with(ctx, p); }
            None => self.note = Some("That doesn't look like a pairing code.".into()),
        }
    }
    fn connect_local(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.local.clone() { self.connect_with(ctx, p); }
    }

    fn send(&self, m: ClientMsg) { if let Some(h) = &self.handle { let _ = h.tx.send(m); } }
    fn input(&self, i: Input) { self.send(ClientMsg::Input(i)); }
    fn key(&self, code: u16, down: bool) { self.input(Input::Key { code, down }); }
    fn tap_key(&self, code: u16) { self.key(code, true); self.key(code, false); }

    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.theme_rx { while let Ok(t) = rx.try_recv() { self.local_theme = t; } }
        let mut drop_handle = false;
        if let Some(h) = &self.handle {
            while let Ok(ev) = h.rx.try_recv() {
                match ev {
                    net::Event::Connecting => self.conn = Conn::Connecting,
                    net::Event::Connected(n) => { self.conn = Conn::Up(n); self.stream_sent = None; }
                    net::Event::Frame(img) => match &mut self.tex {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.tex = Some(ctx.load_texture("screen", img, egui::TextureOptions::LINEAR)),
                    },
                    net::Event::Stats(s) => {
                        let mem = if s.mem_total > 0 { s.mem_used as f32 / s.mem_total as f32 * 100.0 } else { 0.0 };
                        self.hist.push_back((s.cpu, mem, s.net_rx_bps as f32, s.net_tx_bps as f32));
                        if self.hist.len() > HIST { self.hist.pop_front(); }
                        self.stats = Some(s);
                    }
                    net::Event::Theme(t) => self.host_theme = Some(t),
                    net::Event::Lost(m) => self.conn = Conn::Down(m),
                    net::Event::HostError(m) => self.note = Some(m),
                    net::Event::Fatal(m) => { self.conn = Conn::Down(m); drop_handle = true; }
                }
            }
        }
        if drop_handle { self.handle = None; }

        // Palette: desktop follows the local desktop theme, phone follows the PC (or the phone's light/dark).
        let wanted = if MOBILE {
            match (&self.host_theme, self.settings.sync_theme) {
                (Some(t), true) => Palette::derive(t),
                _ => Palette::from_system(ctx.system_theme().map_or(true, |t| t == egui::Theme::Dark)),
            }
        } else {
            match &self.local_theme {
                Some(t) => Palette::derive(t),
                None => Palette::from_system(ctx.system_theme().map_or(true, |t| t == egui::Theme::Dark)),
            }
        };
        if wanted != self.palette { self.palette = wanted; theme::apply(ctx, &self.palette); }
    }

    fn sync_stream(&mut self) {
        if !MOBILE || !matches!(self.conn, Conn::Up(_)) { return; }
        let want = (self.tab, self.settings.quality, self.settings.max_width, self.settings.fps);
        if self.stream_sent != Some(want) {
            self.send(ClientMsg::Stream { on: self.tab == Tab::Screen, max_width: self.settings.max_width, quality: self.settings.quality, fps: self.settings.fps });
            self.stream_sent = Some(want);
        }
    }

    fn release_sticky(&mut self) {
        for (i, code) in [keys::CTRL, keys::ALT, keys::SHIFT, keys::SUPER].into_iter().enumerate() {
            if self.sticky[i] { self.key(code, false); self.sticky[i] = false; }
        }
    }

    fn forward_keyboard(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                Event::Text(t) => {
                    let t: String = t.chars().filter(|c| !c.is_control()).collect();
                    if t.is_empty() { continue; }
                    if self.sticky.iter().any(|s| *s) {
                        for c in t.chars() {
                            if let Some((code, shift)) = proto::char_to_key(c) {
                                if shift && !self.sticky[2] { self.key(keys::SHIFT, true); }
                                self.tap_key(code);
                                if shift && !self.sticky[2] { self.key(keys::SHIFT, false); }
                            }
                        }
                        self.release_sticky();
                    } else {
                        self.input(Input::Text(t));
                    }
                }
                Event::Key { key, pressed, repeat: false, modifiers, .. } => {
                    let now = [modifiers.ctrl, modifiers.alt, modifiers.shift];
                    for (i, code) in [keys::CTRL, keys::ALT, keys::SHIFT].into_iter().enumerate() {
                        if now[i] != self.phys[i] { self.phys[i] = now[i]; if !self.sticky[i] { self.key(code, now[i]); } }
                    }
                    let Some((code, is_text)) = keys::map(key) else { continue };
                    if is_text && !(modifiers.ctrl || modifiers.alt) { continue; }
                    self.key(code, pressed);
                    if !pressed { self.release_sticky(); }
                }
                _ => {}
            }
        }
        let m = ctx.input(|i| i.modifiers);
        for (i, (now, code)) in [(m.ctrl, keys::CTRL), (m.alt, keys::ALT), (m.shift, keys::SHIFT)].into_iter().enumerate() {
            if now != self.phys[i] { self.phys[i] = now; if !self.sticky[i] { self.key(code, now); } }
        }
    }

    // ───────────────────────────── remote screen (phone) ─────────────────────────────

    fn screen(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let ctx = ui.ctx().clone();
        let avail = ui.available_rect_before_wrap();
        let Some(tex) = self.tex.clone() else {
            ui.scope_builder(egui::UiBuilder::new().max_rect(avail), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(avail.height() * 0.38);
                    ui.label(icon(ic::MONITOR, 40.0, p.faint));
                    ui.label(RichText::new("Waiting for the first frame…").color(p.muted));
                });
            });
            return;
        };
        let ts = tex.size_vec2();
        let scale = (avail.width() / ts.x).min(avail.height() / ts.y);
        let rect = Rect::from_center_size(avail.center(), ts * scale);
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.painter().image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

        let norm = |q: Pos2| Input::MoveTo { x: ((q.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), y: ((q.y - rect.min.y) / rect.height()).clamp(0.0, 1.0) };
        let (events, multi) = ctx.input(|i| (i.events.clone(), i.multi_touch()));
        if events.iter().any(|e| matches!(e, Event::Touch { .. })) { self.touch_seen = true; }
        if multi.is_none() {
            if let Some(q) = resp.hover_pos().or(resp.interact_pointer_pos()) {
                let due = self.last_move.map_or(true, |(lp, t)| lp != q && t.elapsed().as_millis() >= 8);
                if due { self.input(norm(q)); self.last_move = Some((q, Instant::now())); }
            }
        }
        for e in &events {
            match e {
                Event::PointerButton { pos, button, pressed, .. } if rect.contains(*pos) || !pressed => {
                    let b = match button { egui::PointerButton::Primary => Button::Left, egui::PointerButton::Secondary => Button::Right, egui::PointerButton::Middle => Button::Middle, _ => continue };
                    if self.touch_seen && !self.drag_mode { continue; }
                    if *pressed { self.input(norm(*pos)); }
                    self.input(Input::Button { button: b, down: *pressed });
                }
                Event::MouseWheel { unit, delta, .. } if resp.hovered() && !self.touch_seen => {
                    let k = if matches!(unit, egui::MouseWheelUnit::Line) { 1.0 } else { 1.0 / 40.0 };
                    self.input(Input::Scroll { dx: (delta.x * k).round() as i32, dy: (delta.y * k).round() as i32 });
                }
                _ => {}
            }
        }
        if self.touch_seen && !self.drag_mode && resp.clicked() {
            if let Some(q) = resp.interact_pointer_pos() {
                let b = if std::mem::take(&mut self.right_next) { Button::Right } else { Button::Left };
                self.input(norm(q));
                self.input(Input::Button { button: b, down: true });
                self.input(Input::Button { button: b, down: false });
            }
        }
        if let Some(mt) = multi {
            self.scroll_acc += mt.translation_delta.y;
            let notches = (self.scroll_acc / 24.0).trunc();
            if notches != 0.0 { self.scroll_acc -= notches * 24.0; self.input(Input::Scroll { dx: 0, dy: notches as i32 }); }
        } else { self.scroll_acc = 0.0; }
        if self.kb_open || !self.touch_seen { self.forward_keyboard(&ctx); }
    }

    fn key_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::ScrollArea::horizontal().auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if pill_button(ui, &p, ic::KEYBOARD, "", self.kb_open).clicked() {
                    self.kb_open = !self.kb_open;
                    #[cfg(target_os = "android")]
                    if let Some(a) = crate::ANDROID_APP.get() { if self.kb_open { a.show_soft_input(false) } else { a.hide_soft_input(false) } }
                }
                if self.touch_seen {
                    if pill_button(ui, &p, ic::TAP, "Hold-drag", self.drag_mode).clicked() { self.drag_mode = !self.drag_mode; }
                    if pill_button(ui, &p, ic::CLICK, "Right-click", self.right_next).clicked() { self.right_next = !self.right_next; }
                }
                ui.separator();
                for (code, glyph, label) in [(1, "", "Esc"), (15, "", "Tab"), (28, ic::RETURN, ""), (14, ic::BACKSPACE, ""), (105, ic::LEFT, ""), (103, ic::UP, ""), (108, ic::DOWN, ""), (106, ic::RIGHT, "")] {
                    if pill_button(ui, &p, glyph, label, false).clicked() { self.tap_key(code); }
                }
                ui.separator();
                for (i, (code, label)) in [(keys::CTRL, "Ctrl"), (keys::ALT, "Alt"), (keys::SHIFT, "Shift"), (keys::SUPER, "Super")].into_iter().enumerate() {
                    if pill_button(ui, &p, "", label, self.sticky[i]).clicked() { self.sticky[i] = !self.sticky[i]; self.key(code, self.sticky[i]); }
                }
            });
        });
    }

    // ───────────────────────────── dashboard ─────────────────────────────

    fn flow(&self, ui: &mut egui::Ui, width: f32, cols: usize, items: &[(Card, usize, f32)]) {
        let unit = (width - GAP * (cols as f32 - 1.0)) / cols as f32;
        ui.spacing_mut().item_spacing = Vec2::splat(GAP);
        let mut i = 0;
        while i < items.len() {
            let (mut row, mut used) = (vec![], 0);
            while i < items.len() && used + items[i].1.min(cols) <= cols { used += items[i].1.min(cols); row.push(items[i]); i += 1; }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for (c, span, h) in row {
                    let span = span.min(cols) as f32;
                    self.draw_card(ui, c, unit * span + GAP * (span - 1.0), h);
                }
            });
        }
    }

    fn dashboard(&self, ui: &mut egui::Ui, width: f32) {
        let p = self.palette;
        let Some(s) = &self.stats else {
            card(ui, &p, width, Some(160.0), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(icon(ic::CHART, 30.0, p.faint));
                    ui.label(RichText::new("Waiting for the first stats…").color(p.muted));
                });
            });
            return;
        };
        let cols = if width >= 900.0 { 4 } else { 2 };
        let wide = if cols == 4 { 2 } else { cols };
        let h_disks = (s.disks.len().max(1) as f32) * 50.0 + 96.0;
        let h_procs = 5.0 * 42.0 + 60.0;
        let (h_disks, h_procs) = if wide == cols { (h_disks, h_procs) } else { let m = h_disks.max(h_procs); (m, m) };
        let items = [
            (Card::Cpu, 1, 206.0), (Card::Mem, 1, 206.0), (Card::Net, 1, 206.0), (Card::Sys, 1, 206.0),
            (Card::CpuHist, wide, 214.0), (Card::NetHist, wide, 214.0),
            (Card::Disks, wide, h_disks), (Card::Procs, wide, h_procs),
            (Card::Cores, cols, 150.0),
        ];
        self.flow(ui, width, cols, &items);
    }

    fn draw_card(&self, ui: &mut egui::Ui, kind: Card, w: f32, h: f32) {
        let p = self.palette;
        let Some(s) = &self.stats else { return };
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        let level = |f: f32| if f > 0.92 { p.bad } else if f > 0.8 { p.warn } else { p.accent };
        let rate = |b: f32| if b >= 1048576.0 { format!("{:.1} MB/s", b / 1048576.0) } else { format!("{:.0} KB/s", b / 1024.0) };
        card(ui, &p, w, Some(h), |ui| match kind {
            Card::Cpu => {
                card_title(ui, &p, ic::CPU, "CPU");
                let f = s.cpu / 100.0;
                ui.vertical_centered(|ui| ring(ui, &p, f, 112.0, level(f), &format!("{:.0}%", s.cpu), &format!("{} cores", s.cpu_cores.len())));
            }
            Card::Mem => {
                card_title(ui, &p, ic::MEMORY, "Memory");
                let f = s.mem_used as f32 / s.mem_total.max(1) as f32;
                ui.vertical_centered(|ui| ring(ui, &p, f, 112.0, level(f), &format!("{:.0}%", f * 100.0), &format!("{:.1}/{:.0} GiB", gib(s.mem_used), gib(s.mem_total))));
            }
            Card::Net => {
                card_title(ui, &p, ic::WIFI, "Network");
                ui.add_space(6.0);
                for (g, col, label, v) in [(ic::DOWN, p.accent, "Download", s.net_rx_bps as f32), (ic::UP, p.series2, "Upload", s.net_tx_bps as f32)] {
                    ui.horizontal(|ui| {
                        ui.label(icon(g, 22.0, col));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(rate(v)).font(FontId::new(19.0, bold())).color(p.text));
                            ui.label(RichText::new(label).size(12.0).color(p.muted));
                        });
                    });
                    ui.add_space(4.0);
                }
            }
            Card::Sys => {
                card_title(ui, &p, ic::GAUGE, "System");
                ui.add_space(4.0);
                let up = s.uptime_s;
                let rows = [
                    (ic::THERMO, "Temp", s.temp_c.map_or("n/a".to_string(), |t| format!("{t:.0} °C"))),
                    (ic::CLOCK, "Uptime", if up >= 86400 { format!("{}d {}h", up / 86400, up / 3600 % 24) } else { format!("{}h {}m", up / 3600, up / 60 % 60) }),
                    (ic::GAUGE, "Load", format!("{:.2}", s.load[0])),
                ];
                for (g, k, v) in rows {
                    split(ui, 26.0, |ui| { ui.label(icon(g, 16.0, p.muted)); ui.label(RichText::new(k).color(p.muted).size(13.0)); },
                        |ui| { ui.label(RichText::new(v).font(FontId::new(14.5, bold())).color(p.text)); });
                }
            }
            Card::CpuHist => {
                card_title(ui, &p, ic::CHART, "CPU, last 2 minutes");
                chart(ui, &p, "cpu", &[Series { name: "CPU", data: self.hist.iter().map(|h| h.0).collect(), color: p.accent }], HIST, 100.0, h - 90.0, &|v| format!("{v:.0}%"));
                split(ui, 16.0, |ui| { ui.label(RichText::new("2 min ago").size(11.0).color(p.faint)); }, |ui| { ui.label(RichText::new("now").size(11.0).color(p.faint)); });
            }
            Card::NetHist => {
                card_title(ui, &p, ic::WIFI, "Network, last 2 minutes");
                let peak = self.hist.iter().map(|h| h.2.max(h.3)).fold(0.0, f32::max).max(100.0 * 1024.0) * 1.15;
                chart(ui, &p, "net", &[
                    Series { name: "Down", data: self.hist.iter().map(|h| h.2).collect(), color: p.accent },
                    Series { name: "Up", data: self.hist.iter().map(|h| h.3).collect(), color: p.series2 },
                ], HIST, peak, h - 90.0, &|v| rate(v));
                split(ui, 16.0, |ui| {
                    ui.label(icon(ic::DOWN, 12.0, p.accent)); ui.label(RichText::new("Down").size(11.5).color(p.muted));
                    ui.label(icon(ic::UP, 12.0, p.series2)); ui.label(RichText::new("Up").size(11.5).color(p.muted));
                }, |ui| { ui.label(RichText::new(format!("peak {}", rate(peak / 1.15))).size(11.0).color(p.faint)); });
            }
            Card::Disks => {
                card_title(ui, &p, ic::DRIVE, "Storage");
                ui.add_space(4.0);
                if s.disks.is_empty() { ui.label(RichText::new("No disks reported").color(p.muted)); }
                for d in &s.disks {
                    let f = d.used as f32 / d.total.max(1) as f32;
                    split(ui, 22.0, |ui| { ui.label(RichText::new(&d.mount).font(FontId::new(13.5, bold())).color(p.text)); },
                        |ui| { ui.label(RichText::new(format!("{:.0} / {:.0} GiB · {:.0}%", gib(d.used), gib(d.total), f * 100.0)).size(12.5).color(p.muted)); });
                    meter(ui, &p, f, level(f), 8.0);
                    ui.add_space(8.0);
                }
                if s.disks.len() > 1 {
                    let (u, t) = s.disks.iter().fold((0, 0), |(u, t), d| (u + d.used, t + d.total));
                    ui.label(RichText::new(format!("{:.0} GiB used of {:.0} GiB in total", gib(u), gib(t))).size(12.0).color(p.faint));
                }
            }
            Card::Procs => {
                card_title(ui, &p, ic::SQUARES, "Top processes");
                ui.add_space(2.0);
                let maxc = s.top.iter().map(|x| x.cpu).fold(10.0, f32::max);
                for pr in &s.top {
                    split(ui, 22.0, |ui| { ui.set_max_width((ui.available_width() - 130.0).max(40.0)); ui.add(egui::Label::new(RichText::new(&pr.name).size(13.5).color(p.text)).truncate()); },
                        |ui| {
                            ui.label(RichText::new(format!("{:.0} MiB", pr.mem as f64 / 1048576.0)).size(12.0).color(p.muted));
                            ui.label(RichText::new(format!("{:.1}%", pr.cpu)).font(FontId::new(13.5, bold())).color(p.text));
                        });
                    meter(ui, &p, pr.cpu / maxc, p.accent, 4.0);
                }
            }
            Card::Cores => {
                card_title(ui, &p, ic::CPU, "Cores");
                let n = s.cpu_cores.len().max(1);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h - 80.0), Sense::hover());
                let gap = 6.0;
                let bw = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).clamp(4.0, 34.0);
                let total = bw * n as f32 + gap * (n as f32 - 1.0);
                let x0 = rect.min.x + (rect.width() - total) / 2.0;
                for (i, c) in s.cpu_cores.iter().enumerate() {
                    let x = x0 + i as f32 * (bw + gap);
                    let col = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(bw, rect.height()));
                    let r = CornerRadius::same((bw / 2.0).min(6.0) as u8);
                    ui.painter().rect_filled(col, r, p.line);
                    let fh = (rect.height() * (c / 100.0).clamp(0.0, 1.0)).max(3.0);
                    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, rect.max.y - fh), col.max), r, level(c / 100.0));
                    ui.interact(col, ui.id().with(("core", i)), Sense::hover()).on_hover_text(format!("Core {i}: {c:.0}%"));
                }
            }
        });
    }

    // ───────────────────────────── power & settings ─────────────────────────────

    fn power(&mut self, ui: &mut egui::Ui, width: f32) {
        let p = self.palette;
        if let Some((_, t)) = self.confirm { if t.elapsed() > Duration::from_secs(4) { self.confirm = None; } else { ui.ctx().request_repaint_after(Duration::from_millis(500)); } }
        let cols = if width >= 640.0 { 4 } else { 2 };
        let tw = (width - GAP * (cols as f32 - 1.0)) / cols as f32;
        let acts = [
            (PowerAction::Lock, ic::LOCK, "Lock", "Lock the session", false),
            (PowerAction::Suspend, ic::MOON, "Suspend", "Sleep to RAM", false),
            (PowerAction::Reboot, ic::REFRESH, "Reboot", "Restart the PC", true),
            (PowerAction::Shutdown, ic::POWER, "Shut down", "Power off", true),
        ];
        ui.spacing_mut().item_spacing = Vec2::splat(GAP);
        for row in acts.chunks(cols) {
            ui.horizontal(|ui| {
                for (a, g, label, sub, danger) in row {
                    let armed = matches!(self.confirm, Some((c, _)) if c == *a);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(tw, 132.0), Sense::click());
                    let bg = if armed { mix(p.bad, Color32::BLACK, 0.3) } else if resp.hovered() { mix(p.card, p.accent, 0.12) } else { p.card };
                    ui.painter().rect(rect, CornerRadius::same(18), bg, Stroke::new(1.0, if armed { p.bad } else { p.hair }), egui::StrokeKind::Inside);
                    let _ = sub;
                    let fg = if armed { Color32::WHITE } else { p.text };
                    ui.painter().text(Pos2::new(rect.center().x, rect.center().y - 24.0), Align2::CENTER_CENTER, if armed { ic::WARNING } else { *g }, FontId::new(34.0, egui::FontFamily::Proportional), if armed { Color32::WHITE } else if *danger { p.warn } else { p.accent });
                    ui.painter().text(Pos2::new(rect.center().x, rect.center().y + 18.0), Align2::CENTER_CENTER, if armed { "Tap again to confirm".to_string() } else { label.to_string() }, FontId::new(14.5, bold()), fg);
                    if !armed { ui.painter().text(Pos2::new(rect.center().x, rect.center().y + 40.0), Align2::CENTER_CENTER, *sub, FontId::new(12.0, egui::FontFamily::Proportional), p.muted); }
                    if resp.clicked() {
                        if *danger && !armed { self.confirm = Some((*a, Instant::now())); } else { self.confirm = None; self.send(ClientMsg::Power(*a)); }
                    }
                }
            });
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Actions run on your PC immediately. Reboot and shut down need a second tap.").size(12.5).color(p.faint));
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, width: f32) {
        let p = self.palette;
        let width = width.min(760.0);
        let host = if let Conn::Up(n) = &self.conn { n.clone() } else { "–".into() };
        let addr = self.handle.as_ref().and(Pairing::parse(&self.code).or_else(|| self.local.clone())).map(|p| p.addr);
        card(ui, &p, width, None, |ui| {
            card_title(ui, &p, ic::PLUGS, "Connection");
            let (c, g, t) = match &self.conn {
                Conn::Up(_) => (p.ok, ic::CHECK, "Connected".to_string()),
                Conn::Connecting => (p.warn, ic::REFRESH, "Connecting…".to_string()),
                Conn::Down(m) => (p.bad, ic::WARNING, format!("Offline: {m}")),
                Conn::Idle => (p.faint, ic::PLUGS, "Not connected".to_string()),
            };
            status(ui, &p, c, g, &t);
            kv(ui, &p, "Host", &host);
            if let Some(a) = &addr { kv(ui, &p, "Address", a); }
            ui.add_space(2.0);
            let local_only = self.local.is_some() && Pairing::parse(&self.code).is_none();
            if pill_button(ui, &p, ic::SIGN_OUT, if local_only { "Disconnect" } else { "Forget this PC and pair again" }, false).clicked() {
                self.handle = None; self.conn = Conn::Idle; self.tex = None; self.stats = None; self.hist.clear(); self.code.clear();
                let _ = std::fs::remove_file(&self.cfg);
                self.local = None;
            }
        });
        card(ui, &p, width, None, |ui| {
            card_title(ui, &p, ic::PALETTE, "Appearance");
            let src = if MOBILE {
                match (&self.host_theme, self.settings.sync_theme) { (Some(t), true) => format!("Synced from your PC ({})", t.source), _ => "Phone light/dark".into() }
            } else {
                self.local_theme.as_ref().map_or("Default".to_string(), |t| format!("Following your desktop theme ({})", t.source))
            };
            ui.label(RichText::new(src).color(p.muted));
            ui.horizontal(|ui| {
                for (c, name) in [(p.page, "Page"), (p.card, "Card"), (p.raised, "Raised"), (p.accent, "Accent"), (p.series2, "Secondary"), (p.text, "Text")] {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                    ui.painter().rect(r, CornerRadius::same(9), c, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                    resp.on_hover_text(name);
                }
            });
            if MOBILE && toggle_row(ui, &p, "Match my PC's theme", &mut self.settings.sync_theme) { self.settings.save(&self.cfg_settings); }
        });
        if MOBILE {
            card(ui, &p, width, None, |ui| {
                card_title(ui, &p, ic::MONITOR, "Screen stream");
                let mut changed = false;
                let mut q = self.settings.quality as f32;
                if slider_row(ui, &p, "Image quality", &format!("{}%", self.settings.quality), &mut q, 20.0, 90.0) { self.settings.quality = q.round() as u8; changed = true; }
                let mut w = self.settings.max_width as f32;
                if slider_row(ui, &p, "Maximum width", &format!("{} px", self.settings.max_width), &mut w, 640.0, 1920.0) { self.settings.max_width = ((w / 40.0).round() * 40.0) as u32; changed = true; }
                let mut f = self.settings.fps as f32;
                if slider_row(ui, &p, "Frame rate", &format!("{} fps", self.settings.fps), &mut f, 5.0, 60.0) { self.settings.fps = f.round() as u8; changed = true; }
                ui.label(RichText::new("Lower quality and width save battery and data.").size(12.0).color(p.faint));
                if changed { self.settings.save(&self.cfg_settings); }
            });
        }
        card(ui, &p, width, None, |ui| {
            card_title(ui, &p, ic::SHIELD, "Security");
            ui.label(RichText::new("Traffic is encrypted with TLS and pinned to your PC's certificate. Keep port 7878 off the public internet; use Tailscale or WireGuard when you're away.").size(13.0).color(p.muted));
        });
        ui.label(RichText::new(format!("PC Control {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(p.faint));
    }

    // ───────────────────────────── shell ─────────────────────────────

    fn onboarding(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let ctx = ui.ctx().clone();
        let w = ui.available_width().min(480.0);
        let off = (ui.available_width() - w) / 2.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(off);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.add_space(if MOBILE { 40.0 } else { 90.0 });
                    ui.vertical_centered(|ui| { self.logo_img(ui, 84.0); });
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        ui.label(heading("PC Control", 28.0, p.text));
                        ui.label(RichText::new("See and control your PC from anywhere on your network.").color(p.muted));
                    });
                    ui.add_space(14.0);
                    card(ui, &p, w, None, |ui| {
                        if !MOBILE && self.local.is_some() {
                            card_title(ui, &p, ic::LAPTOP, "This computer");
                            ui.label(RichText::new("A PC Control host is installed here.").color(p.muted));
                            if primary_button(ui, &p, "Connect to this PC", true).clicked() { self.connect_local(&ctx); }
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(8.0);
                        }
                        card_title(ui, &p, ic::PHONE, "Pair with a pairing code");
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(RichText::new("On the PC run").size(13.0).color(p.muted));
                            ui.label(code("pccontrol-host pair", &p));
                            ui.label(RichText::new("and paste the code here.").size(13.0).color(p.muted));
                        });
                        let r = ui.add(egui::TextEdit::singleline(&mut self.code).hint_text(RichText::new("192.168.1.5:7878#token#fingerprint").color(p.faint)).desired_width(f32::INFINITY).margin(Vec2::new(12.0, 12.0)));
                        #[cfg(target_os = "android")]
                        {
                            if r.gained_focus() { if let Some(a) = crate::ANDROID_APP.get() { a.show_soft_input(false); } }
                            if pill_button(ui, &p, ic::CLIPBOARD, "Paste from clipboard", false).clicked() {
                                if let Some(t) = clipboard() { self.code = t.trim().to_string(); }
                            }
                        }
                        let _ = r;
                        let ok = Pairing::parse(&self.code).is_some();
                        if primary_button(ui, &p, "Connect", ok).clicked() && ok { self.connect(&ctx); }
                        if let Conn::Down(m) = &self.conn { status(ui, &p, p.bad, ic::WARNING, m); }
                        if let Some(n) = &self.note { status(ui, &p, p.bad, ic::WARNING, n); }
                    });
                });
            });
        });
    }

    fn status_pill(&self, ui: &mut egui::Ui) {
        let p = self.palette;
        match &self.conn {
            Conn::Up(n) => chip(ui, &p, ic::CHECK, &if self.compact { "Connected".to_string() } else { format!("Connected · {n}") }, p.ok),
            Conn::Connecting => chip(ui, &p, ic::REFRESH, "Connecting…", p.warn),
            Conn::Down(m) => chip(ui, &p, ic::WARNING, &if self.compact { "Offline".to_string() } else { format!("Offline · {m}") }, p.bad),
            Conn::Idle => chip(ui, &p, ic::PLUGS, "Not connected", p.faint),
        }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let pad = if self.compact { 12.0 } else { 28.0 };
        let avail = ui.available_width();
        let w = (avail - 2.0 * pad).min(1100.0);
        let off = (avail - w) / 2.0;
        let title = match self.tab { Tab::Dashboard => "Dashboard", Tab::Power => "Power", Tab::Settings => "Settings", Tab::Screen => "" };
        let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(o) = self.dev_scroll { sa = sa.vertical_scroll_offset(o); }
        sa.show(ui, |ui| {
            ui.add_space(pad);
            ui.horizontal_top(|ui| {
                ui.add_space(off);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    split(ui, 44.0, |ui| { ui.label(heading(title, if self.compact { 24.0 } else { 28.0 }, p.text)); }, |ui| self.status_pill(ui));
                    ui.add_space(4.0);
                    match self.tab {
                        Tab::Dashboard => self.dashboard(ui, w),
                        Tab::Power => self.power(ui, w),
                        Tab::Settings => self.settings_tab(ui, w),
                        Tab::Screen => {}
                    }
                    ui.add_space(pad);
                });
            });
        });
    }

    fn logo_img(&self, ui: &mut egui::Ui, size: f32) {
        ui.add(egui::Image::new((self.logo.id(), Vec2::splat(size))));
    }

    fn nav_items(&self) -> Vec<(Tab, &'static str, &'static str)> {
        let mut v = vec![];
        if MOBILE { v.push((Tab::Screen, ic::MONITOR, "Screen")); }
        v.extend([(Tab::Dashboard, ic::CHART, "Dashboard"), (Tab::Power, ic::POWER, "Power"), (Tab::Settings, ic::GEAR, "Settings")]);
        v
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            self.logo_img(ui, 40.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(heading("PC Control", 16.0, p.text));
                let host = if let Conn::Up(n) = &self.conn { n.as_str() } else { "offline" };
                ui.label(RichText::new(host).size(12.0).color(p.muted));
            });
        });
        ui.add_space(22.0);
        for (tab, glyph, label) in self.nav_items() {
            let sel = self.tab == tab;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
            let rect = rect.shrink2(Vec2::new(10.0, 0.0));
            if sel { ui.painter().rect_filled(rect, CornerRadius::same(12), p.tint); }
            else if resp.hovered() { ui.painter().rect_filled(rect, CornerRadius::same(12), p.raised); }
            ui.painter().text(Pos2::new(rect.min.x + 16.0, rect.center().y), Align2::LEFT_CENTER, glyph, FontId::new(19.0, egui::FontFamily::Proportional), if sel { p.accent } else { p.muted });
            ui.painter().text(Pos2::new(rect.min.x + 46.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(14.5, bold()), if sel { p.text } else { p.muted });
            if resp.clicked() { self.tab = tab; }
        }
    }

    fn bottom_nav(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let items = self.nav_items();
        let w = ui.available_width() / items.len() as f32;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.horizontal(|ui| {
            for (tab, glyph, label) in items {
                let sel = self.tab == tab;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 64.0), Sense::click());
                let pill = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 20.0), Vec2::new(54.0, 30.0));
                if sel { ui.painter().rect_filled(pill, CornerRadius::same(99), p.tint); }
                ui.painter().text(pill.center(), Align2::CENTER_CENTER, glyph, FontId::new(21.0, egui::FontFamily::Proportional), if sel { p.accent } else { p.muted });
                ui.painter().text(Pos2::new(rect.center().x, rect.min.y + 46.0), Align2::CENTER_CENTER, label, FontId::new(11.5, bold()), if sel { p.text } else { p.muted });
                if resp.clicked() { self.tab = tab; }
            }
        });
    }
}

fn kv(ui: &mut egui::Ui, p: &Palette, k: &str, v: &str) {
    split(ui, 24.0, |ui| { ui.label(RichText::new(k).size(13.0).color(p.muted)); }, |ui| { ui.label(RichText::new(v).font(FontId::new(13.5, bold())).color(p.text)); });
}

fn logo_image() -> egui::ColorImage {
    let img = image::load_from_memory(include_bytes!("../../packaging/icon-512.png")).map(|i| i.to_rgba8()).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

#[cfg(target_os = "android")]
fn clipboard() -> Option<String> {
    use jni::{objects::{JObject, JString}, JavaVM};
    let app = crate::ANDROID_APP.get()?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _).ok()? };
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _) };
    let name = env.new_string("clipboard").ok()?;
    let cm = env.call_method(&activity, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[(&name).into()]).ok()?.l().ok()?;
    let clip = env.call_method(&cm, "getPrimaryClip", "()Landroid/content/ClipData;", &[]).ok()?.l().ok()?;
    if clip.is_null() { return None; }
    let item = env.call_method(&clip, "getItemAt", "(I)Landroid/content/ClipData$Item;", &[0.into()]).ok()?.l().ok()?;
    let cs = env.call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[]).ok()?.l().ok()?;
    if cs.is_null() { return None; }
    let s = env.call_method(&cs, "toString", "()Ljava/lang/String;", &[]).ok()?.l().ok()?;
    let out: String = env.get_string(&JString::from(s)).ok()?.into();
    Some(out)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        let p = self.palette;
        if self.handle.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.onboarding(ui));
            return;
        }
        self.sync_stream();
        self.compact = MOBILE || ui.available_width() < 720.0;
        if !MOBILE && self.tab == Tab::Screen { self.tab = Tab::Dashboard; }
        if self.compact {
            egui::Panel::bottom("nav").frame(egui::Frame::new().fill(p.bar).inner_margin(egui::Margin { left: 0, right: 0, top: 4, bottom: 10 })).show(ui, |ui| self.bottom_nav(ui));
            if self.tab == Tab::Screen {
                egui::Panel::bottom("keys").frame(egui::Frame::new().fill(p.page).inner_margin(egui::Margin::symmetric(10, 8))).show(ui, |ui| self.key_bar(ui));
            }
        } else {
            egui::Panel::left("side").exact_size(236.0).resizable(false).frame(egui::Frame::new().fill(p.bar)).show(ui, |ui| self.sidebar(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.page)).show(ui, |ui| {
            if self.tab == Tab::Screen && MOBILE {
                self.screen(ui);
                let portrait = ui.max_rect().height() > ui.max_rect().width();
                if !matches!(self.conn, Conn::Up(_)) {
                    egui::Area::new(ui.id().with("offline")).anchor(Align2::LEFT_TOP, Vec2::new(12.0, 12.0)).show(ui.ctx(), |ui| self.status_pill(ui));
                }
                if self.rotate_hint && portrait && self.tex.is_some() {
                    egui::Area::new(ui.id().with("rotate")).pivot(Align2::CENTER_BOTTOM).fixed_pos(Pos2::new(ui.max_rect().center().x, ui.max_rect().max.y - 12.0)).show(ui.ctx(), |ui| {
                        if pill_button(ui, &p, ic::PHONE, "Rotate your phone for a bigger view", false).clicked() { self.rotate_hint = false; }
                    });
                }
                if let Some(n) = self.note.clone() {
                    egui::Area::new(ui.id().with("note")).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 12.0)).show(ui.ctx(), |ui| {
                        if pill_button(ui, &p, ic::WARNING, &n, false).clicked() { self.note = None; }
                    });
                }
            } else {
                self.page(ui);
            }
        });
    }
}
