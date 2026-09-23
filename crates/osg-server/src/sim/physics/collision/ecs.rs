use super::*;
use crate::sim::{
    GameState,
    hardware::{self, Hull, InstalledPart, PartDevices, ShipInventory, ShipThermal, Weapon},
    physics::{
        AccelerometerState, AccumulatedForce, AccumulatedTorque, AngularVelocity,
        GravityAcceleration, MassProps, RigidBody, Velocity,
    },
    precision::PreciseTransform,
    simulation::SimulationSystems,
    vessel::ShipDesign,
};
use ahash::AHashMap;
use bevy::prelude::*;

#[derive(Component, Default)]
pub struct CollisionBody;

/// A reusable physical slug. Weapons can spawn this with mass, pose and velocity;
/// it participates in precisely the same impact/thermal pipeline as ships.
#[derive(Component)]
#[require(RigidBody, CollisionBody)]
pub struct Projectile {
    pub launch_owner: Option<Entity>,
    pub remaining_s: f64,
    pub radius_m: f64,
    pub hull_hp: f64,
    pub thermal: ThermalState,
}

impl Projectile {
    pub fn new(radius_m: f64, mass_kg: f64) -> Self {
        assert!(radius_m.is_finite() && radius_m > 0.0 && mass_kg.is_finite() && mass_kg > 0.0);
        Self {
            launch_owner: None,
            remaining_s: 2.0,
            radius_m,
            hull_hp: mass_kg,
            thermal: ThermalState {
                hull_energy_j: 0.0,
                ..Default::default()
            },
        }
    }
}

#[derive(Resource, Default, Clone)]
pub struct CollisionStats {
    pub bodies: usize,
    pub candidates: u64,
    pub groups: usize,
    pub grouped_bodies: usize,
    pub reused_worlds: usize,
    pub contact_pairs: u64,
    pub impacts: u64,

    pub dissipated_j: f64,
    pub index_seconds: f64,
    pub query_seconds: f64,
    pub solve_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Resource, Default)]
struct GeometryCache(AHashMap<usize, (std::sync::Weak<CompiledShipDesign>, Arc<Geometry>)>);

#[derive(Resource, Default)]
pub struct CollisionReport {
    pub epoch: f64,
    pub report: Report,
}

#[derive(bevy::ecs::schedule::ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct CollisionTick;

#[derive(Clone, Copy)]
struct ForceSample {
    pose: PreciseTransform,
    mass: MassProps,
    angular: DVec3,
    force: DVec3,
    torque: DVec3,
    gravity: DVec3,
}

#[derive(Resource, Default)]
struct TickState {
    bodies: Vec<Body>,
    roots: AHashMap<Entity, ForceSample>,
    report: Report,
    impacts: Vec<rapier::Impact>,
    epoch: f64,
    dt: f64,
    started: Option<std::time::Instant>,
}

fn schedule() -> Schedule {
    let mut schedule = Schedule::new(CollisionTick);
    schedule.add_systems(
        (
            crate::sim::spatial::collect,
            prepare,
            synchronize,
            activate_shields,
            prepare_weapons,
            launch,
            synchronize,
            beams,
            kick,
            integrate,
            damage,
            (write_physics, write_weapons),
            publish,
            crate::sim::combat::ingest,
            crate::sim::travel::destroy_collisions,
        )
            .chain(),
    );
    schedule
}

fn initialize(world: &mut World) {
    reset(world);
    world.init_resource::<crate::sim::spatial::SpatialIndex>();
    world.init_resource::<crate::sim::session::Events>();
    world.init_resource::<crate::sim::combat::CombatHistory>();
    world.init_resource::<crate::sim::travel::TravelEvents>();
    world.add_schedule(schedule());
}

/// Startup and checkpoint restoration discard all transient solver state.
pub(crate) fn reset(world: &mut World) {
    world.insert_resource(GeometryCache::default());
    world.insert_resource(CollisionStats::default());
    world.insert_resource(CollisionReport::default());
    world.insert_resource(SolverWorkspace::default());
    world.insert_resource(TickState::default());
}

pub fn install(app: &mut App) {
    initialize(app.world_mut());
    app.add_systems(
        FixedPostUpdate,
        step.in_set(SimulationSystems::Integrate)
            .run_if(in_state(GameState::Game)),
    );
}

/// The exclusive boundary only drives the named schedule.
pub fn step(world: &mut World) {
    world.run_schedule(CollisionTick);
}

