use super::{BodyClass, Celestial, Star, Universe, orrery_cfg::Body};
use crate::{
    camera::FollowTarget,
    physics::{
        AccumulatedForce, AccumulatedTorque, AngularVelocity, PreviousAcceleration, Velocity,
        WithinSoi, sim_time,
    },
    precision::{InterpolatedTransform, PreciseTransform},
    vessel::{ControlledVessel, Vessel},
};
use bevy::{
    light::{Atmosphere, atmosphere::ScatteringMedium},
    math::DVec3,
    prelude::*,
};
use smol_str::SmolStr;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Component, Clone)]
pub struct CelestialState {
    pub body: Body,
    pub system: usize,
    pub anchor: crate::precision::GalacticPosition,
    pub influence: f64,
    pub velocity: DVec3,
}
#[derive(Resource)]
pub struct CelestialMesh(pub Handle<Mesh>);
#[derive(Resource, Default)]
pub struct ActiveSystems {
    pub entities: BTreeMap<usize, Vec<Entity>>,
    pub ship_systems: BTreeSet<usize>,
}
#[derive(Resource, Default)]
pub struct UniverseDebug {
    pub inspect: Option<SmolStr>,
    pub follow_pending: bool,
    pub relocate: Option<SmolStr>,
    pub search: String,
    pub relocation_revision: u64,
}

pub fn activate(
    mut commands: Commands,
    universe: Res<Universe>,
    mut active: ResMut<ActiveSystems>,
    mut debug: ResMut<UniverseDebug>,
    time: Res<Time<Fixed>>,
    ships: Query<(&PreciseTransform, &Velocity), With<Vessel>>,
    mesh: Res<CelestialMesh>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut media: ResMut<Assets<ScatteringMedium>>,
    mut follow: MessageWriter<FollowTarget>,
    bodies: Query<(Entity, &Celestial)>,
) {
    let mut needed = BTreeSet::new();
    for (p, v) in &ships {
        needed.extend(
            universe
                .tree
                .containing_segment(p.translation_um, v.0 * time.timestep().as_secs_f64()),
        );
    }
    active.ship_systems = needed.clone();
    if let Some(name) = &debug.inspect {
        if let Some(id) = universe.system_for(name) {
            needed.insert(id);
        }
    }
    let remove: Vec<_> = active
        .entities
        .keys()
        .filter(|id| !needed.contains(id))
        .copied()
        .collect();
    for id in remove {
        for entity in active.entities.remove(&id).unwrap() {
            commands.entity(entity).despawn();
        }
    }
    let epoch = sim_time(&time) - hifitime::Duration::from_seconds(time.timestep().as_secs_f64());
    for id in needed {
        if active.entities.contains_key(&id) {
            continue;
        }
        let system = &universe.systems[id];
        let mut entities = Vec::new();
        for b in system.solver.iter() {
            let mut root = commands.spawn((
                Celestial(b.name.clone()),
                CelestialState {
                    body: b.clone(),
                    system: id,
                    anchor: system.solver.anchor,
                    influence: system.influence,
                    velocity: system.solver.solve_velocity(&b.name, epoch).unwrap(),
                },
                crate::spatial::SpatialBody {
                    radius_m: b.radius,
                    occludes: true,
                },
                Visibility::default(),
                PreciseTransform {
                    translation_um: system.solver.solve_position(&b.name, epoch).unwrap(),
                    rotation: system.solver.solve_rotation(&b.name, epoch).unwrap(),
                },
            ));
            if let Some(a) = &b.atmosphere {
                root.insert(Atmosphere {
                    inner_radius: b.radius as f32,
                    outer_radius: (b.radius + a.height) as f32,
                    ground_albedo: Vec3::from_array(a.ground_albedo),
                    medium: media.add(a.scattering_medium()),
                });
            }
            if let BodyClass::Star { lumens } = b.class_params {
                root.insert(Star {
                    lumens,
                    color_temp: 5000.0,
                });
            } else {
                let material = materials.add(StandardMaterial {
                    base_color: Color::srgb_from_array(b.surface_color),
                    perceptual_roughness: 1.0,
                    ..default()
                });
                root.with_children(|p| {
                    p.spawn((
                        Mesh3d(mesh.0.clone()),
                        MeshMaterial3d(material),
                        Transform::from_scale(Vec3::splat(b.radius as f32)),
                    ));
                });
            }
            if debug.follow_pending && debug.inspect.as_ref() == Some(&b.name) {
                follow.write(FollowTarget(root.id()));
                debug.follow_pending = false;
            }
            entities.push(root.id());
        }
        active.entities.insert(id, entities);
    }
    if debug.follow_pending {
        if let Some((entity, _)) = bodies
            .iter()
            .find(|(_, b)| debug.inspect.as_ref() == Some(&b.0))
        {
            follow.write(FollowTarget(entity));
            debug.follow_pending = false;
        }
    }
}

