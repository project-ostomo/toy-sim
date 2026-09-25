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
        app.add_systems(Last, discover)
            .add_systems(PostUpdate, animate);
    }
}

fn discover(
    mut commands: Commands,
    nodes: Query<
        (
            Entity,
            &Name,
            &Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
            Option<&bevy::camera::visibility::RenderLayers>,
        ),
        (Added<MeshMaterial3d<StandardMaterial>>, Without<Emitter>),
    >,
    parents: Query<&ChildOf>,
    rings: Query<(), With<SlipRing>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, name, mesh, material, layers) in &nodes {
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
        // The solid channel remains a normal metered surface. Its glow is a
        // separate transparent draw, after the exposure meter samples it.
        instance.emissive = LinearRgba::BLACK;
        commands
            .entity(entity)
            .insert(MeshMaterial3d(materials.add(instance)));
        commands.spawn((
            ChildOf(entity),
            Emitter { ring },
            mesh.clone(),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::BLACK,
                emissive: LinearRgba::BLACK,
                emissive_exposure_weight: 0.0,
                reflectance: 0.0,
                alpha_mode: AlphaMode::Add,
                depth_bias: 1.0,
                ..default()
            })),
            layers.cloned().unwrap_or_default(),
            bevy::light::NotShadowCaster,
            Transform::default(),
        ));
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
        let phase = time.elapsed_secs() * 2.7;
        let surge = 0.8 + 0.2 * (phase + (phase * 0.37).sin()).sin();
        let power = 0.08 + 3.0 * ring.displayed.powi(2) * surge;
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.emissive =
                LinearRgba::rgb(power * (0.22 + 0.1 * phase.sin()), power * 0.55, power);
        }
    }
}
