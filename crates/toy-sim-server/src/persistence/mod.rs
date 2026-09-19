mod storage;
pub(crate) mod world;

use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use storage::{Database, SectionData, Snapshot};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub path: PathBuf,
    pub interval_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("world.sqlite"),
            interval_seconds: 900,
        }
    }
}

fn capture(world: &World) -> Result<Snapshot> {
    let tick = world
        .resource::<crate::sim::simulation::SimulationCounters>()
        .ticks;
    let saved_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let records = BTreeMap::from([(
        "world".into(),
        SectionData {
            version: 3,
            bytes: self::world::capture(world).context("capture world checkpoint")?,
        },
    )]);
    Ok(Snapshot {
        tick,
        saved_at_unix_ms,
        sections: records,
    })
}

fn restore(world: &mut World, snapshot: Snapshot) -> Result<()> {
    ensure!(
        snapshot.sections.len() == 1,
        "unsupported snapshot section set"
    );
    let saved = snapshot
        .sections
        .get("world")
        .context("snapshot missing world section")?;
    ensure!(
        saved.version == 3,
        "unsupported world snapshot section version {}; explicitly start a new database for this universe",
        saved.version
    );
    self::world::restore(world, &saved.bytes).context("restore world checkpoint")?;
    info!(
        tick = snapshot.tick,
        saved_at_unix_ms = snapshot.saved_at_unix_ms,
        "World checkpoint restored"
    );
    Ok(())
}

struct Written {
    generation: u64,
    tick: u64,
    bytes: usize,
    write_ms: f64,
}

#[derive(Resource)]
pub struct Checkpoints {
    send: Mutex<Option<mpsc::SyncSender<Snapshot>>>,
    receive: Mutex<mpsc::Receiver<Result<Written>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    interval: Duration,
    next_capture: Instant,
    pending: bool,
    requested: Arc<AtomicBool>,
}

pub struct Prepared {
    database: Database,
    snapshot: Option<Snapshot>,
    path: PathBuf,
    interval: Duration,
}

pub fn prepare(config: &Config, config_directory: &Path) -> Result<Option<Prepared>> {
    if !config.enabled {
        return Ok(None);
    }
    ensure!(
        (1..=31_536_000).contains(&config.interval_seconds),
        "snapshot interval_seconds must be between one second and one year"
    );
    ensure!(
        !config.path.as_os_str().is_empty(),
        "snapshot path cannot be empty"
    );
    let path = if config.path.is_absolute() {
        config.path.clone()
    } else {
        config_directory.join(&config.path)
    };
    let database = Database::open(&path)?;
    let snapshot = database.load()?;
    Ok(Some(Prepared {
        database,
        snapshot,
        path,
        interval: Duration::from_secs(config.interval_seconds),
    }))
}

