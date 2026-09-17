use super::Selection;
use crate::state::{Contact, DisplayPose, Outgoing, OwnedShip, RenderTime, ShipDetails};
use crate::ui::SelectedTarget;
use bevy::{math::DQuat, prelude::*};
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::*;
use toy_sim_ship_view::instruments::{self, AttitudePresentation};

#[derive(Resource, Default)]
pub(super) struct FlightControls {
    hud: bool,
    pub throttle: f64,
    pub steering: [f64; 3],
}

#[derive(Resource, Default)]
pub(super) struct ContactControls {
    filter: String,
    sort_name: bool,
}

#[derive(Resource)]
pub(super) struct NavigationControls {
    throttle_limit: f64,
    stand_off: f64,
}

impl Default for NavigationControls {
    fn default() -> Self {
        Self {
            throttle_limit: 1.,
            stand_off: 100.,
        }
    }
}

pub(super) fn release_manual(
    flight: &mut FlightControls,
    outgoing: &mut Outgoing,
    ship: &ShipTelemetry,
) {
    let throttle = flight.throttle;
    outgoing.ship(
        ship,
        ShipCommand::Manual {
            throttle,
            steering: [0.; 3],
        },
    );
    *flight = FlightControls::default();
}

pub(super) fn manual(
    mut flight: ResMut<FlightControls>,
    selection: Res<Selection>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<&OwnedShip>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut contexts: EguiContexts,
) {
    let captured = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.egui_wants_keyboard_input());
    let Some(ship) = ships
        .iter()
        .map(|ship| &ship.0)
        .find(|ship| Some(ship.ship) == selection.ship)
    else {
        return;
    };
    let previous = (flight.throttle, flight.steering);
    flight.steering = [0.; 3];
    if !captured {
        let axis = |positive, negative| {
            f64::from(keys.pressed(positive)) - f64::from(keys.pressed(negative))
        };
        flight.throttle = (flight.throttle
            + axis(KeyCode::ShiftLeft, KeyCode::ControlLeft) * time.delta_secs_f64() / 2.)
            .clamp(0., 1.);
        flight.steering = [
            axis(KeyCode::KeyS, KeyCode::KeyW),
            axis(KeyCode::KeyA, KeyCode::KeyD),
            axis(KeyCode::KeyQ, KeyCode::KeyE),
        ];
    }
    let next = (flight.throttle, flight.steering);
    if previous != next {
        outgoing.ship(
            ship,
            ShipCommand::Manual {
                throttle: next.0,
                steering: next.1,
            },
        );
    }
}

pub(super) fn is_ship_contact(track: &Track) -> bool {
    track
        .tags
        .iter()
        .any(|tag| matches!(tag, Tag::Kind(kind) if kind == "ship"))
}

