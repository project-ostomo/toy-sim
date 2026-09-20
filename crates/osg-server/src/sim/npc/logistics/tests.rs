use super::*;
use crate::sim::{
    self, hardware,
    precision::PreciseTransform,
    travel::{self, Travel},
    vessel,
};
use bevy::{ecs::system::RunSystemOnce, math::DVec3};
use osg_model::{
    GalacticPosition,
    ownership::{AccessGrant, AccessPolicy, Principal},
};
use osg_ships::{Catalogue, ShipBlueprint};
use std::{collections::BTreeSet, sync::Arc};

struct Fixture {
    world: World,
    account: Id,
    source: Entity,
    destination: Entity,
    ship: Entity,
}

impl Fixture {
    fn new() -> Self {
        let mut world = World::new();
        identity::initialize(&mut world, &[]);
        world.init_resource::<SimulationCounters>();
        world.init_resource::<vessel::WasmRuntime>();
        world.insert_resource(Time::<Fixed>::from_hz(10.0));
        world.insert_resource(vessel::ShipCatalogue(Catalogue::builtin()));
        Self::in_world(world, DVec3::ZERO)
    }

    fn in_world(mut world: World, frame_velocity: DVec3) -> Self {
        let account = Id::new();
        identity::add_account(&mut world, account, false);
        let station = ShipBlueprint::from_bytes(include_bytes!(
            "../../../../../../assets/ships/neris-anchorage.ship"
        ))
        .unwrap();
        let source = Self::spawn(
            &mut world,
            account,
            &station,
            DVec3::ZERO,
            frame_velocity,
            "Ore source",
        );
        let destination = Self::spawn(
            &mut world,
            account,
            &station,
            DVec3::NEG_Z * 100_000.0,
            frame_velocity,
            "Ore receiver",
        );
        for station in [source, destination] {
            world.entity_mut(station).remove::<vessel::ShipSoftware>();
            world.entity_mut(station).insert(identity::BeaconEmitter);
            for bay in &mut world.get_mut::<travel::DockingBays>(station).unwrap().0 {
                bay.public = true;
            }
        }

        let ship = Self::spawn(
            &mut world,
            account,
            &osg_ships::ntr_patrol(),
            DVec3::X * 400.0,
            frame_velocity,
            "Ore freighter",
        );
        industry::initialize(&mut world).unwrap();
        industry::refresh_publication(&mut world);
        travel::dock(&mut world, ship, source, 0).unwrap();
        let duty = HaulDuty {
            account,
            source: world.get::<identity::Identity>(source).unwrap().0,
            destination: world.get::<identity::Identity>(destination).unwrap().0,
            item: CargoItem::Resource("industrial_ore".into()),
            quantity: 100,
            stage: HaulStage::Loading,
            next_check_tick: 0,
            problem: None,
        };
        world.entity_mut(ship).insert(duty);
        Self {
            world,
            account,
            source,
            destination,
            ship,
        }
    }

    fn spawn(
        world: &mut World,
        account: Id,
        blueprint: &ShipBlueprint,
        position: DVec3,
        frame_velocity: DVec3,
        name: &str,
    ) -> Entity {
        let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
        let design = Arc::new(blueprint.compile(catalogue).unwrap());
        let offset = GalacticPosition::from_meters(position);
        let entity = vessel::spawn_ship(
            world,
            design,
            PreciseTransform {
                translation_um: GalacticPosition {
                    x: (1_i128 << 100) + offset.x,
                    y: offset.y,
                    z: offset.z,
                },
                ..Default::default()
            },
            frame_velocity,
            name.into(),
        )
        .unwrap();
        identity::attach_ship(world, entity, account).unwrap();
        world.run_system_once(hardware::initialize).unwrap();
        entity
    }

    fn id(&self, entity: Entity) -> Id {
        self.world.get::<identity::Identity>(entity).unwrap().0
    }

    fn put_ore(&mut self, entity: Entity, quantity: u64) {
        self.put_resource(entity, "industrial_ore", quantity);
    }

