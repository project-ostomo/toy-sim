use super::{NavigationStatus, SessionReset, requests};
use bevy::prelude::*;
use osg_model::*;

#[derive(Resource, Default)]
pub(crate) struct NavigationState {
    pub universe_descriptor: Option<UniverseDescriptor>,
    pub inhabited: std::sync::Arc<InhabitedDirectory>,
    pub navigation: std::sync::Arc<NavigationCatalogue>,
    pub navigation_hash: Option<[u8; 32]>,
    pub navigation_status: NavigationStatus,
}

pub(super) fn reset_navigation(_: On<SessionReset>, mut state: ResMut<NavigationState>) {
    let universe_descriptor = state.universe_descriptor.take();
    *state = NavigationState {
        universe_descriptor,
        ..Default::default()
    };
}

#[derive(Resource, Default)]
pub(crate) struct SocietyState {
    pub society: ownership::SocietySnapshot,
    pub directory_entries: Vec<ownership::Principal>,
    pub directory_next: Option<ownership::Principal>,
    pub society_assets_next: Option<Id>,
    pub society_asset_loaded: Option<Id>,
    pub declaration_history: Vec<diplomacy::Declaration>,
    pub declaration_history_next: Option<u64>,
    pub declaration_history_key: Option<(
        ownership::Principal,
        diplomacy::DeclarationCategory,
        ownership::Principal,
    )>,
    pub society_error: Option<String>,
    pub interest: requests::SocietyInterest,
    pub(crate) load: requests::Load<requests::SocietyInterest, requests::SocietyView>,
    pub(crate) context: Option<(Id, u64)>,
}

#[derive(Resource, Default)]
pub(crate) struct WalletState {
    pub wallet: Option<economy::WalletSnapshot>,
    pub interest: Option<economy::WalletQuery>,
    pub(crate) load: requests::Load<economy::WalletQuery, economy::WalletSnapshot>,
}

#[derive(Resource, Default)]
pub(crate) struct MarketState {
    pub market: Option<market::MarketSnapshot>,
    pub interest: Option<market::MarketQuery>,
    pub(crate) load: requests::Load<market::MarketQuery, market::MarketSnapshot>,
}

#[derive(Resource, Default)]
pub(crate) struct AssetsState {
    pub assets: Option<assets::AssetsSnapshot>,
    pub interest: Option<assets::AssetsQuery>,
    pub(crate) load: requests::Load<assets::AssetsQuery, assets::AssetsSnapshot>,
}

#[derive(Resource, Default)]
pub(crate) struct ServiceState {
    pub services: requests::services::View,
    pub interest: Option<requests::services::Interest>,
    pub(crate) load: requests::Load<requests::services::Interest, requests::services::View>,
}

#[derive(Resource, Default)]
pub(crate) struct CommandState {
    pub results: Vec<CommandResult>,
    pub events: Vec<osg_model::Event>,
}

#[derive(Resource, Default)]
pub(crate) struct PlaybackState {
    pub diagnostics: Option<Diagnostics>,
    pub target_frames: usize,
    pub underruns: u64,
}
