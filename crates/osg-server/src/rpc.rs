//! Network handlers enqueue work onto the authoritative simulation thread.
use crate::{blueprint_uploads::BlueprintUploads, sim};
use bevy::prelude::*;
use osg_model::{AccountId, Id, rpc::GameError};
use tokio::sync::{mpsc, oneshot};

mod methods;
mod queries;
#[cfg(test)]
pub use queries::{
    diplomacy, list_access_profiles, list_assets, list_orders, resolve_standing, standings,
    stock_locations, wallet_balance, wallet_history,
};

pub enum Request {
    Direct(Box<dyn FnOnce(&World, AccountId, &BlueprintUploads) + Send>),
    Industry(sim::industry::Incoming),
    Society(sim::industry::Mutation<sim::society::Action>),
}

#[derive(Clone)]
pub struct Handler {
    requests: mpsc::Sender<Request>,
}

impl Handler {
    async fn queue_society<A: Into<sim::society::Action>>(
        &self,
        world: Id,
        command: A,
    ) -> Result<(), GameError> {
        let (reply, receive) = oneshot::channel();
        self.requests
            .send(Request::Society(sim::industry::Mutation {
                account: Id([0; 16]),
                world,
                arguments: command.into(),
                reply,
            }))
            .await
            .map_err(|_| GameError("Session closed".into()))?;
        receive
            .await
            .map_err(|_| GameError("Session closed".into()))?
    }

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

    async fn queue_mutation<A>(
        &self,
        world: Id,
        arguments: A,
        wrap: impl FnOnce(sim::industry::Mutation<A>) -> sim::industry::Incoming,
    ) -> Result<(), GameError> {
        self.enqueue_industry(|reply| {
            wrap(sim::industry::Mutation {
                account: Id([0; 16]),
                world,
                arguments,
                reply,
            })
        })
        .await
    }

    async fn call<T, F>(&self, epoch: Id, function: F) -> Result<T, GameError>
    where
        T: Send + 'static,
        F: FnOnce(&World, AccountId, &BlueprintUploads) -> anyhow::Result<T> + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        self.requests
            .send(Request::Direct(Box::new(move |world, account, uploads| {
                if send.is_closed() {
                    return;
                }
                let result = if world.resource::<sim::identity::WorldEpoch>().0 != epoch {
                    Err(GameError("World changed; refresh state".into()))
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
