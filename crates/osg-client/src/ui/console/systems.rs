use super::*;

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut Console,
    ship: &ShipTelemetry,
    d: &ShipPresentation,
    time: u64,
    fixed: &Time<Fixed>,
    connected: bool,
) {
    if !matches!(d.computer, ComputerStatus::Running { .. }) {
        state.pending = None;
        state.requested = None;
        state.feedback = None;
    }
    let command = throttle(d, time);
    let enabled = manual(ship, d, connected) && command.is_some();
    if ship.travel.autopilot_enabled {
        state.pending = None;
    }
    if state.smooth.stamp != d.sim_time_ns {
        state.smooth.previous_force = state.smooth.force;
        state.smooth.previous_torque = state.smooth.torque;
        state.smooth.force = Vec3::from_array(d.propulsion.force_n.map(|v| v as f32));
        state.smooth.torque = Vec3::from_array(d.propulsion.torque_nm.map(|v| v as f32));
        if state.smooth.stamp == 0 {
            state.smooth.previous_force = state.smooth.force;
            state.smooth.previous_torque = state.smooth.torque;
        }
        state.smooth.stamp = d.sim_time_ns;
    }
    let rotation = Quat::from_array(d.control_rotation.map(|v| v as f32)).inverse();
    let force = rotation
        * state
            .smooth
            .previous_force
            .lerp(state.smooth.force, fixed.overstep_fraction());
    let torque = rotation
        * state
            .smooth
            .previous_torque
            .lerp(state.smooth.torque, fixed.overstep_fraction());
    let max = d.propulsion.rated_forward_n;
    let label = format!(
        "{} / {} · {}",
        units((-force.z as f64).max(0.), "N"),
        units(max, "N"),
        command.map_or("—".into(), |v| format!("{:.0}%", v * 100.))
    );
    let response = gauges::gauge(
        ui,
        &label,
        30.,
        if max > 0. { -force.z as f64 / max } else { 0. },
        command,
        state.pending.map(|(_, value, _)| value),
        enabled,
        gauges::Tone::Normal,
    );
    if enabled
        && (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        state.requested =
            Some(((pos.x - response.rect.left()) / response.rect.width()).clamp(0., 1.) as f64);
    }
    for (axis, name) in ["PITCH", "YAW", "ROLL"].iter().enumerate() {
        gauges::bipolar(
            ui,
            &format!("{name} {}", units(torque[axis] as f64, "Nm")),
            torque[axis] as f64,
            d.propulsion.negative_torque_nm[axis],
            d.propulsion.positive_torque_nm[axis],
        );
    }
    let hull = d
        .health
        .as_ref()
        .map_or(0., |h| h.hull_hp / h.hull_max_hp.max(1.));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        ui.allocate_ui_with_layout(
            egui::vec2(156., 30.),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                gauges::gauge(
                    ui,
                    &format!("HULL {:.0}%", hull * 100.),
                    30.,
                    hull,
                    None,
                    None,
                    false,
                    gauges::Tone::Reserve,
                );
            },
        );
        reactor_box(ui, d);
    });
    ui.horizontal(|ui| {
        let heat = ship.hull_heat_j / d.hull_heat_capacity_j.max(1.);
        gauges::vertical(
            ui,
            "HULL HEAT",
            &format!("{:.0}%", heat * 100.),
            heat,
            gauges::Tone::Heat,
            egui::vec2(86., 100.),
        )
        .on_hover_text(format!(
            "{} / {}",
            units(ship.hull_heat_j, "J"),
            units(d.hull_heat_capacity_j, "J")
        ));
        gauges::vertical(
            ui,
            "SHIELD",
            &format!("{:.0} K", ship.shield_temperature_k),
            ship.shield_temperature_k / osg_ships::thermal::VAPORIZATION_K,
            gauges::Tone::Heat,
            egui::vec2(86., 100.),
        );
        ui.vertical(|ui| {
            ui.label(format!(
                "{:.0}% coverage",
                d.health.as_ref().map_or(0., |h| h.shield_strength) * 100.
            ));
            power_flow(ui, &mut state.power, ship, d);
            if let Some(message) = &state.feedback {
                ui.colored_label(egui::Color32::LIGHT_RED, message);
            }
        });
    });
}

