use super::*;

pub(super) fn planning_progress(ui: &mut egui::Ui, travel: &travel::TravelState) {
    if travel.status != travel::Status::Planning {
        return;
    }
    let Some(progress) = &travel.planning else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.));
            ui.weak("Starting route planner…");
        });
        return;
    };
    let label = match progress.stage {
        travel::PlanningStage::LoadingCatalogue => "Loading gate catalogue",
        travel::PlanningStage::BuildingGraph => "Building transfer graph",
        travel::PlanningStage::SearchingRoutes => "Comparing routes",
    };
    if let Some(total) = progress.total.filter(|&total| total > 0) {
        let fraction = progress.completed as f32 / total as f32;
        ui.add(
            egui::ProgressBar::new(fraction.clamp(0., 1.))
                .desired_width(280.)
                .text(format!("{label} · {} / {total}", progress.completed)),
        );
    } else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.));
            ui.label(
                egui::RichText::new(format!("{label} · {}", progress.completed))
                    .size(11.)
                    .color(ACCENT),
            );
        });
    }
}

pub(super) fn navigation(ui: &mut egui::Ui, model: &FrameModel, intents: &mut Vec<Intent>) {
    let Some(ship) = model.ship else {
        ui.weak("No controlled ship");
        return;
    };
    ui.label(
        egui::RichText::new("Flight guidance")
            .color(ACCENT)
            .strong(),
    );
    if let Some(navigation) = model
        .details
        .and_then(|details| details.instruments.as_ref())
        .and_then(|instruments| instruments.navigation.as_ref())
    {
        ui.label(format!("Throttle   {:.0}%", navigation.throttle * 100.));
        ui.label(format!("Stand-off   {}", distance(navigation.stand_off_m)));
        if !navigation.reason.is_empty() {
            ui.weak(&navigation.reason);
        }
    }
    ui.add_enabled_ui(
        model.connected && ship.presence == travel::Presence::Space,
        |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("Stop guidance")
                    .on_hover_text("Cancel automatic guidance; the ship retains its velocity")
                    .clicked()
                {
                    intents.push(Intent::Command(
                        ShipCommand::SetAutopilot(false),
                        "Stop guidance",
                    ));
                }
            });
        },
    );
    ui.separator();
    ui.label(format!(
        "Fuel allowance: {:.0}%",
        ship.travel.preferences.fuel_fraction * 100.
    ));
    ui.small("Change the preference in Gate Network and preview the destination to replan.");
    fuel_budget(ui, model);
    ui.separator();
    ui.label(egui::RichText::new("Travel orders").color(ACCENT).strong());
    ui.label(travel_status(&ship.travel.status));
    if ship.travel.orders.is_empty() {
        ui.weak("No route queued.");
    }
    let now = model.time_ns / 100_000_000;
    let arrivals = ship.travel.stage_arrivals(now);
    egui::ScrollArea::vertical()
        .max_height(180.)
        .show(ui, |ui| {
            for (index, order) in ship
                .travel
                .orders
                .iter()
                .enumerate()
                .skip(ship.travel.order)
            {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  {}",
                            index + 1,
                            order_label(&order.action, model.navigation, &model.celestial_systems)
                        ))
                        .color(if index == ship.travel.order {
                            ACCENT
                        } else {
                            MUTED
                        }),
                    );
                    ui.monospace(eta_label(order, arrivals[index - ship.travel.order], now));
                    if ui
                        .small_button("×")
                        .on_hover_text("Remove command")
                        .clicked()
                    {
                        let mut orders: Vec<_> = ship
                            .travel
                            .orders
                            .iter()
                            .skip(ship.travel.order)
                            .map(|stage| stage.action.clone())
                            .collect();
                        orders.remove(index - ship.travel.order);
                        intents.push(Intent::EditQueue(orders));
                    }
                    if index > ship.travel.order
                        && ui.small_button("↑").on_hover_text("Move earlier").clicked()
                    {
                        let mut orders: Vec<_> = ship
                            .travel
                            .orders
                            .iter()
                            .skip(ship.travel.order)
                            .map(|stage| stage.action.clone())
                            .collect();
                        orders.swap(index - ship.travel.order, index - ship.travel.order - 1);
                        intents.push(Intent::EditQueue(orders));
                    }
                });
            }
        });
    if !ship.travel.orders.is_empty() {
        if ui.button("Clear queue").clicked() {
            intents.push(Intent::EditQueue(Vec::new()));
        }
        let paused = ship.travel.status == travel::Status::Paused;
        if ui
            .add_enabled(
                model.connected,
                egui::Button::new(if paused {
                    "Resume route"
                } else {
                    "Pause route"
                }),
            )
            .clicked()
        {
            intents.push(Intent::Command(
                if paused {
                    ShipCommand::SetAutopilot(true)
                } else {
                    ShipCommand::SetAutopilot(false)
                },
                "Travel route",
            ));
        }
    }
}

