//! Cosmetic vacuum exhaust. One shared mesh/material per engine prototype;
//! per-engine delivered thrust travels in MeshTag, preserving GPU batching.
use bevy::{
    asset::embedded_asset,
    camera::{Exposure, visibility::VisibilitySystems},
    mesh::{MeshTag, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use toy_sim_ships::{Equipment, PartDef};

pub struct PlumePlugin;
impl Plugin for PlumePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "plume.wgsl");
        app.add_plugins(MaterialPlugin::<PlumeMaterial>::default())
            .add_systems(
                PostUpdate,
                update_plumes.before(VisibilitySystems::VisibilityPropagate),
            );
    }
}

#[derive(Component)]
pub struct EnginePlume {
    /// Actual thrust / rated thrust, supplied by the simulation or editor preview.
    pub output: f32,
    pub max_thrust_n: f64,
}

/// Signed device axis producing thrust; exhaust points in the opposite direction.
#[derive(Component)]
pub struct RcsNozzle {
    pub axis: usize,
    pub sign: f64,
}

#[derive(Clone)]
pub struct PlumeAssets {
    mesh: Handle<Mesh>,
    material: Handle<PlumeMaterial>,
    origin: Vec3,
    max_thrust_n: f64,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct PlumeMaterial {
    #[uniform(0)]
    shape: Vec4, // length, opening radius, expansion slope, end radius
    #[uniform(0)]
    emission: Vec4, // linear RGB, emission scale at unit exposure
    #[uniform(0)]
    falloff: Vec4, // axial, radial, noise strength, noise scale
    #[uniform(0)]
    animation: Vec4, // noise speed, unused padding
}
impl Material for PlumeMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://toy_sim_ship_view/plume.wgsl".into()
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
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The shader selects the entry face, or the exit face when inside.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub(crate) fn prepare_plume(
    part: &PartDef,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<PlumeMaterial>,
) -> Option<PlumeAssets> {
    if let Equipment::Rcs { thrust_n, .. } = &part.equipment {
        return Some(PlumeAssets {
            mesh: meshes.add(Cuboid::new(0.38, 0.38, 1.5)),
            material: materials.add(PlumeMaterial {
                shape: Vec4::new(1.5, 0.04, 0.1, 0.19),
                emission: Vec4::new(0.45, 0.65, 1.0, 2.0 / Exposure::default().exposure()),
                falloff: Vec4::new(1.5, 2.0, 0.15, 0.1),
                animation: Vec4::new(3.0, 0.0, 0.0, 0.0),
            }),
            origin: Vec3::Z * 1.02,
            max_thrust_n: *thrust_n,
        });
    }

    let (thrust_n, p) = match &part.equipment {
        Equipment::Engine {
            thrust_n,
            plume: Some(plume),
            ..
        }
        | Equipment::ThermalEngine {
            thrust_n,
            plume: Some(plume),
            ..
        }
        | Equipment::MicropulseEngine {
            thrust_n,
            plume: Some(plume),
            ..
        } => (*thrust_n, plume),
        _ => return None,
    };
    let slope = p.expansion_half_angle_rad.tan();
    let radius = p.nozzle_radius_m + p.length_m * slope;
    Some(PlumeAssets {
        mesh: meshes.add(Cuboid::new(2. * radius, 2. * radius, p.length_m)),
        material: materials.add(PlumeMaterial {
            shape: Vec4::new(p.length_m, p.nozzle_radius_m, slope, radius),
            // Artistic intensity is calibrated at Bevy's default EV100. The
            // actual camera exposure still affects it, and HDR reaches bloom.
            emission: Vec3::from_array(p.color_linear_rgb)
                .extend(p.intensity / Exposure::default().exposure()),
            falloff: Vec4::new(
                p.axial_falloff,
                p.radial_falloff,
                p.noise_strength,
                p.noise_scale_m,
            ),
            animation: Vec4::new(p.noise_speed_m_s, 0., 0., 0.),
        }),
        origin: Vec3::from_array(p.origin_m) + Vec3::Z * p.length_m * 0.5,
        max_thrust_n: thrust_n,
    })
}

/// Parent to the part so placement, rotation and floating-origin changes apply
/// naturally. The proxy bounds are visual only and never enter ship physics.
pub fn spawn_plume(commands: &mut Commands, part: Entity, assets: &PlumeAssets) -> Entity {
    commands
        .spawn((
            ChildOf(part),
            EnginePlume {
                output: 0.,
                max_thrust_n: assets.max_thrust_n,
            },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            MeshTag(0),
            Transform::from_translation(assets.origin),
            Visibility::Hidden,
        ))
        .id()
}

/// Six independently driven exhaust volumes on the faces of an RCS block.
pub fn spawn_rcs_plumes(commands: &mut Commands, part: Entity, assets: &PlumeAssets) {
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let exhaust = Vec3::AXES[axis] * -sign as f32;
            let rotation = Quat::from_rotation_arc(Vec3::Z, exhaust);
            let plume = spawn_plume(commands, part, assets);
            commands.entity(plume).insert((
                RcsNozzle { axis, sign },
                Transform::from_translation(rotation * assets.origin).with_rotation(rotation),
            ));
        }
    }
}

fn update_plumes(mut plumes: Query<(&EnginePlume, &mut MeshTag, &mut Visibility)>) {
    for (plume, mut tag, mut visibility) in &mut plumes {
        let output = if plume.output.is_finite() {
            plume.output.clamp(0., 1.)
        } else {
            0.
        };
        tag.set_if_neq(MeshTag(output.to_bits()));
        visibility.set_if_neq(if output > 0. {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}
