use super::*;
use crate::sim::{
    physics::{MassProps, Velocity},
    precision::PreciseTransform,
    simulation::SimulationCounters,
};
use bevy::{ecs::system::RunSystemOnce, math::DVec3};
use osg_model::{
    ownership::{AccessGrant, AccessPolicy},
    travel::Presence,
};
use osg_ships::ShipBlueprint;
use std::sync::Arc;

struct Fixture {
    world: World,
    facility: Entity,
    account: Id,
}

impl Fixture {
    fn new() -> Self {
        let mut world = World::new();
        let account = Id::new();
        identity::initialize(&mut world, &[account]);
        world.init_resource::<SimulationCounters>();
        world.init_resource::<vessel::WasmRuntime>();
        world.insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION));
        world.insert_resource(vessel::ShipCatalogue(Catalogue::builtin()));
        let blueprint = ShipBlueprint::from_bytes(include_bytes!(
            "../../../../../assets/ships/neris-anchorage.ship"
        ))
        .unwrap();
        let facility = Self::spawn(&mut world, account, blueprint, DVec3::ZERO);
        world
            .entity_mut(facility)
            .insert(IndustrialFacility::default());
        for bay in &mut world.get_mut::<travel::DockingBays>(facility).unwrap().0 {
            bay.public = true;
        }
        install(&mut world);
        world.run_schedule(FixedPreUpdate);
        Self {
            world,
            facility,
            account,
        }
    }

    fn spawn(world: &mut World, owner: Id, blueprint: ShipBlueprint, position: DVec3) -> Entity {
        let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
        let design = Arc::new(blueprint.compile(catalogue).unwrap());
        let entity = vessel::spawn_ship(
            world,
            design,
            PreciseTransform {
                translation_um: osg_model::GalacticPosition::from_meters(position),
                ..Default::default()
            },
            DVec3::ZERO,
            "Industry acceptance fixture".into(),
        )
        .unwrap();
        identity::attach_ship(world, entity, owner, Id::new()).unwrap();
        world.run_system_once(hardware::initialize).unwrap();
        entity
    }

    fn guest(&mut self, owner: Id) -> Entity {
        let guest = Self::spawn(
            &mut self.world,
            owner,
            osg_ships::expedition_patrol(),
            DVec3::X * 200.0,
        );
        travel::dock(&mut self.world, guest, self.facility, 0).unwrap();
        guest
    }

    fn id(&self, entity: Entity) -> Id {
        self.world.get::<identity::Identity>(entity).unwrap().0
    }

    fn put(&mut self, entity: Entity, stacks: &[ItemStack]) {
        let catalogue = self.world.resource::<vessel::ShipCatalogue>().0.clone();
        let capacity = self
            .world
            .get::<vessel::ShipDesign>(entity)
            .unwrap()
            .0
            .capacity_m3;
        let mut inventory = self
            .world
            .get_mut::<hardware::ShipInventory>(entity)
            .unwrap();
        for stack in stacks {
            inventory
                .0
                .insert_item(&stack.item, stack.quantity, capacity, &catalogue)
                .unwrap();
        }
        hardware::synchronize_mass(&mut self.world, &[entity]);
    }

    fn inventory(&self, entity: Entity) -> &Inventory {
        &self.world.get::<hardware::ShipInventory>(entity).unwrap().0
    }

    fn inventory_bytes(&self, entity: Entity) -> Vec<u8> {
        postcard::to_stdvec(self.inventory(entity)).unwrap()
    }

    fn quantity(&self, entity: Entity, item: &CargoItem) -> u64 {
        self.inventory(entity)
            .cargo_quantity(item, &self.world.resource::<vessel::ShipCatalogue>().0)
            .unwrap()
    }

    fn recipe(&self, capability: IndustryCapability) -> Recipe {
        self.world
            .resource::<ManufacturingCatalogue>()
            .0
            .recipes
            .iter()
            .find(|recipe| recipe.capability == capability)
            .unwrap()
            .clone()
    }

    fn start(&mut self, recipe: &Recipe) -> Id {
        let facility = self.id(self.facility);
        enqueue_command(
            &mut self.world,
            self.account,
            IndustryCommand::StartRecipe {
                facility,
                recipe: recipe.id.clone(),
                batches: 1,
            },
            None,
        )
        .unwrap();
        self.world
            .get::<IndustrialFacility>(self.facility)
            .unwrap()
            .jobs
            .last()
            .unwrap()
            .id
    }

    fn grant(&mut self, entity: Entity, account: Id, permissions: &[Permission]) {
        identity::add_account(&mut self.world, account, false);
        self.world
            .entity_mut(entity)
            .insert(ownership::AssetAccess(AccessPolicy {
                public: BTreeSet::new(),
                grants: vec![AccessGrant {
                    principal: Principal::Player(account),
                    permissions: permissions.iter().copied().collect(),
                }],
            }));
    }
}

#[test]
fn work_queue_is_fifo_and_rejects_stale_or_abandoned_requests() {
    use requests::*;

    let mut fixture = Fixture::new();
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    fixture.put(fixture.facility, &recipe.inputs);
    let facility = fixture.id(fixture.facility);
    let epoch = fixture.world.resource::<identity::WorldEpoch>().0;
    let mut enqueue = |world| {
        let (reply, receive) = tokio::sync::oneshot::channel();
        fixture
            .world
            .resource_mut::<WorkQueue>()
            .0
            .push_back(WorkItem {
                request: Mutation {
                    account: fixture.account,
                    world,
                    arguments: StartWork::Recipe {
                        facility,
                        recipe: recipe.id.clone(),
                        batches: 1,
                    },
                    reply,
                },
                uploads: Default::default(),
            });
        receive
    };
    let mut first = enqueue(epoch);
    let mut second = enqueue(epoch);
    let mut stale = enqueue(Id::new());
    let abandoned = enqueue(epoch);
    drop(abandoned);

    fixture.world.run_schedule(FixedPreUpdate);
    assert!(first.try_recv().unwrap().is_ok());
    assert!(second.try_recv().unwrap().is_err());
    assert!(
        stale
            .try_recv()
            .unwrap()
            .unwrap_err()
            .0
            .contains("World changed")
    );
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .len(),
        1
    );
    assert_eq!(
        fixture.inventory(fixture.facility).reservations,
        osg_ships::aggregate_stacks(
            &recipe.inputs,
            &fixture.world.resource::<vessel::ShipCatalogue>().0
        )
        .unwrap()
    );
}