pub(super) fn order_name(order: &travel::Order) -> String {
    match order {
        travel::Order::Guidance(guidance) => {
            format!("{:?} · {}", guidance.mode, distance(guidance.range_m))
        }
        travel::Order::Jump(id) => format!("Jump via {}", short_id(*id)),
        travel::Order::TravelTo(travel::Destination::Beacon(id)) => {
            format!("Travel to beacon {}", short_id(*id))
        }
        travel::Order::TravelTo(travel::Destination::Galactic(_)) => {
            "Travel to galactic coordinates".into()
        }
        travel::Order::TravelTo(travel::Destination::Relative { .. }) => {
            "Travel to relative coordinates".into()
        }
        travel::Order::Sublight(_) => "Sublight transfer".into(),
        travel::Order::Slip {
            destination:
                travel::Destination::Relative {
                    reference: travel::Reference::Celestial(id),
                    ..
                },
        } => format!("Slip near celestial {}", short_id(*id)),
        travel::Order::Slip { .. } => "Slip transit".into(),
        travel::Order::Dock(id) => format!("Dock at {}", short_id(*id)),
        travel::Order::Undock => "Undock".into(),
        travel::Order::WaitUntil(time) => format!("Wait until tick {time}"),
    }
}

pub(super) fn order_system(
    order: &travel::Order,
    navigation: &NavigationCatalogue,
    celestial_systems: &std::collections::BTreeMap<Id, Id>,
) -> Option<Id> {
    use travel::{Destination, Order, Reference};

    let reference = match order {
        Order::Jump(id) => {
            let entry = navigation.beacons.iter().find(|b| b.id == *id)?;
            return navigation
                .beacons
                .iter()
                .find(|b| Some(b.id) == entry.gate_exit)
                .map(|b| b.system);
        }
        Order::TravelTo(Destination::Relative {
            reference: Reference::Celestial(id),
            ..
        })
        | Order::Sublight(Destination::Relative {
            reference: Reference::Celestial(id),
            ..
        })
        | Order::Slip {
            destination:
                Destination::Relative {
                    reference: Reference::Celestial(id),
                    ..
                },
        } => return celestial_systems.get(id).copied(),
        Order::Dock(id)
        | Order::TravelTo(Destination::Beacon(id))
        | Order::Sublight(Destination::Beacon(id))
        | Order::Slip {
            destination: Destination::Beacon(id),
        }
        | Order::TravelTo(Destination::Relative {
            reference: Reference::Beacon(id),
            ..
        })
        | Order::Sublight(Destination::Relative {
            reference: Reference::Beacon(id),
            ..
        })
        | Order::Slip {
            destination:
                Destination::Relative {
                    reference: Reference::Beacon(id),
                    ..
                },
        } => Some(*id),
        _ => None,
    };
    if let Some(id) = reference {
        return navigation
            .beacons
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.system);
    }
    let position = match order {
        Order::Slip {
            destination: Destination::Galactic(destination),
        }
        | Order::TravelTo(Destination::Galactic(destination))
        | Order::Sublight(Destination::Galactic(destination)) => *destination,
        _ => return None,
    };
    navigation
        .systems
        .iter()
        .min_by(|a, b| {
            a.position
                .relative_to(position)
                .length_squared()
                .total_cmp(&b.position.relative_to(position).length_squared())
        })
        .map(|system| system.id)
}

pub(super) fn order_label(
    order: &travel::Order,
    navigation: &NavigationCatalogue,
    celestial_systems: &std::collections::BTreeMap<Id, Id>,
) -> String {
    use travel::{Destination, Order};

    let system = order_system(order, navigation, celestial_systems)
        .and_then(|id| navigation.systems.iter().find(|s| s.id == id));
    let action = match order {
        Order::Jump(_) => "Gate",
        Order::Slip { .. } => "Slip",
        Order::Sublight(_) => "Transfer",
        Order::TravelTo(_) => "Travel",
        _ => "",
    };
    if let Some(system) = system.filter(|_| !action.is_empty()) {
        return format!("{action} · {}", system.name);
    }
    let reference = match order {
        Order::Jump(id) | Order::Dock(id) | Order::TravelTo(Destination::Beacon(id)) => Some(*id),
        _ => None,
    };
    if let Some(beacon) = reference.and_then(|id| navigation.beacons.iter().find(|b| b.id == id)) {
        return format!(
            "{} · {}",
            if matches!(order, Order::Dock(_)) {
                "Dock"
            } else {
                action
            },
            beacon.name
        );
    }
    order_name(order)
}

