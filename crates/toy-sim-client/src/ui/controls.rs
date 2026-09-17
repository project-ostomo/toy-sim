use super::Selection;
use crate::state::*;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::*;

#[derive(Resource, Default)]
pub(super) struct ControlPanel {
    status: String,
    group_key: String,
}

#[derive(Resource, Default)]
pub(super) struct TravelControls {
    coordinates: String,
    append_waypoint: bool,
    relative_destination: bool,
    body_fixed: bool,
}

pub(super) fn window(
    mut contexts: EguiContexts,
    mut client: ResMut<ControlPanel>,
    mut travel: ResMut<TravelControls>,
    mut mfds: ResMut<super::mfd::MfdWindows>,
    mut selection: ResMut<Selection>,
    mut outgoing: ResMut<Outgoing>,
    clock: Res<RenderTime>,
    session: Res<SessionInfo>,
    time: Res<Time>,
    ships: Query<&OwnedShip>,
    details: Query<&ShipDetails>,
    contacts: Query<(&Contact, &DisplayPose)>,
    views: Query<&ViewObservation>,
    celestials: Query<&Celestial>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let viewport = ctx.content_rect();
    let sidebar = (viewport.width() * 0.33).clamp(220., 360.);
    if session.world.is_none() {
        egui::Window::new("Connection")
            .default_width(sidebar)
            .show(ctx, |ui| {
                ui.label("Waiting for observations…");
                ui.label(&session.status);
            });
        return Ok(());
    }
    egui::Window::new("Ship controls")
        .default_pos(egui::pos2(8., 8.))
        .default_width(sidebar)
        .max_height((viewport.height() - 32.).max(100.))
        .vscroll(true)
        .show(ctx, |ui| {
            ui.heading("Ships");
            ui.label(format!("Simulation {:.1} s", clock.display_ns as f64 / 1e9));
            ui.label(format!(
                "Buffer: {} frames · underruns {}",
                session.target_frames, session.underruns
            ));
            ui.label(format!(
                "Display {:.0} FPS · wall {:.1} s",
                1. / time.delta_secs_f64().max(1e-6),
                time.elapsed_secs_f64()
            ));
            ui.label(&session.status);
            ui.label(&client.status);
            ui.horizontal(|ui| {
                ui.label("Info group key");
                ui.add(
                    egui::TextEdit::singleline(&mut client.group_key)
                        .password(true)
                        .desired_width(120.),
                );
                if ui.button("Join").clicked() {
                    let decoded: Option<Vec<u8>> = (client.group_key.len() == 64
                        && client.group_key.is_ascii())
                    .then(|| {
                        client
                            .group_key
                            .as_bytes()
                            .chunks_exact(2)
                            .map(|pair| {
                                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).ok()
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                    .flatten();
                    if let Some(bytes) = decoded {
                        outgoing.push(Action::JoinGroup(InfoGroupKey(bytes.try_into().unwrap())));
                        client.group_key.clear();
                    } else {
                        client.status = "Enter a 64-digit hexadecimal group key".into();
                    }
                }
            });
            if ui.button("Open beacon map").clicked() {
                let revision = views
                    .iter()
                    .map(|view| &view.0)
                    .find(|view| view.id == 8)
                    .map_or(1, |view| view.revision + 1);
                outgoing.push(Action::Subscribe(ViewSubscription {
                    id: 8,
                    revision,
                    group: PUBLIC_GROUP,
                    focused_ship: ships
                        .iter()
                        .map(|ship| &ship.0)
                        .min_by_key(|ship| ship.ship)
                        .map(|ship| ship.ship),
                    query: TrackQuery {
                        all: std::collections::BTreeSet::from([Tag::Kind("beacon".into())]),
                        limit: 256,
                        work: 300_000,
                        ..Default::default()
                    },
                }));
            }
            ui.label("Destination x y z (galactic micrometres)");
            ui.text_edit_singleline(&mut travel.coordinates);
            ui.checkbox(&mut travel.append_waypoint, "Append waypoint");
            ui.checkbox(
                &mut travel.relative_destination,
                "Offset from selected celestial / beacon",
            );
            if travel.relative_destination {
                ui.checkbox(&mut travel.body_fixed, "Rotate offset with reference body");
            }
            for ship in ships.iter().map(|ship| &ship.0) {
                ui.separator();
                if ui
                    .selectable_label(selection.ship == Some(ship.ship), ship.ship.to_string())
                    .clicked()
                {
                    selection.ship = Some(ship.ship);
                }
                ui.label(format!("{:?}", ship.presence));
                let mut broadcasting = ship.iff.enabled;
                if ui.checkbox(&mut broadcasting, "Broadcast IFF").changed() {
                    outgoing.ship(ship, ShipCommand::SetTransponderEnabled(broadcasting));
                }
                ui.label(format!("Installed IFF owner: {}", ship.iff.owner));

                ui.label(format!(
                    "Battery {:.1} MJ · shield {:.0} K",
                    ship.battery_j / 1e6,
                    ship.shield_temperature_k
                ));
                if ui.button("Open view").clicked() && views.iter().count() < 8 {
                    if let Some(group) = session
                        .groups
                        .iter()
                        .copied()
                        .find(|group| *group != PUBLIC_GROUP)
                    {
                        outgoing.push(Action::Subscribe(ViewSubscription {
                            id: views
                                .iter()
                                .map(|view| &view.0)
                                .map(|view| view.id)
                                .max()
                                .unwrap_or(0)
                                + 1,
                            revision: 1,
                            group,
                            focused_ship: Some(ship.ship),
                            query: TrackQuery {
                                sphere: Some((GalacticPosition::ZERO, 1e8)),
                                limit: 256,
                                work: 300_000,
                                ..Default::default()
                            },
                        }));
                    }
                }
                ui.horizontal(|ui| {
                    for (label, command) in [
                        ("Resume", ShipCommand::ResumeTravel),
                        ("Pause", ShipCommand::PauseTravel),
                        ("Undock", ShipCommand::Undock),
                    ] {
                        if ui.button(label).clicked() {
                            outgoing.push(Action::Ship {
                                ship: ship.ship,
                                authority_revision: ship.authority_revision,
                                command,
                            });
                        }
                    }
                });
                ui.label(format!(
                    "Travel {:?} · order {}",
                    ship.travel.status,
                    ship.travel.order + 1
                ));
                for (index, order) in ship.travel.orders.iter().enumerate() {
                    ui.label(format!("{}. {:?}", index + 1, order));
                }
                let mut order = None;
                if ui.button("Travel to coordinates").clicked() {
                    let parsed = travel
                        .coordinates
                        .split_whitespace()
                        .map(str::parse::<i128>)
                        .collect::<std::result::Result<Vec<_>, _>>();
                    match parsed {
                        Ok(values) if values.len() == 3 => {
                            let position = GalacticPosition {
                                x: values[0],
                                y: values[1],
                                z: values[2],
                            };
                            let reference = selection
                                .celestial()
                                .filter(|entity| {
                                    celestials.iter().any(|body| body.0.entity == *entity)
                                })
                                .map(travel::Reference::Celestial)
                                .or_else(|| {
                                    selection.contact().and_then(|selected| {
                                        let (contact, _) = contacts
                                            .iter()
                                            .find(|(contact, _)| contact.1 == selected)?;
                                        let track = &contact.0;
                                        track
                                            .entity
                                            .filter(|_| {
                                                track.tags.contains(&Tag::Kind("beacon".into()))
                                            })
                                            .map(travel::Reference::Beacon)
                                    })
                                });
                            let destination = if travel.relative_destination {
                                reference.map(|reference| travel::Destination::Relative {
                                    reference,
                                    offset: position,
                                    axes: if travel.body_fixed {
                                        travel::Axes::BodyFixed
                                    } else {
                                        travel::Axes::Galactic
                                    },
                                })
                            } else {
                                Some(travel::Destination::Galactic(position))
                            };
                            if let Some(destination) = destination {
                                order = Some(travel::Order::TravelTo(destination));
                            } else {
                                client.status =
                                    "Select a celestial body or beacon for the offset".into();
                            }
                        }
                        _ => client.status = "Enter three integer coordinates".into(),
                    }
                }
                if let Some(selected) = selection.contact() {
                    let group = selected.group;
                    let target = selected.track;
                    if let Some((contact, _)) =
                        contacts.iter().find(|(contact, _)| contact.1 == selected)
                    {
                        let track = &contact.0;
                        if let Some(beacon) = track
                            .entity
                            .filter(|_| track.tags.contains(&Tag::Kind("beacon".into())))
                        {
                            if ui.button("Travel to selected beacon").clicked() {
                                order = Some(travel::Order::TravelTo(travel::Destination::Beacon(
                                    beacon,
                                )));
                            }
                            if ui.button("Dock at selected station").clicked() {
                                order = Some(travel::Order::Dock(beacon));
                            }
                        }
                        if group != PUBLIC_GROUP && ui.button("Aim at selected contact").clicked() {
                            outgoing.ship(
                                ship,
                                ShipCommand::Aim {
                                    group,
                                    track: target,
                                },
                            );
                        }
                    }
                }
                if let Some(order) = order {
                    let mut orders = if travel.append_waypoint {
                        ship.travel.orders.clone()
                    } else {
                        Vec::new()
                    };
                    orders.push(order);
                    outgoing.ship(
                        ship,
                        ShipCommand::SetTravel {
                            expected_revision: ship.travel.revision,
                            orders,
                        },
                    );
                }
                ui.collapsing("MFD slots", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for slot in 0..8 {
                            if ui.button(format!("Open {}", slot + 1)).clicked() {
                                mfds.closed.remove(&(ship.ship, slot));
                                outgoing.push(Action::ScreenSubscribe {
                                    ship: ship.ship,
                                    slot,
                                    hz: 10,
                                });
                            }
                        }
                    });
                });
                if let Some(presentation) = details
                    .iter()
                    .map(|details| &details.0)
                    .find(|row| row.ship == ship.ship)
                {
                    for screen in &presentation.screens {
                        if ui.button(format!("Open {}", screen.title)).clicked() {
                            mfds.closed.remove(&(ship.ship, screen.slot));
                            outgoing.push(Action::ScreenSubscribe {
                                ship: ship.ship,
                                slot: screen.slot,
                                hz: 10,
                            });
                        }
                    }
                }
            }
            for result in session
                .results
                .iter()
                .filter(|result| result.error.is_some())
                .rev()
                .take(5)
            {
                ui.colored_label(egui::Color32::LIGHT_RED, result.error.as_ref().unwrap());
            }
        });
    Ok(())
}
