mod assets;
mod cargo;
mod chat;
mod hangar;
mod industry;
mod instruments;
mod inventory;
mod layout;
mod map;
mod market;
mod model;
mod overview;
mod panels;
mod society;
mod wallet;

use super::{SelectedTarget, Selection, scene};
use crate::state::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use model::*;
use osg_model::industry as industry_model;
use osg_model::*;
use osg_ui::units::distance;
use osg_ui::{bevy_egui::EguiContexts, desktop::*, egui, icons::Icon};

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
const HANGAR: WindowSpec = WindowSpec {
    id: "hangar",
    title: "HANGAR",
    size: egui::vec2(480., 460.),
    min_size: egui::vec2(370., 280.),
    anchor: egui::Align2::LEFT_TOP,
    offset: egui::vec2(530., 145.),
    open: false,
};
const CARGO: WindowSpec = WindowSpec {
    id: "cargo",
    title: "CARGO",
    size: egui::vec2(450., 390.),
    min_size: egui::vec2(370., 280.),
    anchor: egui::Align2::LEFT_TOP,
    offset: egui::vec2(220., 250.),
    open: false,
};
const MAP: WindowSpec = WindowSpec {
    id: "map",
    title: "GATE NETWORK",
    size: egui::vec2(1200., 800.),
    min_size: egui::vec2(480., 350.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const SOCIETY: WindowSpec = WindowSpec {
    id: "society",
    title: "DIRECTORY",
    size: egui::vec2(940., 740.),
    min_size: egui::vec2(680., 420.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const INDUSTRY: WindowSpec = WindowSpec {
    id: "industry",
    title: "INDUSTRY",
    size: egui::vec2(1180., 780.),
    min_size: egui::vec2(580., 400.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};
const CHAT: WindowSpec = WindowSpec {
    id: "local_chat",
    title: "CHAT",
    size: egui::vec2(780., 420.),
    min_size: egui::vec2(360., 260.),
    anchor: egui::Align2::LEFT_BOTTOM,
    offset: egui::Vec2::ZERO,
    open: false,
};
const SETTINGS: WindowSpec = WindowSpec {
    id: "settings",
    title: "SETTINGS",
    size: egui::vec2(1000., 740.),
    min_size: egui::vec2(500., 320.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};

const WALLET: WindowSpec = WindowSpec {
    id: "wallet",
    title: "WALLET",
    size: egui::vec2(1180., 800.),
    min_size: egui::vec2(650., 420.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};

const MARKET: WindowSpec = WindowSpec {
    id: "market",
    title: "MARKET",
    size: egui::vec2(1180., 800.),
    min_size: egui::vec2(650., 420.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};

const ASSETS: WindowSpec = WindowSpec {
    id: "assets",
    title: "ASSETS",
    size: egui::vec2(1180., 800.),
    min_size: egui::vec2(650., 420.),
    anchor: egui::Align2::CENTER_CENTER,
    offset: egui::Vec2::ZERO,
    open: false,
};

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Filter {
    #[default]
    General,
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

pub(super) struct HudObject {
    pub target: SelectedTarget,
    pub contact: Option<ContactRef>,
    pub position: osg_model::GalacticPosition,
    pub name: String,
    pub standing: Option<ownership::Standing>,
    pub visible: bool,
}

#[derive(Resource)]
pub(super) struct Shell {
    hud: Vec<HudObject>,
    desktop: Desktop,
    pending_intents: Vec<Intent>,
    pending_rows: Vec<Row>,
    filter: Filter,
    sort: Sort,
    descending: bool,
    search: String,
    stand_off: f64,
    feedback: Option<Feedback>,
}

impl Shell {
    pub(super) fn hud(&self) -> &[HudObject] {
        &self.hud
    }
}

struct Feedback {
    pending: Vec<Id>,
    label: String,
    error: Option<String>,
}

#[derive(Resource, Default)]
struct CargoInventory(Option<Id>);

#[derive(SystemParam)]
struct PaneStates<'w> {
    inventory: ResMut<'w, inventory::State>,
    industry: ResMut<'w, industry::State>,
    hangar: ResMut<'w, hangar::State>,
    cargo: ResMut<'w, cargo::PaneState>,
    cargo_inventory: ResMut<'w, CargoInventory>,
    transfers: ResMut<'w, cargo::Transfers>,
    chat: ResMut<'w, chat::State>,
    map: ResMut<'w, map::State>,
    society: ResMut<'w, society::State>,
    wallet: ResMut<'w, wallet::State>,
    market: ResMut<'w, market::State>,
    assets: ResMut<'w, assets::State>,
}

fn install_panes(app: &mut App) {
    fn install<T: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(app: &mut App) {
        app.init_resource::<T>().add_observer(reset_resource::<T>);
    }

    install::<inventory::State>(app);
    install::<industry::State>(app);
    install::<hangar::State>(app);
    install::<cargo::PaneState>(app);
    install::<CargoInventory>(app);
    install::<cargo::Transfers>(app);
    install::<chat::State>(app);
    install::<map::State>(app);
    install::<society::State>(app);
    install::<wallet::State>(app);
    install::<market::State>(app);
    install::<assets::State>(app);
}

impl Feedback {
    fn receive(&mut self, results: &[CommandResult]) {
        self.pending.retain(|id| {
            let Some(result) = results.iter().find(|result| result.id == *id) else {
                return true;
            };

            if let Some(error) = &result.error {
                let message = format!("{}: {error}", short_id(result.id));
                if let Some(previous) = &mut self.error {
                    previous.push_str("; ");
                    previous.push_str(&message);
                } else {
                    self.error = Some(message);
                }
            }
            false
        });
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            hud: Vec::new(),
            desktop: Desktop::default(),
            pending_intents: Vec::new(),
            pending_rows: Vec::new(),
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
    Market(osg_model::market::MarketCommand),
    OpenStorage {
        owner: ownership::Principal,
        station: Id,
        item: industry_model::CargoItem,
    },
    Wallet(economy::WalletCommand),
    Chat(String),
    FocusShip(Id),
    InspectInventory(Id),
    OpenHangar,
    OpenAssets,
    Industry(industry_model::IndustryCommand, &'static str),
    Service(industry::service::Action),
    BuildShip(industry::construction::Request),
    RetryNavigation,
    InspectAffiliation(ownership::Principal),
    Society(ownership::SocietyCommand, &'static str),
    Select(SelectedTarget),
    Look(Option<SelectedTarget>),
    Align(SelectedTarget),
    Approach(ContactRef, f64),
    KeepRange(ContactRef, f64),
    Navigate(Vec<travel::Directive>, bool),
    PlanRoute(Vec<travel::Directive>, bool, travel::PlanningPreferences),
    RetryRoute,
    CancelRoute(crate::state::requests::RouteCall),
    CommitRoute,
    EditItinerary(Vec<travel::Directive>),
    Command(ShipCommand, &'static str),
    Orbits(bool),
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ShellDraw;

pub(super) fn install(app: &mut App) {
    install_panes(app);
    app.init_resource::<Shell>()
        .init_resource::<layout::Store>()
        .add_systems(Startup, layout::load)
        .add_systems(Last, layout::save)
        .add_observer(reset_session)
        .add_systems(Update, industry::construction::update)
        .add_systems(
            PostUpdate,
            dispatch.after(osg_ui::bevy_egui::EguiPostUpdateSet::EndPass),
        )
        .add_systems(
            osg_ui::bevy_egui::EguiPrimaryContextPass,
            draw.in_set(ShellDraw).after(super::console::ConsoleDraw),
        );
}

fn reset_session(_: On<SessionReset>, mut shell: ResMut<Shell>) {
    shell.feedback = None;
    shell.pending_intents.clear();
    shell.pending_rows.clear();
}

#[derive(SystemParam)]
struct DomainViews<'w> {
    navigation: Res<'w, NavigationState>,
    society: Res<'w, SocietyState>,
    wallet: Res<'w, WalletState>,
    market: Res<'w, MarketState>,
    assets: Res<'w, AssetsState>,
    industry: Res<'w, IndustryState>,
    chat: Res<'w, ChatState>,
    services: Res<'w, ServiceState>,
    commands: Res<'w, CommandState>,
}

#[derive(SystemParam)]
struct DomainInterests<'w> {
    navigation: Res<'w, NavigationState>,
    society: ResMut<'w, SocietyState>,
    wallet: ResMut<'w, WalletState>,
    market: ResMut<'w, MarketState>,
    assets: ResMut<'w, AssetsState>,
    industry: ResMut<'w, IndustryState>,
    chat: ResMut<'w, ChatState>,
    services: ResMut<'w, ServiceState>,
    commands: Res<'w, CommandState>,
}

fn draw(
    mut contexts: EguiContexts,
    mut shell: ResMut<Shell>,
    mut panes: PaneStates,
    selection: Res<Selection>,
    session: Res<SessionInfo>,
    domains: DomainViews,
    clock: Res<RenderTime>,
    calendar: Res<CalendarClock>,
    real_time: Res<Time<Real>>,
    diagnostics: Res<ClientDiagnostics>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
    optical: Query<&Optical>,
    beacons: Query<(&NavigationObject, &DisplayPose)>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    views: Query<(
        &ViewObservation,
        &ViewSystems,
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
    let mut vicinity = "No celestial reference".to_string();
    let mut primary_luminosity = 0.;
    let mut system_name = "Deep space".to_string();
    let mut rows = Vec::new();
    if let Some((view, systems, _, _)) = view {
        for (contact, pose) in &contacts {
            let observation = &contact.0;
            if Some(contact.1.observer) != view.0.focused_ship {
                continue;
            }
            let own = observation
                .entity
                .is_some_and(|id| Some(id) == selection.ship);
            if own
                || observation
                    .entity
                    .is_some_and(|id| beacons.iter().any(|(b, _)| b.0.id == id))
            {
                continue;
            }
            let name = observation
                .iff
                .as_ref()
                .and_then(|iff| iff.labels.first())
                .cloned()
                .unwrap_or_else(|| format!("Contact {:08x}", observation.id));
            let kind = "Ship".to_owned();
            rows.push(Row {
                celestial: None,
                target: SelectedTarget::Contact(contact.1),
                contact: Some(contact.1),
                name,
                kind,
                offset: pose.0.position.relative_to(origin),
                distance: pose.0.position.relative_to(origin).length(),
                speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
                radius: observation.radius_m,
                own,
                can_look: pose.0.position.relative_to(origin).length() <= scene::LOOK_AT_RANGE_M
                    && optical.iter().any(|object| {
                        object.0.view == view.0.id && object.0.contact == Some(contact.1)
                    }),
                affiliation: super::standing::advertised_principal(observation.iff.as_ref()),
                standing: domains
                    .society
                    .society
                    .directory
                    .contact_standing(domains.society.society.account, observation.iff.as_ref()),
                detail: "Sensor contact".into(),
            });
        }
        for (body, pose, system) in &bodies {
            if !systems.0.iter().any(|reference| *reference == system.0) {
                continue;
            }
            let range = pose.0.position.relative_to(origin).length();
            if telemetry.and_then(|ship| ship.location.primary) == Some(body.0.reference) {
                vicinity = format!(
                    "{}  /  {} altitude",
                    body.0.name,
                    distance((range - body.0.radius_m).max(0.))
                );
            }
            if telemetry.and_then(|ship| ship.location.system) == Some(body.0.reference.system)
                && body.0.luminosity_lumens > primary_luminosity
            {
                primary_luminosity = body.0.luminosity_lumens;
                system_name = body.0.name.clone();
            }

            rows.push(Row {
                celestial: Some(body.0.reference),
                target: SelectedTarget::Celestial(body.0.entity),
                contact: None,
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
            celestial: None,
            target: SelectedTarget::Beacon(beacon.id),
            contact: view.and_then(|(view, ..)| {
                contacts.iter().find_map(|(contact, _)| {
                    (contact.0.entity == Some(beacon.id)
                        && Some(contact.1.observer) == view.0.focused_ship)
                        .then_some(contact.1)
                })
            }),
            name: beacon.name.clone(),
            kind: if beacon.navigation {
                "Navigation beacon"
            } else {
                "Directory transmitter"
            }
            .into(),
            offset,
            distance: offset.length(),
            speed: (glam::DVec3::from_array(pose.0.velocity) - velocity).length(),
            radius: beacon.radius_m,
            can_look: offset.length() <= scene::LOOK_AT_RANGE_M
                && optical.iter().any(|object| {
                    Some(object.0.view) == selection.view
                        && object.0.known_entity == Some(beacon.id)
                }),
            affiliation: contacts
                .iter()
                .find(|(contact, _)| contact.0.entity == Some(beacon.id))
                .and_then(|(contact, _)| {
                    super::standing::advertised_principal(contact.0.iff.as_ref())
                }),
            standing: contacts
                .iter()
                .find(|(contact, _)| contact.0.entity == Some(beacon.id))
                .and_then(|(contact, _)| {
                    domains
                        .society
                        .society
                        .directory
                        .contact_standing(domains.society.society.account, contact.0.iff.as_ref())
                }),
            detail: "Subspace beacon".into(),
            own: false,
        });
    }
    let model = FrameModel {
        declaration_history: &domains.society.declaration_history,
        declaration_history_next: domains.society.declaration_history_next,
        declaration_history_key: domains.society.declaration_history_key,
        services: &domains.services.services,
        industry: &domains.industry.snapshot,
        industry_ready: domains.industry.ready(),
        navigation: &domains.navigation.navigation,
        inhabited: domains.navigation.inhabited.clone(),
        navigation_status: &domains.navigation.navigation_status,
        navigation_hash: domains.navigation.navigation_hash,
        society: &domains.society.society,
        rows,
        ship: telemetry,
        details,
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
    panes.chat.receive(&domains.commands.results);
    let loading = crate::state::requests::Loading {
        society: domains.society.load.loading(),
        wallet: domains.wallet.load.loading(),
        market: domains.market.load.loading(),
        assets: domains.assets.load.loading(),
        industry: domains.industry.load.loading(),
        services: domains.services.load.loading(),
    };
    let navigation_loading = matches!(
        domains.navigation.navigation_status,
        NavigationStatus::Loading
    );
    for (window, active) in [
        (SOCIETY, loading.society),
        (WALLET, loading.wallet),
        (MARKET, loading.market || loading.industry),
        (ASSETS, loading.assets),
        (INDUSTRY, loading.industry || loading.services),
        (INVENTORY, loading.industry),
        (HANGAR, loading.industry),
        (CARGO, loading.industry),
        (MAP, navigation_loading),
        (NAVIGATION, navigation_loading),
        (CHAT, domains.chat.loading()),
    ] {
        shell.desktop.set_loading(window, active);
    }
    panels::draw(
        ctx,
        &mut shell,
        &mut panes,
        &model,
        &selection,
        &domains.commands.results,
        &domains.chat,
        domains.wallet.wallet.as_ref(),
        domains.market.market.as_ref(),
        domains.assets.assets.as_ref(),
        &mut intents,
    );
    shell.hud = model
        .rows
        .iter()
        .map(|row| HudObject {
            target: row.target,
            contact: row.contact,
            position: origin.offset_by(row.offset),
            name: row.name.clone(),
            standing: row.standing,
            visible: row_visible(row, &shell, selection.target),
        })
        .collect();
    shell.pending_rows = model.rows;
    if shell.pending_intents.is_empty() {
        shell.pending_intents = intents;
    }
    Ok(())
}

fn dispatch(
    mut shell: ResMut<Shell>,
    mut panes: PaneStates,
    mut selection: ResMut<Selection>,
    session: Res<SessionInfo>,
    mut domains: DomainInterests,
    mut outgoing: ResMut<Outgoing>,
    mut requests: ResMut<crate::state::requests::Mutations>,
    mut routes: ResMut<crate::state::requests::Routes>,
    client: Res<crate::state::requests::NetworkClient>,
    real_time: Res<Time<Real>>,
    asset_server: Res<AssetServer>,
    ships: Query<&OwnedShip>,
    mut views: Query<(
        &ViewObservation,
        &ViewSystems,
        &mut scene::CameraOptions,
        &mut scene::ViewOptions,
    )>,
) {
    struct DispatchModel<'a> {
        connected: bool,
        rows: Vec<Row>,
        ships: Vec<&'a ShipTelemetry>,
        industry: &'a industry_model::IndustrySnapshot,
    }
    let model = DispatchModel {
        connected: session.world.is_some() && session.status.is_empty(),
        rows: std::mem::take(&mut shell.pending_rows),
        ships: ships.iter().map(|ship| &ship.0).collect(),
        industry: &domains.industry.snapshot,
    };
    let telemetry = ships
        .iter()
        .find(|ship| Some(ship.0.ship) == selection.ship)
        .map(|ship| &ship.0);
    panes.map.route.update(
        telemetry.filter(|_| model.connected),
        &domains.commands.results,
        &mut routes,
        real_time.elapsed(),
    );
    for intent in std::mem::take(&mut shell.pending_intents) {
        match intent {
            Intent::OpenStorage {
                owner,
                station,
                item,
            } => {
                panes.market.open_storage(owner, station, item);
                shell.desktop.open(MARKET);
            }
            Intent::Market(command) => {
                if model.connected {
                    let id = crate::state::requests::mutations::market(
                        &mut requests,
                        &client.0,
                        session.world.unwrap(),
                        session.generation,
                        command,
                    );
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: "Market order".into(),
                        error: None,
                    });
                }
            }
            Intent::Wallet(command) => {
                if model.connected {
                    let label = match &command {
                        economy::WalletCommand::SetTurnoverTax { .. } => "Turnover tax rate",
                        _ => "Transfer",
                    };
                    let id = crate::state::requests::mutations::wallet(
                        &mut requests,
                        &client.0,
                        session.world.unwrap(),
                        session.generation,
                        command,
                    );
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: label.into(),
                        error: None,
                    });
                }
            }
            Intent::Chat(text) => {
                if let Some(id) = domains.chat.transmit(text.clone(), &mut outgoing) {
                    panes.chat.sent(id, text);
                }
            }
            Intent::FocusShip(ship) => {
                if model.ships.iter().any(|owned| owned.ship == ship)
                    || model.industry.hangar.as_ref().is_some_and(|hangar| {
                        hangar
                            .ships
                            .iter()
                            .any(|entry| entry.inventory.entity == ship && entry.can_focus)
                    })
                {
                    selection.ship = Some(ship);
                    selection.target = None;
                    panes.inventory.focus();
                    for (view, _, mut camera, _) in &mut views {
                        if Some(view.0.id) == selection.view {
                            camera.focus = None;
                        }
                    }
                }
            }
            Intent::InspectInventory(entity) => {
                if selection.ship == Some(entity) {
                    shell.desktop.open(INVENTORY);
                } else {
                    panes.cargo_inventory.0 = Some(entity);
                    *panes.cargo = cargo::PaneState::default();
                    shell.desktop.open(CARGO);
                }
            }
            Intent::OpenHangar => shell.desktop.open(HANGAR),
            Intent::OpenAssets => shell.desktop.open(ASSETS),
            Intent::BuildShip(request) => {
                if let Some(world) = session.world.filter(|_| model.connected) {
                    panes
                        .industry
                        .construction
                        .queue(request, (world, session.generation));
                }
            }
            Intent::Industry(command, label) => {
                if model.connected {
                    let id = crate::state::requests::mutations::industry(
                        &mut requests,
                        &client.0,
                        session.world.unwrap(),
                        session.generation,
                        command,
                    );
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: label.into(),
                        error: None,
                    });
                }
            }
            Intent::Service(action) => {
                if model.connected {
                    let id = Id::new();
                    let operation = osg_model::rpc::Operation {
                        world: session.world.unwrap(),
                        id,
                    };
                    let net = client.0.clone();
                    requests.submit(operation.world, session.generation, id, async move {
                        industry::service::submit(net, operation, action).await
                    });
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: "Industry service".into(),
                        error: None,
                    });
                }
            }
            Intent::RetryNavigation => {
                if let Some(hash) = domains.navigation.navigation_hash {
                    asset_server.reload(crate::assets::path(hash));
                }
            }
            Intent::InspectAffiliation(principal) => {
                panes.society.inspect(principal);
                shell.desktop.open(SOCIETY);
            }
            Intent::Society(command, label) => {
                if model.connected {
                    let id = crate::state::requests::mutations::society(
                        &mut requests,
                        &client.0,
                        session.world.unwrap(),
                        session.generation,
                        command,
                    );
                    if let Some(feedback) = shell
                        .feedback
                        .as_mut()
                        .filter(|feedback| feedback.label == label && !feedback.pending.is_empty())
                    {
                        feedback.pending.push(id);
                    } else {
                        shell.feedback = Some(Feedback {
                            pending: vec![id],
                            label: label.into(),
                            error: None,
                        });
                    }
                }
            }
            Intent::EditItinerary(orders) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    outgoing.ship(
                        ship,
                        ShipCommand::SetItinerary {
                            preferences: ship.travel.preferences,
                            engage: false,
                            expected_revision: ship.travel.directive_revision,
                            itinerary: orders,
                        },
                    );
                }
            }
            Intent::PlanRoute(orders, append, preferences) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    panes
                        .map
                        .route
                        .begin(ship, orders, append, preferences, &mut routes);
                }
            }
            Intent::RetryRoute => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    panes.map.route.retry(ship, &mut routes);
                }
            }
            Intent::CancelRoute(action) => {
                routes.route(action);
            }
            Intent::CommitRoute => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    if let Some(command) = panes.map.route.commit(ship) {
                        let id = outgoing.ship(ship, command);
                        panes.map.route.sent_commit(id);
                        shell.feedback = Some(Feedback {
                            pending: vec![id],
                            label: "Engage planned route".into(),
                            error: None,
                        });
                    }
                }
            }
            Intent::Navigate(orders, append) => {
                if let Some(ship) = telemetry.filter(|_| model.connected) {
                    let mut queue = if append {
                        ship.travel
                            .itinerary
                            .iter()
                            .map(|stage| stage.directive.clone())
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
                        ShipCommand::SetItinerary {
                            preferences: ship.travel.preferences,
                            engage: true,
                            expected_revision: ship.travel.directive_revision,
                            itinerary: queue,
                        },
                    );
                    shell.feedback = Some(Feedback {
                        pending: vec![id],
                        label: "Autopilot itinerary".into(),
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
                            error: None,
                        });
                    }
                }
            }
        }
    }
    let wanted = inventory_subscription(
        &shell,
        &panes,
        selection.ship,
        model.industry.hangar.as_ref(),
        model.connected,
    );
    drop(model);
    let society_open = shell.desktop.is_open(SOCIETY);
    domains.society.interest = panes.society.request(society_open, &domains.society);
    let wallet_open =
        shell.desktop.is_open(WALLET) && session.world.is_some() && session.status.is_empty();
    let account = domains.society.society.account;
    domains.services.interest = panes
        .industry
        .service
        .query(shell.desktop.is_open(INDUSTRY), account);
    domains.wallet.interest = panes.wallet.query(wallet_open, account);
    let market_open =
        shell.desktop.is_open(MARKET) && session.world.is_some() && session.status.is_empty();
    domains.market.interest = panes.market.query(market_open, account);
    let assets_open =
        shell.desktop.is_open(ASSETS) && session.world.is_some() && session.status.is_empty();
    domains.assets.interest = panes.assets.query(assets_open);
    domains.industry.subscribe(wanted);

    let focus = (shell.desktop.is_open(CHAT) && session.status.is_empty())
        .then(|| {
            views.iter().find_map(|(view, _, _, _)| {
                let ship = selection.ship?;
                (Some(view.0.id) == selection.view && view.0.focused_ship == Some(ship)).then_some(
                    ChatFocus {
                        view: view.0.id,
                        view_revision: view.0.revision,
                        ship,
                    },
                )
            })
        })
        .flatten();
    domains.chat.subscribe(focus, &mut outgoing);
}