pub(super) fn eta_label(stage: &travel::QueuedOrder, arrival: Option<u64>, now: u64) -> String {
    let Some(arrival) = arrival else {
        return if matches!(&stage.action, travel::Order::Guidance(g) if g.mode == travel::GuidanceMode::KeepRange)
        {
            "Continuous".into()
        } else {
            "ETA —".into()
        };
    };
    let seconds = arrival.saturating_sub(now).div_ceil(10);
    if seconds >= 3600 {
        format!(
            "ETA ~{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("ETA ~{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

pub(super) fn itinerary(
    ui: &mut egui::Ui,
    state: &travel::TravelState,
    navigation: &NavigationCatalogue,
    celestial_systems: &std::collections::BTreeMap<Id, Id>,
    now: u64,
) {
    let remaining = &state.orders[state.order.min(state.orders.len())..];
    if remaining.is_empty() {
        return;
    }
    let arrivals = state.stage_arrivals(now);
    ui.add_space(6.);
    ui.label(
        egui::RichText::new(format!("ROUTE · {} orders remaining", remaining.len()))
            .size(11.)
            .color(MUTED),
    );
    for (index, order) in remaining.iter().enumerate() {
        let current = index == 0;
        let color = match &order.action {
            travel::Order::Jump(_) => egui::Color32::from_rgb(255, 199, 98),
            travel::Order::Slip { .. } => ACCENT,
            _ if current => TEXT,
            _ => MUTED,
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 24.), egui::Sense::hover());
            let centre = rect.center();
            let stroke = egui::Stroke::new(1., MUTED);
            if index > 0 {
                ui.painter().line_segment([rect.center_top(), centre], stroke);
            }
            if index + 1 < remaining.len() {
                ui.painter().line_segment([centre, rect.center_bottom()], stroke);
            }

            let marker = egui::Rect::from_center_size(centre, egui::vec2(8., 8.));
            let fill = if current { color } else { egui::Color32::TRANSPARENT };
            ui.painter().rect(marker, 1., fill, egui::Stroke::new(1., color), egui::StrokeKind::Inside);

            let name = format!("{}  {}", state.order + index + 1, order_label(&order.action, navigation, celestial_systems));
            ui.label(egui::RichText::new(name).size(12.).color(color));
            ui.label(
                egui::RichText::new(eta_label(order, arrivals[index], now))
                    .monospace()
                    .size(11.)
                    .color(MUTED),
            )
            .on_hover_text("Estimated stage completion from now. Updated during flight; later stages include the time for earlier stages.");
        });
    }
}

pub(super) fn fuel_budget(ui: &mut egui::Ui, model: &FrameModel) {
    let Some(ship) = model.ship else {
        return;
    };
    if ship.travel.order >= ship.travel.orders.len() {
        return;
    }
    ui.label(
        egui::RichText::new("CURRENT ROUTE · PROPULSION FUEL")
            .strong()
            .color(ACCENT),
    );
    let Some(budget) = &ship.travel.fuel_budget else {
        ui.weak("Propulsion fuel estimate pending…");
        return;
    };
    fuel_estimate(ui, budget, model, true);
}

pub(super) fn fuel_estimate(
    ui: &mut egui::Ui,
    budget: &travel::FuelBudget,
    model: &FrameModel,
    current_inventory: bool,
) {
    for requirement in &budget.resources {
        let inventory = model.details.and_then(|details| {
            details
                .inventory
                .iter()
                .find(|resource| resource.resource == requirement.resource)
        });
        let available = if current_inventory {
            inventory.map_or(requirement.available_kg, |resource| resource.amount_kg)
        } else {
            requirement.available_kg
        };
        let name = inventory.map_or(requirement.resource.as_str(), |resource| {
            resource.name.as_str()
        });
        let insufficient = requirement.required_kg > 0. && requirement.required_kg >= available;
        let fraction = requirement.required_kg / available.max(1e-9);
        let color = if insufficient {
            THREAT
        } else if fraction > 0.8 {
            egui::Color32::from_rgb(255, 199, 98)
        } else {
            ACCENT
        };
        ui.colored_label(
            color,
            format!(
                "{name}: ~{} needed / {} aboard",
                toy_sim_ui::units::mass(requirement.required_kg),
                toy_sim_ui::units::mass(available)
            ),
        );
        if insufficient {
            ui.colored_label(
                THREAT,
                format!(
                    "FUEL EXHAUSTION · {} · estimated shortfall {}",
                    name,
                    toy_sim_ui::units::mass((requirement.required_kg - available).max(0.))
                ),
            );
        }
    }
    if !budget.complete {
        ui.colored_label(
            egui::Color32::from_rgb(255, 199, 98),
            "Partial estimate · unplanned or continuous stages excluded",
        );
    }
    ui.small(
        "Slip preserves velocity; estimates include matching destination motion after arrival.",
    );
    ui.small("Allow reserves for steering, gravity and changing mass. Reactor fuel is additional.");
}