#[test]
fn cancellation_phase_releases_materials_before_new_work_is_admitted() {
    use requests::*;

    let mut fixture = Fixture::new();
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    fixture.put(fixture.facility, &recipe.inputs);
    let job = fixture.start(&recipe);
    let facility = fixture.id(fixture.facility);
    let epoch = fixture.world.resource::<identity::WorldEpoch>().0;
    let (reply, mut start) = tokio::sync::oneshot::channel();
    fixture
        .world
        .resource_mut::<WorkQueue>()
        .0
        .push_back(WorkItem {
            request: Mutation {
                account: fixture.account,
                world: epoch,
                arguments: StartWork::Recipe {
                    facility,
                    recipe: recipe.id,
                    batches: 1,
                },
                reply,
            },
            uploads: Default::default(),
        });
    let (reply, mut cancel) = tokio::sync::oneshot::channel();
    fixture
        .world
        .resource_mut::<CancellationQueue>()
        .0
        .push_back(Mutation {
            account: fixture.account,
            world: epoch,
            arguments: CancelWork { facility, job },
            reply,
        });

    fixture.world.run_schedule(FixedPreUpdate);
    assert!(cancel.try_recv().unwrap().is_ok());
    assert!(start.try_recv().unwrap().is_ok());
    let jobs = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs;
    assert_eq!(jobs.len(), 1);
    assert_ne!(jobs[0].id, job);
}