pub fn relocate(
    mut commands: Commands,
    mut debug: ResMut<UniverseDebug>,
    universe: Res<Universe>,
    time: Res<Time<Fixed>>,
    mut ships: Query<
        (
            Entity,
            &mut PreciseTransform,
            &mut InterpolatedTransform,
            &mut Velocity,
            &mut AngularVelocity,
            &mut AccumulatedForce,
            &mut AccumulatedTorque,
            &mut PreviousAcceleration,
        ),
        With<ControlledVessel>,
    >,
    mut follow: MessageWriter<FollowTarget>,
    mut thrusters: Query<(&ChildOf, &mut crate::vessel::Thruster)>,
    mut torquers: Query<(&ChildOf, &mut crate::vessel::Torquer)>,
) {
    let Some(name) = debug.relocate.take() else {
        return;
    };
    let Some(body) = universe.get_body(&name) else {
        return;
    };
    if matches!(body.class_params, BodyClass::Star { .. }) {
        return;
    }
    let epoch = sim_time(&time) - hifitime::Duration::from_seconds(time.timestep().as_secs_f64());
    let (destination, velocity) = arrival(&universe, &name, epoch);
    for (entity, mut p, mut history, mut v, mut w, mut f, mut torque, mut acceleration) in
        &mut ships
    {
        for (parent, mut thruster) in &mut thrusters {
            if parent.parent() == entity {
                thruster.throttle = 0.0;
                thruster.current_thrust = 0.0;
            }
        }
        for (parent, mut torquer) in &mut torquers {
            if parent.parent() == entity {
                torquer.throttle = DVec3::ZERO;
                torquer.torque = DVec3::ZERO;
            }
        }
        history.teleport(&mut p, destination);
        v.0 = velocity;
        w.0 = DVec3::ZERO;
        f.0 = DVec3::ZERO;
        torque.0 = DVec3::ZERO;
        acceleration.0 = DVec3::ZERO;
        commands.entity(entity).remove::<WithinSoi>();
        commands.entity(entity).insert((
            crate::vessel::VesselControlState::default(),
            crate::physics::aerodynamics::AeroEnv::default(),
        ));
        follow.write(FollowTarget(entity));
    }
    debug.inspect = None;
    debug.follow_pending = false;
    debug.relocation_revision += 1;
}
pub fn arrival(
    universe: &Universe,
    name: &str,
    epoch: hifitime::Epoch,
) -> (PreciseTransform, DVec3) {
    let body = universe.get_body(name).unwrap();
    let altitude = 1_000_000f64.max(body.atmosphere.as_ref().map_or(0.0, |a| a.height) + 100_000.0);
    let radius = body.radius + altitude;
    let velocity = DVec3::X * (crate::physics::GRAVITATIONAL_CONSTANT * body.mass / radius).sqrt();
    let mut pose = PreciseTransform {
        translation_um: universe
            .solve_position(name, epoch)
            .unwrap()
            .offset_by(DVec3::Z * radius),
        ..default()
    };
    pose.look_to(velocity.normalize(), DVec3::Y);
    (
        pose,
        universe.solve_velocity(name, epoch).unwrap() + velocity,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        orrery::{
            move_orrery,
            universe::{SyntheticCfg, Universe, generate},
        },
        precision::{FloatingOriginAnchor, GalacticPosition, PrecisionPlugin, PresentationPose},
        simulation::{SimulationPlugin, SimulationSystems},
    };
    use bevy::{state::app::StatesPlugin, time::TimeUpdateStrategy};
    use std::time::Duration;
    #[test]
    fn activation_inspection_relocation_and_interpolation_are_ordered() {
        let mut configs = vec![crate::orrery::example_config()];
        configs.extend(generate(
            &SyntheticCfg {
                count: 1,
                seed: 42,
                min_distance_pc: 2.0,
                max_distance_pc: 2.0,
            },
            GalacticPosition::ZERO,
        ));
        let universe = Universe::from_configs(configs, "Helion system", 1e-8).unwrap();
        let remote = universe.systems[1].solver.anchor;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            TransformPlugin,
            SimulationPlugin,
            PrecisionPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .insert_resource(universe)
        .init_resource::<ActiveSystems>()
        .init_resource::<UniverseDebug>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<ScatteringMedium>>()
        .insert_resource(CelestialMesh(Handle::default()))
        .add_message::<FollowTarget>()
        .add_systems(
            FixedUpdate,
            (relocate, activate)
                .chain()
                .before(SimulationSystems::History),
        )
        .add_systems(
            FixedUpdate,
            move_orrery.in_set(SimulationSystems::Celestials),
        );
        app.world_mut()
            .spawn((FloatingOriginAnchor, PreciseTransform::default()));
        let ship = app
            .world_mut()
            .spawn((
                Vessel {
                    class_name: "test".into(),
                    vessel_name: "test".into(),
                },
                ControlledVessel,
                PreciseTransform::default(),
            ))
            .id();
        app.update();
        app.update();
        assert_eq!(app.world().resource::<ActiveSystems>().entities.len(), 1);
        app.world_mut().resource_mut::<UniverseDebug>().inspect = Some("Synth 001 I".into());
        app.update();
        assert_eq!(app.world().resource::<ActiveSystems>().entities.len(), 2);
        let remote_body = app
            .world_mut()
            .query::<(Entity, &Celestial)>()
            .iter(app.world())
            .find(|(_, c)| c.0 == "Synth 001 I")
            .unwrap()
            .0;
        let present = app.world().get::<PresentationPose>(remote_body).unwrap().0;
        let epoch =
            sim_time(app.world().resource::<Time<Fixed>>()) - hifitime::Duration::from_seconds(0.1);
        let expected = app
            .world()
            .resource::<Universe>()
            .solve_position("Synth 001 I", epoch)
            .unwrap();
        assert!(present.translation_um.relative_to(expected).length() < 0.01);
        app.world_mut().resource_mut::<UniverseDebug>().inspect = None;
        app.update();
        assert!(app.world().get_entity(remote_body).is_err());
        // Re-entry recreates state from the current epoch, without catching up old ticks.
        app.world_mut().resource_mut::<UniverseDebug>().relocate = Some("Synth 001 I".into());
        app.update();
        let active = app.world().resource::<ActiveSystems>();
        assert!(active.entities.contains_key(&1));
        assert!(!active.entities.contains_key(&0));
        let position = app
            .world()
            .get::<PreciseTransform>(ship)
            .unwrap()
            .translation_um;
        assert!(position.relative_to(remote).length() < 1e12);
        assert_eq!(
            app.world().get::<PreviousAcceleration>(ship).unwrap().0,
            DVec3::ZERO
        );
        app.world_mut()
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = GalacticPosition::splat(1 << 90);
        app.update();
        assert!(app.world().resource::<ActiveSystems>().entities.is_empty());
    }
    #[test]
    fn arrival_velocity_inherits_planet_and_gravity_has_a_hard_boundary() {
        let u = Universe::init(crate::orrery::example_config()).unwrap();
        let t = hifitime::Epoch::from_mjd_utc(0.0);
        let (pose, v) = arrival(&u, "Helion I Neris", t);
        let p = pose
            .translation_um
            .relative_to(u.solve_position("Helion I Neris", t).unwrap());
        let relative = v - u.solve_velocity("Helion I Neris", t).unwrap();
        let mass = u.get_body("Helion I Neris").unwrap().mass;
        assert!(relative.dot(p).abs() < 1e-3);
        assert!(
            (relative.length_squared() * p.length()
                / (crate::physics::GRAVITATIONAL_CONSTANT * mass)
                - 1.0)
                .abs()
                < 1e-10
        );
        let s = &u.systems[0];
        assert!(u.gravity_applies(
            "Helion",
            s.solver.anchor.offset_by(DVec3::X * s.influence * 0.999)
        ));
        assert!(!u.gravity_applies(
            "Helion",
            s.solver.anchor.offset_by(DVec3::X * s.influence * 1.001)
        ));
    }
}
