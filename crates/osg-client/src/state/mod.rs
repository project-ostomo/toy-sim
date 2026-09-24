#[derive(bevy::prelude::Resource, Default)]
#[cfg(feature = "ui")]
pub(crate) struct SlipEffects(pub osg_model::slip_visual::SlipPresentation);

mod calendar;
mod chat;
pub(super) use calendar::CalendarClock;
pub(crate) use chat::{ChatFocus, ChatState};
mod diagnostics;
mod domains;
pub(super) use diagnostics::ClientDiagnostics;
pub(super) use domains::*;
mod presentation;
pub(crate) use crate::Outgoing;
mod replication;
pub(crate) mod requests;
mod transport;

use crate::{EventSubscription, OsgNetClient, playback::Playback};
use bevy::prelude::*;
use osg_model::*;
use presentation::interpolate;
use replication::apply;
use std::collections::BTreeMap;
use transport::{receive, send};

#[derive(Component)]
pub(super) struct WorldMember;

#[derive(Component)]
pub(super) struct Contact(pub SensorObservation, pub ContactRef);

#[derive(Component)]
pub(super) struct Optical(pub optical::OpticalObservation);

#[derive(Component, Default)]
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

#[derive(Component, Default)]
pub(super) struct ViewSystems(pub Vec<Id>);

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
    pub status: String,
}

#[derive(Resource, Default)]
pub(super) struct IndustryState {
    pub snapshot: industry::IndustrySnapshot,
    pub interest: Option<industry::IndustryQuery>,
    pub(crate) load: requests::Load<industry::IndustryQuery, industry::IndustrySnapshot>,
    revision: u64,
}

impl IndustryState {
    pub fn ready(&self) -> bool {
        self.interest.as_ref().is_some_and(|subscription| {
            subscription.revision == self.snapshot.subscription_revision
        })
    }

    pub fn subscribe(&mut self, mut wanted: Option<industry::IndustryQuery>) {
        if let Some(wanted) = &mut wanted {
            let mut seen = std::collections::BTreeSet::new();
            wanted
                .inventories
                .retain(|inventory| seen.insert(*inventory));
            wanted.revision = self.revision;
        }
        if wanted == self.interest {
            return;
        }
        if let Some(mut wanted) = wanted {
            self.revision = self
                .revision
                .checked_add(1)
                .expect("industry revision exhausted");
            wanted.revision = self.revision;
            self.interest = Some(wanted);
        } else {
            self.interest = None;
            self.snapshot = Default::default();
        }
    }

    fn apply(&mut self, mut snapshot: industry::IndustrySnapshot) {
        if self
            .interest
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
    client: OsgNetClient,
    events: EventSubscription,
    failed: bool,
}

#[derive(Resource)]
struct BufferedPlayback(Playback);

#[derive(Resource, Default)]
struct Replication {
    contacts: BTreeMap<(Id, u64), Entity>,
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

pub(super) fn install(app: &mut App, client: OsgNetClient, local: bool) {
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
        .init_resource::<CalendarClock>()
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
        .add_observer(domains::reset_navigation)
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
        .add_systems(PreUpdate, receive)
        .add_systems(FixedUpdate, (replication::reset, apply).chain())
        .add_systems(Update, interpolate.in_set(PresentationSet::Interpolate))
        .add_systems(FixedPostUpdate, send);
}