#[test]
fn public_service_reserves_customer_cargo_and_money_then_charges_once() {
    use crate::sim::infrastructure::Landmark;
    use osg_model::{
        economy::{Currency, MONEY_SCALE},
        industry::{CustomerMatch, CustomerTier, ServicePolicy, ServiceRate, ServiceWork},
    };

    let mut fixture = Fixture::new();
    let customer = Id::new();
    identity::add_account(&mut fixture.world, customer, false);
    let facility = fixture.id(fixture.facility);
    fixture.world.entity_mut(fixture.facility).insert(Landmark {
        system: Id::new(),
        name: "Public works".into(),
    });
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    fixture.put(fixture.facility, &recipe.inputs);
    let payer = Principal::Player(customer);
    let operator = Principal::Player(fixture.account);
    for input in &recipe.inputs {
        fixture
            .world
            .get_mut::<hardware::ShipInventory>(fixture.facility)
            .unwrap()
            .0
            .custody
            .insert(input.item.clone(), input.quantity);
        fixture
            .world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .storage
            .set_item((facility, payer), input.item.clone(), input.quantity);
    }
    let now = osg_model::calendar::now_unix_ms();
    fixture
        .world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(payer, Currency::Uec, 1_000_000 * MONEY_SCALE, now)
        .unwrap();
    let rate = ServiceRate {
        capability: recipe.capability,
        energy_per_mj: MONEY_SCALE,
        time_per_hour: MONEY_SCALE,
        public_lanes: 1,
    };
    service_client::publish(
        &mut fixture.world,
        fixture.account,
        facility,
        ServicePolicy {
            revision: 0,
            accepting: true,
            currency: Currency::Uec,
            rates: vec![rate],
            tiers: vec![CustomerTier {
                customer: CustomerMatch::Principal(payer),
                price_basis_points: Some(8_000),
            }],
        },
    )
    .unwrap();
    let uploads = crate::blueprint_uploads::BlueprintUploads::default();
    let work = ServiceWork::Recipe {
        recipe: recipe.id,
        batches: 1,
    };
    let public =
        service_client::list(&mut fixture.world, customer, String::new(), None, 32).unwrap();
    assert_eq!(public.items.len(), 1);
    let quote = service_client::quote(
        &mut fixture.world,
        customer,
        facility,
        payer,
        work,
        &uploads,
    )
    .unwrap();
    assert_eq!(quote.price_basis_points, 8_000);
    assert!(quote.total > 0);
    let mut stale = quote.clone();
    stale.policy_revision += 1;
    assert!(service_client::order(&mut fixture.world, customer, stale, &uploads).is_err());
    let before = (&fixture
        .world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .available(payer, Currency::Uec);
    service_client::order(&mut fixture.world, customer, quote.clone(), &uploads).unwrap();
    let first = service_client::jobs(&mut fixture.world, customer, facility).unwrap()[0].id;
    assert_eq!(
        (&fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .available(payer, Currency::Uec),
        before - quote.total
    );
    assert!(service_client::order(&mut fixture.world, customer, quote.clone(), &uploads).is_err());
    assert!(service_client::cancel(&mut fixture.world, fixture.account, facility, first).is_ok());
    assert_eq!(
        (&fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .available(payer, Currency::Uec),
        before
    );
    for input in &quote.inputs {
        assert_eq!(
            (&fixture
                .world
                .resource::<crate::sim::society::SocietyState>()
                .economy)
                .storage[&(facility, payer)][&input.item],
            input.quantity
        );
    }
    service_client::order(&mut fixture.world, customer, quote.clone(), &uploads).unwrap();
    let previous_presence = fixture
        .world
        .get::<travel::PresenceState>(fixture.facility)
        .unwrap()
        .0
        .clone();
    fixture
        .world
        .get_mut::<travel::PresenceState>(fixture.facility)
        .unwrap()
        .0 = osg_model::travel::Presence::Destroyed;
    tick(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs()
            .is_empty()
    );
    assert_eq!(
        fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .reserved(payer, Currency::Uec),
        0
    );
    assert_eq!(
        fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .available(payer, Currency::Uec),
        before
    );
    fixture
        .world
        .get_mut::<travel::PresenceState>(fixture.facility)
        .unwrap()
        .0 = previous_presence;
    service_client::order(&mut fixture.world, customer, quote.clone(), &uploads).unwrap();
    let second = service_client::jobs(&mut fixture.world, customer, facility).unwrap()[0].id;
    // Fail after calculating the service transfer, when recording its revenue.
    // The production schedule must leave both payment and physical work untouched.
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .outside_revenue
        .uec = u64::MAX;
    let money_before = postcard::to_stdvec(
        &fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy,
    )
    .unwrap();
    let inventory_before = fixture.inventory_bytes(fixture.facility);
    tick(&mut fixture.world);
    assert_eq!(
        postcard::to_stdvec(
            &fixture
                .world
                .resource::<crate::sim::society::SocietyState>()
                .economy
        )
        .unwrap(),
        money_before
    );
    assert_eq!(fixture.inventory_bytes(fixture.facility), inventory_before);
    let blocked = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .job(second)
        .unwrap();
    assert_eq!(blocked.progress_ticks, 0);
    assert!(!blocked.payment.as_ref().unwrap().charged);
    assert_eq!(blocked.status, JobStatus::AwaitingPayment);
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .outside_revenue
        .uec = 0;

    let before_operator = (&fixture
        .world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .balances
        .get(&operator)
        .cloned()
        .unwrap_or_default();
    tick(&mut fixture.world);
    let job = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0];
    assert!(job.payment.as_ref().unwrap().charged);
    assert_eq!(
        fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .reserved(payer, Currency::Uec),
        0
    );
    assert!(
        (&fixture
            .world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .balances[&operator]
            .uec
            > 0
    );
    let received_uec = (&fixture
        .world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .balances[&operator]
        .uec
        - before_operator.uec;
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .outside_revenue
            .uec,
        received_uec
    );
    assert!(service_client::cancel(&mut fixture.world, customer, facility, second).is_err());

    for _ in 1..quote.duration_ticks {
        tick(&mut fixture.world);
    }
    assert!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .is_empty()
    );
    let completed = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap();
    assert_eq!(completed.outside_revenue.uec, received_uec);
    assert_eq!(completed.outside_revenue.lat, 0);
    let saved = postcard::to_allocvec(&completed.to_record()).unwrap();
    let restored: IndustrialFacilityRecord = postcard::from_bytes(&saved).unwrap();
    assert_eq!(restored.outside_revenue.uec, received_uec);
    for output in &quote.outputs {
        assert_eq!(
            (&fixture
                .world
                .resource::<crate::sim::society::SocietyState>()
                .economy)
                .storage[&(facility, payer)][&output.item],
            output.quantity
        );
        assert_eq!(
            fixture
                .world
                .get::<hardware::ShipInventory>(fixture.facility)
                .unwrap()
                .0
                .custody[&output.item],
            output.quantity
        );
    }
}

async fn stage_blueprint(
    uploads: &crate::blueprint_uploads::BlueprintUploads,
    bytes: &[u8],
) -> [u8; 32] {
    let hash = *blake3::hash(bytes).as_bytes();
    let input = [hash.as_slice(), bytes].concat();
    assert_eq!(uploads.receive(&mut input.as_slice()).await.unwrap(), hash);
    hash
}

#[tokio::test]
async fn uploaded_construction_rechecks_private_scope_authority_and_firmware_before_reserving() {
    let mut fixture = Fixture::new();
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut blueprint = osg_ships::industry::starter_ship();
    blueprint.firmware = osg_ships::Firmware::Custom(osg_ships::EXAMPLE_CONTROLLER.to_vec());
    let requirements = osg_ships::industry::construction_requirements(
        &blueprint.compile(&catalogue).unwrap(),
        &catalogue,
    )
    .unwrap();
    fixture.put(fixture.facility, &requirements.inputs);
    let bytes = blueprint.to_bytes().unwrap();
    assert!(bytes.len() > 48 * 1024);
    let uploads = crate::blueprint_uploads::BlueprintUploads::default();
    let hash = stage_blueprint(&uploads, &bytes).await;
    let other_uploads = crate::blueprint_uploads::BlueprintUploads::default();
    let facility = fixture.id(fixture.facility);
    let command = IndustryCommand::BuildShip {
        facility,
        owner: Principal::Player(fixture.account),
        blueprint_hash: hash,
    };
    let before = fixture.inventory_bytes(fixture.facility);
    for source in [None, Some(&other_uploads)] {
        assert!(
            enqueue_command(&mut fixture.world, fixture.account, command.clone(), source).is_err()
        );
        assert_eq!(fixture.inventory_bytes(fixture.facility), before);
        assert!(
            fixture
                .world
                .get::<IndustrialFacility>(fixture.facility)
                .unwrap()
                .jobs
                .is_empty()
        );
    }

    let stranger = Id::new();
    identity::add_account(&mut fixture.world, stranger, false);
    assert!(
        enqueue_command(
            &mut fixture.world,
            stranger,
            command.clone(),
            Some(&uploads)
        )
        .is_err()
    );
    assert_eq!(fixture.inventory_bytes(fixture.facility), before);

    for invalid in [b"not a ship".to_vec(), {
        let mut invalid = blueprint.clone();
        invalid.firmware = osg_ships::Firmware::Custom(b"not wasm".to_vec());
        invalid.to_bytes().unwrap()
    }] {
        let hash = stage_blueprint(&uploads, &invalid).await;
        assert!(
            enqueue_command(
                &mut fixture.world,
                fixture.account,
                IndustryCommand::BuildShip {
                    facility,
                    owner: Principal::Player(fixture.account),
                    blueprint_hash: hash,
                },
                Some(&uploads),
            )
            .is_err()
        );
        assert_eq!(fixture.inventory_bytes(fixture.facility), before);
        assert!(
            fixture
                .world
                .get::<IndustrialFacility>(fixture.facility)
                .unwrap()
                .jobs
                .is_empty()
        );
    }

    enqueue_command(&mut fixture.world, fixture.account, command, Some(&uploads)).unwrap();
    let queue = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap();
    assert_eq!(queue.jobs.len(), 1);
    assert!(
        matches!(&queue.jobs[0].work.output, WorkOutput::Ship(saved) if saved.as_ref() == bytes)
    );
    assert!(!fixture.inventory(fixture.facility).reservations.is_empty());
}

fn large_custom_program() -> Vec<u8> {
    let mut program = wat::parse_str(format!(
        "(module (memory (export \"memory\") 1) \
         (func (export \"game_version\") (result i32) i32.const {}) \
         (func (export \"ship_tick\")))",
        osg_ship_api::GAME_VERSION as u32,
    ))
    .unwrap();
    let payload_len = 1024 * 1024 - 1024;
    program.push(0);
    let mut length = payload_len;
    loop {
        let byte = (length & 0x7f) as u8;
        length >>= 7;
        program.push(byte | if length > 0 { 0x80 } else { 0 });
        if length == 0 {
            break;
        }
    }
    program.push(0);
    program.resize(program.len() + payload_len - 1, 0);
    assert!(program.len() <= 1024 * 1024);
    program
}

#[test]
fn queued_custom_blueprints_have_an_atomic_facility_byte_limit() {
    let mut fixture = Fixture::new();
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut blueprint = osg_ships::industry::starter_ship();
    blueprint.firmware = osg_ships::Firmware::Custom(large_custom_program());
    let bytes = blueprint.to_bytes().unwrap();
    let count = MAX_QUEUED_BLUEPRINT_BYTES / bytes.len();
    assert!(count > 0 && count < MAX_JOBS);
    let requirements = osg_ships::industry::construction_requirements(
        &blueprint.compile(&catalogue).unwrap(),
        &catalogue,
    )
    .unwrap();
    let mut stock = requirements.inputs.clone();
    for stack in &mut stock {
        stack.quantity *= count as u64 + 1;
    }
    fixture.put(fixture.facility, &stock);
    let facility = fixture.id(fixture.facility);
    for _ in 0..count {
        enqueue_blueprint(
            &mut fixture.world,
            fixture.account,
            facility,
            Principal::Player(fixture.account),
            &bytes,
        )
        .unwrap();
    }
    let before = fixture.inventory_bytes(fixture.facility);
    let error = enqueue_blueprint(
        &mut fixture.world,
        fixture.account,
        facility,
        Principal::Player(fixture.account),
        &bytes,
    )
    .unwrap_err();
    assert!(error.to_string().contains("64 MiB"));
    assert_eq!(fixture.inventory_bytes(fixture.facility), before);
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .len(),
        count
    );

    let job = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0]
        .id;
    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::CancelJob { facility, job },
        None,
    )
    .unwrap();
    enqueue_blueprint(
        &mut fixture.world,
        fixture.account,
        facility,
        Principal::Player(fixture.account),
        &bytes,
    )
    .unwrap();
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .len(),
        count
    );
}

