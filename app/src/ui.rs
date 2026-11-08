use crate::{keys, net};
use eframe::egui::{self, Color32, Event, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use proto::{Button, ClientMsg, Input, Pairing, PowerAction, Stats};
use std::{collections::VecDeque, path::PathBuf, time::Instant};

#[derive(PartialEq, Clone, Copy)]
enum Tab { Screen, Stats, Power }

enum Conn { Idle, Connecting, Up(String), Down(String) }

const HIST: usize = 90;

pub struct App {
    cfg: PathBuf,
    code: String,
    handle: Option<net::Handle>,
    conn: Conn,
    note: Option<String>,
    tex: Option<egui::TextureHandle>,
    stats: Option<Stats>,
    hist: VecDeque<(f32, f32, f32, f32)>, // cpu%, mem%, rx, tx
    tab: Tab,
    // input state
    touch_seen: bool,
    drag_mode: bool,
    right_next: bool,
    kb_open: bool,
    sticky: [bool; 4], // ctrl, alt, shift, super
    phys: [bool; 3],   // ctrl, alt, shift held on a physical keyboard
    scroll_acc: f32,
    last_move: Option<(Pos2, Instant)>,
    // stream settings
    quality: u8,
    max_width: u32,
    fps: u8,
    stream_sent: Option<(Tab, u8, u32, u8)>,
    confirm: Option<PowerAction>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: PathBuf) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let code = std::fs::read_to_string(&cfg).unwrap_or_default().trim().to_string();
        let mut app = Self {
            cfg, code, handle: None, conn: Conn::Idle, note: None, tex: None, stats: None, hist: VecDeque::new(),
            tab: Tab::Screen, touch_seen: false, drag_mode: false, right_next: false, kb_open: false,
            sticky: [false; 4], phys: [false; 3], scroll_acc: 0.0, last_move: None,
            quality: 60, max_width: 1280, fps: 30, stream_sent: None, confirm: None,
        };
        if Pairing::parse(&app.code).is_some() { app.connect(&cc.egui_ctx); }
        app
    }

    fn connect(&mut self, ctx: &egui::Context) {
        match Pairing::parse(&self.code) {
            Some(p) => {
                let _ = std::fs::write(&self.cfg, &self.code);
                self.handle = Some(net::spawn(p, ctx.clone()));
                self.conn = Conn::Connecting;
                self.stream_sent = None;
                self.note = None;
            }
            None => self.note = Some("Invalid pairing code. Run `pccontrol-host pair` on your PC.".into()),
        }
    }

    fn send(&self, m: ClientMsg) {
        if let Some(h) = &self.handle { let _ = h.tx.send(m); }
    }
    fn input(&self, i: Input) { self.send(ClientMsg::Input(i)); }
    fn key(&self, code: u16, down: bool) { self.input(Input::Key { code, down }); }
    fn tap_key(&self, code: u16) { self.key(code, true); self.key(code, false); }

    fn pump(&mut self, ctx: &egui::Context) {
        let Some(h) = &self.handle else { return };
        let mut drop_handle = false;
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
                net::Event::Lost(m) => self.conn = Conn::Down(m),
                net::Event::HostError(m) => self.note = Some(m),
                net::Event::Fatal(m) => { self.conn = Conn::Down(m); drop_handle = true; }
            }
        }
        if drop_handle { self.handle = None; }
    }

    /// Only stream while the Screen tab is visible; saves host CPU and bandwidth.
    fn sync_stream(&mut self) {
        if !matches!(self.conn, Conn::Up(_)) { return; }
        let want = (self.tab, self.quality, self.max_width, self.fps);
        if self.stream_sent != Some(want) {
            self.send(ClientMsg::Stream { on: self.tab == Tab::Screen, max_width: self.max_width, quality: self.quality, fps: self.fps });
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
                        // A sticky modifier is armed: turn text into real key chords.
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
                        if now[i] != self.phys[i] {
                            self.phys[i] = now[i];
                            if !self.sticky[i] { self.key(code, now[i]); }
                        }
                    }
                    let Some((code, is_text)) = keys::map(key) else { continue };
                    let chord = modifiers.ctrl || modifiers.alt;
                    if is_text && !chord { continue; } // delivered as Event::Text
                    self.key(code, pressed);
                    if !pressed { self.release_sticky(); }
                }
                _ => {}
            }
        }
        // Modifier release without a key event (e.g. Ctrl let go alone).
        let m = ctx.input(|i| i.modifiers);
        for (i, (now, code)) in [(m.ctrl, keys::CTRL), (m.alt, keys::ALT), (m.shift, keys::SHIFT)].into_iter().enumerate() {
            if now != self.phys[i] {
                self.phys[i] = now;
                if !self.sticky[i] { self.key(code, now); }
            }
        }
    }

    fn screen(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let avail = ui.available_rect_before_wrap();
        let Some(tex) = self.tex.clone() else {
            ui.centered_and_justified(|ui| { ui.label("Waiting for first frame…"); });
            return;
        };
        let ts = tex.size_vec2();
        let scale = (avail.width() / ts.x).min(avail.height() / ts.y);
        let rect = Rect::from_center_size(avail.center(), ts * scale);
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.painter().image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

        let norm = |p: Pos2| Input::MoveTo {
            x: ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
            y: ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
        };
        let (events, multi) = ctx.input(|i| (i.events.clone(), i.multi_touch()));
        if events.iter().any(|e| matches!(e, Event::Touch { .. })) { self.touch_seen = true; }

        // Pointer position (throttled to ~120 Hz and skipped while two fingers scroll).
        if multi.is_none() {
            if let Some(p) = resp.hover_pos().or(resp.interact_pointer_pos()) {
                let due = self.last_move.map_or(true, |(lp, t)| lp != p && t.elapsed().as_millis() >= 8);
                if due { self.input(norm(p)); self.last_move = Some((p, Instant::now())); }
            }
        }

        for e in &events {
            match e {
                Event::PointerButton { pos, button, pressed, .. } if rect.contains(*pos) || !pressed => {
                    let b = match button {
                        egui::PointerButton::Primary => Button::Left,
                        egui::PointerButton::Secondary => Button::Right,
                        egui::PointerButton::Middle => Button::Middle,
                        _ => continue,
                    };
                    // On touch screens a plain tap is handled below; here only hold-to-drag and real mice.
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
            if let Some(p) = resp.interact_pointer_pos() {
                let b = if std::mem::take(&mut self.right_next) { Button::Right } else { Button::Left };
                self.input(norm(p));
                self.input(Input::Button { button: b, down: true });
                self.input(Input::Button { button: b, down: false });
            }
        }
        // Two-finger drag scrolls (content follows the fingers).
        if let Some(mt) = multi {
            self.scroll_acc += mt.translation_delta.y;
            let notches = (self.scroll_acc / 24.0).trunc();
            if notches != 0.0 {
                self.scroll_acc -= notches * 24.0;
                self.input(Input::Scroll { dx: 0, dy: notches as i32 });
            }
        } else {
            self.scroll_acc = 0.0;
        }

        if self.kb_open || !self.touch_seen { self.forward_keyboard(&ctx); }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            #[cfg(target_os = "android")]
            if ui.selectable_label(self.kb_open, "Keyboard").clicked() {
                self.kb_open = !self.kb_open;
                if let Some(a) = crate::ANDROID_APP.get() {
                    if self.kb_open { a.show_soft_input(false) } else { a.hide_soft_input(false) }
                }
            }
            if self.touch_seen {
                ui.toggle_value(&mut self.drag_mode, "Hold-drag");
                ui.toggle_value(&mut self.right_next, "Right-click");
            }
            for (code, label) in [(1, "Esc"), (15, "Tab"), (28, "Enter"), (14, "Bksp"), (105, "Left"), (103, "Up"), (108, "Down"), (106, "Right")] {
                if ui.button(label).clicked() { self.tap_key(code); }
            }
            for (i, (code, label)) in [(keys::CTRL, "Ctrl"), (keys::ALT, "Alt"), (keys::SHIFT, "Shift"), (keys::SUPER, "Super")].into_iter().enumerate() {
                if ui.selectable_label(self.sticky[i], label).clicked() {
                    self.sticky[i] = !self.sticky[i];
                    self.key(code, self.sticky[i]);
                }
            }
        });
    }

    fn stats_tab(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.stats else { ui.label("Waiting for stats…"); return; };
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        let rate = |b: u64| if b > 1 << 20 { format!("{:.1} MB/s", b as f64 / 1048576.0) } else { format!("{:.0} KB/s", b as f64 / 1024.0) };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("CPU");
            ui.add(egui::ProgressBar::new(s.cpu / 100.0).text(format!("{:.0}%", s.cpu)));
            graph(ui, self.hist.iter().map(|h| h.0 / 100.0).collect(), Color32::from_rgb(80, 160, 255));
            ui.horizontal_wrapped(|ui| {
                for (i, c) in s.cpu_cores.iter().enumerate() { ui.label(format!("{i}: {c:.0}%")); }
            });
            ui.heading("Memory");
            let m = s.mem_used as f32 / s.mem_total.max(1) as f32;
            ui.add(egui::ProgressBar::new(m).text(format!("{:.1} / {:.1} GiB", gib(s.mem_used), gib(s.mem_total))));
            if s.swap_total > 0 {
                ui.add(egui::ProgressBar::new(s.swap_used as f32 / s.swap_total as f32).text(format!("swap {:.1} / {:.1} GiB", gib(s.swap_used), gib(s.swap_total))));
            }
            ui.heading("Network");
            ui.label(format!("Down {}   Up {}", rate(s.net_rx_bps), rate(s.net_tx_bps)));
            let peak = self.hist.iter().map(|h| h.2.max(h.3)).fold(1.0, f32::max);
            graph(ui, self.hist.iter().map(|h| h.2 / peak).collect(), Color32::from_rgb(90, 200, 120));
            graph(ui, self.hist.iter().map(|h| h.3 / peak).collect(), Color32::from_rgb(240, 170, 60));
            ui.heading("Disks");
            for d in &s.disks {
                ui.add(egui::ProgressBar::new(d.used as f32 / d.total.max(1) as f32).text(format!("{}  {:.0} / {:.0} GiB", d.mount, gib(d.used), gib(d.total))));
            }
            ui.heading("System");
            let up = s.uptime_s;
            ui.label(format!("Uptime {}d {}h {}m   Load {:.2} {:.2} {:.2}", up / 86400, up / 3600 % 24, up / 60 % 60, s.load[0], s.load[1], s.load[2]));
            if let Some(t) = s.temp_c { ui.label(format!("Hottest sensor {t:.0} C")); }
            ui.heading("Top processes");
            for p in &s.top { ui.label(format!("{:<24} {:>5.1}%  {:>6.0} MiB", p.name, p.cpu, p.mem as f64 / 1048576.0)); }
        });
    }

    fn power_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Power");
        for (a, label, danger) in [
            (PowerAction::Lock, "Lock screen", false),
            (PowerAction::Suspend, "Suspend", false),
            (PowerAction::Reboot, "Reboot", true),
            (PowerAction::Shutdown, "Shut down", true),
        ] {
            let armed = self.confirm == Some(a);
            let text = if armed { format!("Tap again to confirm: {label}") } else { label.to_string() };
            if ui.add_sized([260.0, 44.0], egui::Button::new(text)).clicked() {
                if danger && !armed { self.confirm = Some(a); } else { self.confirm = None; self.send(ClientMsg::Power(a)); }
            }
        }
        ui.separator();
        ui.label("Stream quality");
        ui.add(egui::Slider::new(&mut self.quality, 20..=90).text("JPEG"));
        ui.add(egui::Slider::new(&mut self.max_width, 640..=1920).text("max width"));
        ui.add(egui::Slider::new(&mut self.fps, 5..=60).text("fps"));
        ui.separator();
        if ui.button("Forget this PC / re-pair").clicked() {
            self.handle = None;
            self.conn = Conn::Idle;
            self.tex = None;
            self.stats = None;
            self.code.clear();
            let _ = std::fs::remove_file(&self.cfg);
        }
    }

    fn pairing_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.heading("Pair with your PC");
        ui.label("On the PC run `pccontrol-host pair` and paste the code here.");
        let r = ui.add(egui::TextEdit::singleline(&mut self.code).hint_text("192.168.x.x:7878#token#fingerprint").desired_width(f32::INFINITY));
        #[cfg(target_os = "android")]
        {
            if r.gained_focus() { if let Some(a) = crate::ANDROID_APP.get() { a.show_soft_input(false); } }
            if ui.button("Paste from clipboard").clicked() {
                if let Some(t) = clipboard() { self.code = t.trim().to_string(); }
            }
        }
        let _ = r;
        if ui.add_sized([160.0, 40.0], egui::Button::new("Connect")).clicked() { self.connect(ui.ctx()); }
        if let Conn::Down(m) = &self.conn { ui.colored_label(Color32::LIGHT_RED, m); }
        if let Some(n) = &self.note { ui.colored_label(Color32::LIGHT_RED, n); }
    }
}

