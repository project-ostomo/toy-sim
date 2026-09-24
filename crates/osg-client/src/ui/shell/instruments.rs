use super::*;

pub(super) fn planning_progress(ui: &mut egui::Ui, progress: Option<&travel::PlanningProgress>) {
    let Some(progress) = progress else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.));
            ui.weak("Starting route planner…");
        });
        return;
    };
    let label = match progress.stage {
        travel::PlanningStage::LoadingCatalogue => "Loading navigation catalogue",
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
                        ShipCommand::SetGuidance(None),
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
    ui.small("Change the preference in Navigation and preview the destination to replan.");
    fuel_budget(ui, model);
    ui.separator();
    ui.label(
        egui::RichText::new("Autopilot itinerary")
            .color(ACCENT)
            .strong(),
    );
    ui.label(travel_status(&ship.travel));
    if let Some(reason) = &ship.travel.failure {
        ui.colored_label(THREAT, reason);
    }
    if !ship.travel.status.summary.is_empty() {
        ui.label(&ship.travel.status.summary);
    }
    if ship.travel.itinerary.is_empty() {
        ui.weak("No itinerary.");
    }
    let now = model.time_ns / osg_model::TICK_NS;
    let arrivals = stage_arrivals(&ship.travel, now);
    egui::ScrollArea::vertical()
        .max_height(180.)
        .show(ui, |ui| {
            for (index, order) in ship.travel.itinerary.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  {}", index + 1, order.label)).color(
                            if matches!(order.directive, travel::Directive::SlipToSystem(_)) {
                                crate::ui::travel_risk::color(Some(order.max_loss_ppm))
                            } else if index == 0 {
                                ACCENT
                            } else {
                                MUTED
                            },
                        ),
                    );
                    ui.monospace(eta_label(order, arrivals[index], now));
                    if ui
                        .small_button("×")
                        .on_hover_text("Remove command")
                        .clicked()
                    {
                        let mut orders: Vec<_> = ship
                            .travel
                            .itinerary
                            .iter()
                            .map(|stage| stage.directive.clone())
                            .collect();
                        orders.remove(index);
                        intents.push(Intent::EditItinerary(orders));
                    }
                    if index > 0 && ui.small_button("↑").on_hover_text("Move earlier").clicked() {
                        let mut orders: Vec<_> = ship
                            .travel
                            .itinerary
                            .iter()
                            .map(|stage| stage.directive.clone())
                            .collect();
                        orders.swap(index, index - 1);
                        intents.push(Intent::EditItinerary(orders));
                    }
                });
            }
        });
    if !ship.travel.itinerary.is_empty() {
        if ui.button("Clear itinerary").clicked() {
            intents.push(Intent::EditItinerary(Vec::new()));
        }
        let paused = !ship.travel.enabled;
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

pub(super) fn eta_label(_stage: &travel::ItineraryEntry, arrival: Option<u64>, now: u64) -> String {
    let Some(arrival) = arrival else {
        return "ETA —".into();
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

pub(super) fn stage_arrivals(state: &travel::AutopilotState, now: u64) -> Vec<Option<u64>> {
    let mut arrival = Some(now);
    state
        .itinerary
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            arrival = if index == 0 && state.enabled {
                state.status.estimated_arrival_tick.or_else(|| {
                    arrival
                        .zip(entry.estimated_duration_ticks)
                        .map(|(tick, duration)| tick.saturating_add(duration))
                })
            } else {
                arrival
                    .zip(entry.estimated_duration_ticks)
                    .map(|(tick, duration)| tick.saturating_add(duration))
            };
            arrival
        })
        .collect()
}

pub(super) fn itinerary(ui: &mut egui::Ui, state: &travel::AutopilotState, now: u64) {
    let remaining = &state.itinerary;
    if remaining.is_empty() {
        return;
    }
    let arrivals = stage_arrivals(state, now);
    ui.add_space(6.);
    ui.label(
        egui::RichText::new(format!("ROUTE · {} directives remaining", remaining.len()))
            .size(11.)
            .color(MUTED),
    );
    for (index, order) in remaining.iter().enumerate() {
        let current = index == 0;
        let color = match &order.directive {
            travel::Directive::SlipToSystem(_) => {
                crate::ui::travel_risk::color(Some(order.max_loss_ppm))
            }
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

            let name = format!("{}  {}", index + 1, order.label);
            let response = ui.label(egui::RichText::new(name).size(12.).color(color));
            if matches!(order.directive, travel::Directive::SlipToSystem(_)) {
                response.on_hover_text(format!("Risk allowance: {:.2} ppm", order.max_loss_ppm));
            }
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
    if ship.travel.itinerary.is_empty() {
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
            POSITIVE
        };
        meter(
            ui,
            name,
            requirement.required_kg,
            available,
            &format!(
                "~{} / {} aboard",
                osg_ui::units::mass(requirement.required_kg),
                osg_ui::units::mass(available)
            ),
            color,
        );
        if insufficient {
            ui.colored_label(
                THREAT,
                format!(
                    "FUEL EXHAUSTION · {} · estimated shortfall {}",
                    name,
                    osg_ui::units::mass((requirement.required_kg - available).max(0.))
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
        "The flight computer budgets arrival velocity changes and local maneuvers during flight.",
    );
    ui.small("Allow reserves for steering, gravity and changing mass. Reactor fuel is additional.");
}
