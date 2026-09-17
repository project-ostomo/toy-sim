use crate::assets::AssetProblems;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

pub(super) fn window(
    mut contexts: EguiContexts,
    mut problems: ResMut<AssetProblems>,
    server: Res<AssetServer>,
) -> Result {
    if problems.0.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Asset downloads").show(ctx, |ui| {
        problems.0.retain(|path, error| {
            ui.colored_label(egui::Color32::LIGHT_RED, error.error.as_str());
            let retry = ui.push_id(path, |ui| ui.button("Retry").clicked()).inner;
            if retry {
                server.reload(path.clone());
            }
            !retry
        });
    });
    Ok(())
}
