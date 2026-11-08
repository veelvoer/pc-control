#![allow(deprecated, dead_code)]
mod icons;
mod keys;
mod net;
mod theme;
mod ui;
mod update;
mod widgets;

use std::path::PathBuf;

#[cfg(target_os = "android")]
pub static ANDROID_APP: std::sync::OnceLock<android_activity::AndroidApp> = std::sync::OnceLock::new();

fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PC Control")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([380.0, 560.0])
            .with_app_id("pccontrol")
            .with_icon(icon()),
        ..Default::default()
    }
}

fn icon() -> eframe::egui::IconData {
    match image::load_from_memory(include_bytes!("../../packaging/icon.png")) {
        Ok(i) => { let i = i.to_rgba8(); eframe::egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() } }
        Err(_) => Default::default(),
    }
}

fn run(cfg: PathBuf, opts: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native("PC Control", opts, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, cfg)))))
}

#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("pc-control");
    let _ = std::fs::create_dir_all(&base);
    run(base.join("client.txt"), options())
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let dir = app.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp"));
    let _ = ANDROID_APP.set(app.clone());
    let mut opts = options();
    opts.android_app = Some(app);
    if let Err(e) = run(dir.join("client.txt"), opts) {
        log::error!("{e}");
    }
}
