mod assets;
mod camera;
mod game_logic;
mod gui;
mod orrery;
mod physics;
mod precision;
mod vessel;

use bevy::{
    core_pipeline::auto_exposure::AutoExposurePlugin, diagnostic::FrameTimeDiagnosticsPlugin,
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

/// Dummy non-send resource
struct NonSendMarker;

fn main() {
    App::new()
        .insert_resource(AmbientLight::NONE)
        .add_plugins(DefaultPlugins.build().set(WindowPlugin {
            primary_window: Some(Window {
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .insert_non_send_resource(NonSendMarker)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(Time::from_hz(101.0)) // a prime number
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
            MainCameraPlugin,
            PrecisionPlugin,
            OrreryPlugin,
            PhysicsPlugin,
            VesselsPlugin,
            GuiPlugin,
        ))
        // .add_plugins(WorldInspectorPlugin::new())
        // .add_plugins(TransformInterpolationPlugin::interpolate_all())
        .run();
}