#[derive(Default)]
pub(super) struct PowerDisplay {
    updated: Option<f64>,
    numbers_at: f64,
    scale: f64,
    generated: f64,
    supplied: f64,
    requested: f64,
    charge: f64,
    numbers: [f64; 2],
}

fn power_flow(
    ui: &mut egui::Ui,
    state: &mut PowerDisplay,
    ship: &ShipTelemetry,
    d: &ShipPresentation,
) {
    let now = ui.input(|input| input.time);
    let dt = state
        .updated
        .map_or(1., |previous| (now - previous).max(0.));
    let alpha = if state.updated.is_none() {
        1.
    } else {
        1. - (-dt / 0.2).exp()
    };
    state.updated = Some(now);
    let scale = d.generation_capacity_w.max(1.);
    state.scale = if scale >= state.scale {
        scale
    } else {
        scale + (state.scale - scale) * (-dt / 3.).exp()
    };
    state.generated += (d.power_generated_w - state.generated) * alpha;
    state.supplied += (d.power_consumed_w - state.supplied) * alpha;
    state.requested += (d.power_requested_w - state.requested) * alpha;
    if now >= state.numbers_at {
        state.numbers = [d.power_generated_w, d.power_consumed_w];
        state.numbers_at = now + 0.25;
    }
    power_bar(
        ui,
        Icon::Industry,
        &units(state.numbers[0], "W"),
        state.generated / state.scale,
        None,
    )
    .on_hover_text(format!(
        "Generation: {}\nAvailable generation capacity: {}\nShared scale: {}",
        units(d.power_generated_w, "W"),
        units(d.generation_capacity_w, "W"),
        units(state.scale, "W")
    ));
    power_bar(ui, Icon::Power, &units(state.numbers[1], "W"), state.supplied / state.scale, Some(state.requested / state.scale))
        .on_hover_text(format!("Supplied load: {}\nRequested load: {}\nIncludes slipdrive charging.\nRed indicates unserved demand.", units(d.power_consumed_w, "W"), units(d.power_requested_w, "W")));
    let (label, fraction, tooltip) = if let Some(charge) = &d.slip_charge {
        let fraction = charge.stored_j as f64 / charge.required_j.max(1) as f64;
        let eta = charge.remaining_s.map_or_else(
            || "Waiting for power".into(),
            |seconds| {
                let seconds = seconds.ceil() as u64;
                format!("~{}:{:02}", seconds / 60, seconds % 60)
            },
        );
        (
            format!("{:.0}% · {eta}", fraction * 100.),
            fraction,
            format!(
                "Slipdrive charging\n{} / {}\nInput: {}",
                units(charge.stored_j as f64, "J"),
                units(charge.required_j as f64, "J"),
                units(charge.input_w, "W")
            ),
        )
    } else if matches!(ship.presence, travel::Presence::SlipTransit(_)) {
        ("In transit".into(), 1., "Slip transit in progress".into())
    } else if d.slip_available {
        ("Ready".into(), 1., "Slipdrive ready".into())
    } else {
        ("Not fitted".into(), 0., "No slipdrive installed".into())
    };
    state.charge += (fraction - state.charge) * alpha;
    power_bar(ui, Icon::Navigation, &label, state.charge, None).on_hover_text(tooltip);
}

fn reactor_status(status: ReactorStatus) -> (&'static str, u8) {
    match status {
        ReactorStatus::Running => ("RUNNING", 0),
        ReactorStatus::Standby => ("STANDBY", 1),
        ReactorStatus::Shutdown => ("SHUTDOWN", 2),
        ReactorStatus::Damaged => ("DAMAGED", 3),
    }
}

fn reactor_margin(reactor: &ReactorTelemetry) -> f64 {
    ((reactor.shutdown_temperature_k - reactor.temperature_k)
        / (reactor.shutdown_temperature_k - reactor.operating_temperature_k).max(1.))
    .clamp(0., 1.)
}

