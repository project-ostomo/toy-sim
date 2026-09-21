//! Emissive channels on the fixed slipdrive model, shared with the ship editor.
use bevy::prelude::*;

#[derive(Component, Default)]
pub struct SlipRing {
    pub readiness: f32,
    displayed: f32,
}

#[derive(Component)]
struct Emitter {
    ring: Entity,
}

pub struct SlipRingPlugin;

impl Plugin for SlipRingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (discover, animate).chain());
    }
}

fn discover(
    mut commands: Commands,
    nodes: Query<
        (Entity, &Name, &MeshMaterial3d<StandardMaterial>),
        (Added<MeshMaterial3d<StandardMaterial>>, Without<Emitter>),
    >,
    parents: Query<&ChildOf>,
    rings: Query<(), With<SlipRing>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, name, material) in &nodes {
        if !name.as_str().starts_with("slip_emitter") {
            continue;
        }
        let Some(ring) = parents
            .iter_ancestors(entity)
            .find(|ancestor| rings.contains(*ancestor))
        else {
            continue;
        };
        let Some(mut instance) = materials.get(&material.0).cloned() else {
            continue;
        };
        instance.emissive = LinearRgba::rgb(1.0, 4.0, 8.0);
        commands
            .entity(entity)
            .insert((Emitter { ring }, MeshMaterial3d(materials.add(instance))));
    }
}

fn animate(
    time: Res<Time>,
    mut rings: Query<&mut SlipRing>,
    emitters: Query<(&Emitter, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for mut ring in &mut rings {
        let smoothing = if ring.readiness < ring.displayed {
            0.1
        } else {
            0.15
        };
        let blend = 1.0 - (-time.delta_secs() / smoothing).exp();
        ring.displayed += (ring.readiness.clamp(0.0, 1.0) - ring.displayed) * blend;
    }
    for (emitter, material) in &emitters {
        let Ok(ring) = rings.get(emitter.ring) else {
            continue;
        };
        let power = 8.0 + 60_000.0 * ring.displayed.powi(2);
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.emissive = LinearRgba::rgb(power * 0.22, power * 0.65, power);
        }
    }
}
