use super::{RenderSource, ShipMesh, ViewCamera, ViewMember};
use crate::state::{DisplayPose, Optical, OpticalLight, OwnedShip, ViewObservation};
use bevy::{
    asset::embedded_asset,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    mesh::MeshTag,
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
use osg_model::optical::flux_w_m2;
use std::collections::{HashMap, HashSet};

const MESH_PIXELS: f64 = 3.;
const MESH_HYSTERESIS_PIXELS: f64 = 0.5;
const ZERO_MAGNITUDE_FLUX_W_M2: f64 = 3.6e-8;
const REFERENCE_MAGNITUDE: f64 = 6.;

#[derive(Component)]
pub(super) struct VisualContact(pub Option<osg_model::Id>);

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct GlintMaterial {
    #[uniform(0)]
    gain: Vec4,
}

impl Material for GlintMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://osg_client/ui/scene/glints.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }
}

#[derive(Resource)]
struct GlintAssets {
    mesh: Handle<Mesh>,
    material: Handle<GlintMaterial>,
}

#[derive(Component)]
struct Glint;

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "glints.wgsl");
    app.add_plugins(MaterialPlugin::<GlintMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            render
                .before(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GlintMaterial>>,
) {
    commands.insert_resource(GlintAssets {
        mesh: meshes.add(Rectangle::new(2., 2.)),
        material: materials.add(GlintMaterial { gain: Vec4::ONE }),
    });
}

pub(super) fn diameter_pixels(radius: f64, depth: f64, height: f64, fov: f64) -> f64 {
    if depth <= radius {
        return f64::INFINITY;
    }
    radius * height / ((depth * depth - radius * radius).sqrt() * (fov * 0.5).tan())
}

pub(super) fn mesh_needed(pixels: f64, already_spawned: bool) -> bool {
    let threshold = MESH_PIXELS
        + if already_spawned {
            -MESH_HYSTERESIS_PIXELS
        } else {
            MESH_HYSTERESIS_PIXELS
        };
    pixels >= threshold
}

fn sprite_luminance(flux_w_m2: f64) -> f32 {
    let reference_flux = ZERO_MAGNITUDE_FLUX_W_M2 * 10_f64.powf(-0.4 * REFERENCE_MAGNITUDE);
    (flux_w_m2 / reference_flux).clamp(0., 1e6) as f32
}

fn reflection(direction: bevy::math::DVec3, rotation: [f64; 4]) -> f64 {
    let attitude = bevy::math::DQuat::from_array(rotation);
    let aspect = direction.dot(attitude * bevy::math::DVec3::Z).abs();
    0.65 + 0.35 * aspect.powi(12)
}

fn render(
    mut commands: Commands,
    optical: Query<(Entity, &Optical, &DisplayPose, &OpticalLight)>,
    ship_meshes: Query<(&RenderSource, &ViewMember), With<ShipMesh>>,
    owned: Query<(Entity, &OwnedShip)>,
    views: Query<
        (
            Entity,
            &ViewCamera,
            &ViewObservation,
            &Transform,
            Option<&bevy::camera::Exposure>,
        ),
        Without<Glint>,
    >,
    mut glints: Query<
        (
            Entity,
            Option<&RenderSource>,
            &ViewMember,
            &mut Transform,
            &mut MeshTag,
            &mut Visibility,
            &mut RenderLayers,
        ),
        With<Glint>,
    >,
    assets: Res<GlintAssets>,
) {
    let resolved: HashSet<_> = ship_meshes
        .iter()
        .map(|(source, member)| (member.0, source.0))
        .collect();
    let existing: HashMap<_, _> = glints
        .iter()
        .filter_map(|(entity, source, member, ..)| {
            source.map(|source| ((member.0, source.0), entity))
        })
        .collect();
    let mut retained = HashSet::new();

    for (view_entity, view, observation, transform, exposure) in &views {
        let position = view.origin.offset_by(transform.translation.as_dvec3());
        let forward = transform.rotation.as_dquat() * bevy::math::DVec3::NEG_Z;
        let exposure_gain = exposure.map_or(1., |exposure| {
            exposure.exposure() / bevy::camera::Exposure::SUNLIGHT.exposure()
        });
        for (source, optical, pose, light) in &optical {
            let object = &optical.0;
            if object.view != observation.0.id {
                continue;
            }
            let displacement = pose.0.position.relative_to(position);
            let distance = displacement.length();
            let render_source = object
                .known_entity
                .filter(|id| Some(*id) == observation.0.focused_ship)
                .and_then(|id| owned.iter().find(|(_, ship)| ship.0.ship == id))
                .map_or(source, |(entity, _)| entity);
            let visible = !view.private
                && distance > object.radius_m
                && displacement.dot(forward) > 0.
                && !resolved.contains(&(view_entity, render_source));
            let luminance = if visible {
                let flux = flux_w_m2(light.display_w, distance, object.radius_m)
                    * reflection(-displacement / distance, pose.0.rotation);
                sprite_luminance(flux) * exposure_gain
            } else {
                0.
            };
            let pose =
                Transform::from_translation(pose.0.position.relative_to(view.origin).as_vec3());
            let tag = MeshTag(luminance.to_bits());
            let visibility = if visible {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            let layers = RenderLayers::layer(view.layer);
            let entity = if let Some(&entity) = existing.get(&(view_entity, source)) {
                if let Ok((_, _, _, mut current, mut brightness, mut shown, mut layer)) =
                    glints.get_mut(entity)
                {
                    current.set_if_neq(pose);
                    brightness.set_if_neq(tag);
                    shown.set_if_neq(visibility);
                    layer.set_if_neq(layers);
                }
                entity
            } else {
                commands
                    .spawn((
                        Glint,
                        Mesh3d(assets.mesh.clone()),
                        MeshMaterial3d(assets.material.clone()),
                        tag,
                        pose,
                        visibility,
                        // The shader expands the quad in screen space.
                        NoFrustumCulling,
                        bevy::light::NotShadowCaster,
                        bevy::light::NotShadowReceiver,
                        layers,
                        RenderSource(source),
                        ViewMember(view_entity),
                    ))
                    .id()
            };
            retained.insert(entity);
        }
    }
    for (entity, ..) in &glints {
        if !retained.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glints_keep_shared_assets_and_entities_across_view_and_mesh_changes() {
        use osg_model::{GalacticPosition, Id, Pose, ViewState, optical::OpticalObservation};

        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GlintMaterial>>()
            .add_systems(Startup, setup)
            .add_systems(Update, (super::super::camera::setup_views, render).chain());
        let view = app
            .world_mut()
            .spawn(ViewObservation(ViewState {
                focused_ship: None,
                origin: GalacticPosition::ZERO,
                id: 1,
                revision: 1,
            }))
            .id();
        let mut sources = Vec::new();
        for id in 1..=3 {
            let pose = Pose {
                position: GalacticPosition::ZERO.offset_by(bevy::math::DVec3::new(0., 0., -1000.)),
                ..default()
            };
            let mut light = OpticalLight::default();
            light.display_w = 1000.;
            sources.push(
                app.world_mut()
                    .spawn((
                        Optical(OpticalObservation {
                            view: if id == 3 { 2 } else { 1 },
                            id: Id([id; 16]),
                            spatial_instance: Id([id; 16]),
                            iff: None,
                            known_entity: None,
                            contact: None,
                            pose: pose.clone(),
                            radius_m: 1.,
                            luminosity_w: light.display_w,
                            appearance: None,
                            visual: osg_model::ShipVisual {
                                slip_readiness: 0.0,
                                engines: Vec::new(),
                                turrets: Vec::new(),
                                shield: None,
                            },
                        }),
                        DisplayPose(pose),
                        light,
                    ))
                    .id(),
            );
        }
        app.update();
        *app.world_mut().get_mut::<Transform>(view).unwrap() = Transform::IDENTITY;
        app.update();

        let entities: HashMap<_, _> = app
            .world_mut()
            .query_filtered::<(Entity, &RenderSource), With<Glint>>()
            .iter(app.world())
            .map(|(entity, source)| (source.0, entity))
            .collect();
        assert_eq!(entities.len(), 2);
        let glint = entities[&sources[0]];
        let other = entities[&sources[1]];
        assert_eq!(
            app.world().get::<Mesh3d>(glint),
            app.world().get::<Mesh3d>(other)
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<GlintMaterial>>(glint)
                .unwrap()
                .0,
            app.world()
                .get::<MeshMaterial3d<GlintMaterial>>(other)
                .unwrap()
                .0,
        );
        let brightness = f32::from_bits(app.world().get::<MeshTag>(glint).unwrap().0);

        app.world_mut()
            .get_mut::<DisplayPose>(sources[0])
            .unwrap()
            .0
            .position = GalacticPosition::ZERO.offset_by(bevy::math::DVec3::new(0., 0., -2000.));
        app.world_mut().get_mut::<ViewCamera>(view).unwrap().layer = 3;
        app.update();
        let distant = f32::from_bits(app.world().get::<MeshTag>(glint).unwrap().0);
        assert!((brightness / distant - 4.).abs() < 1e-5);
        assert_eq!(
            app.world().get::<Transform>(glint).unwrap().translation.z,
            -2000.
        );
        assert_eq!(
            app.world().get::<RenderLayers>(glint),
            Some(&RenderLayers::layer(3))
        );

        let resolved = app
            .world_mut()
            .spawn((
                ShipMesh {
                    appearance: [0; 32],
                },
                RenderSource(sources[0]),
                ViewMember(view),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(glint),
            Some(&Visibility::Hidden)
        );
        app.world_mut().despawn(resolved);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(glint),
            Some(&Visibility::Visible)
        );

        app.world_mut().get_mut::<ViewCamera>(view).unwrap().private = true;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(glint),
            Some(&Visibility::Hidden)
        );
        app.world_mut().get_mut::<ViewCamera>(view).unwrap().private = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(glint),
            Some(&Visibility::Visible)
        );
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
        assert_eq!(app.world().resource::<Assets<GlintMaterial>>().len(), 1);

        app.world_mut().despawn(sources[0]);
        app.update();
        assert!(app.world().get_entity(glint).is_err());
        assert!(app.world().get_entity(other).is_ok());
        app.world_mut().despawn(view);
        assert!(app.world().get_entity(other).is_err());
    }

    #[test]
    fn projected_sphere_mesh_switch_has_hysteresis() {
        assert!(!mesh_needed(3.2, false));
        assert!(mesh_needed(3.2, true));
        assert!(mesh_needed(3.6, false));
        assert!(!mesh_needed(2.4, true));
        assert!(diameter_pixels(10., 5., 1080., 1.).is_infinite());
    }

    #[test]
    fn apparent_brightness_uses_inverse_square_and_calibrated_magnitudes() {
        let near = flux_w_m2(1., 1000., 1.);
        let far = flux_w_m2(1., 2000., 1.);
        assert!((sprite_luminance(near) / sprite_luminance(far) - 4.).abs() < 1e-6);
        let reference = ZERO_MAGNITUDE_FLUX_W_M2 * 10_f64.powf(-0.4 * REFERENCE_MAGNITUDE);
        assert!((sprite_luminance(reference) - 1.).abs() < 1e-6);
        assert!((sprite_luminance(reference / 100.) - 0.01).abs() < 1e-6);
    }

    #[test]
    fn reflected_glints_follow_attitude_and_cannot_exceed_published_brightness() {
        for i in 0..100 {
            let rotation = bevy::math::DQuat::from_rotation_x(i as f64 * 0.1).to_array();
            let value = reflection(bevy::math::DVec3::Z, rotation);
            assert!((0.65..=1.).contains(&value));
            assert_eq!(value, reflection(bevy::math::DVec3::Z, rotation));
        }
    }
}
