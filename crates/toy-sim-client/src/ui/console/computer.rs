use super::*;

fn remaining(seconds: f64, snapshot: u64, display: u64) -> f64 {
    let offset = if snapshot >= display {
        (snapshot - display) as f64 * 1e-9
    } else {
        -((display - snapshot) as f64 * 1e-9)
    };
    (seconds + offset).max(0.)
}

pub(super) fn draw(ui: &mut egui::Ui, details: &ShipPresentation, display_ns: u64) {
    let (label, fill, tone, tooltip) = match &details.computer {
        ComputerStatus::Running {
            gas_used,
            gas_limit,
            execution,
        } => {
            let fraction = *gas_used as f64 / (*gas_limit).max(1) as f64;
            let (suffix, status, tone) = match execution {
                ExecutionStatus::Ready => ("", "Ready for the next tick.", gauges::Tone::Heat),
                ExecutionStatus::Suspended => (
                    " · SUSPENDED",
                    "Execution is preserved and continues on the next tick.",
                    gauges::Tone::Heat,
                ),
                ExecutionStatus::WaitingForGas => (
                    " · NO GAS",
                    "Insufficient account gas to resume. Commands wait until the account can fund the pending work.",
                    gauges::Tone::Reserve,
                ),
            };
            (
                format!("CPU {:.0}%{suffix}", fraction * 100.),
                fraction,
                tone,
                format!(
                    "Last simulation tick: {gas_used} / {gas_limit} gas\n{status}\n\nThe meter shows this computer's physical work capacity per tick. Work spends the owner's shared gas account; the account balance does not increase that capacity. See Society → Computer gas for balances."
                ),
            )
        }
        ComputerStatus::Fault {
            message,
            reboot_remaining_s,
        } => {
            let label = reboot_remaining_s.map_or_else(
                || "FAULTED · waiting for power".into(),
                |seconds| {
                    let seconds = remaining(seconds, details.sim_time_ns, display_ns);
                    if seconds > 0. {
                        format!("FAULTED · reboot in {seconds:.1} s")
                    } else {
                        "FAULTED · restarting".into()
                    }
                },
            );
            (label, 1., gauges::Tone::Heat, message.clone())
        }
        ComputerStatus::Booting {
            progress,
            remaining_s,
        } => (
            format!(
                "CPU BOOTING · {:.1} s",
                remaining(*remaining_s, details.sim_time_ns, display_ns)
            ),
            *progress,
            gauges::Tone::Normal,
            "Flight computer is starting.".into(),
        ),
        ComputerStatus::Unpowered => (
            "CPU UNPOWERED".into(),
            0.,
            gauges::Tone::Reserve,
            "Flight computer has no power.".into(),
        ),
        ComputerStatus::Paused => (
            "CPU PAUSED".into(),
            0.,
            gauges::Tone::Normal,
            "Flight computer is suspended while the ship is stored or in transit.".into(),
        ),
    };
    gauges::gauge(ui, &label, 22., fill, None, None, false, tone).on_hover_ui(|ui| {
        ui.set_max_width(460.);
        egui::ScrollArea::vertical()
            .max_height(220.)
            .show(ui, |ui| {
                ui.label(tooltip);
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reboot_countdown_uses_simulation_time_and_stops_at_zero() {
        assert!((remaining(5., 1_000_000_000, 1_150_000_000) - 4.85).abs() < 1e-9);
        assert_eq!(remaining(5., 1_000_000_000, 7_000_000_000), 0.);
    }
}
