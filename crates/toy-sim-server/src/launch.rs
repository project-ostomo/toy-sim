use anyhow::{Context, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use toy_sim_model::AccountId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: String,
    server_secret: String,
    accounts: Vec<Account>,
    debug_account: Option<String>,
    ship: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    id: String,
    public_key: String,
}

#[derive(Default)]
pub struct Options {
    pub ready_file: Option<PathBuf>,
    pub shutdown_on_stdin_close: bool,
}

pub async fn run(path: &Path, options: Options) -> Result<()> {
    let config: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
    let key = SigningKey::from_bytes(&crate::key_bytes(&config.server_secret)?);
    let accounts: BTreeMap<AccountId, VerifyingKey> = config
        .accounts
        .into_iter()
        .map(|account| {
            Ok((
                account.id.parse()?,
                VerifyingKey::from_bytes(&crate::key_bytes(&account.public_key)?)?,
            ))
        })
        .collect::<Result<_>>()?;
    let debug_account = config.debug_account.map(|id| id.parse()).transpose()?;
    if let Some(account) = debug_account {
        anyhow::ensure!(
            accounts.contains_key(&account),
            "debug account must be authorized"
        );
    }
    let ship = config.ship.map(|ship| {
        if ship.is_absolute() {
            ship
        } else {
            path.parent().unwrap_or(Path::new(".")).join(ship)
        }
    });
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    let address = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let (send, receive) = tokio::sync::mpsc::channel(64);
    let (initialized, initialization) = tokio::sync::oneshot::channel();
    let simulation_stop = stop.clone();
    let account_ids = accounts.keys().copied().collect::<Vec<_>>();
    let thread = std::thread::Builder::new()
        .name("simulation".into())
        .spawn(move || {
            let simulation = crate::scenario(&account_ids, debug_account, ship)?;
            let assets = crate::assets(&simulation);
            if initialized.send(assets).is_err() {
                return Ok(());
            }
            crate::run(simulation, receive, simulation_stop)
        })?;
    let assets = match initialization.await {
        Ok(assets) => assets,
        Err(_) => {
            return thread
                .join()
                .map_err(|_| anyhow::anyhow!("simulation initialization panicked"))?
                .and_then(|()| anyhow::bail!("simulation initialization stopped"));
        }
    };
    let (shutdown, mut closed) = tokio::sync::mpsc::channel(1);
    if options.shutdown_on_stdin_close {
        std::thread::Builder::new()
            .name("parent-lifetime".into())
            .spawn(move || {
                let mut buffer = [0u8; 256];
                while std::io::stdin()
                    .read(&mut buffer)
                    .is_ok_and(|count| count != 0)
                {}
                let _ = shutdown.blocking_send(());
            })?;
    }
    let ready_result = if let Some(path) = &options.ready_file {
        std::fs::write(path, address.to_string()).context("write readiness address")
    } else {
        Ok(())
    };
    let result = match ready_result {
        Ok(()) => {
            eprintln!("Listening on {address}");
            tokio::select! {
                result = crate::listen(listener, key, accounts, send, assets) => result,
                result = tokio::signal::ctrl_c() => result.map_err(Into::into),
                _ = closed.recv(), if options.shutdown_on_stdin_close => Ok(()),
                _ = async {
                    while !thread.is_finished() {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                } => Ok(()),
            }
        }
        Err(error) => Err(error),
    };
    stop.store(true, Ordering::Release);
    let simulation_result = thread
        .join()
        .map_err(|_| anyhow::anyhow!("simulation thread panicked"))?;
    if let Some(path) = options.ready_file {
        let _ = std::fs::remove_file(path);
    }
    result?;
    simulation_result
}
