mod assets;
mod camera;
mod gaia;

mod gui;
mod orrery;
mod physics;
mod precision;
mod sensors;
mod simulation;
mod spatial;
mod starfield;
mod vessel;

// Force Bevy's dynamic linkage for native debug builds only.
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
#[allow(unused_imports)]
use bevy_dylib;

use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin, post_process::auto_exposure::AutoExposurePlugin,
    prelude::*, window::PresentMode,
};
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

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--benchmark-gaia") {
        let path = args
            .get(2)
            .expect("usage: toy-sim --benchmark-gaia DATABASE");
        gaia::benchmark(std::path::Path::new(path)).expect("Gaia benchmark failed");
        return;
    }
    App::new()
        .insert_resource(GlobalAmbientLight::NONE)
        .add_plugins(DefaultPlugins.build().set(WindowPlugin {
            primary_window: Some(Window {
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_non_send(NonSendMarker)
        .insert_resource(ClearColor(Color::BLACK))
        .init_state::<GameState>()
        .add_loading_state(LoadingState::new(GameState::Loading).continue_to_state(GameState::Game))
        .add_plugins((
            FrameTimeDiagnosticsPlugin::new(10000),
            AutoExposurePlugin,
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
            GuiPlugin,
        ))
        // .add_plugins(WorldInspectorPlugin::new())
        // .add_plugins(TransformInterpolationPlugin::interpolate_all())
        .run();
}
