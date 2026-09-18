use crate::{Editor, Firmware, orientation, previews::PartPreviews};
use toy_sim_ui::{egui, parts};

use super::InspectorTab;

pub fn show(ui: &mut egui::Ui, editor: &mut Editor, previews: &PartPreviews) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut editor.inspector, InspectorTab::Part, "Part");
        ui.selectable_value(&mut editor.inspector, InspectorTab::Ship, "Ship");
    });
    ui.separator();
    let target = match editor.inspector {
        InspectorTab::Part => editor
            .active_part()
            .map(|(index, installed)| (index, installed.map(|part| part.id))),
        InspectorTab::Ship => None,
    };
    egui::ScrollArea::vertical()
        .id_salt((editor.inspector, target))
        .auto_shrink([false, false])
        .show(ui, |ui| match editor.inspector {
            InspectorTab::Part => part(ui, editor, previews),
            InspectorTab::Ship => ship(ui, editor),
        });
}

fn part(ui: &mut egui::Ui, editor: &mut Editor, previews: &PartPreviews) {
    let Some((index, installed)) = editor.active_part() else {
        ui.add_space(12.);
        ui.heading("Part information");
        ui.label("Pick up a catalogue part or select a part on your ship to inspect it.");
        return;
    };
    let installed = installed.cloned();
    let definition = &editor.catalogue.parts[index];
    let description = &editor.descriptions[index];
    parts::heading(
        ui,
        &definition.title,
        description,
        Some(previews.image(index)),
        120.,
    );
    ui.add_space(6.);
    if let Some(installed) = &installed {
        ui.label(
            egui::RichText::new(format!("Installed · part #{}", installed.id))
                .small()
                .weak(),
        );
    } else {
        ui.label(egui::RichText::new("Ready to place").color(ui.visuals().hyperlink_color));
    }

    if let Some(installed) = installed {
        ui.add_space(8.);
        ui.separator();
        ui.strong("Installation");
        let mut changed = installed.clone();
        ui.label("Part name");
        ui.add(
            egui::TextEdit::singleline(&mut changed.name)
                .hint_text(&definition.title)
                .char_limit(64)
                .desired_width(f32::INFINITY),
        );
        ui.label("Position · 0.1 m grid");
        ui.horizontal(|ui| {
            for (axis, value) in ["X", "Y", "Z"].into_iter().zip(&mut changed.position) {
                ui.add(
                    egui::DragValue::new(value)
                        .prefix(format!("{axis} "))
                        .speed(1.),
                );
            }
        });
        ui.add(egui::Slider::new(&mut changed.orientation, 0..=23).text("Orientation"));
        let forward = orientation(changed.orientation) * bevy::math::DVec3::NEG_Z;
        ui.small(format!("Forward: {}", axis_label(forward)));
        tanks(
            ui,
            &mut changed.tanks,
            definition.tank_volume_m3,
            &editor.catalogue,
        );
        if changed != installed {
            editor.edit(|ship| {
                if let Some(part) = ship.parts.iter_mut().find(|part| part.id == installed.id) {
                    *part = changed;
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Duplicate for placement").clicked() {
                editor.duplicate_for_placement(&installed);
            }
            if ui.button("Delete part").clicked() {
                editor.delete_part(installed.id);
            }
        });
    }

    parts::specifications(ui, &editor.descriptions[index]);
}

fn axis_label(direction: bevy::math::DVec3) -> &'static str {
    if direction.x > 0.5 {
        "+X"
    } else if direction.x < -0.5 {
        "−X"
    } else if direction.y > 0.5 {
        "+Y"
    } else if direction.y < -0.5 {
        "−Y"
    } else if direction.z > 0.5 {
        "+Z"
    } else {
        "−Z"
    }
}