#[test]
fn remote_management_does_not_grant_material_transfer_or_private_inventory_access() {
    let mut fixture = Fixture::new();
    let operator = Id::new();
    let outsider = Id::new();
    identity::add_account(&mut fixture.world, outsider, false);
    fixture.grant(fixture.facility, operator, &[Permission::Industry]);
    let remote = Fixture::spawn(
        &mut fixture.world,
        operator,
        osg_ships::industry::starter_ship(),
        DVec3::X * 1e12,
    );
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    fixture.put(fixture.facility, &recipe.inputs);
    let facility_id = fixture.id(fixture.facility);
    enqueue_command(
        &mut fixture.world,
        operator,
        IndustryCommand::StartRecipe {
            facility: facility_id,
            recipe: recipe.id,
            batches: 1,
        },
        None,
    )
    .unwrap();
    let job = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0]
        .id;
    let before = fixture.inventory_bytes(fixture.facility);
    assert!(
        enqueue_command(
            &mut fixture.world,
            outsider,
            IndustryCommand::CancelJob {
                facility: facility_id,
                job
            },
            None,
        )
        .is_err()
    );
    assert_eq!(fixture.inventory_bytes(fixture.facility), before);
    assert!(read_facility(&mut fixture.world, outsider, facility_id).is_err());
    assert!(
        read_directory(&mut fixture.world, outsider, None, 128)
            .unwrap()
            .items
            .is_empty()
    );
    let visible = read_facility(&mut fixture.world, operator, facility_id).unwrap();
    assert!(visible.can_manage && !visible.can_transfer);
    enqueue_command(
        &mut fixture.world,
        operator,
        IndustryCommand::CancelJob {
            facility: facility_id,
            job,
        },
        None,
    )
    .unwrap();
    fixture.grant(
        fixture.facility,
        operator,
        &[Permission::Industry, Permission::TransferCargo],
    );
    let remote_id = fixture.id(remote);
    let command = IndustryCommand::Transfer {
        source: facility_id,
        target: remote_id,
        item: recipe.inputs[0].item.clone(),
        quantity: 1,
    };
    let before = fixture.inventory_bytes(fixture.facility);
    assert!(enqueue_command(&mut fixture.world, operator, command, None,).is_err());
    assert_eq!(fixture.inventory_bytes(fixture.facility), before);
    fixture
        .world
        .get_mut::<PreciseTransform>(remote)
        .unwrap()
        .translation_um = osg_model::GalacticPosition::from_meters(DVec3::X * 200.0);
    travel::dock(&mut fixture.world, remote, fixture.facility, 0).unwrap();
    enqueue_command(
        &mut fixture.world,
        operator,
        IndustryCommand::Transfer {
            source: facility_id,
            target: remote_id,
            item: recipe.inputs[0].item.clone(),
            quantity: 1,
        },
        None,
    )
    .unwrap();
    assert_eq!(fixture.quantity(remote, &recipe.inputs[0].item), 1);
}

#[test]
fn industry_grant_cannot_convert_an_organizations_materials_into_personal_ships() {
    let mut fixture = Fixture::new();
    let operator = Id::new();
    let stranger = Id::new();
    identity::add_account(&mut fixture.world, stranger, false);
    let organization = ownership::organization_id("Unifleet Station Services");
    let facility_owner = Principal::Organization(organization);
    fixture
        .world
        .entity_mut(fixture.facility)
        .insert(ownership::AssetOwner(facility_owner));
    fixture.grant(fixture.facility, operator, &[Permission::Industry]);
    let blueprint = osg_ships::industry::starter_ship();
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let requirements = osg_ships::industry::construction_requirements(
        &blueprint.compile(&catalogue).unwrap(),
        &catalogue,
    )
    .unwrap();
    fixture.put(fixture.facility, &requirements.inputs);
    let facility = fixture.id(fixture.facility);
    let bytes = blueprint.to_bytes().unwrap();
    let before = fixture.inventory_bytes(fixture.facility);
    for owner in [Principal::Player(operator), Principal::Player(stranger)] {
        assert!(
            enqueue_blueprint(
                &mut fixture.world,
                operator,
                facility,
                owner,
                &(bytes.clone()),
            )
            .is_err()
        );
        assert_eq!(fixture.inventory_bytes(fixture.facility), before);
    }
    enqueue_blueprint(
        &mut fixture.world,
        operator,
        facility,
        facility_owner,
        &(bytes.clone()),
    )
    .unwrap();
    let job = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0];
    assert_eq!(job.owner, facility_owner);
    let job_id = job.id;
    let kit = job
        .work
        .inputs
        .iter()
        .find(|stack| matches!(stack.item, CargoItem::Part(_)))
        .unwrap()
        .clone();
    let guest = fixture.guest(fixture.account);
    let guest_id = fixture.id(guest);
    fixture
        .world
        .get_mut::<ownership::AssetAccess>(fixture.facility)
        .unwrap()
        .0
        .grants
        .push(AccessGrant {
            principal: Principal::Player(fixture.account),
            permissions: [Permission::TransferCargo].into(),
        });
    ownership::authorize(
        &fixture.world,
        fixture.account,
        fixture.facility,
        Permission::TransferCargo,
    )
    .unwrap();
    assert!(
        osg_ships::industry::item_volume_m3(&kit.item, &catalogue).unwrap()
            <= fixture
                .world
                .get::<vessel::ShipDesign>(guest)
                .unwrap()
                .0
                .capacity_m3
    );
    assert!(
        enqueue_command(
            &mut fixture.world,
            fixture.account,
            IndustryCommand::Transfer {
                source: facility,
                target: guest_id,
                item: kit.item,
                quantity: 1
            },
            None,
        )
        .is_err()
    );
    enqueue_command(
        &mut fixture.world,
        operator,
        IndustryCommand::CancelJob {
            facility,
            job: job_id,
        },
        None,
    )
    .unwrap();
    fixture.grant(
        fixture.facility,
        operator,
        &[Permission::Industry, Permission::TransferCargo],
    );
    assert!(
        enqueue_blueprint(
            &mut fixture.world,
            operator,
            facility,
            Principal::Player(stranger),
            &(bytes.clone()),
        )
        .is_err()
    );
    enqueue_blueprint(
        &mut fixture.world,
        operator,
        facility,
        Principal::Player(operator),
        &(bytes),
    )
    .unwrap();
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs[0]
            .owner,
        Principal::Player(operator)
    );
}

