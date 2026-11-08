//! In-app updates from GitHub releases (same idea as Hearth): look at the newest release,
//! show its notes, and install it. Only github.com over HTTPS is ever contacted.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io::{Read, Write}, sync::mpsc::Sender, time::Duration};

pub const REPO: &str = "veelvoer/pc-control";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Patch notes that ship inside the app, newest last.
pub const NOTES: &[(&str, &str)] = &[
    ("0.1.0", include_str!("../release-notes/0.1.0.md")),
    ("0.2.0", include_str!("../release-notes/0.2.0.md")),
    ("0.3.0", include_str!("../release-notes/0.3.0.md")),
    ("0.4.0", include_str!("../release-notes/0.4.0.md")),
];

#[derive(Clone, Debug)]
pub struct Asset { pub name: String, pub url: String, pub size: u64, pub sha256: Option<String> }

#[derive(Clone, Debug)]
pub struct Release { pub version: String, pub notes: String, pub assets: Vec<Asset> }

pub enum Msg {
    Checked(Result<Release, String>),
    Progress(f32),
    /// Desktop: installed, ready to restart. Android: download handed to the system.
    Done(String),
    Failed(String),
}

fn parts(v: &str) -> Vec<u32> { v.trim_start_matches('v').split('.').map(|n| n.parse().unwrap_or(0)).collect() }
pub fn newer(a: &str, b: &str) -> bool {
    let (x, y) = (parts(a), parts(b));
    for i in 0..3 {
        let (p, q) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        if p != q { return p > q; }
    }
    false
}

/// Markdown to plain lines: no `#` or `**`, bullets as dots.
pub fn plain(md: &str) -> Vec<String> {
    md.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| {
        let l = l.trim_start_matches('#').trim_start();
        let l = if let Some(r) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) { format!("• {r}") } else { l.to_string() };
        l.replace("**", "").replace('`', "")
    }).collect()
}

fn allowed(url: &str) -> bool {
    url.strip_prefix("https://").and_then(|r| r.split('/').next()).map_or(false, |h| {
        h == "github.com" || h == "api.github.com" || h.ends_with(".githubusercontent.com")
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(900)))
            .build(),
    )
}

pub fn latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent().get(&url).header("User-Agent", "pc-control-updater").header("Accept", "application/vnd.github+json")
        .call().map_err(|e| format!("could not reach GitHub ({e})"))?;
    let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("no release found")?;
    let assets = v["assets"].as_array().map(|a| a.iter().filter_map(|x| Some(Asset {
        name: x["name"].as_str()?.to_string(),
        url: x["browser_download_url"].as_str()?.to_string(),
        size: x["size"].as_u64().unwrap_or(0),
        sha256: x["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
    })).collect()).unwrap_or_default();
    Ok(Release { version: tag.trim_start_matches('v').to_string(), notes: v["body"].as_str().unwrap_or("").chars().take(2000).collect(), assets })
}

pub fn check(tx: Sender<Msg>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || { let _ = tx.send(Msg::Checked(latest())); ctx.request_repaint(); });
}

#[cfg(not(target_os = "android"))]
fn download(a: &Asset, dest: &std::path::Path, tx: &Sender<Msg>, ctx: &eframe::egui::Context) -> Result<(), String> {
    if !allowed(&a.url) { return Err("refusing to download from an unknown host".into()); }
    let mut resp = agent().get(&a.url).header("User-Agent", "pc-control-updater").call().map_err(|e| format!("download failed ({e})"))?;
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut rd = resp.body_mut().as_reader();
    let (mut buf, mut got, mut hash, mut last) = (vec![0u8; 64 * 1024], 0u64, Sha256::new(), 0.0f32);
    loop {
        let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hash.update(&buf[..n]);
        got += n as u64;
        if a.size > 0 {
            let p = got as f32 / a.size as f32;
            if p - last > 0.01 { last = p; let _ = tx.send(Msg::Progress(p.min(1.0))); ctx.request_repaint(); }
        }
    }
    drop(f);
    if a.size > 0 && got != a.size { let _ = std::fs::remove_file(&part); return Err("the download is incomplete".into()); }
    if let Some(want) = &a.sha256 {
        if hex::encode(hash.finalize()) != *want { let _ = std::fs::remove_file(&part); return Err("the download does not match its checksum".into()); }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// Downloads the new AppImage and runs its `install` (replaces the binaries, restarts the host service).
#[cfg(not(target_os = "android"))]
pub fn apply(rel: Release, tx: Sender<Msg>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let res = (|| -> Result<String, String> {
            let a = rel.assets.iter().find(|a| a.name.ends_with(".AppImage")).ok_or("this release has no desktop file")?;
            let dir = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
                .ok_or("no cache directory")?.join("pc-control");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let dest = dir.join(&a.name);
            download(a, &dest, &tx, &ctx)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
            let st = std::process::Command::new(&dest).arg("install").env("APPIMAGE_EXTRACT_AND_RUN", "1")
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map_err(|e| e.to_string())?;
            if !st.success() { return Err("the installer did not finish".into()); }
            let _ = std::fs::remove_file(&dest);
            Ok(format!("Version {} is installed. Restart to use it.", rel.version))
        })();
        let _ = tx.send(match res { Ok(m) => Msg::Done(m), Err(e) => Msg::Failed(e) });
        ctx.request_repaint();
    });
}

