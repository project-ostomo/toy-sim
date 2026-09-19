use super::{hardware, identity, physics, precision, registry, spatial, travel, vessel};
use anyhow::Result;
use bevy::{ecs::system::RunSystemOnce, math::DVec3, prelude::*};
use std::sync::Arc;
use toy_sim_model::*;

pub(crate) mod exclusion;
mod navigation;
#[cfg(test)]
use navigation::catalogue;
pub use navigation::{NavigationPublication, navigation_snapshot, publish_navigation};
pub(crate) use navigation::{capture_navigation, restore_navigation};

#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct Landmark {
    pub system: Id,
    pub name: String,
}

#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct GateOrbit {
    pub system: usize,
    body: String,
    offset: DVec3,
    axis: DVec3,
    rate: f64,
    epoch_seconds: f64,
}

impl GateOrbit {
    pub(crate) fn pose(
        &self,
        universe: &toy_sim_universe::universe::Universe,
        epoch: hifitime::Epoch,
    ) -> Option<(GalacticPosition, DVec3)> {
        let seconds = (epoch - hifitime::Epoch::from_mjd_utc(0.0)).to_seconds();
        let angle = self.rate * (seconds - self.epoch_seconds);
        let offset = bevy::math::DQuat::from_axis_angle(self.axis, angle) * self.offset;
        let position = universe.solve_position(&self.body, epoch)?;
        let motion = universe.solve_velocity(&self.body, epoch)?;
        Some((
            position.offset_by(offset),
            motion + self.axis.cross(offset) * self.rate,
        ))
    }

    pub(crate) fn envelope(
        &self,
        universe: &toy_sim_universe::universe::Universe,
    ) -> Option<(GalacticPosition, f64)> {
        let system = universe.systems.get(self.system)?;
        let mut radius = self.offset.length();
        let mut body = system.solver.get_body(&self.body)?;
        loop {
            radius += body.orbit.semi_major * (1.0 + body.orbit.eccentricity);
            let Some(parent) = &body.parent else {
                break;
            };
            body = system.solver.get_body(parent)?;
        }
        Some((system.solver.anchor, radius))
    }

    pub(crate) fn valid(&self, universe: &toy_sim_universe::universe::Universe) -> bool {
        universe.system_for(&self.body) == Some(self.system)
            && self.offset.is_finite()
            && self.offset.length_squared() > 0.0
            && self.axis.is_normalized()
            && self.rate.is_finite()
            && self.rate >= 0.0
            && self.epoch_seconds.is_finite()
    }
}

pub fn move_gates(
    universe: Res<super::orrery::Universe>,
    time: Res<Time<Fixed>>,
    mut gates: Query<(
        &GateOrbit,
        &mut precision::PreciseTransform,
        &mut physics::Velocity,
    )>,
) {
    let epoch = physics::sim_time(&time);
    let mut references = std::collections::HashMap::new();
    for (orbit, mut pose, mut velocity) in &mut gates {
        let angle = orbit.rate * (time.elapsed_secs_f64() - orbit.epoch_seconds);
        let offset = bevy::math::DQuat::from_axis_angle(orbit.axis, angle) * orbit.offset;
        let reference = references.entry(orbit.body.as_str()).or_insert_with(|| {
            universe
                .solve_position(&orbit.body, epoch)
                .zip(universe.solve_velocity(&orbit.body, epoch))
        });
        if let Some((position, motion)) = *reference {
            pose.translation_um = position.offset_by(offset);
            velocity.0 = motion + orbit.axis.cross(offset) * orbit.rate;
        }
    }
}

pub fn enforce_exclusion(world: &mut World) {
    let mouths = exclusion::candidates(world);
    let mut unstable = Vec::new();
    for (entity, position, exclusion) in mouths {
        let candidates = travel::geometry::mouth_candidates(world, position, exclusion);
        for other in candidates {
            if entity == other {
                continue;
            }
            let overlap = match (
                world.get::<travel::Gate>(other),
                world.get::<precision::PreciseTransform>(other),
            ) {
                (Some(gate), Some(pose)) => {
                    gate.enabled
                        && pose.translation_um.relative_to(position).length()
                            < exclusion + gate.exclusion_m
                }
                _ => false,
            };
            if overlap {
                unstable.extend([entity, other]);
            }
        }
    }
    unstable.sort_unstable();
    unstable.dedup();
    for entity in unstable {
        world.get_mut::<travel::Gate>(entity).unwrap().enabled = false;
        travel::geometry::update(world, entity);
        warn!(
            ?entity,
            "Wormhole mouth lost stability: exclusion volumes overlap"
        );
    }
}

