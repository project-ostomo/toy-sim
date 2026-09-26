use super::*;
use osg_model::{
    assets::*, diplomacy::*, economy::*, industry::*, market::*, ownership::*, rpc::*,
};
use sim::industry::{
    CancelWork, CargoAction, ConfigureService, Incoming, IndustryQueryRequest, ReadRequest,
    StartWork,
};
use std::collections::{BTreeMap, BTreeSet};

macro_rules! industry_query {
    ($name:ident($($argument:ident: $ty:ty),*) -> $result:ty, $variant:ident, $args:expr) => {
        async fn $name(&self, epoch: Id, $($argument: $ty),*) -> Result<$result, GameError> {
            self.enqueue_industry(|reply| Incoming::Query(IndustryQueryRequest::$variant(ReadRequest {
                world: epoch, arguments: $args, reply,
            }))).await
        }
    };
}

macro_rules! industry_mutation {
    ($name:ident($($argument:ident: $ty:ty),*) => $args:expr, $variant:ident) => {
        async fn $name(&self, world: Id, $($argument: $ty),*) -> Result<(), GameError> {
            self.queue_mutation(world, $args, Incoming::$variant).await
        }
    };
}

macro_rules! query {
    ($name:ident($($argument:ident: $ty:ty),*) -> $result:ty) => {
        async fn $name(&self, epoch: Id, $($argument: $ty),*) -> Result<$result, GameError> {
            self.call(epoch, move |world, account, _| queries::$name(world, account, $($argument),*)).await
        }
    };
}

macro_rules! mutation {
    ($name:ident($($argument:ident: $ty:ty),*) => $command:expr, $apply:ident) => {
        async fn $name(&self, world: Id, $($argument: $ty),*) -> Result<(), GameError> {
            self.queue_society(world, $command).await
        }
    };
}

impl osg_net::GameRpc for Handler {
    query!(society_view(query: osg_model::society::SocietyQuery) -> osg_model::society::SocietyView);
    query!(my_affiliation() -> PlayerAffiliation);
    query!(list_blocs() -> Vec<PoliticalBloc>);
    query!(list_polities() -> Vec<Sovereignty>);
    query!(list_organizations(polity: Id) -> Vec<Organization>);
    query!(list_players(organization: Option<Id>) -> Vec<PlayerAffiliation>);
    query!(search_identities(search: String) -> IdentitySearch);
    query!(resolve_identities(principals: Vec<Principal>) -> Vec<IdentityRecord>);
    query!(asset_access(asset: Id) -> AssetAccessDetails);
    query!(list_access_profiles() -> Vec<AccessProfile>);
    query!(diplomacy(principal: Principal) -> DiplomacyView);
    query!(resolve_standing(target: Principal) -> StandingReport);
    query!(declaration_history(source: Principal, category: DeclarationCategory, target: Principal, before: Option<u64>, limit: u16) -> Page<Declaration, u64>);
    query!(standings() -> BTreeMap<(Principal, Principal), Standing>);
    query!(list_assets(search: String, owner: Option<Principal>, after: Option<Id>, limit: u16) -> Page<AssetSummary, Id>);
    query!(goods_totals(search: String, owner: Option<Principal>, after: Option<CargoItem>, limit: u16) -> Page<GoodsSummary, CargoItem>);
    query!(stock_locations(item: CargoItem, owner: Option<Principal>, after: Option<StockKey>, limit: u16) -> Page<StockLocation, StockKey>);
    query!(list_wallets() -> Vec<WalletBalance>);
    query!(wallet_balance(owner: Principal) -> WalletAccount);
    query!(wallet_history(owner: Principal, before: Option<u64>, limit: u16) -> Page<LedgerEntry, u64>);
    query!(gas_balances() -> Vec<GasAccountSnapshot>);
    query!(order_book(instrument: Instrument, limit: u16) -> OrderBook);
    query!(list_orders(owner: Principal, instrument: Option<Instrument>, status: Option<OrderStatus>, after: Option<Id>, limit: u16) -> Page<Order, Id>);
    query!(trade_history(instrument: Instrument, before: Option<u64>, limit: u16) -> Page<Trade, u64>);
    query!(list_market_stations(after: Option<Id>, limit: u16) -> Page<MarketStation, Id>);
    query!(compare_commodity_offers(item: CargoItem, after: Option<CommodityOfferCursor>, limit: u16) -> Page<CommodityOffer, CommodityOfferCursor>);
    query!(storage_stock(owner: Principal, station: Id) -> Vec<StoredStock>);
    industry_query!(list_facilities(after: Option<Id>, limit: u16) -> Page<FacilitySummary, Id>, Directory, (after, limit));
    industry_query!(facility(facility: Id) -> FacilityView, Facility, facility);
    industry_query!(hangar(ship: Id, after: Option<Id>) -> HangarView, Hangar, (ship, after));
    industry_query!(industry_catalogue() -> IndustryCatalogue, Catalogue, ());

