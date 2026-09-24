//! Simple mounted weapon visuals shared by the editor and simulator.
use bevy::prelude::*;

#[derive(Component)]
pub struct WeaponDefinition(pub osg_ships::weapons::WeaponDef);

#[derive(Component)]
pub struct ModeledWeapon(pub Entity);

#[derive(Component)]
pub struct WeaponVisual {
    pub part_index: usize,
}

pub fn spawn(
    commands: &mut Commands,
    parent: Entity,
    part_index: Option<usize>,
    definition: &osg_ships::weapons::WeaponDef,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let length = bevy::math::DVec3::from_array(definition.muzzle_offset_m).length() as f32;
    let aperture = match definition.mechanism {
        osg_ships::weapons::WeaponMechanism::Gun {
            projectile_radius_m,
            ..
        } => projectile_radius_m,
        osg_ships::weapons::WeaponMechanism::Laser { beam_waist_m, .. } => beam_waist_m,
    };
    let radius = (aperture as f32 * 1.5).max(0.06);
    let mesh = meshes.add(Cuboid::new(radius * 2.0, radius * 2.0, length));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.19, 0.23),
        metallic: 0.8,
        perceptual_roughness: 0.4,
        ..Default::default()
    });
    let pivot = commands
        .spawn((ChildOf(parent), Transform::default(), Visibility::default()))
        .id();
    if let Some(part_index) = part_index {
        commands.entity(pivot).insert(WeaponVisual { part_index });
    }
    commands.spawn((
        ChildOf(pivot),
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_translation(
            Vec3::from_array(definition.muzzle_offset_m.map(|v| v as f32)) * 0.5,
        ),
        Visibility::default(),
    ));
}
