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
pub(super) struct Optical(pub optical::OpticalObservation);

#[derive(Component)]
pub(super) struct OpticalLight {
    previous: f64,
    current: f64,
    pub display_w: f64,
}

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
pub(super) struct DisplayVisual(pub ShipVisual);

#[derive(Component)]
pub(super) struct CombatPublication(pub CombatEvent);

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
    pub navigation: std::sync::Arc<NavigationCatalogue>,
    pub navigation_hash: Option<[u8; 32]>,
    pub navigation_status: NavigationStatus,
    pub navigation_ephemerides: Vec<CelestialSystemRef>,
    pub society: ownership::SocietySnapshot,
    pub industry: IndustryState,
    pub results: Vec<CommandResult>,
    pub events: Vec<toy_sim_model::Event>,
    pub target_frames: usize,
    pub underruns: u64,
    pub status: String,
}

#[derive(Default)]
pub(super) struct IndustryState {
    pub snapshot: industry::IndustrySnapshot,
    subscription: Option<industry::IndustrySubscription>,
    revision: u64,
}

impl IndustryState {
    pub fn subscribe(
        &mut self,
        mut wanted: Option<industry::IndustrySubscription>,
        outgoing: &mut Outgoing,
    ) {
        if let Some(wanted) = &mut wanted {
            let mut seen = std::collections::BTreeSet::new();
            wanted
                .inventories
                .retain(|inventory| seen.insert(*inventory));
            wanted.revision = self.revision;
        }
        if wanted == self.subscription {
            return;
        }
        if let Some(mut wanted) = wanted {
            self.revision = self
                .revision
                .checked_add(1)
                .expect("industry revision exhausted");
            wanted.revision = self.revision;
            outgoing.push(Action::IndustrySubscribe(wanted.clone()));
            self.subscription = Some(wanted);
        } else {
            outgoing.push(Action::IndustryUnsubscribe);
            self.subscription = None;
            self.snapshot = Default::default();
        }
    }

    fn apply(&mut self, mut snapshot: industry::IndustrySnapshot) {
        if self
            .subscription
            .as_ref()
            .is_none_or(|subscription| subscription.revision != snapshot.subscription_revision)
        {
            return;
        }
        if snapshot.catalogue.is_none() {
            snapshot.catalogue = self.snapshot.catalogue.take();
        }
        self.snapshot = snapshot;
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) enum NavigationStatus {
    #[default]
    Unavailable,
    Loading,
    Ready,
    Failed(String),
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
    optical: BTreeMap<(u64, Id), Entity>,
    beacons: BTreeMap<Id, Entity>,
    views: BTreeMap<u64, Entity>,
    events: BTreeMap<u64, (Entity, u64)>,
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
