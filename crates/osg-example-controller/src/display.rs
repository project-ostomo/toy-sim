use osg_model::travel::{AutopilotState, FirmwarePhase, FirmwareStatus};
use osg_ship_api::abi;

pub const ROWS_PER_PAGE: usize = 17;
const LINE_COLUMNS: usize = 88;

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ComputerControl {
    SetAutopilot {
        directive_revision: u64,
        enabled: bool,
    },
    ClearItinerary {
        directive_revision: u64,
    },
}

impl ComputerControl {
    pub fn action(self) -> osg_model::ProgramAction {
        match self {
            Self::SetAutopilot {
                directive_revision,
                enabled,
            } => osg_model::ProgramAction::SetAutopilot {
                directive_revision,
                enabled,
            },
            Self::ClearItinerary { directive_revision } => {
                osg_model::ProgramAction::ClearItinerary { directive_revision }
            }
        }
    }
}

pub fn computer_control(
    event: &abi::ScreenEvent,
    state: &AutopilotState,
) -> Option<ComputerControl> {
    if event.screen != 1
        || event.kind != abi::EVENT_BEZEL
        || matches!(state.status.phase, FirmwarePhase::Transit)
    {
        return None;
    }
    match event.code {
        0 => Some(ComputerControl::SetAutopilot {
            directive_revision: state.directive_revision,
            enabled: !state.enabled,
        }),
        1 => Some(ComputerControl::ClearItinerary {
            directive_revision: state.directive_revision,
        }),
        _ => None,
    }
}

pub fn phase_name(phase: &FirmwarePhase) -> &'static str {
    match phase {
        FirmwarePhase::Idle => "IDLE",
        FirmwarePhase::Planning => "PLANNING",
        FirmwarePhase::Waiting { .. } => "WAITING",
        FirmwarePhase::Charging => "CHARGING",
        FirmwarePhase::Transit => "TRANSIT",
        FirmwarePhase::Maneuvering => "MANEUVERING",
        FirmwarePhase::Docking => "DOCKING",
        FirmwarePhase::Completed => "COMPLETED",
    }
}

pub fn status_line(status: &FirmwareStatus) -> String {
    let reason = match &status.phase {
        FirmwarePhase::Waiting { why, .. } => why,
        _ => &status.summary,
    };
    format!("{}: {reason}", phase_name(&status.phase))
}

pub fn autopilot_lines(state: &AutopilotState, tick: u64) -> Vec<String> {
    let mut lines = Vec::new();
    let remaining = |at: Option<u64>| {
        at.map_or_else(
            || "--".into(),
            |at| {
                format!(
                    "{:.0}s",
                    at.saturating_sub(tick) as f64 * osg_model::TICK_SECONDS
                )
            },
        )
    };
    if let Some(failure) = &state.failure {
        lines.push(format!("FAILURE: {failure}"));
    }
    if let Some(entry) = state.itinerary.first() {
        lines.push(format!("ACTIVE: {}", entry.label));
    }
    if let FirmwarePhase::Waiting { until, why } = &state.status.phase {
        lines.push(format!("WAIT {}: {why}", remaining(*until)));
        if !state.status.summary.is_empty() && state.status.summary != *why {
            lines.push(state.status.summary.clone());
        }
    } else if !state.status.summary.is_empty() {
        lines.push(state.status.summary.clone());
    }
    if let Some(body) = state.status.capture_body {
        lines.push(format!("CAPTURE {}", body.body));
    }
    if let Some(offset) = state.status.aim_offset_m {
        lines.push(format!(
            "AIM OFFSET {:.0} / {:.0} / {:.0} km",
            offset[0] / 1000.,
            offset[1] / 1000.,
            offset[2] / 1000.
        ));
    }
    lines.push(format!(
        "DEPART {}   ARRIVAL {}",
        remaining(state.status.departure_tick),
        remaining(state.status.estimated_arrival_tick)
    ));
    lines.push(format!(
        "ARRIVAL DV {:.2} km/s   PLANNED RISK {:.4} ppm",
        state.status.planned_delta_v_m_s / 1000.,
        state.status.planned_loss_ppm
    ));
    lines.push(format!(
        "RISK SPENT {:.4} ppm   TRIP LIMIT {:.4} ppm",
        state.status.spent_loss_ppm, state.preferences.max_loss_ppm
    ));
    lines.push(format!(
        "EXOTIC {:.3} kg planned / {:.3} kg spent",
        state.status.planned_exotic_fuel_kg, state.status.spent_exotic_fuel_kg
    ));
    lines.push("ITINERARY".into());
    for (index, entry) in state.itinerary.iter().enumerate() {
        lines.push(format!(
            "{} {}. {}",
            if index == 0 { ">" } else { " " },
            index + 1,
            entry.label
        ));
    }
    lines.into_iter().flat_map(|line| wrap(&line)).collect()
}

