use anyhow::{Result, ensure};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use toy_sim_model::{AccountId, industry::BlueprintUploadAck};

const SESSION_BYTES: usize = 64 * 1024 * 1024;
const ACCOUNT_BYTES: usize = 128 * 1024 * 1024;
const GLOBAL_BYTES: usize = 256 * 1024 * 1024;
const INITIAL_CAPACITY: usize = 64 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub(crate) struct BlueprintUploadBudget(Arc<Mutex<Usage>>);

#[derive(Default)]
struct Usage {
    total: usize,
    accounts: BTreeMap<AccountId, usize>,
}

struct SessionQuota {
    budget: BlueprintUploadBudget,
    account: AccountId,
    used: AtomicUsize,
}

struct Lease {
    quota: Arc<SessionQuota>,
    bytes: usize,
}

impl Lease {
    fn reserve(&mut self, additional: usize) -> Result<()> {
        let mut usage = self.quota.budget.0.lock().unwrap();
        let session = self.quota.used.load(Ordering::Relaxed);
        let account = usage
            .accounts
            .get(&self.quota.account)
            .copied()
            .unwrap_or(0);
        ensure!(
            additional <= SESSION_BYTES.saturating_sub(session),
            "blueprint session quota exceeded"
        );
        ensure!(
            additional <= ACCOUNT_BYTES.saturating_sub(account),
            "blueprint account quota exceeded"
        );
        ensure!(
            additional <= GLOBAL_BYTES.saturating_sub(usage.total),
            "blueprint server quota exceeded"
        );

        self.quota
            .used
            .store(session + additional, Ordering::Relaxed);
        usage
            .accounts
            .insert(self.quota.account, account + additional);
        usage.total += additional;
        self.bytes += additional;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut usage = self.quota.budget.0.lock().unwrap();
        self.quota.used.fetch_sub(self.bytes, Ordering::Relaxed);
        usage.total -= self.bytes;
        if let Some(account) = usage.accounts.get_mut(&self.quota.account) {
            *account -= self.bytes;
            if *account == 0 {
                usage.accounts.remove(&self.quota.account);
            }
        }
    }
}

impl BlueprintUploadBudget {
    pub(crate) fn session(&self, account: AccountId) -> BlueprintUploads {
        BlueprintUploads(Arc::new(Store {
            files: Mutex::new(BTreeMap::new()),
            quota: Arc::new(SessionQuota {
                budget: self.clone(),
                account,
                used: AtomicUsize::new(0),
            }),
        }))
    }
}

#[derive(Clone)]
pub struct BlueprintUploads(Arc<Store>);

struct Store {
    files: Mutex<BTreeMap<[u8; 32], Arc<UploadedBlueprint>>>,
    quota: Arc<SessionQuota>,
}

pub struct UploadedBlueprint {
    bytes: Vec<u8>,
    _lease: Lease,
}

impl AsRef<[u8]> for UploadedBlueprint {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Default for BlueprintUploads {
    fn default() -> Self {
        BlueprintUploadBudget::default().session(AccountId::default())
    }
}

impl BlueprintUploads {
    pub fn get(&self, hash: [u8; 32]) -> Result<Arc<UploadedBlueprint>> {
        self.0
            .files
            .lock()
            .unwrap()
            .get(&hash)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("blueprint has not been uploaded in this session"))
    }

