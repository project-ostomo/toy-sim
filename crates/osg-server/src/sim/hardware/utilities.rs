use super::*;
use crate::sim::{
    identity::{DirectoryEmitter, NavigationBeaconEmitter},
    ownership::{self, AssetAccess, AssetOwner, Directory},
    travel::{Bay, DockingBays, Dormant},
};
use osg_ships::utilities::UtilityDef;

#[derive(Component)]
pub struct Utility(pub UtilityDef);

#[derive(Component, Default)]
pub struct Crew {
    pub people: u32,
    pub capacity: u32,
    pub support_fraction: f64,
}

#[derive(Component, Default)]
pub struct DockServices {
    pub cargo_kg_s: f64,
    pub power_w: f64,
}

#[derive(Component, Default)]
pub struct DockServiceRequest {
    pub cargo: bool,
    pub power: bool,
}

/// Rebuild derived emission state after hardware restoration, without spending energy.
pub fn refresh_emitters(world: &mut World) {
    let states: Vec<_> = world
        .query::<(Entity, &PartDevices, &Hull, Has<Dormant>)>()
        .iter(world)
        .map(|(ship, parts, hull, dormant)| {
            let mut directory = false;
            let mut navigation = false;
            if hull.0 > 0.0 && !dormant {
                for &part in &parts.0 {
                    let Some(device) = world.get::<Device>(part) else {
                        continue;
                    };
                    if !device.0.operational || !device.0.powered {
                        continue;
                    }
                    match world.get::<Utility>(part).map(|utility| &utility.0) {
                        Some(UtilityDef::DirectoryTransmitter { .. }) => directory = true,
                        Some(UtilityDef::NavigationBeacon { .. }) => navigation = true,
                        _ => {}
                    }
                }
            }
            (ship, directory, navigation)
        })
        .collect();

    for (ship, directory, navigation) in states {
        let mut ship = world.entity_mut(ship);
        if directory {
            ship.insert(DirectoryEmitter);
        } else {
            ship.remove::<DirectoryEmitter>();
        }
        if navigation {
            ship.insert(NavigationBeaconEmitter);
        } else {
            ship.remove::<NavigationBeaconEmitter>();
        }
    }
}