fn inventory_subscription(
    shell: &Shell,
    panes: &PaneStates,
    focused: Option<Id>,
    hangar: Option<&industry_model::HangarView>,
    connected: bool,
) -> Option<industry_model::IndustryQuery> {
    let mut interest = industry_model::IndustryQuery::default();
    if shell.desktop.is_open(MARKET) {
        interest.directory = true;
        interest.catalogue = true;
    }
    if shell.desktop.is_open(INDUSTRY) {
        interest.directory = true;
        interest.catalogue = true;
        interest.directory_after = panes.industry.directory_after;
        interest.inventories.extend(panes.industry.facility);
    }
    if shell.desktop.is_open(INVENTORY) {
        interest.inventories.extend(focused);
    }
    if shell.desktop.is_open(HANGAR) {
        interest.hangar = focused.map(|ship| industry_model::HangarQuery {
            ship,
            after: panes.hangar.after,
        });
        if let Some(hangar) = hangar.filter(|view| Some(view.ship) == focused) {
            interest
                .inventories
                .extend(hangar.host_inventory.as_ref().map(|host| host.entity));
        }
    }
    if shell.desktop.is_open(CARGO) {
        interest.inventories.extend(panes.cargo_inventory.0);
    }
    interest.inventories.extend(panes.transfers.inventories());
    let wanted =
        interest.directory || interest.hangar.is_some() || !interest.inventories.is_empty();
    (connected && wanted).then_some(interest)
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
                SelectedTarget::Celestial(_) => {
                    travel::Target::Destination(travel::Destination::Relative {
                        reference: travel::Reference::Celestial(row.celestial?),
                        offset: GalacticPosition::ZERO,
                        axes: travel::Axes::Galactic,
                    })
                }
            };
            (
                vec![guidance_command(travel::GuidanceMode::Align, target, 0.)],
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
                vec![guidance_command(
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

fn guidance_command(
    mode: travel::GuidanceMode,
    target: travel::Target,
    range_m: f64,
) -> ShipCommand {
    ShipCommand::SetGuidance(Some(travel::Guidance {
        mode,
        target,
        range_m,
    }))
}
