use super::*;
use crate::sim::{physics::MassProps, precision::PreciseTransform, simulation::SimulationCounters};
use bevy::math::DVec3;
use osg_model::ownership::{AccessGrant, AccessPolicy};
use osg_ships::DeviceSetting;
use std::time::Duration;

struct Fixture {
    app: App,
    account: Id,
    ship: Id,
    station: Id,
}

impl Fixture {
    fn new() -> Self {
        let account = Id::new();
        let mut blueprint = osg_ships::ntr_patrol();
        blueprint.parts.retain(|part| part.prototype != "storage");
        let path = std::env::temp_dir().join(format!("toy-product-ship-{account}.ship"));
        std::fs::write(&path, blueprint.to_bytes().unwrap()).unwrap();
        let app = crate::scenario(&[account], Some(account), Some(path.clone()));
        std::fs::remove_file(path).unwrap();
        let mut app = app.unwrap();
        for _ in 0..3 {
            app.update();
        }

        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let station = world
            .query_filtered::<Entity, With<IndustryFacility>>()
            .single(world)
            .unwrap();
        assert_eq!(
            world.get::<vessel::ShipDesign>(ship).unwrap().0.capacity_m3,
            0.
        );
        let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
        assert!(
            world
                .get::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .cargo_stacks(catalogue)
                .unwrap()
                .is_empty(),
            "scenario supplies must respect an absent cargo hold"
        );

        world.entity_mut(station).remove::<MineSource>();
        let mut inventory = world.get_mut::<hardware::ShipInventory>(station).unwrap();
        inventory.0.cargo.fill(0);
        inventory.0.packaged_parts.clear();
        inventory.0.reservations.clear();
        inventory.0.energy_j = 0;
        synchronize_mass(world, &[station]);

        Self {
            account,
            ship: world.get::<identity::Identity>(ship).unwrap().0,
            station: world.get::<identity::Identity>(station).unwrap().0,
            app,
        }
    }

    fn entity(&self, id: Id) -> Entity {
        identity::lookup(self.app.world(), id).unwrap()
    }

    fn inventory(&self, id: Id) -> &Inventory {
        &self
            .app
            .world()
            .get::<hardware::ShipInventory>(self.entity(id))
            .unwrap()
            .0
    }

    fn resource(&self, name: &str) -> usize {
        self.app
            .world()
            .resource::<vessel::ShipCatalogue>()
            .0
            .resources
            .iter()
            .position(|resource| resource.id == name)
            .unwrap()
    }

    fn cargo(&self, id: Id, resource: &str) -> u64 {
        self.inventory(id)
            .cargo_quantity(
                &CargoItem::Resource(resource.into()),
                &self.app.world().resource::<vessel::ShipCatalogue>().0,
            )
            .unwrap()
    }

    fn burn_fuel(&mut self) {
        let ship = self.entity(self.ship);
        let fuel = self.resource("reactor_fuel");
        let spent = self.resource("spent_fuel");
        let station = self.entity(self.station);
        let world = self.app.world_mut();
        let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
        let design = world.get::<vessel::ShipDesign>(ship).unwrap().0.clone();
        let parts = world.get::<hardware::PartDevices>(ship).unwrap().0.clone();
        let (index, reactor) = parts
            .iter()
            .enumerate()
            .find(|(_, entity)| world.get::<hardware::reactors::Reactor>(**entity).is_some())
            .map(|(index, entity)| (index, *entity))
            .unwrap();

        // An integer, fuel-limited burn makes product creation independent of rounding luck.
        let mut component = world
            .get_mut::<hardware::reactors::Reactor>(reactor)
            .unwrap();
        component.spec.fuel_energy_j_kg = 1.;
        component.core_energy_j = 0.;
        component.decay_energy_j = 0.;
        component.shutdown = false;
        world.get_mut::<hardware::DeviceSettings>(ship).unwrap().0
            [design.part_devices[index].unwrap()] = Some(DeviceSetting::GeneratorDemand(1.));
        let mut inventory = world.get_mut::<hardware::ShipInventory>(ship).unwrap();
        inventory.0.quantities[fuel] = 10;
        inventory.0.quantities[spent] = 0;
        inventory.0.energy_j = 0;
        let initial_mass = inventory.0.mass(&catalogue);

        world.run_system_once(hardware::reactors::generate).unwrap();
        world
            .get_mut::<hardware::ShipInventory>(station)
            .unwrap()
            .0
            .energy_j = 0;
        synchronize_mass(world, &[ship]);
        let inventory = &world.get::<hardware::ShipInventory>(ship).unwrap().0;
        assert_eq!(inventory.quantities[fuel], 0);
        assert_eq!(inventory.quantities[spent], 10);
        assert_eq!(inventory.mass(&catalogue), initial_mass);
    }