#[test]
fn a_full_hold_can_cancel_reserved_work_and_completion_retries_without_duplication() {
    let mut fixture = Fixture::new();
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let recipe = fixture
        .world
        .resource::<ManufacturingCatalogue>()
        .0
        .recipes
        .iter()
        .find(|recipe| {
            let volume = |items: &[ItemStack]| {
                items
                    .iter()
                    .map(|stack| {
                        osg_ships::industry::item_volume_m3(&stack.item, &catalogue).unwrap()
                            * stack.quantity as f64
                    })
                    .sum::<f64>()
            };
            volume(&recipe.outputs) > volume(&recipe.inputs) * 1.01
        })
        .unwrap()
        .clone();
    fixture.put(fixture.facility, &recipe.inputs);
    let starting_quantity: Vec<_> = recipe
        .outputs
        .iter()
        .map(|stack| fixture.quantity(fixture.facility, &stack.item))
        .collect();
    let job = fixture.start(&recipe);
    let input_volume = fixture.inventory(fixture.facility).cargo_volume(&catalogue);
    let original_design = fixture
        .world
        .get::<vessel::ShipDesign>(fixture.facility)
        .unwrap()
        .0
        .clone();
    let mut cramped = (*original_design).clone();
    cramped.capacity_m3 = input_volume;
    fixture
        .world
        .get_mut::<vessel::ShipDesign>(fixture.facility)
        .unwrap()
        .0 = Arc::new(cramped);
    let total_energy = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0]
        .work
        .energy_j;
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = total_energy;
    for _ in 0..recipe.duration_ticks {
        tick(&mut fixture.world);
    }
    let queue = fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap();
    assert_eq!(queue.jobs.len(), 1);
    assert_eq!(queue.jobs[0].progress_ticks, recipe.duration_ticks);
    assert_eq!(queue.jobs[0].status, JobStatus::AwaitingCargoSpace);
    assert_eq!(fixture.inventory(fixture.facility).energy_j, 0);
    let blocked = fixture.inventory_bytes(fixture.facility);
    for _ in 0..10 {
        tick(&mut fixture.world);
        assert_eq!(fixture.inventory_bytes(fixture.facility), blocked);
    }
    let facility = fixture.id(fixture.facility);
    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::CancelJob { facility, job },
        None,
    )
    .unwrap();
    assert!(fixture.inventory(fixture.facility).reservations.is_empty());
    for input in &recipe.inputs {
        assert_eq!(
            fixture.quantity(fixture.facility, &input.item),
            input.quantity
        );
    }
    fixture
        .world
        .get_mut::<vessel::ShipDesign>(fixture.facility)
        .unwrap()
        .0 = original_design;
    fixture.start(&recipe);
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = total_energy;
    for _ in 0..recipe.duration_ticks {
        tick(&mut fixture.world);
    }
    assert!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .is_empty()
    );
    for (index, output) in recipe.outputs.iter().enumerate() {
        assert_eq!(
            fixture.quantity(fixture.facility, &output.item),
            starting_quantity[index] + output.quantity
        );
    }
    let completed = fixture.inventory_bytes(fixture.facility);
    for _ in 0..10 {
        tick(&mut fixture.world);
    }
    assert_eq!(fixture.inventory_bytes(fixture.facility), completed);
}

#[test]
fn blocked_ship_construction_retries_atomically_and_spawns_a_cold_mass_paid_hull() {
    let mut fixture = Fixture::new();
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut blueprint = osg_ships::industry::starter_ship();
    for part in &mut blueprint.parts {
        for tank in &mut part.tanks {
            tank.initial_fill = 1.0;
        }
    }
    let design = blueprint.compile(&catalogue).unwrap();
    let requirements = osg_ships::industry::construction_requirements(&design, &catalogue).unwrap();
    let paid_mass = osg_ships::industry::stack_mass_mg(&requirements.inputs, &catalogue).unwrap();
    assert_eq!(
        paid_mass,
        u128::from(osg_ships::industry::mass_mg(design.dry_mass).unwrap())
    );
    assert!(
        design.dry_mass
            > design
                .parts
                .iter()
                .map(|part| part.definition.mass_kg)
                .sum::<f64>()
                + 71.0
    );
    fixture.put(fixture.facility, &requirements.inputs);
    let mass_before = fixture
        .world
        .get::<MassProps>(fixture.facility)
        .unwrap()
        .mass;
    let facility = fixture.id(fixture.facility);
    enqueue_blueprint(
        &mut fixture.world,
        fixture.account,
        facility,
        Principal::Player(fixture.account),
        &(blueprint.to_bytes().unwrap()),
    )
    .unwrap();
    let inbound_reservation = Some((Id::new(), u64::MAX));
    let bay_radii: Vec<_> = fixture
        .world
        .get::<travel::DockingBays>(fixture.facility)
        .unwrap()
        .0
        .iter()
        .map(|bay| bay.radius_m)
        .collect();
    for bay in &mut fixture
        .world
        .get_mut::<travel::DockingBays>(fixture.facility)
        .unwrap()
        .0
    {
        bay.radius_m = design.radius * 0.5;
        bay.reservation = inbound_reservation;
    }
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = requirements.energy_j;
    for _ in 0..requirements.duration_ticks {
        tick(&mut fixture.world);
    }
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs[0]
            .status,
        JobStatus::AwaitingBerth
    );
    let identities_before = fixture
        .world
        .resource::<identity::IdentityIndex>()
        .entries()
        .len();
    let entities_before = fixture.world.entities().len();
    let inventory_before = fixture.inventory_bytes(fixture.facility);
    for _ in 0..8 {
        tick(&mut fixture.world);
        assert_eq!(
            fixture
                .world
                .resource::<identity::IdentityIndex>()
                .entries()
                .len(),
            identities_before
        );
        assert_eq!(fixture.world.entities().len(), entities_before);
        assert_eq!(fixture.inventory_bytes(fixture.facility), inventory_before);
    }
    for (bay, radius) in fixture
        .world
        .get_mut::<travel::DockingBays>(fixture.facility)
        .unwrap()
        .0
        .iter_mut()
        .zip(bay_radii)
    {
        assert_eq!(bay.reservation, inbound_reservation);
        bay.radius_m = radius;
    }
    tick(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<travel::DockingBays>(fixture.facility)
            .unwrap()
            .0
            .iter()
            .all(|bay| bay.reservation == inbound_reservation)
    );
    assert!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs
            .is_empty()
    );
    let contained: Vec<_> = fixture
        .world
        .get::<travel::StoredShips>(fixture.facility)
        .unwrap()
        .iter()
        .collect();
    assert_eq!(contained.len(), 1);
    let built = contained[0];
    assert!(
        fixture
            .world
            .get::<hardware::PendingHardwareReset>(built)
            .is_some()
    );
    assert!(
        fixture
            .world
            .get::<hardware::PartDevices>(built)
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(
        fixture.world.get::<ownership::AssetOwner>(built).unwrap().0,
        Principal::Player(fixture.account)
    );
    assert!(
        matches!(fixture.world.get::<travel::PresenceState>(built).unwrap().0, Presence::Docked { host, .. } if host == facility)
    );
    assert!(fixture.world.get::<travel::Dormant>(built).is_some());
    assert!(fixture.world.get::<Velocity>(built).is_none());
    let inventory = fixture.inventory(built);
    assert!(inventory.quantities.iter().all(|&quantity| quantity == 0));
    assert!(inventory.cargo.iter().all(|&quantity| quantity == 0));
    assert_eq!(inventory.energy_j, 0);
    let thermal = &fixture.world.get::<hardware::ShipThermal>(built).unwrap().0;
    assert_eq!(thermal.shield_deployed_kg, 0.0);
    assert_eq!(thermal.shield_reserve_mg, 0);
    let mass = fixture.world.get::<MassProps>(built).unwrap().mass;
    assert!((mass - design.dry_mass).abs() < 1e-6);
    assert!(
        (fixture
            .world
            .get::<MassProps>(fixture.facility)
            .unwrap()
            .mass
            - mass_before)
            .abs()
            < 1e-5
    );
    assert!(
        (fixture
            .world
            .get::<travel::StoredMass>(fixture.facility)
            .unwrap()
            .0
            - mass)
            .abs()
            < 1e-6
    );
    let after = fixture.inventory_bytes(fixture.facility);
    tick(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<hardware::PendingHardwareReset>(built)
            .is_none()
    );
    assert_eq!(
        fixture
            .world
            .get::<hardware::PartDevices>(built)
            .unwrap()
            .0
            .len(),
        design.parts.len()
    );
    assert_eq!(fixture.inventory_bytes(fixture.facility), after);
    assert_eq!(fixture.inventory(built).energy_j, 0);
    for _ in 0..8 {
        tick(&mut fixture.world);
    }
    assert_eq!(fixture.inventory_bytes(fixture.facility), after);
    assert_eq!(
        fixture
            .world
            .get::<travel::StoredShips>(fixture.facility)
            .unwrap()
            .iter()
            .count(),
        1
    );
}