fn graph(ui: &mut egui::Ui, pts: Vec<f32>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, Color32::from_gray(24));
    if pts.len() < 2 { return; }
    let line: Vec<Pos2> = pts.iter().enumerate().map(|(i, v)| {
        Pos2::new(rect.min.x + rect.width() * i as f32 / (HIST - 1) as f32, rect.max.y - 2.0 - (rect.height() - 4.0) * v.clamp(0.0, 1.0))
    }).collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, color)));
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
        if self.handle.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.pairing_screen(ui));
            return;
        }
        self.sync_stream();
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Screen, "Screen");
                ui.selectable_value(&mut self.tab, Tab::Stats, "Stats");
                ui.selectable_value(&mut self.tab, Tab::Power, "Settings");
                ui.separator();
                match &self.conn {
                    Conn::Up(n) => ui.colored_label(Color32::LIGHT_GREEN, format!("Connected: {n}")),
                    Conn::Connecting => ui.colored_label(Color32::YELLOW, "connecting…"),
                    Conn::Down(m) => ui.colored_label(Color32::LIGHT_RED, format!("{m} – retrying")),
                    Conn::Idle => ui.label(""),
                };
                if let Some(n) = self.note.clone() {
                    if ui.link(RichText::new(format!("! {n}")).color(Color32::LIGHT_RED)).clicked() { self.note = None; }
                }
            });
        });
        if self.tab == Tab::Screen {
            egui::Panel::bottom("kb").show(ui, |ui| self.toolbar(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| match self.tab {
            Tab::Screen => self.screen(ui),
            Tab::Stats => egui::Frame::NONE.inner_margin(8.0).show(ui, |ui| self.stats_tab(ui)).inner,
            Tab::Power => egui::Frame::NONE.inner_margin(8.0).show(ui, |ui| self.power_tab(ui)).inner,
        });
    }
}
