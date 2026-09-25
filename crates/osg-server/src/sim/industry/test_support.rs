//! Test clients submit the same typed requests and run the registered schedules.
use super::*;
use crate::blueprint_uploads::BlueprintUploads;
use osg_model::rpc::{GameError, Operation};
use requests::*;
use tokio::sync::oneshot;

pub fn install(world: &mut World) {
    if world.contains_resource::<WorkQueue>() {
        return;
    }
    world.init_resource::<super::super::economy::Economy>();
    world.init_resource::<super::super::gas::GasLedger>();
    world.init_resource::<travel::TravelEvents>();
    world.init_resource::<vessel::WasmRuntime>();
    world.init_resource::<bevy::ecs::schedule::Schedules>();
    let mut app = App::new();
    *app.world_mut() = std::mem::take(world);
    hardware::install_initialization(&mut app);
    app.add_plugins(IndustryPlugin);
    *world = std::mem::take(app.world_mut());
}

pub fn tick(world: &mut World) {
    world.run_schedule(FixedPreUpdate);
    world.run_schedule(FixedUpdate);
}

pub struct TestLane {
    pub capability: IndustryCapability,
    pub device: Entity,
}

pub fn lanes(world: &World, entity: Entity) -> Vec<TestLane> {
    let facility = world.get::<IndustrialFacility>(entity).unwrap();
    let parts = world.get::<hardware::PartDevices>(entity).unwrap();
    facility
        .modules
        .iter()
        .flat_map(|module| {
            module.lanes.iter().map(|_| TestLane {
                capability: module.capability,
                device: parts.0[module.part_index],
            })
        })
        .collect()
}

pub fn cumulative_energy(energy: u64, tick: u64, duration: u64) -> u64 {
    (u128::from(energy) * u128::from(tick) / u128::from(duration)) as u64
}

fn submit<A>(
    world: &mut World,
    account: Id,
    arguments: A,
    uploads: &BlueprintUploads,
    wrap: impl FnOnce(Mutation<A>) -> Incoming,
) -> Result<()> {
    install(world);
    let (reply, mut receive) = oneshot::channel();
    let operation = Operation {
        world: world.resource::<identity::WorldEpoch>().0,
        id: Id::new(),
    };
    let request = wrap(Mutation {
        account,
        operation,
        fingerprint: [0; 32],
        arguments,
        reply,
    });
    crate::rpc::enqueue_industry(world, account, uploads, request);
    world.run_schedule(FixedPreUpdate);
    receive
        .try_recv()?
        .map_err(|error| anyhow::anyhow!(error.0))
}

pub fn enqueue_command(
    world: &mut World,
    account: Id,
    command: IndustryCommand,
    uploads: Option<&BlueprintUploads>,
) -> Result<()> {
    osg_protocol::industry::validate_command(&command)?;
    let uploads = uploads.cloned().unwrap_or_default();
    match command {
        IndustryCommand::StartRecipe {
            facility,
            recipe,
            batches,
        } => submit(
            world,
            account,
            StartWork::Recipe {
                facility,
                recipe,
                batches,
            },
            &uploads,
            Incoming::Work,
        ),
        IndustryCommand::BuildShip {
            facility,
            owner,
            blueprint_hash,
        } => submit(
            world,
            account,
            StartWork::Ship {
                facility,
                owner,
                blueprint_hash,
            },
            &uploads,
            Incoming::Work,
        ),
        IndustryCommand::CancelJob { facility, job } => submit(
            world,
            account,
            CancelWork { facility, job },
            &uploads,
            Incoming::Cancel,
        ),
        IndustryCommand::Transfer {
            source,
            target,
            item,
            quantity,
        } => submit(
            world,
            account,
            CargoAction::Transfer {
                source,
                target,
                item,
                quantity,
            },
            &uploads,
            Incoming::Cargo,
        ),
        IndustryCommand::Refill {
            source,
            ship,
            resource,
            quantity,
        } => submit(
            world,
            account,
            CargoAction::Refill {
                source,
                target: ship,
                resource,
                quantity,
            },
            &uploads,
            Incoming::Cargo,
        ),
        IndustryCommand::UnloadProduct {
            source,
            target,
            resource,
            quantity,
        } => submit(
            world,
            account,
            CargoAction::UnloadProduct {
                source,
                target,
                resource,
                quantity,
            },
            &uploads,
            Incoming::Cargo,
        ),
    }
}

