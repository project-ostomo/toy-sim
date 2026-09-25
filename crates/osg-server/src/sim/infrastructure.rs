use super::{hardware, identity, ownership, physics, precision, registry, spatial, travel, vessel};
use anyhow::{Context, Result};
use bevy::{ecs::system::RunSystemOnce, math::DVec3, prelude::*};
use osg_model::{
    Id, industry,
    ownership::{AccessPolicy, Permission, Principal},
};
use std::{collections::BTreeMap, sync::Arc};

#[cfg(test)]
#[path = "infrastructure/docking_tests.rs"]
mod docking_tests;
mod navigation;
pub use navigation::{NavigationPublication, navigation_snapshot, publish_navigation};

#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct Landmark {
    pub system: Id,
    pub name: String,
}

/// Authentication is independent of whether the installation advertises itself publicly.
pub fn authenticated_navigation_beacon(world: &World, ship: Entity, beacon: Id) -> bool {
    let Ok(beacon) = identity::lookup(world, beacon) else {
        return false;
    };
    let Some(owner) = world.get::<ownership::AssetOwner>(ship) else {
        return false;
    };
    world
        .get::<identity::NavigationBeaconEmitter>(beacon)
        .is_some()
        && world.get::<travel::Dormant>(beacon).is_none()
        && ownership::principal_access(world, owner.0, beacon, Permission::Navigate)
}

pub fn spawn(world: &mut World, player: Entity) -> Result<()> {
    let owner = Id::new();
    let organization = ownership::organization_id("Helion Flight Cooperative");
    ownership::affiliate(world, owner, Some(organization))?;
    let player_pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    let velocity = world.get::<physics::Velocity>(player).unwrap().0;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = osg_ships::ShipBlueprint::from_bytes(include_bytes!(
        "../../../../assets/ships/neris-anchorage.ship"
    ))?
    .compile(&catalogue)?;
    let station_pose = precision::PreciseTransform {
        translation_um: player_pose
            .translation_um
            .offset_by(player_pose.rotation * DVec3::new(0.0, 0.0, -5000.0)),
        rotation: player_pose.rotation,
    };
    let station = vessel::spawn_ship(
        world,
        Arc::new(design),
        station_pose,
        velocity,
        "Neris Anchorage".into(),
    )?;
    identity::attach_ship(world, station, owner, osg_model::Id::new())?;
    let system = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .system_id_for_name("Helion system")
        .context("Helion system missing")?;
    world.entity_mut(station).insert((
        ownership::AssetOwner(Principal::Organization(organization)),
        Landmark {
            system: Id(system),
            name: "Neris Anchorage".into(),
        },
    ));
    world
        .run_system_once(hardware::initialize)
        .map_err(|error| anyhow::anyhow!("hardware initialization failed: {error:?}"))?;
    if let Some(mut bays) = world.get_mut::<travel::DockingBays>(station) {
        for bay in &mut bays.0 {
            bay.public = true;
        }
    }
    let capacity = world
        .get::<vessel::ShipDesign>(station)
        .unwrap()
        .0
        .capacity_m3;
    world
        .get_mut::<hardware::ShipInventory>(station)
        .unwrap()
        .0
        .insert_item(
            &industry::CargoItem::Resource("exotic_fuel".into()),
            10_000_000,
            capacity,
            &catalogue,
        )?;
    super::hardware::synchronize_mass(world, &[player]);

    let account = world.get::<identity::Control>(player).unwrap().account;
    let mut access = world
        .get::<ownership::AssetAccess>(station)
        .map(|access| access.0.clone())
        .unwrap_or_default();
    access.grants.push(osg_model::ownership::AccessGrant {
        principal: Principal::Player(account),
        permissions: [
            Permission::View,
            Permission::Industry,
            Permission::TransferCargo,
        ]
        .into(),
    });
    for stack in osg_ships::industry::starter_stock(&catalogue)? {
        world
            .get_mut::<hardware::ShipInventory>(station)
            .unwrap()
            .0
            .insert_item(&stack.item, stack.quantity, capacity, &catalogue)?;
    }
    let design = &world.get::<vessel::ShipDesign>(station).unwrap().0;
    let mut facility = super::industry::IndustrialFacility::from_design(design);
    facility.set_mine(super::industry::MineSource {
        output: industry::CargoItem::Resource("industrial_ore".into()),
        units_per_second: 10,
        remainder: 0,
        last_recipient: None,
    });
    world
        .entity_mut(station)
        .insert((ownership::AssetAccess(access), facility));
    hardware::synchronize_mass(world, &[station]);
    spawn_navigation_installations(world)?;
    hardware::utilities::refresh_emitters(world);
    Ok(())
}

