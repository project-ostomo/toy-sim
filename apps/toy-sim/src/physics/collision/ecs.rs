use super::*;
use crate::{
    GameState,
    physics::{
        AccelerometerState, AccumulatedForce, AccumulatedTorque, AngularVelocity,
        GravityAcceleration, MassProps, RigidBody, Velocity,
        docking::{DockChild, DockParent},
    },
    precision::PreciseTransform,
    simulation::SimulationSystems,
    vessel::{ShipDesign, ShipHardware},
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
    pub contact_reviews: u64,
    pub dissipated_j: f64,
    pub index_seconds: f64,
    pub query_seconds: f64,
    pub solve_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Resource, Default)]
struct GeometryCache(AHashMap<usize, (std::sync::Weak<CompiledShipDesign>, Arc<Geometry>)>);

pub fn install(app: &mut App) {
    crate::combat_effects::install(app);
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
        ), (With<CollisionBody>, Without<DockChild>)>()
        .iter(world)
        .map(|(e, p, m, v, w, f, t)| (e, *p, *m, v.0, w.0, f.0, t.0))
        .collect();
    roots.sort_unstable_by_key(|r| r.0);
    let mut lookup = AHashMap::default();
    let mut bodies = Vec::with_capacity(roots.len());
    for (id, (entity, p, m, v, w, force, torque)) in roots.iter().copied().enumerate() {
        lookup.insert(entity, id);
        let momentum = p.rotation * (m.inertia * (p.rotation.inverse() * w)) + torque * dt;
        bodies.push(Body {
            projectile: world.get::<Projectile>(entity).is_some(),
            launch_owner: world.get::<Projectile>(entity).and_then(|p| p.launch_owner),
            expires_at: world.get::<Projectile>(entity).map(|p| p.remaining_s),
            entity,
            position: p.translation_um,
            rotation: p.rotation,
            time: 0.0,
            velocity: v + force / m.mass * dt,
            momentum,
            mass: m.mass,
            inertia_inv: m.inertia_inv,
            members: Vec::new(),
            radius: 0.0,
            feature: f64::INFINITY,
            generation: 0,
            impulse_dv: DVec3::ZERO,
            impulse_dw: DVec3::ZERO,
        });
    }
    let ships: Vec<_> = world
        .query::<(
            Entity,
            &ShipDesign,
            &ShipHardware,
            Option<&DockChild>,
            &MassProps,
        )>()
        .iter(world)
        .map(|(e, d, h, dock, mass)| {
            (
                e,
                d.0.clone(),
                h.0.hull,
                h.0.thermal,
                dock.map(|d| (d.parent, d.rel_tf)),
                *mass,
            )
        })
        .collect();
    world.resource_scope(|_, mut cache: Mut<GeometryCache>| {
        cache.0.retain(|_, (design, _)| design.strong_count() > 0);
        for (entity, design, hull, thermal, dock, mass) in ships {
            let root = dock.map_or(entity, |d| d.0);
            let Some(&id) = lookup.get(&root) else {
                continue;
            };
            let cache_key = Arc::as_ptr(&design) as usize;
            let geometry = cache
                .0
                .entry(cache_key)
                .or_insert_with(|| (Arc::downgrade(&design), Arc::new(Geometry::ship(&design))))
                .1
                .clone();
            let local = dock.map_or(PreciseTransform::default(), |d| d.1);
            let local_position = local.translation_um.to_meters_64();
            let model = ThermalModel::from(design.as_ref());
            bodies[id].radius = bodies[id]
                .radius
                .max(local_position.length() + geometry.shield_radius);
            bodies[id].feature = bodies[id].feature.min(geometry.feature);
            bodies[id].members.push(Member {
                entity,
                geometry,
                local_position,
                local_rotation: local.rotation,
                mass: mass.mass,
                inertia: mass.inertia,
                hull,
                thermal,
                model,
                thermal_time: 0.0,
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
            hull: SharedShape::ball(r),
            shield: SharedShape::ball(r),
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
                hull_heat_capacity_j: mass.mass * toy_sim_ships::thermal::HEAT_STORAGE_J_KG,
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
    let epoch = elapsed - dt;
    activate(&mut bodies);
    let mut combat = std::collections::BTreeMap::new();
    for (entity, design, hardware) in world
        .query::<(Entity, &ShipDesign, &ShipHardware)>()
        .iter(world)
    {
        if design.0.weapon_parts.is_empty() {
            continue;
        }
        let mut weapons = hardware.0.weapons.clone();
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
                inventory: hardware.0.inventory.clone(),
                weapons,
                operational: design
                    .0
                    .weapon_parts
                    .iter()
                    .map(|&p| hardware.0.devices[p].operational)
                    .collect(),
            },
        );
    }
    let (report, mut combat) =
        world.resource_scope(|world, mut workspace: Mut<SolverWorkspace>| {
            workspace.weapons = combat;
            workspace.time_s = epoch;
            let report = simulate_with_workspace(&mut bodies, dt, &mut workspace, &mut || {
                world.entity_allocator().alloc()
            });
            (report, std::mem::take(&mut workspace.weapons))
        });
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
                angular_acceleration_body: body.rotation.inverse() * (alpha + body.impulse_dw / dt),
                angular_velocity_body: body.rotation.inverse() * angular,
                time_s: Some(elapsed),
            };
        }
        for member in &body.members {
            if let Some(mut h) = world.get_mut::<ShipHardware>(member.entity) {
                if let Some(result) = combat.remove(&member.entity) {
                    h.0.inventory = result.inventory;
                    h.0.weapons = result.weapons;
                }
                h.0.hull = member.hull;
                h.0.thermal = member.thermal;
            }
            if let Some(mut p) = world.get_mut::<Projectile>(member.entity) {
                p.remaining_s = body.expires_at.unwrap() - dt;
                p.hull_hp = member.hull;
                p.thermal = member.thermal;
            }
            if member.entity != body.entity {
                if let Some(mut mass) = world.get_mut::<MassProps>(member.entity) {
                    *mass = MassProps {
                        mass: member.mass,
                        inertia: member.inertia,
                        inertia_inv: member.inertia.inverse(),
                    };
                }
                let pose = PreciseTransform {
                    translation_um: body
                        .position
                        .offset_by(body.rotation * member.local_position),
                    rotation: body.rotation * member.local_rotation,
                };
                world.entity_mut(member.entity).insert(pose);
                if let Some(mut dock) = world.get_mut::<DockChild>(member.entity) {
                    dock.rel_tf = PreciseTransform {
                        translation_um: GalacticPosition::from_meters(member.local_position),
                        rotation: member.local_rotation,
                    };
                }
            }
        }
    }
    crate::combat_effects::ingest(world, &report, epoch);
    for destruction in &report.destroyed {
        crate::combat_effects::destroy(world, destruction, epoch);
    }
    // Empty dock parents have no gameplay identity and must not keep integrating.
    for body in &bodies {
        if world.get::<DockParent>(body.entity).is_some() && !body.alive() {
            world.despawn(body.entity);
        }
    }
    *world.resource_mut::<CollisionStats>() = CollisionStats {
        bodies: bodies.len(),
        candidates: report.candidates,
        detailed_queries: report.detailed,
        impacts: report.impacts,
        contact_reviews: report.reviews,
        dissipated_j: report.dissipated_j,
        index_seconds: report.index_seconds,
        query_seconds: report.query_seconds,
        solve_seconds: report.solve_seconds,
        total_seconds: timer.elapsed().as_secs_f64(),
    };
}

