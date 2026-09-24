use anyhow::{Context, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use osg_model::AccountId;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: String,
    server_secret: String,
    accounts: Vec<Account>,
    debug_account: Option<String>,
    ship: Option<PathBuf>,
    #[serde(default)]
    persistence: crate::persistence::Config,
    #[serde(default = "official_rate")]
    official_uec_per_lat: String,
}

fn official_rate() -> String {
    "3.20".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    id: String,
    public_key: String,
    #[serde(default)]
    initial_uec: String,
    #[serde(default)]
    initial_lat: String,
    #[serde(default)]
    lat_licence: bool,
    #[serde(default)]
    polity_officer: Vec<String>,
    #[serde(default)]
    bloc_officer: Vec<String>,
}

#[derive(Default)]
pub struct Options {
    pub ready_file: Option<PathBuf>,
    pub shutdown_on_stdin_close: bool,
}

pub async fn run(path: &Path, options: Options) -> Result<()> {
    let config: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
    let key = SigningKey::from_bytes(&crate::key_bytes(&config.server_secret)?);
    let funding = config
        .accounts
        .iter()
        .map(|account| {
            let amount = |text: &str| {
                if text.is_empty() {
                    Ok(0)
                } else {
                    osg_model::economy::parse_amount(text)
                        .context("invalid initial currency amount")
                }
            };
            Ok((
                account.id.parse::<AccountId>()?,
                amount(&account.initial_uec)?,
                amount(&account.initial_lat)?,
                account.lat_licence,
                account.polity_officer.clone(),
                account.bloc_officer.clone(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let official_rate = osg_model::economy::parse_amount(&config.official_uec_per_lat)
        .filter(|rate| *rate > 0)
        .context("invalid official conversion rate")?;
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
    let persistence = config.persistence;
    let config_directory = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
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
            let prepared = crate::persistence::prepare(&persistence, &config_directory)?;
            let restoring = prepared.as_ref().is_some_and(|saved| saved.has_snapshot());
            let bootstrap_ship = if restoring { None } else { ship };
            let mut simulation = crate::scenario(&account_ids, debug_account, bootstrap_ship)?;
            if !restoring {
                let world = simulation.world_mut();
                let now = osg_model::calendar::now_unix_ms();
                world
                    .resource_mut::<crate::sim::economy::Economy>()
                    .official_uec_per_lat = official_rate;
                for (account, uec, lat, licensed, polities, blocs) in &funding {
                    let owner = osg_model::ownership::Principal::Player(*account);
                    let mut economy = world.resource_mut::<crate::sim::economy::Economy>();
                    if *licensed {
                        economy.licences.insert(owner);
                    }
                    if *uec > 0 {
                        economy.issue(owner, osg_model::economy::Currency::Uec, *uec, now)?;
                    }
                    if *lat > 0 {
                        economy.issue(owner, osg_model::economy::Currency::Lat, *lat, now)?;
                    }
                    let mut directory = world.resource_mut::<crate::sim::ownership::Directory>();
                    for name in polities {
                        directory
                            .0
                            .sovereignties
                            .values_mut()
                            .find(|polity| polity.name == *name)
                            .context("configured officer polity unavailable")?
                            .officers
                            .insert(*account);
                    }
                    for name in blocs {
                        directory
                            .0
                            .diplomacy
                            .blocs
                            .values_mut()
                            .find(|bloc| bloc.name == *name)
                            .context("configured officer bloc unavailable")?
                            .officers
                            .insert(*account);
                    }
                }
            }
            if let Some(prepared) = prepared {
                prepared.initialize(simulation.world_mut())?;
            }
            let assets = crate::assets(&simulation);
            let checkpoint_trigger = crate::persistence::trigger(simulation.world());
            if initialized.send((assets, checkpoint_trigger)).is_err() {
                return Ok(());
            }
            crate::run(simulation, receive, simulation_stop)
        })?;
    let (assets, checkpoint_trigger) = match initialization.await {
        Ok(assets) => assets,
        Err(_) => {
            return thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                .and_then(|()| anyhow::bail!("simulation initialization stopped"));
        }
    };
    #[cfg(unix)]
    let checkpoint_signal = if let Some(trigger) = checkpoint_trigger {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
            Ok(mut signals) => Some(tokio::spawn(async move {
                while signals.recv().await.is_some() {
                    trigger.store(true, Ordering::Release);
                }
            })),
            Err(error) => {
                eprintln!("Manual checkpoint signal unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    #[cfg(not(unix))]
    let _ = checkpoint_trigger;
    let termination = async {
        #[cfg(unix)]
        terminate.recv().await;
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
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
                _ = termination => Ok(()),
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
    #[cfg(unix)]
    if let Some(task) = checkpoint_signal {
        task.abort();
    }
    stop.store(true, Ordering::Release);
    let simulation_result = thread
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    if let Some(path) = options.ready_file {
        let _ = std::fs::remove_file(path);
    }
    result?;
    simulation_result
}
