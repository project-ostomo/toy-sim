mod assets;
mod camera;
mod combat_effects;
mod gaia;

mod gui;
mod navigation;
mod orrery;
mod physics;
mod precision;
mod scenario;
mod sensors;
mod simulation;
mod spatial;
mod spatial_tree;
mod starfield;
mod vessel;

// Force Bevy's dynamic linkage for native debug builds only.
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
#[allow(unused_imports)]
use bevy_dylib;

use bevy::render::{
    RenderPlugin,
    settings::{RenderCreation, WgpuFeatures, WgpuSettings},
};
use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, window::PresentMode};
use bevy_asset_loader::loading_state::{LoadingState, LoadingStateAppExt};
use bevy_egui::{EguiGlobalSettings, EguiPlugin};

use crate::{
    camera::MainCameraPlugin, gui::GuiPlugin, orrery::OrreryPlugin, physics::PhysicsPlugin,
    precision::PrecisionPlugin, vessel::VesselsPlugin,
};

#[derive(Clone, Eq, PartialEq, Debug, Hash, Default, States)]
enum GameState {
    #[default]
    Loading,
    Game,
}

/// Dummy non-send data
struct NonSendMarker;

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let ship_path = args
        .iter()
        .position(|a| a == "--ship")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from);
    if args.iter().any(|a| a == "--ship") && ship_path.is_none() {
        anyhow::bail!("--ship requires a file path");
    }
    let mut wasm = toy_sim_ship_wasm::ControllerRuntime::new()?;
    if let Some(path) = &ship_path {
        let ship = toy_sim_ships::ShipBlueprint::load(path)?;
        ship.compile(&toy_sim_ships::Catalogue::builtin())?;
        wasm.instantiate(ship.controller_bytes())?;
    }
    App::new()
        .insert_resource(vessel::WasmRuntime(wasm))
        .insert_resource(vessel::ShipLaunch(ship_path))
        .insert_resource(GlobalAmbientLight::NONE)
        .add_plugins(
            DefaultPlugins
                .build()
                .set(RenderPlugin {
                    render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                        // The stellar skybox stores physical radiance in filtered RGBA32F.
                        features: WgpuSettings::default().features
                            | WgpuFeatures::FLOAT32_FILTERABLE,
                        ..default()
                    })),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_non_send(NonSendMarker)
        .insert_resource(ClearColor(Color::BLACK))
        .init_state::<GameState>()
        .add_loading_state(LoadingState::new(GameState::Loading).continue_to_state(GameState::Game))
        .add_plugins((
            FrameTimeDiagnosticsPlugin::new(10000),
            EguiPlugin::default(),
        ))
        .insert_resource(EguiGlobalSettings {
            enable_absorb_bevy_input_system: true,
            ..default()
        })
        .add_plugins((
            simulation::SimulationPlugin,
            spatial::SpatialPlugin,
            sensors::SensorsPlugin,
            MainCameraPlugin,
            gaia::GaiaPlugin,
            PrecisionPlugin,
            OrreryPlugin,
            starfield::StarfieldPlugin,
            PhysicsPlugin,
            VesselsPlugin,
            toy_sim_ship_view::tracer::TracerPlugin,
            toy_sim_ship_view::explosion::ExplosionPlugin,
            GuiPlugin,
        ))
        // .add_plugins(WorldInspectorPlugin::new())
        // .add_plugins(TransformInterpolationPlugin::interpolate_all())
        .run();
    Ok(())
}
