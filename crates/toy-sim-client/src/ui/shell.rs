mod industry;
mod instruments;
mod inventory;
mod map;
mod model;
mod overview;
mod panels;
mod society;

use super::{SelectedTarget, Selection, scene};
use crate::state::*;
use bevy::prelude::*;
use model::*;
use toy_sim_model::industry as industry_model;
use toy_sim_model::*;
use toy_sim_ui::units::distance;
use toy_sim_ui::{bevy_egui::EguiContexts, desktop::*, egui, icons::Icon};

const SELECTED: WindowSpec = WindowSpec {
    id: "selected",
    title: "SELECTED ITEM",
    size: egui::vec2(390., 288.),
    min_size: egui::vec2(380., 140.),
    anchor: egui::Align2::RIGHT_TOP,
    offset: egui::Vec2::ZERO,
    open: true,
};
const OVERVIEW: WindowSpec = WindowSpec {
    id: "overview",
    title: "OVERVIEW",
    size: egui::vec2(390., 380.),
    min_size: egui::vec2(350., 150.),
    anchor: egui::Align2::RIGHT_TOP,
    offset: egui::vec2(0., 302.),
    open: true,
};
const NAVIGATION: WindowSpec = WindowSpec {
    id: "navigation",
    title: "NAVIGATION",
    size: egui::vec2(330., 260.),
    min_size: egui::vec2(280., 200.),
    anchor: egui::Align2::LEFT_BOTTOM,
    offset: egui::Vec2::ZERO,
    open: false,
};
const INVENTORY: WindowSpec = WindowSpec {
    id: "inventory",
    title: "INVENTORY",
    size: egui::vec2(510., 390.),
    min_size: egui::vec2(370., 280.),
    anchor: egui::Align2::LEFT_TOP,
    offset: egui::vec2(0., 145.),
    open: false,
};
const MAP: WindowSpec = WindowSpec {
    id: "map",
    title: "GATE NETWORK",
    size: egui::vec2(720., 510.),
    min_size: egui::vec2(480., 350.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const SOCIETY: WindowSpec = WindowSpec {
    id: "society",
    title: "SOCIETY & OWNERSHIP",
    size: egui::vec2(760., 560.),
    min_size: egui::vec2(650., 420.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const INDUSTRY: WindowSpec = WindowSpec {
    id: "industry",
    title: "INDUSTRY",
    size: egui::vec2(720., 560.),
    min_size: egui::vec2(580., 400.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const SETTINGS: WindowSpec = WindowSpec {
    id: "settings",
    title: "INTERFACE",
    size: egui::vec2(310., 240.),
    min_size: egui::vec2(260., 200.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Filter {
    #[default]
    All,
    Ships,
    Celestials,
}
#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Sort {
    #[default]
    Distance,
    Name,
    Kind,
    Speed,
}

#[derive(Resource)]
struct Shell {
    desktop: Desktop,
    inventory: inventory::State,
    industry: industry::State,
    map: map::State,
    society: society::State,
    filter: Filter,
    sort: Sort,
    descending: bool,
    search: String,
    stand_off: f64,
    feedback: Option<Feedback>,
}

struct Feedback {
    pending: Vec<Id>,
    label: String,
    last_tick: u64,
    error: Option<String>,
}

impl Feedback {
    fn receive(&mut self, results: &[CommandResult]) {
        self.pending.retain(|id| {
            let Some(result) = results.iter().find(|result| result.id == *id) else {
                return true;
            };

            self.last_tick = self.last_tick.max(result.effective_tick);
            if let Some(error) = &result.error {
                self.error = Some(error.clone());
            }
            false
        });
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            desktop: Desktop::default(),
            inventory: inventory::State::default(),
            industry: industry::State::default(),
            map: map::State::default(),
            society: society::State::default(),
            filter: Filter::default(),
            sort: Sort::default(),
            descending: false,
            search: String::new(),
            stand_off: 1000.,
            feedback: None,
        }
    }
}

enum Intent {
    FocusShip(Id),
    InspectInventory(Id),
    Industry(industry_model::IndustryCommand, &'static str),
    RetryNavigation,
    InspectAffiliation(ownership::Principal),
    Society(ownership::SocietyCommand, &'static str),
    Select(SelectedTarget),
    Look(Option<SelectedTarget>),
    Align(SelectedTarget),
    Approach(ContactRef, f64),
    KeepRange(ContactRef, f64),
    Queue(Vec<travel::Order>, bool),
    PlanRoute(Vec<travel::Order>, bool, travel::PlanningPreferences),
    EditQueue(Vec<travel::Order>),
    Command(ShipCommand, &'static str),
    Orbits(bool),
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ShellDraw;

pub(super) fn install(app: &mut App) {
    app.init_resource::<Shell>()
        .add_observer(reset_session)
        .add_systems(
            toy_sim_ui::bevy_egui::EguiPrimaryContextPass,
            draw.in_set(ShellDraw).after(super::console::ConsoleDraw),
        );
}

fn reset_session(_: On<SessionReset>, mut shell: ResMut<Shell>) {
    shell.feedback = None;
    shell.society = society::State::default();
    shell.map = map::State::default();
    shell.inventory = inventory::State::default();
    shell.industry = industry::State::default();
}

fn draw(
    mut contexts: EguiContexts,
    mut shell: ResMut<Shell>,
    mut selection: ResMut<Selection>,
    mut outgoing: ResMut<Outgoing>,
    mut session: ResMut<SessionInfo>,
    clock: Res<RenderTime>,
    calendar: Res<CalendarClock>,
    real_time: Res<Time<Real>>,
    asset_server: Res<AssetServer>,
    diagnostics: Res<ClientDiagnostics>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
    optical: Query<&Optical>,
    beacons: Query<(&NavigationObject, &DisplayPose)>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    mut views: Query<(
        &ViewObservation,
        &SystemSubscription,
        &mut scene::CameraOptions,
        &mut scene::ViewOptions,
    )>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let ship = ships
        .iter()
        .find(|(ship, _, _)| Some(ship.0.ship) == selection.ship);
    let telemetry = ship.map(|(ship, _, _)| &ship.0);
    let details = ship.and_then(|(_, details, _)| details.map(|details| &details.0));
    let view = views
        .iter()
        .find(|(view, _, _, _)| Some(view.0.id) == selection.view);
    let displayed_ship = ship.and_then(|(_, _, pose)| pose.map(|pose| &pose.0));
    let origin = displayed_ship
        .map(|pose| pose.position)
        .or_else(|| view.map(|(view, _, _, _)| view.0.origin))
        .unwrap_or_default();
    let velocity = displayed_ship.map_or(glam::DVec3::ZERO, |pose| {
        glam::DVec3::from_array(pose.velocity)
    });
    let mut primary_acceleration = 0.;
    let mut vicinity = "No celestial reference".to_string();
    let mut primary_luminosity = 0.;
    let mut system_name = "Deep space".to_string();
    let mut rows = Vec::new();
    if let Some((view, systems, _, _)) = view {
        for (contact, pose) in &contacts {
            let track = &contact.0;
            if contact.1.group != view.0.group || !view.0.tracks.contains(&track.id) {
                continue;
            }
            let own = track.entity.is_some_and(|id| Some(id) == selection.ship);
            if own
                || track
                    .entity
                    .is_some_and(|id| beacons.iter().any(|(b, _)| b.0.id == id))
            {
                continue;
            }
            let name = track
                .tags
                .iter()
                .find_map(|tag| match tag {
                    Tag::Advertised(name) => Some(name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("Contact {}", short_id(track.id)));
            let kind = super::contacts::kind(&track.tags).to_owned();
            rows.push(Row {
                target: SelectedTarget::Contact(contact.1),
                name,
                kind,
                offset: pose.0.position.relative_to(origin),
                distance: pose.0.position.relative_to(origin).length(),
                speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
                radius: track.radius_m.unwrap_or(0.),
                own,
                can_look: pose.0.position.relative_to(origin).length() <= scene::LOOK_AT_RANGE_M
                    && optical.iter().any(|object| {
                        object.0.view == view.0.id && object.0.contact == Some(contact.1)
                    }),
                affiliation: super::standing::advertised_principal(&track.tags),
                standing: session
                    .society
                    .directory
                    .track_standing(session.society.account, &track.tags),
                detail: format!(
                    "{:?} · position uncertainty {}",
                    track.provenance,
                    distance(track.position_sigma_m)
                ),
            });
        }
        for (body, pose, system) in &bodies {
            if !systems
                .0
                .iter()
                .any(|reference| reference.system == system.0)
            {
                continue;
            }
            let range = pose.0.position.relative_to(origin).length();
            let acceleration =
                body.0.gravitational_parameter / range.max(body.0.radius_m).max(1.).powi(2);
            if acceleration > primary_acceleration {
                primary_acceleration = acceleration;
                vicinity = format!(
                    "{}  /  {} altitude",
                    body.0.name,
                    distance((range - body.0.radius_m).max(0.))
                );
            }
            if body.0.luminosity_lumens > primary_luminosity {
                primary_luminosity = body.0.luminosity_lumens;
                system_name = body.0.name.clone();
            }

            rows.push(Row {
                target: SelectedTarget::Celestial(body.0.entity),
                name: body.0.name.clone(),
                kind: if body.0.luminosity_lumens > 0. {
                    "Star"
                } else {
                    "Celestial"
                }
                .into(),
                offset: pose.0.position.relative_to(origin),
                distance: pose.0.position.relative_to(origin).length(),
                speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
                radius: body.0.radius_m,
                affiliation: None,
                standing: None,
                detail: "Orrery ephemeris".into(),
                own: false,
                can_look: true,
            });
        }
    }
    for (beacon, pose) in &beacons {
        let beacon = &beacon.0;
        let offset = pose.0.position.relative_to(origin);
        if offset.length() > 1e12 {
            continue;
        }
        rows.push(Row {
            target: SelectedTarget::Beacon(beacon.id),
            name: beacon.name.clone(),
            kind: if beacon.gate_exit.is_some() {
                "Stargate"
            } else {
                "Station"
            }
            .into(),
            offset,
            distance: offset.length(),
            speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
            radius: beacon.radius_m,
            can_look: offset.length() <= scene::LOOK_AT_RANGE_M
                && (beacon.gate_exit.is_some()
                    || optical.iter().any(|object| {
                        Some(object.0.view) == selection.view
                            && object.0.known_entity == Some(beacon.id)
                    })),
            affiliation: contacts
                .iter()
                .find(|(contact, _)| contact.0.entity == Some(beacon.id))
                .and_then(|(contact, _)| super::standing::advertised_principal(&contact.0.tags)),
            standing: contacts
                .iter()
                .find(|(contact, _)| contact.0.entity == Some(beacon.id))
                .and_then(|(contact, _)| {
                    session
                        .society
                        .directory
                        .track_standing(session.society.account, &contact.0.tags)
                }),
            detail: "Subspace beacon".into(),
            own: false,
        });
    }
    let model = FrameModel {
        industry: &session.industry.snapshot,
        navigation: &session.navigation,
        navigation_status: &session.navigation_status,
        navigation_hash: session.navigation_hash,
        celestial_systems: bodies
            .iter()
            .map(|(body, _, system)| (body.0.entity, system.0))
            .collect(),
        society: &session.society,
        rows,
        ship: telemetry,
        details,
        ships: ships.iter().map(|(ship, _, _)| &ship.0).collect(),
        system: system_name,
        vicinity,
        connected: session.world.is_some() && session.status.is_empty(),
        status: &session.status,
        time_ns: clock.display_ns,
        calendar_unix_ms: calendar.now(real_time.elapsed()),
        diagnostics: *diagnostics,
        orbits: view.is_some_and(|(_, _, _, options)| options.enabled),
    };
    let mut intents = Vec::new();
    panels::draw(
        ctx,
        &mut shell,
        &model,
        &selection,
        &session.results,
        &mut intents,
    );
    for intent in intents {
        let preferences = match &intent {
            Intent::PlanRoute(_, _, preferences) => *preferences,
            _ => telemetry.map_or_else(Default::default, |ship| ship.travel.preferences),
        };
        match intent {
            Intent::FocusShip(ship) => {
                if model.ships.iter().any(|owned| owned.ship == ship) {
                    selection.ship = Some(ship);
                    selection.target = None;
                    shell.inventory.focus(ship);
                    for (view, _, mut camera, _) in &mut views {
                        if Some(view.0.id) == selection.view {
                            camera.focus = None;
                        }
                    }
                }
            }
            Intent::InspectInventory(entity) => {
                shell.inventory.focus(entity);
                shell.desktop.open(INVENTORY);
            }
            Intent::Industry(command, label) => {
                if model.connected {
                    let id = outgoing.push(Action::Industry(command));
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: label.into(),
                        last_tick: 0,
                        error: None,
                    });
                }
            }
            Intent::RetryNavigation => {
                if let Some(hash) = session.navigation_hash {
                    asset_server.reload(crate::assets::path(hash));
                }
            }
            Intent::InspectAffiliation(principal) => {
                shell.society.inspect(principal);
                shell.desktop.open(SOCIETY);
            }
            Intent::Society(command, label) => {
                if model.connected {
                    let id = outgoing.push(Action::Society(command));
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: label.into(),
                        last_tick: 0,
                        error: None,
                    });
                }
            }
            Intent::EditQueue(orders) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    outgoing.ship(
                        ship,
                        ShipCommand::SetTravel {
                            preferences: ship.travel.preferences,
                            engage: false,
                            expected_revision: ship.travel.revision,
                            orders,
                        },
                    );
                }
            }
            Intent::Queue(orders, append) | Intent::PlanRoute(orders, append, _) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    let mut queue = if append {
                        ship.travel
                            .orders
                            .iter()
                            .skip(ship.travel.order)
                            .map(|stage| stage.action.clone())
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    queue.extend(orders);
                    if queue.len() > 256 {
                        continue;
                    }
                    let id = outgoing.ship(
                        ship,
                        ShipCommand::SetTravel {
                            preferences,
                            engage: true,
                            expected_revision: ship.travel.revision,
                            orders: queue,
                        },
                    );
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: "Command queue".into(),
                        last_tick: 0,
                        error: None,
                    });
                }
            }
            Intent::Select(target) => {
                selection.target = Some(target);
                shell.desktop.open(SELECTED);
            }
            Intent::Look(target) => {
                if target.is_some() {
                    if !model
                        .rows
                        .iter()
                        .any(|row| Some(row.target) == target && row.can_look)
                    {
                        shell.feedback = Some(Feedback {
                            pending: vec![],
                            label: "Look at".into(),
                            last_tick: session.tick,
                            error: Some(
                                "Object must be optically visible and within camera range (100 km)"
                                    .into(),
                            ),
                        });
                        continue;
                    }
                }
                for (view, _, mut camera, _) in &mut views {
                    if Some(view.0.id) == selection.view {
                        camera.focus = target;
                    }
                }
            }
            Intent::Orbits(enabled) => {
                for (view, _, _, mut options) in &mut views {
                    if Some(view.0.id) == selection.view {
                        options.enabled = enabled;
                    }
                }
            }
            intent => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    if let Some((commands, label)) = commands_for(intent, ship, &model.rows) {
                        let pending = commands
                            .into_iter()
                            .map(|command| outgoing.ship(ship, command))
                            .collect();
                        shell.feedback = Some(Feedback {
                            pending,
                            label: label.into(),
                            last_tick: 0,
                            error: None,
                        });
                    }
                }
            }
        }
    }
    let mut interest = industry_model::IndustrySubscription::default();
    if shell.desktop.is_open(INDUSTRY) {
        interest.directory = true;
        interest.catalogue = true;
        interest.directory_after = shell.industry.directory_after;
        interest.inventories.extend(shell.industry.facility);
    }
    if shell.desktop.is_open(INVENTORY) {
        interest.directory = true;
        interest.inventories.extend(selection.ship);
        interest.inventories.extend(shell.inventory.inventories());
    }
    let wanted = (interest.directory && model.connected).then_some(interest);
    drop(model);
    session.industry.subscribe(wanted, &mut outgoing);
    Ok(())
}

fn commands_for(
    intent: Intent,
    ship: &ShipTelemetry,
    rows: &[Row],
) -> Option<(Vec<ShipCommand>, &'static str)> {
    if !matches!(intent, Intent::Command(_, _)) && ship.presence != travel::Presence::Space {
        return None;
    }
    let (commands, label) = match intent {
        Intent::Align(target) => {
            let row = rows.iter().find(|row| row.target == target)?;
            row.offset.try_normalize()?;
            let target = match target {
                SelectedTarget::Contact(reference) => travel::Target::Contact(reference),
                SelectedTarget::Beacon(id) => {
                    travel::Target::Destination(travel::Destination::Beacon(id))
                }
                SelectedTarget::Celestial(id) => {
                    travel::Target::Destination(travel::Destination::Relative {
                        reference: travel::Reference::Celestial(id),
                        offset: GalacticPosition::ZERO,
                        axes: travel::Axes::Galactic,
                    })
                }
            };
            (
                vec![queue_command(ship, travel::GuidanceMode::Align, target, 0.)],
                "Align",
            )
        }
        Intent::Approach(reference, range) | Intent::KeepRange(reference, range) => {
            let row = rows
                .iter()
                .find(|row| row.target == SelectedTarget::Contact(reference))?;
            if row.own {
                return None;
            }
            let mode = if matches!(intent, Intent::KeepRange(..)) {
                travel::GuidanceMode::KeepRange
            } else {
                travel::GuidanceMode::Approach
            };
            (
                vec![queue_command(
                    ship,
                    mode,
                    travel::Target::Contact(reference),
                    range.max(row.radius + 100.),
                )],
                "Approach / keep range",
            )
        }
        Intent::Command(command, label) => (vec![command], label),
        _ => return None,
    };
    Some((commands, label))
}

#[cfg(test)]
mod tests;

fn queue_command(
    ship: &ShipTelemetry,
    mode: travel::GuidanceMode,
    target: travel::Target,
    range_m: f64,
) -> ShipCommand {
    ShipCommand::SetTravel {
        preferences: ship.travel.preferences,
        engage: true,
        expected_revision: ship.travel.revision,
        orders: vec![travel::Order::Guidance(travel::Guidance {
            mode,
            target,
            range_m,
        })],
    }
}
