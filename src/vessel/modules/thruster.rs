use std::f64::consts::PI;

use bevy::{math::DVec3, prelude::*};

use crate::{
    physics::{AccumulatedForce, AccumulatedTorque, aerodynamics::AeroEnv},
    precision::PreciseTransform,
    vessel::consumable::{Consumable, ConsumableTanks},
};

pub fn start_thrusters(app: &mut App) {
    app.add_systems(Startup, load_flame_model);
    app.add_systems(
        FixedUpdate,
        (
            render_flames,
            magic_thrusters,
            electric_fans,
            apply_thrusters,
        ),
    );
}

fn magic_thrusters(mut query: Query<(&mut Thruster, &MagicThruster)>) {
    // magic thrusters produce thrust out of nothing, with instantaneous throttle response
    for (mut thruster, magic) in query.iter_mut() {
        thruster.current_thrust = thruster.throttle * magic.thrust;
        debug!(
            thrust = display(thruster.current_thrust),
            "setting magic thruster thrust"
        );
    }
}

fn apply_thrusters(
    thrusters: Query<(&Thruster, &ChildOf)>,
    mut vessels: Query<(
        &PreciseTransform,
        &mut AccumulatedForce,
        &mut AccumulatedTorque,
    )>,
) {
    for (thruster, child_of) in thrusters {
        if let Ok((ptf, mut force, mut torque)) = vessels.get_mut(child_of.parent()) {
            // Calculate the thrust force vector
            let thrust_force =
                ptf.rotation.mul_vec3(thruster.direction).normalize() * thruster.current_thrust;

            // Add the force to accumulated force
            force.0 += thrust_force;

            // Calculate and add torque (cross product of offset and force)
            let world_offset = ptf.rotation.mul_vec3(thruster.offset);
            torque.0 += world_offset.cross(thrust_force);
        }
    }
}

#[derive(Component, Default)]
#[require(Transform)]
/// A thruster.
pub struct Thruster {
    pub throttle: f64,
    pub current_thrust: f64,
    pub offset: DVec3,
    pub direction: DVec3,
}

#[derive(Component)]
#[require(Thruster)]
pub struct MagicThruster {
    pub thrust: f64,
}

#[derive(Component)]
#[require(MeshMaterial3d<StandardMaterial>, Mesh3d)]
pub struct SimpleThrusterFlame {
    pub radius: f32,
    pub length_per_newton: f32,
}

#[derive(Resource)]
pub struct FlameModel {
    pub mesh: Mesh3d,
    pub material: MeshMaterial3d<StandardMaterial>,
}

fn load_flame_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut mesh = Mesh::from(Cone::new(1.0, 1.0));
    mesh.rotate_by(Quat::from_rotation_arc(Vec3::Y, Vec3::Z));
    let mesh = Mesh3d(meshes.add(mesh));
    let material = MeshMaterial3d(materials.add(StandardMaterial {
        emissive: LinearRgba::WHITE * 100.0,
        emissive_exposure_weight: 0.0,
        ..default()
    }));
    commands.insert_resource(FlameModel { mesh, material });
}

fn render_flames(
    model: Res<FlameModel>,
    thrusters: Query<(
        &Thruster,
        &SimpleThrusterFlame,
        &mut Mesh3d,
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Transform,
    )>,
) {
    for (thruster, flame, mut mesh, mut material, mut transform) in thrusters {
        let flame_length = thruster.current_thrust as f32 * flame.length_per_newton;
        if flame_length == 0.0 {
            *mesh = Default::default();
            *material = Default::default()
        } else {
            transform.translation = thruster.offset.as_vec3();
            transform.translation.z += flame_length / 2.0;
            *mesh = model.mesh.clone();
            *material = model.material.clone();
            transform.scale.z = flame_length;
            transform.scale.x = flame.radius;
            transform.scale.y = flame.radius;
        }
    }
}

#[derive(Component)]
#[require(Thruster)]
pub struct ElectricFan {
    pub power: f64,
    pub efficiency: f64,
    pub diameter: f64,
}

fn electric_fans(
    fans: Query<(&mut Thruster, &ElectricFan, &ChildOf)>,
    mut ships: Query<(&mut ConsumableTanks, &AeroEnv)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs_f64();
    for (mut thruster, fan, ChildOf(ship)) in fans {
        let (mut tank, aero) = ships.get_mut(*ship).unwrap();
        // todo: non-instantaneous power?
        let power_consumption = thruster.throttle * fan.power;
        if tank.consume(Consumable::ElectricJoules, power_consumption * dt) == 0.0 {
            thruster.current_thrust = 0.0;
            continue; // no thrust!
        }
        let effective_power = fan.power * fan.efficiency;
        let a = PI * fan.diameter.powi(2) / 4.0;
        let stat_thrust =
            (2.0 * aero.density * a).powf(1.0 / 3.0) * effective_power.powf(2.0 / 3.0);
        let dyn_thrust = effective_power / aero.airspeed.length().max(0.01);
        thruster.current_thrust = stat_thrust.min(dyn_thrust) * thruster.throttle;
    }
}