pub(super) fn contact_rows<'a>(
    contacts: impl IntoIterator<Item = (&'a Contact, &'a DisplayPose)>,
    filter: &str,
    sort_name: bool,
) -> Vec<(GroupId, &'a Track, &'a Pose, String)> {
    let filter = filter.to_lowercase();
    let mut rows: Vec<_> = contacts
        .into_iter()
        .filter(|(contact, _)| is_ship_contact(&contact.0))
        .map(|(contact, pose)| {
            let track = &contact.0;
            let label = track
                .tags
                .iter()
                .find_map(|tag| match tag {
                    Tag::Advertised(name) => Some(name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| track.entity.unwrap_or(track.id).to_string());
            (contact.1.group, track, &pose.0, label)
        })
        .filter(|(_, _, _, label)| label.to_lowercase().contains(&filter))
        .collect();
    rows.sort_by(|a, b| {
        let identity = (a.0, a.1.id).cmp(&(b.0, b.1.id));
        if sort_name {
            a.3.cmp(&b.3).then(identity)
        } else {
            identity
        }
    });
    rows
}

struct ShipPanel<'a> {
    ship: &'a ShipTelemetry,
    presentation: &'a ShipPresentation,
    display_pose: Option<&'a Pose>,
    online: bool,
}

pub(super) fn windows(
    mut contexts: EguiContexts,
    mut flight: ResMut<FlightControls>,
    mut contact_controls: ResMut<ContactControls>,
    mut navigation: ResMut<NavigationControls>,
    mut selection: ResMut<Selection>,
    mut outgoing: ResMut<Outgoing>,
    clock: Res<RenderTime>,
    ships: Query<(&OwnedShip, &ShipDetails, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
) -> Result {
    let Some((ship, details, pose)) = ships
        .iter()
        .find(|(ship, _, _)| Some(ship.0.ship) == selection.ship)
    else {
        return Ok(());
    };
    let data = ShipPanel {
        ship: &ship.0,
        presentation: &details.0,
        display_pose: pose.map(|pose| &pose.0),
        online: matches!(details.0.computer, ComputerStatus::Running { .. }),
    };
    let ctx = contexts.ctx_mut()?;
    show_flight(ctx, &data, &mut flight, &mut outgoing);
    show_contacts(
        ctx,
        &data,
        &mut contact_controls,
        &mut selection,
        &mut outgoing,
        &contacts,
    );
    show_navigation(ctx, &data, &mut navigation, &mut outgoing, &clock);
    show_weapons(ctx, &data, &selection, &mut outgoing);
    show_hardware(ctx, &data);
    Ok(())
}

fn show_flight(
    ctx: &egui::Context,
    data: &ShipPanel,
    flight: &mut FlightControls,
    outgoing: &mut Outgoing,
) {
    let ship = data.ship;
    let presentation = data.presentation;
    let display_pose = data.display_pose;
    let online = data.online;
    egui::Window::new("Flight computer")
        .default_pos([650., 10.])
        .show(ctx, |ui| {
            match &presentation.computer {
                ComputerStatus::Booting { progress } => {
                    ui.label("Flight computer booting");
                    ui.add(egui::ProgressBar::new(*progress as f32).show_percentage());
                }
                ComputerStatus::Running {
                    gas_used,
                    gas_limit,
                } => {
                    ui.label(format!("Online · gas {gas_used} / {gas_limit}"));
                }
                ComputerStatus::Fault(reason) => {
                    ui.colored_label(egui::Color32::RED, reason);
                }
                status => {
                    ui.label(format!("{status:?}"));
                }
            }
            ui.checkbox(&mut flight.hud, "Show attitude as HUD");
            ui.add_enabled_ui(online, |ui| {
                ui.horizontal(|ui| {
                    for (label, flight) in [
                        ("Hold attitude", FlightCommand::HoldAttitude),
                        ("Manual / abort", FlightCommand::StopGuidance),
                    ] {
                        if ui.button(label).clicked() {
                            outgoing.ship(ship, ShipCommand::Flight(flight));
                        }
                    }
                });
            });
            if let Some(environment) = &presentation.environment {
                ui.label(format!(
                    "Altitude {:.1} m · airspeed {:.1} m/s",
                    environment.altitude_m,
                    glam::DVec3::from_array(environment.airspeed_m_s).length()
                ));
                ui.label(format!(
                    "Density {:.3e} kg/m³ · pressure {:.3} atm",
                    environment.density_kg_m3,
                    environment.pressure_pa / 101_300.
                ));
            }
            ui.label("Shift / Ctrl: throttle · W/S A/D Q/E: steering");
            ui.label(format!("Manual throttle {:.0}%", flight.throttle * 100.));
            if !flight.hud {
                attitude(ui, display_pose, presentation, AttitudePresentation::Panel);
            }
        });
    if flight.hud {
        egui::Area::new(egui::Id::new("attitude-hud"))
            .anchor(egui::Align2::CENTER_TOP, [0., 20.])
            .interactable(false)
            .show(ctx, |ui| {
                attitude(ui, display_pose, presentation, AttitudePresentation::Hud)
            });
    }
}

fn show_contacts(
    ctx: &egui::Context,
    data: &ShipPanel,
    contact_controls: &mut ContactControls,
    selection: &mut Selection,
    outgoing: &mut Outgoing,
    contacts: &Query<(&Contact, &DisplayPose)>,
) {
    let ship = data.ship;
    let display_pose = data.display_pose;
    egui::Window::new("Contacts")
        .default_pos([10., 390.])
        .show(ctx, |ui| {
            ui.text_edit_singleline(&mut contact_controls.filter);
            ui.checkbox(&mut contact_controls.sort_name, "Name order");
            let contacts = contact_rows(
                contacts.iter(),
                &contact_controls.filter,
                contact_controls.sort_name,
            );
            egui::ScrollArea::vertical()
                .max_height(240.)
                .show(ui, |ui| {
                    for (group, track, contact_pose, label) in contacts {
                        ui.push_id(("contact", group, track.id), |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(
                                        selection.contact()
                                            == Some(ContactRef {
                                                group,
                                                track: track.id,
                                            }),
                                        label,
                                    )
                                    .clicked()
                                {
                                    selection.target = Some(SelectedTarget::Contact(ContactRef {
                                        group,
                                        track: track.id,
                                    }));
                                    outgoing.ship(
                                        ship,
                                        ShipCommand::Flight(FlightCommand::SelectTarget(
                                            ContactRef {
                                                group,
                                                track: track.id,
                                            },
                                        )),
                                    );
                                }
                                if let Some(pose) = display_pose {
                                    ui.label(format!(
                                        "{:.1} km",
                                        contact_pose.position.relative_to(pose.position).length()
                                            / 1000.
                                    ));
                                }
                                ui.label(format!("± {:.0} m", track.position_sigma_m));
                                if ui.small_button("Aim").clicked() {
                                    outgoing.ship(
                                        ship,
                                        ShipCommand::Aim {
                                            group,
                                            track: track.id,
                                        },
                                    );
                                }
                            });
                        });
                    }
                });
        });
}

