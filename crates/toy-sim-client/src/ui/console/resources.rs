use super::*;

pub(super) fn draw(ui: &mut egui::Ui, ship: &ShipTelemetry, details: &ShipPresentation) {
    ui.horizontal_top(|ui| {
        battery(ui, ship, details);
        ui.vertical(|ui| {
            ui.set_width(194.);
            for drive in &details.propulsion.drives {
                drive_reserve(ui, drive, details);
            }
            for resource in details.inventory.iter().filter(|resource| {
                resource.capacity_kg > 0.
                    && (details.propulsion.fuels.contains(&resource.resource)
                        || details.propulsion.ammunition.contains(&resource.resource))
            }) {
                reserve(
                    ui,
                    Icon::Cargo,
                    &resource.name,
                    resource.amount_kg,
                    resource.capacity_kg,
                );
            }
            if let Some(health) = &details.health
                && health.shield_reserve_capacity_kg > 0.
            {
                reserve(
                    ui,
                    Icon::Shield,
                    "Shield reserve",
                    ship.coolant_reserve_kg,
                    health.shield_reserve_capacity_kg,
                );
            }
        });
    });
}

fn battery(ui: &mut egui::Ui, ship: &ShipTelemetry, details: &ShipPresentation) {
    let charge = ship.battery_j as f64 / details.battery_capacity_j.max(1) as f64;
    let net = details.power_generated_w - details.power_consumed_w;
    ui.vertical(|ui| {
        ui.set_width(70.);
        gauges::vertical(
            ui,
            "BATTERY",
            &format!("{:.0}%", charge * 100.),
            charge,
            gauges::Tone::Reserve,
            egui::vec2(70., 194.),
        )
        .on_hover_text(format!(
            "{} / {}\nNet {}",
            units(ship.battery_j as f64, "J"),
            units(details.battery_capacity_j as f64, "J"),
            units(net, "W"),
        ));
    });
}

fn drive_reserve(ui: &mut egui::Ui, drive: &DriveReserve, details: &ShipPresentation) {
    let fraction = drive.delta_v_m_s / drive.full_delta_v_m_s.max(1.);
    ui.label(egui::RichText::new(&drive.name).monospace().size(10.));
    let response = gauges::gauge(
        ui,
        &format!("Δv {:.2} km/s", drive.delta_v_m_s / 1000.),
        27.,
        fraction,
        None,
        None,
        false,
        gauges::Tone::Reserve,
    );
    if let Some(resource) = details
        .inventory
        .iter()
        .find(|r| r.resource == drive.resource)
    {
        response.on_hover_text(format!(
            "{}: {} / {}\nRated flow: {}\nIdeal independent drive estimate; other fuel and power reserves may limit operation.",
            resource.name,
            units(resource.amount_kg, "kg"),
            units(resource.capacity_kg, "kg"),
            units(drive.flow_kg_s, "kg/s"),
        ));
    }
}

fn reserve(ui: &mut egui::Ui, icon: Icon, name: &str, amount: f64, capacity: f64) {
    let fraction = amount / capacity;
    ui.horizontal(|ui| {
        ui.label(icon.text(12.).color(gauges::Tone::Reserve.color(fraction)));
        ui.label(egui::RichText::new(format!("{name} {:.0}%", fraction * 100.)).size(10.))
            .on_hover_text(format!(
                "{} / {}",
                units(amount, "kg"),
                units(capacity, "kg")
            ));
    });
}
