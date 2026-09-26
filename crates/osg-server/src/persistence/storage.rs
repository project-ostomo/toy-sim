use anyhow::{Context, Result, ensure};
use osg_model::GAME_VERSION;
use rusqlite::{Connection, OptionalExtension, params};
use std::{fs::File, path::Path, time::Duration};

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub tick: u64,
    pub saved_at_unix_ms: u64,
    pub bytes: Vec<u8>,
}

pub struct Database {
    connection: Connection,
    _lock: Option<File>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let lock = if path == Path::new(":memory:") {
            None
        } else {
            let mut options = File::options();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(path).context("open world database lock")?;
            file.try_lock()
                .with_context(|| format!("world database {} is already in use", path.display()))?;
            Some(file)
        };
        let connection = Connection::open(path).context("open world snapshot database")?;
        connection.busy_timeout(Duration::from_secs(30))?;
        let application_id: u32 =
            connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if application_id == 0 {
            let tables: u64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            ensure!(tables == 0, "database belongs to another application");
        } else {
            ensure!(
                application_id == 0x5453594d,
                "database belongs to another application"
            );
            let version: u16 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            ensure!(
                version == GAME_VERSION,
                "world database game version {version} does not match {GAME_VERSION}; start a new world"
            );
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS snapshots (
                 generation INTEGER PRIMARY KEY AUTOINCREMENT,
                 simulation_tick INTEGER NOT NULL,
                 saved_at_unix_ms INTEGER NOT NULL,
                 payload BLOB NOT NULL
             );
             PRAGMA application_id=1414748493;",
        )?;
        connection.pragma_update(None, "user_version", GAME_VERSION)?;
        connection.execute_batch("COMMIT;")?;
        Ok(Self {
            connection,
            _lock: lock,
        })
    }

    pub fn save(&mut self, snapshot: &Snapshot) -> Result<u64> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO snapshots (simulation_tick, saved_at_unix_ms, payload)
             VALUES (?1, ?2, ?3)",
            params![snapshot.tick, snapshot.saved_at_unix_ms, snapshot.bytes],
        )?;
        let generation = transaction.last_insert_rowid();
        transaction.execute(
            "DELETE FROM snapshots WHERE generation NOT IN
             (SELECT generation FROM snapshots ORDER BY generation DESC LIMIT 3)",
            [],
        )?;
        transaction.commit()?;
        Ok(generation as u64)
    }

    pub fn load(&self) -> Result<Option<Snapshot>> {
        Ok(self.connection.query_row(
            "SELECT simulation_tick, saved_at_unix_ms, payload FROM snapshots ORDER BY generation DESC LIMIT 1",
            [],
            |row| Ok(Snapshot {
                tick: row.get(0)?,
                saved_at_unix_ms: row.get(1)?,
                bytes: row.get(2)?,
            }),
        ).optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(tick: u64) -> Snapshot {
        Snapshot {
            tick,
            saved_at_unix_ms: 1234 + tick,
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn failed_transaction_cannot_publish_half_a_world() {
        let mut db = Database::open(Path::new(":memory:")).unwrap();
        db.save(&snapshot(1)).unwrap();
        db.connection
            .execute_batch(
                "CREATE TRIGGER fail_world BEFORE INSERT ON snapshots BEGIN
                     SELECT RAISE(ABORT, 'simulated disk write failure');
                 END;",
            )
            .unwrap();
        assert!(db.save(&snapshot(2)).is_err());
        assert_eq!(db.load().unwrap(), Some(snapshot(1)));
        let count: u64 = db
            .connection
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn mismatched_game_version_is_rejected() {
        let directory = std::env::temp_dir().join(format!("osg-version-{}", osg_model::Id::new()));
        let path = directory.join("world.sqlite");
        let database = Database::open(&path).unwrap();
        database
            .connection
            .pragma_update(None, "user_version", GAME_VERSION + 1)
            .unwrap();
        drop(database);
        let error = Database::open(&path).err().unwrap();
        assert!(error.to_string().contains("game version"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn retention_keeps_three_complete_generations() {
        let mut database = Database::open(Path::new(":memory:")).unwrap();
        for tick in 1..=6 {
            database.save(&snapshot(tick)).unwrap();
        }

        let generations = database
            .connection
            .prepare("SELECT generation FROM snapshots ORDER BY generation")
            .unwrap()
            .query_map([], |row| row.get::<_, u64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(generations, [4, 5, 6]);
        assert_eq!(database.load().unwrap(), Some(snapshot(6)));
    }

    #[test]
    fn file_lock_prevents_a_second_server_and_releases_on_close() {
        let directory = std::env::temp_dir().join(format!("osg-storage-{}", osg_model::Id::new()));
        let path = directory.join("world.sqlite");
        let mut first = Database::open(&path).unwrap();
        first.save(&snapshot(1)).unwrap();
        let second = Database::open(&path);
        assert!(second.is_err());
        assert!(second.err().unwrap().to_string().contains("already in use"));

        drop(first);
        let second = Database::open(&path).unwrap();
        assert_eq!(second.load().unwrap(), Some(snapshot(1)));
        drop(second);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
