pub mod geometry;

#[cfg(test)]
mod router_tests;

use super::{
    identity::{self, BeaconEmitter, Identity},
    intelligence::pose,
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
use std::collections::BTreeSet;
use toy_sim_model::{AccountId, EntityId, GalacticPosition, Id, Pose, travel::*};
use toy_sim_ships::StochasticRound;

pub const LIGHT_YEAR_M: f64 = 9.4607304725808e15;

#[derive(Component, Default)]
pub struct Travel(pub TravelState);

#[derive(Component)]
pub struct PresenceState(pub Presence);

impl Default for PresenceState {
    fn default() -> Self {
        Self(Presence::Space)
    }
}

#[derive(Component)]
pub struct Dormant;

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

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Gate {
    pub paired: EntityId,
    pub radius_m: f64,
    pub exclusion_m: f64,
    pub enabled: bool,
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SlipDrive {
    pub power_w: f64,
    pub ready_tick: u64,
    pub preparation: Option<Preparation>,
}

impl Default for SlipDrive {
    fn default() -> Self {
        Self {
            power_w: 100e6,
            ready_tick: 0,
            preparation: None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Preparation {
    pub destination: GalacticPosition,
    pub started: u64,
    pub mass: f64,
    pub work_j: f64,
    pub required_j: f64,
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Transit {
    pub origin: GalacticPosition,
    pub departed: u64,
    pub destination: GalacticPosition,
    pub next_attempt: u64,
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
fn owner(world: &World, entity: Entity) -> Result<toy_sim_model::ownership::Principal> {
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
    geometry::update(world, entity);
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
        toy_sim_model::ownership::Permission::Dock,
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
            || matches!(account, toy_sim_model::ownership::Principal::Player(account) if bay.allowed.contains(&account)),
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
    reserve_bay(world, ship, host, bay)?;
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
    let mass = world.get::<MassProps>(ship).unwrap().mass;
    let host_id = id(world, host)?;
    set_dormant(world, ship, Presence::Docked { host: host_id, bay });
    add_stored_mass(world, host, mass);
    let mut bays = world.get_mut::<DockingBays>(host).unwrap();
    bays.0[bay as usize].reservation = None;
    world.entity_mut(ship).insert(DockedIn(host));
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        if matches!(
            travel
                .0
                .orders
                .get(travel.0.order)
                .map(|stage| &stage.action),
            Some(Order::Dock(_))
        ) {
            complete_order(&mut travel.0);
        }
    }
    emit(world, ship, "docked", None);
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
            toy_sim_model::ownership::Permission::Dock,
            docking_bay.public,
            &docking_bay.allowed
        ),
        "departure denied"
    );
    let mut target = ship_pose(world, host)?;
    target.rotation =
        (DQuat::from_array(target.rotation) * DQuat::from_array(docking_bay.rotation)).to_array();
    let displacement = DQuat::from_array(target.rotation)
        * DVec3::NEG_Z
        * (radius(world, host)? + radius(world, ship)? + 10.);
    target.position = target.position.offset_by(displacement);
    target.velocity = (DVec3::from_array(target.velocity)
        + DVec3::from_array(target.angular_velocity).cross(displacement))
    .to_array();
    let ship_radius = radius(world, ship)?;
    ensure!(
        clear_at(world, ship, None, target.position, ship_radius),
        "undocking exit obstructed"
    );
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
        match (
            world.get::<PreciseTransform>(other),
            world.get::<super::spatial::SpatialBody>(other),
        ) {
            (Some(pose), Some(body)) => {
                pose.translation_um.relative_to(position).length() > radius + body.radius_m
            }
            _ => true,
        }
    });
    clear && celestial_conditions(world, position, radius).0
}

fn celestial_conditions(world: &mut World, position: GalacticPosition, radius: f64) -> (bool, f64) {
    if let Some(universe) = world.get_resource::<super::orrery::Universe>() {
        let epoch = world
            .get_resource::<Time<Fixed>>()
            .map(super::physics::sim_time)
            .unwrap_or_else(|| hifitime::Epoch::from_mjd_utc(0.));
        let mut clear = true;
        let mut curvature = 0.;
        for system in universe.index.containing_segment(position, DVec3::ZERO) {
            let solver = &universe.systems[system].solver;
            for body in solver
                .iter()
                .filter(|body| !matches!(body.class_params, super::orrery::BodyClass::Barycenter))
            {
                if let Some(center) = solver.solve_position(&body.name, epoch) {
                    let distance = center.relative_to(position).length();
                    clear &= distance > body.radius + radius;
                    curvature += 2. * super::physics::GRAVITATIONAL_CONSTANT * body.mass
                        / distance.max(body.radius).powi(3);
                }
            }
        }
        return (clear, curvature);
    }

    let mut query = world.query::<(&PreciseTransform, &CelestialState)>();
    query
        .iter(world)
        .fold((true, 0.), |(clear, curvature), (pose, body)| {
            let distance = pose.translation_um.relative_to(position).length();
            (
                clear && distance > body.body.radius + radius,
                curvature
                    + 2. * super::physics::GRAVITATIONAL_CONSTANT * body.body.mass
                        / distance.max(body.body.radius).powi(3),
            )
        })
}

pub fn low_curvature(world: &mut World, position: GalacticPosition) -> bool {
    celestial_conditions(world, position, 0.).1 <= 1e-8
}

pub fn slip_admissible(
    world: &mut World,
    ship: Entity,
    position: GalacticPosition,
    radius: f64,
) -> bool {
    if !clear_at(world, ship, None, position, radius) || !low_curvature(world, position) {
        return false;
    }
    let Some(candidates) = geometry::candidates(world, position, radius) else {
        return false;
    };
    candidates.into_iter().all(|other| {
        match (
            world.get::<PreciseTransform>(other),
            world.get::<Gate>(other),
        ) {
            (Some(pose), Some(gate)) if world.get::<Dormant>(other).is_none() => {
                !gate.enabled
                    || pose.translation_um.relative_to(position).length()
                        > gate.exclusion_m + radius
            }
            _ => true,
        }
    })
}

pub(crate) fn slip_flight_seconds(origin: GalacticPosition, destination: GalacticPosition) -> f64 {
    let light_years = origin.relative_to(destination).length() / LIGHT_YEAR_M;
    ((30.0 + 8.64 * light_years) * 10.0).ceil() * 0.1
}

fn slip_energy_j(origin: GalacticPosition, destination: GalacticPosition, mass: f64) -> f64 {
    let light_years = origin.relative_to(destination).length() / LIGHT_YEAR_M;
    (1e5 * mass * (1.0 + light_years / 1000.0)).ceil()
}

pub(crate) fn slip_times(
    origin: GalacticPosition,
    destination: GalacticPosition,
    mass: f64,
    power_w: f64,
    preparation: Option<&Preparation>,
    now: u64,
) -> (f64, f64) {
    let mass = preparation.map_or(mass, |preparation| preparation.mass);
    let work = preparation.map_or(0.0, |preparation| preparation.work_j);
    let energy = (slip_energy_j(origin, destination, mass) - work).max(0.0);
    let charge_ticks = (energy / (power_w.max(1.0) * 0.1)).ceil().max(1.0);
    let minimum_ticks = preparation.map_or(100, |preparation| {
        (preparation.started + 100).saturating_sub(now)
    });
    (
        charge_ticks.max(minimum_ticks as f64) * 0.1,
        slip_flight_seconds(origin, destination),
    )
}

pub fn prepare_slip(world: &mut World, ship: Entity, destination: GalacticPosition) -> Result<()> {
    toy_sim_protocol::validate_order(&Order::Slip {
        destination: Destination::Galactic(destination),
    })?;
    let pose = ship_pose(world, ship)?;
    let radius = radius(world, ship)?;
    ensure!(active(world, ship), "slip requires a ship in space");
    let mass = world.get::<MassProps>(ship).unwrap().mass;
    let now = tick(world);
    let drive = world
        .get::<SlipDrive>(ship)
        .ok_or_else(|| anyhow::anyhow!("no slipdrive"))?;
    ensure!(drive.ready_tick <= now, "drive not ready");
    let (preparation_s, flight_s) = slip_times(
        pose.position,
        destination,
        mass,
        drive.power_w,
        drive.preparation.as_ref(),
        now,
    );
    ensure!(
        slip_admissible(world, ship, pose.position, radius)
            && super::services::predicted_aperture_clear(
                world,
                ship,
                destination,
                radius,
                preparation_s + flight_s,
            ),
        "inadmissible slip aperture"
    );
    let mut drive = world.get_mut::<SlipDrive>(ship).unwrap();
    if let Some(preparation) = &mut drive.preparation {
        ensure!(
            mass <= preparation.mass * 1.001,
            "ship mass increased during charging"
        );
        preparation.destination = destination;
        preparation.required_j =
            slip_energy_j(pose.position, destination, preparation.mass).max(preparation.work_j);
    } else {
        drive.preparation = Some(Preparation {
            destination,
            started: now,
            mass,
            work_j: 0.0,
            required_j: slip_energy_j(pose.position, destination, mass),
        });
    }
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        if travel.0.autopilot_enabled && matches!(travel.0.status, Status::Blocked(_)) {
            travel.0.status = Status::Active;
        }
    }
    Ok(())
}

pub fn submit_route(
    world: &mut World,
    ship: Entity,
    revision: u64,
    orders: Vec<QueuedOrder>,
) -> Result<()> {
    ensure!(orders.len() <= 256, "invalid route length");
    for stage in &orders {
        toy_sim_protocol::validate_order(&stage.action)?;
    }
    ensure!(
        orders.iter().all(|stage| stage
            .estimated_propellant_kg
            .is_none_or(|kg| kg.is_finite() && kg >= 0.)),
        "invalid fuel estimate"
    );
    let now = tick(world);
    let mut travel = world
        .get_mut::<Travel>(ship)
        .ok_or_else(|| anyhow::anyhow!("travel unavailable"))?;
    ensure!(
        travel.0.revision == revision
            && travel.0.autopilot_enabled
            && matches!(travel.0.status, Status::Planning | Status::Blocked(_)),
        "stale or inactive route"
    );
    travel.0.search_limited = false;
    let index = travel.0.order;
    if index == travel.0.orders.len() && orders.is_empty() {
        travel.0.planning = None;
        travel.0.status = Status::Completed;
        return Ok(());
    }
    ensure!(!orders.is_empty(), "empty expansion");
    ensure!(index < travel.0.orders.len(), "no order to expand");
    ensure!(
        travel.0.orders.len() - 1 + orders.len() <= 256,
        "queue too long"
    );
    travel.0.orders.splice(index..=index, orders);
    travel.0.revision = travel.0.revision.wrapping_add(1);
    travel.0.estimated_arrival_tick = travel.0.orders[index]
        .estimated_duration_ticks
        .map(|duration| now.saturating_add(duration));
    travel.0.planning = None;
    travel.0.status = Status::Active;
    Ok(())
}

pub fn gate_transferred(world: &mut World, ship: Entity, entry: Entity) {
    identity::renew_spatial_instance(world, ship);
    geometry::update(world, ship);
    let entry_id = world.get::<Identity>(entry).map(|identity| identity.0);
    if let Some(mut travel) = world.get_mut::<Travel>(ship) {
        if matches!(travel.0.orders.get(travel.0.order).map(|stage| &stage.action), Some(Order::Jump(entry)) if Some(*entry) == entry_id)
        {
            complete_order(&mut travel.0);
        }
    }
    emit(world, ship, "gate-transferred", None);
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
    spatial: Option<super::spatial::SpatialBody>,
    rigid_body: bool,
    collision_body: bool,
    beacon: bool,
}

pub(crate) fn set_dormant(world: &mut World, ship: Entity, presence: Presence) {
    identity::renew_spatial_instance(world, ship);
    super::hardware::shutdown(world, ship);
    let velocity = world.get::<Velocity>(ship).map_or(DVec3::ZERO, |v| v.0);
    let angular_velocity = world
        .get::<AngularVelocity>(ship)
        .map_or(DVec3::ZERO, |v| v.0);
    let spatial = world.get::<super::spatial::SpatialBody>(ship).copied();
    let rigid_body = world.get::<super::physics::RigidBody>(ship).is_some();
    let collision_body = world
        .get::<super::physics::collision::CollisionBody>(ship)
        .is_some();
    let beacon = world.get::<BeaconEmitter>(ship).is_some();
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
                beacon,
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
        )>();
    geometry::update(world, ship);
}

