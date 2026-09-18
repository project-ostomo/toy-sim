mod instruments;
mod inventory;
mod map;
mod model;
mod overview;
mod panels;

use super::{SelectedTarget, Selection, scene};
use crate::state::*;
use bevy::prelude::*;
use model::*;
use toy_sim_model::*;
use toy_sim_ui::{bevy_egui::EguiContexts, desktop::*, egui, icons::Icon};

const SELECTED: WindowSpec = WindowSpec {
    id: "selected",
    title: "SELECTED ITEM",
    size: egui::vec2(390., 244.),
    min_size: egui::vec2(350., 140.),
    anchor: egui::Align2::RIGHT_TOP,
    offset: egui::Vec2::ZERO,
    open: true,
};
const OVERVIEW: WindowSpec = WindowSpec {
    id: "overview",
    title: "OVERVIEW",
    size: egui::vec2(390., 380.),
    min_size: egui::vec2(350., 230.),
    anchor: egui::Align2::RIGHT_TOP,
    offset: egui::vec2(0., 258.),
    open: true,
};
const SHIP: WindowSpec = WindowSpec {
    id: "ship",
    title: "SHIP STATUS",
    size: egui::vec2(320., 390.),
    min_size: egui::vec2(280., 260.),
    anchor: egui::Align2::LEFT_TOP,
    offset: egui::vec2(0., 145.),
    open: false,
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
    map: map::State,
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
            map: map::State::default(),
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
    Select(SelectedTarget),
    Look(Option<SelectedTarget>),
    Align(SelectedTarget),
    Approach(ContactRef, f64),
    KeepRange(ContactRef, f64),
    Queue(Vec<travel::Order>, bool),
    Engage(ContactRef),
    Command(ShipCommand, &'static str),
    Orbits(bool),
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Shell>()
        .add_observer(reset_session)
        .add_systems(toy_sim_ui::bevy_egui::EguiPrimaryContextPass, draw);
}

fn reset_session(_: On<SessionReset>, mut shell: ResMut<Shell>) {
    shell.feedback = None;
}

fn draw(
    mut contexts: EguiContexts,
    mut shell: ResMut<Shell>,
    mut selection: ResMut<Selection>,
    mut outgoing: ResMut<Outgoing>,
    session: Res<SessionInfo>,
    clock: Res<RenderTime>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
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
                    .is_some_and(|id| session.navigation.beacons.iter().any(|b| b.id == id))
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
            let kind = track
                .tags
                .iter()
                .find_map(|tag| match tag {
                    Tag::Kind(kind) => Some(kind.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "Contact".into());
            rows.push(Row {
                target: SelectedTarget::Contact(contact.1),
                name,
                kind,
                offset: pose.0.position.relative_to(origin),
                distance: pose.0.position.relative_to(origin).length(),
                speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
                radius: track.radius_m.unwrap_or(0.),
                own,
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
                detail: "Orrery ephemeris".into(),
                own: false,
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
            detail: "Subspace beacon".into(),
            own: false,
        });
    }
    let model = FrameModel {
        navigation: &session.navigation,
        rows,
        ship: telemetry,
        details,
        ships: ships.iter().map(|(ship, _, _)| &ship.0).collect(),
        system: system_name,
        vicinity,
        connected: session.world.is_some() && session.status.is_empty(),
        status: &session.status,
        time_ns: clock.display_ns,
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
        match intent {
            Intent::Queue(orders, append) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    let mut queue = if append {
                        ship.travel
                            .orders
                            .iter()
                            .skip(ship.travel.order)
                            .cloned()
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
                if let Some(SelectedTarget::Contact(_)) = target {
                    if !model.rows.iter().any(|row| {
                        Some(row.target) == target && row.distance <= scene::LOOK_AT_RANGE_M
                    }) {
                        shell.feedback = Some(Feedback {
                            pending: vec![],
                            label: "Look at".into(),
                            last_tick: session.tick,
                            error: Some("Ship is outside camera range (100 km)".into()),
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
        Intent::Engage(reference) => {
            let row = rows
                .iter()
                .find(|row| row.target == SelectedTarget::Contact(reference))?;
            if row.own {
                return None;
            }
            (
                vec![queue_command(
                    ship,
                    travel::GuidanceMode::Engage,
                    travel::Target::Contact(reference),
                    1000_f64.max(row.radius + 100.),
                )],
                "Engage",
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
        expected_revision: ship.travel.revision,
        orders: vec![travel::Order::Guidance(travel::Guidance {
            mode,
            target,
            range_m,
        })],
    }
}