pub fn spawn(world: &mut World, player: Entity) -> Result<()> {
    let owner = Id::new();
    let organization = super::ownership::organization_id("Helion Flight Cooperative");
    super::ownership::affiliate(world, owner, Some(organization))?;
    let player_pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    let velocity = world.get::<physics::Velocity>(player).unwrap().0;
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let design = toy_sim_ships::ShipBlueprint::from_bytes(include_bytes!(
        "../../../../assets/ships/neris-anchorage.ship"
    ))?
    .compile(&world.resource::<vessel::ShipCatalogue>().0)?;
    let station_pose = precision::PreciseTransform {
        translation_um: player_pose
            .translation_um
            .offset_by(player_pose.rotation * DVec3::new(0., 0., -5000.)),
        rotation: player_pose.rotation,
    };
    let station = vessel::spawn_ship(
        world,
        Arc::new(design),
        station_pose,
        velocity,
        "Neris Anchorage".into(),
    )?;
    world
        .run_system_once(hardware::initialize)
        .map_err(|error| anyhow::anyhow!("hardware initialization failed: {error:?}"))?;
    identity::attach_ship(world, station, owner)?;
    world
        .entity_mut(station)
        .insert(super::ownership::AssetOwner(
            ownership::Principal::Organization(organization),
        ));
    world.entity_mut(station).insert((
        identity::BeaconEmitter,
        Landmark {
            system: registry::system_identity("Helion system"),
            name: "Neris Anchorage".into(),
        },
    ));
    world
        .get_mut::<identity::Transponder>(station)
        .unwrap()
        .0
        .labels
        .insert("Neris Anchorage".into());
    let design = &world.get::<vessel::ShipDesign>(station).unwrap().0;
    let bays = design
        .parts
        .iter()
        .filter_map(|part| {
            let toy_sim_ships::Equipment::Utility {
                utility:
                    toy_sim_ships::utilities::UtilityDef::Docking {
                        radius_m,
                        mass_capacity_kg,
                    },
            } = part.definition.equipment
            else {
                return None;
            };
            Some(travel::Bay {
                centre_m: (part.centre - design.centre).to_array(),
                rotation: bevy::math::DQuat::from_mat3(&part.rotation).to_array(),
                radius_m,
                mass_capacity_kg,
                public: true,
                allowed: Default::default(),
                reservation: None,
            })
        })
        .collect();
    world.entity_mut(station).insert(travel::DockingBays(bays));
    let resource = world
        .resource::<vessel::ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|r| r.id == "repair_material");
    if let Some(resource) = resource {
        world
            .get_mut::<hardware::ShipInventory>(station)
            .unwrap()
            .0
            .cargo[resource] = 20000;
        world
            .get_mut::<hardware::ShipInventory>(player)
            .unwrap()
            .0
            .cargo[resource] = 100;
    }

    let account = world.get::<identity::Control>(player).unwrap().account;
    super::industry::seed_demo(world, station, account)?;
    spawn_gates(world, &universe, player_pose, velocity)?;
    Ok(())
}

fn gate_operator(world: &mut World, sovereignty: &str) -> Result<(Id, Id)> {
    use toy_sim_model::ownership::Organization;

    let name = match sovereignty {
        "USE" => "Unifleet Station Services".to_owned(),
        "Helion Commonwealth" => "Helion Flight Cooperative".to_owned(),
        "St Raphael Commonwealth" => "St Raphael Trade Confraternity".to_owned(),
        name => format!("{name} Gate Services"),
    };
    let organization = super::ownership::organization_id(&name);
    world
        .resource_mut::<super::ownership::Directory>()
        .0
        .organizations
        .entry(organization)
        .or_insert_with(|| Organization {
            id: organization,
            name,
            sovereignty: super::ownership::sovereignty_id(sovereignty),
            officers: Default::default(),
            open_membership: false,
        });
    let account = super::ownership::principal_id("gate operator", sovereignty);
    super::ownership::affiliate(world, account, Some(organization))?;
    identity::add_account(world, account, false);
    Ok((account, organization))
}

pub fn gate_reference(system: &toy_sim_universe::universe::SystemDefinition) -> (&str, f64) {
    let mut stars: Vec<_> = system
        .solver
        .iter()
        .filter_map(|body| {
            if let super::orrery::BodyClass::Star { lumens } = body.class_params {
                Some((body, lumens))
            } else {
                None
            }
        })
        .collect();
    stars.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (star, _) in &stars {
        let radius = 1.5e11_f64.max(star.radius * 4.0);
        let mut stable_radius = f64::INFINITY;
        let mut child = *star;
        let mut inner_extent = 0.0;
        while let Some(parent) = child
            .parent
            .as_ref()
            .and_then(|name| system.solver.get_body(name))
        {
            let companion = system
                .solver
                .iter()
                .find(|body| body.parent.as_ref() == Some(&parent.name) && body.name != child.name);
            if let Some(companion) = companion {
                let separation = child.orbit.semi_major + companion.orbit.semi_major;
                let periapsis =
                    separation * (1.0 - child.orbit.eccentricity.max(companion.orbit.eccentricity));
                let bound = 0.1 * periapsis * (star.mass / parent.mass).cbrt() - inner_extent;
                stable_radius = stable_radius.min(bound);
            }
            inner_extent += child.orbit.semi_major * (1.0 + child.orbit.eccentricity);
            child = parent;
        }
        if radius + 1e7 < stable_radius {
            return (star.name.as_str(), radius);
        }
    }
    let stellar_extent = stars
        .iter()
        .map(|(body, _)| orbital_extent(system, body))
        .fold(0.0, f64::max);
    (
        system.root_name.as_str(),
        (stellar_extent * 4.0).max(1.5e11),
    )
}

fn orbital_extent<'a>(
    system: &'a toy_sim_universe::universe::SystemDefinition,
    body: &'a super::orrery::orrery_cfg::Body,
) -> f64 {
    let mut extent = body.radius;
    let mut ancestor = Some(body);
    while let Some(body) = ancestor {
        extent += body.orbit.semi_major * (1.0 + body.orbit.eccentricity);
        ancestor = body
            .parent
            .as_ref()
            .and_then(|parent| system.solver.get_body(parent));
    }
    extent
}

pub fn gate_activation_extent(system: &toy_sim_universe::universe::SystemDefinition) -> f64 {
    let (reference, radius) = gate_reference(system);
    orbital_extent(system, system.solver.get_body(reference).unwrap()) + radius + 1e10
}

