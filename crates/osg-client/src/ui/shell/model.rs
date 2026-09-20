use super::*;

#[cfg(test)]
pub(super) fn empty_industry() -> &'static industry_model::IndustrySnapshot {
    static EMPTY: std::sync::OnceLock<industry_model::IndustrySnapshot> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default)
}

pub(super) struct Row {
    pub target: SelectedTarget,
    pub contact: Option<ContactRef>,
    pub name: String,
    pub kind: String,
    pub offset: glam::DVec3,
    pub distance: f64,
    pub speed: f64,
    pub radius: f64,
    pub detail: String,
    pub own: bool,
    pub can_look: bool,
    pub affiliation: Option<ownership::Principal>,
    pub standing: Option<ownership::Standing>,
}

impl Row {
    pub fn key(&self) -> (u8, Id, Id) {
        match self.target {
            SelectedTarget::Contact(reference) => (0, reference.group, reference.track),
            SelectedTarget::Celestial(id) => (1, id, id),
            SelectedTarget::Beacon(id) => (2, id, id),
        }
    }

    pub fn icon(&self) -> Icon {
        match self.target {
            SelectedTarget::Contact(_) => crate::ui::contacts::icon(&self.kind),
            SelectedTarget::Celestial(_) => Icon::Planet,
            SelectedTarget::Beacon(_) => Icon::Navigation,
        }
    }
}

pub(super) struct FrameModel<'a> {
    pub industry_ready: bool,
    pub industry: &'a industry_model::IndustrySnapshot,
    pub society: &'a ownership::SocietySnapshot,
    pub navigation: &'a NavigationCatalogue,
    pub navigation_status: &'a NavigationStatus,
    pub navigation_hash: Option<[u8; 32]>,
    pub celestial_systems: std::collections::BTreeMap<Id, Id>,
    pub ships: Vec<&'a ShipTelemetry>,
    pub rows: Vec<Row>,
    pub ship: Option<&'a ShipTelemetry>,
    pub details: Option<&'a ShipPresentation>,
    pub system: String,
    pub vicinity: String,
    pub connected: bool,
    pub status: &'a str,
    pub time_ns: u64,
    pub calendar_unix_ms: Option<i64>,
    pub diagnostics: ClientDiagnostics,
    pub orbits: bool,
}

pub(super) fn ship_name(ship: &ShipTelemetry) -> String {
    ship.iff
        .labels
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| "Your ship".into())
}

pub(super) fn short_id(id: Id) -> String {
    id.to_string().chars().take(8).collect()
}

pub(super) fn row_visible(row: &Row, state: &Shell, selected: Option<SelectedTarget>) -> bool {
    if selected == Some(row.target) {
        return true;
    }
    let kind_matches = match state.filter {
        Filter::General => {
            !row.kind.eq_ignore_ascii_case("missile")
                && !row.kind.eq_ignore_ascii_case("projectile")
        }
        Filter::All => true,
        Filter::Ships => matches!(row.target, SelectedTarget::Contact(_)) && row.kind != "Missile",
        Filter::Celestials => matches!(row.target, SelectedTarget::Celestial(_)),
    };
    let search = state.search.to_lowercase();
    kind_matches
        && (search.is_empty()
            || row.name.to_lowercase().contains(&search)
            || row.kind.to_lowercase().contains(&search))
}

pub(super) fn sorted_rows<'a>(
    rows: &'a [Row],
    state: &Shell,
    selected: Option<SelectedTarget>,
) -> Vec<&'a Row> {
    let mut rows: Vec<_> = rows
        .iter()
        .filter(|row| row_visible(row, state, selected))
        .collect();
    rows.sort_by(|a, b| {
        let order = match state.sort {
            Sort::Distance => a.distance.total_cmp(&b.distance),
            Sort::Name => a.name.cmp(&b.name),
            Sort::Kind => a.kind.cmp(&b.kind),
            Sort::Speed => a.speed.total_cmp(&b.speed),
        };
        let order = if state.descending {
            order.reverse()
        } else {
            order
        };
        order.then_with(|| a.key().cmp(&b.key()))
    });
    rows
}

pub(super) fn travel_status(status: &travel::Status) -> String {
    match status {
        travel::Status::Idle => "No route".into(),
        travel::Status::Planning => "Planning route".into(),
        travel::Status::Active => "Following route".into(),
        travel::Status::Paused => "Route paused".into(),
        travel::Status::Blocked(reason) => format!("Route blocked: {reason}"),
        travel::Status::Completed => "Route complete".into(),
    }
}

pub(super) fn computer_status(status: &ComputerStatus) -> String {
    match status {
        ComputerStatus::Unpowered => "Unpowered".into(),
        ComputerStatus::Booting { progress, .. } => format!("Booting · {:.0}%", progress * 100.),
        ComputerStatus::Running { execution, .. } => match execution {
            ExecutionStatus::Ready => "Running".into(),
            ExecutionStatus::Suspended => "Suspended · continuing next tick".into(),
            ExecutionStatus::WaitingForGas => "No gas · commands queued".into(),
        },
        ComputerStatus::Paused => "Paused".into(),
        ComputerStatus::Fault { message, .. } => format!("Fault: {message}"),
    }
}