/// Starts the freshly installed app and quits this one.
#[cfg(not(target_os = "android"))]
pub fn restart() {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let installed = home.map(|h| h.join(".local/bin/pccontrol"));
    let exe = installed.filter(|p| p.exists()).or_else(|| std::env::current_exe().ok());
    if let Some(e) = exe { let _ = std::process::Command::new(e).spawn(); }
    std::process::exit(0);
}

/// Hands the APK to Android's DownloadManager; tapping the finished notification installs it.
#[cfg(target_os = "android")]
pub fn apply(rel: Release, tx: Sender<Msg>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let res = (|| -> Result<String, String> {
            let a = rel.assets.iter().find(|a| a.name.ends_with(".apk")).ok_or("this release has no phone app")?;
            if !allowed(&a.url) { return Err("refusing to download from an unknown host".into()); }
            android_download(&a.url, &a.name, &format!("PC Control {}", rel.version)).or_else(|_| android_open(&a.url))?;
            Ok("Downloading. When it finishes, open the notification to install.".to_string())
        })();
        let _ = tx.send(match res { Ok(m) => Msg::Done(m), Err(e) => Msg::Failed(e) });
        ctx.request_repaint();
    });
}

#[cfg(target_os = "android")]
fn with_env<R>(f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> jni::errors::Result<R>) -> Result<R, String> {
    use jni::{objects::JObject, JavaVM};
    let app = crate::ANDROID_APP.get().ok_or("no activity")?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) { let _ = env.exception_clear(); return Err("Android refused the request".into()); }
    r.map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
fn android_download(url: &str, file: &str, title: &str) -> Result<(), String> {
    with_env(|env, activity| {
        let u = env.new_string(url)?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[(&u).into()])?.l()?;
        let req = env.new_object("android/app/DownloadManager$Request", "(Landroid/net/Uri;)V", &[(&uri).into()])?;
        let t = env.new_string(title)?;
        env.call_method(&req, "setTitle", "(Ljava/lang/CharSequence;)Landroid/app/DownloadManager$Request;", &[(&t).into()])?;
        let m = env.new_string("application/vnd.android.package-archive")?;
        env.call_method(&req, "setMimeType", "(Ljava/lang/String;)Landroid/app/DownloadManager$Request;", &[(&m).into()])?;
        env.call_method(&req, "setNotificationVisibility", "(I)Landroid/app/DownloadManager$Request;", &[1.into()])?;
        let (d, f) = (env.new_string("Download")?, env.new_string(file)?);
        env.call_method(&req, "setDestinationInExternalPublicDir", "(Ljava/lang/String;Ljava/lang/String;)Landroid/app/DownloadManager$Request;", &[(&d).into(), (&f).into()])?;
        let name = env.new_string("download")?;
        let dm = env.call_method(activity, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[(&name).into()])?.l()?;
        env.call_method(&dm, "enqueue", "(Landroid/app/DownloadManager$Request;)J", &[(&req).into()])?;
        Ok(())
    })
}

/// Fallback: let the browser download it.
#[cfg(target_os = "android")]
fn android_open(url: &str) -> Result<(), String> {
    with_env(|env, activity| {
        let u = env.new_string(url)?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[(&u).into()])?.l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[(&action).into(), (&uri).into()])?;
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[0x1000_0000.into()])?;
        env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[(&intent).into()])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(newer("0.4.0", "0.3.9"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(!newer("0.3.0", "0.3.0"));
        assert!(!newer("0.2.9", "0.3.0"));
    }
    #[test]
    fn hosts() {
        assert!(allowed("https://github.com/x/y/releases/download/v1/a.apk"));
        assert!(allowed("https://objects.githubusercontent.com/a"));
        assert!(!allowed("http://github.com/a"));
        assert!(!allowed("https://evil.com/github.com"));
        assert!(!allowed("https://github.com.evil.com/a"));
    }
    #[test]
    fn plain_md() {
        assert_eq!(plain("## T\n- a **b** `c`\n\n"), vec!["T".to_string(), "• a b c".to_string()]);
    }
}
