pub mod geometry;
mod slip;

pub(crate) use slip::finish_arrivals;
pub use slip::{Preparation, SlipChargingPower, SlipDrive, Transit, prepare_slip};

#[cfg(test)]
mod router_tests;

#[cfg(test)]
mod undock_tests;

use super::{
    identity::pose,
    identity::{self, DirectoryEmitter, Identity, NavigationBeaconEmitter},
    orrery::activity::CelestialState,
    physics::{AngularVelocity, MassProps, Velocity},
    precision::PreciseTransform,
    simulation::SimulationCounters,
    vessel::ShipDesign,
};
use anyhow::{Result, ensure};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use osg_model::{AccountId, EntityId, GalacticPosition, Id, Pose, travel::*};
use std::collections::BTreeSet;

#[derive(Component, Default)]
pub struct Travel(pub TravelState);

#[derive(Debug)]
pub struct StaleOrder;

impl std::fmt::Display for StaleOrder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("stale or inactive order")
    }
}

impl std::error::Error for StaleOrder {}

#[derive(Component)]
pub struct PresenceState(pub Presence);

impl Default for PresenceState {
    fn default() -> Self {
        Self(Presence::Space)
    }
}

#[derive(Component)]
pub struct Dormant;

#[derive(Component)]
pub struct SystemsSuspended;

#[derive(Component, Default)]
pub struct StoredMass(pub f64);

#[derive(Component)]
#[relationship(relationship_target = StoredShips)]
pub struct DockedIn(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = DockedIn)]
pub struct StoredShips(Vec<Entity>);

#[derive(Component, Default)]
pub struct DockingBays(pub Vec<Bay>);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Bay {
    pub centre_m: [f64; 3],
    pub rotation: [f64; 4],
    pub radius_m: f64,
    pub mass_capacity_kg: f64,
    pub public: bool,
    pub allowed: BTreeSet<AccountId>,
    pub reservation: Option<(EntityId, u64)>,
}

