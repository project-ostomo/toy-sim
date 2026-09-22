//! Point-sprite star field: one quad per snapshot star, with direction and
//! brightness evaluated on the GPU each frame from the camera's offset to the
//! snapshot origin.
use super::{ViewSky, snapshot};
use crate::ui::scene::{ViewCamera, ViewMember};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    camera::visibility::{NoFrustumCulling, RenderLayers},
    mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError, VertexFormat,
    },
    shader::ShaderRef,
};
use osg_space::GalacticPosition;
use std::f64::consts::PI;

/// Positions are uploaded in units of 1e12 m so squared distances out to
/// galactic scale stay finite in f32.
pub(super) const UNIT_M: f64 = 1e12;

const ATTRIBUTE_FLUX: MeshVertexAttribute =
    MeshVertexAttribute::new("StarFlux", 2_418_663_009, VertexFormat::Float32x4);
const ATTRIBUTE_CORNER: MeshVertexAttribute =
    MeshVertexAttribute::new("StarCorner", 2_418_663_010, VertexFormat::Float32x2);
const CORNERS: [[f32; 2]; 4] = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]];

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(super) struct StarMaterial {
    /// xyz: camera offset from the snapshot origin in `UNIT_M`; w: fade-out
    /// illuminance in lux.
    #[uniform(0)]
    camera: Vec4,
}

impl Material for StarMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://osg_client/ui/scene/sky/sprites.wgsl".into()
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

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ATTRIBUTE_FLUX.at_shader_location(1),
            ATTRIBUTE_CORNER.at_shader_location(2),
        ])?];
        Ok(())
    }
}

/// The star field drawn for one view.
#[derive(Component)]
pub(super) struct StarField {
    view: Entity,
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "sprites.wgsl");
    app.add_plugins(MaterialPlugin::<StarMaterial>::default());
}

/// One quad per star. `None` for an empty snapshot, which draws nothing.
pub(super) fn star_mesh(stars: &[snapshot::Sprite]) -> Option<Mesh> {
    if stars.is_empty() {
        return None;
    }
    let mut positions = Vec::with_capacity(stars.len() * 4);
    let mut fluxes = Vec::with_capacity(stars.len() * 4);
    let mut corners = Vec::with_capacity(stars.len() * 4);
    let mut indices = Vec::with_capacity(stars.len() * 6);
    for (index, star) in stars.iter().enumerate() {
        let position = (star.offset / UNIT_M).as_vec3().to_array();
        let flux = star.luminosity / (4.0 * PI * UNIT_M * UNIT_M);
        let [r, g, b] = star.colour.map(|c| (c as f64 * flux) as f32);
        for corner in CORNERS {
            positions.push(position);
            fluxes.push([r, g, b, flux as f32]);
            corners.push(corner);
        }
        let base = index as u32 * 4;
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Some(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(ATTRIBUTE_FLUX, fluxes)
        .with_inserted_attribute(ATTRIBUTE_CORNER, corners)
        .with_inserted_indices(Indices::U32(indices)),
    )
}

fn uniform(camera: GalacticPosition, snapshot: &snapshot::Snapshot) -> Vec4 {
    let offset = camera.relative_to(snapshot.origin) / UNIT_M;
    offset
        .as_vec3()
        .extend((snapshot.cutoff / (4.0 * PI)) as f32)
}

/// Keeps one star field per public view, showing that view's current
/// snapshot mesh at the camera's current offset.
pub(super) fn sync(
    mut commands: Commands,
    cameras: Query<(Entity, &ViewCamera, &Transform, &ViewSky)>,
    mut fields: Query<(
        Entity,
        &StarField,
        &mut Mesh3d,
        &MeshMaterial3d<StarMaterial>,
        &mut Visibility,
        &mut RenderLayers,
    )>,
    mut materials: ResMut<Assets<StarMaterial>>,
) {
    let mut drawn = std::collections::HashSet::new();
    for (view, camera, transform, sky) in &cameras {
        let (Some(snapshot), Some(mesh)) = (&sky.snapshot, &sky.mesh) else {
            continue;
        };
        let value = uniform(
            camera.origin.offset_by(transform.translation.as_dvec3()),
            snapshot,
        );
        let visibility = if camera.private {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        let layers = RenderLayers::layer(camera.layer);
        drawn.insert(view);
        if let Some((_, _, mut current, material, mut shown, mut layer)) =
            fields.iter_mut().find(|(_, field, ..)| field.view == view)
        {
            if current.0 != *mesh {
                current.0 = mesh.clone();
            }
            if let Some(mut material) = materials.get_mut(&material.0) {
                material.camera = value;
            }
            shown.set_if_neq(visibility);
            layer.set_if_neq(layers);
            continue;
        }
        commands.spawn((
            StarField { view },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(materials.add(StarMaterial { camera: value })),
            Transform::default(),
            visibility,
            // The vertex shader places every star; the mesh bounds are meaningless.
            NoFrustumCulling,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
            layers,
            ViewMember(view),
        ));
    }
    for (entity, field, ..) in &fields {
        if !drawn.contains(&field.view) {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    fn sprite(offset: bevy::math::DVec3) -> snapshot::Sprite {
        snapshot::Sprite {
            offset,
            luminosity: osg_stars::SOLAR_LUMENS,
            colour: [1.0, 0.5, 0.25],
        }
    }

    #[test]
    fn empty_snapshots_have_no_mesh() {
        assert!(star_mesh(&[]).is_none());
    }

    #[test]
    fn each_star_is_one_indexed_quad() {
        let mesh =
            star_mesh(&[sprite(bevy::math::DVec3::X), sprite(bevy::math::DVec3::Y)]).unwrap();
        assert_eq!(mesh.count_vertices(), 8);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("expected u32 indices");
        };
        assert_eq!(indices, &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn shader_units_reproduce_physical_illuminance() {
        let distance = 1.0e17;
        let star = sprite(bevy::math::DVec3::Z * distance);
        let mesh = star_mesh(std::slice::from_ref(&star)).unwrap();
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(VertexAttributeValues::Float32x4(fluxes)) = mesh.attribute(ATTRIBUTE_FLUX) else {
            panic!("flux");
        };
        // What the vertex shader computes, in f32 like the GPU.
        let p = Vec3::from_array(positions[0]);
        let lux = fluxes[0][3] / p.length_squared();
        let expected = osg_stars::SOLAR_LUMENS / (4.0 * PI * distance * distance);
        assert!(
            ((lux as f64) / expected - 1.0).abs() < 1e-5,
            "{lux} vs {expected}"
        );
        assert!((fluxes[0][1] / fluxes[0][3] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn uniform_carries_camera_offset_and_cutoff_in_shader_units() {
        let origin = GalacticPosition::from_meters(bevy::math::DVec3::new(1.0e16, 0.0, 0.0));
        let snapshot = snapshot::Snapshot::new(vec![], origin, 6.0, 0, 0, 4.0 * PI);
        let camera = origin.offset_by(bevy::math::DVec3::new(0.0, 2.0e12, 3.0e3));
        let value = uniform(camera, &snapshot);
        assert!((value.y - 2.0).abs() < 1e-6);
        assert!(value.x.abs() < 1e-6);
        assert!((value.w - 1.0).abs() < 1e-6);
    }
}
