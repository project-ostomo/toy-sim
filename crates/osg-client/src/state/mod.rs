#[derive(bevy::prelude::Resource, Default)]
#[cfg(feature = "ui")]
pub(crate) struct SlipEffects(pub osg_model::slip_visual::SlipPresentation);

mod calendar;
mod chat;
pub(super) use calendar::CalendarClock;
pub(crate) use chat::{ChatFocus, ChatState};
mod diagnostics;
pub(super) use diagnostics::ClientDiagnostics;
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
    pub diagnostics: Option<Diagnostics>,
    pub universe_descriptor: Option<UniverseDescriptor>,
    pub inhabited: std::sync::Arc<InhabitedDirectory>,
    pub navigation: std::sync::Arc<NavigationCatalogue>,
    pub navigation_hash: Option<[u8; 32]>,
    pub navigation_status: NavigationStatus,
    pub society: ownership::SocietySnapshot,
    pub directory_entries: Vec<ownership::Principal>,
    pub directory_next: Option<ownership::Principal>,
    pub society_assets_next: Option<Id>,
    pub society_asset_loaded: Option<Id>,
    pub declaration_history: Vec<osg_model::diplomacy::Declaration>,
    pub declaration_history_next: Option<u64>,
    pub declaration_history_key: Option<(ownership::Principal, osg_model::diplomacy::DeclarationCategory, ownership::Principal)>,
    pub society_error: Option<String>,
    pub wallet: Option<economy::WalletSnapshot>,
    pub market: Option<osg_model::market::MarketSnapshot>,
    pub assets: Option<osg_model::assets::AssetsSnapshot>,
    pub industry: IndustryState,
    pub services: requests::services::View,
    pub chat: ChatState,
    pub results: Vec<CommandResult>,
    pub events: Vec<osg_model::Event>,
    pub target_frames: usize,
    pub underruns: u64,
    pub status: String,
}

#[derive(Default)]
pub(super) struct IndustryState {
    pub snapshot: industry::IndustrySnapshot,
    subscription: Option<industry::IndustryQuery>,
    revision: u64,
}

impl IndustryState {
    pub fn ready(&self) -> bool {
        self.subscription.as_ref().is_some_and(|subscription| {
            subscription.revision == self.snapshot.subscription_revision
        })
    }

    pub fn subscribe(
        &mut self,
        mut wanted: Option<industry::IndustryQuery>,
        requests: &mut requests::Requests,
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
            requests.industry = Some(wanted.clone());
            self.subscription = Some(wanted);
        } else {
            requests.industry = None;
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
    app.insert_resource(requests::NetworkClient(client.clone()))
        .init_resource::<requests::Requests>()
        .add_systems(Update, requests::update);
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
