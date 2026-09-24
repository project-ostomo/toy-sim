use crate::{RpcError, picomux::PicoMux, rpc};
use osg_model::{AccountId, Action, Frame, Id, InputFrame, UniverseDescriptor};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Semaphore, broadcast, mpsc, watch},
};

#[derive(Clone, Debug)]
pub enum NetEvent {
    Session {
        world: Id,
        universe: UniverseDescriptor,
    },
    Frame(Arc<Frame>),
}

pub struct EventSubscription {
    initial: Option<NetEvent>,
    events: broadcast::Receiver<NetEvent>,
}

impl EventSubscription {
    pub fn len(&self) -> usize {
        self.events.len() + usize::from(self.initial.is_some())
    }

    pub async fn recv(&mut self) -> Result<NetEvent, broadcast::error::RecvError> {
        if let Some(initial) = self.initial.take() {
            return Ok(initial);
        }
        self.events.recv().await
    }

    pub fn try_recv(&mut self) -> Result<NetEvent, broadcast::error::TryRecvError> {
        if let Some(initial) = self.initial.take() {
            return Ok(initial);
        }
        self.events.try_recv()
    }
}

struct Events {
    session: Option<NetEvent>,
    sender: Option<broadcast::Sender<NetEvent>>,
}

struct Inner {
    mux: Arc<PicoMux>,
    runtime: tokio::runtime::Handle,
    calls: Arc<Semaphore>,
    events: Arc<Mutex<Events>>,
    input: mpsc::Sender<InputBatch>,
    closed: watch::Sender<bool>,
    status: watch::Receiver<Option<String>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closed.send_replace(true);
    }
}

#[derive(Clone)]
pub struct OsgNetClient(Arc<Inner>);

struct InputBatch {
    world: Id,
    actions: Vec<(Id, Action)>,
}

#[derive(Debug)]
pub struct InputError {
    pub actions: Vec<(Id, Action)>,
    pub reason: &'static str,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for InputError {}

impl OsgNetClient {
    pub async fn connect(
        address: &str,
        server_key: ed25519_dalek::VerifyingKey,
        account: AccountId,
        account_key: &ed25519_dalek::SigningKey,
    ) -> anyhow::Result<Self> {
        let mux = crate::connect_mux(address, server_key, account, account_key).await?;
        Self::from_mux(mux).await
    }

    async fn from_mux(mux: PicoMux) -> anyhow::Result<Self> {
        let mux = Arc::new(mux);
        let main = mux.open(b"main").await?;
        let (sender, _) = broadcast::channel(64);
        let events = Arc::new(Mutex::new(Events {
            session: None,
            sender: Some(sender),
        }));
        let (input, received) = mpsc::channel(16);
        let (closed, closing) = watch::channel(false);
        let (status, connection_status) = watch::channel(None);
        let inner = Arc::new(Inner {
            mux: mux.clone(),
            runtime: tokio::runtime::Handle::current(),
            calls: Arc::new(Semaphore::new(rpc::MAX_CALLS)),
            events: events.clone(),
            input,
            closed,
            status: connection_status,
        });
        let calls = inner.calls.clone();
        tokio::spawn(async move {
            let result = pump(mux, main, events.clone(), received, closing).await;
            let reason = match result {
                Ok(()) => "Connection closed".to_owned(),
                Err(error) => format!("Connection closed: {error:#}"),
            };
            status.send_replace(Some(reason));
            calls.close();
            events.lock().unwrap().sender.take();
        });
        Ok(Self(inner))
    }

    /// Start with the current descriptor, then receive all future main events.
    /// Lag is explicit: the receiver must not silently resume playback after it.
    pub fn subscribe_events(&self) -> EventSubscription {
        let events = self.0.events.lock().unwrap();
        let receiver = match &events.sender {
            Some(sender) => sender.subscribe(),
            None => broadcast::channel(1).1,
        };
        EventSubscription {
            initial: events.session.clone(),
            events: receiver,
        }
    }

    pub fn session(&self) -> Option<(Id, UniverseDescriptor)> {
        match &self.0.events.lock().unwrap().session {
            Some(NetEvent::Session { world, universe }) => Some((*world, universe.clone())),
            _ => None,
        }
    }

    pub fn status(&self) -> watch::Receiver<Option<String>> {
        self.0.status.clone()
    }

    pub fn close(&self) {
        self.0.closed.send_replace(true);
        self.0.calls.close();
    }

    fn validate_world(&self, world: Id) -> Result<(), &'static str> {
        if *self.0.closed.borrow() || self.0.status.borrow().is_some() {
            return Err("connection closed");
        }
        if self.session().is_none_or(|(current, _)| current != world) {
            return Err("world changed or session not ready");
        }
        Ok(())
    }

    pub fn try_send_inputs(&self, world: Id, actions: Vec<(Id, Action)>) -> Result<(), InputError> {
        if let Err(reason) = self.validate_world(world) {
            return Err(InputError { actions, reason });
        }
        self.0
            .input
            .try_send(InputBatch { world, actions })
            .map_err(|error| {
                let reason = match &error {
                    mpsc::error::TrySendError::Full(_) => "input queue busy",
                    mpsc::error::TrySendError::Closed(_) => "connection closed",
                };
                InputError {
                    actions: error.into_inner().actions,
                    reason,
                }
            })
    }

