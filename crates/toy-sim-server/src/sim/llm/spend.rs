use super::{Key, LlmStatus};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, time::Duration};
use toy_sim_model::Id;

pub const MAX_MICRODOLLARS: u64 = 100_000_000;
const FORMAT: i64 = 1;
const MAX_DURABLE_REQUESTS: u64 = 100_000;

pub struct SpendLedger {
    connection: Connection,
    session: Id,
}

pub enum Admission {
    New,
    Existing(LlmStatus),
    Conflicting {
        fingerprint: [u8; 32],
        status: LlmStatus,
    },
    BudgetExhausted,
    RequestLimit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spending {
    pub settled_microdollars: u64,
    pub reserved_microdollars: u64,
}

impl SpendLedger {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS llm_metadata (
                 singleton INTEGER PRIMARY KEY CHECK (singleton=1),
                 format INTEGER NOT NULL,
                 cap INTEGER NOT NULL CHECK (cap=100000000)
             );
             INSERT OR IGNORE INTO llm_metadata VALUES (1,1,100000000);
             CREATE TABLE IF NOT EXISTS llm_requests (
                 key BLOB PRIMARY KEY,
                 fingerprint BLOB NOT NULL CHECK (length(fingerprint)=32),
                 session BLOB NOT NULL,
                 reservation INTEGER NOT NULL CHECK (reservation>0),
                 charged INTEGER CHECK (charged>=0 AND charged<=reservation),
                 result BLOB NOT NULL,
                 result_hash BLOB,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        let (format, cap): (i64, u64) = connection.query_row(
            "SELECT format,cap FROM llm_metadata WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            format == FORMAT && cap == MAX_MICRODOLLARS,
            "unsupported LLM spending ledger"
        );
        let ledger = Self {
            connection,
            session: Id::new(),
        };
        let spending = ledger.spending()?;
        ensure!(
            spending
                .settled_microdollars
                .checked_add(spending.reserved_microdollars)
                .is_some_and(|total| total <= MAX_MICRODOLLARS),
            "LLM spending ledger exceeds its authorized cap"
        );
        Ok(ledger)
    }

    pub fn spending(&self) -> Result<Spending> {
        self.connection
            .query_row(
                "SELECT COALESCE(SUM(charged),0),
                    COALESCE(SUM(CASE WHEN charged IS NULL THEN reservation ELSE 0 END),0)
             FROM llm_requests",
                [],
                |row| {
                    Ok(Spending {
                        settled_microdollars: row.get(0)?,
                        reserved_microdollars: row.get(1)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<(Key, [u8; 32], LlmStatus)>> {
        let mut query = self.connection.prepare(
            "SELECT key,fingerprint,result,charged FROM llm_requests
             ORDER BY created_at DESC,rowid DESC LIMIT ?1",
        )?;
        let rows = query.query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<u64>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (key, fingerprint, result, charged) = row?;
            let fingerprint = fingerprint
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid LLM fingerprint"))?;
            let mut status = postcard::from_bytes(&result)?;
            if charged.is_none() && status == LlmStatus::Pending {
                status = LlmStatus::Indeterminate;
            }
            Ok((postcard::from_bytes(&key)?, fingerprint, status))
        })
        .collect()
    }

    pub fn reserve(&mut self, key: Key, fingerprint: [u8; 32], amount: u64) -> Result<Admission> {
        ensure!(
            amount > 0 && amount <= MAX_MICRODOLLARS,
            "invalid LLM spending reservation"
        );
        let encoded = postcard::to_stdvec(&key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT fingerprint,result,charged,session FROM llm_requests WHERE key=?1",
                [&encoded],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((previous, result, charged, session)) = existing {
            let mut result = postcard::from_bytes(&result)?;
            if charged.is_none() && session != self.session.0 && result == LlmStatus::Pending {
                result = LlmStatus::Indeterminate;
            }
            if previous != fingerprint {
                return Ok(Admission::Conflicting {
                    fingerprint: previous
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("invalid LLM fingerprint"))?,
                    status: result,
                });
            }
            return Ok(Admission::Existing(result));
        }
        let (committed, count): (u64, u64) = transaction.query_row(
            "SELECT COALESCE(SUM(COALESCE(charged,reservation)),0),COUNT(*) FROM llm_requests",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if amount > MAX_MICRODOLLARS.saturating_sub(committed) {
            return Ok(Admission::BudgetExhausted);
        }
        if count >= MAX_DURABLE_REQUESTS {
            return Ok(Admission::RequestLimit);
        }
        transaction.execute(
            "INSERT INTO llm_requests(key,fingerprint,session,reservation,result) VALUES (?1,?2,?3,?4,?5)",
            params![encoded, fingerprint.as_slice(), self.session.0.as_slice(), amount,
                postcard::to_stdvec(&LlmStatus::Pending)?],
        )?;
        transaction.commit()?;
        Ok(Admission::New)
    }

    pub fn settle(&mut self, key: Key, charged: Option<u64>, result: &LlmStatus) -> Result<()> {
        let encoded = postcard::to_stdvec(&key)?;
        let result_bytes = postcard::to_stdvec(result)?;
        let result_hash = blake3::hash(&result_bytes);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (reserved, prior, previous_hash): (u64, Option<u64>, Option<Vec<u8>>) = transaction
            .query_row(
                "SELECT reservation,charged,result_hash FROM llm_requests WHERE key=?1",
                [&encoded],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("LLM settlement has no reservation")?;
        ensure!(
            charged.is_none_or(|amount| amount <= reserved),
            "provider charge exceeded LLM reservation"
        );
        if prior.is_some() {
            ensure!(
                prior == charged
                    && previous_hash.as_deref() == Some(result_hash.as_bytes().as_slice()),
                "conflicting duplicate LLM settlement"
            );
            return Ok(());
        }
        transaction.execute(
            "UPDATE llm_requests SET charged=?2,result=?3,result_hash=?4 WHERE key=?1",
            params![
                encoded,
                charged,
                result_bytes,
                result_hash.as_bytes().as_slice()
            ],
        )?;
        transaction.execute(
            "UPDATE llm_requests SET result=?1 WHERE charged IS NOT NULL
             AND rowid NOT IN (SELECT rowid FROM llm_requests ORDER BY created_at DESC,rowid DESC LIMIT 1024)
             AND length(result)>?2",
            params![postcard::to_stdvec(&super::failed("Result expired from retained LLM history"))?,256],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