    fn put_resource(&mut self, entity: Entity, resource: &str, quantity: u64) {
        let catalogue = self.world.resource::<vessel::ShipCatalogue>().0.clone();
        let capacity = self
            .world
            .get::<vessel::ShipDesign>(entity)
            .unwrap()
            .0
            .capacity_m3;
        self.world
            .get_mut::<hardware::ShipInventory>(entity)
            .unwrap()
            .0
            .insert_item(
                &CargoItem::Resource(resource.into()),
                quantity,
                capacity,
                &catalogue,
            )
            .unwrap();
        industry::synchronize_mass(&mut self.world, &[entity]);
    }

    fn ore(&self, entity: Entity) -> u64 {
        ore(&self.world, entity)
    }

    fn advance(&mut self) {
        self.world.resource_mut::<SimulationCounters>().ticks += 10;
        advance(&mut self.world);
    }

    fn duty(&self) -> &HaulDuty {
        self.world.get::<HaulDuty>(self.ship).unwrap()
    }

    fn restrict(&mut self, entity: Entity, permissions: &[Permission]) {
        let foreign = Id::new();
        identity::add_account(&mut self.world, foreign, false);
        self.world.entity_mut(entity).insert((
            ownership::AssetOwner(Principal::Player(foreign)),
            ownership::AssetAccess(AccessPolicy {
                public: BTreeSet::new(),
                grants: vec![AccessGrant {
                    principal: Principal::Player(self.account),
                    permissions: permissions.iter().copied().collect(),
                }],
            }),
        ));
    }
}

fn ore(world: &World, entity: Entity) -> u64 {
    world
        .get::<hardware::ShipInventory>(entity)
        .unwrap()
        .0
        .cargo_quantity(
            &CargoItem::Resource("industrial_ore".into()),
            &world.resource::<vessel::ShipCatalogue>().0,
        )
        .unwrap()
}

#[test]
fn finite_warehouse_loads_only_available_cargo_then_queues_real_departure() {
    let mut fixture = Fixture::new();
    fixture.put_ore(fixture.source, 125);
    let catalogue = fixture.world.resource::<vessel::ShipCatalogue>().0.clone();
    fixture
        .world
        .get_mut::<hardware::ShipInventory>(fixture.source)
        .unwrap()
        .0
        .reserve_cargo(
            &[osg_model::industry::ItemStack {
                item: CargoItem::Resource("industrial_ore".into()),
                quantity: 25,
            }],
            &catalogue,
        )
        .unwrap();

    fixture.advance();
    assert_eq!(fixture.ore(fixture.source), 25);
    assert_eq!(fixture.ore(fixture.ship), 100);
    assert_eq!(fixture.ore(fixture.destination), 0);
    assert_eq!(fixture.duty().stage, HaulStage::Loading);

    fixture.advance();
    assert_eq!(fixture.duty().stage, HaulStage::Outbound);
    let travel = &fixture.world.get::<Travel>(fixture.ship).unwrap().0;
    assert_eq!(
        travel
            .orders
            .iter()
            .map(|order| order.action.clone())
            .collect::<Vec<_>>(),
        vec![Order::Undock, Order::Dock(fixture.id(fixture.destination))]
    );
    assert!(travel.autopilot_enabled);
    assert_eq!(
        fixture
            .world
            .get::<travel::DockedIn>(fixture.ship)
            .unwrap()
            .0,
        fixture.source
    );
    assert!(fixture.duty().problem.is_none());
}

#[test]
fn loading_waits_for_real_mine_production_and_remote_unloading_is_rejected() {
    let mut fixture = Fixture::new();
    fixture
        .world
        .entity_mut(fixture.source)
        .insert(industry::MineSource {
            output: CargoItem::Resource("industrial_ore".into()),
            units_per_second: 10,
            remainder: 0,
            last_recipient: None,
        });
    fixture.advance();
    assert_eq!(fixture.ore(fixture.ship), 0);
    assert_eq!(fixture.duty().stage, HaulStage::Loading);

    for _ in 0..100 {
        industry::advance(&mut fixture.world);
    }
    assert_eq!(fixture.ore(fixture.ship), 100);
    assert_eq!(fixture.ore(fixture.source), 0);
    fixture.advance();
    assert_eq!(fixture.duty().stage, HaulStage::Outbound);

    fixture
        .world
        .get_mut::<HaulDuty>(fixture.ship)
        .unwrap()
        .stage = HaulStage::Unloading;
    fixture.advance();
    assert_eq!(fixture.duty().stage, HaulStage::Paused);
    assert_eq!(fixture.ore(fixture.ship), 100);
    assert_eq!(fixture.ore(fixture.destination), 0);
}