    /// The caller can retain its batch until this returns, including cancellation.
    pub async fn send_inputs(&self, world: Id, actions: &[(Id, Action)]) -> Result<(), InputError> {
        let permit = self.0.input.reserve().await.map_err(|_| InputError {
            actions: actions.to_vec(),
            reason: "connection closed",
        })?;
        self.validate_world(world).map_err(|reason| InputError {
            actions: actions.to_vec(),
            reason,
        })?;
        permit.send(InputBatch {
            world,
            actions: actions.to_vec(),
        });
        Ok(())
    }

    async fn run<T, F>(&self, deadline: std::time::Duration, operation: F) -> Result<T, RpcError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, RpcError>> + Send + 'static,
    {
        let permits = self.0.calls.clone();
        let mut closing = self.0.closed.subscribe();
        let mut status = self.0.status.clone();
        let task = self.0.runtime.spawn(async move {
            if *closing.borrow() || status.borrow().is_some() {
                return Err(RpcError::Closed);
            }
            tokio::select! {
                result = tokio::time::timeout(deadline, async {
                    let _permit = permits.acquire_owned().await.map_err(|_| RpcError::Closed)?;
                    operation.await
                }) => result.unwrap_or(Err(RpcError::Timeout)),
                _ = closing.changed() => Err(RpcError::Closed),
                _ = status.changed() => Err(RpcError::Closed),
            }
        });
        struct AbortOnDrop(tokio::task::AbortHandle);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _guard = AbortOnDrop(task.abort_handle());
        task.await.map_err(|_| RpcError::Closed)?
    }

    pub(crate) async fn rpc_call(
        &self,
        method: &'static str,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, RpcError> {
        let mux = self.0.mux.clone();
        self.run(rpc::DEADLINE, async move {
            let mut stream = mux.open(format!("rpc:{method}").as_bytes()).await?;
            rpc::write(&mut stream, &bytes).await?;
            rpc::read(&mut stream, rpc::MAX_RESPONSE).await
        })
        .await
    }

    pub async fn fetch_asset(&self, hash: [u8; 32]) -> Result<Vec<u8>, RpcError> {
        let mux = self.0.mux.clone();
        self.run(std::time::Duration::from_secs(60), async move {
            let mut stream = mux.open(b"assets").await?;
            stream.write_all(&hash).await?;
            stream.shutdown().await?;
            let mut bytes = Vec::new();
            stream
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(RpcError::Protocol("asset exceeds byte limit".into()));
            }
            if blake3::hash(&bytes).as_bytes() != &hash {
                return Err(RpcError::Protocol(
                    "asset missing or content hash mismatch".into(),
                ));
            }
            Ok(bytes)
        })
        .await
    }

    pub async fn upload_blueprint(&self, bytes: Arc<[u8]>) -> Result<[u8; 32], RpcError> {
        if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
            return Err(RpcError::Protocol(
                "blueprint must contain between 1 byte and 16 MiB".into(),
            ));
        }
        let mux = self.0.mux.clone();
        self.run(std::time::Duration::from_secs(60), async move {
            let hash = *blake3::hash(&bytes).as_bytes();
            let mut stream = mux.open(b"blueprint-upload").await?;
            stream.write_all(&hash).await?;
            stream.write_all(&bytes).await?;
            stream.shutdown().await?;
            let mut ack = Vec::new();
            stream.take(64 * 1024 + 1).read_to_end(&mut ack).await?;
            if ack.len() > 64 * 1024 {
                return Err(RpcError::Protocol(
                    "upload acknowledgement exceeds byte limit".into(),
                ));
            }
            match osg_protocol::decode_blueprint_upload_ack(&ack)
                .map_err(|error| RpcError::Protocol(error.to_string()))?
            {
                osg_model::industry::BlueprintUploadAck::Ready { hash: received }
                    if received == hash =>
                {
                    Ok(hash)
                }
                osg_model::industry::BlueprintUploadAck::Ready { .. } => {
                    Err(RpcError::Protocol("upload hash mismatch".into()))
                }
                osg_model::industry::BlueprintUploadAck::Rejected { reason } => {
                    Err(RpcError::Protocol(reason))
                }
            }
        })
        .await
    }
}

async fn pump(
    mux: Arc<PicoMux>,
    stream: crate::picomux::Stream,
    events: Arc<Mutex<Events>>,
    mut input: mpsc::Receiver<InputBatch>,
    mut closing: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (mut read, mut write) = tokio::io::split(stream);
    let reader = async {
        loop {
            let event = match crate::read_message(&mut read).await? {
                osg_protocol::Message::Session { world, universe } => {
                    NetEvent::Session { world, universe }
                }
                osg_protocol::Message::State(frame) => NetEvent::Frame(Arc::new(frame)),
                _ => anyhow::bail!("expected session descriptor or state"),
            };
            let mut events = events.lock().unwrap();
            if matches!(event, NetEvent::Session { .. }) {
                events.session = Some(event.clone());
            } else {
                anyhow::ensure!(events.session.is_some(), "state before session descriptor");
            }
            if let Some(sender) = &events.sender {
                let _ = sender.send(event);
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let writer = async {
        let mut sequence = 0_u64;
        while let Some(batch) = input.recv().await {
            let current = match &events.lock().unwrap().session {
                Some(NetEvent::Session { world, .. }) => Some(*world),
                _ => None,
            };
            if current != Some(batch.world) {
                continue;
            }
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("input sequence exhausted"))?;
            let frame = InputFrame {
                world: batch.world,
                sequence,
                actions: batch.actions,
            };
            crate::write_message(&mut write, &osg_protocol::Message::Input(frame)).await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        result = reader => result,
        result = writer => result,
        result = mux.wait_until_dead() => result.map_err(Into::into),
        _ = closing.changed() => Ok(()),
    }
}

#[cfg(test)]
mod tests;
