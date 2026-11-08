use crate::{capture, config::Config, input::Injector, stats::Sampler};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use proto::{dec, enc, ClientMsg, HostMsg, PowerAction};
use std::{
    process::Command,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_rustls::{rustls, TlsAcceptor};
use tokio_tungstenite::tungstenite::Message;

type Shared = Arc<Mutex<Option<Injector>>>;

pub async fn run(cfg: Config, port: u16) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cfg.cert_der.clone().into()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(cfg.key_der.clone().into()),
        )?;
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let inj: Shared = Arc::new(Mutex::new(match Injector::new() {
        Ok(i) => Some(i),
        Err(e) => { eprintln!("input disabled: {e:#}"); None }
    }));
    let token = Arc::new(cfg.token);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    loop {
        tokio::select! {
            r = listener.accept() => {
                let (sock, peer) = r?;
                let _ = sock.set_nodelay(true);
                let (acc, inj, token) = (acceptor.clone(), inj.clone(), token.clone());
                tokio::spawn(async move {
                    if let Err(e) = conn(sock, acc, inj, token).await {
                        // Clients that just close the socket (app killed, phone asleep) are routine.
                        if !format!("{e:#}").contains("close_notify") { eprintln!("{peer}: {e:#}"); }
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

struct StopOnDrop(Arc<AtomicBool>);
impl Drop for StopOnDrop { fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); } }

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

async fn conn(sock: tokio::net::TcpStream, acc: TlsAcceptor, inj: Shared, token: Arc<String>) -> Result<()> {
    let tls = tokio::time::timeout(Duration::from_secs(5), acc.accept(sock)).await??;
    let ws = tokio_tungstenite::accept_async(tls).await?;
    let (mut wr, mut rd) = ws.split();

    // First message must be a valid Auth.
    let first = tokio::time::timeout(Duration::from_secs(5), rd.next()).await?;
    let ok = matches!(first, Some(Ok(Message::Binary(ref b))) if matches!(dec::<ClientMsg>(b), Some(ClientMsg::Auth { token: ref t }) if ct_eq(t, &token)));
    if !ok {
        tokio::time::sleep(Duration::from_secs(1)).await; // slow down guessing
        wr.send(Message::Binary(enc(&HostMsg::AuthFail).into())).await?;
        return Ok(());
    }
    let hostname = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "pc".into());
    wr.send(Message::Binary(enc(&HostMsg::AuthOk { hostname }).into())).await?;
    // The service can start before logind grants /dev/uinput access at login, so retry per connection.
    let unavailable = {
        let mut g = inj.lock().unwrap();
        if g.is_none() {
            match Injector::new() {
                Ok(i) => *g = Some(i),
                Err(e) => eprintln!("input disabled: {e:#}"),
            }
        }
        g.is_none()
    };
    if unavailable {
        wr.send(Message::Binary(enc(&HostMsg::Error("input injection unavailable: no access to /dev/uinput".into())).into())).await?;
    }

    let (ftx, mut frx) = mpsc::channel::<Vec<u8>>(1);
    let mut stop = StopOnDrop(Arc::new(AtomicBool::new(true)));
    let mut sampler = Sampler::new();
    let mut theme: Option<proto::ThemeColors> = None;
    let mut ticks = 0u32;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            m = rd.next() => {
                let Some(m) = m else { break };
                let Message::Binary(b) = m? else { continue };
                match dec::<ClientMsg>(&b) {
                    Some(ClientMsg::Input(i)) => {
                        if let Some(inj) = inj.lock().unwrap().as_mut() {
                            if let Err(e) = inj.handle(i) { eprintln!("input: {e:#}"); }
                        }
                    }
                    Some(ClientMsg::Stream { on, max_width, quality, fps }) => {
                        stop.0.store(true, Ordering::Relaxed);
                        if on {
                            let flag = Arc::new(AtomicBool::new(false));
                            stop = StopOnDrop(flag.clone());
                            let (tx, s) = (ftx.clone(), capture::Settings { max_width: max_width.clamp(160, 3840), quality, fps });
                            std::thread::spawn(move || {
                                if let Err(e) = capture::run(s, tx.clone(), flag) {
                                    let _ = tx.try_send(enc(&HostMsg::Error(format!("{e:#}"))));
                                }
                            });
                        }
                    }
                    Some(ClientMsg::Power(a)) => power(a),
                    Some(ClientMsg::Ping) => wr.send(Message::Binary(enc(&HostMsg::Pong).into())).await?,
                    _ => {}
                }
            }
            _ = tick.tick() => {
                // Theme is re-checked every 5s while a client is connected (portal lookups shell out, so off the async path).
                if ticks % 5 == 0 {
                    if let Ok(t) = tokio::task::spawn_blocking(|| proto::theme::detect(true)).await {
                        if t != theme { theme = t.clone(); if let Some(t) = t { wr.send(Message::Binary(enc(&HostMsg::Theme(t)).into())).await?; } }
                    }
                }
                ticks += 1;
                wr.send(Message::Binary(enc(&HostMsg::Stats(sampler.sample())).into())).await?;
            }
            Some(f) = frx.recv() => wr.send(Message::Binary(f.into())).await?,
        }
    }
    Ok(())
}

fn power(a: PowerAction) {
    let (prog, args): (&str, &[&str]) = match a {
        PowerAction::Lock => ("loginctl", &["lock-session"]),
        PowerAction::Suspend => ("systemctl", &["suspend"]),
        PowerAction::Reboot => ("systemctl", &["reboot"]),
        PowerAction::Shutdown => ("systemctl", &["poweroff"]),
    };
    if let Err(e) = Command::new(prog).args(args).spawn() { eprintln!("power: {e}"); }
}
