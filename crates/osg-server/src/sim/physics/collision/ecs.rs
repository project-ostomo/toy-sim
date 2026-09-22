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
    pub detailed_queries: u64,
    pub impacts: u64,

    pub dissipated_j: f64,
    pub index_seconds: f64,
    pub query_seconds: f64,
    pub solve_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Resource, Default)]
struct GeometryCache(AHashMap<usize, (std::sync::Weak<CompiledShipDesign>, Arc<Geometry>)>);

#[derive(Resource)]
pub struct CollisionReport {
    pub epoch: f64,
    pub report: Report,
}

pub fn install(app: &mut App) {
    app.init_resource::<GeometryCache>()
        .init_resource::<CollisionStats>()
        .init_resource::<SolverWorkspace>()
        .add_systems(
            FixedPostUpdate,
            step.in_set(SimulationSystems::Integrate)
                .after(super::super::apply_forces)
                .run_if(in_state(GameState::Game)),
        );
}

pub fn step(world: &mut World) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("collision_step");
    let prepare_profile = crate::sim::diagnostics::ProfileScope::new("collision_prepare_bodies");
    let timer = std::time::Instant::now();
    let dt = world.resource::<Time<Fixed>>().delta_secs_f64();
    let elapsed = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    let mut roots: Vec<_> = world
        .query_filtered::<(
            Entity,
            &PreciseTransform,
            &MassProps,
            &Velocity,
            &AngularVelocity,
            &AccumulatedForce,
            &AccumulatedTorque,
        ), With<CollisionBody>>()
        .iter(world)
        .map(|(e, p, m, v, w, f, t)| (e, *p, *m, v.0, w.0, f.0, t.0))
        .collect();
    roots.sort_unstable_by_key(|r| r.0);
    let mut lookup = AHashMap::default();
    let mut bodies = Vec::with_capacity(roots.len());
    for (id, (entity, p, m, v, w, force, torque)) in roots.iter().copied().enumerate() {
        lookup.insert(entity, id);
        let start = world
            .get::<crate::sim::travel::ArrivalOffset>(entity)
            .map_or(0.0, |offset| offset.0.clamp(0.0, dt));
        let duration = dt - start;
        let momentum = p.rotation * (m.inertia * (p.rotation.inverse() * w)) + torque * duration;
        bodies.push(Body {
            projectile: world.get::<Projectile>(entity).is_some(),
            launch_owner: world.get::<Projectile>(entity).and_then(|p| p.launch_owner),
            expires_at: world.get::<Projectile>(entity).map(|p| p.remaining_s),
            entity,
            position: p.translation_um,
            rotation: p.rotation,
            time: start,
            velocity: v + force / m.mass * duration,
            momentum,
            mass: m.mass,
            inertia_inv: m.inertia_inv,
            members: Vec::new(),
            radius: 0.0,
            feature: f64::INFINITY,
            generation: 0,
            impulse_dv: DVec3::ZERO,
            rotation_path: None,
        });
    }
    let ships: Vec<_> = world
        .query::<(Entity, &ShipDesign, &Hull, &ShipThermal, &MassProps)>()
        .iter(world)
        .map(|(e, d, h, thermal, mass)| (e, d.0.clone(), h.0, thermal.0, *mass))
        .collect();
    world.resource_scope(|_, mut cache: Mut<GeometryCache>| {
        cache.0.retain(|_, (design, _)| design.strong_count() > 0);
        for (entity, design, hull, thermal, mass) in ships {
            let Some(&id) = lookup.get(&entity) else {
                continue;
            };
            let cache_key = Arc::as_ptr(&design) as usize;
            let geometry = cache
                .0
                .entry(cache_key)
                .or_insert_with(|| (Arc::downgrade(&design), Arc::new(Geometry::ship(&design))))
                .1
                .clone();
            let local_position = DVec3::ZERO;
            let model = ThermalModel::from(design.as_ref());
            bodies[id].radius = bodies[id]
                .radius
                .max(local_position.length() + geometry.shield_radius);
            bodies[id].feature = bodies[id].feature.min(geometry.feature);
            let start = bodies[id].time;
            bodies[id].members.push(Member {
                entity,
                geometry,
                local_position,
                local_rotation: DQuat::IDENTITY,
                mass: mass.mass,
                inertia: mass.inertia,
                hull,
                thermal,
                model,
                thermal_time: start,
                destroyed: false,
            });
        }
    });
    for (entity, projectile, mass) in world
        .query::<(Entity, &Projectile, &MassProps)>()
        .iter(world)
    {
        let Some(&id) = lookup.get(&entity) else {
            continue;
        };
        let r = projectile.radius_m;
        let geometry = Arc::new(Geometry {
            surface: SharedShape::ball(r),

            radius: r,
            shield_radius: r,
            feature: r * 2.0,
        });
        bodies[id].radius = r;
        bodies[id].feature = 2.0 * r;
        bodies[id].members.push(Member {
            entity,
            geometry,
            local_position: DVec3::ZERO,
            local_rotation: DQuat::IDENTITY,
            mass: mass.mass,
            inertia: mass.inertia,
            hull: projectile.hull_hp,
            thermal: projectile.thermal,
            model: ThermalModel {
                hull_hp: mass.mass,
                hull_heat_capacity_j: mass.mass * osg_ships::thermal::HEAT_STORAGE_J_KG,
                hull_area: 4.0 * std::f64::consts::PI * r * r,
                shield_deployed_kg: 0.0,
                shield_reserve_capacity_kg: 0.0,
                shield_feed_kg_s: 0.0,
                shield_area: 0.0,
            },
            thermal_time: 0.0,
            destroyed: false,
        });
    }
    drop(prepare_profile);
    let epoch = elapsed - dt;
    let rebuild_profile = crate::sim::diagnostics::ProfileScope::new("collision_spatial_rebuild");
    let index_timer = std::time::Instant::now();
    crate::sim::spatial::rebuild_with_collisions(
        world,
        bodies
            .iter()
            .filter(|body| body.alive())
            .map(|body| body.spatial_entry(dt))
            .collect(),
    );
    let build_seconds = index_timer.elapsed().as_secs_f64();
    drop(rebuild_profile);
    {
        let _profile = crate::sim::diagnostics::ProfileScope::new("shield_activation");
        let spatial = world
            .resource::<crate::sim::spatial::SpatialIndex>()
            .service();
        activate(&mut bodies, spatial);
    }
    let combat_profile = crate::sim::diagnostics::ProfileScope::new("collision_prepare_weapons");
    let mut combat = std::collections::BTreeMap::new();
    for (entity, design) in world
        .query_filtered::<(Entity, &ShipDesign), Without<crate::sim::travel::Dormant>>()
        .iter(world)
    {
        if design.0.weapon_parts.is_empty() {
            continue;
        }
        if world
            .get::<crate::sim::travel::ArrivalOffset>(entity)
            .is_some()
        {
            continue;
        }
        let Some(hardware) = hardware::snapshot(world, entity) else {
            continue;
        };
        let mut weapons = hardware.weapons.clone();
        for weapon in &mut weapons {
            weapon.advanced_s = epoch;
            if let Some(command) = &mut weapon.command {
                command.epoch_s = epoch;
            }
        }
        combat.insert(
            entity,
            weapons::WeaponShip {
                design: design.0.clone(),
                inventory: hardware.inventory.clone(),
                weapons,
                operational: design
                    .0
                    .weapon_parts
                    .iter()
                    .map(|&p| hardware.devices[p].operational)
                    .collect(),
            },
        );
    }
    drop(combat_profile);
    let solver_profile = crate::sim::diagnostics::ProfileScope::new("collision_simulate");
    let (report, mut combat) =
        world.resource_scope(|world, mut workspace: Mut<SolverWorkspace>| {
            workspace.weapons = combat;
            workspace.time_s = epoch;
            let spatial = world
                .resource::<crate::sim::spatial::SpatialIndex>()
                .service();
            let mut report =
                simulate_with_workspace(&mut bodies, dt, spatial, &mut workspace, &mut || {
                    world.entity_allocator().alloc()
                });
            report.index_seconds += build_seconds;
            (report, std::mem::take(&mut workspace.weapons))
        });
    drop(solver_profile);
    let _writeback_profile = crate::sim::diagnostics::ProfileScope::new("collision_writeback");
    for shot in &report.shots {
        world
            .spawn_at(
                shot.projectile,
                (
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
                        inertia: DMat3::IDENTITY * (0.4 * shot.mass_kg * shot.radius_m.powi(2)),
                        inertia_inv: DMat3::IDENTITY / (0.4 * shot.mass_kg * shot.radius_m.powi(2)),
                    },
                ),
            )
            .expect("allocated projectile id");
    }
    for (id, body) in bodies.iter().enumerate() {
        let (_, old_pose, mass, _, old_w, force, torque) = if id < roots.len() {
            roots[id]
        } else {
            (
                body.entity,
                PreciseTransform::default(),
                MassProps {
                    mass: body.mass,
                    inertia: body.inertia_inv.inverse(),
                    inertia_inv: body.inertia_inv,
                },
                DVec3::ZERO,
                DVec3::ZERO,
                DVec3::ZERO,
                DVec3::ZERO,
            )
        };
        let gravity = world
            .get::<GravityAcceleration>(body.entity)
            .map_or(DVec3::ZERO, |g| g.0);
        let mut entity = world.entity_mut(body.entity);
        entity.remove::<crate::sim::travel::ArrivalOffset>();
        entity.get_mut::<PreciseTransform>().unwrap().translation_um = body.position;
        entity.get_mut::<PreciseTransform>().unwrap().rotation = body.rotation;
        entity.get_mut::<Velocity>().unwrap().0 = body.velocity;
        *entity.get_mut::<MassProps>().unwrap() = MassProps {
            mass: body.mass,
            inertia: body.inertia_inv.inverse(),
            inertia_inv: body.inertia_inv,
        };
        let angular = body.orientation(dt).1;
        entity.get_mut::<AngularVelocity>().unwrap().0 = angular;
        entity.get_mut::<AccumulatedForce>().unwrap().0 = DVec3::ZERO;
        entity.get_mut::<AccumulatedTorque>().unwrap().0 = DVec3::ZERO;
        if let Some(mut g) = entity.get_mut::<GravityAcceleration>() {
            g.0 = DVec3::ZERO;
        }
        if let Some(mut sample) = entity.get_mut::<AccelerometerState>() {
            let q0 = old_pose.rotation;
            let old_body_w = q0.inverse() * old_w;
            let alpha = q0
                * (mass.inertia_inv
                    * (q0.inverse() * torque - old_body_w.cross(mass.inertia * old_body_w)));
            *sample = AccelerometerState {
                specific_force_body: body.rotation.inverse()
                    * (force / mass.mass - gravity + body.impulse_dv / dt),
                angular_acceleration_body: body.rotation.inverse() * alpha,
                angular_velocity_body: body.rotation.inverse() * angular,
                time_s: Some(elapsed),
            };
        }
        for member in &body.members {
            if let Some(result) = combat.remove(&member.entity) {
                if let Some(mut inventory) = world.get_mut::<ShipInventory>(member.entity) {
                    inventory.0 = result.inventory;
                }
                let parts = world
                    .get::<PartDevices>(member.entity)
                    .map(|p| p.0.clone())
                    .unwrap_or_default();
                let design = world.get::<ShipDesign>(member.entity).map(|d| d.0.clone());
                if let Some(design) = design {
                    for entity in parts {
                        let index = world
                            .get::<InstalledPart>(entity)
                            .and_then(|p| design.part_weapons[p.index]);
                        if let (Some(index), Some(mut weapon)) =
                            (index, world.get_mut::<Weapon>(entity))
                        {
                            weapon.0 = result.weapons[index].clone();
                        }
                    }
                }
            }
            if let Some(mut hull) = world.get_mut::<Hull>(member.entity) {
                hull.0 = member.hull;
            }
            if let Some(mut thermal) = world.get_mut::<ShipThermal>(member.entity) {
                thermal.0 = member.thermal;
            }
            if let Some(mut p) = world.get_mut::<Projectile>(member.entity) {
                p.remaining_s = body.expires_at.unwrap() - dt;
                p.hull_hp = member.hull;
                p.thermal = member.thermal;
            }
        }
    }
    crate::sim::combat::ingest(world, &report, epoch);
    for destruction in &report.destroyed {
        if world.get::<ShipDesign>(destruction.entity).is_some() {
            crate::sim::travel::destroy(world, destruction.entity);
        } else {
            world.despawn(destruction.entity);
        }
    }
    *world.resource_mut::<CollisionStats>() = CollisionStats {
        bodies: bodies.len(),
        candidates: report.candidates,
        detailed_queries: report.detailed,
        impacts: report.impacts,

        dissipated_j: report.dissipated_j,
        index_seconds: report.index_seconds,
        query_seconds: report.query_seconds,
        solve_seconds: report.solve_seconds,
        total_seconds: timer.elapsed().as_secs_f64(),
    };
    world.insert_resource(CollisionReport { epoch, report });
}

#[cfg(test)]
mod tests;