    pub(crate) async fn receive<R: AsyncRead + Unpin>(&self, read: &mut R) -> Result<[u8; 32]> {
        let mut hash = [0; 32];
        read.read_exact(&mut hash).await?;
        if let Ok(existing) = self.get(hash) {
            let mut offset = 0;
            let mut buffer = [0; 8192];
            loop {
                let count = read.read(&mut buffer).await?;
                if count == 0 {
                    ensure!(offset == existing.bytes.len(), "blueprint truncated");
                    return Ok(hash);
                }
                ensure!(
                    existing.bytes.get(offset..offset + count) == Some(&buffer[..count]),
                    "blueprint hash mismatch"
                );
                offset += count;
            }
        }

        let mut lease = Lease {
            quota: self.0.quota.clone(),
            bytes: 0,
        };
        let mut bytes = Vec::new();
        loop {
            if bytes.len() == toy_sim_ships::MAX_FILE {
                let mut extra = [0];
                ensure!(
                    read.read(&mut extra).await? == 0,
                    "blueprint exceeds 16 MiB"
                );
                break;
            }
            if bytes.len() == bytes.capacity() {
                let capacity = (bytes.capacity() * 2)
                    .max(INITIAL_CAPACITY)
                    .min(toy_sim_ships::MAX_FILE);
                lease.reserve(capacity - bytes.capacity())?;
                bytes.try_reserve_exact(capacity - bytes.len())?;
            }
            if read.read_buf(&mut bytes).await? == 0 {
                break;
            }
        }
        ensure!(!bytes.is_empty(), "empty blueprint");
        ensure!(
            *blake3::hash(&bytes).as_bytes() == hash,
            "blueprint hash mismatch"
        );

        let uploaded = Arc::new(UploadedBlueprint {
            bytes,
            _lease: lease,
        });
        self.0.files.lock().unwrap().entry(hash).or_insert(uploaded);
        Ok(hash)
    }
}

pub(crate) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    uploads: &BlueprintUploads,
) -> Result<()> {
    serve_with_timeout(stream, uploads, TRANSFER_TIMEOUT).await
}