#[test]
fn external_navigation_supersedes_the_standing_order_without_later_overwrite() {
    let mut fixture = Fixture::new();
    fixture.put_ore(fixture.source, 100);
    fixture.advance();
    fixture.advance();
    assert_eq!(fixture.duty().stage, HaulStage::Outbound);

    let ship = fixture.id(fixture.ship);
    let state = commands::telemetry(&fixture.world, fixture.account, ship).unwrap();
    let external = Order::WaitUntil(50_000);
    commands::execute(
        &mut fixture.world,
        fixture.account,
        ship,
        state.telemetry.authority_revision,
        ShipCommand::SetTravel {
            preferences: PlanningPreferences::default(),
            engage: true,
            expected_revision: state.telemetry.travel.revision,
            orders: vec![external.clone()],
        },
    )
    .unwrap();
    let replacement = fixture.world.get::<Travel>(fixture.ship).unwrap().0.clone();
    assert_eq!(fixture.duty().stage, HaulStage::Paused);

    for _ in 0..4 {
        fixture.advance();
    }
    assert_eq!(
        fixture.world.get::<Travel>(fixture.ship).unwrap().0,
        replacement
    );
    assert_eq!(replacement.orders[0].action, external);
    assert_eq!(fixture.ore(fixture.ship), 100);
    assert_eq!(fixture.ore(fixture.destination), 0);
}

#[test]
fn permission_revocation_pauses_empty_loading_and_in_flight_duties() {
    let mut loading = Fixture::new();
    loading.restrict(loading.source, &[Permission::View]);
    loading.advance();
    assert_eq!(loading.duty().stage, HaulStage::Paused);
    assert!(loading.duty().problem.is_some());
    assert_eq!(loading.ore(loading.ship), 0);

    let mut outbound = Fixture::new();
    outbound.put_ore(outbound.source, 100);
    outbound.advance();
    outbound.advance();
    let route = outbound
        .world
        .get::<Travel>(outbound.ship)
        .unwrap()
        .0
        .clone();
    outbound.restrict(
        outbound.ship,
        &[
            Permission::View,
            Permission::Configure,
            Permission::TransferCargo,
        ],
    );
    assert!(
        commands::telemetry(
            &outbound.world,
            outbound.account,
            outbound.id(outbound.ship)
        )
        .is_ok()
    );
    outbound.advance();
    assert_eq!(outbound.duty().stage, HaulStage::Paused);
    assert!(outbound.duty().problem.is_some());
    assert_eq!(
        outbound.world.get::<Travel>(outbound.ship).unwrap().0,
        route
    );
    assert_eq!(outbound.ore(outbound.ship), 100);
}

#[test]
fn ntr_freighter_physically_completes_a_hundred_kilometre_roundtrip() {
    physical_roundtrip(DVec3::ZERO);
}

#[test]
fn ntr_freighter_roundtrip_preserves_fuel_in_a_moving_inertial_frame() {
    physical_roundtrip(DVec3::new(18_000., 24_000., 0.));
}

