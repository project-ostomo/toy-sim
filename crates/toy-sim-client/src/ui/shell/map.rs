use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct State {
    selected: Option<Id>,
    pan: egui::Vec2,
    zoom: f32,
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let catalogue = model.navigation;
    let connected: Vec<_> = catalogue
        .systems
        .iter()
        .filter(|system| {
            catalogue
                .beacons
                .iter()
                .any(|beacon| beacon.system == system.id && beacon.gate_exit.is_some())
        })
        .collect();
    let origin = model
        .ship
        .and_then(|ship| ship.pose.as_ref())
        .and_then(|pose| {
            connected.iter().min_by(|a, b| {
                a.position
                    .relative_to(pose.position)
                    .length_squared()
                    .total_cmp(&b.position.relative_to(pose.position).length_squared())
            })
        })
        .map(|s| s.id);
    let selected = state.selected.or(origin);
    let route = origin
        .zip(selected)
        .and_then(|(a, b)| toy_sim_model::navigation::gate_route(catalogue, a, b));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("WORMHOLE NETWORK")
                .strong()
                .color(ACCENT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Recenter").clicked() {
                state.pan = egui::Vec2::ZERO;
                state.zoom = 1.;
            }
            ui.weak("Drag to pan · scroll to zoom");
        });
    });
    let height = (ui.available_height() - 125.).max(150.);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );
    if state.zoom == 0. {
        state.zoom = 1.;
    }
    if response.dragged() {
        state.pan += ui.input(|i| i.pointer.delta());
    }
    if response.hovered() {
        state.zoom =
            (state.zoom * (ui.input(|i| i.smooth_scroll_delta.y) * 0.002).exp()).clamp(0.5, 3.);
    }
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 4., egui::Color32::from_rgb(9, 17, 26));
    for x in (0..rect.width() as usize).step_by(24) {
        for y in (0..rect.height() as usize).step_by(24) {
            painter.circle_filled(
                rect.min + egui::vec2(x as f32, y as f32),
                0.7,
                egui::Color32::from_rgb(30, 45, 58),
            );
        }
    }
    let anchor = connected.first().map(|s| s.position).unwrap_or_default();
    let extents = connected
        .iter()
        .map(|s| s.position.relative_to(anchor).truncate())
        .fold((glam::DVec2::ZERO, glam::DVec2::ZERO), |(lo, hi), p| {
            (lo.min(p), hi.max(p))
        });
    let span = (extents.1 - extents.0).max(glam::DVec2::ONE);
    let mut occupied = std::collections::BTreeSet::new();
    let positions: BTreeMap<_, _> = connected
        .iter()
        .map(|system| {
            let p = (system.position.relative_to(anchor).truncate() - extents.0) / span;
            let mut slot = ((p.x * 6.).round() as i32, (p.y * 3.).round() as i32);
            while !occupied.insert(slot) {
                slot.1 += 1;
            }
            let grid = egui::vec2(slot.0 as f32 / 6. - 0.5, slot.1 as f32 / 3. - 0.5);
            (
                system.id,
                rect.center()
                    + state.pan
                    + grid
                        * egui::vec2(
                            (rect.width() - 170.).max(100.),
                            (rect.height() - 95.).max(60.),
                        )
                        * state.zoom,
            )
        })
        .collect();
    for gate in &catalogue.beacons {
        let Some(exit) = gate
            .gate_exit
            .and_then(|id| catalogue.beacons.iter().find(|b| b.id == id))
        else {
            continue;
        };
        if gate.id > exit.id {
            continue;
        }
        let (Some(&a), Some(&b)) = (positions.get(&gate.system), positions.get(&exit.system))
        else {
            continue;
        };
        let active = route
            .as_ref()
            .is_some_and(|route| route.contains(&gate.id) || route.contains(&exit.id));
        let delta = b - a;
        let diagonal = delta.x.abs().min(delta.y.abs());
        let bend = a + egui::vec2(delta.x.signum(), delta.y.signum()) * diagonal;
        let color = if active {
            egui::Color32::from_rgb(255, 199, 98)
        } else {
            egui::Color32::from_rgb(51, 101, 131)
        };
        let close_to_segment = |p: egui::Pos2, a: egui::Pos2, b: egui::Pos2| {
            let d = b - a;
            let t = ((p - a).dot(d) / d.length_sq().max(0.001)).clamp(0., 1.);
            p.distance(a + d * t) < 18.
        };
        let crosses_stop = positions.iter().any(|(id, &p)| {
            *id != gate.system
                && *id != exit.system
                && (close_to_segment(p, a, bend) || close_to_segment(p, bend, b))
        });
        let points = if crosses_stop {
            vec![
                a,
                egui::pos2(a.x, (a.y + b.y) * 0.5),
                egui::pos2(b.x, (a.y + b.y) * 0.5),
                b,
            ]
        } else {
            vec![a, bend, b]
        };
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(if active { 3. } else { 2. }, color),
        ));
    }
    for system in connected {
        let p = positions[&system.id];
        let hit = ui.interact(
            egui::Rect::from_center_size(p, egui::vec2(28., 28.)),
            ui.id().with(system.id),
            egui::Sense::click(),
        );
        if hit.clicked() {
            state.selected = Some(system.id);
        }
        hit.on_hover_text(format!("{}\nClick to plan a route", system.name));
        let is_origin = origin == Some(system.id);
        let color = if selected == Some(system.id) {
            egui::Color32::WHITE
        } else if is_origin {
            ACCENT
        } else {
            MUTED
        };
        painter.circle_filled(p, 8., SURFACE);
        painter.circle_stroke(p, 8., egui::Stroke::new(2., color));
        if is_origin {
            painter.circle_filled(p, 3., ACCENT);
        }
        if selected == Some(system.id) {
            painter.circle_stroke(p, 13., egui::Stroke::new(1., ACCENT));
        }
        painter.text(
            p + egui::vec2(0., 19.),
            egui::Align2::CENTER_TOP,
            &system.name,
            egui::FontId::proportional(12.),
            color,
        );
        if is_origin {
            painter.text(
                p + egui::vec2(0., -17.),
                egui::Align2::CENTER_BOTTOM,
                "YOU ARE HERE",
                egui::FontId::proportional(9.),
                ACCENT,
            );
        }
    }
    ui.add_space(6.);
    if let Some(system) = catalogue.systems.iter().find(|s| Some(s.id) == selected) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&system.name).strong().size(16.));
            ui.weak(route.as_ref().map_or_else(
                || "No connected route".into(),
                |r| format!("{} jumps", r.len()),
            ));
        });
        ui.horizontal(|ui| {
            let orders: Vec<_> = route
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(travel::Order::Jump)
                .collect();
            if ui
                .add_enabled(
                    model.connected && route.as_ref().is_some_and(|r| !r.is_empty()),
                    egui::Button::new("Set destination"),
                )
                .clicked()
            {
                intents.push(Intent::Queue(orders.clone(), false));
            }
            let endpoint = model
                .ship
                .and_then(|ship| {
                    ship.travel
                        .orders
                        .iter()
                        .skip(ship.travel.order)
                        .rev()
                        .find_map(|order| {
                            let (id, jump) = match order {
                                travel::Order::Jump(id) => (*id, true),
                                travel::Order::Dock(id)
                                | travel::Order::TravelTo(travel::Destination::Beacon(id)) => {
                                    (*id, false)
                                }
                                _ => return None,
                            };
                            let beacon = catalogue.beacons.iter().find(|b| b.id == id)?;
                            if jump {
                                catalogue
                                    .beacons
                                    .iter()
                                    .find(|b| Some(b.id) == beacon.gate_exit)
                                    .map(|b| b.system)
                            } else {
                                Some(beacon.system)
                            }
                        })
                })
                .or(origin);
            let appended_route = endpoint.and_then(|start| {
                toy_sim_model::navigation::gate_route(catalogue, start, system.id)
            });
            if ui
                .add_enabled(
                    model.connected && appended_route.as_ref().is_some_and(|r| !r.is_empty()),
                    egui::Button::new("Add waypoint"),
                )
                .clicked()
            {
                if let Some(route) = &appended_route {
                    intents.push(Intent::Queue(
                        route.iter().copied().map(travel::Order::Jump).collect(),
                        true,
                    ));
                }
            }
            for station in catalogue
                .beacons
                .iter()
                .filter(|b| b.system == system.id && b.docking)
            {
                if ui
                    .add_enabled(
                        model.connected && route.is_some(),
                        egui::Button::new(format!("Dock · {}", station.name)),
                    )
                    .clicked()
                {
                    let append = ui.input(|i| i.modifiers.shift);
                    let planned = if append { &appended_route } else { &route };
                    if let Some(planned) = planned {
                        let mut orders: Vec<_> =
                            planned.iter().copied().map(travel::Order::Jump).collect();
                        orders.push(travel::Order::Dock(station.id));
                        intents.push(Intent::Queue(orders, append));
                    }
                }
            }
        });
        if let Some(route) = route {
            let names: Vec<_> = route
                .iter()
                .filter_map(|entry| {
                    catalogue
                        .beacons
                        .iter()
                        .find(|b| b.id == *entry)
                        .map(|b| b.name.as_str())
                })
                .collect();
            ui.label(
                egui::RichText::new(names.join("  →  "))
                    .size(11.)
                    .color(ACCENT),
            );
        }
    }
}
