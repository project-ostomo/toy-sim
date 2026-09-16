//! Shared transparent thermal fields. The LUT contains absolute visible spectral
//! radiance, so cooling into the infrared naturally makes a field disappear.
use bevy::{
    asset::embedded_asset,
    mesh::{MeshTag, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};

#[derive(Component)]
pub struct ThermalSphere {
    pub temperature_k: f32,
    pub strength: f32,
}

#[derive(Resource, Clone)]
pub struct ThermalAssets {
    pub mesh: Handle<Mesh>,
    pub material: Handle<ThermalMaterial>,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ThermalMaterial {
    #[uniform(0)]
    colors: [Vec4; 256],
}

impl Material for ThermalMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://toy_sim_ship_view/thermal.wgsl".into()
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
        d: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        d.primitive.cull_mode = None;
        Ok(())
    }
}

pub struct ThermalPlugin;
impl Plugin for ThermalPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "thermal.wgsl");
        app.add_plugins(MaterialPlugin::<ThermalMaterial>::default())
            .add_systems(Startup, prepare)
            .add_systems(
                PostUpdate,
                update.before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

fn prepare(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ThermalMaterial>>,
) {
    let colors =
        std::array::from_fn(|i| blackbody(300.0 + (i as f64) * 9700.0 / 255.0).extend(0.0));
    commands.insert_resource(ThermalAssets {
        mesh: meshes.add(Sphere::new(1.0).mesh().uv(32, 16)),
        material: materials.add(ThermalMaterial { colors }),
    });
}

fn update(mut spheres: Query<(&ThermalSphere, &mut MeshTag, &mut Visibility)>) {
    for (sphere, mut tag, mut visibility) in &mut spheres {
        let t = sphere.temperature_k;
        let temperature = t.clamp(0.0, 65535.0).round() as u32;
        let strength = (sphere.strength.clamp(0.0, 1.0) * 65535.0).round() as u32;
        tag.set_if_neq(MeshTag(temperature | (strength << 16)));
        visibility.set_if_neq(if t >= 300.0 && sphere.strength > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// Planck radiance integrated at 5 nm intervals against the analytic CIE 1931
/// matching-function fit of Wyman, Sloan & Shirley (2013). Multiply by 683 to
/// use photometric XYZ before conversion to linear sRGB. No peak normalization.
pub fn blackbody(temperature: f64) -> Vec3 {
    if temperature < 300.0 {
        return Vec3::ZERO;
    }
    let gaussian = |w: f64, centre: f64, left: f64, right: f64| {
        let t = (w - centre) * if w < centre { left } else { right };
        (-0.5 * t * t).exp()
    };
    let mut xyz = [0.0; 3];
    for nm in (360..=830).step_by(5) {
        let w = nm as f64;
        let lambda = w * 1e-9;
        let radiance =
            1.191042972e-16 / (lambda.powi(5) * (0.01438776877 / (lambda * temperature)).exp_m1());
        let x = 1.056 * gaussian(w, 599.8, 0.0264, 0.0323)
            + 0.362 * gaussian(w, 442.0, 0.0624, 0.0374)
            - 0.065 * gaussian(w, 501.1, 0.0490, 0.0382);
        let y =
            0.821 * gaussian(w, 568.8, 0.0213, 0.0247) + 0.286 * gaussian(w, 530.9, 0.0613, 0.0322);
        let z =
            1.217 * gaussian(w, 437.0, 0.0845, 0.0278) + 0.681 * gaussian(w, 459.0, 0.0385, 0.0725);
        for (out, matching) in xyz.iter_mut().zip([x, y, z]) {
            *out += radiance * matching * 5e-9 * 683.0;
        }
    }
    let [x, y, z] = xyz;
    Vec3::new(
        (3.2406 * x - 1.5372 * y - 0.4986 * z).max(0.0) as f32,
        (-0.9689 * x + 1.8758 * y + 0.0415 * z).max(0.0) as f32,
        (0.0557 * x - 0.2040 * y + 1.0570 * z).max(0.0) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_radiance_fades_and_changes_chromaticity() {
        let cold = blackbody(300.0);
        let red = blackbody(1000.0);
        let hot = blackbody(3000.0);
        assert!(cold.max_element() < 1e-10);
        assert!(red.x > red.y && red.y > red.z);
        assert!(hot.length() > red.length() * 100.0);
        assert!(hot.z / hot.x > red.z / red.x);
    }
}
