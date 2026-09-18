mod catalogue;
mod inspector;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InspectorTab {
    Part,
    Ship,
}

use toy_sim_ui::bevy_egui::{EguiContexts, egui};
pub fn editor(
    mut contexts: EguiContexts,
    mut e: ResMut<Editor>,
    camera: Single<(&Camera, &GlobalTransform), With<viewport::EditorCamera>>,
    window: Single<&Window>,
    previews: Res<previews::PartPreviews>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    e.pixels_per_point = ctx.pixels_per_point();
    let mut root = egui::Ui::new(
        ctx.clone(),
        "editor-root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    if let Some(child) = &mut e.child {
        if let Ok(Some(status)) = child.try_wait() {
            e.status = format!("Simulation exited: {status}");
            e.child = None;
        }
    }
    egui::Panel::top("toolbar").show(&mut root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("SHIP EDITOR");
            if ui.button("New").clicked() {
                e.replace_ship(ShipBlueprint::default());
            }
            if ui.button("Starter").clicked() {
                e.replace_ship(ntr_patrol());
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut e.file).desired_width(220.));
            if ui.button("Open").clicked() {
                match ShipBlueprint::load(&e.file) {
                    Ok(s) => {
                        e.replace_ship(s);
                        e.dirty = false;
                    }
                    Err(err) => e.status = format!("Open failed: {err:#}"),
                }
            }
            if ui.button(if e.dirty { "Save *" } else { "Save" }).clicked() {
                match e.ship.save(&e.file) {
                    Ok(()) => {
                        e.dirty = false;
                        e.status = "Saved".into();
                    }
                    Err(err) => e.status = format!("Save failed: {err:#}"),
                }
            }
            if ui
                .add_enabled(!e.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                if let Some(old) = e.undo.pop() {
                    let current = std::mem::replace(&mut e.ship, old);
                    e.redo.push(current);
                    e.changed();
                }
            }
            if ui
                .add_enabled(!e.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                if let Some(next) = e.redo.pop() {
                    let current = std::mem::replace(&mut e.ship, next);
                    e.undo.push(current);
                    e.changed();
                }
            }
            ui.separator();
            ui.selectable_value(&mut e.devices_mode, false, "Assembly");
            if ui
                .selectable_value(&mut e.devices_mode, true, "Systems")
                .clicked()
            {
                e.cancel_placement();
            }
            if ui
                .add_enabled(
                    e.validation.is_ok() && e.child.is_none(),
                    egui::Button::new("Launch sim"),
                )
                .clicked()
            {
                e.status = match e.launch() {
                    Ok(()) => "Simulation launched with a temporary snapshot".into(),
                    Err(err) => format!("Launch failed: {err:#}"),
                };
            }
        });
    });
    egui::Panel::bottom("status")
        .resizable(true)
        .default_size(72.)
        .show(&mut root, |ui| {
            match &e.validation {
                Ok(message) => {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, message);
                }
                Err(message) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                }
            }
            ui.label(&e.status);
            if !e.devices_mode {
                ui.small("Click sockets to attach · right-drag orbit · middle-drag pan · wheel zoom · R roll · Delete subtree · Esc cancel");
            }
        });
    egui::Panel::left("parts-catalogue")
        .resizable(true)
        .default_size(288.)
        .min_size(200.)
        .max_size((ctx.viewport_rect().width() * 0.4).max(200.))
        .show(&mut root, |ui| {
            if e.devices_mode {
                ui.heading("Ship systems");
                ui.label("Standard avionics and automatic flight control.");
                ui.small("Physical placement determines actuator capabilities.");
            } else {
                catalogue::show(ui, &mut e, &previews);
            }
        });
    egui::Panel::right("part-inspector")
        .resizable(true)
        .default_size(352.)
        .min_size(280.)
        .max_size((ctx.viewport_rect().width() * 0.45).max(280.))
        .show(&mut root, |ui| inspector::show(ui, &mut e, &previews));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut root, |ui| {
            e.viewport = ui.available_rect_before_wrap();
            if e.devices_mode {
                crate::devices::panel(ui, &mut e);
            } else {
                let (c, g) = *camera;
                crate::viewport::interact(ui, &mut e, c, g, &window);
            }
        });
    Ok(())
}
