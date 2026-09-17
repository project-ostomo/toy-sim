use super::*;
use bevy_egui::{EguiContexts, egui};
pub fn editor(
    mut contexts: EguiContexts,
    mut e: ResMut<Editor>,
    camera: Single<(&Camera, &GlobalTransform), With<viewport::EditorCamera>>,
    window: Single<&Window>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    e.pixels_per_point = ctx.pixels_per_point();
    toy_sim_ship_view::square_style(ctx);
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
        ui.horizontal(|ui| {
            ui.strong("SHIP EDITOR");
            if ui.button("New").clicked() {
                e.edit(|s| *s = ShipBlueprint::default());
                e.selected = None;
            }
            if ui.button("Starter").clicked() {
                e.edit(|s| *s = starter(EXAMPLE_CONTROLLER.to_vec()));
                e.camera_target = Vec3::new(0.5, 0.5, 4.5);
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut e.file).desired_width(220.));
            if ui.button("Open").clicked() {
                match ShipBlueprint::load(&e.file) {
                    Ok(s) => {
                        e.edit(|old| *old = s);
                        e.dirty = false;
                        e.selected = None;
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
            ui.selectable_value(&mut e.devices_mode, true, "Systems");
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
    egui::Panel::bottom("status").resizable(true).default_size(72.).show(&mut root,|ui|{
        match &e.validation {Ok(s)=>{ui.colored_label(egui::Color32::LIGHT_GREEN,s);},Err(s)=>{ui.colored_label(egui::Color32::LIGHT_RED,s);}}
        ui.label(&e.status);
        if !e.devices_mode {
            ui.small("Assembly: click to place/select · right-drag orbit · middle-drag pan · wheel zoom · R rotate · Delete remove · Esc cancel");
        }
    });
    egui::Panel::left("palette")
        .resizable(true)
        .default_size(190.)
        .show(&mut root, |ui| {
            if e.devices_mode {
                ui.heading("Ship systems");
                ui.label("Standard avionics and automatic flight control.");
                ui.small("Physical placement determines actuator capabilities.");
            } else {
                ui.heading("Predefined parts");
                for p in e.catalogue.parts.clone() {
                    if ui
                        .selectable_label(e.place.as_ref() == Some(&p.id), &p.title)
                        .clicked()
                    {
                        e.place = Some(p.id);
                        e.selected = None;
                    }
                }
                if ui.button("Select mode").clicked() {
                    e.place = None;
                }
                ui.separator();
                ui.label(format!("Placement rotation {} / 24", e.orientation + 1));
                if ui.button("Rotate placement (R)").clicked() {
                    e.orientation = (e.orientation + 1) % 24;
                }
            }
        });
    egui::Panel::right("inspector")
        .resizable(true)
        .default_size(270.)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Ship");
                if !e.devices_mode {
                    ui.add(
                        egui::Slider::new(&mut e.preview_thrust, 0.0..=1.0).text("Preview thrust"),
                    );
                }
                let mut name = e.ship.name.clone();
                if ui.text_edit_singleline(&mut name).changed() {
                    e.edit(|s| s.name = name);
                }
                ui.separator();
                ui.label("Flight computer");
                if ui
                    .radio(
                        matches!(e.ship.firmware, Firmware::Standard),
                        "Standard firmware",
                    )
                    .clicked()
                {
                    e.edit(|s| s.firmware = Firmware::Standard);
                }
                ui.small(match e.ship.firmware {
                    Firmware::Standard => "Automatic hardware discovery and flight control",
                    Firmware::Custom(_) => "Custom firmware installed",
                });
                ui.collapsing("Custom firmware (advanced)", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.wasm_file)
                            .hint_text("Path to compiled .wasm"),
                    );
                    if ui.button("Import WASM").clicked() {
                        match std::fs::read(&e.wasm_file)
                            .map_err(anyhow::Error::from)
                            .and_then(|bytes| {
                                e.runtime.validate_program(&bytes)?;
                                Ok(bytes)
                            }) {
                            Ok(bytes) => e.edit(|s| s.firmware = Firmware::Custom(bytes)),
                            Err(err) => e.status = format!("WASM rejected: {err:#}"),
                        }
                    }
                    ui.label(format!(
                        "Program: {} bytes",
                        e.ship.controller_bytes().len()
                    ));
                });
                ui.collapsing("Launch settings", |ui| {
                    ui.label("Optional toy-sim-debug executable override");
                    ui.text_edit_singleline(&mut e.sim_path);
                });
                if !e.devices_mode
                    && let Some(id) = e.selected
                {
                    if let Some(part) = e.ship.parts.iter().find(|p| p.id == id).cloned() {
                        ui.separator();
                        ui.heading(format!("Part {id}"));
                        if let Some(def) = e.catalogue.part(&part.prototype) {
                            ui.label(&def.title);
                            ui.label(format!("{} kg · {} hull", def.mass_kg, def.hull));
                            ui.label(format!("Dimensions: {:?} × 0.1 m", def.dimensions));
                            ui.small(format!("{:?}", def.equipment));
                        }
                        let mut changed = part.clone();
                        ui.label("Part name");
                        ui.add(
                            egui::TextEdit::singleline(&mut changed.name)
                                .hint_text("Default part name")
                                .char_limit(64),
                        );
                        ui.label("Position (0.1 m grid)");
                        ui.horizontal(|ui| {
                            for v in &mut changed.position {
                                ui.add(egui::DragValue::new(v).speed(1.));
                            }
                        });
                        ui.add(
                            egui::Slider::new(&mut changed.orientation, 0..=23).text("Rotation"),
                        );
                        if changed != part {
                            e.edit(|s| *s.parts.iter_mut().find(|p| p.id == id).unwrap() = changed);
                        }
                        if ui.button("Duplicate for placement").clicked() {
                            e.place = Some(part.prototype);
                            e.orientation = part.orientation;
                        }
                        if ui.button("Delete part").clicked() {
                            e.delete_part(id);
                        }
                    }
                }
            });
        });
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