impl Prepared {
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    pub fn initialize(self, world: &mut World) -> Result<()> {
        ensure!(
            !world.contains_resource::<Checkpoints>(),
            "world persistence already initialized"
        );
        let Self {
            mut database,
            snapshot,
            path,
            interval,
        } = self;
        if let Some(snapshot) = snapshot {
            restore(world, snapshot)?;
        } else {
            let started = Instant::now();
            let snapshot = capture(world)?;
            let generation = database.save(&snapshot)?;
            info!(
                generation,
                tick = snapshot.tick,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                "Initial world checkpoint committed"
            );
        }
        let (send, jobs) = mpsc::sync_channel::<Snapshot>(1);
        let (completed, receive) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("world-checkpoint".into())
            .spawn(move || {
                while let Ok(snapshot) = jobs.recv() {
                    let started = Instant::now();
                    let bytes = snapshot
                        .sections
                        .values()
                        .map(|section| section.bytes.len())
                        .sum();
                    let result = database.save(&snapshot).map(|generation| Written {
                        generation,
                        tick: snapshot.tick,
                        bytes,
                        write_ms: started.elapsed().as_secs_f64() * 1000.0,
                    });
                    let failed = result.is_err();
                    if completed.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        world.insert_resource(Checkpoints {
            send: Mutex::new(Some(send)),
            receive: Mutex::new(receive),
            worker: Mutex::new(Some(worker)),
            interval,
            next_capture: Instant::now() + interval,
            pending: false,
            requested: Arc::new(AtomicBool::new(false)),
        });
        info!(path=%path.display(), interval_seconds=interval.as_secs(), "World persistence enabled");
        Ok(())
    }
}

pub fn trigger(world: &World) -> Option<Arc<AtomicBool>> {
    world
        .get_resource::<Checkpoints>()
        .map(|checkpoints| checkpoints.requested.clone())
}

pub fn request(world: &World) {
    if let Some(trigger) = trigger(world) {
        trigger.store(true, Ordering::Release);
    }
}

pub fn poll(world: &mut World) -> Result<()> {
    let Some(mut checkpoints) = world.remove_resource::<Checkpoints>() else {
        return Ok(());
    };
    let result = (|| {
        loop {
            match checkpoints.receive.get_mut().unwrap().try_recv() {
                Ok(result) => {
                    report(result.context("write world checkpoint")?);
                    checkpoints.pending = false;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("checkpoint worker stopped unexpectedly");
                }
            }
        }
        if !checkpoints.pending
            && (checkpoints.requested.load(Ordering::Acquire)
                || Instant::now() >= checkpoints.next_capture)
        {
            checkpoints.requested.swap(false, Ordering::AcqRel);
            let started = Instant::now();
            let snapshot = capture(world)?;
            let capture_ms = started.elapsed().as_secs_f64() * 1000.0;
            let tick = snapshot.tick;
            checkpoints
                .send
                .get_mut()
                .unwrap()
                .as_ref()
                .context("checkpoint worker stopped")?
                .try_send(snapshot)
                .context("queue world checkpoint")?;
            checkpoints.pending = true;
            checkpoints.next_capture = Instant::now() + checkpoints.interval;
            info!(tick, capture_ms, "World checkpoint captured");
        }
        Ok(())
    })();
    world.insert_resource(checkpoints);
    result
}

pub fn shutdown(world: &mut World) -> Result<()> {
    let Some(mut checkpoints) = world.remove_resource::<Checkpoints>() else {
        return Ok(());
    };
    let started = Instant::now();
    let snapshot = capture(world)?;
    info!(
        tick = snapshot.tick,
        capture_ms = started.elapsed().as_secs_f64() * 1000.0,
        "Final world checkpoint captured"
    );
    let send = checkpoints
        .send
        .get_mut()
        .unwrap()
        .take()
        .context("checkpoint worker stopped")?;
    let queued = send.send(snapshot);
    drop(send);
    let worker = checkpoints
        .worker
        .get_mut()
        .unwrap()
        .take()
        .context("checkpoint worker unavailable")?;
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("checkpoint worker panicked"))?;
    for result in checkpoints.receive.get_mut().unwrap().try_iter() {
        report(result.context("write final world checkpoint")?);
    }
    queued.context("queue final world checkpoint")?;
    Ok(())
}

fn report(written: Written) {
    info!(
        generation = written.generation,
        tick = written.tick,
        bytes = written.bytes,
        write_ms = written.write_ms,
        "World checkpoint committed"
    );
}

impl Drop for Checkpoints {
    fn drop(&mut self) {
        self.send.get_mut().unwrap().take();
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            if worker.join().is_err() {
                error!("Checkpoint worker panicked during cleanup");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_write_failure_is_returned_to_server_loop() {
        let mut world = World::new();
        let (completed, receive) = mpsc::channel();
        completed.send(Err(anyhow::anyhow!("disk full"))).unwrap();
        world.insert_resource(Checkpoints {
            send: Mutex::new(None),
            receive: Mutex::new(receive),
            worker: Mutex::new(None),
            interval: Duration::from_secs(900),
            next_capture: Instant::now() + Duration::from_secs(900),
            pending: true,
            requested: Arc::new(AtomicBool::new(false)),
        });

        let error = poll(&mut world).unwrap_err();
        assert!(format!("{error:#}").contains("disk full"));
    }

    #[test]
    fn disabled_persistence_creates_no_database_or_worker() {
        let mut world = World::new();
        let directory = std::env::temp_dir().join(format!(
            "toy-sim-disabled-checkpoint-{}",
            toy_sim_model::Id::new()
        ));
        let config = Config {
            enabled: false,
            ..Default::default()
        };

        assert!(prepare(&config, &directory).unwrap().is_none());
        poll(&mut world).unwrap();
        shutdown(&mut world).unwrap();
        assert!(!world.contains_resource::<Checkpoints>());
        assert!(!directory.exists());
    }

    #[test]
    fn earlier_world_sections_are_rejected_before_restore() {
        let mut world = World::new();
        let epoch = toy_sim_model::Id::new();
        world.insert_resource(crate::sim::identity::WorldEpoch(epoch));

        for version in [1, 2] {
            let error = restore(
                &mut world,
                Snapshot {
                    tick: 0,
                    saved_at_unix_ms: 0,
                    sections: BTreeMap::from([(
                        "world".into(),
                        SectionData {
                            version,
                            bytes: Vec::new(),
                        },
                    )]),
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("unsupported world snapshot"));
            assert_eq!(
                world.resource::<crate::sim::identity::WorldEpoch>().0,
                epoch
            );
        }
    }

    #[test]
    fn preparation_distinguishes_empty_database_from_saved_world() {
        let directory = std::env::temp_dir().join(format!(
            "toy-sim-prepared-checkpoint-{}",
            toy_sim_model::Id::new()
        ));
        let config = Config::default();
        let prepared = prepare(&config, &directory).unwrap().unwrap();
        assert!(!prepared.has_snapshot());
        assert!(directory.join(&config.path).exists());
        drop(prepared);

        let mut database = Database::open(&directory.join(&config.path)).unwrap();
        database
            .save(&Snapshot {
                tick: 1,
                saved_at_unix_ms: 1,
                sections: BTreeMap::new(),
            })
            .unwrap();
        drop(database);
        let prepared = prepare(&config, &directory).unwrap().unwrap();
        assert!(prepared.has_snapshot());
        drop(prepared);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
