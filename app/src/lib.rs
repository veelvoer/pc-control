mod keys;
mod net;
mod ui;

use std::path::PathBuf;

#[cfg(target_os = "android")]
pub static ANDROID_APP: std::sync::OnceLock<android_activity::AndroidApp> = std::sync::OnceLock::new();

fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("PC Control").with_inner_size([1100.0, 700.0]).with_app_id("pccontrol"),
        ..Default::default()
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
