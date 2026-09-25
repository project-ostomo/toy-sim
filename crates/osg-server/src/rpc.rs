//! Network handlers enqueue work onto the authoritative simulation thread.
use crate::{blueprint_uploads::BlueprintUploads, sim};
use bevy::prelude::*;
use osg_model::{
    AccountId, Id,
    rpc::{GameError, Operation},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use tokio::sync::{mpsc, oneshot};

mod methods;
mod queries;
#[cfg(test)]
pub use queries::{
    diplomacy, list_access_profiles, list_assets, list_orders, resolve_standing, standings,
    stock_locations, wallet_balance, wallet_history,
};

pub enum Request {
    Direct(Box<dyn FnOnce(&mut World, AccountId, &BlueprintUploads) + Send>),
    Industry(sim::industry::Incoming),
}

#[derive(Clone)]
pub struct Handler {
    requests: mpsc::Sender<Request>,
}

impl Handler {
    async fn enqueue_industry<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, GameError>>) -> sim::industry::Incoming,
    ) -> Result<T, GameError> {
        let (reply, receive) = oneshot::channel();
        self.requests
            .send(Request::Industry(build(reply)))
            .await
            .map_err(|_| GameError("Session closed".into()))?;
        receive
            .await
            .map_err(|_| GameError("Session closed".into()))?
    }

    async fn queue_mutation<A: Serialize, B>(
        &self,
        operation: Operation,
        method: &str,
        wire: A,
        arguments: B,
        wrap: impl FnOnce(sim::industry::Mutation<B>) -> sim::industry::Incoming,
    ) -> Result<(), GameError> {
        let bytes =
            postcard::to_allocvec(&(method, wire)).map_err(|error| GameError(error.to_string()))?;
        let fingerprint = *blake3::hash(&bytes).as_bytes();
        self.enqueue_industry(|reply| {
            wrap(sim::industry::Mutation {
                account: Id([0; 16]),
                operation,
                fingerprint,
                arguments,
                reply,
            })
        })
        .await
    }

    async fn call<T, F>(&self, epoch: Id, function: F) -> Result<T, GameError>
    where
        T: Send + 'static,
        F: FnOnce(&mut World, AccountId, &BlueprintUploads) -> anyhow::Result<T> + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        self.requests
            .send(Request::Direct(Box::new(move |world, account, uploads| {
                if send.is_closed() {
                    return;
                }
                let result = if world.resource::<sim::identity::WorldEpoch>().0 != epoch {
                    Err(GameError("World changed; reload before retrying".into()))
                } else {
                    function(world, account, uploads).map_err(|error| GameError(error.to_string()))
                };
                let _ = send.send(result);
            })))
            .await
            .map_err(|_| GameError("Session closed".into()))?;
        receive
            .await
            .map_err(|_| GameError("Session closed".into()))?
    }

    async fn mutate<A, T, F>(
        &self,
        operation: Operation,
        method: &'static str,
        arguments: A,
        function: F,
    ) -> Result<T, GameError>
    where
        A: Serialize + Send + 'static,
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(&mut World, AccountId, &BlueprintUploads, A) -> anyhow::Result<T>
            + Send
            + 'static,
    {
        let bytes = postcard::to_allocvec(&(method, &arguments))
            .map_err(|error| GameError(error.to_string()))?;
        let fingerprint = *blake3::hash(&bytes).as_bytes();
        self.call(operation.world, move |world, account, uploads| {
            world.init_resource::<OperationHistory>();
            let key = (account, operation.id);
            if let Some(previous) = world.resource::<OperationHistory>().0.get(&key) {
                anyhow::ensure!(
                    previous.fingerprint == fingerprint,
                    "Operation ID already used for different arguments"
                );
                return Ok(postcard::from_bytes::<Result<T, GameError>>(
                    &previous.result,
                )?);
            }
            let result = function(world, account, uploads, arguments)
                .map_err(|error| GameError(error.to_string()));
            let encoded = postcard::to_allocvec(&result)?;
            world.resource_mut::<OperationHistory>().0.insert(
                key,
                StoredResult {
                    fingerprint,
                    result: encoded,
                },
            );
            Ok(result)
        })
        .await?
    }
}

pub fn channel() -> (Handler, mpsc::Receiver<Request>) {
    let (requests, receive) = mpsc::channel(osg_net::rpc::MAX_CALLS);
    (Handler { requests }, receive)
}

pub fn enqueue_industry(
    world: &mut World,
    account: AccountId,
    uploads: &BlueprintUploads,
    incoming: sim::industry::Incoming,
) {
    use sim::industry::*;
    let pending = world.resource::<ServicePolicyQueue>().0.len()
        + world.resource::<CancellationQueue>().0.len()
        + world.resource::<CargoQueue>().0.len()
        + world.resource::<WorkQueue>().0.len()
        + world.resource::<IndustryQueryQueue>().0.len();
    if pending >= MAX_QUEUED_REQUESTS {
        incoming.reject("Industry request queue is full");
        return;
    }
    match incoming {
        Incoming::Configure(mut request) => {
            request.account = account;
            world
                .resource_mut::<ServicePolicyQueue>()
                .0
                .push_back(request);
        }
        Incoming::Cancel(mut request) => {
            request.account = account;
            world
                .resource_mut::<CancellationQueue>()
                .0
                .push_back(request);
        }
        Incoming::Cargo(mut request) => {
            request.account = account;
            world.resource_mut::<CargoQueue>().0.push_back(request);
        }
        Incoming::Work(mut request) => {
            request.account = account;
            world.resource_mut::<WorkQueue>().0.push_back(WorkItem {
                request,
                uploads: uploads.clone(),
            });
        }
        Incoming::Query(request) => {
            world
                .resource_mut::<IndustryQueryQueue>()
                .0
                .push_back(QueryItem {
                    account,
                    uploads: uploads.clone(),
                    request,
                });
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredResult {
    fingerprint: [u8; 32],
    result: Vec<u8>,
}

#[derive(Resource, Clone, Default, Serialize, Deserialize)]
pub struct OperationHistory(BTreeMap<(AccountId, Id), StoredResult>);

impl OperationHistory {
    pub fn replay<T: DeserializeOwned>(
        &self,
        account: AccountId,
        operation: Id,
        fingerprint: [u8; 32],
    ) -> anyhow::Result<Option<Result<T, GameError>>> {
        let Some(previous) = self.0.get(&(account, operation)) else {
            return Ok(None);
        };
        anyhow::ensure!(
            previous.fingerprint == fingerprint,
            "Operation ID already used for different arguments"
        );
        Ok(Some(postcard::from_bytes(&previous.result)?))
    }

    pub fn record<T: Serialize>(
        &mut self,
        account: AccountId,
        operation: Id,
        fingerprint: [u8; 32],
        result: &Result<T, GameError>,
    ) {
        self.0.insert(
            (account, operation),
            StoredResult {
                fingerprint,
                result: postcard::to_allocvec(result).expect("RPC result serialization"),
            },
        );
    }
}