#[test]
fn failed_power_and_damaged_modules_cannot_make_free_progress_or_spend_reserved_inputs() {
    let mut fixture = Fixture::new();
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    fixture.put(fixture.facility, &recipe.inputs);
    fixture.start(&recipe);
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = 0;
    let materials = fixture.inventory(fixture.facility).cargo.clone();
    tick(&mut fixture.world);
    let job = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0];
    assert_eq!(job.status, JobStatus::AwaitingPower);
    assert_eq!(job.progress_ticks, 0);
    assert_eq!(fixture.inventory(fixture.facility).cargo, materials);
    let devices: Vec<_> = lanes(&fixture.world, fixture.facility)
        .into_iter()
        .filter(|lane| lane.capability == recipe.capability)
        .map(|lane| lane.device)
        .collect();
    assert!(!devices.is_empty());
    for &device in &devices {
        fixture
            .world
            .get_mut::<hardware::Device>(device)
            .unwrap()
            .0
            .operational = false;
    }
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = recipe.energy_j;
    for _ in 0..4 {
        tick(&mut fixture.world);
    }
    let job = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0];
    assert_eq!(job.status, JobStatus::ModuleUnavailable);
    assert_eq!(job.progress_ticks, 0);
    assert_eq!(
        fixture.inventory(fixture.facility).energy_j,
        recipe.energy_j
    );
    for &device in &devices {
        fixture
            .world
            .get_mut::<hardware::Device>(device)
            .unwrap()
            .0
            .operational = true;
    }
    tick(&mut fixture.world);
    let job = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs[0];
    assert_eq!(job.progress_ticks, 1);
    assert_eq!(
        fixture.inventory(fixture.facility).energy_j,
        recipe.energy_j - cumulative_energy(recipe.energy_j, 1, recipe.duration_ticks)
    );
    fixture
        .world
        .get_mut::<hardware::Hull>(fixture.facility)
        .unwrap()
        .0 = 0.0;
    let before = fixture.inventory_bytes(fixture.facility);
    tick(&mut fixture.world);
    assert_eq!(fixture.inventory_bytes(fixture.facility), before);
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .jobs[0]
            .progress_ticks,
        1
    );
}

#[test]
fn physical_transfers_update_docked_mass_immediately_and_undocking_conserves_the_total() {
    let mut fixture = Fixture::new();
    let guest = fixture.guest(fixture.account);
    let item = CargoItem::Resource("water".into());
    fixture.put(
        fixture.facility,
        &[ItemStack {
            item: item.clone(),
            quantity: 3,
        }],
    );
    let host_before = fixture
        .world
        .get::<MassProps>(fixture.facility)
        .unwrap()
        .mass;
    let guest_before = fixture.world.get::<MassProps>(guest).unwrap().mass;
    let stored_before = fixture
        .world
        .get::<travel::StoredMass>(fixture.facility)
        .unwrap()
        .0;
    let unit_mass = osg_ships::industry::item_mass_kg(
        &item,
        &fixture.world.resource::<vessel::ShipCatalogue>().0,
    )
    .unwrap();
    let host = fixture.id(fixture.facility);
    let target = fixture.id(guest);
    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::Transfer {
            source: host,
            target,
            item,
            quantity: 3,
        },
        None,
    )
    .unwrap();
    assert!(
        (fixture.world.get::<MassProps>(guest).unwrap().mass - guest_before - 3.0 * unit_mass)
            .abs()
            < 1e-6
    );
    assert!(
        (fixture
            .world
            .get::<travel::StoredMass>(fixture.facility)
            .unwrap()
            .0
            - stored_before
            - 3.0 * unit_mass)
            .abs()
            < 1e-6
    );
    assert!(
        (fixture
            .world
            .get::<MassProps>(fixture.facility)
            .unwrap()
            .mass
            - host_before)
            .abs()
            < 1e-5
    );
    travel::undock(&mut fixture.world, guest).unwrap();
    assert_eq!(
        fixture
            .world
            .get::<travel::StoredMass>(fixture.facility)
            .unwrap()
            .0,
        0.0
    );
    let sum = fixture
        .world
        .get::<MassProps>(fixture.facility)
        .unwrap()
        .mass
        + fixture.world.get::<MassProps>(guest).unwrap().mass;
    assert!((sum - host_before).abs() < 1e-5);
}

