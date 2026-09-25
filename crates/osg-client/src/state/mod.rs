pub use requests::views::*;
#[derive(bevy::prelude::Resource, Default)]
#[cfg(feature = "ui")]
pub struct SlipEffects(pub osg_model::slip_visual::SlipPresentation);

mod calendar;
mod chat;
pub use calendar::CalendarClock;
pub use chat::{ChatFocus, ChatState};
mod diagnostics;
mod domains;
pub use diagnostics::ClientDiagnostics;
pub use domains::*;
mod presentation;
pub use crate::Outgoing;
mod lifecycle;
mod query;
mod replication;
pub mod requests;
mod transport;
#[cfg(test)]
pub use lifecycle::render_session_status;
pub use lifecycle::{
    Bootstrap, ClientPhase, ClientSystems, GameSession, SessionFailure, SessionInstalled,
    SessionKey,
};
pub use query::{QueryState, render_query};

use crate::{EventSubscription, OsgNetClient, playback::Playback};
use bevy::prelude::*;
use osg_model::*;
use presentation::interpolate;
use replication::apply;
use std::collections::BTreeMap;
pub use transport::SessionEvents;
use transport::{receive_session_events, send};

#[derive(Component)]
pub struct WorldMember;

#[derive(Component)]
pub struct Contact(pub SensorObservation, pub ContactRef);

#[derive(Component)]
pub struct Optical(pub optical::OpticalObservation);

#[derive(Component, Default)]
pub struct OpticalLight {
    previous: f64,
    current: f64,
    pub display_w: f64,
}

#[derive(Component)]
pub struct OwnedShip(pub ShipTelemetry);

#[derive(Component)]
pub struct ShipDetails(pub ShipPresentation);

#[derive(Component)]
pub struct NavigationObject(pub NavigationBeacon);

#[derive(Component)]
pub struct ViewObservation(pub ViewState);

#[derive(Component, Default)]
pub struct ViewSystems(pub Vec<Id>);

#[derive(Component)]
pub struct Celestial(pub CelestialPresentation);

#[derive(Component)]
pub struct CelestialSystem(pub Id);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct SpatialInstance(pub Id);

#[derive(Component)]
pub struct DisplayPose(pub Pose);

#[derive(Component)]
pub struct DisplayVisual(pub ShipVisual);

#[derive(Component)]
pub struct CombatPublication(pub CombatEvent);

#[derive(Component)]
struct PoseSamples {
    previous: Pose,
    current: Pose,
}

#[derive(Component)]
struct VisualSamples {
    previous: ShipVisual,
    current: ShipVisual,
}

#[derive(Resource, Default)]
pub struct RenderTime {
    pub previous_ns: u64,
    pub current_ns: u64,
    pub display_ns: u64,
}

#[derive(Resource, Default)]
pub struct SessionInfo {
    pub tick: u64,
    pub sequence: u64,
    pub capabilities: Vec<DebugCapability>,
    pub status: String,
}

pub use requests::inventory::IndustryState;

#[derive(Default, Debug, PartialEq, Eq)]
pub enum NavigationStatus {
    #[default]
    Unavailable,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Event)]
pub struct SessionReset;

pub fn reset_resource<T: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(
    _: On<SessionReset>,
    mut value: ResMut<T>,
) {
    *value = T::default();
}

#[derive(Resource)]
struct Transport {
    client: OsgNetClient,
    events: EventSubscription,
    failed: bool,
}

#[derive(Resource)]
struct BufferedPlayback(Playback);

#[derive(Resource, Default)]
pub struct ScreenFrames(pub BTreeMap<(Id, u8), ScreenUpdate>);

#[derive(Resource, Default)]
struct Replication {
    applied: Option<SessionKey>,
    contacts: BTreeMap<(Id, u64), Entity>,
    ships: BTreeMap<Id, Entity>,
    optical: BTreeMap<(u64, Id), Entity>,
    beacons: BTreeMap<Id, Entity>,
    views: BTreeMap<u64, Entity>,
    events: BTreeMap<u64, (Entity, u64)>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationSet {
    Interpolate,
    Views,
    Render,
}

pub fn install(app: &mut App, client: OsgNetClient, local: bool) {
    lifecycle::install(app);
    app.insert_resource(requests::NetworkClient(client.clone()));
    requests::install(app);
    app.insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION))
        .insert_resource(Transport {
            events: client.subscribe_events(),
            client,
            failed: false,
        })
        .insert_resource(BufferedPlayback(Playback::new(local)))
        .init_resource::<ClientDiagnostics>()
        .add_systems(Update, diagnostics::update)
        .init_resource::<Replication>()
        .init_resource::<RenderTime>()
        .init_resource::<SlipEffects>()
        .add_observer(reset_resource::<SlipEffects>)
        .init_resource::<Bootstrap>()
        .init_resource::<SessionEvents>()
        .init_resource::<SessionInfo>()
        .init_resource::<NavigationState>()
        .init_resource::<SocietyState>()
        .init_resource::<WalletState>()
        .init_resource::<MarketState>()
        .init_resource::<AssetsState>()
        .init_resource::<IndustryState>()
        .init_resource::<ServiceState>()
        .init_resource::<ChatState>()
        .init_resource::<CommandState>()
        .init_resource::<PlaybackState>()
        .init_resource::<ScreenFrames>()
        .add_observer(reset_resource::<ScreenFrames>)
        .add_observer(reset_resource::<NavigationState>)
        .add_observer(reset_resource::<SocietyState>)
        .add_observer(reset_resource::<WalletState>)
        .add_observer(reset_resource::<MarketState>)
        .add_observer(reset_resource::<AssetsState>)
        .add_observer(reset_resource::<IndustryState>)
        .add_observer(reset_resource::<ServiceState>)
        .add_observer(reset_resource::<ChatState>)
        .add_observer(reset_resource::<CommandState>)
        .add_observer(reset_resource::<PlaybackState>)
        .init_resource::<Outgoing>()
        .add_observer(reset_resource::<Outgoing>)
        .configure_sets(
            Update,
            (
                PresentationSet::Interpolate,
                PresentationSet::Views,
                PresentationSet::Render,
            )
                .chain(),
        )
        .add_systems(
            PreUpdate,
            transport::receive_network_events.before(receive_session_events),
        )
        .add_systems(OnEnter(ClientPhase::Loading), lifecycle::start_bootstrap)
        .add_systems(
            osg_ui::bevy_egui::EguiPrimaryContextPass,
            lifecycle::draw_session_status,
        )
        .add_systems(Update, interpolate.in_set(PresentationSet::Interpolate))
        .add_systems(FixedPostUpdate, send.in_set(ClientSystems::Gameplay));
}