#[derive(bevy::ecs::query::QueryData)]
struct PhysicalSource {
    entity: Entity,
    pose: &'static PreciseTransform,
    mass: &'static MassProps,
    velocity: &'static Velocity,
    angular: &'static AngularVelocity,
    force: &'static AccumulatedForce,
    torque: &'static AccumulatedTorque,
    projectile: Option<&'static Projectile>,
    gravity: Option<&'static GravityAcceleration>,
}

fn prepare(
    time: Res<Time<Fixed>>,
    sources: Query<PhysicalSource, (With<CollisionBody>, Without<crate::sim::travel::Dormant>)>,
    ships: Query<(&ShipDesign, &Hull, &ShipThermal)>,
    mut cache: ResMut<GeometryCache>,
    mut state: ResMut<TickState>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.prepare");
    state.started = Some(std::time::Instant::now());
    state.dt = time.delta_secs_f64();
    state.epoch = time.elapsed_secs_f64() - state.dt;
    state.bodies.clear();
    state.roots.clear();
    state.report = Report::default();
    state.impacts.clear();
    cache.0.retain(|_, (design, _)| design.strong_count() > 0);

    for source in &sources {
        let mass = *source.mass;
        let pose = *source.pose;
        let mut body = if let Some(projectile) = source.projectile {
            let mut body = weapons::projectile_body(
                source.entity,
                pose.translation_um,
                source.velocity.0,
                mass.mass,
                projectile.radius_m,
                0.0,
            );
            body.launch_owner = projectile.launch_owner;
            body.expires_at = Some(projectile.remaining_s);
            body.members[0].hull = projectile.hull_hp;
            body.members[0].thermal = projectile.thermal;
            body
        } else if let Ok((design, hull, thermal)) = ships.get(source.entity) {
            let key = Arc::as_ptr(&design.0) as usize;
            let geometry = cache
                .0
                .entry(key)
                .or_insert_with(|| {
                    (
                        Arc::downgrade(&design.0),
                        Arc::new(Geometry::ship(&design.0)),
                    )
                })
                .1
                .clone();
            Body {
                entity: source.entity,
                position: pose.translation_um,
                rotation: pose.rotation,
                velocity: source.velocity.0,
                momentum: DVec3::ZERO,
                mass: mass.mass,
                inertia_inv: mass.inertia_inv,
                radius: geometry.shield_radius,
                impulse_dv: DVec3::ZERO,
                projectile: false,
                launch_owner: None,
                expires_at: None,
                time: 0.0,
                members: vec![Member {
                    entity: source.entity,
                    geometry,
                    local_position: DVec3::ZERO,
                    local_rotation: DQuat::IDENTITY,
                    mass: mass.mass,
                    inertia: mass.inertia,
                    hull: hull.0,
                    thermal: thermal.0,
                    model: ThermalModel::from(design.0.as_ref()),
                    thermal_time: 0.0,
                    destroyed: false,
                }],
            }
        } else {
            continue;
        };
        body.rotation = pose.rotation;
        body.momentum =
            pose.rotation * (mass.inertia * (pose.rotation.inverse() * source.angular.0));
        state.roots.insert(
            source.entity,
            ForceSample {
                pose,
                mass,
                angular: source.angular.0,
                force: source.force.0,
                torque: source.torque.0,
                gravity: source.gravity.map_or(DVec3::ZERO, |gravity| gravity.0),
            },
        );
        state.bodies.push(body);
    }
}

fn synchronize(
    mut spatial: ResMut<crate::sim::spatial::SpatialIndex>,
    mut state: ResMut<TickState>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.synchronize");
    let started = std::time::Instant::now();
    for body in &state.bodies {
        spatial.insert_collision(body.entity, body.position, body.radius);
    }
    tick::synchronize(&state.bodies, &mut spatial.hash.write().unwrap());
    state.report.index_seconds += started.elapsed().as_secs_f64();
}

fn activate_shields(spatial: Res<crate::sim::spatial::SpatialIndex>, mut state: ResMut<TickState>) {
    let _profile =
        crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.activate_shields");
    activate(&mut state.bodies, &spatial.hash.read().unwrap());
}