#[test]
fn mine_loading_is_fractional_fair_authorized_and_never_accumulates_a_blocked_burst() {
    let mut fixture = Fixture::new();
    let first = fixture.guest(fixture.account);
    let second = fixture.guest(fixture.account);
    let foreign_owner = Id::new();
    let denied = fixture.guest(foreign_owner);
    let item = CargoItem::Resource("water".into());
    for ship in [first, second, denied] {
        fixture
            .world
            .entity_mut(ship)
            .insert(hardware::utilities::DockServiceRequest {
                cargo: true,
                power: false,
            });
    }
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .set_mine(MineSource {
            output: item.clone(),
            units_per_second: 13,
            remainder: 0,
            last_recipient: None,
        });
    for _ in 0..100 {
        tick(&mut fixture.world);
    }
    let a = fixture.quantity(first, &item);
    let b = fixture.quantity(second, &item);
    assert_eq!(a + b, 130);
    assert!(
        a.abs_diff(b) <= 1,
        "recipient cursor must distribute indivisible units fairly: {a}/{b}"
    );
    assert_eq!(fixture.quantity(denied, &item), 0);
    assert_eq!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .mine
            .as_ref()
            .unwrap()
            .remainder,
        0
    );
    let full = fixture.guest(fixture.account);
    fixture
        .world
        .entity_mut(full)
        .insert(hardware::utilities::DockServiceRequest {
            cargo: true,
            power: false,
        });
    let mut no_room = (*fixture.world.get::<vessel::ShipDesign>(full).unwrap().0).clone();
    no_room.capacity_m3 = 0.0;
    fixture.world.get_mut::<vessel::ShipDesign>(full).unwrap().0 = Arc::new(no_room);
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .mine
        .as_mut()
        .unwrap()
        .units_per_second = 30;
    for _ in 0..100 {
        tick(&mut fixture.world);
    }
    let odd_a = fixture.quantity(first, &item) - a;
    let odd_b = fixture.quantity(second, &item) - b;
    assert_eq!(odd_a + odd_b, 300);
    assert!(
        odd_a.abs_diff(odd_b) <= 1,
        "odd batch allocation must rotate: {odd_a}/{odd_b}"
    );
    assert_eq!(fixture.quantity(full, &item), 0);
    let a = fixture.quantity(first, &item);
    let b = fixture.quantity(second, &item);
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .mine
        .as_mut()
        .unwrap()
        .units_per_second = 13;
    for ship in [first, second] {
        fixture
            .world
            .entity_mut(ship)
            .remove::<hardware::utilities::DockServiceRequest>();
    }
    for _ in 0..1000 {
        tick(&mut fixture.world);
    }
    assert!(
        fixture
            .world
            .get::<IndustrialFacility>(fixture.facility)
            .unwrap()
            .mine
            .as_ref()
            .unwrap()
            .remainder
            < 10
    );
    fixture
        .world
        .entity_mut(first)
        .insert(hardware::utilities::DockServiceRequest {
            cargo: true,
            power: false,
        });
    tick(&mut fixture.world);
    assert_eq!(fixture.quantity(first, &item) - a, 1);
    assert_eq!(fixture.quantity(second, &item), b);

    // A small recipient takes its one available unit each tick. The remaining
    // thirteen units must alternate evenly between the two larger holds.
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut small_hold = (*fixture.world.get::<vessel::ShipDesign>(full).unwrap().0).clone();
    small_hold.capacity_m3 = osg_ships::industry::item_volume_m3(&item, &catalogue).unwrap();
    fixture.world.get_mut::<vessel::ShipDesign>(full).unwrap().0 = Arc::new(small_hold);
    fixture
        .world
        .entity_mut(second)
        .insert(hardware::utilities::DockServiceRequest {
            cargo: true,
            power: false,
        });
    fixture
        .world
        .get_mut::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .mine
        .as_mut()
        .unwrap()
        .units_per_second = 140;
    let partial_a = fixture.quantity(first, &item);
    let partial_b = fixture.quantity(second, &item);
    for _ in 0..60 {
        tick(&mut fixture.world);
        assert_eq!(fixture.quantity(full, &item), 1);
        fixture
            .world
            .get_mut::<hardware::ShipInventory>(full)
            .unwrap()
            .0
            .withdraw_cargo(&item, 1, &catalogue)
            .unwrap();
        hardware::synchronize_mass(&mut fixture.world, &[full]);
    }
    let delivered_a = fixture.quantity(first, &item) - partial_a;
    let delivered_b = fixture.quantity(second, &item) - partial_b;
    assert_eq!(delivered_a + delivered_b, 780);
    assert!(
        delivered_a.abs_diff(delivered_b) <= 1,
        "capacity-limited recipients must not bias larger holds: {delivered_a}/{delivered_b}"
    );
}

#[test]
fn transferring_inside_a_docked_carrier_preserves_all_ancestor_masses() {
    let mut fixture = Fixture::new();
    for bay in &mut fixture
        .world
        .get_mut::<travel::DockingBays>(fixture.facility)
        .unwrap()
        .0
    {
        bay.radius_m = 1000.0;
    }
    let blueprint = ShipBlueprint::from_bytes(include_bytes!(
        "../../../../../assets/ships/neris-anchorage.ship"
    ))
    .unwrap();
    let carrier = Fixture::spawn(
        &mut fixture.world,
        fixture.account,
        blueprint,
        DVec3::X * 200.0,
    );
    let child = Fixture::spawn(
        &mut fixture.world,
        fixture.account,
        osg_ships::expedition_patrol(),
        DVec3::X * 250.0,
    );
    travel::dock(&mut fixture.world, child, carrier, 0).unwrap();
    travel::dock(&mut fixture.world, carrier, fixture.facility, 0).unwrap();
    let item = CargoItem::Resource("water".into());
    fixture.put(
        carrier,
        &[ItemStack {
            item: item.clone(),
            quantity: 3,
        }],
    );
    let outer_before = fixture
        .world
        .get::<MassProps>(fixture.facility)
        .unwrap()
        .mass;
    let carrier_before = fixture.world.get::<MassProps>(carrier).unwrap().mass;
    let child_before = fixture.world.get::<MassProps>(child).unwrap().mass;
    let carrier_stored = fixture.world.get::<travel::StoredMass>(carrier).unwrap().0;
    let unit_mass = osg_ships::industry::item_mass_kg(
        &item,
        &fixture.world.resource::<vessel::ShipCatalogue>().0,
    )
    .unwrap();
    let source = fixture.id(carrier);
    let target = fixture.id(child);
    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::Transfer {
            source,
            target,
            item,
            quantity: 3,
        },
        None,
    )
    .unwrap();
    assert!(
        (fixture
            .world
            .get::<MassProps>(fixture.facility)
            .unwrap()
            .mass
            - outer_before)
            .abs()
            < 1e-5
    );
    assert!((fixture.world.get::<MassProps>(carrier).unwrap().mass - carrier_before).abs() < 1e-5);
    assert!(
        (fixture.world.get::<MassProps>(child).unwrap().mass - child_before - 3.0 * unit_mass)
            .abs()
            < 1e-6
    );
    assert!(
        (fixture.world.get::<travel::StoredMass>(carrier).unwrap().0
            - carrier_stored
            - 3.0 * unit_mass)
            .abs()
            < 1e-6
    );
    travel::undock(&mut fixture.world, carrier).unwrap();
    travel::undock(&mut fixture.world, child).unwrap();
    assert_eq!(
        fixture
            .world
            .get::<travel::StoredMass>(fixture.facility)
            .unwrap()
            .0,
        0.0
    );
    assert_eq!(
        fixture.world.get::<travel::StoredMass>(carrier).unwrap().0,
        0.0
    );
    let total: f64 = [fixture.facility, carrier, child]
        .into_iter()
        .map(|entity| fixture.world.get::<MassProps>(entity).unwrap().mass)
        .sum();
    assert!((total - outer_before).abs() < 1e-5);
}