fn set_active(world: &mut World, ship: Entity) {
    identity::renew_spatial_instance(world, ship);
    let motion = world.entity_mut(ship).take::<DormantMotion>();
    let velocity = world.get::<Velocity>(ship).map(|v| v.0);
    let angular = world.get::<AngularVelocity>(ship).map(|v| v.0);
    let mut entity = world.entity_mut(ship);
    entity
        .remove::<Dormant>()
        .insert(PresenceState(Presence::Space));
    if let Some(motion) = motion {
        if motion.beacon {
            entity.insert(BeaconEmitter);
        }
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
    geometry::update(world, ship);
}

pub fn dispatch(
    world: &mut World,
    ship: Entity,
    action: toy_sim_model::ProgramAction,
) -> Result<()> {
    use toy_sim_model::ProgramAction;
    match action {
        ProgramAction::PlanningProgress { revision, progress } => {
            ensure!(
                progress
                    .total
                    .is_none_or(|total| progress.completed <= total),
                "invalid planning progress"
            );
            let mut travel = world
                .get_mut::<Travel>(ship)
                .ok_or_else(|| anyhow::anyhow!("travel unavailable"))?;
            ensure!(
                travel.0.revision == revision
                    && travel.0.autopilot_enabled
                    && matches!(travel.0.status, Status::Planning | Status::Blocked(_)),
                "stale or inactive planning progress"
            );
            travel.0.status = Status::Planning;
            travel.0.planning = Some(progress);
            Ok(())
        }
        ProgramAction::Route {
            revision,
            search_limited,
            orders,
            fuel_budget,
        } => {
            ensure!(fuel_budget.valid(), "invalid fuel budget");
            submit_route(world, ship, revision, orders)?;
            let mut travel = world.get_mut::<Travel>(ship).unwrap();
            travel.0.fuel_budget = Some(fuel_budget);
            travel.0.search_limited = search_limited;
            Ok(())
        }
        ProgramAction::Estimate {
            revision,
            order,
            remaining_ticks,
            fuel_budget,
        } => {
            ensure!(fuel_budget.valid(), "invalid fuel budget");
            let now = tick(world);
            let mut travel = world
                .get_mut::<Travel>(ship)
                .ok_or_else(|| anyhow::anyhow!("travel unavailable"))?;
            ensure!(
                travel.0.revision == revision
                    && travel.0.order == order
                    && travel.0.status == Status::Active,
                "stale estimate"
            );
            travel.0.estimated_arrival_tick =
                remaining_ticks.map(|remaining| now.saturating_add(remaining));
            travel.0.fuel_budget = Some(fuel_budget);
            Ok(())
        }
        ProgramAction::Block { revision, reason } => {
            ensure!(
                world
                    .get::<Travel>(ship)
                    .is_some_and(|t| t.0.revision == revision),
                "stale revision"
            );
            blocked(world, ship, reason.chars().take(256).collect());
            Ok(())
        }
        ProgramAction::CompleteOrder { revision, order } => {
            let mut travel = world
                .get_mut::<Travel>(ship)
                .ok_or_else(|| anyhow::anyhow!("travel unavailable"))?;
            ensure!(
                travel.0.revision == revision
                    && travel.0.order == order
                    && travel.0.status == Status::Active,
                "stale order"
            );
            complete_order(&mut travel.0);
            Ok(())
        }
        ProgramAction::Slip(destination) => prepare_slip(world, ship, destination),

        ProgramAction::ReserveBay { station, bay } => {
            let host = entity(world, station)?;
            reserve_bay(world, ship, host, bay).map(|_| ())
        }
        ProgramAction::Dock { station, bay } => {
            let host = entity(world, station)?;
            dock(world, ship, host, bay)
        }
        ProgramAction::Undock => {
            undock(world, ship)?;
            if let Some(mut travel) = world.get_mut::<Travel>(ship) {
                if matches!(
                    travel
                        .0
                        .orders
                        .get(travel.0.order)
                        .map(|stage| &stage.action),
                    Some(Order::Undock)
                ) {
                    complete_order(&mut travel.0);
                }
            }
            Ok(())
        }
    }
}

fn complete_order(travel: &mut TravelState) {
    travel.order += 1;
    travel.estimated_arrival_tick = None;
    travel.fuel_budget = None;
    travel.planning = None;
    travel.status = if travel.order >= travel.orders.len() {
        Status::Completed
    } else {
        Status::Planning
    };
}

pub fn advance(world: &mut World) {
    let now = tick(world);
    let dormant_orders: Vec<_> = world
        .query::<(Entity, &PresenceState, &Travel)>()
        .iter(world)
        .filter(|(_, presence, travel)| {
            matches!(presence.0, Presence::Docked { .. })
                && matches!(travel.0.status, Status::Planning | Status::Active)
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
                    complete_order(&mut world.get_mut::<Travel>(ship).unwrap().0)
                }
                Ok(()) => world.get_mut::<Travel>(ship).unwrap().0.status = Status::Planning,
                Err(error) => blocked(world, ship, error.to_string()),
            }
        }
    }

    let arriving: Vec<_> = world
        .query::<(Entity, &Transit)>()
        .iter(world)
        .filter(|(_, transit)| transit.next_attempt <= now)
        .map(|(ship, transit)| (ship, transit.clone()))
        .collect();
    for (ship, transit) in arriving {
        if world
            .get::<super::hardware::Hull>(ship)
            .is_some_and(|hull| hull.0 <= 0.)
        {
            destroy(world, ship);
            continue;
        }
        let ship_radius = radius(world, ship).unwrap_or(f64::INFINITY);
        if slip_admissible(world, ship, transit.destination, ship_radius) {
            world
                .get_mut::<PreciseTransform>(ship)
                .unwrap()
                .translation_um = transit.destination;
            world.entity_mut(ship).remove::<Transit>();
            set_active(world, ship);
            if let Some(mut travel) = world.get_mut::<Travel>(ship) {
                if matches!(
                    travel
                        .0
                        .orders
                        .get(travel.0.order)
                        .map(|stage| &stage.action),
                    Some(Order::Slip { .. })
                ) {
                    complete_order(&mut travel.0);
                }
            }
            emit(world, ship, "slip-arrived", Some(transit.destination));
        } else {
            world.get_mut::<Transit>(ship).unwrap().next_attempt = now + 10;
            blocked(world, ship, "Arrival obstructed".into());
        }
    }

    let preparing: Vec<_> = world
        .query::<(Entity, &SlipDrive)>()
        .iter(world)
        .filter_map(|(ship, drive)| {
            drive
                .preparation
                .as_ref()
                .map(|preparation| (ship, drive.power_w, preparation.clone()))
        })
        .collect();
    for (ship, power, preparation) in preparing {
        let Ok(pose) = ship_pose(world, ship) else {
            continue;
        };
        let ship_radius = radius(world, ship).unwrap_or(f64::INFINITY);
        let mass = world
            .get::<MassProps>(ship)
            .map_or(f64::INFINITY, |m| m.mass);
        let valid = active(world, ship)
            && mass <= preparation.mass * 1.001
            && slip_admissible(world, ship, pose.position, ship_radius);
        if !valid {
            world.get_mut::<SlipDrive>(ship).unwrap().preparation = None;
            blocked(world, ship, "Slip preparation invalidated".into());
            continue;
        }
        let requested = (power * 0.1)
            .min(preparation.required_j - preparation.work_j)
            .max(0.);
        let requested_j = requested.stochastic_round();
        let paid_j = world
            .get::<super::hardware::ShipInventory>(ship)
            .map_or(0, |inventory| inventory.0.energy_j.min(requested_j));
        let work = paid_j as f64;
        let remaining_j = (preparation.required_j - preparation.work_j - work).max(0.);
        let charging_s = if remaining_j == 0. {
            0.
        } else if work > 0. {
            remaining_j / (work * 10.)
        } else {
            f64::INFINITY
        };
        let minimum_s = (preparation.started + 100).saturating_sub(now) as f64 * 0.1;
        let flight_s = slip_flight_seconds(pose.position, preparation.destination);
        let predicted_charge_s = (remaining_j / (power.max(1.0) * 0.1)).ceil() * 0.1;
        if !super::services::predicted_aperture_clear(
            world,
            ship,
            preparation.destination,
            ship_radius,
            predicted_charge_s.max(minimum_s) + flight_s,
        ) {
            blocked(world, ship, "Predicted slip exit is obstructed".into());
            continue;
        }
        if let Some(mut inventory) = world.get_mut::<super::hardware::ShipInventory>(ship) {
            inventory.0.energy_j -= paid_j;
        }
        super::hardware::add_travel_heat(world, ship, work * 0.2, 0.1);
        let remaining_s = charging_s.max(minimum_s) + flight_s;
        if let Some(mut travel) = world.get_mut::<Travel>(ship) {
            travel.0.estimated_arrival_tick = remaining_s
                .is_finite()
                .then(|| now.saturating_add((remaining_s * 10.).ceil() as u64));
        }
        let mut drive = world.get_mut::<SlipDrive>(ship).unwrap();
        drive.preparation.as_mut().unwrap().work_j += work;
        if drive.preparation.as_ref().unwrap().work_j >= preparation.required_j
            && now >= preparation.started + 100
        {
            let arrival = now + (flight_s * 10.0).round() as u64;
            drive.preparation = None;
            drive.ready_tick = arrival + 600;
            world.entity_mut(ship).insert(Transit {
                origin: pose.position,
                departed: now,
                destination: preparation.destination,
                next_attempt: arrival,
            });
            set_dormant(world, ship, Presence::SlipTransit(Id::new()));
            if let Some(mut travel) = world.get_mut::<Travel>(ship) {
                travel.0.estimated_arrival_tick = Some(arrival);
            }
            emit(world, ship, "slip-departed", None);
        }
    }
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
    world.entity_mut(ship).remove::<BeaconEmitter>();
    super::missiles::destroyed(world, ship);
    emit(world, ship, "destroyed", None);
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
        let mut state = toy_sim_ships::ShipState::new(&design, catalogue);
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

    fn publish(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(super::super::services::publish_indexes)
            .unwrap();
    }

    fn ship(world: &mut World, position: DVec3, account: Id) -> Entity {
        crate::sim::ownership::add_account(world, account);
        let design = Arc::new(
            toy_sim_ships::armed_starter()
                .compile(&toy_sim_ships::Catalogue::builtin())
                .unwrap(),
        );
        let radius = design.radius;
        let entity = world
            .spawn((
                ShipDesign(design),
                crate::sim::ownership::AssetOwner(toy_sim_model::ownership::Principal::Player(
                    account,
                )),
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
        use toy_sim_model::ownership::{AccessPolicy, Permission, Principal};
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
                toy_sim_model::ownership::Principal::Player(Id::new()),
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
    fn blocked_slip_arrival_keeps_destination_and_retries_after_one_second() {
        let mut world = world();
        let account = Id::new();
        let child = ship(&mut world, DVec3::ZERO, account);
        let destination = GalacticPosition::default().offset_by(DVec3::X * 10000.);
        let blocker = ship(&mut world, DVec3::X * 10000., account);
        world.entity_mut(child).insert(Transit {
            origin: GalacticPosition::ZERO,
            departed: 0,
            destination,
            next_attempt: 0,
        });
        set_dormant(&mut world, child, Presence::SlipTransit(Id::new()));
        publish(&mut world);
        advance(&mut world);
        assert_eq!(world.get::<Transit>(child).unwrap().next_attempt, 10);
        world.despawn(blocker);
        world.resource_mut::<SimulationCounters>().ticks = 10;
        publish(&mut world);
        advance(&mut world);
        assert_eq!(
            world.get::<PreciseTransform>(child).unwrap().translation_um,
            destination
        );
        assert!(world.get::<Dormant>(child).is_none());
        assert!(world.get::<Transit>(child).is_none());
    }

    #[test]
    fn moving_slip_charges_departs_and_preserves_arrival_velocity() {
        let mut world = world();
        let child = ship(&mut world, DVec3::ZERO, Id::new());
        let velocity = DVec3::Y * 30_000.;
        world.get_mut::<Velocity>(child).unwrap().0 = velocity;
        world.entity_mut(child).insert((
            SlipDrive::default(),
            super::super::hardware::ShipInventory(toy_sim_ships::Inventory {
                tank_capacities_m3: vec![0.; 2],
                quantities: vec![0; 2],
                cargo: vec![0; 2],
                energy_j: 1_000_000_000_000,
            }),
        ));
        let destination = GalacticPosition::ZERO.offset_by(DVec3::X * 1e9);
        publish(&mut world);
        prepare_slip(&mut world, child, destination).unwrap();
        for tick in 0..=100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            world
                .get_mut::<PreciseTransform>(child)
                .unwrap()
                .translation_um = GalacticPosition::ZERO.offset_by(velocity * (tick as f64 * 0.1));
            publish(&mut world);
            advance(&mut world);
            if tick < 100 {
                assert!(world.get::<Transit>(child).is_none());
                assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_some());
            }
        }
        let transit = world.get::<Transit>(child).unwrap();
        assert_eq!(
            transit.origin,
            GalacticPosition::ZERO.offset_by(velocity * 10.)
        );
        let arrival = transit.next_attempt;
        assert!(world.get::<Velocity>(child).is_none());
        world.resource_mut::<SimulationCounters>().ticks = arrival;
        publish(&mut world);
        advance(&mut world);
        assert!(world.get::<Transit>(child).is_none());
        assert!(world.get::<Dormant>(child).is_none());
        assert_eq!(world.get::<Velocity>(child).unwrap().0, velocity);
        assert_eq!(
            world.get::<PreciseTransform>(child).unwrap().translation_um,
            destination
        );
    }

    #[test]
    fn updating_slip_target_preserves_charge_and_freezes_the_departed_endpoint() {
        let mut world = world();
        let child = ship(&mut world, DVec3::ZERO, Id::new());
        world.entity_mut(child).insert((
            SlipDrive::default(),
            super::super::hardware::ShipInventory(toy_sim_ships::Inventory {
                tank_capacities_m3: Vec::new(),
                quantities: Vec::new(),
                cargo: Vec::new(),
                energy_j: 1_000_000_000_000,
            }),
        ));
        publish(&mut world);
        let original = GalacticPosition::from_meters(DVec3::X * 1e9);
        prepare_slip(&mut world, child, original).unwrap();
        advance(&mut world);
        let initial = world
            .get::<SlipDrive>(child)
            .unwrap()
            .preparation
            .clone()
            .unwrap();
        assert!(initial.work_j > 0.0);
        let energy = world
            .get::<super::super::hardware::ShipInventory>(child)
            .unwrap()
            .0
            .energy_j;
        let destination = original.offset_by(DVec3::Y * 1e6);
        world.resource_mut::<SimulationCounters>().ticks = 25;
        prepare_slip(&mut world, child, destination).unwrap();
        let updated = world
            .get::<SlipDrive>(child)
            .unwrap()
            .preparation
            .clone()
            .unwrap();
        assert_eq!(updated.started, initial.started);
        assert_eq!(updated.work_j, initial.work_j);
        assert_eq!(updated.destination, destination);
        assert_eq!(
            world
                .get::<super::super::hardware::ShipInventory>(child)
                .unwrap()
                .0
                .energy_j,
            energy
        );
        let (preparation_s, flight_s) = slip_times(
            GalacticPosition::ZERO,
            destination,
            1000.0,
            100e6,
            Some(&updated),
            25,
        );
        assert_eq!(preparation_s, 7.5);
        assert_eq!(
            flight_s,
            slip_flight_seconds(GalacticPosition::ZERO, destination)
        );
        for tick in 25..=100 {
            world.resource_mut::<SimulationCounters>().ticks = tick;
            publish(&mut world);
            advance(&mut world);
        }
        let frozen = world.get::<Transit>(child).unwrap().clone();
        assert_eq!(frozen.destination, destination);
        assert!(prepare_slip(&mut world, child, original).is_err());
        assert_eq!(
            world.get::<Transit>(child).unwrap().destination,
            destination
        );
    }

    #[test]
    fn blocked_route_cancels_its_pending_slip_charge() {
        let mut world = world();
        let child = ship(&mut world, DVec3::ZERO, Id::new());
        world.entity_mut(child).insert(SlipDrive::default());
        publish(&mut world);
        prepare_slip(
            &mut world,
            child,
            GalacticPosition::from_meters(DVec3::X * 1e9),
        )
        .unwrap();
        let revision = world.get::<Travel>(child).unwrap().0.revision;
        dispatch(
            &mut world,
            child,
            toy_sim_model::ProgramAction::Block {
                revision,
                reason: "Destination prediction unavailable".into(),
            },
        )
        .unwrap();
        assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_none());
        assert!(world.get::<Transit>(child).is_none());
    }

    #[test]
    fn obstructed_slip_exit_does_not_spend_more_energy_and_can_restart() {
        let mut world = world();
        let child = ship(&mut world, DVec3::ZERO, Id::new());
        world.entity_mut(child).insert((
            SlipDrive::default(),
            super::super::hardware::ShipInventory(toy_sim_ships::Inventory {
                tank_capacities_m3: Vec::new(),
                quantities: Vec::new(),
                cargo: Vec::new(),
                energy_j: 1_000_000_000,
            }),
        ));
        world.get_mut::<Travel>(child).unwrap().0.autopilot_enabled = true;
        publish(&mut world);
        let destination = GalacticPosition::from_meters(DVec3::X * 1e9);
        prepare_slip(&mut world, child, destination).unwrap();
        advance(&mut world);
        let energy = world
            .get::<super::super::hardware::ShipInventory>(child)
            .unwrap()
            .0
            .energy_j;
        let obstacle = ship(&mut world, DVec3::X * 1e9, Id::new());
        publish(&mut world);
        advance(&mut world);
        assert_eq!(
            world
                .get::<super::super::hardware::ShipInventory>(child)
                .unwrap()
                .0
                .energy_j,
            energy
        );
        assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_none());
        assert!(matches!(
            world.get::<Travel>(child).unwrap().0.status,
            Status::Blocked(_)
        ));
        world.despawn(obstacle);
        publish(&mut world);
        prepare_slip(&mut world, child, destination).unwrap();
        assert_eq!(world.get::<Travel>(child).unwrap().0.status, Status::Active);
        assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_some());
    }

    #[test]
    fn slip_spends_available_energy_waits_for_preparation_and_can_be_cancelled() {
        let mut world = world();
        let account = Id::new();
        let child = ship(&mut world, DVec3::ZERO, account);
        world.entity_mut(child).insert((
            SlipDrive::default(),
            super::super::hardware::ShipInventory(toy_sim_ships::Inventory {
                tank_capacities_m3: vec![0.; 2],
                quantities: vec![0; 2],
                cargo: vec![0; 2],
                energy_j: 5,
            }),
        ));
        let destination = GalacticPosition::default().offset_by(DVec3::X * 10000.);
        publish(&mut world);
        prepare_slip(&mut world, child, destination).unwrap();
        publish(&mut world);
        advance(&mut world);
        assert_eq!(
            world
                .get::<super::super::hardware::ShipInventory>(child)
                .unwrap()
                .0
                .energy_j,
            0
        );
        assert_eq!(
            world
                .get::<SlipDrive>(child)
                .unwrap()
                .preparation
                .as_ref()
                .unwrap()
                .work_j,
            5.
        );
        assert!(world.get::<Transit>(child).is_none());
        cancel_pending(&mut world, child);
        assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_none());
        publish(&mut world);
        prepare_slip(&mut world, child, destination).unwrap();
        world.get_mut::<MassProps>(child).unwrap().mass *= 2.;
        publish(&mut world);
        advance(&mut world);
        assert!(world.get::<SlipDrive>(child).unwrap().preparation.is_none());
        assert!(matches!(
            world.get::<Travel>(child).unwrap().0.status,
            Status::Blocked(_)
        ));
    }

    #[test]
    fn inactive_celestial_body_still_prevents_slip_arrival() {
        let mut world = world();
        let universe =
            super::super::orrery::Universe::init(toy_sim_universe::example_config()).unwrap();
        let body = universe.iter().next().unwrap().name.clone();
        let position = universe
            .solve_position(&body, hifitime::Epoch::from_mjd_utc(0.))
            .unwrap();
        world.insert_resource(universe);
        assert!(!celestial_conditions(&mut world, position, 1.).0);
        assert!(!low_curvature(&mut world, position));
    }

    #[test]
    fn debug_recovery_keeps_identity_and_rejects_occupied_wrecks() {
        let mut world = world();
        world.insert_resource(super::super::vessel::ShipCatalogue(
            toy_sim_ships::Catalogue::builtin(),
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
            toy_sim_model::ownership::Principal::Player(account)
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
    fn simultaneous_arrivals_cannot_occupy_the_same_aperture() {
        let mut world = world();
        let account = Id::new();
        let first = ship(&mut world, DVec3::ZERO, account);
        let second = ship(&mut world, DVec3::Z * 1000., account);
        let destination = GalacticPosition::ZERO.offset_by(DVec3::X * 10000.);
        for ship in [first, second] {
            world.entity_mut(ship).insert(Transit {
                origin: GalacticPosition::ZERO,
                departed: 0,
                destination,
                next_attempt: 0,
            });
            set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        }
        publish(&mut world);
        advance(&mut world);
        let arrived = [first, second]
            .into_iter()
            .filter(|&ship| world.get::<Dormant>(ship).is_none())
            .count();
        assert_eq!(arrived, 1);
        let delayed = [first, second]
            .into_iter()
            .find(|&ship| world.get::<Dormant>(ship).is_some())
            .unwrap();
        assert_eq!(world.get::<Transit>(delayed).unwrap().next_attempt, 10);
    }

    #[test]
    fn spatial_lifetime_changes_survive_an_unpublished_departure_and_return() {
        use super::super::intelligence::{
            AssociationIndex, Measurements, TrackEstimate, TrackGroup,
        };
        let mut world = world();
        world.init_resource::<AssociationIndex>();
        let account = Id::new();
        let ship = ship(&mut world, DVec3::ZERO, account);
        let ship_id = id(&world, ship).unwrap();
        let platform = Id::new();
        let sample = toy_sim_intel::Measurement::sensor(
            &[7; 32],
            platform,
            ship_id,
            GalacticPosition::ZERO.offset_by(DVec3::X * 1000.),
            &ship_pose(&world, ship).unwrap(),
            0,
        );
        let first_group = world.spawn(Measurements(vec![sample.clone()])).id();
        let second_group = world.spawn(Measurements(vec![sample])).id();
        let mut schedule = Schedule::default();
        schedule.add_systems(super::super::intelligence::fuse);
        schedule.run(&mut world);
        let published = |world: &mut World, group| {
            world
                .query::<(&TrackEstimate, &TrackGroup)>()
                .iter(world)
                .find(|(_, owner)| owner.0 == group)
                .unwrap()
                .0
                .0
                .clone()
        };
        let first = published(&mut world, first_group);
        let other_group = published(&mut world, second_group);
        assert_ne!(first.spatial_instance, other_group.spatial_instance);
        assert!(first.entity.is_none());
        let owned_before = world.get::<identity::SpatialInstance>(ship).unwrap().0;

        set_dormant(&mut world, ship, Presence::SlipTransit(Id::new()));
        set_active(&mut world, ship);
        schedule.run(&mut world);
        let returned = published(&mut world, first_group);
        assert_eq!(id(&world, ship).unwrap(), ship_id);
        assert_eq!(returned.id, first.id);
        assert!(returned.entity.is_none());
        assert_ne!(returned.spatial_instance, first.spatial_instance);
        assert_ne!(returned.spatial_instance, ship_id);
        let owned_after = world.get::<identity::SpatialInstance>(ship).unwrap().0;
        assert_ne!(owned_after, owned_before);
        assert_ne!(returned.spatial_instance, owned_after);

        let pose = ship_pose(&world, ship).unwrap();
        write_pose(&mut world, ship, pose);
        schedule.run(&mut world);
        let transferred = published(&mut world, first_group);
        assert_eq!(transferred.id, returned.id);
        assert_ne!(transferred.spatial_instance, returned.spatial_instance);
    }
}
