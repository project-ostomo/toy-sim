use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use toy_sim_model::Id;

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
    directory: PathBuf,
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.directory);
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
        "toy-sim-server.exe"
    } else {
        "toy-sim-server"
    });
    let mut check = false;
    while let Some(arg) = args.next() {
        if arg == "--ship" {
            ship = Some(std::fs::canonicalize(
                args.next().context("--ship requires a path")?,
            )?);
        } else if arg == "--server" {
            server_binary = PathBuf::from(args.next().context("--server requires an executable")?);
        } else if arg == "--check" {
            check = true;
        } else {
            bail!("usage: toy-sim-debug [--ship PATH] [--server EXECUTABLE] [--check]");
        }
    }
    ensure!(
        server_binary.is_file(),
        "build toy-sim-server first, or pass --server EXECUTABLE"
    );
    let directory = std::env::temp_dir().join(format!("toy-sim-debug-{}", Id::new()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory)?;
    let mut server = LocalServer {
        child: None,
        directory,
    };
    let account = Id::new();
    let account_key = SigningKey::from_bytes(&rand::random());
    let server_key = SigningKey::from_bytes(&rand::random());
    let config = ServerConfig {
        listen: "127.0.0.1:0".into(),
        server_secret: hex(&server_key.to_bytes()),
        debug_account: account.to_string(),
        ship,
        accounts: vec![Account {
            id: account.to_string(),
            public_key: hex(&account_key.verifying_key().to_bytes()),
        }],
    };
    let config_path = server.directory.join("server.toml");
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&config_path)?
        .write_all(toml::to_string(&config)?.as_bytes())?;
    let ready = server.directory.join("ready");
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
        toy_sim_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await?;
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
        toy_sim_client::ui::run(endpoint, true);
    }
    Ok(())
}
