use super::{RenderSource, ShipMesh, ViewCamera, ViewMember};
use crate::state::{DisplayPose, Optical, OpticalLight, OwnedShip, ViewObservation};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    camera::visibility::{NoFrustumCulling, RenderLayers, VisibilityRange},
    mesh::PrimitiveTopology,
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
use toy_sim_model::{ContactRef, optical::flux_w_m2};

pub(super) const MIN_MESH_PIXELS: f64 = 3.;
pub(super) const FULL_MESH_PIXELS: f64 = 8.;
const MESH_HYSTERESIS_PIXELS: f64 = 0.5;
const ZERO_MAGNITUDE_FLUX_W_M2: f64 = 3.6e-8;
const REFERENCE_MAGNITUDE: f64 = 6.;

#[derive(Component)]
pub(super) struct VisualContact(pub Option<ContactRef>);

#[derive(Component, Clone, Copy)]
pub(super) struct MeshLod {
    pub near_m: f64,
    pub far_m: f64,
    pub mesh_fraction: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct GlintMaterial {
    #[uniform(0)]
    gain: Vec4,
}

impl Material for GlintMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://toy_sim_client/ui/scene/glints.wgsl".into()
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

#[derive(Resource, Default)]
struct GlintAssets(Option<Handle<GlintMaterial>>);

#[derive(Component, Default)]
struct ViewGlints {
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "glints.wgsl");
    app.add_plugins(MaterialPlugin::<GlintMaterial>::default())
        .init_resource::<GlintAssets>()
        .add_systems(
            Update,
            apply_lod.after(crate::state::PresentationSet::Render),
        )
        .add_systems(
            PostUpdate,
            (prepare, render)
                .chain()
                .before(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}

pub(super) fn diameter_pixels(radius: f64, depth: f64, height: f64, fov: f64) -> f64 {
    if depth <= radius {
        return f64::INFINITY;
    }
    radius * height / ((depth * depth - radius * radius).sqrt() * (fov * 0.5).tan())
}

pub(super) fn distance_for_pixels(radius: f64, height: f64, fov: f64, pixels: f64) -> f64 {
    radius * (1. + (height / (pixels * (fov * 0.5).tan())).powi(2)).sqrt()
}

pub(super) fn mesh_needed(pixels: f64, already_spawned: bool) -> bool {
    let threshold = MIN_MESH_PIXELS
        + if already_spawned {
            -MESH_HYSTERESIS_PIXELS
        } else {
            MESH_HYSTERESIS_PIXELS
        };
    pixels >= threshold
}

impl MeshLod {
    pub fn at(radius: f64, distance: f64, depth: f64, height: f64, fov: f64) -> Self {
        let scale = distance / depth.max(radius).max(1e-6);
        let near_m = distance_for_pixels(radius, height, fov, FULL_MESH_PIXELS) * scale;
        let far_m = distance_for_pixels(radius, height, fov, MIN_MESH_PIXELS) * scale;
        let glint = ((distance - near_m) / (far_m - near_m).max(1e-6)).clamp(0., 1.);
        Self {
            near_m,
            far_m,
            mesh_fraction: (1. - glint) as f32,
        }
    }
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

fn prepare(mut commands: Commands, views: Query<Entity, (With<ViewCamera>, Without<ViewGlints>)>) {
    for entity in &views {
        commands.entity(entity).insert(ViewGlints::default());
    }
}

fn apply_lod(
    mut commands: Commands,
    roots: Query<(Entity, Option<&MeshLod>), With<ShipMesh>>,
    children: Query<&Children>,
    meshes: Query<Option<&VisibilityRange>, With<Mesh3d>>,
    mut shields: Query<&mut toy_sim_ship_view::thermal::ThermalSphere>,
    mut plumes: Query<&mut toy_sim_ship_view::plume::EnginePlume>,
) {
    for (entity, lod) in &roots {
        let desired = lod.map(|lod| VisibilityRange {
            start_margin: 0. ..0.,
            end_margin: lod.near_m as f32..lod.far_m as f32,
            use_aabb: false,
        });
        for child in children.iter_descendants(entity) {
            if let Some(lod) = lod {
                if let Ok(mut shield) = shields.get_mut(child) {
                    shield.strength *= lod.mesh_fraction;
                }
                if let Ok(mut plume) = plumes.get_mut(child) {
                    plume.output *= lod.mesh_fraction;
                }
            }
            if let Ok(current) = meshes.get(child) {
                match (&desired, current) {
                    (Some(desired), current)
                        if current.is_none_or(|current| current != desired) =>
                    {
                        commands.entity(child).insert(desired.clone());
                    }
                    (None, Some(_)) => {
                        commands.entity(child).remove::<VisibilityRange>();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render(
    mut commands: Commands,
    optical: Query<(Entity, &Optical, &DisplayPose, &OpticalLight)>,
    ship_meshes: Query<(&RenderSource, &ViewMember, Option<&MeshLod>), With<ShipMesh>>,
    owned: Query<(Entity, &OwnedShip)>,
    mut views: Query<(
        Entity,
        &ViewCamera,
        &ViewObservation,
        &Camera,
        &Transform,
        &Projection,
        Option<&bevy::camera::Exposure>,
        &mut ViewGlints,
    )>,
    mut assets: ResMut<GlintAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GlintMaterial>>,
) {
    let resolved: std::collections::HashMap<_, _> = ship_meshes
        .iter()
        .map(|(source, member, lod)| ((member.0, source.0), lod.copied()))
        .collect();
    for (view_entity, view, observation, camera, transform, projection, exposure, mut state) in
        &mut views
    {
        let height = camera
            .physical_viewport_size()
            .map_or(1080., |size| size.y.max(1) as f64);
        let fov = match projection {
            Projection::Perspective(projection) => projection.fov as f64,
            _ => 1.,
        };
        let position = view.origin.offset_by(transform.translation.as_dvec3());
        let right = transform.rotation.as_dquat() * bevy::math::DVec3::X;
        let up = transform.rotation.as_dquat() * bevy::math::DVec3::Y;
        let mut vertices = Vec::new();
        let mut uvs = Vec::new();
        let mut colors = Vec::new();
        if !view.private {
            for (source, object, pose, light) in &optical {
                let object = &object.0;
                if object.view != observation.0.id {
                    continue;
                }
                let displacement = pose.0.position.relative_to(position);
                let distance = displacement.length();
                if distance <= object.radius_m {
                    continue;
                }
                let forward = transform.rotation.as_dquat() * bevy::math::DVec3::NEG_Z;
                if displacement.dot(forward) <= 0. {
                    continue;
                }
                let render_source = object
                    .known_entity
                    .filter(|id| Some(*id) == observation.0.focused_ship)
                    .and_then(|id| owned.iter().find(|(_, ship)| ship.0.ship == id))
                    .map_or(source, |(entity, _)| entity);
                let blend = resolved
                    .get(&(view_entity, render_source))
                    .map_or(1., |lod| {
                        lod.map_or(0., |lod| 1. - lod.mesh_fraction as f64)
                    });
                if blend <= 0. {
                    continue;
                }
                let flux = flux_w_m2(light.display_w, distance, object.radius_m)
                    * reflection(-displacement / distance, pose.0.rotation)
                    * blend;
                let exposure_gain = exposure.map_or(1., |exposure| {
                    exposure.exposure() / bevy::camera::Exposure::SUNLIGHT.exposure()
                });
                let luminance = sprite_luminance(flux) * exposure_gain;
                let half_width = 4. * distance * (fov * 0.5).tan() * 2. / height;
                let color = [luminance, luminance * 0.95, luminance * 0.88, 1.];
                for (x, y) in [
                    (-1., -1.),
                    (1., -1.),
                    (1., 1.),
                    (-1., -1.),
                    (1., 1.),
                    (-1., 1.),
                ] {
                    vertices.push(
                        (displacement + (right * x + up * y) * half_width)
                            .as_vec3()
                            .to_array(),
                    );
                    uvs.push([(x as f32 + 1.) * 0.5, (y as f32 + 1.) * 0.5]);
                    colors.push(color);
                }
            }
        }
        if vertices.is_empty() {
            if let Some(entity) = state.entity {
                commands.entity(entity).insert(Visibility::Hidden);
            }
            continue;
        }
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        let handle = if let Some(handle) = &state.mesh {
            if let Some(mut current) = meshes.get_mut(handle) {
                *current = mesh;
            }
            handle.clone()
        } else {
            let handle = meshes.add(mesh);
            state.mesh = Some(handle.clone());
            handle
        };
        let material = assets
            .0
            .get_or_insert_with(|| materials.add(GlintMaterial { gain: Vec4::ONE }))
            .clone();
        if let Some(entity) = state.entity {
            commands
                .entity(entity)
                .insert((Visibility::Visible, RenderLayers::layer(view.layer)));
        } else {
            state.entity = Some(
                commands
                    .spawn((
                        Mesh3d(handle),
                        MeshMaterial3d(material),
                        Transform::default(),
                        Visibility::Visible,
                        NoFrustumCulling,
                        bevy::light::NotShadowCaster,
                        bevy::light::NotShadowReceiver,
                        RenderLayers::layer(view.layer),
                        ViewMember(view_entity),
                    ))
                    .id(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_sphere_lod_is_conservative_smooth_and_has_hysteresis() {
        for pixels in [MIN_MESH_PIXELS, 5., FULL_MESH_PIXELS] {
            let distance = distance_for_pixels(10., 1080., 1., pixels);
            assert!((diameter_pixels(10., distance, 1080., 1.) - pixels).abs() < 1e-10);
        }
        assert!(!mesh_needed(3.2, false));
        assert!(mesh_needed(3.2, true));
        assert!(mesh_needed(3.6, false));
        assert!(!mesh_needed(2.4, true));
        let near = distance_for_pixels(10., 1080., 1., FULL_MESH_PIXELS);
        let far = distance_for_pixels(10., 1080., 1., MIN_MESH_PIXELS);
        assert_eq!(MeshLod::at(10., near, near, 1080., 1.).mesh_fraction, 1.);
        assert_eq!(MeshLod::at(10., far, far, 1080., 1.).mesh_fraction, 0.);
        let middle = (near + far) * 0.5;
        assert!((MeshLod::at(10., middle, middle, 1080., 1.).mesh_fraction - 0.5).abs() < 1e-6);
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
