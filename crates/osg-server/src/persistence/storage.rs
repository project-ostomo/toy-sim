use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, path::Path, time::Duration};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub tick: u64,
    pub saved_at_unix_ms: u64,
    pub sections: BTreeMap<String, SectionData>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SectionData {
    pub version: u32,
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
        let schema_version: u32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            schema_version <= 1,
            "unsupported world database schema {schema_version}"
        );
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
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS snapshots (
                 generation INTEGER PRIMARY KEY AUTOINCREMENT,
                 format_version INTEGER NOT NULL,
                 simulation_tick INTEGER NOT NULL,
                 saved_at_unix_ms INTEGER NOT NULL,
                 section_count INTEGER NOT NULL,
                 checksum BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS snapshot_sections (
                 generation INTEGER NOT NULL REFERENCES snapshots(generation) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 payload BLOB NOT NULL,
                 checksum BLOB NOT NULL,
                 PRIMARY KEY (generation, name)
             );
             PRAGMA user_version=1;
             PRAGMA application_id=1414748493;
             COMMIT;",
        )?;
        Ok(Self {
            connection,
            _lock: lock,
        })
    }

    pub fn save(&mut self, snapshot: &Snapshot) -> Result<u64> {
        let digest = checksum(snapshot);
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO snapshots (format_version, simulation_tick, saved_at_unix_ms, section_count, checksum)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                FORMAT_VERSION,
                snapshot.tick,
                snapshot.saved_at_unix_ms,
                snapshot.sections.len() as u64,
                digest.as_slice()
            ],
        )?;
        let generation = transaction.last_insert_rowid();
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO snapshot_sections (generation, name, version, payload, checksum)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (name, section) in &snapshot.sections {
                insert.execute(params![
                    generation,
                    name,
                    section.version,
                    section.bytes,
                    blake3::hash(&section.bytes).as_bytes().as_slice()
                ])?;
            }
        }
        transaction.execute(
            "DELETE FROM snapshots WHERE generation NOT IN
             (SELECT generation FROM snapshots ORDER BY generation DESC LIMIT 3)",
            [],
        )?;
        transaction.commit()?;
        Ok(generation as u64)
    }

    pub fn load(&self) -> Result<Option<Snapshot>> {
        let generation = self
            .connection
            .query_row(
                "SELECT generation FROM snapshots ORDER BY generation DESC LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        generation
            .map(|generation| {
                self.load_generation(generation).with_context(|| {
                    format!(
                        "cannot restore latest checkpoint generation {generation}; \
                         older generations retained for manual recovery"
                    )
                })
            })
            .transpose()
    }

    fn load_generation(&self, generation: i64) -> Result<Snapshot> {
        let (version, tick, saved_at_unix_ms, count, expected): (u32, u64, u64, usize, Vec<u8>) =
            self.connection.query_row(
                "SELECT format_version, simulation_tick, saved_at_unix_ms, section_count, checksum
                 FROM snapshots WHERE generation=?1",
                [generation],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        ensure!(
            version == FORMAT_VERSION,
            "unsupported snapshot format {version}"
        );
        let mut statement = self.connection.prepare(
            "SELECT name, version, payload, checksum FROM snapshot_sections
             WHERE generation=?1 ORDER BY name",
        )?;
        let rows = statement.query_map([generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut sections = BTreeMap::new();
        for row in rows {
            let (name, version, bytes, expected) = row?;
            ensure!(
                blake3::hash(&bytes).as_bytes().as_slice() == expected,
                "snapshot section {name} checksum mismatch"
            );
            sections.insert(name, SectionData { version, bytes });
        }
        ensure!(sections.len() == count, "incomplete snapshot generation");
        let snapshot = Snapshot {
            tick,
            saved_at_unix_ms,
            sections,
        };
        ensure!(
            checksum(&snapshot).as_slice() == expected,
            "snapshot manifest checksum mismatch"
        );
        Ok(snapshot)
    }
}

fn checksum(snapshot: &Snapshot) -> [u8; 32] {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame world checkpoint v1");
    hash.update(&snapshot.tick.to_le_bytes());
    hash.update(&snapshot.saved_at_unix_ms.to_le_bytes());
    for (name, section) in &snapshot.sections {
        hash.update(&(name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update(&section.version.to_le_bytes());
        hash.update(&(section.bytes.len() as u64).to_le_bytes());
        hash.update(blake3::hash(&section.bytes).as_bytes());
    }
    *hash.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(tick: u64) -> Snapshot {
        Snapshot {
            tick,
            saved_at_unix_ms: 1234 + tick,
            sections: BTreeMap::from([
                (
                    "world".into(),
                    SectionData {
                        version: 1,
                        bytes: vec![1, 2, 3],
                    },
                ),
                (
                    "economy".into(),
                    SectionData {
                        version: 2,
                        bytes: vec![4, 5, 6],
                    },
                ),
            ]),
        }
    }

    #[test]
    fn corrupt_latest_generation_fails_instead_of_silently_rolling_back() {
        let mut db = Database::open(Path::new(":memory:")).unwrap();
        db.save(&snapshot(1)).unwrap();
        db.save(&snapshot(2)).unwrap();
        assert_eq!(db.load().unwrap(), Some(snapshot(2)));
        db.connection
            .execute(
                "UPDATE snapshot_sections SET payload=x'00' WHERE generation=2 AND name='world'",
                [],
            )
            .unwrap();
        assert!(db.load().is_err());
        assert_eq!(db.load_generation(1).unwrap(), snapshot(1));
    }

    #[test]
    fn failed_transaction_cannot_publish_half_a_world() {
        let mut db = Database::open(Path::new(":memory:")).unwrap();
        db.save(&snapshot(1)).unwrap();
        db.connection
            .execute_batch(
                "CREATE TRIGGER fail_world BEFORE INSERT ON snapshot_sections
                 WHEN NEW.name='world' BEGIN
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
    fn unsupported_latest_format_is_rejected() {
        let mut database = Database::open(Path::new(":memory:")).unwrap();
        database.save(&snapshot(1)).unwrap();
        database.save(&snapshot(2)).unwrap();
        database
            .connection
            .execute(
                "UPDATE snapshots SET format_version=99 WHERE generation=2",
                [],
            )
            .unwrap();

        let error = database.load().unwrap_err();
        assert!(format!("{error:#}").contains("unsupported snapshot format 99"));
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
        let orphan_sections: u64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM snapshot_sections WHERE generation NOT IN (SELECT generation FROM snapshots)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_sections, 0);
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

    #[test]
    fn metadata_changes_are_detected() {
        let mut database = Database::open(Path::new(":memory:")).unwrap();
        database.save(&snapshot(1)).unwrap();
        database
            .connection
            .execute("UPDATE snapshots SET simulation_tick=999", [])
            .unwrap();

        let error = database.load().unwrap_err();
        assert!(format!("{error:#}").contains("manifest checksum mismatch"));
    }
}