#[test]
fn concurrent_factory_lanes_share_the_last_joule_without_partial_tick_progress() {
    let mut fixture = Fixture::new();
    let recipe = fixture.recipe(IndustryCapability::Fabricator);
    assert!(
        lanes(&fixture.world, fixture.facility)
            .iter()
            .filter(|lane| lane.capability == recipe.capability)
            .count()
            >= 2
    );
    fixture.put(fixture.facility, &recipe.inputs);
    fixture.put(fixture.facility, &recipe.inputs);
    fixture.start(&recipe);
    fixture.start(&recipe);
    let first_tick_energy = cumulative_energy(recipe.energy_j, 1, recipe.duration_ticks);
    assert!(first_tick_energy > 0);
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = first_tick_energy;
    tick(&mut fixture.world);
    let queue = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs;
    assert_eq!(queue.iter().map(|job| job.progress_ticks).sum::<u64>(), 1);
    assert_eq!(
        queue
            .iter()
            .filter(|job| job.status == JobStatus::AwaitingPower)
            .count(),
        1
    );
    assert_eq!(fixture.inventory(fixture.facility).energy_j, 0);
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .energy_j = first_tick_energy - 1;
    tick(&mut fixture.world);
    let queue = &fixture
        .world
        .get::<IndustrialFacility>(fixture.facility)
        .unwrap()
        .jobs;
    assert_eq!(queue.iter().map(|job| job.progress_ticks).sum::<u64>(), 1);
    assert_eq!(
        fixture.inventory(fixture.facility).energy_j,
        first_tick_energy - 1
    );
}

#[test]
fn cold_shield_reserves_accept_only_paid_unreserved_coolant_with_exact_mass() {
    let mut fixture = Fixture::new();
    let guest = fixture.guest(fixture.account);
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = fixture
        .world
        .get::<vessel::ShipDesign>(guest)
        .unwrap()
        .0
        .clone();
    let cold = osg_ships::ShipState::cold(&design, &catalogue);
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(guest)
        .unwrap()
        .0 = cold.inventory;
    fixture
        .world
        .get_mut::<hardware::ShipThermal>(guest)
        .unwrap()
        .0 = cold.thermal;
    hardware::synchronize_mass(&mut fixture.world, &[guest]);

    let coolant = CargoItem::Resource("shield_coolant".into());
    let unit_mass = osg_ships::industry::item_mass_kg(&coolant, &catalogue).unwrap();
    let capacity = (design.shield_reserve_capacity_kg / unit_mass).floor() as u64;
    assert!(capacity >= 3);
    let stock = ItemStack {
        item: coolant.clone(),
        quantity: capacity + 1,
    };
    fixture.put(fixture.facility, &[stock]);
    let source = fixture.id(fixture.facility);
    let ship = fixture.id(guest);
    let host_mass = fixture
        .world
        .get::<MassProps>(fixture.facility)
        .unwrap()
        .mass;
    let guest_mass = fixture.world.get::<MassProps>(guest).unwrap().mass;
    let host_inventory = fixture.inventory_bytes(fixture.facility);

    let overfill = IndustryCommand::Refill {
        source,
        ship,
        resource: "shield_coolant".into(),
        quantity: capacity + 1,
    };
    assert!(enqueue_command(&mut fixture.world, fixture.account, overfill, None,).is_err());
    assert_eq!(fixture.inventory_bytes(fixture.facility), host_inventory);
    assert_eq!(
        fixture
            .world
            .get::<hardware::ShipThermal>(guest)
            .unwrap()
            .0
            .shield_reserve_mg,
        0
    );

    let reserved = [ItemStack {
        item: coolant.clone(),
        quantity: capacity - 1,
    }];
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .reserve_cargo(&reserved, &catalogue)
        .unwrap();
    let reserved_inventory = fixture.inventory_bytes(fixture.facility);
    assert!(
        enqueue_command(
            &mut fixture.world,
            fixture.account,
            IndustryCommand::Refill {
                source,
                ship,
                resource: "shield_coolant".into(),
                quantity: 3,
            },
            None,
        )
        .is_err()
    );
    assert_eq!(
        fixture.inventory_bytes(fixture.facility),
        reserved_inventory
    );
    assert_eq!(
        fixture
            .world
            .get::<hardware::ShipThermal>(guest)
            .unwrap()
            .0
            .shield_reserve_mg,
        0
    );

    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::Refill {
            source,
            ship,
            resource: "shield_coolant".into(),
            quantity: 2,
        },
        None,
    )
    .unwrap();
    let delivered_mg = osg_ships::industry::mass_mg(2.0 * unit_mass).unwrap();
    let thermal = &fixture.world.get::<hardware::ShipThermal>(guest).unwrap().0;
    assert_eq!(thermal.shield_reserve_mg, delivered_mg);
    assert_eq!(thermal.shield_deployed_kg, 0.0);
    assert_eq!(fixture.quantity(fixture.facility, &coolant), capacity - 1);
    assert_eq!(
        fixture
            .inventory(fixture.facility)
            .cargo_available(&coolant, &catalogue)
            .unwrap(),
        0
    );
    assert!(
        (fixture.world.get::<MassProps>(guest).unwrap().mass - guest_mass - 2.0 * unit_mass).abs()
            < 1e-6
    );
    assert!(
        (fixture
            .world
            .get::<MassProps>(fixture.facility)
            .unwrap()
            .mass
            - host_mass)
            .abs()
            < 1e-5
    );

    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.facility)
        .unwrap()
        .0
        .release_cargo(&reserved, &catalogue)
        .unwrap();
    enqueue_command(
        &mut fixture.world,
        fixture.account,
        IndustryCommand::Refill {
            source,
            ship,
            resource: "shield_coolant".into(),
            quantity: capacity - 2,
        },
        None,
    )
    .unwrap();
    assert_eq!(
        fixture
            .world
            .get::<hardware::ShipThermal>(guest)
            .unwrap()
            .0
            .shield_reserve_mg,
        osg_ships::industry::mass_mg(capacity as f64 * unit_mass).unwrap()
    );
    assert_eq!(fixture.quantity(fixture.facility, &coolant), 1);
    assert!(
        (fixture
            .world
            .get::<MassProps>(fixture.facility)
            .unwrap()
            .mass
            - host_mass)
            .abs()
            < 1e-5
    );
    let full_inventory = fixture.inventory_bytes(fixture.facility);
    assert!(
        enqueue_command(
            &mut fixture.world,
            fixture.account,
            IndustryCommand::Refill {
                source,
                ship,
                resource: "shield_coolant".into(),
                quantity: 1,
            },
            None,
        )
        .is_err()
    );
    assert_eq!(fixture.inventory_bytes(fixture.facility), full_inventory);
}