pub fn enqueue_blueprint(
    world: &mut World,
    account: Id,
    facility: Id,
    owner: Principal,
    bytes: &[u8],
) -> Result<()> {
    let uploads = BlueprintUploads::default();
    let hash = *blake3::hash(bytes).as_bytes();
    let mut payload = hash.to_vec();
    payload.extend_from_slice(bytes);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(uploads.receive(&mut payload.as_slice()))?;
    enqueue_command(
        world,
        account,
        IndustryCommand::BuildShip {
            facility,
            owner,
            blueprint_hash: hash,
        },
        Some(&uploads),
    )
}

fn read<A, R>(
    world: &mut World,
    account: Id,
    arguments: A,
    uploads: &BlueprintUploads,
    wrap: impl FnOnce(ReadRequest<A, R>) -> IndustryQueryRequest,
) -> Result<R> {
    install(world);
    let (reply, mut receive) = oneshot::channel::<Result<R, GameError>>();
    let request = wrap(ReadRequest {
        world: world.resource::<identity::WorldEpoch>().0,
        arguments,
        reply,
    });
    crate::rpc::enqueue_industry(world, account, uploads, Incoming::Query(request));
    world.run_schedule(FixedLast);
    receive
        .try_recv()?
        .map_err(|error| anyhow::anyhow!(error.0))
}

pub fn read_facility(world: &mut World, account: Id, facility: Id) -> Result<FacilityView> {
    read(
        world,
        account,
        facility,
        &BlueprintUploads::default(),
        IndustryQueryRequest::Facility,
    )
}

pub fn read_directory(
    world: &mut World,
    account: Id,
    after: Option<Id>,
    limit: u16,
) -> Result<osg_model::rpc::Page<FacilitySummary, Id>> {
    read(
        world,
        account,
        (after, limit),
        &BlueprintUploads::default(),
        IndustryQueryRequest::Directory,
    )
}

pub fn read_hangar(
    world: &mut World,
    account: Id,
    ship: Id,
    after: Option<Id>,
) -> Option<HangarView> {
    read(
        world,
        account,
        (ship, after),
        &BlueprintUploads::default(),
        IndustryQueryRequest::Hangar,
    )
    .ok()
}

pub mod service_client {
    use super::*;
    pub fn publish(
        world: &mut World,
        account: Id,
        facility: Id,
        policy: ServicePolicy,
    ) -> Result<()> {
        submit(
            world,
            account,
            ConfigureService { facility, policy },
            &BlueprintUploads::default(),
            Incoming::Configure,
        )
    }
    pub fn order(
        world: &mut World,
        account: Id,
        quote: ServiceQuote,
        uploads: &BlueprintUploads,
    ) -> Result<()> {
        submit(
            world,
            account,
            StartWork::Public(quote),
            uploads,
            Incoming::Work,
        )
    }
    pub fn cancel(world: &mut World, account: Id, facility: Id, job: Id) -> Result<()> {
        submit(
            world,
            account,
            CancelWork { facility, job },
            &BlueprintUploads::default(),
            Incoming::Cancel,
        )
    }
    pub fn quote(
        world: &mut World,
        account: Id,
        facility: Id,
        payer: Principal,
        work: ServiceWork,
        uploads: &BlueprintUploads,
    ) -> Result<ServiceQuote> {
        read(
            world,
            account,
            (facility, payer, work),
            uploads,
            IndustryQueryRequest::Quote,
        )
    }
    pub fn jobs(world: &mut World, account: Id, facility: Id) -> Result<Vec<JobView>> {
        read(
            world,
            account,
            facility,
            &BlueprintUploads::default(),
            IndustryQueryRequest::Jobs,
        )
    }
    pub fn list(
        world: &mut World,
        account: Id,
        search: String,
        after: Option<Id>,
        limit: u16,
    ) -> Result<osg_model::rpc::Page<PublicFacility, Id>> {
        read(
            world,
            account,
            (search, after, limit),
            &BlueprintUploads::default(),
            IndustryQueryRequest::PublicFacilities,
        )
    }
}
