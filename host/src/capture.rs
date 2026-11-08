use anyhow::{Context, Result};
use image::imageops::{resize, FilterType};
use libwayshot::WayshotConnection;
use proto::HostMsg;
use std::{
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{error::TrySendError, Sender};

pub struct Settings { pub max_width: u32, pub quality: u8, pub fps: u8 }

fn hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for c in b.chunks(32) {
        h = (h ^ c[0] as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Blocking capture loop; run on a dedicated thread. Exits when `stop` is set
/// or the receiver is dropped. Unchanged frames are skipped, so an idle
/// desktop costs almost nothing.
pub fn run(s: Settings, tx: Sender<Vec<u8>>, stop: Arc<AtomicBool>) -> Result<()> {
    let wl = WayshotConnection::new().context("wayland screencopy unavailable (needs a wlroots-style compositor such as Hyprland/Sway)")?;
    let period = Duration::from_secs_f32(1.0 / s.fps.clamp(1, 60) as f32);
    let (mut last_hash, mut last_sent) = (0u64, Instant::now() - Duration::from_secs(10));
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        let img = wl.screenshot_all(true).context("capture failed")?.to_rgb8();
        let (w, h) = img.dimensions();
        let img = if w > s.max_width {
            resize(&img, s.max_width, (h * s.max_width / w).max(1), FilterType::Triangle)
        } else {
            img
        };
        let hsh = hash(img.as_raw());
        // Resend at least every 2s so a freshly-opened view always gets a frame.
        if hsh != last_hash || last_sent.elapsed() > Duration::from_secs(2) {
            let (w, h) = img.dimensions();
            let mut jpeg = Vec::with_capacity(64 * 1024);
            jpeg_encoder::Encoder::new(&mut jpeg, s.quality.clamp(10, 95))
                .encode(img.as_raw(), w as u16, h as u16, jpeg_encoder::ColorType::Rgb)?;
            match tx.try_send(proto::enc(&HostMsg::Frame { width: w, height: h, jpeg })) {
                Ok(()) => { last_hash = hsh; last_sent = Instant::now(); }
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => return Ok(()),
            }
        }
        if let Some(rest) = period.checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    Ok(())
}
