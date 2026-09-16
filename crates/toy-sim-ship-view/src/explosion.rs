//! Grey thermal emission for an expanding, thinning cloud.
use bevy::{
    asset::embedded_asset,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ExplosionMaterial {
    #[uniform(0)]
    pub radiance: Vec4,
    #[uniform(1)]
    pub optical_depth: Vec4,
    #[uniform(2)]
    pub flash_radiance: Vec4,
}

impl Material for ExplosionMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://toy_sim_ship_view/explosion.wgsl".into()
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
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub struct ExplosionPlugin;
impl Plugin for ExplosionPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "explosion.wgsl");
        app.add_plugins(MaterialPlugin::<ExplosionMaterial>::default());
    }
}

/// Frame-averaged power of a normalized exponential flash, including its birth.
pub fn flash_power(energy: f64, previous_age: f64, age: f64) -> f64 {
    if age <= 0.0 || age <= previous_age {
        return 0.0;
    }
    let begin = previous_age.max(0.0);
    let emitted = energy * ((-begin / 0.01).exp() - (-age / 0.01).exp());
    emitted / (age - previous_age).max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flash_integrates_to_same_energy_at_different_frame_rates() {
        for fps in [30, 60, 144] {
            let mut energy = 0.0;
            for frame in 0..fps {
                let a = frame as f64 / fps as f64;
                let b = (frame + 1) as f64 / fps as f64;
                energy += flash_power(1000.0, a, b) * (b - a);
            }
            assert!((energy - 1000.0).abs() < 1e-8);
        }
    }
}
