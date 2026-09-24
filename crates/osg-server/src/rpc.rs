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

pub(crate) type Request = Box<dyn FnOnce(&mut World, AccountId, &BlueprintUploads) + Send>;

#[derive(Clone)]
pub(crate) struct Handler {
    requests: mpsc::Sender<Request>,
}

pub(crate) fn channel() -> (Handler, mpsc::Receiver<Request>) {
    let (requests, receive) = mpsc::channel(osg_net::rpc::MAX_CALLS);
    (Handler { requests }, receive)
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredResult {
    fingerprint: [u8; 32],
    result: Vec<u8>,
}

#[derive(Resource, Clone, Default, Serialize, Deserialize)]
pub(crate) struct OperationHistory(BTreeMap<(AccountId, Id), StoredResult>);

impl Handler {
    async fn call<T, F>(&self, epoch: Id, function: F) -> Result<T, GameError>
    where
        T: Send + 'static,
        F: FnOnce(&mut World, AccountId, &BlueprintUploads) -> anyhow::Result<T> + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        self.requests
            .send(Box::new(move |world, account, uploads| {
                if send.is_closed() {
                    return;
                }
                let result = if world.resource::<sim::identity::WorldEpoch>().0 != epoch {
                    Err(GameError("World changed; reload before retrying".into()))
                } else {
                    function(world, account, uploads).map_err(|error| GameError(error.to_string()))
                };
                let _ = send.send(result);
            }))
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
