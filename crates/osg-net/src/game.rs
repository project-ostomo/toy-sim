//! Each method describes one operation. Screen composition belongs to the caller.
use osg_model::{
    AccountId, Id, assets::*, diplomacy::*, economy::*, industry::*, market::*, ownership::*,
    rpc::*,
};
use std::collections::{BTreeMap, BTreeSet};

#[osg_net_macros::rpc]
pub trait GameRpc {
    async fn my_affiliation(&self, world: Id) -> Result<PlayerAffiliation, GameError>;
    async fn list_blocs(&self, world: Id) -> Result<Vec<PoliticalBloc>, GameError>;
    async fn list_polities(&self, world: Id) -> Result<Vec<Sovereignty>, GameError>;
    async fn list_organizations(
        &self,
        world: Id,
        polity: Id,
    ) -> Result<Vec<Organization>, GameError>;
    async fn list_players(
        &self,
        world: Id,
        organization: Option<Id>,
    ) -> Result<Vec<PlayerAffiliation>, GameError>;
    async fn search_identities(
        &self,
        world: Id,
        search: String,
    ) -> Result<IdentitySearch, GameError>;
    async fn resolve_identities(
        &self,
        world: Id,
        principals: Vec<Principal>,
    ) -> Result<Vec<IdentityRecord>, GameError>;
    async fn asset_access(&self, world: Id, asset: Id) -> Result<AssetAccessDetails, GameError>;
    async fn list_access_profiles(&self, world: Id) -> Result<Vec<AccessProfile>, GameError>;
    async fn diplomacy(&self, world: Id, principal: Principal) -> Result<Diplomacy, GameError>;
    async fn resolve_standing(
        &self,
        world: Id,
        target: Principal,
    ) -> Result<StandingReport, GameError>;
    async fn declaration_history(
        &self,
        world: Id,
        source: Principal,
        category: DeclarationCategory,
        target: Principal,
        before: Option<u64>,
        limit: u16,
    ) -> Result<Page<Declaration, u64>, GameError>;
    async fn standings(
        &self,
        world: Id,
    ) -> Result<BTreeMap<(Principal, Principal), Standing>, GameError>;