fn activate(bodies: &mut [Body]) {
    let mut pending = Vec::new();
    for (a, body) in bodies.iter_mut().enumerate() {
        body.members.sort_unstable_by_key(|m| m.entity);
        for (member, m) in body.members.iter_mut().enumerate() {
            let old = m.shielded();
            m.thermal.shield_state = if m.model.shield_deployed_kg == 0.0 {
                abi::SHIELD_ABSENT
            } else if !m.thermal.shield_enabled {
                abi::SHIELD_OFF
            } else if !m.thermal.shield_powered {
                abi::SHIELD_UNPOWERED
            } else if m.thermal.shield_deployed_kg <= 0.0 {
                abi::SHIELD_DEPLETED
            } else if old {
                abi::SHIELD_ACTIVE
            } else {
                pending.push((a, member));
                abi::SHIELD_BLOCKED
            };
        }
    }
    if pending.is_empty() {
        return;
    }

    let proxies: Vec<_> = bodies
        .iter()
        .enumerate()
        .filter(|(_, b)| b.alive())
        .map(|(i, b)| b.proxy(i, 0.0))
        .collect();
    let index = RegionIndex::build(&proxies);
    for (a, member) in pending {
        let anchor = bodies[a].position;
        let pa = bodies[a].shape_pose(member, 0.0, anchor);
        let blocked = index
            .neighbors(bodies[a].proxy(a, 0.0))
            .into_iter()
            .any(|b| {
                let b = &bodies[b as usize];
                if b.launch_owner == Some(bodies[a].members[member].entity) {
                    return false;
                }
                b.members
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.destroyed)
                    .any(|(mb, m)| {
                        query::intersection_test(
                            &pa,
                            bodies[a].members[member].geometry.shield.as_ref(),
                            &b.shape_pose(mb, 0.0, anchor),
                            m.shape().as_ref(),
                        )
                        .expect("supported shield clearance")
                    })
            });
        if !blocked {
            bodies[a].members[member].thermal.shield_state = abi::SHIELD_ACTIVE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_ships::{Catalogue, EXAMPLE_CONTROLLER, ShipState, starter};

    #[test]
    fn ecs_materializes_timed_launches_and_writes_back_their_physics() {
        let mut world = World::new();
        world.init_resource::<GeometryCache>();
        world.init_resource::<SolverWorkspace>();
        world.init_resource::<CollisionStats>();
        let mut time = Time::<Fixed>::from_hz(10.0);
        time.advance_by(std::time::Duration::from_millis(100));
        world.insert_resource(time);

        let catalogue = Catalogue::builtin();
        let design = Arc::new(toy_sim_ships::armed_starter().compile(&catalogue).unwrap());
        let mut hardware = ShipState::new(&design, &catalogue);
        hardware.test_loadout(&design, &catalogue);
        let part = design.weapon_parts[0];
        hardware.settings[design.part_devices[part].unwrap()] =
            Some(toy_sim_ships::DeviceSetting::Weapon(abi::WeaponSetting {
                aim_direction: [0.0, 0.0, -1.0],
                maximum_pointing_error_rad: 0.01,
                valid_until_s: 0.1,
                trigger: 1,
                ..Default::default()
            }));
        hardware.step(&design, &catalogue, 0.1);
        hardware.thermal.shield_enabled = false;
        hardware.thermal.shield_state = abi::SHIELD_OFF;
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let owner = world
            .spawn((
                RigidBody,
                CollisionBody,
                PreciseTransform::default(),
                Velocity(DVec3::ZERO),
                MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
                ShipDesign(design),
                ShipHardware(hardware),
            ))
            .id();

        step(&mut world);
        assert_eq!(
            world.query::<&Projectile>().iter(&world).count(),
            2,
            "{:?}",
            world.get::<ShipHardware>(owner).unwrap().0.weapons[0]
        );
        assert_eq!(
            world.get::<ShipHardware>(owner).unwrap().0.weapons[0].shots_fired,
            2
        );
        for (pose, velocity) in world
            .query_filtered::<(&PreciseTransform, &Velocity), With<Projectile>>()
            .iter(&world)
        {
            assert!(pose.translation_um.to_meters().z < -200.0);
            assert!(velocity.0.z < -4900.0);
        }
    }

    #[test]
    fn barrage_overwhelms_shield_and_preserves_explosion_camera() {
        use crate::scenario::{
            INITIAL_SCENARIO, INITIAL_SLUG_MASS, INITIAL_SLUG_RADIUS, incoming_slugs,
        };

        let universe = crate::orrery::Universe::init(crate::orrery::example_config()).unwrap();
        let planet = universe.get_body(INITIAL_SCENARIO.body).unwrap();
        let epoch = hifitime::Epoch::from_mjd_utc(0.0);
        let (position, velocity) = INITIAL_SCENARIO
            .relative_state(planet.radius, planet.mass)
            .unwrap();
        let mut pose = PreciseTransform {
            translation_um: universe
                .solve_position(INITIAL_SCENARIO.body, epoch)
                .unwrap()
                .offset_by(position),
            ..Default::default()
        };
        pose.look_to(velocity.normalize(), position.normalize());
        let velocity = velocity
            + universe
                .solve_velocity(INITIAL_SCENARIO.body, epoch)
                .unwrap();
        let slugs = incoming_slugs(&universe, epoch, pose.translation_um, velocity, position);

        let mut world = World::new();
        world.init_resource::<GeometryCache>();
        world.init_resource::<SolverWorkspace>();
        world.init_resource::<CollisionStats>();
        world.insert_resource(Time::<Fixed>::from_hz(10.0));
        let catalogue = Catalogue::builtin();
        let design = Arc::new(
            starter(EXAMPLE_CONTROLLER.to_vec())
                .compile(&catalogue)
                .unwrap(),
        );
        let mut hardware = ShipState::new(&design, &catalogue);
        hardware.test_loadout(&design, &catalogue);
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let ship = world
            .spawn((
                RigidBody,
                CollisionBody,
                pose,
                Velocity(velocity),
                MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
                ShipDesign(design.clone()),
                ShipHardware(hardware),
            ))
            .id();
        let inertia = DMat3::IDENTITY * (0.4 * INITIAL_SLUG_MASS * INITIAL_SLUG_RADIUS.powi(2));
        for (position, velocity) in slugs {
            world.spawn((
                Projectile {
                    // This deliberately long-range CCD barrage predates the weapon lifetime.
                    remaining_s: 40.0,
                    ..Projectile::new(INITIAL_SLUG_RADIUS, INITIAL_SLUG_MASS)
                },
                PreciseTransform {
                    translation_um: position,
                    ..Default::default()
                },
                Velocity(velocity),
                MassProps {
                    mass: INITIAL_SLUG_MASS,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
            ));
        }

        world.entity_mut(ship).insert(crate::camera::CameraFocus);
        let camera = world.spawn(crate::camera::MainCamera).id();
        world
            .get_mut::<crate::camera::CameraParams>(camera)
            .unwrap()
            .frame_navigation(GalacticPosition::ZERO, 10000.0, None);

        let initial_reserve = world
            .get::<ShipHardware>(ship)
            .unwrap()
            .0
            .thermal
            .shield_reserve_kg;
        let mut consumed_reserve = false;
        for tick in 0..300 {
            let time = epoch + hifitime::Duration::from_seconds(tick as f64 * 0.1);
            let sources: Vec<_> = universe
                .iter()
                .map(|body| (body, universe.solve_position(&body.name, time).unwrap()))
                .collect();
            world
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_millis(100));
            let (mass, inertia) = {
                let mut hardware = world.get_mut::<ShipHardware>(ship).unwrap();
                hardware.0.step(&design, &catalogue, 0.1);
                hardware.0.mass_properties(&design, &catalogue)
            };
            world.entity_mut(ship).insert(MassProps {
                mass,
                inertia,
                inertia_inv: inertia.inverse(),
            });
            for (pose, mass, mut force) in world
                .query::<(&PreciseTransform, &MassProps, &mut AccumulatedForce)>()
                .iter_mut(&mut world)
            {
                for &(body, source) in &sources {
                    if universe.gravity_applies(&body.name, pose.translation_um) {
                        let offset = source.relative_to(pose.translation_um);
                        force.0 += offset.normalize()
                            * (crate::physics::GRAVITATIONAL_CONSTANT * body.mass * mass.mass
                                / offset.length_squared());
                    }
                }
            }
            step(&mut world);
            let elapsed = (tick + 1) as f64 * 0.1;
            let Some(hardware) = world.get::<ShipHardware>(ship) else {
                println!("barrage: destruction at {elapsed:.1} s");
                assert!(consumed_reserve);
                let focus: Vec<_> = world
                    .query_filtered::<&crate::combat_effects::Explosion, With<crate::camera::CameraFocus>>()
                    .iter(&world)
                    .collect();
                assert_eq!(focus.len(), 1);
                assert_eq!(focus[0].lifetime, 10.0);
                assert!(
                    world
                        .get::<crate::camera::CameraParams>(camera)
                        .unwrap()
                        .navigation_focus
                        .is_none()
                );
                return;
            };
            consumed_reserve |= hardware.0.thermal.shield_reserve_kg < initial_reserve;
        }
        panic!("stock ship survived the entire barrage");
    }

    #[test]
    fn ecs_destroys_a_slug_after_transferring_its_impulse_to_a_ship() {
        let mut world = World::new();
        world.init_resource::<GeometryCache>();
        world.init_resource::<SolverWorkspace>();
        world.init_resource::<CollisionStats>();
        let mut time = Time::<Fixed>::from_hz(10.0);
        time.advance_by(std::time::Duration::from_millis(100));
        world.insert_resource(time);

        let catalogue = Catalogue::builtin();
        let design = Arc::new(
            starter(EXAMPLE_CONTROLLER.to_vec())
                .compile(&catalogue)
                .unwrap(),
        );
        let hardware = ShipState::new(&design, &catalogue);
        let initial_hull = hardware.hull;
        let ship = world
            .spawn((
                RigidBody,
                CollisionBody,
                PreciseTransform::default(),
                MassProps {
                    mass: design.dry_mass,
                    inertia: design.inertia,
                    inertia_inv: design.inertia.inverse(),
                },
                ShipDesign(design),
                ShipHardware(hardware),
            ))
            .id();
        let slug = world
            .spawn((
                Projectile::new(0.01, 0.01),
                MassProps {
                    mass: 0.01,
                    inertia: DMat3::IDENTITY * 0.000001,
                    inertia_inv: DMat3::IDENTITY * 1e6,
                },
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(-DVec3::Z * 100.0),
                    ..Default::default()
                },
                Velocity(DVec3::Z * 10_000.0),
            ))
            .id();

        step(&mut world);
        assert!(world.get_entity(slug).is_err());
        assert!(world.get::<ShipHardware>(ship).unwrap().0.hull < initial_hull);
        assert!(world.get::<Velocity>(ship).unwrap().0.z > 0.0);
        assert_eq!(
            world
                .query::<&crate::combat_effects::Explosion>()
                .iter(&world)
                .count(),
            0
        );
        assert!(world.resource::<CollisionStats>().dissipated_j > 0.0);
    }

    #[test]
    fn deployment_waits_for_clearance_and_uses_stable_entity_order() {
        let mut world = World::new();
        let mut bodies = vec![
            super::super::tests::object(
                &mut world,
                SharedShape::ball(0.5),
                2.0,
                1.0,
                DVec3::ZERO,
                DVec3::ZERO,
                1.0,
            ),
            super::super::tests::object(
                &mut world,
                SharedShape::ball(0.5),
                2.0,
                1.0,
                DVec3::X,
                DVec3::ZERO,
                1.0,
            ),
        ];
        for body in &mut bodies {
            let member = &mut body.members[0];
            member.model.shield_deployed_kg = 5.0;
            member.thermal.shield_deployed_kg = 5.0;
            member.thermal.shield_enabled = true;
            member.thermal.shield_powered = true;
        }
        activate(&mut bodies);
        assert!(
            bodies
                .iter()
                .all(|b| b.members[0].thermal.shield_state == abi::SHIELD_BLOCKED)
        );

        bodies[1].position = GalacticPosition::from_meters(DVec3::X * 3.0);
        activate(&mut bodies);
        assert!(bodies[0].members[0].shielded());
        assert_eq!(
            bodies[1].members[0].thermal.shield_state,
            abi::SHIELD_BLOCKED
        );
        bodies[1].position = GalacticPosition::from_meters(DVec3::X * 5.0);
        activate(&mut bodies);
        assert!(bodies[1].members[0].shielded());
    }
}