fn reactor_box(ui: &mut egui::Ui, details: &ShipPresentation) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(156., 30.), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0., egui::Color32::from_gray(18));
    painter.rect_stroke(
        rect,
        0.,
        egui::Stroke::new(1., egui::Color32::from_gray(65)),
        egui::StrokeKind::Inside,
    );
    let Some(worst) = details.reactors.iter().max_by(|a, b| {
        reactor_status(a.status)
            .1
            .cmp(&reactor_status(b.status).1)
            .then_with(|| reactor_margin(b).total_cmp(&reactor_margin(a)))
    }) else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "NO REACTOR",
            egui::FontId::monospace(10.),
            egui::Color32::GRAY,
        );
        response.on_hover_text("No conventional reactor fitted.");
        return;
    };

    let margin = details
        .reactors
        .iter()
        .map(reactor_margin)
        .fold(1.0_f64, f64::min);
    let color = if reactor_status(worst.status).1 >= 2 || margin < 0.2 {
        egui::Color32::from_rgb(235, 90, 80)
    } else if margin < 0.5 {
        egui::Color32::from_rgb(230, 180, 85)
    } else {
        egui::Color32::LIGHT_GRAY
    };
    painter.text(
        rect.center_top() + egui::vec2(0., 4.),
        egui::Align2::CENTER_TOP,
        format!("REACTOR · {}", reactor_status(worst.status).0),
        egui::FontId::monospace(10.),
        color,
    );
    let meter = egui::Rect::from_min_max(
        rect.left_bottom() + egui::vec2(5., -8.),
        rect.right_bottom() + egui::vec2(-5., -4.),
    );
    painter.rect_filled(meter, 0., egui::Color32::from_gray(40));
    painter.rect_filled(
        egui::Rect::from_min_size(
            meter.min,
            egui::vec2(meter.width() * margin as f32, meter.height()),
        ),
        0.,
        color,
    );
    response.on_hover_text(format!(
        "Thermal margin: full at operating temperature, empty at shutdown.\nWorst status and smallest thermal margin across {} reactor(s).\n\n{}",
        details.reactors.len(),
        details.reactors.iter().map(|reactor| format!(
            "{} · {}\nCore {:.0} K · Coolant {:.0} K\nOperating {:.0} K · Shutdown {:.0} K\nThermal headroom {:.0} K",
            reactor.name,
            reactor_status(reactor.status).0,
            reactor.temperature_k,
            reactor.coolant_temperature_k,
            reactor.operating_temperature_k,
            reactor.shutdown_temperature_k,
            (reactor.shutdown_temperature_k - reactor.temperature_k).max(0.),
        )).collect::<Vec<_>>().join("\n\n"),
    ));
}

fn power_bar(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    fraction: f64,
    requested: Option<f64>,
) -> egui::Response {
    let height = if label.contains('\n') { 32. } else { 22. };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(124., height), egui::Sense::hover());
    let painter = ui.painter();
    let white = egui::Color32::LIGHT_GRAY;
    painter.text(
        rect.left_center() + egui::vec2(8., 0.),
        egui::Align2::CENTER_CENTER,
        icon.glyph(),
        Icon::font(14.),
        white,
    );
    let bar = egui::Rect::from_min_max(rect.min + egui::vec2(20., 0.), rect.max);
    painter.rect_filled(bar, 0., egui::Color32::from_gray(22));
    let end = bar.left() + bar.width() * fraction.clamp(0., 1.) as f32;
    painter.rect_filled(
        egui::Rect::from_min_max(bar.min, egui::pos2(end, bar.bottom())),
        0.,
        egui::Color32::from_gray(65),
    );
    if let Some(requested) = requested {
        let marker = bar.left() + bar.width() * requested.clamp(0., 1.) as f32;
        if marker > end + 0.5 {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(end, bar.top()),
                    egui::pos2(marker, bar.bottom()),
                ),
                0.,
                egui::Color32::from_rgb(130, 38, 38),
            );
        }
        painter.line_segment(
            [
                egui::pos2(marker, bar.top()),
                egui::pos2(marker, bar.bottom()),
            ],
            egui::Stroke::new(1., white),
        );
    }
    painter.text(
        bar.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.),
        white,
    );
    response
}