async fn serve_with_timeout<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    uploads: &BlueprintUploads,
    timeout: Duration,
) -> Result<()> {
    tokio::time::timeout(timeout, async {
        let ack = match uploads.receive(stream).await {
            Ok(hash) => BlueprintUploadAck::Ready { hash },
            Err(error) => {
                let mut reason = error.to_string();
                let mut end = reason
                    .len()
                    .min(toy_sim_model::industry::MAX_BLUEPRINT_UPLOAD_ERROR_BYTES);
                while !reason.is_char_boundary(end) {
                    end -= 1;
                }
                reason.truncate(end);
                BlueprintUploadAck::Rejected { reason }
            }
        };
        let bytes = toy_sim_protocol::encode_blueprint_upload_ack(&ack)?;
        stream.write_all(&bytes).await?;
        stream.shutdown().await?;
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::DuplexStream;

    fn wire(bytes: &[u8]) -> Vec<u8> {
        let mut wire = blake3::hash(bytes).as_bytes().to_vec();
        wire.extend_from_slice(bytes);
        wire
    }

    async fn read_ack(stream: &mut DuplexStream) -> BlueprintUploadAck {
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        toy_sim_protocol::decode_blueprint_upload_ack(&bytes).unwrap()
    }

    #[tokio::test]
    async fn upload_commits_at_eof_and_remains_private_with_a_leased_handle() {
        let budget = BlueprintUploadBudget::default();
        let account = AccountId::new();
        let uploads = budget.session(account);
        let other_session = budget.session(account);
        let bytes = vec![23; 70 * 1024];
        let hash = *blake3::hash(&bytes).as_bytes();
        let (mut client, mut server) = tokio::io::duplex(4096);
        let serving_store = uploads.clone();
        let serving = tokio::spawn(async move { serve(&mut server, &serving_store).await });

        client.write_all(&wire(&bytes)).await.unwrap();
        let mut first = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(10), client.read(&mut first))
                .await
                .is_err()
        );
        assert!(uploads.get(hash).is_err());
        client.shutdown().await.unwrap();
        assert!(
            matches!(read_ack(&mut client).await, BlueprintUploadAck::Ready { hash: received } if received == hash)
        );
        serving.await.unwrap().unwrap();

        let handle = uploads.get(hash).unwrap();
        assert_eq!(handle.as_ref().as_ref(), bytes.as_slice());
        assert!(other_session.get(hash).is_err());
        let used = budget.0.lock().unwrap().total;
        uploads.receive(&mut wire(&bytes).as_slice()).await.unwrap();
        assert!(Arc::ptr_eq(&handle, &uploads.get(hash).unwrap()));
        assert_eq!(budget.0.lock().unwrap().total, used);

        drop(uploads);
        assert_eq!(budget.0.lock().unwrap().total, used);
        drop(handle);
        assert_eq!(budget.0.lock().unwrap().total, 0);
    }

    #[tokio::test]
    async fn invalid_uploads_return_bounded_rejections_and_release_all_bytes() {
        let uploads = BlueprintUploads::default();
        let mut incorrect_hash = wire(b"a blueprint");
        incorrect_hash[0] ^= 1;
        let mut truncated = wire(b"a truncated blueprint");
        truncated.pop();
        let oversized = wire(&vec![7; toy_sim_ships::MAX_FILE + 1]);
        for request in [incorrect_hash, truncated, vec![0; 12], wire(b""), oversized] {
            let (mut client, mut server) = tokio::io::duplex(4096);
            let serving_store = uploads.clone();
            let serving = tokio::spawn(async move { serve(&mut server, &serving_store).await });
            client.write_all(&request).await.unwrap();
            client.shutdown().await.unwrap();
            assert!(matches!(
                read_ack(&mut client).await,
                BlueprintUploadAck::Rejected { .. }
            ));
            serving.await.unwrap().unwrap();
            assert!(uploads.0.files.lock().unwrap().is_empty());
            assert_eq!(uploads.0.quota.budget.0.lock().unwrap().total, 0);
        }
    }

    #[test]
    fn session_account_and_listener_quotas_reserve_atomically_and_release() {
        let budget = BlueprintUploadBudget::default();
        let account = AccountId::new();
        let mut first = Lease {
            quota: budget.session(account).0.quota.clone(),
            bytes: 0,
        };
        first.reserve(SESSION_BYTES).unwrap();
        assert!(
            first
                .reserve(1)
                .unwrap_err()
                .to_string()
                .contains("session")
        );

        let mut second = Lease {
            quota: budget.session(account).0.quota.clone(),
            bytes: 0,
        };
        second.reserve(SESSION_BYTES).unwrap();
        let mut third = Lease {
            quota: budget.session(account).0.quota.clone(),
            bytes: 0,
        };
        assert!(
            third
                .reserve(1)
                .unwrap_err()
                .to_string()
                .contains("account")
        );
        assert_eq!(budget.0.lock().unwrap().total, ACCOUNT_BYTES);

        let mut other = Vec::new();
        for _ in 0..2 {
            let mut lease = Lease {
                quota: budget.session(AccountId::new()).0.quota.clone(),
                bytes: 0,
            };
            lease.reserve(SESSION_BYTES).unwrap();
            other.push(lease);
        }
        let mut final_lease = Lease {
            quota: budget.session(AccountId::new()).0.quota.clone(),
            bytes: 0,
        };
        assert!(
            final_lease
                .reserve(1)
                .unwrap_err()
                .to_string()
                .contains("server")
        );
        assert_eq!(budget.0.lock().unwrap().total, GLOBAL_BYTES);
        drop(first);
        third.reserve(SESSION_BYTES).unwrap();
        drop(other);
        drop(second);
        drop(third);
        drop(final_lease);
        let usage = budget.0.lock().unwrap();
        assert_eq!(usage.total, 0);
        assert!(usage.accounts.is_empty());
    }

    #[tokio::test]
    async fn stalled_transfer_deadline_releases_inflight_capacity() {
        let uploads = BlueprintUploads::default();
        let (mut client, mut server) = tokio::io::duplex(4096);
        let serving_store = uploads.clone();
        let serving = tokio::spawn(async move {
            serve_with_timeout(&mut server, &serving_store, Duration::from_millis(30)).await
        });
        client.write_all(&[0; 33]).await.unwrap();
        assert!(serving.await.unwrap().is_err());
        assert_eq!(uploads.0.quota.budget.0.lock().unwrap().total, 0);
        assert!(uploads.0.files.lock().unwrap().is_empty());
    }
}
