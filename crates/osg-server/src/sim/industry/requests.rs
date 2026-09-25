use super::*;
use crate::{OperationHistory, blueprint_uploads::BlueprintUploads};
use osg_model::rpc::{GameError, Operation, Page};
use std::collections::VecDeque;
use tokio::sync::oneshot;

pub const MAX_QUEUED_REQUESTS: usize = 16_384;

pub struct Mutation<A> {
    pub account: AccountId,
    pub operation: Operation,
    pub fingerprint: [u8; 32],
    pub arguments: A,
    pub reply: oneshot::Sender<Result<(), GameError>>,
}

impl<A> Mutation<A> {
    pub fn previous(&self, epoch: Id, history: &OperationHistory) -> Option<Result<(), GameError>> {
        if epoch != self.operation.world {
            return Some(Err(GameError(
                "World changed; reload before retrying".into(),
            )));
        }
        match history.replay(self.account, self.operation.id, self.fingerprint) {
            Ok(previous) => previous,
            Err(error) => Some(Err(GameError(error.to_string()))),
        }
    }

    pub fn finish(self, history: &mut OperationHistory, result: Result<()>) {
        let result = result.map_err(|error| GameError(error.to_string()));
        history.record(self.account, self.operation.id, self.fingerprint, &result);
        let _ = self.reply.send(result);
    }
}

pub enum StartWork {
    Recipe {
        facility: Id,
        recipe: String,
        batches: u32,
    },
    Ship {
        facility: Id,
        owner: Principal,
        blueprint_hash: [u8; 32],
    },
    Public(ServiceQuote),
}

pub struct WorkItem {
    pub request: Mutation<StartWork>,
    pub uploads: BlueprintUploads,
}

pub struct ConfigureService {
    pub facility: Id,
    pub policy: ServicePolicy,
}

pub struct CancelWork {
    pub facility: Id,
    pub job: Id,
}

pub enum CargoAction {
    Transfer {
        source: Id,
        target: Id,
        item: CargoItem,
        quantity: u64,
    },
    Refill {
        source: Id,
        target: Id,
        resource: String,
        quantity: u64,
    },
    UnloadProduct {
        source: Id,
        target: Id,
        resource: String,
        quantity: u64,
    },
}

#[derive(Resource, Default)]
pub struct ServicePolicyQueue(pub VecDeque<Mutation<ConfigureService>>);
#[derive(Resource, Default)]
pub struct CancellationQueue(pub VecDeque<Mutation<CancelWork>>);
#[derive(Resource, Default)]
pub struct CargoQueue(pub VecDeque<Mutation<CargoAction>>);
#[derive(Resource, Default)]
pub struct WorkQueue(pub VecDeque<WorkItem>);
#[derive(Resource, Default)]
pub struct IndustryQueryQueue(pub VecDeque<QueryItem>);

pub struct ReadRequest<A, R> {
    pub world: Id,
    pub arguments: A,
    pub reply: oneshot::Sender<Result<R, GameError>>,
}

impl<A, R> ReadRequest<A, R> {
    pub fn answer(self, epoch: Id, answer: impl FnOnce(A) -> Result<R>) {
        if self.reply.is_closed() {
            return;
        }
        let result = if self.world != epoch {
            Err(anyhow::anyhow!("World changed; reload before retrying"))
        } else {
            answer(self.arguments)
        };
        let _ = self
            .reply
            .send(result.map_err(|error| GameError(error.to_string())));
    }
}

pub enum IndustryQueryRequest {
    Directory(ReadRequest<(Option<Id>, u16), Page<FacilitySummary, Id>>),
    Facility(ReadRequest<Id, FacilityView>),
    Hangar(ReadRequest<(Id, Option<Id>), HangarView>),
    Catalogue(ReadRequest<(), IndustryCatalogue>),
    PublicFacilities(ReadRequest<(String, Option<Id>, u16), Page<PublicFacility, Id>>),
    Quote(ReadRequest<(Id, Principal, ServiceWork), ServiceQuote>),
    Jobs(ReadRequest<Id, Vec<JobView>>),
}

pub struct QueryItem {
    pub account: AccountId,
    pub uploads: BlueprintUploads,
    pub request: IndustryQueryRequest,
}

/// Transport attaches authentication before handing these values to the queues.
pub enum Incoming {
    Configure(Mutation<ConfigureService>),
    Cancel(Mutation<CancelWork>),
    Cargo(Mutation<CargoAction>),
    Work(Mutation<StartWork>),
    Query(IndustryQueryRequest),
}

impl Incoming {
    pub fn reject(self, message: &str) {
        let error = || GameError(message.into());
        match self {
            Self::Configure(request) => {
                let _ = request.reply.send(Err(error()));
            }
            Self::Cancel(request) => {
                let _ = request.reply.send(Err(error()));
            }
            Self::Cargo(request) => {
                let _ = request.reply.send(Err(error()));
            }
            Self::Work(request) => {
                let _ = request.reply.send(Err(error()));
            }
            Self::Query(query) => match query {
                IndustryQueryRequest::Directory(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::Facility(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::Hangar(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::Catalogue(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::PublicFacilities(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::Quote(request) => {
                    let _ = request.reply.send(Err(error()));
                }
                IndustryQueryRequest::Jobs(request) => {
                    let _ = request.reply.send(Err(error()));
                }
            },
        }
    }
}
