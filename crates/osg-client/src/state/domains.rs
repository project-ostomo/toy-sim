use super::requests::views::*;
use super::{NavigationStatus, QueryState, SessionKey, requests};
use bevy::prelude::*;
use osg_model::*;

#[derive(Resource, Default)]
pub struct NavigationState {
    pub inhabited: std::sync::Arc<InhabitedDirectory>,
    pub navigation: std::sync::Arc<NavigationCatalogue>,
    pub navigation_hash: Option<[u8; 32]>,
    pub navigation_status: NavigationStatus,
}

#[derive(Resource, Default)]
pub struct SocietyUiState {
    pub society: SocietyData,
    pub directory: requests::directory::State,
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
    pub query: requests::SocietyQuery,
    pub load: requests::Load<requests::SocietyQuery, requests::SocietyView>,
    pub context: Option<SessionKey>,
}

#[derive(Resource, Default)]
pub struct WalletState {
    pub wallet: QueryState<WalletView>,
    pub query: Option<WalletQuery>,
    pub load: requests::Load<WalletQuery, WalletView>,
}

#[derive(Resource, Default)]
pub struct MarketState {
    pub market: QueryState<MarketView>,
    pub query: Option<MarketQuery>,
    pub load: requests::Load<MarketQuery, MarketView>,
}

#[derive(Resource, Default)]
pub struct AssetsState {
    pub assets: QueryState<AssetsView>,
    pub query: Option<AssetsQuery>,
    pub load: requests::Load<AssetsQuery, AssetsView>,
}

#[derive(Resource, Default)]
pub struct ServiceState {
    pub services: QueryState<requests::services::View>,
    pub query: Option<requests::services::Query>,
    pub load: requests::Load<requests::services::Query, requests::services::View>,
}

#[derive(Resource, Default)]
pub struct CommandState {
    pub results: Vec<CommandResult>,
    pub events: Vec<osg_model::Event>,
}

#[derive(Resource, Default)]
pub struct PlaybackState {
    pub diagnostics: Option<Diagnostics>,
    pub target_frames: usize,
    pub underruns: u64,
}