fn spawn_gates(
    world: &mut World,
    universe: &toy_sim_universe::universe::Universe,
    player_pose: precision::PreciseTransform,
    player_velocity: DVec3,
) -> Result<()> {
    use anyhow::ensure;

    let map = toy_sim_universe::civilization::map();
    ensure!(
        universe.systems.len() == map.systems.len(),
        "inhabited map and universe differ"
    );
    let mut operators = std::collections::HashMap::new();
    for system in &map.systems {
        if !operators.contains_key(&system.sovereignty) {
            let operator = gate_operator(world, &system.sovereignty)?;
            operators.insert(system.sovereignty.clone(), operator);
        }
    }
    let mut degrees = vec![0; map.systems.len()];
    for link in &map.links {
        degrees[link.a] += 1;
        degrees[link.b] += 1;
    }
    let mut slots = vec![0; map.systems.len()];
    let epoch = physics::sim_time(world.resource::<Time<Fixed>>());
    let epoch_seconds = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    for link in &map.links {
        let ids = [
            registry::gate_identity(
                &map.systems[link.a].catalogue_id,
                &map.systems[link.b].catalogue_id,
            ),
            registry::gate_identity(
                &map.systems[link.b].catalogue_id,
                &map.systems[link.a].catalogue_id,
            ),
        ];
        for (side, index) in [link.a, link.b].into_iter().enumerate() {
            let system = &universe.systems[index];
            let settlement = &map.systems[index];
            ensure!(
                system.solver.name.as_str() == settlement.name,
                "map system order differs"
            );
            let remote = &universe.systems[if side == 0 { link.b } else { link.a }];
            let (owner, organization) = operators[&settlement.sovereignty];
            let slot = slots[index];
            slots[index] += 1;
            let starting_system = system.solver.name == "Helion system";
            let (gate_reference, orbit_radius) = gate_reference(system);
            let reference = if starting_system {
                super::scenario::INITIAL_SCENARIO.body
            } else {
                gate_reference
            };
            let body = universe.get_body(reference).unwrap();
            let centre = universe.solve_position(reference, epoch).unwrap();
            let body_velocity = universe.solve_velocity(reference, epoch).unwrap();
            let (offset, axis) = if starting_system {
                let first = player_pose
                    .translation_um
                    .offset_by(DVec3::new(20000., 0., -10000.));
                let base = first.relative_to(centre);
                let axis = base
                    .cross(player_velocity - body_velocity)
                    .normalize_or(DVec3::Z);
                let angle = slot as f64 * std::f64::consts::TAU / degrees[index] as f64;
                (bevy::math::DQuat::from_axis_angle(axis, angle) * base, axis)
            } else {
                let radius = orbit_radius;
                let angle = slot as f64 * 6e7 / radius;
                (DVec3::new(angle.cos(), angle.sin(), 0.0) * radius, DVec3::Z)
            };
            let position = centre.offset_by(offset);
            let radius = if starting_system {
                player_pose
                    .translation_um
                    .offset_by(DVec3::new(20000., 0., -10000.))
                    .relative_to(centre)
                    .length()
            } else {
                orbit_radius
            };
            let rate = (physics::GRAVITATIONAL_CONSTANT * body.mass / radius.powi(3)).sqrt();
            let name = format!("{} gate", remote.solver.name);
            let entity = world
                .spawn((
                    precision::PreciseTransform {
                        translation_um: position,
                        ..Default::default()
                    },
                    identity::Control {
                        account: owner,
                        revision: 1,
                    },
                    super::ownership::AssetOwner(ownership::Principal::Organization(organization)),
                    super::ownership::AssetAccess::default(),
                    identity::Transponder(IffIdentity {
                        owner,
                        faction: Some(organization),
                        labels: [name.clone()].into(),
                        enabled: true,
                        range_m: 1e12,
                    }),
                    identity::BeaconEmitter,
                    identity::FixedBeacon,
                    physics::Velocity(body_velocity + axis.cross(offset) * rate),
                    GateOrbit {
                        system: index,
                        body: reference.into(),
                        offset,
                        axis,
                        rate,
                        epoch_seconds,
                    },
                    spatial::SpatialBody {
                        radius_m: 256.,
                        occludes: false,
                    },
                    travel::Gate {
                        paired: ids[1 - side],
                        radius_m: 220.,
                        exclusion_m: 1e7,
                        enabled: true,
                    },
                    Landmark {
                        system: registry::system_identity(&system.solver.name),
                        name,
                    },
                ))
                .id();
            identity::register(world, entity, ids[side]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_slip_makes_terminus_affordable_and_respects_fuel_preference() {
        use toy_sim_model::travel::{Destination, Order, PlanningPreferences, Status, TravelState};
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        for _ in 0..100 {
            app.update();
        }
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let terminus = registry::system_identity("Terminus system");
        let destination = world
            .query::<(&identity::Identity, &Landmark)>()
            .iter(world)
            .find(|(_, landmark)| landmark.system == terminus)
            .unwrap()
            .0
            .0;
        let mut budgets = Vec::new();
        for fuel_priority in [1., 10., 100.] {
            let revision = app
                .world()
                .get::<travel::Travel>(player)
                .unwrap()
                .0
                .revision
                + 1;
            app.world_mut()
                .entity_mut(player)
                .insert(travel::Travel(TravelState {
                    autopilot_enabled: true,
                    revision,
                    preferences: PlanningPreferences { fuel_priority },
                    orders: vec![Order::TravelTo(Destination::Beacon(destination)).into()],
                    status: Status::Planning,
                    ..Default::default()
                }));
            let mut planned = None;
            let mut observed_progress = None;
            let mut last_progress_tick = 0;
            for elapsed in 0..600 {
                app.update();
                let state = &app.world().get::<travel::Travel>(player).unwrap().0;
                assert!(
                    app.world()
                        .get::<vessel::ShipSoftware>(player)
                        .unwrap()
                        .controller
                        .fault
                        .is_none()
                );
                if state.status == Status::Planning {
                    if state.planning != observed_progress {
                        observed_progress = state.planning.clone();
                        last_progress_tick = elapsed;
                    }
                    assert!(
                        elapsed < 10 || state.planning.is_some(),
                        "planning must report progress"
                    );
                    assert!(
                        elapsed - last_progress_tick < 150,
                        "route planning stopped making progress: {:?}",
                        state.planning
                    );
                }
                if state.status == Status::Active {
                    let budget = state.fuel_budget.clone().unwrap();
                    assert!(
                        budget.complete && !budget.resources.is_empty(),
                        "{budget:?}"
                    );
                    eprintln!(
                        "fuel priority {fuel_priority}: planning completed in {} ticks; {budget:?}",
                        elapsed + 1
                    );
                    planned = Some(budget);
                    break;
                }
            }
            budgets.push(planned.expect("route planning must finish"));
        }
        assert!(budgets.iter().all(|budget| !budget.exhausted()));
        let required = |budget: &toy_sim_model::travel::FuelBudget| {
            budget.resources.iter().map(|r| r.required_kg).sum::<f64>()
        };
        assert!(required(&budgets[2]) < required(&budgets[1]));
        assert!(required(&budgets[1]) < required(&budgets[0]));
    }

    #[test]
    fn stock_computer_plans_terminus_from_default_spawn() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        for _ in 0..100 {
            app.update();
        }
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let terminus = registry::system_identity("Terminus system");
        let destination = world
            .query::<(&identity::Identity, &Landmark)>()
            .iter(world)
            .find(|(_, landmark)| landmark.system == terminus)
            .unwrap()
            .0
            .0;
        world
            .entity_mut(player)
            .insert(travel::Travel(toy_sim_model::travel::TravelState {
                autopilot_enabled: true,
                revision: 1,
                orders: vec![
                    toy_sim_model::travel::Order::TravelTo(
                        toy_sim_model::travel::Destination::Beacon(destination),
                    ),
                    toy_sim_model::travel::Order::WaitUntil(999_999),
                ]
                .into_iter()
                .map(Into::into)
                .collect(),
                status: toy_sim_model::travel::Status::Planning,
                ..default()
            }));
        let mut observed_progress = None;
        let mut last_progress_tick = 0;
        for elapsed in 0..600 {
            app.update();
            let world = app.world();
            let software = world.get::<vessel::ShipSoftware>(player).unwrap();
            assert!(
                software.controller.fault.is_none(),
                "routing fault {:?}, gas used {} / {}",
                software.controller.fault,
                software.last_gas_used,
                software.last_gas_limit
            );
            let state = &world.get::<travel::Travel>(player).unwrap().0;
            if state.status == toy_sim_model::travel::Status::Planning {
                if state.planning != observed_progress {
                    observed_progress = state.planning.clone();
                    last_progress_tick = elapsed;
                }
                assert!(
                    elapsed < 10 || state.planning.is_some(),
                    "planning must report progress"
                );
                assert!(
                    elapsed - last_progress_tick < 150,
                    "route planning stopped making progress: {:?}",
                    state.planning
                );
            }
            if state.status == toy_sim_model::travel::Status::Active {
                assert!(!state.orders.is_empty());
                assert!(state.orders.iter().any(|order| matches!(
                    &order.action,
                    toy_sim_model::travel::Order::Slip { .. }
                )));
                let slip_index = state
                    .orders
                    .iter()
                    .position(|order| {
                        matches!(order.action, toy_sim_model::travel::Order::Slip { .. })
                    })
                    .unwrap();
                assert!(slip_index > 0);
                assert!(matches!(
                    state.orders[slip_index - 1].action,
                    toy_sim_model::travel::Order::Sublight(
                        toy_sim_model::travel::Destination::Relative {
                            reference: toy_sim_model::travel::Reference::Beacon(_),
                            axes: toy_sim_model::travel::Axes::Galactic,
                            ..
                        }
                    )
                ));
                assert!(!state.orders.windows(2).any(|pair| pair[0] == pair[1]));
                assert_eq!(
                    state.orders.last().map(|stage| &stage.action),
                    Some(&toy_sim_model::travel::Order::WaitUntil(999_999))
                );
                assert!(!state.orders.iter().any(|order| matches!(
                    &order.action,
                    toy_sim_model::travel::Order::TravelTo(_)
                )));
                assert!(
                    state.orders[..state.orders.len() - 1]
                        .iter()
                        .all(|stage| stage.estimated_duration_ticks.is_some())
                );
                let arrivals: Vec<_> = state
                    .stage_arrivals(0)
                    .into_iter()
                    .collect::<Option<_>>()
                    .unwrap();
                assert!(arrivals.windows(2).all(|pair| pair[0] <= pair[1]));
                assert!(state.revision > 1);
                eprintln!(
                    "Terminus planning completed in {} ticks after 100 idle ticks",
                    elapsed + 1
                );
                return;
            }
        }
        panic!(
            "Terminus route did not complete planning: {:?}",
            app.world().get::<travel::Travel>(player).unwrap().0
        );
    }

    #[test]
    fn overlapping_macromouths_cannot_remain_stable() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let mouths: Vec<_> = world
            .query_filtered::<Entity, With<travel::Gate>>()
            .iter(world)
            .take(2)
            .collect();
        let position = world
            .get::<precision::PreciseTransform>(mouths[0])
            .unwrap()
            .translation_um;
        world
            .get_mut::<precision::PreciseTransform>(mouths[1])
            .unwrap()
            .translation_um = position.offset_by(DVec3::X * 1e6);
        travel::geometry::refresh(world);
        enforce_exclusion(world);
        for mouth in mouths {
            assert!(!world.get::<travel::Gate>(mouth).unwrap().enabled);
        }
    }

    #[test]
    fn default_network_is_connected_and_fixed_gates_have_no_physics_bodies() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let catalogue = catalogue(world);
        assert_eq!(catalogue.systems.len(), 3000);
        assert_eq!(
            catalogue
                .beacons
                .iter()
                .filter(|b| b.gate_exit.is_some())
                .count(),
            toy_sim_universe::civilization::map().links.len() * 2
        );
        let network = toy_sim_model::navigation::GateNetwork::from_catalogue(&catalogue);
        let neighbors: std::collections::HashMap<_, _> = network
            .regions
            .iter()
            .map(|region| (region.id, &region.gates))
            .collect();
        let mut reachable = std::collections::HashSet::new();
        let mut pending = vec![catalogue.systems[0].id];
        while let Some(system) = pending.pop() {
            if reachable.insert(system) {
                pending.extend(neighbors[&system].iter().map(|gate| gate.destination));
            }
        }
        assert_eq!(reachable.len(), catalogue.systems.len());
        for system in &catalogue.systems {
            let mouths: Vec<_> = catalogue
                .beacons
                .iter()
                .filter(|b| b.system == system.id && b.gate_exit.is_some())
                .collect();
            assert!((1..=6).contains(&mouths.len()));
            assert!(system.sovereignty.is_some());
            assert!(system.population > 0);
            for pair in mouths.windows(2) {
                assert!(
                    pair[0]
                        .pose
                        .position
                        .relative_to(pair[1].pose.position)
                        .length()
                        > 2e7
                );
            }
        }
        let gates: Vec<_> = world
            .query_filtered::<Entity, With<travel::Gate>>()
            .iter(world)
            .collect();
        for gate in gates {
            assert!(world.get::<physics::RigidBody>(gate).is_none());
            assert!(world.get::<physics::AccumulatedForce>(gate).is_none());
            assert!(world.get::<GateOrbit>(gate).is_some());
        }
        let station = catalogue.beacons.iter().find(|b| b.docking).unwrap();
        let station = identity::lookup(world, station.id).unwrap();
        assert!(world.get::<travel::DockingBays>(station).unwrap().0[0].public);
    }

    #[test]
    fn generated_system_activation_gate_hops_and_checkpoint_preserve_topology() {
        let account = Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let player_id = world.get::<identity::Identity>(player).unwrap().0;
        let universe = world
            .resource::<registry::UniverseRegistry>()
            .universe
            .clone();
        let generated = universe
            .systems
            .iter()
            .enumerate()
            .skip(10)
            .find(|(_, system)| {
                system.solver.iter().any(|body| {
                    matches!(
                        body.class_params,
                        super::super::orrery::BodyClass::Barycenter
                    )
                })
            })
            .map(|(index, _)| index)
            .unwrap();
        let mut system = generated;
        for _ in 0..3 {
            let world = app.world_mut();
            let entry = world
                .query::<(Entity, &GateOrbit)>()
                .iter(world)
                .find(|(_, orbit)| orbit.system == system)
                .unwrap()
                .0;
            let entry_pose = *world.get::<precision::PreciseTransform>(entry).unwrap();
            let entry_velocity = world.get::<physics::Velocity>(entry).unwrap().0;
            let exit =
                identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired).unwrap();
            let next_system = world.get::<GateOrbit>(exit).unwrap().system;
            let aperture = world.get::<travel::Gate>(entry).unwrap().radius_m;
            world
                .get_mut::<precision::PreciseTransform>(player)
                .unwrap()
                .translation_um = entry_pose
                .translation_um
                .offset_by(DVec3::X * (aperture + 1.0));
            world.get_mut::<physics::Velocity>(player).unwrap().0 =
                entry_velocity - DVec3::X * 50.0;
            world.get_mut::<travel::Travel>(player).unwrap().0 = Default::default();
            app.update();
            let world = app.world();
            let exit_pose = world
                .get::<precision::PreciseTransform>(exit)
                .unwrap()
                .translation_um;
            let pose = world
                .get::<precision::PreciseTransform>(player)
                .unwrap()
                .translation_um;
            assert!(
                pose.relative_to(exit_pose).length() < 1000.0,
                "physical gate passage did not reach exit: entry_system={system} destination_system={next_system} entry_distance={} exit_distance={} entry_enabled={} exit_enabled={} active={:?} velocity={:?} hull={:?} influence={} orbit_radius={}",
                pose.relative_to(entry_pose.translation_um).length(),
                pose.relative_to(exit_pose).length(),
                world.get::<travel::Gate>(entry).unwrap().enabled,
                world.get::<travel::Gate>(exit).unwrap().enabled,
                world
                    .resource::<super::super::orrery::activity::ActiveSystems>()
                    .entities
                    .keys(),
                world.get::<physics::Velocity>(player).unwrap().0 - entry_velocity,
                world.get::<hardware::Hull>(player).unwrap().0,
                universe.systems[system].influence,
                entry_pose
                    .translation_um
                    .relative_to(universe.systems[system].solver.anchor)
                    .length()
            );
            app.update();
            let active = app
                .world()
                .resource::<super::super::orrery::activity::ActiveSystems>();
            assert!(active.entities.contains_key(&next_system));
            assert!(active.entities.len() <= 3);
            system = next_system;
        }
        let world = app.world_mut();
        for celestial in world
            .query::<&super::super::orrery::activity::CelestialState>()
            .iter(world)
        {
            assert!(!matches!(
                celestial.body.class_params,
                super::super::orrery::BodyClass::Barycenter
            ));
        }
        let active = world.resource::<super::super::orrery::activity::ActiveSystems>();
        let optical = world.resource::<spatial::SpatialIndex>();
        for (entity, orbit) in world
            .iter_entities()
            .filter_map(|entity| Some((entity.id(), entity.get::<GateOrbit>()?)))
        {
            assert_eq!(
                optical.object_index(entity).is_some(),
                active.entities.contains_key(&orbit.system)
            );
        }
        publish_navigation(world);
        let before = catalogue(world);
        let saved = crate::persistence::world::capture(world).unwrap();
        crate::persistence::world::restore(world, &saved).unwrap();
        let after = catalogue(world);
        assert_eq!(before, after);
        assert!(identity::lookup(world, player_id).is_ok());
        app.update();
        assert_eq!(
            catalogue(app.world_mut()).topology_revision,
            before.topology_revision
        );
    }

    #[test]
    #[ignore]
    fn inhabited_map_snapshot_and_tick_profile() {
        use bevy::ecs::system::RunSystemOnce;
        use std::{io::Write, time::Instant};
        let account = Id::new();
        let started = Instant::now();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let startup_ms = started.elapsed().as_secs_f64() * 1000.0;
        for _ in 0..5 {
            app.update();
        }
        let session = super::super::session::connect(app.world_mut(), account).unwrap();
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
        encoder.window_log(21).unwrap();
        let mut tick_ms = Vec::new();
        let mut publication_ms = Vec::new();
        let mut compression_ms = Vec::new();
        let mut wire_bytes = Vec::new();
        let mut raw_bytes = 0;
        for _ in 0..30 {
            let started = Instant::now();
            app.update();
            tick_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            publish_navigation(app.world_mut());
            let frame = super::super::session::frame(app.world_mut(), session).unwrap();
            let bytes = toy_sim_protocol::encode(&toy_sim_protocol::Message::State(frame)).unwrap();
            publication_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            raw_bytes = bytes.len();
            let started = Instant::now();
            let before = encoder.get_ref().len();
            encoder.write_all(&bytes).unwrap();
            encoder.flush().unwrap();
            compression_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            wire_bytes.push(encoder.get_ref().len() - before);
        }
        let started = Instant::now();
        for _ in 0..30 {
            app.world_mut().run_system_once(spatial::rebuild).unwrap();
        }
        let optical_ms = started.elapsed().as_secs_f64() * 1000.0 / 30.0;
        let mut stage_ms = Vec::new();
        let started = Instant::now();
        for _ in 0..30 {
            app.world_mut().run_system_once(move_gates).unwrap();
        }
        stage_ms.push((
            "move gates",
            started.elapsed().as_secs_f64() * 1000.0 / 30.0,
        ));
        let started = Instant::now();
        for _ in 0..30 {
            travel::geometry::refresh(app.world_mut());
        }
        stage_ms.push((
            "travel hash",
            started.elapsed().as_secs_f64() * 1000.0 / 30.0,
        ));
        let started = Instant::now();
        for _ in 0..30 {
            enforce_exclusion(app.world_mut());
        }
        stage_ms.push(("exclusion", started.elapsed().as_secs_f64() * 1000.0 / 30.0));
        let mut publication_schedule = Schedule::default();
        publication_schedule.add_systems(super::super::services::publish_indexes);
        publication_schedule.run(app.world_mut());
        let started = Instant::now();
        for _ in 0..30 {
            publication_schedule.run(app.world_mut());
        }
        stage_ms.push((
            "public indexes",
            started.elapsed().as_secs_f64() * 1000.0 / 30.0,
        ));
        eprintln!("map stage times: {stage_ms:?}");
        let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
        eprintln!(
            "inhabited map: startup={startup_ms:.2}ms tick_mean={:.2}ms tick_max={:.2}ms optical={optical_ms:.2}ms publication={:.2}ms compression={:.2}ms raw={raw_bytes} first_wire={} steady_wire={} mouths={} optical_objects={}",
            mean(&tick_ms),
            tick_ms.iter().copied().fold(0.0, f64::max),
            mean(&publication_ms),
            mean(&compression_ms),
            wire_bytes[0],
            wire_bytes[1..].iter().sum::<usize>() / 29,
            toy_sim_universe::civilization::map().links.len() * 2,
            app.world()
                .resource::<spatial::SpatialIndex>()
                .objects
                .len()
        );
    }

    #[test]
    fn docking_removes_physics_and_private_pose_follows_the_station() {
        let account = Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .single(world)
            .unwrap();
        let berth = travel::reserve_bay(world, player, station, 0).unwrap();
        world.entity_mut(player).insert((
            precision::PreciseTransform {
                translation_um: berth.position,
                rotation: bevy::math::DQuat::from_array(berth.rotation),
            },
            physics::Velocity(DVec3::from_array(berth.velocity)),
            physics::AngularVelocity(DVec3::ZERO),
        ));
        travel::dock(world, player, station, 0).unwrap();
        assert!(world.get::<physics::RigidBody>(player).is_none());
        world
            .get_mut::<precision::PreciseTransform>(station)
            .unwrap()
            .translation_um = world
            .get::<precision::PreciseTransform>(station)
            .unwrap()
            .translation_um
            .offset_by(DVec3::X * 1000.);
        let private = super::super::session::ship_pose(world, player).unwrap();
        assert!((private.position.relative_to(berth.position) - DVec3::X * 1000.).length() < 0.001);
        travel::geometry::refresh(world);
        let destination = private.position.offset_by(DVec3::Z * 1000.);
        world.get_mut::<travel::Travel>(player).unwrap().0 = toy_sim_model::travel::TravelState {
            autopilot_enabled: true,
            revision: 1,
            orders: vec![toy_sim_model::travel::Order::TravelTo(
                toy_sim_model::travel::Destination::Galactic(destination),
            )]
            .into_iter()
            .map(Into::into)
            .collect(),
            status: toy_sim_model::travel::Status::Planning,
            ..default()
        };
        travel::advance(world);
        assert!(world.get::<physics::RigidBody>(player).is_some());
        assert_eq!(world.get::<travel::Travel>(player).unwrap().0.order, 0);
        assert!(world.get::<travel::DockedIn>(player).is_none());
    }
    #[test]
    fn stock_computer_flies_from_the_starting_scenario_through_the_sol_gate() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let entry = world
            .query_filtered::<Entity, With<travel::Gate>>()
            .iter(world)
            .find(|entity| {
                world
                    .get::<Landmark>(*entity)
                    .is_some_and(|landmark| landmark.name == "Sol gate")
            })
            .unwrap();
        let exit =
            identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired).unwrap();
        let entry_id = world.get::<identity::Identity>(entry).unwrap().0;
        world
            .entity_mut(player)
            .insert(travel::Travel(toy_sim_model::travel::TravelState {
                autopilot_enabled: true,
                revision: 1,
                orders: vec![toy_sim_model::travel::Order::Jump(entry_id)]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                status: toy_sim_model::travel::Status::Planning,
                ..default()
            }));
        let departure = world
            .get::<precision::PreciseTransform>(player)
            .unwrap()
            .translation_um;
        let entry_position = world
            .get::<precision::PreciseTransform>(entry)
            .unwrap()
            .translation_um;
        assert!(departure.relative_to(entry_position).length() > 20_000.);
        for _ in 0..6000 {
            app.update();
            if app.world().get::<travel::Travel>(player).unwrap().0.status
                == toy_sim_model::travel::Status::Completed
            {
                let world = app.world();
                let position = world
                    .get::<precision::PreciseTransform>(player)
                    .unwrap()
                    .translation_um;
                let mouth = world
                    .get::<precision::PreciseTransform>(exit)
                    .unwrap()
                    .translation_um;
                let gate = world.get::<travel::Gate>(exit).unwrap();
                let radius = world.get::<vessel::ShipDesign>(player).unwrap().0.radius;
                assert!(position.relative_to(mouth).length() > gate.radius_m + radius);
                let relative_velocity = world.get::<physics::Velocity>(player).unwrap().0
                    - world.get::<physics::Velocity>(exit).unwrap().0;
                assert!(relative_velocity.length() <= toy_sim_model::travel::GATE_ENTRY_SPEED_M_S);
                assert!(position.relative_to(mouth).dot(relative_velocity) > 0.);
                let fuel = world
                    .resource::<vessel::ShipCatalogue>()
                    .0
                    .resources
                    .iter()
                    .position(|resource| resource.id == "micropulse_charge")
                    .unwrap();
                assert!(
                    world
                        .get::<hardware::ShipInventory>(player)
                        .unwrap()
                        .0
                        .quantities[fuel]
                        > 0
                );
                return;
            }
        }
        panic!(
            "gate jump did not complete: {:?}, nav {:?}, hull {:?}, inventory {:?}",
            app.world().get::<travel::Travel>(player).unwrap().0,
            app.world()
                .get::<vessel::ShipSoftware>(player)
                .unwrap()
                .controller
                .state
                .navigation,
            app.world().get::<hardware::Hull>(player).unwrap().0,
            app.world()
                .get::<hardware::ShipInventory>(player)
                .unwrap()
                .0
        );
    }

    #[test]
    fn terminus_route_keeps_flying_outward_after_the_first_gate_transit() {
        use toy_sim_model::travel::{Destination, Order, Status, TravelState};
        use toy_sim_ship_api::abi;

        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        for _ in 0..100 {
            app.update();
        }
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let entry = world
            .query::<(Entity, &Landmark)>()
            .iter(world)
            .find(|(_, landmark)| landmark.name == "Sol gate")
            .unwrap()
            .0;
        let exit =
            identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired).unwrap();
        let terminus = registry::system_identity("Terminus system");
        let destination = world
            .query::<(&identity::Identity, &Landmark)>()
            .iter(world)
            .find(|(_, landmark)| landmark.system == terminus)
            .unwrap()
            .0
            .0;
        world
            .get_mut::<vessel::ShipSoftware>(player)
            .unwrap()
            .controller
            .instrument_interest = abi::INTEREST_PATHS | abi::INTEREST_MARKERS;
        world.get_mut::<travel::Travel>(player).unwrap().0 = TravelState {
            autopilot_enabled: true,
            revision: 1,
            orders: vec![Order::TravelTo(Destination::Beacon(destination)).into()],
            status: Status::Planning,
            ..default()
        };

        let mut crossed_at = None;
        let mut arrival_distance = 0.0;
        let mut returned_at = None;
        for elapsed in 0..6500 {
            app.update();
            let world = app.world();
            let software = world.get::<vessel::ShipSoftware>(player).unwrap();
            let state = &world.get::<travel::Travel>(player).unwrap().0;
            assert!(
                software.controller.fault.is_none(),
                "computer fault at tick {elapsed}, crossed {crossed_at:?}, travel {state:?}: {:?}",
                software.controller.fault
            );
            let position = world
                .get::<precision::PreciseTransform>(player)
                .unwrap()
                .translation_um;
            let exit_pose = world
                .get::<precision::PreciseTransform>(exit)
                .unwrap()
                .translation_um;
            let distance = position.relative_to(exit_pose).length();
            if crossed_at.is_none() && distance < 1000.0 {
                crossed_at = Some(elapsed);
                arrival_distance = distance;
                eprintln!("First transit at tick {elapsed}: {state:?}");
            }
            if let Some(crossed) = crossed_at {
                if distance >= 1e9 && returned_at.is_none() {
                    returned_at = Some(elapsed);
                    eprintln!("Unexpected return at tick {elapsed}: {state:?}");
                }
                assert!(
                    state.autopilot_enabled,
                    "autopilot stopped after transit: {state:?}"
                );
                if elapsed % 100 == 0 {
                    let velocity = world.get::<physics::Velocity>(player).unwrap().0
                        - world.get::<physics::Velocity>(exit).unwrap().0;
                    eprintln!(
                        "Post-transit tick {elapsed}: range {distance}, velocity {velocity}, {state:?}"
                    );
                }
                if elapsed >= crossed + 400 {
                    assert!(
                        returned_at.is_none(),
                        "ship returned through exit at {returned_at:?}"
                    );
                    let coast_distance = toy_sim_model::travel::GATE_ENTRY_SPEED_M_S * 40.0;
                    assert!(
                        distance > arrival_distance + coast_distance + 1000.0,
                        "exit transfer did not accelerate away: range {distance}, {state:?}"
                    );
                    assert_eq!(state.status, Status::Active);
                    assert!(state.order > 0);
                    return;
                }
            }
        }
        panic!(
            "first gate transit did not occur: {:?}",
            app.world().get::<travel::Travel>(player).unwrap().0
        );
    }

    #[test]
    fn stock_computer_docks_from_default_spawn_without_entering_station() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .single(world)
            .unwrap();
        let hostile: Vec<_> = world
            .query_filtered::<Entity, (
                With<vessel::Vessel>,
                Without<Landmark>,
                Without<vessel::ControlledVessel>,
            )>()
            .iter(world)
            .collect();
        for ship in hostile {
            world.despawn(ship);
        }
        let station_id = world.get::<identity::Identity>(station).unwrap().0;
        world
            .entity_mut(player)
            .insert((travel::Travel(toy_sim_model::travel::TravelState {
                autopilot_enabled: true,
                revision: 1,
                orders: vec![toy_sim_model::travel::Order::Dock(station_id)]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                status: toy_sim_model::travel::Status::Planning,
                ..default()
            }),));
        let minimum_distance = world.get::<vessel::ShipDesign>(station).unwrap().0.radius
            + world.get::<vessel::ShipDesign>(player).unwrap().0.radius;
        for _ in 0..3000 {
            app.update();
            if matches!(
                app.world().get::<travel::PresenceState>(player).unwrap().0,
                toy_sim_model::travel::Presence::Docked { .. }
            ) {
                return;
            }
            let world = app.world();
            let player_pose = super::super::session::ship_pose(world, player).unwrap();
            let station_pose = super::super::session::ship_pose(world, station).unwrap();
            assert!(
                player_pose
                    .position
                    .relative_to(station_pose.position)
                    .length()
                    >= minimum_distance,
                "autopilot entered station collision envelope: player {player_pose:?}, station {station_pose:?}, travel {:?}, navigation {:?}",
                world.get::<travel::Travel>(player).unwrap().0,
                world
                    .get::<vessel::ShipSoftware>(player)
                    .unwrap()
                    .controller
                    .state
                    .navigation,
            );
        }
        let world = app.world();
        let software = world.get::<vessel::ShipSoftware>(player).unwrap();
        panic!(
            "did not dock: {:?}, fault {:?}, navigation {:?}, hull {}, delta {:?}",
            world.get::<travel::Travel>(player).unwrap().0,
            software.controller.fault,
            software.controller.state.navigation,
            world.get::<hardware::Hull>(player).unwrap().0,
            super::super::session::ship_pose(world, player)
                .unwrap()
                .position
                .relative_to(
                    super::super::session::ship_pose(world, station)
                        .unwrap()
                        .position
                )
        );
    }
    #[test]
    fn docked_inventory_accepts_multiple_ships_and_transfers_only_cargo() {
        let account = Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .single(world)
            .unwrap();
        let design = world.get::<vessel::ShipDesign>(player).unwrap().0.clone();
        let other = vessel::spawn_ship(
            world,
            design,
            precision::PreciseTransform::default(),
            DVec3::ZERO,
            "Docked tender".into(),
        )
        .unwrap();
        identity::attach_ship(world, other, account).unwrap();
        for ship in [player, other] {
            let berth = travel::reserve_bay(world, ship, station, 0).unwrap();
            world.entity_mut(ship).insert((
                precision::PreciseTransform {
                    translation_um: berth.position,
                    rotation: bevy::math::DQuat::from_array(berth.rotation),
                },
                physics::Velocity(DVec3::from_array(berth.velocity)),
                physics::AngularVelocity(DVec3::ZERO),
            ));
            travel::dock(world, ship, station, 0).unwrap();
            assert!(world.get::<physics::RigidBody>(ship).is_none());
        }
        assert_eq!(world.get::<travel::StoredShips>(station).unwrap().len(), 2);
        let tanks = world
            .get::<hardware::ShipInventory>(player)
            .unwrap()
            .0
            .quantities
            .clone();
        let stored_mass = world.get::<travel::StoredMass>(station).unwrap().0;
        crate::sim::industry::transfer(
            world,
            account,
            player,
            other,
            toy_sim_model::industry::CargoItem::Resource("repair_material".into()),
            20,
        )
        .unwrap();
        assert_eq!(
            world
                .get::<hardware::ShipInventory>(player)
                .unwrap()
                .0
                .quantities,
            tanks
        );
        assert_eq!(
            world.get::<travel::StoredMass>(station).unwrap().0,
            stored_mass
        );
        assert!(
            crate::sim::industry::transfer(
                world,
                account,
                player,
                other,
                toy_sim_model::industry::CargoItem::Resource("water".into()),
                1
            )
            .is_err()
        );
    }
}