fn show_navigation(
    ctx: &egui::Context,
    data: &ShipPanel,
    navigation: &mut NavigationControls,
    outgoing: &mut Outgoing,
    clock: &RenderTime,
) {
    let ship = data.ship;
    let presentation = data.presentation;
    let online = data.online;
    egui::Window::new("Navigation")
        .default_pos([350., 390.])
        .show(ctx, |ui| {
            ui.add(
                egui::Slider::new(&mut navigation.throttle_limit, 0. ..=1.).text("Throttle limit"),
            );
            ui.add(
                egui::DragValue::new(&mut navigation.stand_off)
                    .range(0. ..=1e12)
                    .suffix(" m stand-off"),
            );
            if ui
                .add_enabled(online, egui::Button::new("Engage navigation"))
                .clicked()
            {
                let flight = FlightCommand::EngageNavigation {
                    throttle_limit: navigation.throttle_limit,
                    stand_off_m: navigation.stand_off,
                };
                outgoing.ship(ship, ShipCommand::Flight(flight));
            }
            if let Some(nav) = presentation
                .instruments
                .as_ref()
                .and_then(|data| data.navigation.as_ref())
            {
                ui.label(&nav.reason);
                ui.label(format!(
                    "Throttle {:.0}% · braking {:.1} m",
                    nav.throttle * 100.,
                    nav.braking_distance_m
                ));
                if let Some(arrival) = nav.arrival_time_ns {
                    ui.label(format!(
                        "Arrival in {:.1} s",
                        arrival.saturating_sub(clock.display_ns) as f64 / 1e9
                    ));
                }
            }
        });
}

