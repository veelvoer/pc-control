mod capture;
mod config;
mod input;
mod server;
mod stats;

use anyhow::Result;

fn main() -> Result<()> {
    // Under a systemd user service the compositor socket is not exported; fall back to the usual name.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-1");
    }
    let mut args = std::env::args().skip(1);
    let mut port = proto::DEFAULT_PORT;
    let mut cmd = "run".to_string();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "run" | "pair" | "reset-token" => cmd = a,
            "-h" | "--help" => {
                println!("pccontrol-host [run|pair|reset-token] [--port N]\n  run          start the host (default)\n  pair         print the pairing code for the phone app\n  reset-token  generate a new token (invalidates paired devices)");
                return Ok(());
            }
            _ => anyhow::bail!("unknown argument: {a} (try --help)"),
        }
    }
    let cfg = config::load(cmd == "reset-token")?;
    match cmd.as_str() {
        "pair" | "reset-token" => {
            println!("{}", cfg.pairing(port).encode());
            Ok(())
        }
        _ => {
            eprintln!("pccontrol-host listening on :{port}\npairing code: {}", cfg.pairing(port).encode());
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?
                .block_on(server::run(cfg, port))
        }
    }
}
