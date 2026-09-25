use super::*;

#[cfg(test)]
pub fn empty_services() -> &'static QueryState<crate::state::requests::services::View> {
    static EMPTY: std::sync::OnceLock<QueryState<crate::state::requests::services::View>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default)
}

#[cfg(test)]
pub fn empty_industry() -> &'static IndustryView {
    static EMPTY: std::sync::OnceLock<IndustryView> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default)
}

pub struct Row {
    pub celestial: Option<travel::CelestialRef>,
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
    pub fn key(&self) -> (u8, Id, u64) {
        match self.target {
            SelectedTarget::Contact(reference) => (0, reference.observer, reference.contact),
            SelectedTarget::Celestial(id) => (1, id, 0),
            SelectedTarget::Beacon(id) => (2, id, 0),
        }
    }

    pub fn icon(&self) -> Icon {
        match self.target {
            SelectedTarget::Contact(_) => Icon::Ship,
            SelectedTarget::Celestial(_) => Icon::Planet,
            SelectedTarget::Beacon(_) => Icon::Navigation,
        }
    }
}

pub struct FrameModel<'a> {
    pub declaration_history: &'a [osg_model::diplomacy::Declaration],
    pub declaration_history_next: Option<u64>,
    pub declaration_history_key: Option<(
        ownership::Principal,
        osg_model::diplomacy::DeclarationCategory,
        ownership::Principal,
    )>,
    pub services: &'a QueryState<crate::state::requests::services::View>,

    pub industry: &'a IndustryView,
    pub society: &'a SocietyData,
    pub navigation: &'a NavigationCatalogue,
    pub inhabited: std::sync::Arc<osg_model::InhabitedDirectory>,
    pub navigation_status: &'a NavigationStatus,
    pub navigation_hash: Option<[u8; 32]>,
    pub rows: Vec<Row>,
    pub ship: Option<&'a ShipTelemetry>,
    pub details: Option<&'a ShipPresentation>,
    pub system: String,
    pub vicinity: String,
    pub connected: bool,
    pub status: &'a str,
    pub time_ns: u64,
    pub calendar_unix_ms: i64,
    pub diagnostics: ClientDiagnostics,
    pub orbits: bool,
}

pub fn ship_name(ship: &ShipTelemetry) -> String {
    ship.iff
        .labels
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| "Your ship".into())
}

pub fn short_id(id: Id) -> String {
    id.to_string().chars().take(8).collect()
}

pub fn row_visible(row: &Row, state: &Shell, selected: Option<SelectedTarget>) -> bool {
    if selected == Some(row.target) {
        return true;
    }
    let kind_matches = match state.filter {
        Filter::General => !row.kind.eq_ignore_ascii_case("projectile"),
        Filter::All => true,
        Filter::Ships => matches!(row.target, SelectedTarget::Contact(_)),
        Filter::Celestials => matches!(row.target, SelectedTarget::Celestial(_)),
    };
    let search = state.search.to_lowercase();
    kind_matches
        && (search.is_empty()
            || row.name.to_lowercase().contains(&search)
            || row.kind.to_lowercase().contains(&search))
}

pub fn sorted_rows<'a>(
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

pub fn computer_status(status: &ComputerStatus) -> String {
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
