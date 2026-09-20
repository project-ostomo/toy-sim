//! Camera-relative, depth-tested luminous ribbons. Geometry is supplied by the client.
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

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)]
pub struct TracerMaterial {
    #[uniform(0)]
    pub emission: Vec4,
}

impl Material for TracerMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://osg_ship_view/tracer.wgsl".into()
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

pub struct TracerPlugin;
impl Plugin for TracerPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "tracer.wgsl");
        app.add_plugins(MaterialPlugin::<TracerMaterial>::default());
    }
}