#[derive(Resource, Default)]
pub struct TravelEvents(pub Vec<(Entity, &'static str, Option<GalacticPosition>)>);

pub fn cancel_pending(world: &mut World, ship: Entity) {
    if let Some(mut drive) = world.get_mut::<SlipDrive>(ship) {
        drive.preparation = None;
    }
}

fn tick(world: &World) -> u64 {
    world.resource::<SimulationCounters>().ticks
}
fn active(world: &World, entity: Entity) -> bool {
    world
        .get::<PresenceState>(entity)
        .is_none_or(|p| p.0 == Presence::Space)
        && world.get_entity(entity).is_ok()
}
fn id(world: &World, entity: Entity) -> Result<Id> {
    Ok(world
        .get::<Identity>(entity)
        .ok_or_else(|| anyhow::anyhow!("identity unavailable"))?
        .0)
}
fn entity(world: &World, id: Id) -> Result<Entity> {
    identity::lookup(world, id)
}
fn owner(world: &World, entity: Entity) -> Result<osg_model::ownership::Principal> {
    Ok(world
        .get::<super::ownership::AssetOwner>(entity)
        .ok_or_else(|| anyhow::anyhow!("owner unavailable"))?
        .0)
}
fn radius(world: &World, entity: Entity) -> Result<f64> {
    Ok(world
        .get::<ShipDesign>(entity)
        .ok_or_else(|| anyhow::anyhow!("ship unavailable"))?
        .0
        .radius)
}
fn ship_pose(world: &World, entity: Entity) -> Result<Pose> {
    let transform = world
        .get::<PreciseTransform>(entity)
        .ok_or_else(|| anyhow::anyhow!("position unavailable"))?;
    let mut result = pose(
        transform,
        world.get::<Velocity>(entity),
        world.get::<AngularVelocity>(entity),
    );
    if let Some(motion) = world.get::<DormantMotion>(entity) {
        result.velocity = motion.velocity.to_array();
        result.angular_velocity = motion.angular_velocity.to_array();
    }
    Ok(result)
}
fn write_pose(world: &mut World, entity: Entity, pose: Pose) {
    identity::renew_spatial_instance(world, entity);
    world.entity_mut(entity).insert((
        PreciseTransform {
            translation_um: pose.position,
            rotation: DQuat::from_array(pose.rotation),
        },
        Velocity(DVec3::from_array(pose.velocity)),
        AngularVelocity(DVec3::from_array(pose.angular_velocity)),
    ));
}
fn emit(world: &mut World, entity: Entity, kind: &'static str, position: Option<GalacticPosition>) {
    world
        .get_resource_or_init::<TravelEvents>()
        .0
        .push((entity, kind, position));
}
fn blocked(world: &mut World, entity: Entity, reason: String) {
    cancel_pending(world, entity);
    if let Some(mut travel) = world.get_mut::<Travel>(entity) {
        travel.0.planning = None;
        travel.0.status = Status::Blocked(reason);
        travel.0.estimated_arrival_tick = None;
    }
}

fn containment_depth(world: &World, root: Entity) -> Result<usize> {
    let mut pending = vec![(root, 0)];
    let mut seen = BTreeSet::new();
    let mut maximum = 0;
    while let Some((current, depth)) = pending.pop() {
        ensure!(
            seen.insert(current) && seen.len() <= 8192 && depth <= 8,
            "containment limit or cycle"
        );
        maximum = maximum.max(depth);
        if let Some(ships) = world.get::<StoredShips>(current) {
            pending.extend(ships.iter().map(|child| (child, depth + 1)));
        }
    }
    Ok(maximum)
}

pub fn reserve_bay(world: &mut World, ship: Entity, host: Entity, bay_id: u32) -> Result<Pose> {
    ensure!(
        ship != host && active(world, ship) && active(world, host),
        "ship or host unavailable"
    );
    ensure!(
        containment_depth(world, ship)? < 8,
        "containment depth exceeded"
    );
    let ship_id = id(world, ship)?;
    let account = owner(world, ship)?;
    let has_access = super::ownership::principal_access(
        world,
        account,
        host,
        osg_model::ownership::Permission::Dock,
    );
    let ship_radius = radius(world, ship)?;
    let mass = world
        .get::<MassProps>(ship)
        .ok_or_else(|| anyhow::anyhow!("mass unavailable"))?
        .mass;
    let host_pose = ship_pose(world, host)?;
    let now = tick(world);
    let mut bays = world
        .get_mut::<DockingBays>(host)
        .ok_or_else(|| anyhow::anyhow!("no docking bays"))?;
    let bay = bays
        .0
        .get_mut(bay_id as usize)
        .ok_or_else(|| anyhow::anyhow!("bay unavailable"))?;
    ensure!(
        bay.public
            || has_access
            || matches!(account, osg_model::ownership::Principal::Player(account) if bay.allowed.contains(&account)),
        "docking denied"
    );
    ensure!(
        ship_radius <= bay.radius_m && mass <= bay.mass_capacity_kg,
        "bay capacity exceeded"
    );
    ensure!(
        bay.reservation
            .is_none_or(|(reserved, expires)| reserved == ship_id || expires < now),
        "bay reserved"
    );
    bay.reservation = Some((ship_id, now + 600));
    Ok(bay_pose(&host_pose, bay))
}

pub fn dock(world: &mut World, ship: Entity, host: Entity, bay: u32) -> Result<()> {
    let target = ship_pose(world, host)?;
    let current = ship_pose(world, ship)?;
    let surface_gap = current.position.relative_to(target.position).length()
        - radius(world, host)?
        - radius(world, ship)?;
    ensure!(surface_gap <= DOCKING_CLEARANCE_M, "outside docking range");
    ensure!(
        (DVec3::from_array(current.velocity) - DVec3::from_array(target.velocity)).length()
            <= DOCKING_SPEED_M_S,
        "docking velocity too high"
    );
    reserve_bay(world, ship, host, bay)?;
    let mass = world.get::<MassProps>(ship).unwrap().mass;
    let host_id = id(world, host)?;
    set_dormant(world, ship, Presence::Docked { host: host_id, bay });
    add_stored_mass(world, host, mass);
    let mut bays = world.get_mut::<DockingBays>(host).unwrap();
    bays.0[bay as usize].reservation = None;
    world.entity_mut(ship).insert(DockedIn(host));
    let now = tick(world);
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        if matches!(
            travel
                .0
                .orders
                .get(travel.0.order)
                .map(|stage| &stage.action),
            Some(Order::Dock(_))
        ) {
            complete_order(&mut travel.0, now);
        }
    }
    emit(world, ship, "docked", None);
    Ok(())
}

pub(crate) fn construction_bay(
    world: &World,
    host: Entity,
    owner: osg_model::ownership::Principal,
    radius: f64,
    mass: f64,
) -> Result<u32> {
    ensure!(active(world, host), "shipyard unavailable");
    ensure!(
        world
            .get::<super::hardware::Hull>(host)
            .is_some_and(|hull| hull.0 > 0.),
        "shipyard destroyed"
    );
    ensure!(
        containment_depth(world, host)? < 8,
        "containment limit exceeded"
    );
    let bays = world
        .get::<DockingBays>(host)
        .ok_or_else(|| anyhow::anyhow!("shipyard has no docking aperture"))?;
    let bay = bays
        .0
        .iter()
        .position(|bay| {
            radius <= bay.radius_m
                && mass <= bay.mass_capacity_kg
                && super::ownership::port_access(
                    world,
                    owner,
                    host,
                    osg_model::ownership::Permission::Dock,
                    bay.public,
                    &bay.allowed,
                )
        })
        .ok_or_else(|| anyhow::anyhow!("no available authorized docking aperture"))?;
    Ok(bay as u32)
}

pub(crate) fn store_constructed(
    world: &mut World,
    ship: Entity,
    host: Entity,
    bay: u32,
) -> Result<()> {
    ensure!(ship != host, "cannot contain self");
    let mass = world
        .get::<MassProps>(ship)
        .ok_or_else(|| anyhow::anyhow!("constructed mass unavailable"))?
        .mass;
    let selected = construction_bay(world, host, owner(world, ship)?, radius(world, ship)?, mass)?;
    ensure!(
        selected == bay,
        "docking aperture changed during construction"
    );
    let host_id = id(world, host)?;
    set_dormant(world, ship, Presence::Docked { host: host_id, bay });
    world.entity_mut(ship).insert(DockedIn(host));
    add_stored_mass(world, host, mass);
    emit(world, ship, "constructed", None);
    Ok(())
}

fn add_stored_mass(world: &mut World, host: Entity, delta: f64) {
    let stored = world.get::<StoredMass>(host).map_or(0., |m| m.0);
    world
        .entity_mut(host)
        .insert(StoredMass((stored + delta).max(0.)));
    if let Some(mut mass) = world.get_mut::<MassProps>(host) {
        mass.mass = (mass.mass + delta).max(0.);
    }
}

pub fn collision_radius(world: &World, ship: Entity) -> Result<f64> {
    let design = world
        .get::<ShipDesign>(ship)
        .ok_or_else(|| anyhow::anyhow!("ship geometry unavailable"))?;
    // The collision voxelizer uses one-metre cells; a full diagonal bounds its padding.
    let voxel_diagonal_m = 3.0_f64.sqrt();
    Ok(osg_ships::thermal::shield_radius(
        design.0.radius + voxel_diagonal_m,
    ))
}

pub fn departure_pose(
    host: &Pose,
    bay_rotation: [f64; 4],
    host_radius: f64,
    ship_radius: f64,
) -> Pose {
    let mut pose = host.clone();
    let rotation = DQuat::from_array(host.rotation) * DQuat::from_array(bay_rotation);
    let displacement = rotation * DVec3::NEG_Z * (host_radius + ship_radius + DOCKING_CLEARANCE_M);

    pose.rotation = rotation.to_array();
    pose.position = host.position.offset_by(displacement);
    pose.velocity = (DVec3::from_array(host.velocity)
        + DVec3::from_array(host.angular_velocity).cross(displacement))
    .to_array();
    pose
}

pub fn undock_pose(world: &World, ship: Entity, host: Entity, bay: u32) -> Result<Pose> {
    let bay = world
        .get::<DockingBays>(host)
        .and_then(|bays| bays.0.get(bay as usize))
        .ok_or_else(|| anyhow::anyhow!("bay unavailable"))?;
    Ok(departure_pose(
        &ship_pose(world, host)?,
        bay.rotation,
        radius(world, host)?,
        radius(world, ship)?,
    ))
}

pub fn undock(world: &mut World, ship: Entity) -> Result<()> {
    ensure!(
        world
            .get::<super::hardware::Hull>(ship)
            .is_none_or(|hull| hull.0 > 0.),
        "destroyed ship cannot undock"
    );
    let presence = world
        .get::<PresenceState>(ship)
        .ok_or_else(|| anyhow::anyhow!("ship unavailable"))?
        .0
        .clone();
    let Presence::Docked { host, bay } = presence else {
        anyhow::bail!("ship is not docked");
    };
    let host = entity(world, host)?;
    ensure!(active(world, host), "host unavailable");
    let docking_bay = &world
        .get::<DockingBays>(host)
        .ok_or_else(|| anyhow::anyhow!("bay unavailable"))?
        .0[bay as usize];
    let account = owner(world, ship)?;
    ensure!(
        super::ownership::port_access(
            world,
            account,
            host,
            osg_model::ownership::Permission::Dock,
            docking_bay.public,
            &docking_bay.allowed
        ),
        "departure denied"
    );
    let target = undock_pose(world, ship, host, bay)?;
    let mass = world.get::<MassProps>(ship).unwrap().mass;
    add_stored_mass(world, host, -mass);
    world.entity_mut(ship).remove::<DockedIn>();
    write_pose(world, ship, target.clone());
    set_active(world, ship);
    emit(world, ship, "undocked", Some(target.position));
    Ok(())
}

pub fn clear_at(
    world: &mut World,
    ship: Entity,
    ignore: Option<Entity>,
    position: GalacticPosition,
    radius: f64,
) -> bool {
    let Some(candidates) = geometry::candidates(world, position, radius) else {
        return false;
    };
    let clear = candidates.into_iter().all(|other| {
        if other == ship || Some(other) == ignore || world.get::<Dormant>(other).is_some() {
            return true;
        }
        false
    });
    clear && celestial_conditions(world, position, radius).0
}

fn celestial_conditions(
    world: &mut World,
    position: GalacticPosition,
    radius: f64,
) -> (bool, bool) {
    if let Some(universe) = world.get_resource::<super::orrery::Universe>() {
        let epoch = slip::epoch(world);
        let mut clear = true;
        let mut outside = true;
        for system in universe.index.containing_segment(position, DVec3::ZERO) {
            let Ok(definition) = universe.resolve_index(system) else {
                return (false, false);
            };
            let solver = &definition.solver;
            for body in solver
                .iter()
                .filter(|body| !matches!(body.class_params, super::orrery::BodyClass::Barycenter))
            {
                if let Some(center) = solver.solve_position(&body.name, epoch) {
                    let distance = center.relative_to(position).length();
                    clear &= distance > body.radius + radius;
                    outside &=
                        distance > osg_model::travel::slip::exclusion_radius_m(body.mass) + radius;
                }
            }
        }
        return (clear, outside);
    }

    let Some(scene) = world.get_resource::<super::spatial::SpatialIndex>() else {
        return (false, false);
    };
    let Ok(candidates) = scene.hash.within_radius(position, radius, true) else {
        return (false, false);
    };
    candidates
        .into_iter()
        .filter_map(|key| {
            let super::spatial::SpatialKey::Entity(entity) = key else {
                return None;
            };
            world
                .get::<CelestialState>(entity)
                .map(|body| (scene.hash.get(&key).unwrap(), body))
        })
        .filter(|(_, body)| !matches!(body.body.class_params, super::orrery::BodyClass::Barycenter))
        .fold((true, true), |(clear, outside), (record, body)| {
            let distance = record.position.relative_to(position).length();
            (
                clear && distance > body.body.radius + radius,
                outside
                    && distance
                        > osg_model::travel::slip::exclusion_radius_m(body.body.mass) + radius,
            )
        })
}

pub fn slip_admissible(
    world: &mut World,
    ship: Entity,
    position: GalacticPosition,
    radius: f64,
) -> bool {
    clear_at(world, ship, None, position, radius) && celestial_conditions(world, position, radius).1
}

pub fn apply_plan(
    world: &mut World,
    ship: Entity,
    expected_revision: u64,
    plan: osg_model::routing::Plan,
    preferences: PlanningPreferences,
    engage: bool,
    goals: Vec<Order>,
    new_itinerary: bool,
) -> Result<()> {
    ensure!(preferences.valid(), "invalid planning preference");
    ensure!(
        plan.travel_revision == expected_revision,
        "stale route plan"
    );
    ensure!(plan.orders.len() <= 256, "invalid route length");
    ensure!(plan.fuel_budget.valid(), "invalid fuel budget");
    for stage in &plan.orders {
        osg_protocol::validate_queued_order(stage)?;
        ensure!(
            !matches!(stage.action, Order::TravelTo(_) | Order::TravelToSystem(_)),
            "route contains an unplanned destination"
        );
        ensure!(
            stage
                .estimated_propellant_kg
                .is_none_or(|kg| kg.is_finite() && kg >= 0.),
            "invalid fuel estimate"
        );
    }
    ensure!(
        world
            .get::<Travel>(ship)
            .is_some_and(|travel| travel.0.revision == expected_revision),
        "stale travel revision"
    );
    ensure!(
        world.get::<Transit>(ship).is_none(),
        "wait for slip arrival before changing the queue"
    );

    cancel_pending(world, ship);
    let now = tick(world);
    let enabled = engage || world.get::<Travel>(ship).unwrap().0.autopilot_enabled;
    let estimated_arrival_tick = enabled
        .then(|| {
            plan.orders
                .first()
                .and_then(|stage| stage.estimated_duration_ticks)
                .map(|duration| now.saturating_add(duration))
        })
        .flatten();
    let status = if plan.orders.is_empty() {
        Status::Completed
    } else if enabled {
        Status::Active
    } else {
        Status::Paused
    };
    let previous = &world.get::<Travel>(ship).unwrap().0;
    let risk_budget = if new_itinerary {
        RiskBudget::new(preferences.max_loss_ppm)
    } else {
        previous.risk_budget
    };
    let preferences = if new_itinerary {
        preferences
    } else {
        previous.preferences
    };
    world.get_mut::<Travel>(ship).unwrap().0 = TravelState {
        autopilot_enabled: enabled,
        preferences,
        fuel_budget: Some(plan.fuel_budget),
        revision: expected_revision.wrapping_add(1),
        orders: plan.orders,
        status,
        estimated_arrival_tick,
        risk_budget,
        goals,
        ..Default::default()
    };
    if let Some(mut software) = world.get_mut::<super::vessel::ShipSoftware>(ship) {
        software.schedule.wake();
    }
    Ok(())
}

fn active_order(world: &World, ship: Entity, revision: u64, index: usize) -> Result<&Order> {
    let travel = &world
        .get::<Travel>(ship)
        .ok_or_else(|| anyhow::anyhow!("travel unavailable"))?
        .0;
    if travel.revision != revision
        || travel.order != index
        || !travel.autopilot_enabled
        || travel.status != Status::Active
    {
        return Err(StaleOrder.into());
    }
    travel
        .orders
        .get(index)
        .map(|stage| &stage.action)
        .ok_or_else(|| anyhow::anyhow!("no active order"))
}

pub fn bay_pose(host: &Pose, bay: &Bay) -> Pose {
    let rotation = DQuat::from_array(host.rotation);
    let offset = rotation * DVec3::from_array(bay.centre_m);
    Pose {
        position: host.position.offset_by(offset),
        velocity: (DVec3::from_array(host.velocity)
            + DVec3::from_array(host.angular_velocity).cross(offset))
        .to_array(),
        rotation: (rotation * DQuat::from_array(bay.rotation)).to_array(),
        angular_velocity: host.angular_velocity,
    }
}

#[derive(Component)]
pub struct DormantMotion {
    pub velocity: DVec3,
    pub angular_velocity: DVec3,
    pub(crate) spatial: Option<super::spatial::SpatialBody>,
    pub(crate) rigid_body: bool,
    pub(crate) collision_body: bool,
}

impl DormantMotion {
    pub(crate) fn has_physical_body(&self) -> bool {
        self.spatial.is_some() || self.rigid_body || self.collision_body
    }
}

pub(crate) fn set_dormant(world: &mut World, ship: Entity, presence: Presence) {
    identity::renew_spatial_instance(world, ship);
    if matches!(presence, Presence::SlipTransit(_)) {
        world.entity_mut(ship).remove::<SystemsSuspended>();
    } else {
        super::hardware::shutdown(world, ship);
        world.entity_mut(ship).insert(SystemsSuspended);
    }
    let velocity = world.get::<Velocity>(ship).map_or(DVec3::ZERO, |v| v.0);
    let angular_velocity = world
        .get::<AngularVelocity>(ship)
        .map_or(DVec3::ZERO, |v| v.0);
    let spatial = world.get::<super::spatial::SpatialBody>(ship).copied();
    let rigid_body = world.get::<super::physics::RigidBody>(ship).is_some();
    let collision_body = world
        .get::<super::physics::collision::CollisionBody>(ship)
        .is_some();
    world
        .entity_mut(ship)
        .insert((
            Dormant,
            PresenceState(presence),
            DormantMotion {
                velocity,
                angular_velocity,
                spatial,
                rigid_body,
                collision_body,
            },
        ))
        .remove::<(
            Velocity,
            AngularVelocity,
            super::physics::RigidBody,
            super::physics::AccumulatedForce,
            super::physics::AccumulatedTorque,
            super::physics::WithinSoi,
            super::physics::collision::CollisionBody,
            super::spatial::SpatialBody,
            DirectoryEmitter,
            NavigationBeaconEmitter,
        )>();
}

fn set_active(world: &mut World, ship: Entity) {
    identity::renew_spatial_instance(world, ship);
    let motion = world.entity_mut(ship).take::<DormantMotion>();
    let velocity = world.get::<Velocity>(ship).map(|v| v.0);
    let angular = world.get::<AngularVelocity>(ship).map(|v| v.0);
    let mut entity = world.entity_mut(ship);
    entity
        .remove::<(Dormant, SystemsSuspended)>()
        .insert(PresenceState(Presence::Space));
    if let Some(motion) = motion {
        if motion.rigid_body {
            entity.insert(super::physics::RigidBody);
        }
        if motion.collision_body {
            entity.insert(super::physics::collision::CollisionBody);
        }
        entity.insert((
            Velocity(velocity.unwrap_or(motion.velocity)),
            AngularVelocity(angular.unwrap_or(motion.angular_velocity)),
        ));
        if let Some(spatial) = motion.spatial {
            entity.insert(spatial);
        }
    }
    super::hardware::wake(world, ship);
}

pub fn dispatch(world: &mut World, ship: Entity, action: osg_model::ProgramAction) -> Result<()> {
    use osg_model::ProgramAction;
    match action {
        ProgramAction::UseRoute {
            id,
            revision,
            engage,
        } => {
            if world
                .get::<Travel>(ship)
                .is_none_or(|state| state.0.revision != revision)
            {
                return Err(StaleOrder.into());
            }
            super::commands::use_route(world, ship, id, revision, engage)
        }
        ProgramAction::Estimate {
            revision,
            order,
            remaining_ticks,
            remaining_propellant_kg,
        } => {
            active_order(world, ship, revision, order)?;
            ensure!(
                remaining_propellant_kg.is_none_or(|kg| kg.is_finite() && kg >= 0.),
                "invalid fuel estimate"
            );
            let total = world
                .get::<Travel>(ship)
                .unwrap()
                .0
                .orders
                .iter()
                .skip(order + 1)
                .fold(remaining_propellant_kg, |sum, stage| {
                    sum.zip(stage.estimated_propellant_kg)
                        .map(|(sum, fuel)| sum + fuel)
                });
            let fuel_budget = super::route_service::fuel_budget(world, ship, total);
            let now = tick(world);
            let mut travel = world.get_mut::<Travel>(ship).unwrap();
            travel.0.estimated_arrival_tick =
                remaining_ticks.map(|remaining| now.saturating_add(remaining));
            travel.0.fuel_budget = Some(fuel_budget);
            Ok(())
        }
        ProgramAction::Block {
            revision,
            order,
            reason,
        } => {
            active_order(world, ship, revision, order)?;
            blocked(world, ship, reason.chars().take(256).collect());
            Ok(())
        }
        ProgramAction::CompleteOrder { revision, order } => {
            active_order(world, ship, revision, order)?;
            let now = tick(world);
            complete_order(&mut world.get_mut::<Travel>(ship).unwrap().0, now);
            Ok(())
        }
        ProgramAction::Slip {
            revision,
            order,
            destination,
            navigation_beacon,
        } => {
            ensure!(
                matches!(
                    active_order(world, ship, revision, order)?,
                    Order::Slip { .. }
                ),
                "current order is not slip transit"
            );
            prepare_slip(world, ship, destination, navigation_beacon)
        }
        ProgramAction::ReserveBay {
            revision,
            order,
            station,
            bay,
        } => {
            ensure!(
                matches!(active_order(world, ship, revision, order)?, Order::Dock(target) if *target == station),
                "current order is not docking here"
            );
            let host = entity(world, station)?;
            reserve_bay(world, ship, host, bay).map(|_| ())
        }
        ProgramAction::Dock {
            revision,
            order,
            station,
            bay,
        } => {
            ensure!(
                matches!(active_order(world, ship, revision, order)?, Order::Dock(target) if *target == station),
                "current order is not docking here"
            );
            let host = entity(world, station)?;
            dock(world, ship, host, bay)
        }
        ProgramAction::Undock { revision, order } => {
            ensure!(
                matches!(active_order(world, ship, revision, order)?, Order::Undock),
                "current order is not undocking"
            );
            undock(world, ship)?;
            let now = tick(world);
            complete_order(&mut world.get_mut::<Travel>(ship).unwrap().0, now);
            Ok(())
        }
    }
}

fn complete_order(travel: &mut TravelState, now: u64) {
    if let (Some(goal), Some(stage)) = (travel.goals.first(), travel.orders.get(travel.order)) {
        let completed = match (goal, &stage.action) {
            (
                Order::TravelToSystem(system),
                Order::Slip {
                    destination:
                        Destination::Relative {
                            reference: Reference::Celestial(reference),
                            ..
                        },
                    ..
                },
            ) => *system == reference.system,
            (Order::TravelTo(goal), Order::Sublight(destination)) => goal == destination,
            (goal, actual) => goal == actual,
        };
        if completed {
            travel.goals.remove(0);
        }
    }
    travel.order += 1;
    travel.revision = travel.revision.wrapping_add(1);
    travel.estimated_arrival_tick = travel
        .orders
        .get(travel.order)
        .and_then(|stage| stage.estimated_duration_ticks)
        .map(|duration| now.saturating_add(duration));
    travel.planning = None;
    travel.status = if travel.order >= travel.orders.len() {
        travel.fuel_budget = Some(FuelBudget {
            resources: Vec::new(),
            complete: true,
        });
        travel.estimated_arrival_tick = None;
        Status::Completed
    } else if travel.autopilot_enabled {
        Status::Active
    } else {
        travel.estimated_arrival_tick = None;
        Status::Paused
    };
}

pub fn plan_orders(world: &mut World) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("travel.plan_orders");
    use osg_model::routing::{Request, Status as RouteStatus};

    let planning: Vec<_> = world
        .query::<(Entity, &Travel)>()
        .iter(world)
        .filter(|(_, travel)| travel.0.status == Status::Planning)
        .map(|(ship, travel)| (ship, travel.0.clone()))
        .collect();
    let mut admissions = 0;
    for (ship, state) in planning {
        let request_id = state.revision.saturating_add(1);
        let status = super::route_service::poll_automatic(world, ship, request_id);
        let result = match status {
            Ok(RouteStatus::Unknown) if admissions < 8 => {
                admissions += 1;
                super::route_service::submit_automatic(
                    world,
                    ship,
                    Request {
                        id: request_id,
                        orders: state.goals.clone(),
                        preferences: state.preferences,
                    },
                )
                .map(|_| ())
            }
            Ok(RouteStatus::Unknown) => Ok(()),
            Ok(RouteStatus::Pending { progress }) => {
                world.get_mut::<Travel>(ship).unwrap().0.planning = Some(progress);
                Ok(())
            }
            Ok(RouteStatus::Ready { .. }) => {
                super::route_service::ready_automatic(world, ship, request_id, state.revision)
                    .and_then(|route| {
                        apply_plan(
                            world,
                            ship,
                            state.revision,
                            route.plan,
                            route.preferences,
                            false,
                            route.goals,
                            false,
                        )
                    })
            }
            Ok(RouteStatus::Failed { reason }) => Err(anyhow::anyhow!(reason)),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            blocked(world, ship, error.to_string());
        }
    }
}

