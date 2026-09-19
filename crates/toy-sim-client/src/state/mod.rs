mod calendar;
pub(super) use calendar::CalendarClock;
mod commands;
mod diagnostics;
pub(super) use diagnostics::ClientDiagnostics;
mod presentation;
pub(crate) use commands::Outgoing;
mod replication;
mod transport;

use crate::{Endpoint, playback::Playback};
use bevy::prelude::*;
use presentation::interpolate;
use replication::apply;
use std::collections::BTreeMap;
use toy_sim_model::*;
use transport::{receive, send};

#[derive(Component)]
pub(super) struct WorldMember;

#[derive(Component)]
pub(super) struct Contact(pub Track, pub ContactRef);

#[derive(Component)]
pub(super) struct OwnedShip(pub ShipTelemetry);

#[derive(Component)]
pub(super) struct ShipDetails(pub ShipPresentation);

#[derive(Component)]
pub(super) struct NavigationObject(pub NavigationBeacon);

#[derive(Component)]
pub(super) struct ViewObservation(pub ViewState);

#[derive(Component)]
pub(super) struct SystemSubscription(pub Vec<CelestialSystemRef>);

#[derive(Component)]
pub(super) struct Celestial(pub CelestialPresentation);

#[derive(Component)]
pub(super) struct CelestialSystem(pub Id);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct SpatialInstance(pub Id);

#[derive(Component)]
pub(super) struct DisplayPose(pub Pose);

#[derive(Component)]
pub(super) struct DisplayVisual(pub TrackVisual);

#[derive(Component)]
pub(super) struct CombatPublication(pub CombatEvent);

#[derive(Component)]
pub(super) struct DestroyedAt(pub u64);

#[derive(Component)]
struct PoseSamples {
    previous: Pose,
    current: Pose,
}

#[derive(Component)]
struct VisualSamples {
    previous: TrackVisual,
    current: TrackVisual,
}

#[derive(Resource, Default)]
pub(super) struct RenderTime {
    pub previous_ns: u64,
    pub current_ns: u64,
    pub display_ns: u64,
}

#[derive(Resource, Default)]
pub(super) struct SessionInfo {
    pub world: Option<Id>,
    pub generation: u64,
    pub tick: u64,
    pub sequence: u64,
    pub capabilities: Vec<DebugCapability>,
    pub groups: Vec<GroupId>,
    pub diagnostics: Option<Diagnostics>,
    pub universe: Option<UniverseStatus>,
    pub navigation: NavigationCatalogue,
    pub society: ownership::SocietySnapshot,
    pub results: Vec<CommandResult>,
    pub events: Vec<toy_sim_model::Event>,
    pub target_frames: usize,
    pub underruns: u64,
    pub status: String,
}

#[derive(Event)]
pub(crate) struct SessionReset;

pub(crate) fn reset_resource<T: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(
    _: On<SessionReset>,
    mut value: ResMut<T>,
) {
    *value = T::default();
}

#[derive(Resource)]
struct Transport {
    endpoint: Endpoint,
    input_sequence: u64,
}

#[derive(Resource)]
struct BufferedPlayback(Playback);

#[derive(Resource, Default)]
struct Replication {
    contacts: BTreeMap<(Id, Id), Entity>,
    ships: BTreeMap<Id, Entity>,
    beacons: BTreeMap<Id, Entity>,
    views: BTreeMap<u64, Entity>,
    events: BTreeMap<u64, (Entity, u64)>,
    deaths: BTreeMap<(Id, Id, Id), u64>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PresentationSet {
    Interpolate,
    Views,
    Render,
}

pub(super) fn install(app: &mut App, endpoint: Endpoint, local: bool) {
    app.insert_resource(Time::<Fixed>::from_hz(10.))
        .insert_resource(Transport {
            endpoint,
            input_sequence: 0,
        })
        .insert_resource(BufferedPlayback(Playback::new(local)))
        .init_resource::<ClientDiagnostics>()
        .add_systems(Update, diagnostics::update)
        .init_resource::<Replication>()
        .init_resource::<RenderTime>()
        .init_resource::<CalendarClock>()
        .init_resource::<SessionInfo>()
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
        .add_systems(PreUpdate, receive)
        .add_systems(FixedUpdate, (replication::reset, apply).chain())
        .add_systems(Update, interpolate.in_set(PresentationSet::Interpolate))
        .add_systems(FixedPostUpdate, send);
}