fn physical_roundtrip(frame_velocity: DVec3) {
    let mut app = sim::application(None);
    app.update();
    app.update();
    let entities = app
        .world_mut()
        .query_filtered::<Entity, With<vessel::Vessel>>()
        .iter(app.world())
        .collect::<Vec<_>>();
    for entity in entities {
        app.world_mut().despawn(entity);
    }
    let world = std::mem::replace(app.world_mut(), World::new());
    let mut fixture = Fixture::in_world(world, frame_velocity);
    fixture.put_ore(fixture.source, 100);
    for station in [fixture.source, fixture.destination] {
        fixture.put_resource(station, "water", 20_000);
    }
    sim::infrastructure::publish_navigation(&mut fixture.world);
    let catalogue = &fixture
        .world
        .resource::<sim::infrastructure::NavigationPublication>()
        .catalogue;
    assert_eq!(catalogue.beacons.len(), 2);
    assert!(
        catalogue
            .beacons
            .iter()
            .all(|beacon| beacon.gate_exit.is_none())
    );

    let Fixture {
        world,
        ship,
        source,
        destination,
        ..
    } = fixture;
    *app.world_mut() = world;

    let water = app
        .world()
        .resource::<vessel::ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|resource| resource.id == "water")
        .unwrap();
    let initial_fuel = app
        .world()
        .get::<hardware::ShipInventory>(ship)
        .unwrap()
        .0
        .quantities[water];
    let initial_source_position = app
        .world()
        .get::<PreciseTransform>(source)
        .unwrap()
        .translation_um;
    let initial_mass = app
        .world()
        .get::<crate::sim::physics::MassProps>(ship)
        .unwrap()
        .mass;
    let cost = osg_model::transfer::TransferCost::default();
    let acceleration = 250_000.0 / initial_mass;
    let flow = 250_000.0 / (300.0 * 9.80665);
    let rcs_flow = app
        .world()
        .get::<vessel::ShipDesign>(ship)
        .unwrap()
        .0
        .device_catalogue
        .iter()
        .filter_map(|device| match device.kind {
            osg_ships::DeviceKind::Rcs {
                propellant_kg_s, ..
            } => Some(propellant_kg_s),
            _ => None,
        })
        .sum::<f64>();
    eprintln!("freight frame={frame_velocity:?}; hardware RCS rated flow={rcs_flow} kg/s");
    eprintln!(
        "freight initial mass={initial_mass:.1} kg, default ideal={:?}, cruise={:.2} m/s",
        cost.estimate(100_000.0, acceleration, flow),
        cost.cruise_speed(100_000.0, 0.0, acceleration, flow)
    );
    let mut reached_receiver = false;
    let mut departed_receiver = false;
    let mut furthest_m: f64 = 0.0;
    let mut completed_tick = None;
    let mut previous_stage = HaulStage::Loading;

    for elapsed in 0..60_000 {
        app.update();
        finish_pending_route(app.world_mut(), ship);
        let world = app.world();
        let duty = world.get::<HaulDuty>(ship).unwrap();
        assert_ne!(
            duty.stage,
            HaulStage::Paused,
            "freight duty paused at tick {elapsed}: {:?}",
            duty.problem
        );
        if duty.stage != previous_stage {
            let fuel = world
                .get::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .quantities[water];
            eprintln!(
                "freight stage {:?} at tick {elapsed}, {fuel} water units",
                duty.stage
            );
            previous_stage = duty.stage;
        }
        if elapsed % 500 == 0 {
            let position = freight_position(world, ship);
            let target = if duty.stage == HaulStage::Inbound {
                source
            } else {
                destination
            };
            let target_position = world
                .get::<PreciseTransform>(target)
                .unwrap()
                .translation_um;
            let velocity = world
                .get::<crate::sim::physics::Velocity>(ship)
                .map_or(DVec3::ZERO, |velocity| velocity.0 - frame_velocity);
            let angular = world
                .get::<crate::sim::physics::AngularVelocity>(ship)
                .map_or(DVec3::ZERO, |v| v.0);
            let force = world
                .get::<hardware::propulsion::ActuatorOutput>(ship)
                .unwrap()
                .force;
            let fuel = world
                .get::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .quantities[water];
            let battery = world
                .get::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .energy_j;
            let power = world.get::<hardware::PowerFlow>(ship).unwrap();
            eprintln!(
                "freight power tick={elapsed} battery={battery} generated={:.0} requested={:.0} supplied={:.0}",
                power.generated_w, power.requested_w, power.supplied_w
            );
            eprintln!(
                "freight tick={elapsed} stage={:?} range={:.1} speed={:.2} spin={:.4} force={:.0} fuel={fuel}",
                duty.stage,
                position.relative_to(target_position).length(),
                velocity.length(),
                angular.length(),
                force.length()
            );
        }
        let travel = &world.get::<Travel>(ship).unwrap().0;
        assert!(
            !matches!(travel.status, Status::Blocked(_)),
            "autopilot blocked at tick {elapsed}: {:?}",
            travel.status
        );
        furthest_m = furthest_m.max(
            freight_position(world, ship)
                .relative_to(
                    world
                        .get::<PreciseTransform>(source)
                        .unwrap()
                        .translation_um,
                )
                .length(),
        );
        reached_receiver |= world
            .get::<travel::DockedIn>(ship)
            .is_some_and(|dock| dock.0 == destination);
        departed_receiver |= reached_receiver
            && duty.stage == HaulStage::Inbound
            && world.get::<travel::DockedIn>(ship).is_none();
        if departed_receiver
            && world
                .get::<travel::DockedIn>(ship)
                .is_some_and(|dock| dock.0 == source)
            && duty.stage == HaulStage::Loading
        {
            completed_tick = Some(elapsed);
            break;
        }
    }

    let world = app.world();
    assert!(
        completed_tick.is_some(),
        "roundtrip did not finish: stage={:?}, travel={:?}, distance={furthest_m}",
        world.get::<HaulDuty>(ship).unwrap(),
        world.get::<Travel>(ship).unwrap().0
    );
    assert!(reached_receiver && departed_receiver);
    assert!(
        (99_000.0..110_000.0).contains(&furthest_m),
        "physical excursion relative to the moving source was {furthest_m} m"
    );
    if frame_velocity.length() > 0. {
        let source_displacement = world
            .get::<PreciseTransform>(source)
            .unwrap()
            .translation_um
            .relative_to(initial_source_position);
        assert!(source_displacement.length() > 30_000_000.);
        assert!(
            source_displacement
                .normalize()
                .distance(frame_velocity.normalize())
                < 1e-6
        );
    }
    assert_eq!(ore(world, source), 0);
    assert_eq!(ore(world, ship), 0);
    assert_eq!(ore(world, destination), 100);
    let remaining_fuel = world
        .get::<hardware::ShipInventory>(ship)
        .unwrap()
        .0
        .quantities[water];
    assert!(remaining_fuel > 0 && remaining_fuel < initial_fuel);
    let terminal_water = |entity| {
        world
            .get::<hardware::ShipInventory>(entity)
            .unwrap()
            .0
            .cargo_quantity(
                &CargoItem::Resource("water".into()),
                &world.resource::<vessel::ShipCatalogue>().0,
            )
            .unwrap()
    };
    let source_water = terminal_water(source);
    let destination_water = terminal_water(destination);
    assert!(
        destination_water < 20_000,
        "return leg must use the ordinary finite terminal refill"
    );
    assert!(source_water <= 20_000 && destination_water > 0);
    let consumed_fuel = 40_000 + initial_fuel - source_water - destination_water - remaining_fuel;
    assert!(
        consumed_fuel > 0 && consumed_fuel < 3_000,
        "roundtrip consumed {consumed_fuel} kg of water; clearance burns must remain economical"
    );
    eprintln!(
        "freight consumed={consumed_fuel} water units; terminal stocks={source_water}/{destination_water}"
    );
    assert!(world.get::<hardware::Hull>(ship).unwrap().0 > 0.0);
    eprintln!(
        "freight roundtrip: {} ticks, {:.0} m reach, {remaining_fuel}/{initial_fuel} water units retained",
        completed_tick.unwrap(),
        furthest_m
    );
}

fn freight_position(world: &World, ship: Entity) -> GalacticPosition {
    let physical_body = world
        .get::<travel::DockedIn>(ship)
        .map_or(ship, |dock| dock.0);
    world
        .get::<PreciseTransform>(physical_body)
        .unwrap()
        .translation_um
}

fn finish_pending_route(world: &mut World, ship: Entity) {
    if world.get::<Travel>(ship).unwrap().0.status != Status::Planning {
        return;
    }
    let tick = world.resource::<SimulationCounters>().ticks;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while world.get::<Travel>(ship).unwrap().0.status == Status::Planning {
        travel::plan_orders(world);
        sim::route_service::advance(world);
        assert!(
            std::time::Instant::now() < deadline,
            "route worker did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(world.resource::<SimulationCounters>().ticks, tick);
}