fn ship(ui: &mut egui::Ui, editor: &mut Editor) {
    ui.heading("Ship");
    let mut name = editor.ship.name.clone();
    if ui
        .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY))
        .changed()
    {
        editor.edit(|ship| ship.name = name);
    }
    let tank_mass: f64 = editor
        .ship
        .parts
        .iter()
        .flat_map(|part| &part.tanks)
        .filter_map(|tank| {
            let resource = editor
                .catalogue
                .resources
                .iter()
                .find(|resource| resource.id == tank.resource)?;
            Some(
                tank.volume_m3
                    * resource.storage.usable_fraction
                    * tank.initial_fill
                    * resource.mass_kg
                    / resource.volume_m3,
            )
        })
        .sum::<f64>()
        .max(0.);
    ui.label(format!("Starting tank contents: {tank_mass:.1} kg"));
    if !editor.devices_mode {
        ui.add_space(8.);
        ui.add(egui::Slider::new(&mut editor.preview_thrust, 0.0..=1.0).text("Preview thrust"));
    }
    ui.add_space(8.);
    ui.separator();
    ui.strong("Flight computer");
    if ui
        .radio(
            matches!(editor.ship.firmware, Firmware::Standard),
            "Standard firmware",
        )
        .clicked()
    {
        editor.edit(|ship| ship.firmware = Firmware::Standard);
    }
    ui.small(match editor.ship.firmware {
        Firmware::Standard => "Automatic hardware discovery and flight control",
        Firmware::Custom(_) => "Custom firmware installed",
    });
    ui.collapsing("Custom firmware (advanced)", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut editor.wasm_file)
                .hint_text("Path to compiled .wasm")
                .desired_width(f32::INFINITY),
        );
        if ui.button("Import WASM").clicked() {
            match std::fs::read(&editor.wasm_file)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| {
                    editor.runtime.validate_program(&bytes)?;
                    Ok(bytes)
                }) {
                Ok(bytes) => editor.edit(|ship| ship.firmware = Firmware::Custom(bytes)),
                Err(error) => editor.status = format!("WASM rejected: {error:#}"),
            }
        }
        ui.label(format!(
            "Program: {} bytes",
            editor.ship.controller_bytes().len()
        ));
    });
    ui.collapsing("Launch settings", |ui| {
        ui.label("Optional toy-sim-debug executable override");
        ui.text_edit_singleline(&mut editor.sim_path);
    });
}

fn tanks(
    ui: &mut egui::Ui,
    tanks: &mut Vec<toy_sim_ships::Tank>,
    capacity: f64,
    catalogue: &toy_sim_ships::Catalogue,
) {
    if capacity <= 0. {
        return;
    }
    ui.separator();
    ui.strong("Resource tanks");
    let allocated = tanks.iter().map(|tank| tank.volume_m3).sum::<f64>().max(0.);
    ui.add(
        egui::ProgressBar::new((allocated / capacity) as f32)
            .text(format!("{allocated:.1} / {capacity:.1} m³ allocated")),
    );
    let mut remove = None;
    for (index, tank) in tanks.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("Tank {}", index + 1));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                let title = catalogue
                    .resources
                    .iter()
                    .find(|r| r.id == tank.resource)
                    .map_or(tank.resource.as_str(), |r| r.title.as_str());
                egui::ComboBox::from_id_salt("resource")
                    .selected_text(title)
                    .show_ui(ui, |ui| {
                        for resource in &catalogue.resources {
                            ui.selectable_value(
                                &mut tank.resource,
                                resource.id.clone(),
                                &resource.title,
                            );
                        }
                    });
                let maximum = (capacity - allocated + tank.volume_m3).max(0.001);
                ui.horizontal(|ui| {
                    ui.label("Volume");
                    ui.add(
                        egui::DragValue::new(&mut tank.volume_m3)
                            .range(0.001..=maximum)
                            .speed(0.1)
                            .suffix(" m³"),
                    );
                });
                ui.add(
                    egui::Slider::new(&mut tank.initial_fill, 0.0..=1.0)
                        .text("Starting fill")
                        .show_value(false),
                );
                if let Some(resource) = catalogue.resources.iter().find(|r| r.id == tank.resource) {
                    let density = resource.mass_kg / resource.volume_m3;
                    let usable = tank.volume_m3 * resource.storage.usable_fraction;
                    ui.small(format!(
                        "{:.0}% filled · {:.1} kg loaded",
                        tank.initial_fill * 100.,
                        usable * tank.initial_fill * density
                    ));
                    ui.small(format!(
                        "{density:.1} kg/m³ · {:.1} kg capacity",
                        usable * density
                    ));
                    ui.small(format!(
                        "{:?} containment · {:.1} m³ usable · {:.1} kg hardware",
                        resource.storage.class,
                        usable,
                        tank.volume_m3 * resource.storage.containment_kg_m3
                    ));
                }
            });
        });
    }
    if let Some(index) = remove {
        tanks.remove(index);
    }
    let available = capacity - tanks.iter().map(|tank| tank.volume_m3).sum::<f64>();
    if ui
        .add_enabled(
            available >= 0.001 && tanks.len() < 32,
            egui::Button::new("Add tank"),
        )
        .clicked()
    {
        tanks.push(toy_sim_ships::Tank {
            resource: catalogue.resources[0].id.clone(),
            volume_m3: available.min(10.),
            initial_fill: 1.,
        });
    }
}