pub fn advance(world: &mut World) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("travel.advance");
    for mut power in world.query::<&mut SlipChargingPower>().iter_mut(world) {
        power.0 = 0.;
    }
    let now = tick(world);
    let dormant_orders: Vec<_> = world
        .query::<(Entity, &PresenceState, &Travel)>()
        .iter(world)
        .filter(|(_, presence, travel)| {
            matches!(presence.0, Presence::Docked { .. })
                && travel.0.autopilot_enabled
                && travel.0.status == Status::Active
        })
        .map(|(entity, _, travel)| {
            (
                entity,
                travel
                    .0
                    .orders
                    .get(travel.0.order)
                    .map(|stage| &stage.action)
                    .cloned(),
            )
        })
        .collect();
    for (ship, order) in dormant_orders {
        let already_docked = match (&order, &world.get::<PresenceState>(ship).unwrap().0) {
            (Some(Order::Dock(station)), Presence::Docked { host, .. }) => station == host,
            _ => false,
        };
        let completes =
            already_docked || matches!(order, Some(Order::Undock | Order::WaitUntil(_)));
        let result = if already_docked {
            Some(Ok(()))
        } else {
            match order {
                Some(Order::WaitUntil(until)) => (now >= until).then_some(Ok(())),
                Some(_) => Some(undock(world, ship)),
                None => None,
            }
        };
        if let Some(result) = result {
            match result {
                Ok(()) if completes => {
                    complete_order(&mut world.get_mut::<Travel>(ship).unwrap().0, now)
                }
                Ok(()) => world.get_mut::<Travel>(ship).unwrap().0.status = Status::Active,
                Err(error) => blocked(world, ship, error.to_string()),
            }
        }
    }

    slip::advance(world);
}

