mod orrery_cfg;
use bevy_asset_loader::{
    asset_collection::AssetCollection,
    loading_state::{
        LoadingStateAppExt,
        config::{ConfigureLoadingState, LoadingStateConfig},
    },
};
// Re-export planet classification for external use (e.g., camera behavior)
pub use orrery_cfg::BodyClass;
pub use solver::Orrery;
mod solver;

use bevy::{pbr::NotShadowCaster, prelude::*};
use hifitime::Epoch;
use smol_str::SmolStr;

use crate::{
    GameState, assets::TomlAssetLoader, orrery::orrery_cfg::OrreryCfg, physics::sim_time,
    precision::PreciseTransform,
};

pub struct OrreryPlugin;

impl Plugin for OrreryPlugin {
    fn build(&self, app: &mut App) {
        app.configure_loading_state(
            LoadingStateConfig::new(GameState::Loading).load_collection::<StarSysAssets>(),
        )
        .init_asset::<OrreryCfg>()
        .register_asset_loader(TomlAssetLoader::<OrreryCfg>::new("star.toml"))
        .add_systems(OnEnter(GameState::Game), load_orrery)
        .add_systems(FixedUpdate, move_orrery.run_if(in_state(GameState::Game)));
    }
}

fn move_orrery(
    star_sys: Res<Orrery>,
    time: Res<Time>,
    mut bodies: Query<(&Celestial, &mut PreciseTransform)>,
) {
    let epoch = sim_time(&time);
    for (body, mut ptf) in bodies.iter_mut() {
        ptf.translation_mm = star_sys.solve_position(&body.0, epoch).unwrap();
        ptf.rotation = star_sys.solve_rotation(&body.0, epoch).unwrap();
    }
}

/// Temporary resource holding the handle to the star system configuration asset.
#[derive(Resource, AssetCollection)]
struct StarSysAssets {
    #[asset(path = "stars/taale.star.toml")]
    taale: Handle<OrreryCfg>,
}

/// Once the star system configuration asset is loaded, initializes the star system and spawns celestial bodies.
fn load_orrery(
    mut commands: Commands,
    cfg_handle: Res<StarSysAssets>,
    cfgs: Res<Assets<OrreryCfg>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cfg = cfgs.get(&cfg_handle.taale).unwrap();
    info!("starting star loading");
    let star_sys = Orrery::init(cfg.clone()).unwrap();

    let unit_sphere = Mesh3d(meshes.add(Sphere { radius: 1.0 }.mesh().uv(256, 256)));
    let gray = MeshMaterial3d(materials.add(Color::srgb_u8(128, 128, 128)));
    let star = MeshMaterial3d(materials.add(StandardMaterial {
        emissive: LinearRgba::WHITE * 100.0,
        emissive_exposure_weight: 0.0,
        ..default()
    }));

    for (idx, body) in star_sys.iter().enumerate() {
        let posn = star_sys
            .solve_position(&body.name, Epoch::from_utc_days(0.0))
            .unwrap();

        let mut entity = commands.spawn((
            Celestial(body.name.clone()),
            unit_sphere.clone(),
            gray.clone(),
            PreciseTransform {
                translation_mm: posn,
                ..default()
            },
            Transform {
                scale: Vec3::from_array([body.radius as _, body.radius as _, body.radius as _]),
                ..default()
            },
        ));

        if let BodyClass::Star { lumens } = body.class_params {
            entity.insert((
                star.clone(),
                Star {
                    lumens,
                    color_temp: 5000.0,
                },
                NotShadowCaster,
            ));
        }
    }
    commands.insert_resource(star_sys);
}

#[derive(Component, Default)]
pub struct Celestial(pub SmolStr);

#[derive(Component)]
#[require(Celestial)]
pub struct Star {
    pub lumens: f64,
    pub color_temp: f64,
}
