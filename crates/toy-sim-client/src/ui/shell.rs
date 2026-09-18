mod instruments;
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
    size: egui::vec2(390., 204.),
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
    offset: egui::vec2(0., 218.),
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
            if own {
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
    let model = FrameModel {
        rows,
        ship: telemetry,
        details,
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
            Intent::Select(target) => {
                selection.target = Some(target);
                shell.desktop.open(SELECTED);
            }
            Intent::Look(target) => {
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
            let direction = row.offset.try_normalize()?;
            (
                vec![ShipCommand::Flight(FlightCommand::AimDirection(
                    direction.to_array(),
                ))],
                "Align",
            )
        }
        Intent::Approach(reference, stand_off) => {
            let row = rows
                .iter()
                .find(|row| row.target == SelectedTarget::Contact(reference))?;
            if row.own {
                return None;
            }
            (
                vec![
                    ShipCommand::Flight(FlightCommand::SelectTarget(reference)),
                    ShipCommand::Flight(FlightCommand::EngageNavigation {
                        throttle_limit: 1.,
                        stand_off_m: stand_off.max(row.radius + 100.),
                    }),
                ],
                "Approach",
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
                vec![ShipCommand::EngageWeapons {
                    group: reference.group,
                    track: reference.track,
                    maximum_flight_time_s: 30.,
                }],
                "Engage weapons",
            )
        }
        Intent::Command(command, label) => (vec![command], label),
        _ => return None,
    };
    Some((commands, label))
}

#[cfg(test)]
mod tests;