    fn dock(&mut self) {
        let ship = self.entity(self.ship);
        let station = self.entity(self.station);
        let world = self.app.world_mut();
        let host = *world.get::<PreciseTransform>(station).unwrap();
        let distance = world.get::<vessel::ShipDesign>(station).unwrap().0.radius
            + world.get::<vessel::ShipDesign>(ship).unwrap().0.radius
            + osg_model::travel::DOCKING_CLEARANCE_M * 0.5;
        let velocity = world
            .get::<crate::sim::physics::Velocity>(station)
            .unwrap()
            .0;
        world
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = host.translation_um.offset_by(DVec3::X * distance);
        world
            .get_mut::<crate::sim::physics::Velocity>(ship)
            .unwrap()
            .0 = velocity;
        travel::dock(world, ship, station, 0).unwrap();
    }

    fn unload(&mut self, account: Id, resource: &str, quantity: u64) -> Result<()> {
        execute(
            self.app.world_mut(),
            account,
            IndustryCommand::UnloadProduct {
                source: self.ship,
                target: self.station,
                resource: resource.into(),
                quantity,
            },
            None,
        )
    }

    fn advance_job(&mut self) {
        let world = self.app.world_mut();
        world.resource_mut::<SimulationCounters>().ticks += 1;
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_millis(100));
        advance(world);
    }

    fn restart(&mut self) {
        let bytes = crate::persistence::world::capture(self.app.world()).unwrap();
        crate::persistence::world::restore(self.app.world_mut(), &bytes).unwrap();
    }

    fn state(&self) -> Vec<u8> {
        let world = self.app.world();
        postcard::to_stdvec(&(
            self.inventory(self.ship),
            self.inventory(self.station),
            world.get::<MassProps>(self.entity(self.ship)).unwrap().mass,
            world
                .get::<MassProps>(self.entity(self.station))
                .unwrap()
                .mass,
        ))
        .unwrap()
    }
}

#[test]
fn reactor_products_without_a_cargo_hold_can_be_refined_and_refilled_across_restart() {
    let mut fixture = Fixture::new();
    fixture.burn_fuel();
    fixture.dock();
    let spent = fixture.resource("spent_fuel");
    let fuel = fixture.resource("reactor_fuel");
    let ship = fixture.entity(fixture.ship);
    let station = fixture.entity(fixture.station);
    let ship_mass = fixture.app.world().get::<MassProps>(ship).unwrap().mass;
    let host_mass = fixture.app.world().get::<MassProps>(station).unwrap().mass;
    let view = snapshot(
        fixture.app.world(),
        fixture.account,
        &IndustrySubscription {
            inventories: vec![fixture.ship],
            ..Default::default()
        },
    );
    assert_eq!(view.facilities[0].cargo_capacity_m3, 0.);
    assert!(view.facilities[0].items.is_empty());
    assert!(view.facilities[0].products.iter().any(|stack| {
        stack.item == CargoItem::Resource("spent_fuel".into()) && stack.quantity == 10
    }));

    fixture.unload(fixture.account, "spent_fuel", 10).unwrap();
    assert_eq!(fixture.inventory(fixture.ship).quantities[spent], 0);
    assert_eq!(fixture.cargo(fixture.station, "spent_fuel"), 10);
    assert!(
        (fixture.app.world().get::<MassProps>(ship).unwrap().mass - ship_mass + 10.).abs() < 1e-6
    );
    assert!((fixture.app.world().get::<MassProps>(station).unwrap().mass - host_mass).abs() < 1e-6);

    execute(
        fixture.app.world_mut(),
        fixture.account,
        IndustryCommand::StartRecipe {
            facility: fixture.station,
            recipe: "reprocess_spent_fuel".into(),
            batches: 1,
        },
        None,
    )
    .unwrap();
    fixture.advance_job();
    let job = &fixture
        .app
        .world()
        .get::<IndustryFacility>(station)
        .unwrap()
        .jobs[0];
    assert_eq!(job.view.status, JobStatus::AwaitingPower);
    assert_eq!(job.view.progress_ticks, 0);
    let energy = job.energy_j;
    let duration = job.view.duration_ticks;
    let job_id = job.view.id;
    assert!(energy > 0 && duration > 1);
    assert_eq!(
        fixture.inventory(fixture.station).reservations[&CargoItem::Resource("spent_fuel".into())],
        10
    );
    fixture
        .app
        .world_mut()
        .get_mut::<hardware::ShipInventory>(station)
        .unwrap()
        .0
        .energy_j = energy;
    fixture.advance_job();
    let paid = energy / duration;
    assert_eq!(fixture.inventory(fixture.station).energy_j, energy - paid);

    fixture.restart();
    let station = fixture.entity(fixture.station);
    let queue = &fixture
        .app
        .world()
        .get::<IndustryFacility>(station)
        .unwrap()
        .jobs;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].view.id, job_id);
    assert_eq!(queue[0].view.progress_ticks, 1);
    assert_eq!(fixture.inventory(fixture.ship).quantities[spent], 0);
    assert_eq!(fixture.cargo(fixture.station, "spent_fuel"), 10);
    assert_eq!(fixture.inventory(fixture.station).energy_j, energy - paid);
    for _ in 1..duration {
        fixture.advance_job();
    }
    assert!(
        fixture
            .app
            .world()
            .get::<IndustryFacility>(station)
            .unwrap()
            .jobs
            .is_empty()
    );
    assert!(fixture.inventory(fixture.station).reservations.is_empty());
    assert_eq!(fixture.inventory(fixture.station).energy_j, 0);
    assert_eq!(fixture.cargo(fixture.station, "spent_fuel"), 0);
    assert_eq!(fixture.cargo(fixture.station, "reactor_fuel"), 2);
    assert_eq!(fixture.cargo(fixture.station, "radioactive_waste"), 8);

    execute(
        fixture.app.world_mut(),
        fixture.account,
        IndustryCommand::Refill {
            source: fixture.station,
            ship: fixture.ship,
            resource: "reactor_fuel".into(),
            quantity: 2,
        },
        None,
    )
    .unwrap();
    assert_eq!(fixture.inventory(fixture.ship).quantities[fuel], 2);
    assert_eq!(fixture.cargo(fixture.station, "reactor_fuel"), 0);
    assert!((fixture.app.world().get::<MassProps>(station).unwrap().mass - host_mass).abs() < 1e-6);

    fixture.restart();
    for _ in 0..duration {
        fixture.advance_job();
    }
    assert_eq!(fixture.inventory(fixture.ship).quantities[fuel], 2);
    assert_eq!(fixture.inventory(fixture.ship).quantities[spent], 0);
    assert_eq!(fixture.cargo(fixture.station, "spent_fuel"), 0);
    assert_eq!(fixture.cargo(fixture.station, "reactor_fuel"), 0);
    assert_eq!(fixture.cargo(fixture.station, "radioactive_waste"), 8);
}

