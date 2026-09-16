//! Fixed native instruments. Their controls and presentations are owned by the client.
use crate::precision::{PresentationPose, PresentationVelocity};
use crate::vessel::{ControlledVessel, ShipDesign, ShipHardware, ShipSoftware};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_ship_api::abi;
use toy_sim_ship_view::instruments::{self, AttitudePresentation};
use toy_sim_ship_wasm::{Command, SensorContact};

pub(super) struct Preferences {
    owner: Option<Entity>,
    hud: bool,
    filter: String,
    sort_name: bool,
    throttle: f64,
    distance: f64,
    selected: u64,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            owner: None,
            hud: false,
            filter: String::new(),
            sort_name: false,
            throttle: 1.,
            distance: 100.,
            selected: 0,
        }
    }
}
fn distance(value: f64) -> String {
    if value.abs() >= 1e6 {
        format!("{:.2} Mm", value / 1e6)
    } else if value.abs() >= 1000. {
        format!("{:.2} km", value / 1000.)
    } else {
        format!("{value:.1} m")
    }
}
fn contact_label(contacts: &[SensorContact], id: u64) -> &str {
    contacts
        .iter()
        .find(|c| c.id == id)
        .map_or("No current contact", |c| c.name.as_str())
}
pub(super) fn windows(
    mut contexts: EguiContexts,
    mut ship: Single<
        (
            Entity,
            &ShipDesign,
            &ShipHardware,
            &mut ShipSoftware,
            &PresentationPose,
            &PresentationVelocity,
        ),
        With<ControlledVessel>,
    >,
    time: Res<Time<Fixed>>,
    catalogue: Res<crate::vessel::ShipCatalogue>,
    mut preferences: Local<Preferences>,
    mut orbits: ResMut<super::orbit_hud::OrbitHud>,
    mut recovery: ResMut<crate::vessel::ShipRecovery>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let (entity, design, hardware, software, pose, velocity) = &mut *ship;
    let now = crate::precision::presentation_time(&time);
    if preferences.owner != Some(*entity) {
        *preferences = Preferences {
            owner: Some(*entity),
            ..Default::default()
        };
    }
    let computer = &software.controller;
    let control_frame = bevy::math::DQuat::from_mat3(&toy_sim_ships::orientation(
        design.0.blueprint.avionics.control_orientation,
    ));
    let online = hardware.0.computer_running(&design.0) && !computer.is_booting();
    let mut commands = Vec::new();
    egui::Window::new("Flight computer")
        .default_pos(egui::pos2(650., 10.))
        .show(ctx, |ui| {
            if !hardware.0.computer_running(&design.0) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "AVIONICS OFFLINE — check power and hardware",
                );
            } else if computer.is_booting() {
                ui.heading("FLIGHT COMPUTER BOOTING");
                ui.add(egui::ProgressBar::new(computer.boot_progress() as f32).show_percentage());
                ui.label(format!(
                    "Startup reserve: {:.0} / 50 ticks",
                    computer.boot_progress() * 50.
                ));
            } else if computer.state.attitude.is_none() {
                ui.label("Initializing flight control…");
            } else {
                ui.label("Online");
            }
            if let Some(fault) = &computer.fault {
                ui.colored_label(egui::Color32::RED, fault);
            }
            ui.checkbox(&mut preferences.hud, "Show attitude as HUD");
            if ui.button("Reset encounter").clicked() {
                recovery.requested = true;
                recovery.encounter = true;
            }
            ui.add_enabled_ui(online, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Hold attitude").clicked() {
                        commands.push(Command::HoldAttitude);
                    }
                    if ui.button("Manual / abort").clicked() {
                        commands.push(Command::StopGuidance);
                    }
                });
            });
            if online && !preferences.hud {
                if let (Some(observation), Some(state)) =
                    (&computer.telemetry, &computer.state.attitude)
                {
                    instruments::attitude(
                        ui,
                        observation,
                        state,
                        AttitudePresentation::Panel,
                        control_frame,
                    );
                    if state.control_error > 0.05 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!(
                                "Control demand exceeds available authority ({:.2})",
                                state.control_error
                            ),
                        );
                    }
                }
            }
        });
    if online && preferences.hud {
        if let (Some(observation), Some(state)) = (&computer.telemetry, &computer.state.attitude) {
            egui::Area::new(egui::Id::new("attitude-hud"))
                .anchor(egui::Align2::CENTER_TOP, [0., 20.])
                .interactable(false)
                .show(ctx, |ui| {
                    instruments::attitude(
                        ui,
                        observation,
                        state,
                        AttitudePresentation::Hud,
                        control_frame,
                    );
                });
        }
    }
    egui::Window::new("Contacts")
        .default_pos(egui::pos2(10., 390.))
        .default_width(330.)
        .show(ctx, |ui| {
            if !online || computer.state.contacts.is_none() {
                ui.label("Waiting for sensor instrument");
                return;
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut preferences.filter)
                        .hint_text("Filter contacts"),
                );
                ui.checkbox(&mut preferences.sort_name, "Name order");
            });
            let filter = preferences.filter.to_lowercase();
            let mut contacts: Vec<_> = computer
                .state
                .contact_list
                .iter()
                .filter(|c| c.name.to_lowercase().contains(&filter))
                .collect();
            if preferences.sort_name {
                contacts.sort_by(|a, b| a.name.cmp(&b.name));
            }
            ui.small(format!(
                "{} contacts · scan t = {} s",
                contacts.len(),
                computer.scan_time.map_or("—".into(), |t| format!("{t:.1}"))
            ));
            egui::ScrollArea::vertical().max_height(240.).show_rows(
                ui,
                25.,
                contacts.len(),
                |ui, rows| {
                    for row in rows {
                        let c = contacts[row];
                        ui.horizontal(|ui| {
                            let label = format!(
                                "{}  {}",
                                c.name,
                                computer
                                    .state
                                    .spatial
                                    .tracks
                                    .get(&c.id)
                                    .and_then(|track| track.at(now))
                                    .map_or_else(
                                        || "unavailable".into(),
                                        |estimate| distance(
                                            crate::navigation::position(estimate.position)
                                                .relative_to(pose.0.translation_um)
                                                .length()
                                        )
                                    )
                            );
                            if ui
                                .selectable_label(preferences.selected == c.id, label)
                                .clicked()
                            {
                                preferences.selected = c.id;
                                if c.kind == abi::CONTACT_SHIP {
                                    commands.push(Command::SelectTarget(c.id));
                                }
                            }
                            if ui.small_button("Aim").clicked() {
                                commands.push(Command::AimContact(c.id));
                            }
                        });
                    }
                },
            );
        });
    egui::Window::new("Weapons")
        .default_pos(egui::pos2(700.0, 390.0))
        .default_width(340.0)
        .show(ctx, |ui| {
            ui.add_enabled_ui(online, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            preferences.selected != 0,
                            egui::Button::new("Engage selected"),
                        )
                        .clicked()
                    {
                        commands.push(Command::EngageWeapons {
                            contact: preferences.selected,
                            maximum_flight_time_s: 2.0,
                        });
                    }
                    if ui.button("Hold fire").clicked() {
                        commands.push(Command::HoldFire);
                    }
                });
            });
            let Some(state) = &computer.state.weapons else {
                ui.label("Waiting for weapons instrument");
                return;
            };
            ui.label(if state.mode == abi::WEAPONS_HOLD {
                "HOLD FIRE"
            } else {
                "ENGAGING"
            });
            if state.target_contact != 0 {
                ui.label(contact_label(
                    &computer.state.contact_list,
                    state.target_contact,
                ));
            }
            ui.label(state.reason.as_str().unwrap_or(""));
            for row in &computer.state.weapon_rows {
                ui.separator();
                let device = design
                    .0
                    .device_catalogue
                    .get(row.device.saturating_sub(1) as usize);
                ui.strong(device.map_or("Weapon", |device| device.alias.as_str()));
                ui.label(format!(
                    "{} rounds · {:.2} / {:.2} MJ",
                    row.reading.ammunition_units,
                    row.reading.battery_energy_j / 1e6,
                    row.reading.shot_energy_j / 1e6
                ));
                ui.label(format!(
                    "Aim {:.3}° · flight {:.2} s",
                    row.pointing_error_rad.to_degrees(),
                    row.time_of_flight_s
                ));
                let flags = row.reading.inhibit_flags;
                for (bit, label) in [
                    (abi::WEAPON_AMMO, "No ammunition"),
                    (abi::WEAPON_PROPELLANT, "No propellant"),
                    (abi::WEAPON_BLOCKED, "Muzzle blocked"),
                    (abi::WEAPON_POINTING, "Tracking"),
                    (abi::WEAPON_TRAVEL, "Outside mount travel"),
                    (abi::WEAPON_ENERGY, "Insufficient battery energy"),
                    (abi::WEAPON_UNAVAILABLE, "Hardware unavailable"),
                ] {
                    if flags & bit != 0 {
                        ui.small(label);
                    }
                }
            }
        });
    egui::Window::new("Navigation")
        .default_pos(egui::pos2(350., 390.))
        .default_width(340.)
        .show(ctx, |ui| {
            super::orbit_hud::controls(ui, &mut orbits);
            ui.separator();
            if !online {
                ui.label("Flight computer unavailable");
                return;
            }
            let Some(nav) = &computer.state.navigation else {
                ui.label("Waiting for navigation instrument");
                return;
            };
            const STATUS: [&str; 4] = [
                "Guidance idle",
                "Guidance active",
                "Guidance suspended",
                "Guidance unavailable",
            ];
            ui.heading(STATUS[nav.status.min(3) as usize]);
            ui.label(contact_label(
                &computer.state.contact_list,
                nav.target_contact,
            ));
            if let Some(reason) = nav.reason.as_str().filter(|s| !s.is_empty()) {
                ui.colored_label(egui::Color32::YELLOW, reason);
            }
            if let Some(estimate) = computer
                .state
                .spatial
                .tracks
                .get(&nav.target_contact)
                .and_then(|track| track.at(now))
            {
                let relative = crate::navigation::position(estimate.position)
                    .relative_to(pose.0.translation_um);
                let relative_velocity =
                    bevy::math::DVec3::from_array(estimate.velocity) - velocity.0;
                let closing = -relative_velocity.dot(relative.try_normalize().unwrap_or_default());
                ui.label(format!(
                    "Range {} · closing {:.1} m/s",
                    distance(relative.length()),
                    closing
                ));
                ui.label(format!(
                    "Relative speed {:.1} m/s",
                    relative_velocity.length()
                ));
            } else {
                ui.label("Target estimate unavailable");
            }
            ui.label(format!("Throttle {:.0}%", nav.throttle * 100.));
            // Zero means this guidance publishes no stand-off/braking constraint.
            // Render supplied instrument data without knowing firmware phases.
            if nav.present & abi::NAV_BRAKING_DISTANCE != 0 {
                ui.label(format!("Braking {}", distance(nav.braking_distance_m)));
            }
            if nav.present & abi::NAV_STAND_OFF != 0 {
                ui.label(format!("Stand-off {}", distance(nav.stand_off_m)));
                ui.horizontal(|ui| {
                    ui.label("Stand-off");
                    ui.add(
                        egui::DragValue::new(&mut preferences.distance)
                            .range(25. ..=1e6)
                            .suffix(" m"),
                    );
                });
            }
            ui.add(egui::Slider::new(&mut preferences.throttle, 0.01..=1.).text("Throttle limit"));
            ui.horizontal(|ui| {
                if ui.button("Engage").clicked() {
                    commands.push(Command::EngageNavigation {
                        throttle_limit: preferences.throttle,
                        stand_off_m: preferences.distance,
                    });
                }
                if ui.button("Abort").clicked() {
                    commands.push(Command::StopGuidance);
                }
            });
            if nav.present & abi::NAV_ARRIVAL != 0 {
                ui.label(format!(
                    "ETA {:.1} s · estimated propellant {:.2} kg",
                    (nav.arrival_time_s - now).max(0.),
                    nav.predicted_fuel_kg
                ));
            }
        });
    egui::Window::new("Ship systems")
        .default_pos(egui::pos2(1000., 10.))
        .show(ctx, |ui| {
            let (mass, _) = hardware.0.mass_properties(&design.0, &catalogue.0);
            ui.label(format!(
                "Mass {:.0} kg · hull {:.0} · shield state {}",
                mass,
                hardware.0.hull,
                match hardware.0.thermal.shield_state {
                    abi::SHIELD_ACTIVE => "active",
                    abi::SHIELD_DEPLETED => "depleted",
                    abi::SHIELD_UNPOWERED => "unpowered",
                    abi::SHIELD_BLOCKED => "blocked",
                    abi::SHIELD_OFF => "off",
                    _ => "absent",
                }
            ));
            ui.label(format!(
                "Energy {:.2} MJ",
                hardware.0.inventory.energy_j / 1e6
            ));
            ui.label("Standard avionics: 71 kg · 101 W + sensor 1 kW");

            for (resource, quantity) in catalogue
                .0
                .resources
                .iter()
                .zip(&hardware.0.inventory.quantities)
            {
                ui.label(format!("{}: {quantity:.2}", resource.title));
            }
        });
    for command in commands {
        software.command(command);
    }
    Ok(())
}
