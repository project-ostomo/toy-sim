use super::*;
use bevy_egui::egui;

/// Device metadata is part of the blueprint, independent of assembly selection.
pub fn panel(ui: &mut egui::Ui, e: &mut Editor) {
    ui.heading("Standard avionics");
    ui.label("Flight computer · inertial sensors · contact sensor");
    ui.label(format!("{AVIONICS_MASS_KG:.0} kg distributed mass · {AVIONICS_POWER_W:.0} W base · {SENSOR_POWER_W:.0} W sensor"));
    ui.label(format!("Sensor range: {:.0} km", SENSOR_RANGE_M / 1000.));
    let mut avionics = e.ship.avionics.clone();
    ui.checkbox(
        &mut avionics.sensor_enabled,
        "Enable contact sensor by default",
    );
    ui.separator();
    ui.heading("Automatic flight control");
    ui.label("All compatible actuators participate unless excluded below.");
    let label = |i| {
        let r = orientation(i);
        let axis = |v: bevy::math::DVec3| {
            if v.x > 0.5 {
                "+X"
            } else if v.x < -0.5 {
                "−X"
            } else if v.y > 0.5 {
                "+Y"
            } else if v.y < -0.5 {
                "−Y"
            } else if v.z > 0.5 {
                "+Z"
            } else {
                "−Z"
            }
        };
        format!(
            "Forward {} · up {}",
            axis(r * bevy::math::DVec3::NEG_Z),
            axis(r * bevy::math::DVec3::Y)
        )
    };
    egui::ComboBox::from_id_salt("control-orientation")
        .selected_text(label(avionics.control_orientation))
        .show_ui(ui, |ui| {
            for i in 0..24 {
                ui.selectable_value(&mut avionics.control_orientation, i, label(i));
            }
        });
    if avionics != e.ship.avionics {
        e.edit(|s| s.avionics = avionics);
    }
    if let Ok(design) = e.ship.compile(&e.catalogue) {
        use bevy::math::{DQuat, DVec3};
        use toy_sim_ships::DeviceKind;
        let forward = orientation(e.ship.avionics.control_orientation) * DVec3::NEG_Z;
        let mut thrust = 0.;
        let mut positive = DVec3::ZERO;
        let mut negative = DVec3::ZERO;
        for d in design.device_catalogue.iter().filter(|d| d.control_enabled) {
            let q = DQuat::from_array(d.rotation);
            match d.kind {
                DeviceKind::Engine { thrust_n, .. } => {
                    let force = q * DVec3::NEG_Z * thrust_n;
                    thrust += force.dot(forward).max(0.);
                    let torque = DVec3::from_array(d.position_m).cross(force);
                    positive += torque.max(DVec3::ZERO);
                    negative += (-torque).max(DVec3::ZERO);
                }
                DeviceKind::Rcs { thrust_n, .. } => {
                    for axis in DVec3::AXES {
                        let force = q * axis * thrust_n;
                        thrust += force.dot(forward).abs();
                        let torque = DVec3::from_array(d.position_m).cross(force).abs();
                        positive += torque;
                        negative += torque;
                    }
                }
                DeviceKind::Torquer { torque_nm } => {
                    for axis in DVec3::AXES {
                        let torque = (q * axis * torque_nm).abs();
                        positive += torque;
                        negative += torque;
                    }
                }
                _ => {}
            }
        }
        ui.label(format!(
            "Available forward thrust: {:.1} kN",
            thrust / 1000.
        ));
        if thrust < 1. {
            ui.colored_label(
                egui::Color32::YELLOW,
                "No forward propulsion; check the control orientation and exclusions.",
            );
        }
        let authority = positive.min(negative);
        for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
            if authority[i] < 1. {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "No bidirectional steering about {axis}; automatic rendezvous unavailable."
                    ),
                );
            }
        }
        ui.small("Actual control authority also depends on power, fuel and actuator saturation.");
    }
    let mut groups = std::collections::BTreeMap::<String, Vec<u64>>::new();
    for part in &e.ship.parts {
        if e.catalogue.part(&part.prototype).is_some_and(|d| {
            matches!(
                d.equipment,
                Equipment::Engine { .. }
                    | Equipment::Rcs { .. }
                    | Equipment::Torquer { .. }
                    | Equipment::Weapon { .. }
            )
        }) {
            for group in &part.groups {
                groups.entry(group.clone()).or_default().push(part.id);
            }
        }
    }
    if !groups.is_empty() {
        ui.collapsing("Actuator groups", |ui| {
            for (name, members) in groups {
                let mut enabled = members
                    .iter()
                    .all(|id| !e.ship.avionics.excluded_actuators.contains(id));
                if ui
                    .checkbox(
                        &mut enabled,
                        format!("{name} ({} actuators)", members.len()),
                    )
                    .changed()
                {
                    e.edit(|ship| {
                        ship.avionics
                            .excluded_actuators
                            .retain(|id| !members.contains(id));
                        if !enabled {
                            ship.avionics.excluded_actuators.extend(members);
                        }
                    });
                }
            }
        });
    }
    ui.separator();
    ui.heading("Physical equipment");
    egui::ScrollArea::vertical().show(ui, |ui| {
        for part in e.ship.parts.clone() {
            let Some(def) = e.catalogue.part(&part.prototype) else {
                continue;
            };
            let Some(kind) = def.equipment.device_kind() else {
                continue;
            };
            let title = if part.name.is_empty() {
                format!("{} #{}", def.title, part.id)
            } else {
                part.name.clone()
            };
            let mut edited = part.clone();
            ui.push_id(part.id, |ui| {
                ui.separator();
                ui.strong(title);
                if matches!(
                    kind,
                    toy_sim_ships::DeviceKind::Engine { .. }
                        | toy_sim_ships::DeviceKind::Rcs { .. }
                        | toy_sim_ships::DeviceKind::Torquer { .. }
                        | toy_sim_ships::DeviceKind::Weapon
                ) {
                    let mut enabled = !e.ship.avionics.excluded_actuators.contains(&part.id);
                    if ui
                        .checkbox(
                            &mut enabled,
                            if matches!(kind, DeviceKind::Weapon) {
                                "Use for automatic weapons control"
                            } else {
                                "Use for automatic flight control"
                            },
                        )
                        .changed()
                    {
                        e.edit(|s| {
                            s.avionics.excluded_actuators.retain(|id| *id != part.id);
                            if !enabled {
                                s.avionics.excluded_actuators.push(part.id);
                            }
                        });
                    }
                }
                ui.collapsing("Names, aliases and groups (advanced)", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(
                            egui::TextEdit::singleline(&mut edited.name)
                                .char_limit(64)
                                .hint_text("Default part name"),
                        );
                        ui.label("Script alias");
                        ui.add(
                            egui::TextEdit::singleline(&mut edited.alias)
                                .char_limit(64)
                                .hint_text("Optional unique alias"),
                        );
                    });
                    ui.label(format!("{kind:?}"));
                    ui.collapsing("Groups", |ui| {
                        let mut remove = None;
                        for (i, group) in edited.groups.iter_mut().enumerate() {
                            ui.push_id(i, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(group).char_limit(64));
                                    if ui.button("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                        }
                        if let Some(i) = remove {
                            edited.groups.remove(i);
                        }
                        if ui
                            .add_enabled(edited.groups.len() < 16, egui::Button::new("Add group"))
                            .clicked()
                        {
                            edited.groups.push(String::new());
                        }
                    });
                });
            });
            if edited != part {
                e.edit(|s| *s.parts.iter_mut().find(|p| p.id == part.id).unwrap() = edited);
            }
        }
    });
}