pub fn destroy(world: &mut World, ship: Entity) {
    cancel_pending(world, ship);
    world.entity_mut(ship).remove::<Transit>();
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        travel.0.planning = None;
        travel.0.status = Status::Paused;
    }
    if let Ok(uuid) = id(world, ship) {
        let children: Vec<_> = world
            .get::<StoredShips>(ship)
            .into_iter()
            .flat_map(|ships| ships.iter())
            .collect();
        for child in children {
            world
                .entity_mut(child)
                .insert(PresenceState(Presence::StoredInWreck(uuid)));
        }
    }
    set_dormant(world, ship, Presence::Destroyed);
    world
        .entity_mut(ship)
        .remove::<(DirectoryEmitter, NavigationBeaconEmitter)>();
    emit(world, ship, "destroyed", None);
}

#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
pub(crate) struct DestructionTarget {
    design: &'static ShipDesign,
    identity: Option<&'static Identity>,
    velocity: Option<&'static Velocity>,
    angular: Option<&'static AngularVelocity>,
    spatial: Option<&'static super::spatial::SpatialBody>,
    rigid: Has<super::physics::RigidBody>,
    collision: Has<super::physics::collision::CollisionBody>,
    travel: Option<&'static mut Travel>,
    drive: Option<&'static mut SlipDrive>,
    stored: Option<&'static StoredShips>,
    parts: Option<&'static super::hardware::PartDevices>,
    settings: Option<&'static mut super::hardware::DeviceSettings>,
    avionics: Option<&'static mut super::hardware::Avionics>,
    sensors: Option<&'static mut super::hardware::SensorRange>,
    power: Option<&'static mut super::hardware::PowerFlow>,
    thermal: Option<&'static mut super::hardware::ShipThermal>,
}