fn show_weapons(
    ctx: &egui::Context,
    data: &ShipPanel,
    selection: &Selection,
    outgoing: &mut Outgoing,
) {
    let ship = data.ship;
    let presentation = data.presentation;
    let online = data.online;
    egui::Window::new("Weapons")
        .default_pos([700., 390.])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        online && selection.contact().is_some(),
                        egui::Button::new("Engage selected"),
                    )
                    .clicked()
                {
                    if let Some(ContactRef { group, track }) = selection.contact() {
                        outgoing.ship(
                            ship,
                            ShipCommand::EngageWeapons {
                                group,
                                track,
                                maximum_flight_time_s: 2.,
                            },
                        );
                    }
                }
                if ui.button("Hold fire").clicked() {
                    outgoing.ship(ship, ShipCommand::HoldFire);
                }
            });
            if let Some(instruments) = &presentation.instruments {
                if let Some(weapons) = &instruments.weapons_state {
                    ui.label(if weapons.mode == toy_sim_ship_api::abi::WEAPONS_HOLD {
                        "HOLD FIRE"
                    } else {
                        "ENGAGING"
                    });
                    ui.label(&weapons.reason);
                }
                for weapon in &instruments.weapons {
                    let name = presentation
                        .devices
                        .iter()
                        .find(|device| device.part == weapon.part)
                        .map_or("Weapon", |device| device.name.as_str());
                    ui.strong(name);
                    ui.label(format!(
                        "{:.0} rounds · {:.2} / {:.2} MJ",
                        weapon.ammunition_units,
                        weapon.battery_energy_j / 1e6,
                        weapon.shot_energy_j / 1e6
                    ));
                    ui.label(format!(
                        "Aim {:.3}° · flight {:.2} s",
                        weapon.pointing_error_rad.to_degrees(),
                        weapon.flight_time_s
                    ));
                    for (flag, label) in [
                        (toy_sim_ship_api::abi::WEAPON_AMMO, "No ammunition"),
                        (toy_sim_ship_api::abi::WEAPON_PROPELLANT, "No propellant"),
                        (toy_sim_ship_api::abi::WEAPON_BLOCKED, "Muzzle blocked"),
                        (toy_sim_ship_api::abi::WEAPON_POINTING, "Tracking"),
                        (toy_sim_ship_api::abi::WEAPON_TRAVEL, "Outside mount travel"),
                        (
                            toy_sim_ship_api::abi::WEAPON_ENERGY,
                            "Insufficient battery energy",
                        ),
                        (
                            toy_sim_ship_api::abi::WEAPON_UNAVAILABLE,
                            "Hardware unavailable",
                        ),
                    ] {
                        if weapon.inhibit_flags & flag != 0 {
                            ui.small(label);
                        }
                    }
                }
            }
            for device in &presentation.devices {
                if let DeviceReading::Weapon {
                    yaw_rad,
                    pitch_rad,
                    loaded,
                    firing,
                    progress,
                } = device.reading
                {
                    ui.label(format!(
                        "{} · yaw {:.1}° pitch {:.1}°",
                        device.name,
                        yaw_rad.to_degrees(),
                        pitch_rad.to_degrees()
                    ));
                    ui.label(format!("Loaded {loaded} · firing {firing}"));
                    ui.add(egui::ProgressBar::new(progress as f32));
                }
            }
        });
}

