use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    image::ImageSampler,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
        TextureDimension, TextureFormat,
    },
    shader::ShaderRef,
};

#[derive(Resource)]
pub struct ExplosionAssets {
    pub mesh: Handle<Mesh>,
    pub material: Handle<ExplosionMaterial>,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ExplosionMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub atlas: Handle<Image>,
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
        if let Some(depth) = &mut descriptor.depth_stencil {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

pub struct ExplosionPlugin;

impl Plugin for ExplosionPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "explosion.wgsl");
        app.add_plugins(MaterialPlugin::<ExplosionMaterial>::default())
            .add_systems(Startup, prepare);
    }
}

fn prepare(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ExplosionMaterial>>,
) {
    commands.insert_resource(ExplosionAssets {
        mesh: meshes.add(Rectangle::new(2., 2.)),
        material: materials.add(ExplosionMaterial {
            atlas: images.add(atlas()),
        }),
    });
}

fn atlas() -> Image {
    const TILE: u32 = 64;
    let mut pixels = Vec::with_capacity((TILE * TILE * 4 * 4) as usize);
    for y in 0..TILE {
        for x in 0..TILE * 4 {
            let tile = x / TILE;
            let p =
                Vec2::new((x % TILE) as f32 + 0.5, y as f32 + 0.5) / TILE as f32 * 2. - Vec2::ONE;
            let r2 = p.length_squared();
            let edge = (1. - r2).max(0.).powi(2);
            let (color, alpha) = if tile == 0 {
                let core = (-24. * r2).exp();
                (
                    Vec3::new(1., 0.55 + 0.45 * core, 0.15 + 0.85 * core),
                    edge * (-5. * r2).exp(),
                )
            } else {
                let phase = tile as f32 * 2.1;
                let lobes = 0.65
                    + 0.2 * (p.x * 9. + phase).sin() * (p.y * 7. - phase).cos()
                    + 0.15 * (p.x * 19. + p.y * 13. + phase).sin();
                (Vec3::new(1., 0.32, 0.06), edge * lobes)
            };
            pixels.extend(color.to_array().map(|c| (c * 255.).round() as u8));
            pixels.push((alpha.clamp(0., 1.) * 255.).round() as u8);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: TILE * 4,
            height: TILE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

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