    async fn list_public_facilities(
        &self,
        epoch: Id,
        search: String,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<PublicFacility, Id>, GameError> {
        self.enqueue_industry(|reply| {
            Incoming::Query(IndustryQueryRequest::PublicFacilities(ReadRequest {
                world: epoch,
                arguments: (search, after, limit),
                reply,
            }))
        })
        .await
    }

    async fn service_jobs(&self, epoch: Id, facility: Id) -> Result<Vec<JobView>, GameError> {
        self.enqueue_industry(|reply| {
            Incoming::Query(IndustryQueryRequest::Jobs(ReadRequest {
                world: epoch,
                arguments: facility,
                reply,
            }))
        })
        .await
    }

    async fn quote_industry_job(
        &self,
        epoch: Id,
        facility: Id,
        payer: Principal,
        work: ServiceWork,
    ) -> Result<ServiceQuote, GameError> {
        self.enqueue_industry(|reply| {
            Incoming::Query(IndustryQueryRequest::Quote(ReadRequest {
                world: epoch,
                arguments: (facility, payer, work),
                reply,
            }))
        })
        .await
    }

    async fn publish_service_prices(
        &self,
        world: Id,
        facility: Id,
        policy: ServicePolicy,
    ) -> Result<(), GameError> {
        self.queue_mutation(
            world,
            ConfigureService { facility, policy },
            Incoming::Configure,
        )
        .await
    }

    async fn order_industry_job(&self, world: Id, quote: ServiceQuote) -> Result<(), GameError> {
        self.queue_mutation(world, StartWork::Public(quote), Incoming::Work)
            .await
    }

    async fn cancel_service_job(&self, world: Id, facility: Id, job: Id) -> Result<(), GameError> {
        self.queue_mutation(world, CancelWork { facility, job }, Incoming::Cancel)
            .await
    }

    mutation!(transfer_money(from: Principal, to: Principal, currency: Currency, amount: u64)
        => WalletCommand::Transfer { from, to, currency, amount }, wallet);
    mutation!(transfer_gas(from: Principal, to: Principal, amount: u64)
        => WalletCommand::TransferGas { from, to, amount }, wallet);
    mutation!(set_turnover_tax(sovereignty: Id, basis_points: u16)
        => WalletCommand::SetTurnoverTax { sovereignty, basis_points }, wallet);
    mutation!(place_limit_order(owner: Principal, instrument: Instrument, side: Side, quantity: u64, price: u64)
        => MarketCommand::Limit { owner, instrument, side, quantity, price }, market);
    mutation!(execute_market_order(owner: Principal, instrument: Instrument, side: Side, quantity: u64, worst_price: u64)
        => MarketCommand::Immediate { owner, instrument, side, quantity, price: worst_price }, market);
    mutation!(cancel_order(order: Id) => MarketCommand::Cancel { order }, market);
    mutation!(deposit_storage(owner: Principal, station: Id, ship: Id, item: CargoItem, quantity: u64)
        => MarketCommand::MoveStorage { owner, station, ship, item, quantity, deposit: true }, market);
    mutation!(withdraw_storage(owner: Principal, station: Id, ship: Id, item: CargoItem, quantity: u64)
        => MarketCommand::MoveStorage { owner, station, ship, item, quantity, deposit: false }, market);
    industry_mutation!(transfer_cargo(source: Id, target: Id, item: CargoItem, quantity: u64)
        => CargoAction::Transfer { source, target, item, quantity }, Cargo);
    industry_mutation!(unload_product(source: Id, target: Id, resource: String, quantity: u64)
        => CargoAction::UnloadProduct { source, target, resource, quantity }, Cargo);
    industry_mutation!(refill_ship(source: Id, ship: Id, resource: String, quantity: u64)
        => CargoAction::Refill { source, target: ship, resource, quantity }, Cargo);
    industry_mutation!(start_recipe(facility: Id, recipe: String, batches: u32)
        => StartWork::Recipe { facility, recipe, batches }, Work);
    industry_mutation!(build_ship(facility: Id, owner: Principal, blueprint_hash: [u8; 32])
        => StartWork::Ship { facility, owner, blueprint_hash }, Work);
    industry_mutation!(cancel_industry_job(facility: Id, job: Id) => CancelWork { facility, job }, Cancel);
    mutation!(create_organization(name: String) => SocietyCommand::CreateOrganization { name }, society);
    mutation!(set_organization_officer(organization: Id, account: AccountId, officer: bool)
        => SocietyCommand::SetOfficer { organization, account, officer }, society);
    mutation!(set_membership(account: AccountId, organization: Option<Id>)
        => SocietyCommand::SetMembership { account, organization }, society);
    mutation!(set_personal_standing(target: Principal, standing: Option<Standing>)
        => SocietyCommand::SetStanding { target, standing }, society);
    mutation!(transfer_asset(asset: Id, owner: Principal) => SocietyCommand::TransferAsset { asset, owner }, society);
    mutation!(set_asset_access(asset: Id, policy: AccessPolicy) => SocietyCommand::SetAssetAccess { asset, policy }, society);
    mutation!(set_access_denied(asset: Id, denied: BTreeSet<Permission>) => SocietyCommand::SetAccessDenied { asset, denied }, society);
    mutation!(save_access_profile(profile: AccessProfile) => SocietyCommand::SaveAccessProfile(profile), society);
    mutation!(delete_access_profile(profile: Id) => SocietyCommand::DeleteAccessProfile { id: profile }, society);
    mutation!(apply_access_profile(asset: Id, profile: Id) => SocietyCommand::ApplyAccessProfile { asset, profile }, society);
    mutation!(unlink_access_profile(asset: Id) => SocietyCommand::UnlinkAccessProfile { asset }, society);
    mutation!(publish_declaration(declaration: Declaration) => DiplomacyCommand::Publish(declaration), diplomacy);
    mutation!(set_trust(owner: Principal, category: DeclarationCategory, sources: Vec<Principal>)
        => DiplomacyCommand::SetTrust { owner, category, sources }, diplomacy);
    mutation!(propose_agreement(from: Principal, to: Principal, title: String, terms: Vec<AgreementTerm>, note: String)
        => DiplomacyCommand::ProposeAgreement { from, to, title, terms, note }, diplomacy);
    mutation!(change_agreement(agreement: Id, expected_revision: u64, status: AgreementStatus)
        => DiplomacyCommand::ChangeAgreement { id: agreement, expected_revision, status }, diplomacy);
    mutation!(create_bloc(name: String, founder: Id) => DiplomacyCommand::CreateBloc { name, founder }, diplomacy);
    mutation!(apply_to_bloc(bloc: Id, polity: Id, apply: bool) => DiplomacyCommand::ApplyToBloc { bloc, polity, apply }, diplomacy);
    mutation!(decide_bloc_application(bloc: Id, polity: Id, admit: bool) => DiplomacyCommand::DecideApplication { bloc, polity, admit }, diplomacy);
    mutation!(request_bloc_withdrawal(bloc: Id, polity: Id, request: bool) => DiplomacyCommand::RequestBlocWithdrawal { bloc, polity, request }, diplomacy);
    mutation!(decide_bloc_withdrawal(bloc: Id, polity: Id, grant: bool) => DiplomacyCommand::DecideBlocWithdrawal { bloc, polity, grant }, diplomacy);
    mutation!(remove_bloc_member(bloc: Id, polity: Id) => DiplomacyCommand::RemoveBlocMember { bloc, polity }, diplomacy);
    mutation!(set_bloc_officer(bloc: Id, account: AccountId, officer: bool) => DiplomacyCommand::SetBlocOfficer { bloc, account, officer }, diplomacy);
    mutation!(set_political_posture(polity: Id, target: Id, standing: Standing) => DiplomacyCommand::SetPosture { polity, target, standing }, diplomacy);
    mutation!(set_bloc_posture(bloc: Id, target: Id, standing: Standing) => DiplomacyCommand::SetBlocPosture { bloc, target, standing }, diplomacy);
}
