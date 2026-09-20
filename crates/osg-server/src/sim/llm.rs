mod provider;
mod spend;

use super::gas::GasLedger;
use anyhow::{Context, Result, ensure};
use bevy::prelude::Resource;
use osg_model::{
    Id,
    llm::{LlmRequest, LlmStatus, LlmSubmission, MAX_RESULT_BYTES},
    ownership::Principal,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub use spend::Spending;

const MAX_PENDING: usize = 64;
const MAX_PER_OWNER: usize = 4;
const MAX_PER_COMPUTER: usize = 2;
const MAX_CONCURRENT: usize = 8;
const MAX_RESULTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LlmCaller {
    pub world: Id,
    pub owner: Principal,
    pub computer: Id,
    pub program: [u8; 32],
    pub display: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Key {
    caller: LlmCaller,
    request: u64,
}

struct Entry {
    fingerprint: [u8; 32],
    status: LlmStatus,
    pending: bool,
    cancel: Arc<AtomicBool>,
    sequence: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    sequence: u64,
}

struct Job {
    key: Key,
    fingerprint: [u8; 32],
    request: LlmRequest,
    cancel: Arc<AtomicBool>,
}

struct Completion {
    key: Key,
    fingerprint: Option<[u8; 32]>,
    authoritative: bool,
    status: LlmStatus,
}

#[derive(Resource, Clone)]
pub struct LlmService {
    state: Arc<Mutex<State>>,
    queue: Option<tokio::sync::mpsc::Sender<Job>>,
    completions: Arc<Mutex<mpsc::Receiver<Completion>>>,
    healthy: Arc<AtomicBool>,
    spending: Arc<Mutex<Spending>>,
}

impl Default for LlmService {
    fn default() -> Self {
        let (_, receiver) = mpsc::sync_channel(1);
        Self {
            state: Default::default(),
            queue: None,
            completions: Arc::new(Mutex::new(receiver)),
            healthy: Arc::new(AtomicBool::new(false)),
            spending: Default::default(),
        }
    }
}

impl LlmService {
    pub fn from_env(enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self::default());
        }
        ensure!(!cfg!(test), "LLM unit tests must use a fake provider");

        let key = std::env::var("OPENROUTER_API_KEY")
            .context("LLM service requires OPENROUTER_API_KEY")?;
        ensure!(
            !key.trim().is_empty(),
            "LLM service requires OPENROUTER_API_KEY"
        );
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .context("LLM service needs a persistent user data directory")?;
        Self::start(
            &data.join("openspacegame/llm-spend.sqlite"),
            Arc::new(provider::OpenRouter::new(key)?),
        )
    }

    fn start(path: &Path, provider: Arc<dyn provider::Provider>) -> Result<Self> {
        let ledger = spend::SpendLedger::open(path)?;
        let mut state = State::default();
        for (key, fingerprint, status) in ledger.recent(MAX_RESULTS)?.into_iter().rev() {
            state.entries.insert(
                key,
                Entry {
                    fingerprint,
                    status,
                    pending: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                    sequence: state.sequence,
                },
            );
            state.sequence += 1;
        }
        let spending = Arc::new(Mutex::new(ledger.spending()?));
        let healthy = Arc::new(AtomicBool::new(true));
        let (queue, receiver) = tokio::sync::mpsc::channel(MAX_PENDING);
        let (completion, completions) = mpsc::sync_channel(MAX_PENDING);
        let worker_health = healthy.clone();
        let worker_spending = spending.clone();
        std::thread::Builder::new()
            .name("llm-service".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| {
                        runtime.block_on(worker(
                            receiver,
                            completion,
                            ledger,
                            provider,
                            worker_spending,
                        ))
                    });
                worker_health.store(false, Ordering::Release);
                if let Err(error) = result {
                    bevy::log::error!(
                        %error,
                        "LLM worker stopped; outstanding dollar reservations remain held"
                    );
                }
            })?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            queue: Some(queue),
            completions: Arc::new(Mutex::new(completions)),
            healthy,
            spending,
        })
    }

    pub fn enabled(&self) -> bool {
        self.queue.is_some() && self.healthy.load(Ordering::Acquire)
    }

    pub fn spending(&self) -> Spending {
        *self.spending.lock().unwrap()
    }

    pub fn drain(&self) {
        let receiver = self.completions.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        for completion in receiver.try_iter().take(MAX_PENDING) {
            if let Some(entry) = state.entries.get_mut(&completion.key) {
                if let Some(fingerprint) = completion.fingerprint {
                    entry.fingerprint = fingerprint;
                }
                if completion.authoritative || !entry.cancel.load(Ordering::Acquire) {
                    entry.status = completion.status;
                }
                if completion.authoritative {
                    entry.cancel.store(false, Ordering::Release);
                }
                entry.pending = false;
            }
        }
        if !self.healthy.load(Ordering::Acquire) {
            for entry in state.entries.values_mut().filter(|entry| entry.pending) {
                entry.pending = false;
                if !entry.cancel.load(Ordering::Acquire) {
                    entry.status = LlmStatus::Indeterminate;
                }
            }
        }
    }

    pub fn submit(&self, caller: LlmCaller, request: LlmRequest, gas: &GasLedger) -> LlmSubmission {
        let Some(gas_quote) = request.gas_quote() else {
            return LlmSubmission::InvalidRequest;
        };
        self.drain();
        let key = Key {
            caller,
            request: request.id,
        };
        let fingerprint = *blake3::hash(&postcard::to_stdvec(&request).unwrap()).as_bytes();
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.entries.get(&key) {
            return if existing.fingerprint == fingerprint {
                LlmSubmission::AlreadyKnown
            } else {
                LlmSubmission::InvalidRequest
            };
        }
        if !self.enabled() {
            return LlmSubmission::Unavailable;
        }
        let pending: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.pending)
            .collect();
        if pending.len() >= MAX_PENDING
            || pending
                .iter()
                .filter(|(key, _)| key.caller.owner == caller.owner)
                .count()
                >= MAX_PER_OWNER
            || pending
                .iter()
                .filter(|(key, _)| key.caller.computer == caller.computer)
                .count()
                >= MAX_PER_COMPUTER
        {
            return LlmSubmission::Busy;
        }
        let Ok(permit) = self.queue.as_ref().unwrap().try_reserve() else {
            return LlmSubmission::Busy;
        };
        let Ok(reservation) = gas.reserve(caller.owner, gas_quote) else {
            return LlmSubmission::InsufficientGas;
        };
        reservation
            .settle(gas_quote)
            .expect("validated LLM gas charge");
        if state.entries.len() >= MAX_RESULTS {
            if let Some(oldest) = state
                .entries
                .iter()
                .filter(|(_, entry)| !entry.pending)
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(key, _)| *key)
            {
                state.entries.remove(&oldest);
            }
        }
        state.sequence = state.sequence.wrapping_add(1);
        let sequence = state.sequence;
        let cancel = Arc::new(AtomicBool::new(false));
        state.entries.insert(
            key,
            Entry {
                fingerprint,
                status: LlmStatus::Pending,
                pending: true,
                cancel: cancel.clone(),
                sequence,
            },
        );
        permit.send(Job {
            key,
            fingerprint,
            request,
            cancel,
        });
        LlmSubmission::Accepted
    }

    pub fn poll(&self, caller: LlmCaller, request: u64) -> LlmStatus {
        self.drain();
        self.state
            .lock()
            .unwrap()
            .entries
            .get(&Key { caller, request })
            .map_or(LlmStatus::Unknown, |entry| entry.status.clone())
    }

    pub fn cancel(&self, caller: LlmCaller, request: u64) -> bool {
        self.drain();
        let mut state = self.state.lock().unwrap();
        let Some(entry) = state.entries.get_mut(&Key { caller, request }) else {
            return false;
        };
        if !entry.pending {
            return false;
        }
        entry.cancel.store(true, Ordering::Release);
        entry.status = LlmStatus::Cancelled;
        true
    }
}

