use super::*;

pub(super) fn ship_status(ui: &mut egui::Ui, model: &FrameModel, intents: &mut Vec<Intent>) {
    let Some(ship) = model.ship else {
        ui.weak("Waiting for ship telemetry…");
        return;
    };
    ui.heading(ship_name(ship));
    ui.label(
        egui::RichText::new(presence_name(&ship.presence))
            .size(12.)
            .color(MUTED),
    );
    egui::ScrollArea::vertical().show(ui, |ui| {
        let Some(details) = model.details else {
            ui.weak("Waiting for instruments…");
            return;
        };
        if let Some(health) = &details.health {
            meter(
                ui,
                "Hull integrity",
                health.hull_hp,
                health.hull_max_hp,
                &format!("{:.0} / {:.0}", health.hull_hp, health.hull_max_hp),
                ACCENT,
            );
            meter(
                ui,
                "Shield reserve",
                ship.coolant_reserve_kg,
                health.shield_reserve_capacity_kg,
                &format!("{:.0} kg", ship.coolant_reserve_kg),
                ACCENT,
            );
        }
        meter(
            ui,
            "Battery",
            ship.battery_j,
            details.battery_capacity_j,
            &format!("{:.1} MJ", ship.battery_j / 1e6),
            ACCENT,
        );
        meter(
            ui,
            "Hull heat",
            ship.hull_heat_j,
            details.hull_heat_capacity_j,
            &format!("{:.1} MJ", ship.hull_heat_j / 1e6),
            egui::Color32::from_rgb(226, 178, 109),
        );
        ui.label(format!(
            "Shield temperature   {:.0} K",
            ship.shield_temperature_k
        ));
        ui.label(format!(
            "Power   +{:.2} / −{:.2} MW",
            details.power_generated_w / 1e6,
            details.power_consumed_w / 1e6
        ));
        ui.separator();
        for resource in &details.inventory {
            meter(
                ui,
                &resource.name,
                resource.amount_kg,
                resource.capacity_kg,
                &format!("{:.0} kg", resource.amount_kg),
                MUTED,
            );
        }
        ui.separator();
        let mut iff = ship.iff.enabled;
        if ui
            .checkbox(&mut iff, "Broadcast IFF / transponder")
            .changed()
        {
            intents.push(Intent::Command(
                ShipCommand::SetTransponderEnabled(iff),
                "Transponder",
            ));
        }
        ui.label(
            egui::RichText::new(format!(
                "Flight computer: {}",
                computer_status(&details.computer)
            ))
            .size(11.)
            .color(MUTED),
        );
    });
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
                if ui.button("Hold attitude").clicked() {
                    intents.push(Intent::Command(
                        ShipCommand::Flight(FlightCommand::HoldAttitude),
                        "Hold attitude",
                    ));
                }
                if ui
                    .button("Stop guidance")
                    .on_hover_text("Cancel automatic guidance; the ship retains its velocity")
                    .clicked()
                {
                    intents.push(Intent::Command(ShipCommand::PauseTravel, "Stop guidance"));
                }
            });
        },
    );
    ui.separator();
    ui.label(egui::RichText::new("Travel orders").color(ACCENT).strong());
    ui.label(travel_status(&ship.travel.status));
    if ship.travel.orders.is_empty() {
        ui.weak("No route programmed by the flight computer.");
    }
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
                            order_label(order, model.navigation)
                        ))
                        .color(if index == ship.travel.order {
                            ACCENT
                        } else {
                            MUTED
                        }),
                    );
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
                            .cloned()
                            .collect();
                        orders.remove(index - ship.travel.order);
                        intents.push(Intent::Queue(orders, false));
                    }
                    if index > ship.travel.order
                        && ui.small_button("↑").on_hover_text("Move earlier").clicked()
                    {
                        let mut orders: Vec<_> = ship
                            .travel
                            .orders
                            .iter()
                            .skip(ship.travel.order)
                            .cloned()
                            .collect();
                        orders.swap(index - ship.travel.order, index - ship.travel.order - 1);
                        intents.push(Intent::Queue(orders, false));
                    }
                });
            }
        });
    if !ship.travel.orders.is_empty() {
        if ui.button("Clear queue").clicked() {
            intents.push(Intent::Queue(Vec::new(), false));
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
                    ShipCommand::ResumeTravel
                } else {
                    ShipCommand::PauseTravel
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
        travel::Order::Dock(id) => format!("Dock at {}", short_id(*id)),
        travel::Order::Undock => "Undock".into(),
        travel::Order::WaitUntil(time) => format!("Wait until tick {time}"),
    }
}

pub(super) fn order_label(order: &travel::Order, navigation: &NavigationCatalogue) -> String {
    let reference = match order {
        travel::Order::Jump(id)
        | travel::Order::Dock(id)
        | travel::Order::TravelTo(travel::Destination::Beacon(id)) => Some(*id),
        _ => None,
    };
    if let Some(beacon) = reference.and_then(|id| navigation.beacons.iter().find(|b| b.id == id)) {
        let action = match order {
            travel::Order::Jump(_) => "Jump",
            travel::Order::Dock(_) => "Dock",
            _ => "Travel",
        };
        format!("{action} · {}", beacon.name)
    } else {
        order_name(order)
    }
}
