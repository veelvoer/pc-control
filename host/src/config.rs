use anyhow::{Context, Result};
use proto::Pairing;
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

pub struct Config {
    pub token: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

fn dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no HOME")?;
    let d = base.join("pc-control");
    fs::create_dir_all(&d)?;
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
    Ok(d)
}

fn write_private(p: &PathBuf, data: &[u8]) -> Result<()> {
    fs::write(p, data)?;
    fs::set_permissions(p, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn load(reset_token: bool) -> Result<Config> {
    let d = dir()?;
    let (tp, cp, kp) = (d.join("token"), d.join("cert.der"), d.join("key.der"));
    if reset_token || !tp.exists() {
        let mut b = [0u8; 24];
        getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("rng: {e}"))?;
        write_private(&tp, hex::encode(b).as_bytes())?;
    }
    if !cp.exists() || !kp.exists() {
        let ck = rcgen::generate_simple_self_signed(vec!["pc-control".to_string()])?;
        write_private(&cp, ck.cert.der())?;
        write_private(&kp, &ck.signing_key.serialize_der())?;
    }
    Ok(Config {
        token: fs::read_to_string(&tp)?.trim().to_string(),
        cert_der: fs::read(&cp)?,
        key_der: fs::read(&kp)?,
    })
}

impl Config {
    pub fn pairing(&self, port: u16) -> Pairing {
        Pairing {
            addr: format!("{}:{port}", lan_ip()),
            token: self.token.clone(),
            fingerprint: hex::encode(Sha256::digest(&self.cert_der)),
        }
    }
}

/// Best-effort LAN address (UDP connect sends no packets).
fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").and_then(|_| s.local_addr()))
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "HOST_IP".into())
}
