use anyhow::{Context, Result, ensure};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use osg_model::wasm_world::ReplyCapacity;
use osg_model::{
    Id, ProgramReply,
    ownership::Principal,
    routing::{self as dto, Plan, Request, Status},
    travel::{PlanningProgress, PlanningStage},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    identity, infrastructure::NavigationPublication, ownership, routing, services,
    simulation::SimulationCounters, travel,
};

mod performance;
pub use performance::fuel_budget;

#[cfg(test)]
mod tests;

const MAX_PENDING: usize = 64;
const MAX_PER_OWNER: usize = 8;
const MAX_PER_SHIP: usize = 4;
const MAX_ENTRIES: usize = 256;
const MAX_WORKERS: usize = 2;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Origin {
    Explicit,
    Automatic,
}

#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub world: Id,
    pub ship: Id,
    pub owner: Principal,
    pub authority_revision: u64,
    pub travel_revision: u64,
    pub topology_revision: u64,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct Scope {
    world: Id,
    ship: Id,
    owner: Principal,
    authority: u64,
    origin: Origin,
}

impl Caller {
    fn scope(self) -> Scope {
        Scope {
            world: self.world,
            ship: self.ship,
            owner: self.owner,
            authority: self.authority_revision,
            origin: self.origin,
        }
    }

