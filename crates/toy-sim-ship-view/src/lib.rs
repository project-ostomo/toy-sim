//! Optional ship presentation shared by the editor and monitoring application.
use bevy::prelude::*;
use std::collections::BTreeMap;
use toy_sim_ships::*;
pub mod plume;
pub mod thermal;
#[derive(Component)]
pub struct PartVisual {
    pub id: u64,
    pub index: usize,
}
#[derive(Resource, Default)]
pub struct PartVisualAssets {
    pub meshes: BTreeMap<String, Handle<Mesh>>,
    pub materials: BTreeMap<String, Handle<StandardMaterial>>,
    pub plumes: BTreeMap<String, plume::PlumeAssets>,
}
pub fn prepare_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut plume_materials: ResMut<Assets<plume::PlumeMaterial>>,
) {
    let mut assets = PartVisualAssets::default();
    for p in Catalogue::builtin().parts {
        if let Some(plume) = plume::prepare_plume(&p, &mut meshes, &mut plume_materials) {
            assets.plumes.insert(p.id.clone(), plume);
        }
        assets.meshes.insert(
            p.id.clone(),
            meshes.add(Cuboid::from_size(Vec3::from_array(
                p.dimensions.map(|d| d as f32 * GRID as f32),
            ))),
        );
        assets.materials.insert(
            p.id,
            materials.add(StandardMaterial {
                base_color: Color::srgb(p.color[0], p.color[1], p.color[2]),
                perceptual_roughness: 0.7,
                ..default()
            }),
        );
    }
    commands.insert_resource(assets);
}
pub fn spawn_parts(
    commands: &mut Commands,
    parent: Entity,
    design: &CompiledShipDesign,
    assets: &PartVisualAssets,
    loader: &AssetServer,
) {
    for (i, p) in design.parts.iter().enumerate() {
        let tf = Transform::from_translation((p.centre - design.centre).as_vec3())
            .with_rotation(Quat::from_mat3(&p.rotation.as_mat3()));
        let part = commands
            .spawn((
                ChildOf(parent),
                PartVisual {
                    id: p.placed.id,
                    index: i,
                },
                tf,
                Visibility::default(),
            ))
            .id();
        attach_part_body(commands, part, &p.definition, assets, loader);
        if let Some(plume) = assets.plumes.get(&p.placed.prototype) {
            if matches!(p.definition.equipment, toy_sim_ships::Equipment::Rcs { .. }) {
                plume::spawn_rcs_plumes(commands, part, plume);
            } else {
                plume::spawn_plume(commands, part, plume);
            }
        }
    }
}

pub fn attach_part_body(
    commands: &mut Commands,
    entity: Entity,
    definition: &PartDef,
    assets: &PartVisualAssets,
    loader: &AssetServer,
) {
    if let Some(model) = &definition.model {
        let model_entity = commands
            .spawn((
                ChildOf(entity),
                Transform::from_scale(Vec3::splat(definition.model_scale)),
                Visibility::default(),
                WorldAssetRoot(loader.load(format!("{model}#Scene0"))),
            ))
            .id();
        if matches!(definition.equipment, Equipment::Weapon { .. }) {
            commands
                .entity(entity)
                .insert(weapon::ModeledWeapon(model_entity));
        }
    } else {
        commands.entity(entity).insert((
            Mesh3d(assets.meshes[&definition.id].clone()),
            MeshMaterial3d(assets.materials[&definition.id].clone()),
        ));
    }
    if let Equipment::Weapon { weapon } = &definition.equipment {
        commands
            .entity(entity)
            .insert(weapon::WeaponDefinition(weapon.clone()));
    }
}
pub mod explosion;
pub mod tracer;

pub mod weapon;

/// Add movable barrel children once, including in editor previews.
pub fn add_weapon_visuals(
    mut commands: Commands,
    parts: Query<
        (
            Entity,
            Option<&PartVisual>,
            &weapon::WeaponDefinition,
            Option<&weapon::ModeledWeapon>,
        ),
        Added<weapon::WeaponDefinition>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, part, definition, model) in &parts {
        if let Some(model) = model {
            if let Some(part) = part {
                commands.entity(model.0).insert(weapon::WeaponVisual {
                    part_index: part.index,
                });
            }
            continue;
        }
        weapon::spawn(
            &mut commands,
            entity,
            part.map(|part| part.index),
            &definition.0,
            &mut meshes,
            &mut materials,
        );
    }
}