fn wrap(line: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut columns = 0;
    for word in line.split_whitespace() {
        let width = word.chars().count();
        if columns > 0 && columns + 1 + width > LINE_COLUMNS {
            rows.push(std::mem::take(&mut row));
            columns = 0;
        }
        if columns > 0 {
            row.push(' ');
            columns += 1;
        }
        for character in word.chars() {
            if columns == LINE_COLUMNS {
                rows.push(std::mem::take(&mut row));
                columns = 0;
            }
            row.push(character);
            columns += 1;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

pub fn page_change(event: &abi::ScreenEvent) -> isize {
    if event.screen == 1 && event.kind == abi::EVENT_BEZEL {
        return match event.code {
            10 => -1,
            11 => 1,
            _ => 0,
        };
    }
    if event.screen != 1
        || event.kind != abi::EVENT_POINTER_PRESS
        || !(466. ..500.).contains(&event.y)
    {
        return 0;
    }
    if (500. ..612.).contains(&event.x) {
        -1
    } else if (628. ..752.).contains(&event.x) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_reason_appears_once_and_long_failure_remains_readable() {
        let mut state = AutopilotState::default();
        state.status.phase = FirmwarePhase::Waiting {
            until: Some(100),
            why: "Waiting for the planet to clear".into(),
        };
        state.status.summary = "Waiting for the planet to clear".into();
        state.failure = Some("Long failure description ".repeat(12));
        let lines = autopilot_lines(&state, 50);
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Waiting for the planet"))
                .count(),
            1
        );
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() <= LINE_COLUMNS)
        );
        assert!(lines[0].starts_with("FAILURE:"));
        assert!(lines.iter().filter(|line| line.contains("failure")).count() > 1);
    }

    #[test]
    fn pagination_only_accepts_presses_on_autopilot_buttons() {
        let mut event = abi::ScreenEvent {
            screen: 1,
            kind: abi::EVENT_POINTER_PRESS,
            x: 650.,
            y: 480.,
            ..Default::default()
        };
        assert_eq!(page_change(&event), 1);
        event.kind = abi::EVENT_POINTER_RELEASE;
        assert_eq!(page_change(&event), 0);
        event.kind = abi::EVENT_POINTER_PRESS;
        event.screen = 0;
        assert_eq!(page_change(&event), 0);
    }

    #[test]
    fn stock_controls_preserve_revision_across_the_opaque_message() {
        let mut state = AutopilotState {
            directive_revision: 42,
            ..Default::default()
        };
        let mut event = abi::ScreenEvent {
            screen: 1,
            kind: abi::EVENT_BEZEL,
            ..Default::default()
        };
        let control = computer_control(&event, &state).unwrap();
        let bytes = postcard::to_allocvec(&control).unwrap();
        let restored: ComputerControl = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(
            restored.action(),
            osg_model::ProgramAction::SetAutopilot {
                directive_revision: 42,
                enabled: true
            }
        ));
        state.enabled = true;
        assert!(matches!(
            computer_control(&event, &state).unwrap().action(),
            osg_model::ProgramAction::SetAutopilot { enabled: false, .. }
        ));
        event.code = 1;
        assert!(matches!(
            computer_control(&event, &state).unwrap().action(),
            osg_model::ProgramAction::ClearItinerary {
                directive_revision: 42
            }
        ));
        state.status.phase = FirmwarePhase::Transit;
        assert!(computer_control(&event, &state).is_none());
        assert_eq!(status_line(&state.status), "TRANSIT: ");
    }

    #[test]
    fn every_itinerary_entry_remains_available_across_pages() {
        let mut state = AutopilotState::default();
        state.itinerary = (0..50)
            .map(|index| osg_model::travel::ItineraryEntry {
                directive: osg_model::travel::Directive::DockAt(osg_model::Id::default()),
                label: format!("Destination {index:02}"),
            })
            .collect();
        let rows = autopilot_lines(&state, 0);
        assert!(rows.len().div_ceil(ROWS_PER_PAGE) > 1);
        assert!(rows.last().unwrap().contains("Destination 49"));
    }
}