#[test]
fn unloading_products_rechecks_both_inventories_and_requires_a_shared_dock() {
    let mut fixture = Fixture::new();
    fixture.burn_fuel();
    let before = fixture.state();
    assert!(fixture.unload(fixture.account, "spent_fuel", 10).is_err());
    assert_eq!(fixture.state(), before);
    fixture.dock();

    let visitor = Id::new();
    identity::add_account(fixture.app.world_mut(), visitor, false);
    let before = fixture.state();
    assert!(fixture.unload(visitor, "spent_fuel", 10).is_err());
    assert_eq!(fixture.state(), before);
    let grant = ownership::AssetAccess(AccessPolicy {
        grants: vec![AccessGrant {
            principal: Principal::Player(visitor),
            permissions: [Permission::TransferCargo].into(),
        }],
        ..Default::default()
    });
    let ship = fixture.entity(fixture.ship);
    fixture
        .app
        .world_mut()
        .entity_mut(ship)
        .insert(grant.clone());
    assert!(fixture.unload(visitor, "spent_fuel", 10).is_err());
    assert_eq!(fixture.state(), before);
    let station = fixture.entity(fixture.station);
    fixture.app.world_mut().entity_mut(station).insert(grant);
    assert!(fixture.unload(visitor, "water", 1).is_err());
    assert_eq!(fixture.state(), before);

    let catalogue = fixture
        .app
        .world()
        .resource::<vessel::ShipCatalogue>()
        .0
        .clone();
    let spent = fixture.resource("spent_fuel");
    let capacity = fixture
        .app
        .world()
        .get::<vessel::ShipDesign>(station)
        .unwrap()
        .0
        .capacity_m3;
    let full = (capacity / catalogue.resources[spent].volume_m3).floor() as u64;
    fixture
        .app
        .world_mut()
        .get_mut::<hardware::ShipInventory>(station)
        .unwrap()
        .0
        .insert_item(
            &CargoItem::Resource("spent_fuel".into()),
            full,
            capacity,
            &catalogue,
        )
        .unwrap();
    synchronize_mass(fixture.app.world_mut(), &[station]);
    let full_state = fixture.state();
    assert!(fixture.unload(visitor, "spent_fuel", 10).is_err());
    assert_eq!(fixture.state(), full_state);

    fixture
        .app
        .world_mut()
        .get_mut::<hardware::ShipInventory>(station)
        .unwrap()
        .0
        .withdraw_cargo(&CargoItem::Resource("spent_fuel".into()), full, &catalogue)
        .unwrap();
    synchronize_mass(fixture.app.world_mut(), &[station]);
    fixture.unload(visitor, "spent_fuel", 10).unwrap();
    assert_eq!(fixture.inventory(fixture.ship).quantities[spent], 0);
    assert_eq!(fixture.cargo(fixture.station, "spent_fuel"), 10);
}