fn show_hardware(ctx: &egui::Context, data: &ShipPanel) {
    let ship = data.ship;
    let presentation = data.presentation;
    egui::Window::new("Ship hardware")
        .default_open(false)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.label(format!("Mass {:.1} t", presentation.mass_kg / 1000.));
            if let Some(health) = &presentation.health {
                ui.label(format!(
                    "Hull {:.0} / {:.0} · shield strength {:.0}%",
                    health.hull_hp,
                    health.hull_max_hp,
                    health.shield_strength * 100.
                ));
                ui.label(format!(
                    "Shield reserve {:.1} / {:.1} kg",
                    ship.coolant_reserve_kg, health.shield_reserve_capacity_kg
                ));
            }
            if let Some(execution) = &presentation.execution {
                ui.label(format!(
                    "Ship step {:.1} µs · WASM memory {} KiB",
                    execution.step_us,
                    execution.memory_bytes / 1024
                ));
                ui.label(format!(
                    "Prepare {:.1} / callback {:.1} / publish {:.1} / hardware {:.1} µs",
                    execution.prepare_us,
                    execution.callback_us,
                    execution.publish_us,
                    execution.hardware_us
                ));
                ui.label(format!("Native sensor query {:.1} µs", execution.scan_us));
            }
            ui.label(format!(
                "Power {:.2} MW generated · {:.2} MW consumed",
                presentation.power_generated_w / 1e6,
                presentation.power_consumed_w / 1e6
            ));
            ui.add(
                egui::ProgressBar::new(
                    (ship.battery_j / presentation.battery_capacity_j.max(1.)) as f32,
                )
                .text("Battery"),
            );
            ui.add(
                egui::ProgressBar::new(
                    (ship.hull_heat_j / presentation.hull_heat_capacity_j.max(1.)) as f32,
                )
                .text("Hull heat capacity"),
            );
            ui.label(format!(
                "Shield {:.0} K · reserve {:.1} kg",
                ship.shield_temperature_k, ship.coolant_reserve_kg
            ));
            for amount in &presentation.inventory {
                ui.label(format!(
                    "{}: {:.1} / {:.1} kg",
                    amount.name, amount.amount_kg, amount.capacity_kg
                ));
            }
            for device in &presentation.devices {
                ui.collapsing(&device.name, |ui| {
                    ui.label(format!(
                        "Power {:.1} / {:.1} W",
                        device.power_delivered_w, device.power_requested_w
                    ));
                    ui.label(format!("{:?}", device.reading));
                });
            }
        });
}