fn failed(reason: &str) -> LlmStatus {
    LlmStatus::Failed {
        reason: reason.to_owned(),
    }
}

async fn worker(
    mut queue: tokio::sync::mpsc::Receiver<Job>,
    completions: mpsc::SyncSender<Completion>,
    mut ledger: spend::SpendLedger,
    provider: Arc<dyn provider::Provider>,
    spending: Arc<Mutex<Spending>>,
) -> Result<()> {
    let mut active = tokio::task::JoinSet::new();
    let mut closed = false;
    while !closed || !active.is_empty() {
        tokio::select! {
            job = queue.recv(), if !closed && active.len() < MAX_CONCURRENT => {
                let Some(job) = job else {
                    closed = true;
                    continue;
                };

                let amount = provider::reservation(&job.request);
                match ledger.reserve(job.key, job.fingerprint, amount)? {
                    spend::Admission::New => {
                        let totals = ledger.spending()?;
                        *spending.lock().unwrap() = totals;
                        bevy::log::info!(
                            request = job.key.request,
                            computer = %job.key.caller.computer,
                            reservation_microdollars = amount,
                            settled_microdollars = totals.settled_microdollars,
                            reserved_microdollars = totals.reserved_microdollars,
                            "LLM request admitted"
                        );

                        let provider = provider.clone();
                        active.spawn(async move {
                            let outcome = if job.cancel.load(Ordering::Acquire) {
                                provider::Outcome {
                                    status: LlmStatus::Cancelled,
                                    charged: Some(0),
                                }
                            } else {
                                provider.complete(job.request).await
                            };
                            (job.key, job.cancel, outcome)
                        });
                    }
                    spend::Admission::Existing(status) => {
                        let _ = completions.send(Completion {
                            key: job.key,
                            fingerprint: None,
                            authoritative: true,
                            status,
                        });
                    }
                    spend::Admission::Conflicting { fingerprint, status } => {
                        let _ = completions.send(Completion {
                            key: job.key,
                            fingerprint: Some(fingerprint),
                            authoritative: true,
                            status,
                        });
                    }
                    spend::Admission::BudgetExhausted | spend::Admission::RequestLimit => {
                        let _ = completions.send(Completion {
                            key: job.key,
                            fingerprint: None,
                            authoritative: false,
                            status: failed("Dollar budget or durable request limit exhausted"),
                        });
                    }
                }
            }
            result = active.join_next(), if !active.is_empty() => {
                let (key, cancel, mut outcome) = result.context("LLM task unavailable")??;
                if cancel.load(Ordering::Acquire) {
                    outcome.status = LlmStatus::Cancelled;
                }
                ledger.settle(key, outcome.charged, &outcome.status)?;

                let totals = ledger.spending()?;
                *spending.lock().unwrap() = totals;
                bevy::log::info!(
                    request = key.request,
                    computer = %key.caller.computer,
                    charged_microdollars = outcome.charged,
                    settled_microdollars = totals.settled_microdollars,
                    reserved_microdollars = totals.reserved_microdollars,
                    "LLM request finished"
                );
                let _ = completions.send(Completion {
                    key,
                    fingerprint: None,
                    authoritative: false,
                    status: outcome.status,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn scripted_test_service(
    path: &Path,
    responses: Vec<String>,
) -> (LlmService, Arc<std::sync::atomic::AtomicUsize>) {
    use std::{collections::VecDeque, future::Future, pin::Pin, sync::atomic::AtomicUsize};

    struct Scripted {
        responses: Mutex<VecDeque<String>>,
        calls: Arc<AtomicUsize>,
    }

    impl provider::Provider for Scripted {
        fn complete(
            &self,
            _: LlmRequest,
        ) -> Pin<Box<dyn Future<Output = provider::Outcome> + Send + '_>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::AcqRel);
                let status = self.responses.lock().unwrap().pop_front().map_or_else(
                    || failed("Scripted test provider has no remaining response"),
                    |text| LlmStatus::Ready { text },
                );
                provider::Outcome {
                    status,
                    charged: Some(0),
                }
            })
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Scripted {
        responses: Mutex::new(responses.into()),
        calls: calls.clone(),
    });
    (LlmService::start(path, provider).unwrap(), calls)
}