    async fn list_assets(
        &self,
        world: Id,
        search: String,
        owner: Option<Principal>,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<AssetSummary, Id>, GameError>;
    async fn goods_totals(
        &self,
        world: Id,
        search: String,
        owner: Option<Principal>,
        after: Option<CargoItem>,
        limit: u16,
    ) -> Result<Page<GoodsSummary, CargoItem>, GameError>;
    async fn stock_locations(
        &self,
        world: Id,
        item: CargoItem,
        owner: Option<Principal>,
        after: Option<StockKey>,
        limit: u16,
    ) -> Result<Page<StockLocation, StockKey>, GameError>;

    async fn list_wallets(&self, world: Id) -> Result<Vec<WalletBalance>, GameError>;
    async fn wallet_balance(&self, world: Id, owner: Principal)
    -> Result<WalletAccount, GameError>;
    async fn wallet_history(
        &self,
        world: Id,
        owner: Principal,
        before: Option<u64>,
        limit: u16,
    ) -> Result<Page<LedgerEntry, u64>, GameError>;
    async fn gas_balances(&self, world: Id) -> Result<Vec<GasAccountSnapshot>, GameError>;

    async fn order_book(
        &self,
        world: Id,
        instrument: Instrument,
        limit: u16,
    ) -> Result<OrderBook, GameError>;
    async fn list_orders(
        &self,
        world: Id,
        owner: Principal,
        instrument: Option<Instrument>,
        status: Option<OrderStatus>,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<Order, Id>, GameError>;
    async fn trade_history(
        &self,
        world: Id,
        instrument: Instrument,
        before: Option<u64>,
        limit: u16,
    ) -> Result<Page<Trade, u64>, GameError>;
    async fn list_market_stations(
        &self,
        world: Id,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<MarketStation, Id>, GameError>;
    async fn compare_commodity_offers(
        &self,
        world: Id,
        item: CargoItem,
        after: Option<CommodityOfferCursor>,
        limit: u16,
    ) -> Result<Page<CommodityOffer, CommodityOfferCursor>, GameError>;
    async fn storage_stock(
        &self,
        world: Id,
        owner: Principal,
        station: Id,
    ) -> Result<Vec<StoredStock>, GameError>;

    async fn list_facilities(
        &self,
        world: Id,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<FacilitySummary, Id>, GameError>;
    async fn facility(&self, world: Id, facility: Id) -> Result<FacilityView, GameError>;
    async fn hangar(&self, world: Id, ship: Id, after: Option<Id>)
    -> Result<HangarView, GameError>;
    async fn industry_catalogue(&self, world: Id) -> Result<IndustryCatalogue, GameError>;

    async fn list_public_facilities(
        &self,
        world: Id,
        search: String,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<PublicFacility, Id>, GameError>;
    async fn service_jobs(&self, world: Id, facility: Id) -> Result<Vec<JobView>, GameError>;
    async fn quote_industry_job(
        &self,
        world: Id,
        facility: Id,
        payer: Principal,
        work: ServiceWork,
    ) -> Result<ServiceQuote, GameError>;
    async fn publish_service_prices(
        &self,
        operation: Operation,
        facility: Id,
        policy: ServicePolicy,
    ) -> Result<(), GameError>;
    async fn order_industry_job(
        &self,
        operation: Operation,
        quote: ServiceQuote,
    ) -> Result<(), GameError>;
    async fn cancel_service_job(
        &self,
        operation: Operation,
        facility: Id,
        job: Id,
    ) -> Result<(), GameError>;

    async fn transfer_money(
        &self,
        operation: Operation,
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
    ) -> Result<(), GameError>;
    async fn transfer_gas(
        &self,
        operation: Operation,
        from: Principal,
        to: Principal,
        amount: u64,
    ) -> Result<(), GameError>;
    async fn set_turnover_tax(
        &self,
        operation: Operation,
        sovereignty: Id,
        basis_points: u16,
    ) -> Result<(), GameError>;

    async fn place_limit_order(
        &self,
        operation: Operation,
        owner: Principal,
        instrument: Instrument,
        side: Side,
        quantity: u64,
        price: u64,
    ) -> Result<(), GameError>;
    async fn execute_market_order(
        &self,
        operation: Operation,
        owner: Principal,
        instrument: Instrument,
        side: Side,
        quantity: u64,
        worst_price: u64,
    ) -> Result<(), GameError>;
    async fn cancel_order(&self, operation: Operation, order: Id) -> Result<(), GameError>;
    async fn deposit_storage(
        &self,
        operation: Operation,
        owner: Principal,
        station: Id,
        ship: Id,
        item: CargoItem,
        quantity: u64,
    ) -> Result<(), GameError>;
    async fn withdraw_storage(
        &self,
        operation: Operation,
        owner: Principal,
        station: Id,
        ship: Id,
        item: CargoItem,
        quantity: u64,
    ) -> Result<(), GameError>;

    async fn transfer_cargo(
        &self,
        operation: Operation,
        source: Id,
        target: Id,
        item: CargoItem,
        quantity: u64,
    ) -> Result<(), GameError>;
    async fn unload_product(
        &self,
        operation: Operation,
        source: Id,
        target: Id,
        resource: String,
        quantity: u64,
    ) -> Result<(), GameError>;
    async fn refill_ship(
        &self,
        operation: Operation,
        source: Id,
        ship: Id,
        resource: String,
        quantity: u64,
    ) -> Result<(), GameError>;
    async fn start_recipe(
        &self,
        operation: Operation,
        facility: Id,
        recipe: String,
        batches: u32,
    ) -> Result<(), GameError>;
    async fn build_ship(
        &self,
        operation: Operation,
        facility: Id,
        owner: Principal,
        blueprint_hash: [u8; 32],
    ) -> Result<(), GameError>;
    async fn cancel_industry_job(
        &self,
        operation: Operation,
        facility: Id,
        job: Id,
    ) -> Result<(), GameError>;

    async fn create_organization(
        &self,
        operation: Operation,
        name: String,
    ) -> Result<(), GameError>;
    async fn set_organization_officer(
        &self,
        operation: Operation,
        organization: Id,
        account: AccountId,
        officer: bool,
    ) -> Result<(), GameError>;
    async fn set_membership(
        &self,
        operation: Operation,
        account: AccountId,
        organization: Option<Id>,
    ) -> Result<(), GameError>;
    async fn set_personal_standing(
        &self,
        operation: Operation,
        target: Principal,
        standing: Option<Standing>,
    ) -> Result<(), GameError>;
    async fn transfer_asset(
        &self,
        operation: Operation,
        asset: Id,
        owner: Principal,
    ) -> Result<(), GameError>;
    async fn set_asset_access(
        &self,
        operation: Operation,
        asset: Id,
        policy: AccessPolicy,
    ) -> Result<(), GameError>;
    async fn set_access_denied(
        &self,
        operation: Operation,
        asset: Id,
        denied: BTreeSet<Permission>,
    ) -> Result<(), GameError>;
    async fn save_access_profile(
        &self,
        operation: Operation,
        profile: AccessProfile,
    ) -> Result<(), GameError>;
    async fn delete_access_profile(
        &self,
        operation: Operation,
        profile: Id,
    ) -> Result<(), GameError>;
    async fn apply_access_profile(
        &self,
        operation: Operation,
        asset: Id,
        profile: Id,
    ) -> Result<(), GameError>;
    async fn unlink_access_profile(&self, operation: Operation, asset: Id)
    -> Result<(), GameError>;

    async fn publish_declaration(
        &self,
        operation: Operation,
        declaration: Declaration,
    ) -> Result<(), GameError>;
    async fn set_trust(
        &self,
        operation: Operation,
        owner: Principal,
        category: DeclarationCategory,
        sources: Vec<Principal>,
    ) -> Result<(), GameError>;
    async fn propose_agreement(
        &self,
        operation: Operation,
        from: Principal,
        to: Principal,
        title: String,
        terms: Vec<AgreementTerm>,
        note: String,
    ) -> Result<(), GameError>;
    async fn change_agreement(
        &self,
        operation: Operation,
        agreement: Id,
        expected_revision: u64,
        status: AgreementStatus,
    ) -> Result<(), GameError>;
    async fn create_bloc(
        &self,
        operation: Operation,
        name: String,
        founder: Id,
    ) -> Result<(), GameError>;
    async fn apply_to_bloc(
        &self,
        operation: Operation,
        bloc: Id,
        polity: Id,
        apply: bool,
    ) -> Result<(), GameError>;
    async fn decide_bloc_application(
        &self,
        operation: Operation,
        bloc: Id,
        polity: Id,
        admit: bool,
    ) -> Result<(), GameError>;
    async fn request_bloc_withdrawal(
        &self,
        operation: Operation,
        bloc: Id,
        polity: Id,
        request: bool,
    ) -> Result<(), GameError>;
    async fn decide_bloc_withdrawal(
        &self,
        operation: Operation,
        bloc: Id,
        polity: Id,
        grant: bool,
    ) -> Result<(), GameError>;
    async fn remove_bloc_member(
        &self,
        operation: Operation,
        bloc: Id,
        polity: Id,
    ) -> Result<(), GameError>;
    async fn set_bloc_officer(
        &self,
        operation: Operation,
        bloc: Id,
        account: AccountId,
        officer: bool,
    ) -> Result<(), GameError>;
    async fn set_political_posture(
        &self,
        operation: Operation,
        polity: Id,
        target: Id,
        standing: Standing,
    ) -> Result<(), GameError>;
    async fn set_bloc_posture(
        &self,
        operation: Operation,
        bloc: Id,
        target: Id,
        standing: Standing,
    ) -> Result<(), GameError>;
}
