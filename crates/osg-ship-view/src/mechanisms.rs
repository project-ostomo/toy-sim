use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct MechanismTime(pub f64);

#[derive(Component)]
struct Rotor {
    angular_velocity: f64,
    rest: Quat,
}

pub struct MechanismPlugin;

impl Plugin for MechanismPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MechanismTime>()
            .add_systems(Update, discover)
            .add_systems(PostUpdate, animate.before(TransformSystems::Propagate));
    }
}

fn discover(
    mut commands: Commands,
    nodes: Query<(Entity, &bevy::gltf::GltfExtras, &Transform), Added<bevy::gltf::GltfExtras>>,
) {
    for (entity, extras, transform) in &nodes {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&extras.value) else {
            continue;
        };
        let Some(rate) = value
            .get("angular_velocity_rad_s")
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
        else {
            continue;
        };
        commands.entity(entity).insert(Rotor {
            angular_velocity: rate,
            rest: transform.rotation,
        });
    }
}

fn animate(clock: Res<MechanismTime>, mut rotors: Query<(&Rotor, &mut Transform)>) {
    for (rotor, mut transform) in &mut rotors {
        let angle = (rotor.angular_velocity * clock.0).rem_euclid(std::f64::consts::TAU) as f32;
        transform.rotation = rotor.rest * Quat::from_rotation_z(angle);
    }
}