pub fn install_ship(
    commands: &mut Commands,
    ship: Entity,
    design: &CompiledShipDesign,
    existing_crew: Option<&Crew>,
    existing_bays: Option<&DockingBays>,
) {
    let mut capacity = 0_u32;
    let mut bays = Vec::new();
    for part in &design.parts {
        let Equipment::Utility { utility } = &part.definition.equipment else {
            continue;
        };
        match *utility {
            UtilityDef::Crew { capacity: n } => capacity = capacity.saturating_add(n),
            UtilityDef::Docking {
                radius_m,
                mass_capacity_kg,
            } => bays.push(Bay {
                centre_m: (part.centre - design.centre).to_array(),
                rotation: bevy::math::DQuat::from_mat3(&part.rotation).to_array(),
                radius_m,
                mass_capacity_kg,
                public: false,
                allowed: Default::default(),
                reservation: None,
            }),
            _ => {}
        }
    }
    commands.entity(ship).insert((
        Crew {
            people: existing_crew.map_or(capacity, |crew| crew.people.min(capacity)),
            capacity,
            support_fraction: existing_crew.map_or(1., |crew| crew.support_fraction),
        },
        DockServices::default(),
    ));
    if existing_bays.is_none() && !bays.is_empty() {
        commands.entity(ship).insert(DockingBays(bays));
    }
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct UtilityHardware {
    inventory: &'static mut ShipInventory,
    hull: &'static Hull,
    thermal: &'static mut ShipThermal,
    settings: &'static DeviceSettings,
    range: &'static mut SensorRange,
    parts: &'static PartDevices,
}

pub fn run(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut ships: Query<
        (
            Entity,
            &ShipDesign,
            UtilityHardware,
            &mut Crew,
            &mut DockServices,
            Has<DirectoryEmitter>,
            Has<NavigationBeaconEmitter>,
            &mut crate::sim::sensors::Sensor,
            Has<Dormant>,
        ),
        Without<super::super::travel::SystemsSuspended>,
    >,
    mut parts: Query<(&Utility, &mut Device, &mut DevicePower)>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("hardware.utilities.run");
    let dt = time.delta_secs_f64();
    for (
        ship,
        design,
        mut h,
        mut crew,
        mut services,
        had_beacon,
        had_navigation,
        mut sensor,
        absent,
    ) in &mut ships
    {
        commands.entity(ship).remove::<DockServiceRequest>();
        *services = DockServices::default();
        let mut beacon = false;
        let mut navigation = false;
        let mut supported = 0.;
        for &entity in &h.parts.0 {
            let Ok((utility, mut device, mut power)) = parts.get_mut(entity) else {
                continue;
            };
            if matches!(utility.0, UtilityDef::Command { .. }) {
                continue;
            }
            *power = DevicePower::default();
            device.0.powered = false;
            device.0.actual = 0.;
            if matches!(
                utility.0,
                UtilityDef::Factory { .. } | UtilityDef::Shipyard { .. }
            ) {
                continue;
            }
            if !device.0.operational || h.hull.0 <= 0. {
                continue;
            }
            if absent
                && matches!(
                    utility.0,
                    UtilityDef::Sensor { .. }
                        | UtilityDef::DirectoryTransmitter { .. }
                        | UtilityDef::NavigationBeacon { .. }
                )
            {
                continue;
            }
            let requested = match utility.0 {
                UtilityDef::Command { power_w }
                | UtilityDef::Sensor { power_w, .. }
                | UtilityDef::DirectoryTransmitter { power_w }
                | UtilityDef::NavigationBeacon { power_w }
                | UtilityDef::LifeSupport { power_w, .. }
                | UtilityDef::CargoHandler { power_w, .. } => power_w,
                _ => 0.,
            };
            power.requested_w = requested;
            let fraction = spend(&mut h.inventory.0, [0., 0., requested * dt]);
            power.supplied_w = requested * fraction;
            h.thermal.0.add_waste_heat(power.supplied_w * dt, dt);
            device.0.powered = fraction >= 1. - 1e-9;
            match utility.0 {
                UtilityDef::Sensor { range_m, .. } if device.0.powered => {
                    let enabled = matches!(
                        h.settings.0[design.0.avionics_handles[2].0 as usize],
                        Some(DeviceSetting::SensorEnabled(true))
                    );
                    if enabled {
                        h.range.0 = h.range.0.max(range_m);
                        sensor.range_m = sensor.range_m.max(range_m);
                        device.0.actual = range_m;
                    }
                }
                UtilityDef::DirectoryTransmitter { .. } => beacon |= device.0.powered,
                UtilityDef::NavigationBeacon { .. } => navigation |= device.0.powered,
                UtilityDef::LifeSupport {
                    capacity,
                    supplies_kg_per_person_s,
                    ..
                } => {
                    let people = (crew.people as f64 - supported)
                        .max(0.)
                        .min(capacity as f64);
                    let wanted = people * supplies_kg_per_person_s * dt * fraction;
                    let supplied = cat
                        .0
                        .resources
                        .iter()
                        .enumerate()
                        .find(|(_, resource)| resource.id == "life_support_supplies")
                        .map_or(0.0, |(index, resource)| {
                            let available = h.inventory.0.available(index) * resource.mass_kg;
                            let supplied = wanted.min(available);
                            h.inventory.0.consume(index, supplied / resource.mass_kg);
                            supplied
                        });
                    supported += if wanted > 0. {
                        people * fraction * supplied / wanted
                    } else {
                        0.
                    };
                }
                UtilityDef::CargoHandler { transfer_kg_s, .. } => {
                    services.cargo_kg_s += transfer_kg_s * fraction
                }
                UtilityDef::PowerCoupler { transfer_w } => services.power_w += transfer_w,
                _ => {}
            }
        }
        crew.support_fraction = if crew.people > 0 {
            (supported / crew.people as f64).min(1.)
        } else {
            1.
        };

        if beacon && !had_beacon {
            commands.entity(ship).insert(DirectoryEmitter);
        } else if !beacon && had_beacon {
            commands.entity(ship).remove::<DirectoryEmitter>();
        }
        if navigation && !had_navigation {
            commands.entity(ship).insert(NavigationBeaconEmitter);
        } else if !navigation && had_navigation {
            commands.entity(ship).remove::<NavigationBeaconEmitter>();
        }
    }
}

pub fn service_docked(
    time: Res<Time<Fixed>>,
    directory: Option<Res<Directory>>,
    cat: Res<ShipCatalogue>,
    hosts: Query<
        (
            Entity,
            &DockServices,
            &crate::sim::travel::StoredShips,
            &AssetOwner,
            Option<&AssetAccess>,
        ),
        Without<Dormant>,
    >,
    ships: Query<(&ShipDesign, &AssetOwner, &DockServiceRequest)>,
    mut inventories: Query<&mut ShipInventory>,
    mut stored: Query<&mut crate::sim::travel::StoredMass>,
    mut masses: Query<&mut MassProps>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("hardware.utilities.service_docked");
    let Some(directory) = directory else {
        return;
    };
    let dt = time.delta_secs_f64();
    for (host, services, guests, owner, access) in &hosts {
        let mut cargo_budget = services.cargo_kg_s * dt;
        let mut power_budget = (services.power_w * dt).stochastic_round();
        for guest in guests.iter() {
            let Ok((design, guest_owner, request)) = ships.get(guest) else {
                continue;
            };
            if !ownership::permits_principal(
                &directory.0,
                owner.0,
                access.map(|access| &access.0),
                guest_owner.0,
                osg_model::ownership::Permission::TransferCargo,
            ) {
                continue;
            }
            let Ok([mut source, mut target]) = inventories.get_many_mut([host, guest]) else {
                continue;
            };
            let energy = if request.power {
                power_budget
                    .min(source.0.energy_j)
                    .min(design.0.battery_j.saturating_sub(target.0.energy_j))
            } else {
                0
            };
            source.0.energy_j -= energy;
            target.0.energy_j += energy;
            power_budget -= energy;
            if !request.cargo {
                continue;
            }
            for (i, resource) in cat.0.resources.iter().enumerate() {
                let room = (design.0.capacity_m3 - target.0.cargo_volume(&cat.0)).max(0.0);
                let units = source
                    .0
                    .cargo_available(
                        &osg_model::industry::CargoItem::Resource(resource.id.clone()),
                        &cat.0,
                    )
                    .expect("validated dock inventory")
                    .min((room / resource.volume_m3).floor() as u64)
                    .min((cargo_budget / resource.mass_kg).floor() as u64);
                if units == 0 {
                    continue;
                }
                if source
                    .0
                    .transfer_cargo(&mut target.0, i, units, design.0.capacity_m3, &cat.0)
                    .is_ok()
                {
                    let mass = units as f64 * resource.mass_kg;
                    cargo_budget -= mass;
                    if let Ok(mut value) = stored.get_mut(host) {
                        value.0 += mass;
                    }
                    if let Ok(mut value) = masses.get_mut(guest) {
                        value.mass += mass;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::HardwareFixture;
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn add(fixture: &mut HardwareFixture, utility: UtilityDef) -> Entity {
        let entity = fixture
            .app
            .world_mut()
            .spawn((
                Utility(utility),
                Device(DeviceState::default()),
                DevicePower::default(),
            ))
            .id();
        fixture
            .app
            .world_mut()
            .get_mut::<PartDevices>(fixture.ship)
            .unwrap()
            .0
            .push(entity);
        entity
    }

    fn step(fixture: &mut HardwareFixture) {
        fixture
            .app
            .world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        fixture.app.world_mut().run_system_once(run).unwrap();
    }

    #[test]
    fn sensors_and_beacons_require_power_and_respect_sensor_switch() {
        let mut fixture = HardwareFixture::standard();
        add(
            &mut fixture,
            UtilityDef::Sensor {
                range_m: 2e9,
                power_w: 1000.,
                active: true,
            },
        );
        add(
            &mut fixture,
            UtilityDef::DirectoryTransmitter { power_w: 1000. },
        );
        fixture.set_inventory(|i| i.energy_j = 200);
        step(&mut fixture);
        assert_eq!(
            fixture
                .app
                .world()
                .get::<SensorRange>(fixture.ship)
                .unwrap()
                .0,
            2e9
        );
        assert!(
            fixture
                .app
                .world()
                .get::<DirectoryEmitter>(fixture.ship)
                .is_some()
        );
        fixture
            .app
            .world_mut()
            .get_mut::<SensorRange>(fixture.ship)
            .unwrap()
            .0 = 0.;
        step(&mut fixture);
        assert_eq!(
            fixture
                .app
                .world()
                .get::<SensorRange>(fixture.ship)
                .unwrap()
                .0,
            0.
        );
        assert!(
            fixture
                .app
                .world()
                .get::<DirectoryEmitter>(fixture.ship)
                .is_none()
        );
        fixture.set_inventory(|i| i.energy_j = 200);
        fixture
            .app
            .world_mut()
            .get_mut::<DeviceSettings>(fixture.ship)
            .unwrap()
            .0[fixture.design.avionics_handles[2].0 as usize] =
            Some(DeviceSetting::SensorEnabled(false));
        step(&mut fixture);
        assert_eq!(
            fixture
                .app
                .world()
                .get::<SensorRange>(fixture.ship)
                .unwrap()
                .0,
            0.
        );
    }

    #[test]
    fn directory_and_navigation_hardware_fail_independently() {
        let mut fixture = HardwareFixture::standard();
        let directory = add(
            &mut fixture,
            UtilityDef::DirectoryTransmitter { power_w: 1000.0 },
        );
        add(
            &mut fixture,
            UtilityDef::NavigationBeacon { power_w: 1000.0 },
        );
        fixture.set_inventory(|inventory| inventory.energy_j = 1000);
        step(&mut fixture);
        assert!(
            fixture
                .app
                .world()
                .get::<DirectoryEmitter>(fixture.ship)
                .is_some()
        );
        assert!(
            fixture
                .app
                .world()
                .get::<NavigationBeaconEmitter>(fixture.ship)
                .is_some()
        );

        fixture
            .app
            .world_mut()
            .get_mut::<Device>(directory)
            .unwrap()
            .0
            .operational = false;
        step(&mut fixture);
        assert!(
            fixture
                .app
                .world()
                .get::<DirectoryEmitter>(fixture.ship)
                .is_none()
        );
        assert!(
            fixture
                .app
                .world()
                .get::<NavigationBeaconEmitter>(fixture.ship)
                .is_some()
        );

        fixture.set_inventory(|inventory| inventory.energy_j = 0);
        step(&mut fixture);
        assert!(
            fixture
                .app
                .world()
                .get::<NavigationBeaconEmitter>(fixture.ship)
                .is_none()
        );
    }

    #[test]
    fn life_support_consumes_supplies() {
        let mut fixture = HardwareFixture::standard();
        add(
            &mut fixture,
            UtilityDef::LifeSupport {
                capacity: 2,
                power_w: 100.,
                supplies_kg_per_person_s: 10.,
            },
        );
        let cat = &fixture.app.world().resource::<ShipCatalogue>().0;
        let supplies = cat
            .resources
            .iter()
            .position(|r| r.id == "life_support_supplies")
            .unwrap();
        fixture
            .app
            .world_mut()
            .get_mut::<Crew>(fixture.ship)
            .unwrap()
            .people = 2;
        fixture.set_inventory(|i| {
            i.energy_j = 100;
            i.quantities[supplies] = 1;
        });
        step(&mut fixture);
        let crew = fixture.app.world().get::<Crew>(fixture.ship).unwrap();
        assert!((crew.support_fraction - 0.5).abs() < 1e-9);
        assert_eq!(crew.people, 2);
    }

    #[test]
    fn life_support_remains_continuous_between_discrete_supply_withdrawals() {
        let mut fixture = HardwareFixture::standard();
        add(
            &mut fixture,
            UtilityDef::LifeSupport {
                capacity: 350,
                power_w: 100.0,
                supplies_kg_per_person_s: 0.00003,
            },
        );
        let supplies = fixture
            .app
            .world()
            .resource::<ShipCatalogue>()
            .0
            .resources
            .iter()
            .position(|resource| resource.id == "life_support_supplies")
            .unwrap();
        fixture
            .app
            .world_mut()
            .get_mut::<Crew>(fixture.ship)
            .unwrap()
            .people = 350;
        fixture.set_inventory(|inventory| {
            inventory.energy_j = 100000000;
            inventory.quantities[supplies] = 100;
        });

        for _ in 0..20 {
            step(&mut fixture);
            assert_eq!(
                fixture
                    .app
                    .world()
                    .get::<Crew>(fixture.ship)
                    .unwrap()
                    .support_fraction,
                1.0
            );
        }
        fixture.set_inventory(|inventory| inventory.quantities[supplies] = 0);
        step(&mut fixture);
        assert_eq!(
            fixture
                .app
                .world()
                .get::<Crew>(fixture.ship)
                .unwrap()
                .support_fraction,
            0.0
        );
    }
    #[test]
    fn dock_resupply_requires_opt_in_and_conserves_inventory_and_stored_mass() {
        use crate::sim::identity::{Control, IdentityIndex};
        use crate::sim::travel::StoredMass;
        use osg_model::Id;
        let mut fixture = HardwareFixture::standard();
        let cat = fixture.app.world().resource::<ShipCatalogue>().0.clone();
        let mut source = Inventory::empty(&cat);
        source.cargo[0] = 100;
        source.energy_j = 1000;
        let mut target = Inventory::empty(&cat);
        target.tank_capacities_m3[0] = 1.;
        let owner = Id::new();
        crate::sim::identity::initialize(fixture.app.world_mut(), &[owner]);
        let guest_id = Id::new();
        let guest = fixture
            .app
            .world_mut()
            .spawn((
                ShipDesign(fixture.design.clone()),
                AssetOwner(osg_model::ownership::Principal::Player(owner)),
                Control {
                    account: owner,
                    revision: 1,
                },
                ShipInventory(target),
                Dormant,
                MassProps {
                    mass: 1000.,
                    inertia: bevy::math::DMat3::IDENTITY,
                    inertia_inv: bevy::math::DMat3::IDENTITY,
                },
            ))
            .id();
        fixture.app.world_mut().entity_mut(fixture.ship).insert((
            AssetOwner(osg_model::ownership::Principal::Player(owner)),
            Control {
                account: owner,
                revision: 1,
            },
            ShipInventory(source),
            DockServices {
                cargo_kg_s: 100.,
                power_w: 1000.,
            },
            StoredMass(1000.),
            DockingBays(vec![Bay {
                centre_m: [0.; 3],
                rotation: [0., 0., 0., 1.],
                radius_m: 100.,
                mass_capacity_kg: 1e9,
                public: false,
                allowed: Default::default(),
                reservation: None,
            }]),
        ));
        fixture
            .app
            .world_mut()
            .entity_mut(guest)
            .insert(crate::sim::travel::DockedIn(fixture.ship));
        fixture.app.world_mut().init_resource::<IdentityIndex>();
        fixture
            .app
            .world_mut()
            .entity_mut(guest)
            .insert(crate::sim::identity::Identity(guest_id));
        fixture
            .app
            .world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        fixture
            .app
            .world_mut()
            .run_system_once(service_docked)
            .unwrap();
        assert_eq!(
            fixture
                .app
                .world()
                .get::<ShipInventory>(guest)
                .unwrap()
                .0
                .cargo[0],
            0
        );
        fixture
            .app
            .world_mut()
            .entity_mut(guest)
            .insert(DockServiceRequest {
                cargo: true,
                power: true,
            });
        fixture
            .app
            .world_mut()
            .run_system_once(service_docked)
            .unwrap();
        let src = &fixture
            .app
            .world()
            .get::<ShipInventory>(fixture.ship)
            .unwrap()
            .0;
        let dst = &fixture.app.world().get::<ShipInventory>(guest).unwrap().0;
        assert_eq!(src.cargo[0] + dst.cargo[0], 100);
        assert_eq!(dst.cargo[0] as f64 * cat.resources[0].mass_kg, 10.);
        assert_eq!(src.energy_j + dst.energy_j, 1000);
        assert_eq!(dst.energy_j, 100);
        assert_eq!(
            fixture
                .app
                .world()
                .get::<StoredMass>(fixture.ship)
                .unwrap()
                .0,
            1010.
        );
        let world = fixture.app.world_mut();
        let organization = ownership::organization_id("Helion Flight Cooperative");
        world.entity_mut(fixture.ship).insert(AssetOwner(
            osg_model::ownership::Principal::Organization(organization),
        ));
        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization)
            .unwrap()
            .officers
            .insert(owner);
        ownership::affiliate(world, owner, None).unwrap();
        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization)
            .unwrap()
            .officers
            .remove(&owner);
        let before = world.get::<ShipInventory>(guest).unwrap().0.clone();
        world.run_system_once(service_docked).unwrap();
        assert_eq!(
            world.get::<ShipInventory>(guest).unwrap().0.cargo,
            before.cargo
        );
        assert_eq!(
            world.get::<ShipInventory>(guest).unwrap().0.energy_j,
            before.energy_j
        );
        world
            .entity_mut(fixture.ship)
            .insert(AssetAccess(osg_model::ownership::AccessPolicy {
                public: [osg_model::ownership::Permission::TransferCargo].into(),
                grants: Vec::new(),
            }));
        world.run_system_once(service_docked).unwrap();
        assert!(world.get::<ShipInventory>(guest).unwrap().0.energy_j > before.energy_j);
    }
}