pub(crate) fn destroy_collisions(
    mut commands: Commands,
    report: Res<super::physics::collision::CollisionReport>,
    mut targets: Query<DestructionTarget>,
    mut parts: Query<(
        &mut super::hardware::Device,
        Option<&mut super::hardware::Weapon>,
        Option<&mut super::hardware::DevicePower>,
    )>,
    mut observations: Query<(Entity, &mut super::sensors::Observations)>,
    mut spatial: ResMut<super::spatial::SpatialIndex>,
    mut events: ResMut<TravelEvents>,
) {
    for death in &report.report.destroyed {
        let entity = death.entity;
        spatial
            .hash
            .remove(&super::spatial::SpatialKey::Entity(entity));
        let Ok(mut target) = targets.get_mut(entity) else {
            commands.entity(entity).despawn();
            continue;
        };
        if let Some(travel) = target.travel.as_mut() {
            travel.0.planning = None;
            travel.0.status = Status::Paused;
        }
        if let Some(drive) = target.drive.as_mut() {
            drive.preparation = None;
        }
        if let Some(identity) = target.identity {
            for child in target.stored.into_iter().flat_map(|stored| stored.iter()) {
                commands
                    .entity(child)
                    .insert(PresenceState(Presence::StoredInWreck(identity.0)));
            }
        }
        if let Some(settings) = target.settings.as_mut() {
            settings.0 = super::hardware::default_settings(&target.design.0);
        }
        if let Some(avionics) = target.avionics.as_mut() {
            avionics.0.powered = false;
        }
        if let Some(sensors) = target.sensors.as_mut() {
            sensors.0 = 0.0;
        }
        if let Some(power) = target.power.as_mut() {
            **power = Default::default();
        }
        if let Some(thermal) = target.thermal.as_mut() {
            thermal.0.shield_enabled = false;
            thermal.0.shield_powered = false;
            thermal.0.shield_state = osg_ship_api::abi::SHIELD_OFF;
        }
        for part in target
            .parts
            .into_iter()
            .flat_map(|parts| parts.0.iter().copied())
        {
            commands
                .entity(part)
                .remove::<super::hardware::ActiveDevice>();
            if let Ok((mut device, weapon, power)) = parts.get_mut(part) {
                device.0.actual = 0.0;
                device.0.generated_w = 0.0;
                device.0.thrust_n = [0.0; 3];
                device.0.powered = false;
                if let Some(mut weapon) = weapon {
                    weapon.0.powered = false;
                    weapon.0.command = None;
                }
                if let Some(mut power) = power {
                    *power = Default::default();
                }
            }
        }
        for (observer, mut observation) in &mut observations {
            super::sensors::invalidate_observation(
                observer,
                &mut observation,
                entity,
                target.identity.map(|identity| identity.0),
            );
        }
        commands
            .entity(entity)
            .insert((
                Dormant,
                SystemsSuspended,
                PresenceState(Presence::Destroyed),
                identity::SpatialInstance(Id::new()),
                DormantMotion {
                    velocity: target.velocity.map_or(DVec3::ZERO, |velocity| velocity.0),
                    angular_velocity: target.angular.map_or(DVec3::ZERO, |angular| angular.0),
                    spatial: target.spatial.copied(),
                    rigid_body: target.rigid,
                    collision_body: target.collision,
                },
            ))
            .remove::<(
                Transit,
                Velocity,
                AngularVelocity,
                super::physics::RigidBody,
                super::physics::AccumulatedForce,
                super::physics::AccumulatedTorque,
                super::physics::WithinSoi,
                super::physics::collision::CollisionBody,
                super::spatial::SpatialBody,
                DirectoryEmitter,
                NavigationBeaconEmitter,
            )>();
        events.0.push((entity, "destroyed", None));
    }
}

