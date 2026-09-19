pub use bevy_egui;
pub use bevy_egui::egui;

pub mod desktop;
pub mod gauges;
pub mod icons;
pub mod instruments;
pub mod mfd;
pub mod parts;
pub mod screens;
pub mod theme;
pub mod units;

pub use mfd::MfdRenderer;

use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiPlugin, EguiPreUpdateSet};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((EguiPlugin::default(), mfd::MfdFontPlugin))
            .add_systems(
                PreUpdate,
                configure_contexts
                    .after(EguiPreUpdateSet::InitContexts)
                    .before(EguiPreUpdateSet::BeginPass),
            );
    }
}

fn configure_contexts(mut contexts: Query<&mut EguiContext, Added<EguiContext>>) {
    for mut context in &mut contexts {
        theme::install(context.get_mut());
    }
}