    fn current(self, other: Self) -> bool {
        self.scope() == other.scope() && self.travel_revision == other.travel_revision
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct Key {
    scope: Scope,
    request: u64,
}

struct Entry {
    caller: Caller,
    request: Request,
    fingerprint: [u8; 32],
    sequence: u64,
    status: Status,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    generation: u64,
    sequence: u64,
    entries: HashMap<Key, Entry>,
    queue: VecDeque<Key>,
}

#[derive(Resource, Clone, Default)]
pub struct RouteService(Arc<Mutex<State>>);

struct Running {
    key: Key,
    generation: u64,
    cancel: Arc<AtomicBool>,
    task: Task<Status>,
}

#[derive(Resource, Default)]
struct Workers(Vec<Running>);

pub struct ReadyRoute {
    pub goals: Vec<osg_model::travel::Order>,
    pub plan: Plan,
    pub preferences: osg_model::travel::PlanningPreferences,
}

fn pending(stage: PlanningStage) -> Status {
    Status::Pending {
        progress: PlanningProgress {
            stage,
            completed: 0,
            total: None,
        },
    }
}

fn failed(reason: impl ToString) -> Status {
    let text = reason.to_string();
    let mut end = text.len().min(256);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Status::Failed {
        reason: text[..end].to_owned(),
    }
}

fn fits(id: u64, status: &Status, capacity: ReplyCapacity) -> Result<()> {
    if !osg_model::wasm_beacons::reply_fits(
        &ProgramReply::Route {
            id,
            status: status.clone(),
        },
        capacity,
    ) {
        return Err(osg_ship_wasm::WorldQueryError::BufferTooSmall.into());
    }
    Ok(())
}

impl RouteService {
    pub fn submit(
        &self,
        caller: Caller,
        request: Request,
        reply_capacity: ReplyCapacity,
    ) -> Result<Status> {
        ensure!(
            request.id != 0 && request.orders.len() <= dto::MAX_ORDERS,
            "invalid route request"
        );
        osg_protocol::validate_ship_command(&osg_model::ShipCommand::SetTravel {
            preferences: request.preferences,
            engage: false,
            expected_revision: caller.travel_revision,
            orders: request.orders.clone(),
        })?;
        let fingerprint = *blake3::hash(&postcard::to_stdvec(&request)?).as_bytes();
        let key = Key {
            scope: caller.scope(),
            request: request.id,
        };
        let mut state = self.0.lock().unwrap();
        if let Some(entry) = state.entries.get(&key) {
            ensure!(
                entry.fingerprint == fingerprint,
                "route request ID was already used"
            );
            let status = entry_status(entry, caller);
            fits(request.id, &status, reply_capacity)?;
            return Ok(status);
        }

        let status = pending(PlanningStage::LoadingCatalogue);
        fits(request.id, &status, reply_capacity)?;
        let pending = state
            .entries
            .values()
            .filter(|entry| matches!(entry.status, Status::Pending { .. }))
            .collect::<Vec<_>>();
        ensure!(pending.len() < MAX_PENDING, "route service queue is full");
        ensure!(
            pending
                .iter()
                .filter(|entry| entry.caller.owner == caller.owner)
                .count()
                < MAX_PER_OWNER,
            "owner has too many pending routes"
        );
        ensure!(
            pending
                .iter()
                .filter(|entry| entry.caller.ship == caller.ship)
                .count()
                < MAX_PER_SHIP,
            "ship has too many pending routes"
        );

        if state.entries.len() >= MAX_ENTRIES {
            let oldest = state
                .entries
                .iter()
                .filter(|(_, entry)| !matches!(entry.status, Status::Pending { .. }))
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(&key, _)| key)
                .context("route result cache is full")?;
            state.entries.remove(&oldest);
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .expect("route sequence exhausted");
        let sequence = state.sequence;
        state.entries.insert(
            key,
            Entry {
                caller,
                request,
                fingerprint,
                sequence,
                status: status.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        state.queue.push_back(key);
        Ok(status)
    }

    pub fn poll(&self, caller: Caller, id: u64) -> Status {
        let state = self.0.lock().unwrap();
        state
            .entries
            .get(&Key {
                scope: caller.scope(),
                request: id,
            })
            .map_or(Status::Unknown, |entry| entry_status(entry, caller))
    }
}

fn entry_status(entry: &Entry, caller: Caller) -> Status {
    if entry.caller.current(caller) {
        entry.status.clone()
    } else {
        failed("route inputs changed; request a new plan")
    }
}

pub fn caller(world: &World, ship: Entity) -> Result<Caller> {
    ensure!(
        !matches!(
            world
                .get::<travel::PresenceState>(ship)
                .map(|state| &state.0),
            Some(
                osg_model::travel::Presence::Destroyed
                    | osg_model::travel::Presence::StoredInWreck(_)
            )
        ),
        "ship unavailable"
    );
    Ok(Caller {
        world: world.resource::<identity::WorldEpoch>().0,
        ship: world
            .get::<identity::Identity>(ship)
            .context("ship identity unavailable")?
            .0,
        owner: world
            .get::<ownership::AssetOwner>(ship)
            .context("ship owner unavailable")?
            .0,
        authority_revision: world
            .get::<identity::Control>(ship)
            .context("ship authority unavailable")?
            .revision,
        travel_revision: world
            .get::<travel::Travel>(ship)
            .context("ship navigation unavailable")?
            .0
            .revision,
        topology_revision: world
            .get_resource::<NavigationPublication>()
            .map_or(0, |publication| publication.revision),
        origin: Origin::Explicit,
    })
}

pub fn submit(world: &mut World, ship: Entity, request: Request) -> Result<Status> {
    submit_origin(world, ship, request, Origin::Explicit)
}

pub fn submit_automatic(world: &mut World, ship: Entity, request: Request) -> Result<Status> {
    submit_origin(world, ship, request, Origin::Automatic)
}

fn submit_origin(
    world: &mut World,
    ship: Entity,
    request: Request,
    origin: Origin,
) -> Result<Status> {
    let mut caller = caller(world, ship)?;
    caller.origin = origin;
    world.init_resource::<RouteService>();
    world
        .resource::<RouteService>()
        .submit(caller, request, ReplyCapacity::UNLIMITED)
}

pub fn poll(world: &World, ship: Entity, id: u64) -> Result<Status> {
    poll_origin(world, ship, id, Origin::Explicit)
}

pub fn cancel(world: &World, ship: Entity, id: u64) -> Result<()> {
    let caller = caller(world, ship)?;
    if let Some(service) = world.get_resource::<RouteService>() {
        let key = Key {
            scope: caller.scope(),
            request: id,
        };
        let mut state = service.0.lock().unwrap();
        if let Some(entry) = state.entries.remove(&key) {
            entry.cancel.store(true, Ordering::Relaxed);
        }
        state.queue.retain(|queued| *queued != key);
    }
    Ok(())
}

pub fn poll_automatic(world: &World, ship: Entity, id: u64) -> Result<Status> {
    poll_origin(world, ship, id, Origin::Automatic)
}

fn poll_origin(world: &World, ship: Entity, id: u64, origin: Origin) -> Result<Status> {
    let mut caller = caller(world, ship)?;
    caller.origin = origin;
    Ok(world
        .get_resource::<RouteService>()
        .map_or(Status::Unknown, |service| service.poll(caller, id)))
}

pub fn ready(world: &World, ship: Entity, id: u64, expected_revision: u64) -> Result<ReadyRoute> {
    ready_origin(world, ship, id, expected_revision, Origin::Explicit)
}

pub fn ready_automatic(
    world: &World,
    ship: Entity,
    id: u64,
    expected_revision: u64,
) -> Result<ReadyRoute> {
    ready_origin(world, ship, id, expected_revision, Origin::Automatic)
}

fn ready_origin(
    world: &World,
    ship: Entity,
    id: u64,
    expected_revision: u64,
    origin: Origin,
) -> Result<ReadyRoute> {
    let mut caller = caller(world, ship)?;
    caller.origin = origin;
    ensure!(
        caller.travel_revision == expected_revision,
        "stale travel revision"
    );
    let service = world
        .get_resource::<RouteService>()
        .context("route service unavailable")?;
    let state = service.0.lock().unwrap();
    let entry = state
        .entries
        .get(&Key {
            scope: caller.scope(),
            request: id,
        })
        .context("route plan unavailable")?;
    ensure!(
        entry.caller.current(caller),
        "route inputs changed; request a new plan"
    );
    let Status::Ready { plan } = &entry.status else {
        anyhow::bail!("route plan is not ready")
    };
    let mut plan = plan.clone();
    let total = plan
        .orders
        .iter()
        .map(|order| order.estimated_propellant_kg)
        .collect::<Option<Vec<_>>>()
        .map(|values| values.into_iter().sum());
    plan.fuel_budget = fuel_budget(world, ship, total);
    let performance = performance::performance(world, ship)?;
    plan.fuel_budget
        .resources
        .push(osg_model::travel::FuelRequirement {
            resource: osg_model::travel::slip::EXOTIC_RESOURCE.to_owned(),
            required_kg: plan.exotic_fuel_kg,
            available_kg: performance.exotic_available_kg,
        });
    ensure!(
        plan.fuel_budget
            .resources
            .iter()
            .all(|resource| resource.required_kg
                <= resource.available_kg * entry.request.preferences.fuel_fraction + 1e-9),
        "Fuel allowance no longer covers this route; request a new preview"
    );
    Ok(ReadyRoute {
        goals: entry.request.orders.clone(),
        plan,
        preferences: entry.request.preferences,
    })
}

pub fn install(app: &mut App) {
    app.init_resource::<RouteService>()
        .init_resource::<Workers>()
        .add_systems(
            FixedUpdate,
            advance
                .after(services::prepare_sources)
                .after(super::hardware::HardwareSystems::Initialize)
                .before(super::simulation::SimulationSystems::PrepareBodies)
                .run_if(in_state(super::GameState::Game)),
        );
}

pub fn reset(world: &mut World) {
    world.init_resource::<RouteService>();
    let service = world.resource::<RouteService>().clone();
    let mut state = service.0.lock().unwrap();
    for entry in state.entries.values() {
        entry.cancel.store(true, Ordering::Relaxed);
    }
    state.entries.clear();
    state.queue.clear();
    state.generation = state
        .generation
        .checked_add(1)
        .expect("route generation exhausted");
}

pub fn advance(world: &mut World) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("route_service.advance");
    world.init_resource::<RouteService>();
    world.init_resource::<Workers>();
    let service = world.resource::<RouteService>().clone();
    let mut workers = std::mem::take(&mut world.resource_mut::<Workers>().0);
    let mut index = 0;
    while index < workers.len() {
        let running = &mut workers[index];
        let current = identity::lookup(world, running.key.scope.ship)
            .ok()
            .and_then(|ship| caller(world, ship).ok())
            .map(|mut caller| {
                caller.origin = running.key.scope.origin;
                caller
            });
        let mut state = service.0.lock().unwrap();
        let stale = running.generation != state.generation
            || state
                .entries
                .get(&running.key)
                .is_none_or(|entry| current.is_none_or(|caller| !entry.caller.current(caller)));
        if stale {
            running.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(result) = check_ready(&mut running.task) {
            if running.generation == state.generation
                && let Some(entry) = state.entries.get_mut(&running.key)
            {
                entry.status = if stale {
                    failed("route inputs changed; request a new plan")
                } else {
                    result
                };
            }
            workers.swap_remove(index);
        } else {
            index += 1;
        }
    }

    let mut admissions = 0;
    while workers.len() < MAX_WORKERS && admissions < MAX_WORKERS * 2 {
        let job = {
            let mut state = service.0.lock().unwrap();
            state.queue.pop_front().and_then(|key| {
                state.entries.get(&key).map(|entry| {
                    (
                        key,
                        state.generation,
                        entry.caller,
                        entry.request.clone(),
                        entry.cancel.clone(),
                    )
                })
            })
        };
        let Some((key, generation, admitted, request, cancel)) = job else {
            break;
        };
        admissions += 1;
        let prepared = prepare(world, admitted, &request, cancel.clone());
        match prepared {
            Ok((input, mut environment)) => {
                {
                    let mut state = service.0.lock().unwrap();
                    let entry = state.entries.get_mut(&key).unwrap();
                    entry.status = pending(PlanningStage::SearchingRoutes);
                }
                let task = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new).spawn(
                    async move {
                        let result = environment
                            .prepare()
                            .and_then(|()| routing::plan(&input, &environment));
                        match result {
                            Ok(result) => {
                                let plan = Plan {
                                    planned_tick: input.tick,
                                    travel_revision: admitted.travel_revision,
                                    topology_revision: admitted.topology_revision,
                                    orders: result.orders,
                                    fuel_budget: result.fuel_budget,
                                    estimated_loss_ppm: result.estimated_loss_ppm,
                                    beacon_assumptions: result.beacon_assumptions,
                                    exotic_fuel_kg: result.exotic_fuel_kg,
                                };
                                if plan.orders.len() > dto::MAX_ORDERS {
                                    failed("expanded route exceeds response limit")
                                } else {
                                    Status::Ready { plan }
                                }
                            }
                            Err(error) => failed(error),
                        }
                    },
                );
                workers.push(Running {
                    key,
                    generation,
                    cancel,
                    task,
                });
            }
            Err(error) => {
                service
                    .0
                    .lock()
                    .unwrap()
                    .entries
                    .get_mut(&key)
                    .unwrap()
                    .status = failed(error)
            }
        }
    }
    world.resource_mut::<Workers>().0 = workers;
}

pub(crate) fn prepare(
    world: &mut World,
    admitted: Caller,
    request: &Request,
    cancel: Arc<AtomicBool>,
) -> Result<(
    routing::RouteRequest,
    services::route_environment::Environment,
)> {
    let ship = identity::lookup(world, admitted.ship)?;
    let mut current = caller(world, ship)?;
    current.origin = admitted.origin;
    ensure!(
        admitted.current(current),
        "route inputs changed before planning"
    );

    let mut input = performance::request(world, ship, request)?;
    if admitted.origin == Origin::Automatic {
        if let Some(travel) = world.get::<travel::Travel>(ship) {
            input.preferences.max_loss_ppm = travel.0.risk_budget.remaining_ppm();
        }
    }
    let environment =
        services::route_environment::Environment::capture(world, ship, cancel, &input)?;
    Ok((input, environment))
}