pub fn debug_recover(world: &mut World, ship: Entity) -> Result<()> {
    let presence = world
        .get::<PresenceState>(ship)
        .map(|state| state.0.clone())
        .unwrap_or(Presence::Space);
    ensure!(
        matches!(presence, Presence::Space | Presence::Destroyed),
        "recover requires an uncontained ship"
    );
    if presence == Presence::Destroyed {
        ensure!(
            world
                .get::<StoredShips>(ship)
                .is_none_or(|ships| ships.is_empty())
                && world
                    .get::<StoredMass>(ship)
                    .is_none_or(|mass| mass.0 == 0.),
            "recover cannot restore occupied wreck inventory"
        );
        let design = world
            .get::<ShipDesign>(ship)
            .ok_or_else(|| anyhow::anyhow!("ship unavailable"))?
            .0
            .clone();
        let catalogue = &world.resource::<super::vessel::ShipCatalogue>().0;
        let mut state = osg_ships::ShipState::new(&design, catalogue);
        state.test_loadout(&design, catalogue);
        world
            .entity_mut(ship)
            .insert(super::hardware::PendingHardwareReset(state));
        set_active(world, ship);
    }
    cancel_pending(world, ship);
    world.entity_mut(ship).remove::<Transit>();
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        travel.0.planning = None;
        travel.0.status = Status::Paused;
    }
    if let Some(mut software) = world.get_mut::<super::vessel::ShipSoftware>(ship) {
        software.controller.reboot();
        software.inbox.clear();
        software.world_actions.clear();
        software.last_input = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::identity::Control;
    use std::sync::Arc;

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<identity::IdentityIndex>();
        world.init_resource::<super::super::services::PublishedWorld>();
        crate::sim::ownership::initialize(&mut world);
        world
    }

    fn ship(world: &mut World, position: DVec3, account: Id) -> Entity {
        crate::sim::ownership::add_account(world, account);
        let design = Arc::new(
            osg_ships::armed_starter()
                .compile(&osg_ships::Catalogue::builtin())
                .unwrap(),
        );
        let radius = design.radius;
        let entity = world
            .spawn((
                ShipDesign(design),
                crate::sim::ownership::AssetOwner(osg_model::ownership::Principal::Player(account)),
                PreciseTransform {
                    translation_um: GalacticPosition::default().offset_by(position),
                    rotation: DQuat::IDENTITY,
                },
                Velocity(DVec3::ZERO),
                AngularVelocity(DVec3::ZERO),
                MassProps {
                    mass: 1000.,
                    ..Default::default()
                },
                Control {
                    account,
                    revision: 1,
                },
                Travel::default(),
                PresenceState::default(),
                super::super::spatial::SpatialBody {
                    radius_m: radius,
                    occludes: true,
                },
            ))
            .id();
        identity::register(world, entity, Id::new());
        entity
    }

    #[test]
    fn station_transfer_revokes_private_docking_despite_unchanged_controller() {
        use osg_model::ownership::{AccessPolicy, Permission, Principal};
        let mut world = world();
        let account = Id::new();
        let station = ship(&mut world, DVec3::ZERO, account);
        let guest = ship(&mut world, DVec3::Z * 100., account);
        let mut private = bay();
        private.public = false;
        world.entity_mut(station).insert(DockingBays(vec![private]));
        assert!(reserve_bay(&mut world, guest, station, 0).is_ok());
        world
            .entity_mut(station)
            .insert(crate::sim::ownership::AssetOwner(Principal::Organization(
                crate::sim::ownership::organization_id("Unifleet Station Services"),
            )));
        assert_eq!(world.get::<Control>(station).unwrap().account, account);
        assert!(reserve_bay(&mut world, guest, station, 0).is_err());
        world
            .entity_mut(station)
            .insert(crate::sim::ownership::AssetAccess(AccessPolicy {
                public: [Permission::Dock].into(),
                grants: Vec::new(),
            }));
        assert!(reserve_bay(&mut world, guest, station, 0).is_ok());
    }

    fn bay() -> Bay {
        Bay {
            centre_m: [0., 0., 100.],
            rotation: DQuat::IDENTITY.to_array(),
            radius_m: 50.,
            mass_capacity_kg: 1e9,
            public: true,
            allowed: Default::default(),
            reservation: None,
        }
    }

    #[test]
    fn docking_accepts_any_side_and_attitude_at_surface_range_and_speed_limits() {
        for direction in [DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Z] {
            let mut world = world();
            let account = Id::new();
            let host = ship(&mut world, DVec3::ZERO, account);
            world.entity_mut(host).insert(DockingBays(vec![bay()]));
            let child = ship(&mut world, DVec3::ZERO, account);
            let capture_distance = radius(&world, host).unwrap()
                + radius(&world, child).unwrap()
                + DOCKING_CLEARANCE_M;
            world
                .get_mut::<PreciseTransform>(child)
                .unwrap()
                .translation_um =
                GalacticPosition::ZERO.offset_by(direction * (capture_distance + 0.01));
            assert!(
                dock(&mut world, child, host, 0)
                    .unwrap_err()
                    .to_string()
                    .contains("range")
            );
            world
                .get_mut::<PreciseTransform>(child)
                .unwrap()
                .translation_um =
                GalacticPosition::ZERO.offset_by(direction * (capture_distance - 0.00001));
            world.get_mut::<PreciseTransform>(child).unwrap().rotation = DQuat::from_rotation_y(2.);
            world.get_mut::<Velocity>(host).unwrap().0 = DVec3::Y * 100.;
            world.get_mut::<Velocity>(child).unwrap().0 = DVec3::Y * 110.01;
            assert!(
                dock(&mut world, child, host, 0)
                    .unwrap_err()
                    .to_string()
                    .contains("velocity")
            );
            world.get_mut::<Velocity>(child).unwrap().0 = DVec3::Y * 110.;
            dock(&mut world, child, host, 0).unwrap();
            assert!(world.get::<Velocity>(child).is_none());
        }
    }

    #[test]
    fn proximity_docking_still_checks_access_capacity_and_reservations() {
        let mut world = world();
        let account = Id::new();
        let host = ship(&mut world, DVec3::ZERO, account);
        let child = ship(&mut world, DVec3::Z * 100., Id::new());
        let mut restricted = bay();
        restricted.public = false;
        world.entity_mut(host).insert(DockingBays(vec![restricted]));
        assert!(
            dock(&mut world, child, host, 0)
                .unwrap_err()
                .to_string()
                .contains("denied")
        );
        world.get_mut::<DockingBays>(host).unwrap().0[0].public = true;
        world.get_mut::<DockingBays>(host).unwrap().0[0].mass_capacity_kg = 999.;
        assert!(
            dock(&mut world, child, host, 0)
                .unwrap_err()
                .to_string()
                .contains("capacity")
        );
        world.get_mut::<DockingBays>(host).unwrap().0[0].mass_capacity_kg = 1000.;
        world.get_mut::<DockingBays>(host).unwrap().0[0].reservation = Some((Id::new(), 600));
        assert!(
            dock(&mut world, child, host, 0)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
        world.get_mut::<DockingBays>(host).unwrap().0[0].reservation = None;
        dock(&mut world, child, host, 0).unwrap();
    }

    #[test]
    fn docking_removes_active_motion_and_undocking_inherits_host_rotation() {
        let mut world = world();
        let account = Id::new();
        let host = ship(&mut world, DVec3::ZERO, account);
        world.entity_mut(host).insert(DockingBays(vec![bay()]));
        let child = ship(&mut world, DVec3::Z * 100., account);
        dock(&mut world, child, host, 0).unwrap();
        assert!(world.get::<Velocity>(child).is_none());
        assert!(
            world
                .get::<super::super::spatial::SpatialBody>(child)
                .is_none()
        );
        assert_eq!(world.get::<StoredMass>(host).unwrap().0, 1000.);
        world.get_mut::<Velocity>(host).unwrap().0 = DVec3::X * 7.;
        world.get_mut::<AngularVelocity>(host).unwrap().0 = DVec3::Y * 0.1;
        undock(&mut world, child).unwrap();
        let position = world
            .get::<PreciseTransform>(child)
            .unwrap()
            .translation_um
            .to_meters_64();
        let expected = DVec3::X * 7. + (DVec3::Y * 0.1).cross(position);
        assert!((world.get::<Velocity>(child).unwrap().0 - expected).length() < 1e-6);
        assert!(world.get::<Dormant>(child).is_none());
        assert_eq!(world.get::<StoredMass>(host).unwrap().0, 0.);
    }

    #[test]
    fn nested_inventory_survives_host_destruction_and_capture_does_not_unlock_private_bay() {
        let mut world = world();
        let account = Id::new();
        let host = ship(&mut world, DVec3::ZERO, account);
        let mut private = bay();
        private.public = false;
        world.entity_mut(host).insert(DockingBays(vec![private]));
        let child = ship(&mut world, DVec3::Z * 100., account);
        dock(&mut world, child, host, 0).unwrap();
        world
            .entity_mut(child)
            .insert(crate::sim::ownership::AssetOwner(
                osg_model::ownership::Principal::Player(Id::new()),
            ));
        assert!(undock(&mut world, child).is_err());
        destroy(&mut world, host);
        assert!(matches!(
            world.get::<PresenceState>(child).unwrap().0,
            Presence::StoredInWreck(_)
        ));
        assert!(world.get::<ShipDesign>(child).is_some());
    }

    #[test]
    fn inactive_celestial_body_still_prevents_slip_arrival() {
        let mut world = world();
        let universe =
            super::super::orrery::Universe::init(osg_universe::example_config()).unwrap();
        let system = universe.resolve_index(0).unwrap();
        let body = system.solver.iter().next().unwrap().name.clone();
        let position = system
            .solver
            .solve_position(&body, slip::epoch(&world))
            .unwrap();
        world.insert_resource(universe);
        assert!(!celestial_conditions(&mut world, position, 1.).0);
        assert!(!celestial_conditions(&mut world, position, 0.).1);
    }

    #[test]
    fn debug_recovery_keeps_identity_and_rejects_occupied_wrecks() {
        let mut world = world();
        world.insert_resource(super::super::vessel::ShipCatalogue(
            osg_ships::Catalogue::builtin(),
        ));
        let account = Id::new();
        let host = ship(&mut world, DVec3::ZERO, account);
        world.entity_mut(host).insert(DockingBays(vec![bay()]));
        let child = ship(&mut world, DVec3::Z * 100., account);
        let child_id = id(&world, child).unwrap();
        destroy(&mut world, child);
        debug_recover(&mut world, child).unwrap();
        assert_eq!(id(&world, child).unwrap(), child_id);
        assert_eq!(
            owner(&world, child).unwrap(),
            osg_model::ownership::Principal::Player(account)
        );
        assert!(world.get::<Dormant>(child).is_none());
        assert!(
            world
                .get::<super::super::hardware::PendingHardwareReset>(child)
                .is_some()
        );
        dock(&mut world, child, host, 0).unwrap();
        destroy(&mut world, host);
        assert!(debug_recover(&mut world, host).is_err());
        assert!(debug_recover(&mut world, child).is_err());
    }

    #[test]
    fn nested_inventory_mass_moves_with_its_parent() {
        let mut world = world();
        let account = Id::new();
        let outer = ship(&mut world, DVec3::ZERO, account);
        let inner = ship(&mut world, DVec3::Z * 100., account);
        let cargo = ship(&mut world, DVec3::Z * 200., account);
        world.entity_mut(outer).insert(DockingBays(vec![bay()]));
        world.entity_mut(inner).insert(DockingBays(vec![bay()]));
        dock(&mut world, cargo, inner, 0).unwrap();
        dock(&mut world, inner, outer, 0).unwrap();
        assert_eq!(world.get::<StoredMass>(outer).unwrap().0, 2000.);
        assert!(undock(&mut world, cargo).is_err());
        undock(&mut world, inner).unwrap();
        assert_eq!(world.get::<StoredMass>(outer).unwrap().0, 0.);
        assert_eq!(world.get::<StoredMass>(inner).unwrap().0, 1000.);
        assert!(matches!(
            world.get::<PresenceState>(cargo).unwrap().0,
            Presence::Docked { .. }
        ));
    }

    #[test]
    fn spatial_lifetime_changes_survive_an_unpublished_departure_and_return() {
        let mut world = world();
        let ship = ship(&mut world, DVec3::ZERO, Id::new());
        let original = world.get::<identity::SpatialInstance>(ship).unwrap().0;
        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        set_active(&mut world, ship);
        let returned = world.get::<identity::SpatialInstance>(ship).unwrap().0;
        assert_ne!(original, returned);
        let pose = ship_pose(&world, ship).unwrap();
        write_pose(&mut world, ship, pose);
        assert_ne!(
            world.get::<identity::SpatialInstance>(ship).unwrap().0,
            returned
        );
    }
}
