use futures_util::{SinkExt, StreamExt};
use proto::{dec, enc, ClientMsg, HostMsg, Pairing, Stats};
use sha2::{Digest, Sha256};
use std::{
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender}};
use tokio_rustls::{
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        crypto::CryptoProvider,
        pki_types::{CertificateDer, ServerName, UnixTime},
        DigitallySignedStruct, SignatureScheme,
    },
    TlsConnector,
};
use tokio_tungstenite::tungstenite::Message;

pub enum Event {
    Connecting,
    Connected(String),
    Frame(eframe::egui::ColorImage),
    Stats(Stats),
    Theme(proto::ThemeColors),
    /// Fatal for this session (e.g. wrong token); no auto-retry.
    Fatal(String),
    /// Transient; the worker retries.
    Lost(String),
    HostError(String),
}

pub struct Handle {
    pub tx: UnboundedSender<ClientMsg>,
    pub rx: mpsc::Receiver<Event>,
}

pub fn spawn(p: Pairing, ctx: eframe::egui::Context) -> Handle {
    let (tx, cmd_rx) = unbounded_channel();
    let (ev_tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(worker(p, cmd_rx, ev_tx, ctx));
    });
    Handle { tx, rx }
}

#[derive(Debug)]
struct Pin { fp: Vec<u8>, provider: Arc<CryptoProvider> }

impl ServerCertVerifier for Pin {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if Sha256::digest(end.as_ref()).as_slice() == self.fp.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("host certificate does not match the pairing code".into()))
        }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

async fn worker(p: Pairing, mut cmds: UnboundedReceiver<ClientMsg>, ev: mpsc::Sender<Event>, ctx: eframe::egui::Context) {
    let send = |e: Event| { let ok = ev.send(e).is_ok(); ctx.request_repaint(); ok };
    let mut stream_cfg: Option<ClientMsg> = None;
    let mut backoff = 1u64;
    loop {
        if !send(Event::Connecting) { return; }
        match session(&p, &mut cmds, &send, &mut stream_cfg, &mut backoff).await {
            Ok(()) => {}
            Err(Exit::Fatal(m)) => { send(Event::Fatal(m)); return; }
            Err(Exit::UiGone) => return,
            Err(Exit::Lost(m)) => { if !send(Event::Lost(m)) { return; } }
        }
        // Wait out the backoff but keep draining commands so the UI stays responsive.
        let until = Instant::now() + Duration::from_secs(backoff);
        while Instant::now() < until {
            match tokio::time::timeout(Duration::from_millis(200), cmds.recv()).await {
                Ok(Some(m @ ClientMsg::Stream { .. })) => stream_cfg = Some(m),
                Ok(None) => return,
                _ => {}
            }
        }
        backoff = (backoff * 2).min(8);
    }
}

enum Exit { Fatal(String), Lost(String), UiGone }
impl<E: std::fmt::Display> From<E> for Exit { fn from(e: E) -> Self { Exit::Lost(e.to_string()) } }

async fn session(
    p: &Pairing,
    cmds: &mut UnboundedReceiver<ClientMsg>,
    send: &impl Fn(Event) -> bool,
    stream_cfg: &mut Option<ClientMsg>,
    backoff: &mut u64,
) -> Result<(), Exit> {
    let fp = hex::decode(&p.fingerprint).map_err(|_| Exit::Fatal("bad fingerprint in pairing code".into()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls_cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin { fp, provider }))
        .with_no_client_auth();

    let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&p.addr))
        .await
        .map_err(|_| Exit::Lost("connection timed out".into()))??;
    tcp.set_nodelay(true).ok();
    let tls = TlsConnector::from(Arc::new(tls_cfg))
        .connect(ServerName::try_from("pc-control").unwrap(), tcp)
        .await
        .map_err(|e| Exit::Fatal(format!("TLS: {e}")))?;
    let (ws, _) = tokio_tungstenite::client_async("wss://pc-control/", tls).await?;
    let (mut wr, mut rd) = ws.split();

    wr.send(Message::Binary(enc(&ClientMsg::Auth { token: p.token.clone() }).into())).await?;
    let reply = tokio::time::timeout(Duration::from_secs(5), rd.next())
        .await
        .map_err(|_| Exit::Lost("no auth reply".into()))?;
    match reply {
        Some(Ok(Message::Binary(b))) => match dec::<HostMsg>(&b) {
            Some(HostMsg::AuthOk { hostname }) => { if !send(Event::Connected(hostname)) { return Err(Exit::UiGone); } }
            Some(HostMsg::AuthFail) => return Err(Exit::Fatal("wrong token (pair again)".into())),
            _ => return Err(Exit::Lost("unexpected reply".into())),
        },
        _ => return Err(Exit::Lost("connection closed during auth".into())),
    }
    *backoff = 1;
    if let Some(m) = stream_cfg.as_ref() { wr.send(Message::Binary(enc(m).into())).await?; }

    let mut last_rx = Instant::now();
    let mut watchdog = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            m = rd.next() => {
                let Some(m) = m else { return Err(Exit::Lost("host closed the connection".into())) };
                let Message::Binary(b) = m? else { continue };
                last_rx = Instant::now();
                let ok = match dec::<HostMsg>(&b) {
                    Some(HostMsg::Frame { jpeg, .. }) => match image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            send(Event::Frame(eframe::egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())))
                        }
                        Err(_) => true,
                    },
                    Some(HostMsg::Stats(s)) => send(Event::Stats(s)),
                    Some(HostMsg::Theme(t)) => send(Event::Theme(t)),
                    Some(HostMsg::Error(e)) => send(Event::HostError(e)),
                    _ => true,
                };
                if !ok { return Err(Exit::UiGone); }
            }
            c = cmds.recv() => {
                let Some(c) = c else { return Err(Exit::UiGone) };
                if matches!(c, ClientMsg::Stream { .. }) { *stream_cfg = Some(c.clone()); }
                wr.send(Message::Binary(enc(&c).into())).await?;
            }
            _ = watchdog.tick() => {
                if last_rx.elapsed() > Duration::from_secs(6) { return Err(Exit::Lost("host not responding".into())); }
            }
        }
    }
}