fn attitude(
    ui: &mut egui::Ui,
    display_pose: Option<&Pose>,
    data: &ShipPresentation,
    style: AttitudePresentation,
) {
    let Some(state) = data
        .instruments
        .as_ref()
        .and_then(|data| data.attitude.as_ref())
    else {
        return;
    };
    let Some(pose) = display_pose else {
        return;
    };
    let flight = toy_sim_ship_api::abi::FlightState {
        rotation: pose.rotation,
        ..Default::default()
    };
    let state = toy_sim_ship_api::abi::AttitudeState {
        mode: state.mode,
        present: if state.reference.is_some() {
            toy_sim_ship_api::abi::ATTITUDE_REFERENCE
        } else {
            0
        },
        reference: state.reference.unwrap_or([0., 0., 0., 1.]),
        control_error: state.control_error_rad,
        ..Default::default()
    };
    instruments::attitude(
        ui,
        &flight,
        &state,
        style,
        DQuat::from_array(data.control_rotation),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn contact_rows_keep_identity_order_across_archetype_moves_and_duplicate_names() {
        #[derive(Component)]
        struct FreshVisual;

        let mut world = World::new();
        let mut moved = None;
        for (ordinal, kind) in [(3, "ship"), (1, "ship"), (2, "ship"), (0, "celestial")] {
            let id = Id([ordinal; 16]);
            let entity = world
                .spawn((
                    Contact(
                        Track {
                            id,
                            entity: Some(id),
                            spatial_instance: Id([5; 16]),
                            pose: Pose::default(),
                            position_sigma_m: 0.,
                            velocity_sigma_m_s: 0.,
                            observed_tick: 0,
                            estimate_tick: 0,
                            tags: std::collections::BTreeSet::from([
                                Tag::Kind(kind.into()),
                                Tag::Advertised("Repeated name".into()),
                            ]),
                            provenance: Provenance::GroupMember,
                            radius_m: Some(1.),
                            appearance: None,
                        },
                        ContactRef {
                            group: Id([4; 16]),
                            track: id,
                        },
                    ),
                    DisplayPose(Pose::default()),
                ))
                .id();
            if ordinal == 3 {
                moved = Some(entity);
            }
        }
        let mut query = world.query::<(&Contact, &DisplayPose)>();
        let before: Vec<_> = query
            .iter(&world)
            .map(|(contact, _)| contact.1.track)
            .collect();
        for sort_name in [false, true] {
            let rows = contact_rows(query.iter(&world), "REPEATED", sort_name);
            assert_eq!(
                rows.iter().map(|row| row.1.id).collect::<Vec<_>>(),
                vec![Id([1; 16]), Id([2; 16]), Id([3; 16])]
            );
        }
        world.entity_mut(moved.unwrap()).insert(FreshVisual);
        let after: Vec<_> = query
            .iter(&world)
            .map(|(contact, _)| contact.1.track)
            .collect();
        assert_ne!(before, after);
        for sort_name in [false, true] {
            let rows = contact_rows(query.iter(&world), "REPEATED", sort_name);
            assert_eq!(
                rows.iter().map(|row| row.1.id).collect::<Vec<_>>(),
                vec![Id([1; 16]), Id([2; 16]), Id([3; 16])]
            );
        }
    }

    #[test]
    fn own_contact_range_uses_the_display_sample_for_both_positions() {
        let mut world = World::new();
        let id = Id([1; 16]);
        let mut owned = super::super::tests::ship(id);
        owned.0.pose.as_mut().unwrap().position.x = 2_000_000_000;
        owned.0.pose.as_mut().unwrap().rotation = DQuat::from_rotation_y(1.).to_array();
        world.spawn((owned, DisplayPose(Pose::default())));
        world.spawn(ShipDetails(ShipPresentation {
            ship: id,
            revision: 1,
            sim_time_ns: 0,
            environment: None,
            health: None,
            execution: None,
            mass_kg: 1.,
            inertia_kg_m2: [0.; 9],
            control_rotation: [0., 0., 0., 1.],
            hull_heat_capacity_j: 1.,
            battery_capacity_j: 1.,
            power_generated_w: 0.,
            power_consumed_w: 0.,
            inventory: Vec::new(),
            devices: Vec::new(),
            computer: ComputerStatus::Unpowered,
            instruments: None,
            screens: Vec::new(),
        }));
        let contact = ContactRef {
            group: Id([2; 16]),
            track: Id([3; 16]),
        };
        world.spawn((
            Contact(
                Track {
                    id: contact.track,
                    entity: Some(id),
                    spatial_instance: Id([5; 16]),
                    pose: Pose::default(),
                    position_sigma_m: 0.,
                    velocity_sigma_m_s: 0.,
                    observed_tick: 0,
                    estimate_tick: 0,
                    tags: std::collections::BTreeSet::from([Tag::Kind("ship".into())]),
                    provenance: Provenance::GroupMember,
                    radius_m: Some(1.),
                    appearance: None,
                },
                contact,
            ),
            DisplayPose(Pose::default()),
        ));
        let mut queries = SystemState::<(
            Query<&OwnedShip>,
            Query<&ShipDetails>,
            Query<(&Contact, &DisplayPose)>,
            Query<(&OwnedShip, &DisplayPose)>,
        )>::new(&mut world);
        let (ships, details, contacts, poses) = queries.get(&world).unwrap();
        let mut contact_controls = ContactControls::default();
        let mut selection = Selection {
            ship: Some(id),
            ..Default::default()
        };
        let mut outgoing = Outgoing::default();
        let ctx = egui::Context::default();
        let mut zero_range = false;
        for _ in 0..3 {
            let mut output = ctx.run_ui(Default::default(), |root| {
                let ctx = root.ctx();
                let data = ShipPanel {
                    ship: &ships.iter().next().unwrap().0,
                    presentation: &details.iter().next().unwrap().0,
                    display_pose: Some(&poses.iter().next().unwrap().1.0),
                    online: false,
                };
                show_contacts(
                    ctx,
                    &data,
                    &mut contact_controls,
                    &mut selection,
                    &mut outgoing,
                    &contacts,
                )
            });
            output.textures_delta.clear();
            zero_range |= output.shapes.iter().any(|shape| {
                matches!(
                    &shape.shape, egui::Shape::Text(text) if text.galley.text() == "0.0 km"
                )
            });
        }
        assert!(
            zero_range,
            "own contact must display zero range despite a newer authoritative ship sample"
        );
    }
}
