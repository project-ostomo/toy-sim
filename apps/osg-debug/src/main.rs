use anyhow::{Context, Result, bail, ensure};
use osg_model::Id;
use serde::Serialize;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

mod state;

#[derive(Serialize)]
struct ServerConfig {
    listen: String,
    server_secret: String,
    debug_account: String,
    ship: Option<PathBuf>,
    accounts: Vec<Account>,
}

#[derive(Serialize)]
struct Account {
    id: String,
    public_key: String,
}

struct LocalServer {
    child: Option<Child>,
    state: state::StateDirectory,
}

impl LocalServer {
    fn shutdown(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        ensure!(status.success(), "local server shutdown failed: {status}");
                        break;
                    }
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("local server did not finish saving within 120 seconds");
                    }
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        Ok(())
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("Local server shutdown: {error:#}");
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut ship = None;
    let mut server_binary = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "osg-server.exe"
    } else {
        "osg-server"
    });
    let mut check = false;
    let mut state_directory = None;
    let mut ephemeral = false;
    while let Some(arg) = args.next() {
        if arg == "--ship" {
            let path = PathBuf::from(args.next().context("--ship requires a path")?);
            ship = Some(std::fs::canonicalize(&path).or_else(|_| std::path::absolute(path))?);
        } else if arg == "--server" {
            server_binary = PathBuf::from(args.next().context("--server requires an executable")?);
        } else if arg == "--check" {
            check = true;
        } else if arg == "--state-dir" {
            state_directory = Some(PathBuf::from(
                args.next().context("--state-dir requires a path")?,
            ));
        } else if arg == "--ephemeral" {
            ephemeral = true;
        } else {
            bail!(
                "usage: osg-debug [--ship PATH] [--server EXECUTABLE] [--state-dir PATH | --ephemeral] [--check]"
            );
        }
    }
    ensure!(
        server_binary.is_file(),
        "build osg-server first, or pass --server EXECUTABLE"
    );
    ensure!(
        !ephemeral || state_directory.is_none(),
        "--ephemeral and --state-dir cannot be combined"
    );
    let directory = if ephemeral {
        std::env::temp_dir().join(format!("osg-debug-{}", Id::new()))
    } else {
        state_directory.map_or_else(state::default_path, Ok)?
    };
    let state = state::StateDirectory::open(directory, ephemeral, ship)?;
    eprintln!("Debug world: {}", state.path.display());
    let mut server = LocalServer { child: None, state };
    let account = server.state.identity.account;
    let account_key = server.state.identity.account_key();
    let server_key = server.state.identity.server_key();
    let config = ServerConfig {
        listen: "127.0.0.1:0".into(),
        server_secret: hex(&server_key.to_bytes()),
        debug_account: account.to_string(),
        ship: server.state.identity.ship.clone(),
        accounts: vec![Account {
            id: account.to_string(),
            public_key: hex(&account_key.verifying_key().to_bytes()),
        }],
    };
    let config_path = server.state.path.join("server.toml");
    state::write_private(&config_path, &toml::to_string(&config)?)?;
    let ready = server.state.path.join("ready");
    if ready.exists() {
        std::fs::remove_file(&ready)?;
    }
    server.child = Some(
        Command::new(&server_binary)
            .arg(&config_path)
            .arg("--ready-file")
            .arg(&ready)
            .arg("--shutdown-on-stdin-close")
            .stdin(Stdio::piped())
            .spawn()
            .context("start local server")?,
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let address = loop {
        if let Some(status) = server.child.as_mut().unwrap().try_wait()? {
            bail!("local server exited before becoming ready: {status}");
        }
        if let Ok(address) = std::fs::read_to_string(&ready) {
            if address.parse::<std::net::SocketAddr>().is_ok() {
                break address;
            }
        }
        ensure!(
            Instant::now() < deadline,
            "local server readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let mut endpoint =
        osg_client::connect(&address, server_key.verifying_key(), account, &account_key).await?;
    if check {
        let frame = tokio::time::timeout(Duration::from_secs(10), endpoint.state.recv())
            .await?
            .context("server disconnected before its first frame")?;
        ensure!(
            !frame.ships.is_empty(),
            "server did not provision the debug ship"
        );
        println!(
            "Connected to {address}; received tick {} with {} ship(s)",
            frame.tick,
            frame.ships.len()
        );
    } else {
        osg_client::ui::run(endpoint, true);
    }
    server.shutdown()
}