fn operator(world: &mut World, sovereignty: &str) -> Result<(Id, Id)> {
    use osg_model::ownership::Organization;

    let name = match sovereignty {
        "USE" => "Unifleet Station Services".to_owned(),
        "Helion Commonwealth" => "Helion Flight Cooperative".to_owned(),
        "St Raphael Commonwealth" => "St Raphael Trade Confraternity".to_owned(),
        name => format!("{name} Navigation Services"),
    };
    let organization = ownership::organization_id(&name);
    world
        .resource_mut::<ownership::Directory>()
        .0
        .organizations
        .entry(organization)
        .or_insert_with(|| Organization {
            id: organization,
            name,
            sovereignty: ownership::sovereignty_id(sovereignty),
            officers: Default::default(),
            open_membership: false,
        });
    let account = ownership::principal_id("navigation operator", sovereignty);
    ownership::affiliate(world, account, Some(organization))?;
    identity::add_account(world, account, false);
    Ok((account, organization))
}

pub(crate) fn installation_frame(
    system: &osg_universe::universe::SystemDefinition,
    epoch: hifitime::Epoch,
) -> Result<(precision::PreciseTransform, DVec3)> {
    use super::orrery::BodyClass;

    let solver = &system.solver;
    let mut references: Vec<_> = solver
        .iter()
        .filter(|body| {
            matches!(
                body.class_params,
                BodyClass::Star { .. } | BodyClass::Barycenter
            )
        })
        .collect();
    references.sort_by(|a, b| {
        let rank = |body: &osg_universe::orrery_cfg::Body| {
            matches!(body.class_params, BodyClass::Barycenter)
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.mass.total_cmp(&a.mass))
    });

    for reference in references {
        let extent = solver
            .iter()
            .filter(|body| matches!(body.class_params, BodyClass::Star { .. }))
            .filter_map(|star| {
                let mut reach = star.radius;
                let mut body = star;
                loop {
                    if body.name == reference.name {
                        return Some(reach);
                    }
                    reach += body.orbit.semi_major * (1.0 + body.orbit.eccentricity);
                    body = solver.get_body(body.parent.as_ref()?)?;
                }
            })
            .fold(0.0, f64::max);
        let mut stable_radius = f64::INFINITY;
        let mut child = reference;
        let mut inner_extent = 0.0;
        while let Some(parent) = child.parent.as_ref().and_then(|name| solver.get_body(name)) {
            for companion in solver.iter().filter(|body| {
                body.parent.as_ref() == Some(&parent.name) && body.name != child.name
            }) {
                let separation = child.orbit.semi_major + companion.orbit.semi_major;
                let periapsis =
                    separation * (1.0 - child.orbit.eccentricity.max(companion.orbit.eccentricity));
                let bound = 0.1 * periapsis * (reference.mass / parent.mass).cbrt() - inner_extent;
                stable_radius = stable_radius.min(bound);
            }
            inner_extent += child.orbit.semi_major * (1.0 + child.orbit.eccentricity);
            child = parent;
        }
        let radius = (extent * 4.0).max(1.5e11).min(stable_radius * 0.5);
        if radius < extent * 4.0 || !radius.is_finite() {
            continue;
        }
        let centre = solver
            .solve_position(&reference.name, epoch)
            .context("system position missing")?;
        if centre.relative_to(solver.anchor).length() + radius > system.influence {
            continue;
        }
        let velocity = solver
            .solve_velocity(&reference.name, epoch)
            .context("system velocity missing")?;
        let speed = (physics::GRAVITATIONAL_CONSTANT * reference.mass / radius).sqrt();
        return Ok((
            precision::PreciseTransform {
                translation_um: centre.offset_by(DVec3::X * radius),
                ..Default::default()
            },
            velocity + DVec3::Y * speed,
        ));
    }
    anyhow::bail!("no orbital location inside influence of {}", solver.name)
}