fn prepare_weapons(
    ships: Query<
        (Entity, &ShipDesign, &ShipInventory, &PartDevices),
        Without<crate::sim::travel::Dormant>,
    >,
    parts: Query<(&InstalledPart, &hardware::Device, Option<&Weapon>)>,
    state: Res<TickState>,
    mut workspace: ResMut<SolverWorkspace>,
) {
    let _profile =
        crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.prepare_weapons");
    workspace.time_s = state.epoch;
    workspace.weapons.clear();
    for (entity, design, inventory, devices) in &ships {
        if design.0.weapon_parts.is_empty() || !state.roots.contains_key(&entity) {
            continue;
        }
        let mut weapons =
            vec![osg_ships::weapons::WeaponState::default(); design.0.weapon_parts.len()];
        let mut operational = vec![false; weapons.len()];
        for part in &devices.0 {
            let Ok((installed, device, Some(weapon))) = parts.get(*part) else {
                continue;
            };
            let Some(index) = design.0.part_weapons[installed.index] else {
                continue;
            };
            weapons[index] = weapon.0.clone();
            weapons[index].advanced_s = state.epoch;
            if let Some(command) = &mut weapons[index].command {
                command.epoch_s = state.epoch;
            }
            operational[index] = device.0.operational;
        }
        workspace.weapons.insert(
            entity,
            weapons::WeaponShip {
                design: design.0.clone(),
                inventory: inventory.0.clone(),
                weapons,
                operational,
            },
        );
    }
}

fn launch(
    mut commands: Commands,
    mut state: ResMut<TickState>,
    mut workspace: ResMut<SolverWorkspace>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.launch");
    let state = &mut *state;
    tick::fire(
        &mut state.bodies,
        state.dt,
        &mut workspace,
        &mut || commands.spawn_empty().id(),
        &mut state.report,
    );
    for shot in &state.report.shots {
        let inertia = DMat3::IDENTITY * (0.4 * shot.mass_kg * shot.radius_m.powi(2));
        commands.entity(shot.projectile).insert((
            Name::new("Tracer slug"),
            Projectile {
                launch_owner: Some(shot.owner),
                ..Projectile::new(shot.radius_m, shot.mass_kg)
            },
            PreciseTransform {
                translation_um: shot.position,
                rotation: DQuat::IDENTITY,
            },
            Velocity(shot.velocity),
            MassProps {
                mass: shot.mass_kg,
                inertia,
                inertia_inv: inertia.inverse(),
            },
        ));
    }
}

fn beams(spatial: Res<crate::sim::spatial::SpatialIndex>, mut state: ResMut<TickState>) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.beams");
    let state = &mut *state;
    for beam in std::mem::take(&mut state.report.beams) {
        weapons::resolve_beam(
            beam,
            &mut state.bodies,
            &spatial.hash.read().unwrap(),
            0.0,
            &mut state.report,
        );
    }
}

fn integrate(
    spatial: Res<crate::sim::spatial::SpatialIndex>,
    mut state: ResMut<TickState>,
    mut workspace: ResMut<SolverWorkspace>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.integrate");
    let state = &mut *state;
    state.impacts = tick::integrate(
        &mut state.bodies,
        state.dt,
        &spatial.hash.read().unwrap(),
        &mut workspace,
        &mut state.report,
    );
}

fn kick(field: super::super::GravityField, mut state: ResMut<TickState>) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.kick");
    let state = &mut *state;
    for body in &mut state.bodies {
        let mass = MassProps {
            mass: body.mass,
            inertia: body.inertia_inv.inverse(),
            inertia_inv: body.inertia_inv,
        };
        let angular = body.world_inverse() * body.momentum;
        let sample = state.roots.entry(body.entity).or_insert_with(|| {
            let gravity = field.sample(body.entity, body.position).0;
            ForceSample {
                pose: PreciseTransform {
                    translation_um: body.position,
                    rotation: body.rotation,
                },
                mass,
                angular,
                force: gravity * mass.mass,
                torque: DVec3::ZERO,
                gravity,
            }
        });
        sample.force += sample.gravity * (mass.mass - sample.mass.mass);
        sample.mass = mass;
        (body.velocity, body.momentum) = super::super::force_kick(
            body.velocity,
            body.rotation,
            angular,
            mass,
            sample.force,
            sample.torque,
            state.dt,
        );
    }
}

fn damage(mut state: ResMut<TickState>) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.damage");
    let state = &mut *state;
    tick::damage(
        &mut state.bodies,
        state.dt,
        std::mem::take(&mut state.impacts),
        &mut state.report,
    );
}

