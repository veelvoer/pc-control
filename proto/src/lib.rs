//! Wire protocol shared by host and client. Messages are postcard-encoded
//! and sent as binary WebSocket frames over TLS.
use serde::{Deserialize, Serialize};

pub mod theme;
pub use theme::ThemeColors;

pub const DEFAULT_PORT: u16 = 7878;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMsg {
    Auth { token: String },
    /// Start/stop streaming. `scale` is the max frame width in px.
    Stream { on: bool, max_width: u32, quality: u8, fps: u8 },
    Input(Input),
    Power(PowerAction),
    Ping,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Input {
    /// Normalized absolute position, 0.0..=1.0.
    MoveTo { x: f32, y: f32 },
    Button { button: Button, down: bool },
    Scroll { dx: i32, dy: i32 },
    /// Linux evdev key code.
    Key { code: u16, down: bool },
    Text(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button { Left, Right, Middle }

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction { Lock, Suspend, Reboot, Shutdown }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HostMsg {
    AuthOk { hostname: String },
    AuthFail,
    Frame { width: u32, height: u32, jpeg: Vec<u8> },
    Stats(Stats),
    Pong,
    Error(String),
    Theme(ThemeColors),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Stats {
    pub cpu: f32,
    pub cpu_cores: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    pub disks: Vec<Disk>,
    pub temp_c: Option<f32>,
    pub uptime_s: u64,
    pub load: [f32; 3],
    pub top: Vec<Proc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Disk { pub mount: String, pub used: u64, pub total: u64 }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Proc { pub name: String, pub cpu: f32, pub mem: u64 }

pub fn enc<T: Serialize>(m: &T) -> Vec<u8> {
    postcard::to_stdvec(m).expect("encode")
}
pub fn dec<'a, T: Deserialize<'a>>(b: &'a [u8]) -> Option<T> {
    postcard::from_bytes(b).ok()
}

/// Pairing string: `host:port#token#sha256hex` of the host certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing { pub addr: String, pub token: String, pub fingerprint: String }

impl Pairing {
    pub fn encode(&self) -> String { format!("{}#{}#{}", self.addr, self.token, self.fingerprint) }
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.trim().split('#');
        let (addr, token, fp) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() || addr.is_empty() || token.is_empty() || fp.len() != 64 { return None; }
        Some(Self { addr: addr.into(), token: token.into(), fingerprint: fp.to_lowercase() })
    }
}

/// US-layout ASCII -> (evdev code, needs shift).
pub fn char_to_key(c: char) -> Option<(u16, bool)> {
    const ROWS: [(&str, &str, u16); 4] = [
        ("1234567890-=", "!@#$%^&*()_+", 2),
        ("qwertyuiop[]", "QWERTYUIOP{}", 16),
        ("asdfghjkl;'`", "ASDFGHJKL:\"~", 30),
        ("\\zxcvbnm,./", "|ZXCVBNM<>?", 43),
    ];
    match c {
        ' ' => return Some((57, false)),
        '\n' => return Some((28, false)),
        '\t' => return Some((15, false)),
        _ => {}
    }
    for (plain, shifted, base) in ROWS {
        if let Some(i) = plain.chars().position(|p| p == c) { return Some((base + i as u16, false)); }
        if let Some(i) = shifted.chars().position(|p| p == c) { return Some((base + i as u16, true)); }
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys() {
        assert_eq!(char_to_key('a'), Some((30, false)));
        assert_eq!(char_to_key('A'), Some((30, true)));
        assert_eq!(char_to_key('?'), Some((53, true)));
        assert_eq!(char_to_key('0'), Some((11, false)));
        assert_eq!(char_to_key('é'), None);
    }
    #[test]
    fn pairing_roundtrip() {
        let p = Pairing { addr: "192.168.1.5:7878".into(), token: "abc".into(), fingerprint: "a".repeat(64) };
        assert_eq!(Pairing::parse(&p.encode()), Some(p));
        assert!(Pairing::parse("x#y").is_none());
    }
    #[test]
    fn msg_roundtrip() {
        let b = enc(&ClientMsg::Input(Input::Text("hé".into())));
        assert!(matches!(dec::<ClientMsg>(&b), Some(ClientMsg::Input(Input::Text(t))) if t == "hé"));
    }
}