fn navigation_design(
    catalogue: &osg_ships::Catalogue,
) -> Result<Arc<osg_ships::CompiledShipDesign>> {
    let mut blueprint = osg_ships::ShipBlueprint {
        name: "Navigation installation".into(),
        ..Default::default()
    };
    blueprint.attach("station_core_32m", 0, "", "", 0);
    blueprint.attach("directory_transmitter_48m", 1, "aft", "fore", 0);
    blueprint.attach("navigation_beacon_4m", 2, "left", "right", 0);
    blueprint.attach("reactor_hot_4m", 1, "left", "right", 0);
    blueprint.attach("radiator_32m", 4, "left", "right", 0);
    blueprint.attach("battery", 1, "top", "bottom", 0);
    blueprint.attach("command_2m", 1, "bottom", "top", 0);
    blueprint.parts[0].tanks = vec![
        osg_ships::Tank {
            resource: "reactor_fuel".into(),
            volume_m3: 2.0,
            initial_fill: 1.0,
        },
        osg_ships::Tank {
            resource: "spent_fuel".into(),
            volume_m3: 2.0,
            initial_fill: 0.0,
        },
    ];
    Ok(Arc::new(blueprint.compile(catalogue)?))
}

fn spawn_navigation_installations(world: &mut World) -> Result<()> {
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = navigation_design(&catalogue)?;
    let epoch = physics::sim_time(world.resource::<Time<Fixed>>());
    let mut operators = BTreeMap::new();
    for settlement in &osg_universe::civilization::map().systems {
        let system = universe
            .system_id_for_name(&settlement.name)
            .with_context(|| format!("unknown initial settlement {}", settlement.name))?;
        let definition = universe.resolve(system)?;
        let (pose, velocity) = installation_frame(&definition, epoch)?;
        let (account, organization) = if let Some(operator) = operators.get(&settlement.sovereignty)
        {
            *operator
        } else {
            let value = operator(world, &settlement.sovereignty)?;
            operators.insert(settlement.sovereignty.clone(), value);
            value
        };
        let mut state = osg_ships::ShipState::new(&design, &catalogue);
        state.inventory.energy_j = design.battery_j;
        let (mass, inertia) = state.mass_properties(&design, &catalogue);
        let name = format!("{} navigation beacon", settlement.name);
        let entity = world
            .spawn((
                vessel::Vessel {
                    vessel_name: name.clone().into(),
                },
                vessel::ShipDesign(design.clone()),
                hardware::bundle(&design, state),
                travel::Travel::default(),
                travel::PresenceState::default(),
                travel::StoredMass::default(),
                physics::AngularVelocity(DVec3::ZERO),
                pose,
                physics::Velocity(velocity),
                physics::aerodynamics::AeroModel::new(design.semi_axes),
                physics::MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
                spatial::SpatialBody {
                    radius_m: design.radius,
                    occludes: true,
                },
                super::sensors::Sensor::default(),
                Landmark {
                    system: Id(system),
                    name,
                },
            ))
            .id();
        identity::attach_ship(world, entity, account, osg_model::Id::new())?;
        world.entity_mut(entity).insert((
            ownership::AssetOwner(Principal::Organization(organization)),
            ownership::AssetAccess(AccessPolicy {
                public: if settlement.sovereignty == "USE" {
                    Default::default()
                } else {
                    [Permission::Navigate].into()
                },
                grants: if settlement.sovereignty == "USE" {
                    vec![osg_model::ownership::AccessGrant {
                        principal: Principal::Sovereignty(ownership::sovereignty_id("USE")),
                        permissions: [Permission::Navigate].into(),
                    }]
                } else {
                    Vec::new()
                },
            }),
        ));
    }
    world
        .run_system_once(hardware::initialize)
        .map_err(|error| anyhow::anyhow!("initialize navigation hardware: {error:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_binary_installations_remain_within_system_influence() {
        use osg_universe::orrery_cfg::{Body, BodyClass, Orbit, OrreryCfg};

        let root = Body {
            key: "root".into(),
            name: "Wide pair".into(),
            class_params: BodyClass::Barycenter,
            mass: 4e30,
            ..Default::default()
        };
        let mut bodies = vec![root];
        for (name, anomaly) in [("Primary", 0.0), ("Companion", 180.0)] {
            bodies.push(Body {
                key: name.into(),
                name: name.into(),
                parent: Some("Wide pair".into()),
                class_params: BodyClass::Star { lumens: 1e26 },
                mass: 2e30,
                radius: 1e9,
                orbit: Orbit {
                    semi_major: 3e15,
                    period: std::f64::consts::TAU
                        * (6e15_f64.powi(3) / (physics::GRAVITATIONAL_CONSTANT * 4e30)).sqrt(),
                    mean_anomaly: anomaly,
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        let universe = osg_universe::universe::Universe::init(OrreryCfg {
            key: "wide-system".into(),
            name: "Wide system".into(),
            bodies,
            position_um: Default::default(),
        })
        .unwrap();
        let system = universe.resolve_index(0).unwrap();
        let (pose, velocity) =
            installation_frame(&system, hifitime::Epoch::from_mjd_utc(0.0)).unwrap();
        assert!(velocity.is_finite());
        assert_eq!(
            universe.containing_segment(pose.translation_um, DVec3::ZERO),
            vec![0]
        );
    }

    #[test]
    fn navigation_permission_is_independent_of_public_directory_visibility() {
        let mut world = World::new();
        identity::initialize(&mut world, &[]);
        let ship = world
            .spawn(ownership::AssetOwner(Principal::Player(Id::new())))
            .id();
        let beacon = world
            .spawn((
                ownership::AssetOwner(Principal::Player(Id::new())),
                ownership::AssetAccess(AccessPolicy {
                    public: [Permission::Navigate].into(),
                    grants: Vec::new(),
                }),
                identity::NavigationBeaconEmitter,
            ))
            .id();
        let id = Id::new();
        identity::register(&mut world, beacon, id).unwrap();
        assert!(!identity::public_directory_emitter(&world, beacon));
        assert!(authenticated_navigation_beacon(&world, ship, id));

        world
            .get_mut::<ownership::AssetAccess>(beacon)
            .unwrap()
            .0
            .public
            .clear();
        assert!(!authenticated_navigation_beacon(&world, ship, id));
        world
            .get_mut::<ownership::AssetAccess>(beacon)
            .unwrap()
            .0
            .public
            .insert(Permission::Navigate);
        world
            .entity_mut(beacon)
            .remove::<identity::NavigationBeaconEmitter>();
        assert!(!authenticated_navigation_beacon(&world, ship, id));
    }

    #[test]
    fn seeded_navigation_design_has_both_real_transmitters() {
        use osg_ships::{Equipment, utilities::UtilityDef};
        let catalogue = osg_ships::Catalogue::builtin();
        let design = navigation_design(&catalogue).unwrap();
        assert!(design.parts.iter().any(|part| matches!(
            part.definition.equipment,
            Equipment::Utility {
                utility: UtilityDef::DirectoryTransmitter { .. }
            }
        )));
        assert!(design.parts.iter().any(|part| matches!(
            part.definition.equipment,
            Equipment::Utility {
                utility: UtilityDef::NavigationBeacon { .. }
            }
        )));
    }
}