#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
struct PhysicalTarget {
    pose: &'static mut PreciseTransform,
    mass: &'static mut MassProps,
    velocity: &'static mut Velocity,
    angular: &'static mut AngularVelocity,
    force: &'static mut AccumulatedForce,
    torque: &'static mut AccumulatedTorque,
    gravity: Option<&'static mut GravityAcceleration>,
    sample: Option<&'static mut AccelerometerState>,
    hull: Option<&'static mut Hull>,
    thermal: Option<&'static mut ShipThermal>,
    projectile: Option<&'static mut Projectile>,
}

fn write_physics(state: Res<TickState>, mut targets: Query<PhysicalTarget>) {
    let _profile =
        crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.write_physics");
    for body in &state.bodies {
        let Ok(mut target) = targets.get_mut(body.entity) else {
            continue;
        };
        if let Some(gravity) = target.gravity.as_mut() {
            gravity.0 = DVec3::ZERO;
        }
        let angular = body.world_inverse() * body.momentum;
        if let Some(sample) = target.sample.as_mut() {
            let (specific, alpha) =
                state
                    .roots
                    .get(&body.entity)
                    .map_or((DVec3::ZERO, DVec3::ZERO), |old| {
                        let w = old.pose.rotation.inverse() * old.angular;
                        let alpha = old.pose.rotation
                            * (old.mass.inertia_inv
                                * (old.pose.rotation.inverse() * old.torque
                                    - w.cross(old.mass.inertia * w)));
                        (old.force / old.mass.mass - old.gravity, alpha)
                    });
            **sample = AccelerometerState {
                specific_force_body: body.rotation.inverse()
                    * (specific + body.impulse_dv / state.dt),
                angular_acceleration_body: body.rotation.inverse() * alpha,
                angular_velocity_body: body.rotation.inverse() * angular,
                time_s: Some(state.epoch + state.dt),
            };
        }
        target.pose.translation_um = body.position;
        target.pose.rotation = body.rotation;
        target.velocity.0 = body.velocity;
        target.angular.0 = angular;
        *target.mass = MassProps {
            mass: body.mass,
            inertia: body.inertia_inv.inverse(),
            inertia_inv: body.inertia_inv,
        };
        target.force.0 = DVec3::ZERO;
        target.torque.0 = DVec3::ZERO;
        if let Some(member) = body.members.first() {
            if let Some(hull) = target.hull.as_mut() {
                hull.0 = member.hull;
            }
            if let Some(thermal) = target.thermal.as_mut() {
                thermal.0 = member.thermal;
            }
            if let Some(projectile) = target.projectile.as_mut() {
                projectile.remaining_s = body.expires_at.unwrap() - state.dt;
                projectile.hull_hp = member.hull;
                projectile.thermal = member.thermal;
            }
        }
    }
}

fn write_weapons(
    workspace: Res<SolverWorkspace>,
    mut inventories: Query<&mut ShipInventory>,
    mut parts: Query<(&InstalledPart, &mut Weapon)>,
) {
    let _profile =
        crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.write_weapons");
    for (entity, result) in &workspace.weapons {
        if let Ok(mut inventory) = inventories.get_mut(*entity) {
            inventory.0 = result.inventory.clone();
        }
    }
    for (part, mut weapon) in &mut parts {
        if let Some(result) = workspace.weapons.get(&part.ship)
            && let Some(index) = result.design.part_weapons[part.index]
        {
            weapon.0 = result.weapons[index].clone();
        }
    }
}

fn publish(
    mut state: ResMut<TickState>,
    mut output: ResMut<CollisionReport>,
    mut stats: ResMut<CollisionStats>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.collision.ecs.publish");
    *stats = CollisionStats {
        bodies: state.bodies.len(),
        candidates: state.report.candidates,
        groups: state.report.groups,
        grouped_bodies: state.report.grouped_bodies,
        reused_worlds: state.report.reused_worlds,
        contact_pairs: state.report.contact_pairs,
        impacts: state.report.impacts,
        dissipated_j: state.report.dissipated_j,
        index_seconds: state.report.index_seconds,
        query_seconds: state.report.query_seconds,
        solve_seconds: state.report.solve_seconds,
        total_seconds: state.started.unwrap().elapsed().as_secs_f64(),
    };
    output.epoch = state.epoch;
    output.report = std::mem::take(&mut state.report);
}

#[cfg(test)]
mod tests;
