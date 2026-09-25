//! Compose independent RPC results for the current screen and discard stale work.
use super::QueryState;
use super::*;
use osg_net::OsgNetClient;
use std::time::Duration;
use tokio::sync::oneshot;
pub mod directory;
pub mod inventory;
pub mod mutations;
pub mod services;
pub mod views;

#[derive(Resource)]
pub struct NetworkClient(pub OsgNetClient);

#[derive(Resource, Default)]
pub struct Mutations {
    jobs: Vec<Job>,
    refresh_revision: u64,
}

impl Mutations {
    pub fn retry_queries(&mut self) {
        self.refresh_revision = self.refresh_revision.wrapping_add(1);
    }

    pub fn submit<F>(&mut self, key: SessionKey, id: Id, future: F)
    where
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        self.jobs.push(Job { key, id, receive });
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let _ = send.send(future.await);
            })
            .detach();
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<Mutations>()
        .add_observer(reset_resource::<Mutations>)
        .add_systems(Last, update_mutations.in_set(ClientSystems::Gameplay))
        .add_systems(
            Update,
            (
                update_services,
                update_society,
                update_wallet,
                update_market,
                update_assets,
                update_industry,
            )
                .in_set(ClientSystems::Gameplay),
        );
}

#[derive(Clone, Default, PartialEq)]
pub struct SocietyQuery {
    pub declaration_history: Option<(
        ownership::Principal,
        osg_model::diplomacy::DeclarationCategory,
        ownership::Principal,
    )>,
    pub history_before: Option<u64>,
    pub directory: bool,
    pub search: String,
    pub branches: std::collections::BTreeSet<directory::Branch>,
    pub selected: Option<ownership::Principal>,
    pub asset: Option<Id>,
    pub assets_after: Option<Id>,
    pub assets: bool,
    pub profiles: bool,
    pub gas: bool,
}

pub struct SocietyView {
    history_key: Option<(
        ownership::Principal,
        osg_model::diplomacy::DeclarationCategory,
        ownership::Principal,
    )>,
    history: Vec<osg_model::diplomacy::Declaration>,
    history_next: Option<u64>,
    snapshot: SocietyData,
    assets_next: Option<Id>,
    loaded_asset: Option<Id>,
}

struct Job {
    key: SessionKey,
    id: Id,
    receive: oneshot::Receiver<Result<(), String>>,
}

pub struct Load<Q, T> {
    refresh_revision: u64,
    context: Option<(SessionKey, Q)>,
    pending: Option<oneshot::Receiver<Result<T, String>>>,
    next: Duration,
}

impl<Q: Clone + PartialEq, T: Send + 'static> Load<Q, T> {
    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }

    fn refresh(&mut self, revision: u64) {
        if self.refresh_revision != revision {
            self.pending = None;
            self.next = Duration::ZERO;
            self.refresh_revision = revision;
        }
    }

    fn update<F, Fut>(
        &mut self,
        context: Option<(SessionKey, Q)>,
        now: Duration,
        fetch: F,
    ) -> (bool, Option<Result<T, String>>)
    where
        F: FnOnce(Id, Q) -> Fut,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let changed = self.context != context;
        if changed {
            self.pending = None;
            self.context = context;
            self.next = Duration::ZERO;
        }
        let mut result = None;
        if let Some(receive) = &mut self.pending {
            match receive.try_recv() {
                Ok(value) => result = Some(value),
                Err(oneshot::error::TryRecvError::Closed) => {
                    result = Some(Err("Request cancelled".into()))
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if result.is_some() {
            self.pending = None;
            self.next = now + Duration::from_secs(1);
        }
        if self.pending.is_none() && now >= self.next {
            if let Some((key, query)) = &self.context {
                let future = fetch(key.world, query.clone());
                let (mut send, receive) = oneshot::channel();
                self.pending = Some(receive);
                bevy::tasks::IoTaskPool::get()
                    .spawn(async move {
                        tokio::pin!(future);
                        tokio::select! {
                            value = &mut future => { let _ = send.send(value); }
                            _ = send.closed() => {}
                        }
                    })
                    .detach();
            }
        }
        (changed, result)
    }
}

impl<Q, T> Default for Load<Q, T> {
    fn default() -> Self {
        Self {
            refresh_revision: 0,
            context: None,
            pending: None,
            next: Duration::ZERO,
        }
    }
}

#[derive(Default)]
pub struct Loading {
    pub society: bool,
    pub wallet: bool,
    pub market: bool,
    pub assets: bool,
    pub industry: bool,
    pub services: bool,
}

pub async fn call<T>(
    future: impl Future<Output = Result<Result<T, osg_model::rpc::GameError>, osg_net::RpcError>>,
) -> Result<T, String> {
    future
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

fn update_mutations(
    mut requests: ResMut<Mutations>,
    session: Res<GameSession>,
    mut feedback: ResMut<CommandState>,
) {
    let mut refresh = false;
    requests.jobs.retain_mut(|job| {
        if job.key != session.key {
            return false;
        }
        let result = match job.receive.try_recv() {
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Empty) => return true,
            Err(oneshot::error::TryRecvError::Closed) => {
                Err("Request cancelled; outcome is unknown".into())
            }
        };
        refresh |= result.is_ok();
        feedback.results.push(CommandResult {
            id: job.id,
            error: result.err(),
        });
        false
    });
    if refresh {
        requests.refresh_revision = requests.refresh_revision.wrapping_add(1);
    }
}
fn update_services(
    mut state: ResMut<ServiceState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    let now = time.elapsed();
    state.load.refresh(mutations.refresh_revision);
    let wanted = context.and_then(|key| state.query.clone().map(|query| (key, query)));
    let net = client.0.clone();
    let (changed, result) = state.load.update(wanted, now, move |world, query| {
        services::fetch(net, world, query)
    });
    state.services.apply(changed, result);
}

fn update_society(
    mut state: ResMut<SocietyState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    let now = time.elapsed();
    state.load.refresh(mutations.refresh_revision);
    let current = context;
    let context_changed = state.context != current;
    state.context = current;
    let mut detail_query = state.query.clone();
    detail_query.branches.clear();
    detail_query.search.clear();
    detail_query.directory = false;
    let wanted = context.map(|key| (key, detail_query));
    let net = client.0.clone();
    let (_, result) = state
        .load
        .update(wanted, now, move |world, query| society(net, world, query));
    apply_society(&mut state, context_changed, result);
    let state = &mut *state;
    state.directory.update(
        &client.0,
        current,
        &state.query,
        now,
        mutations.refresh_revision,
        &mut state.society.directory,
    );
}

fn update_wallet(
    mut state: ResMut<WalletState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    let now = time.elapsed();
    state.load.refresh(mutations.refresh_revision);
    let wanted = context.zip(state.query.clone());
    let net = client.0.clone();
    let (changed, result) = state
        .load
        .update(wanted, now, move |world, query| wallet(net, world, query));
    state.wallet.apply(changed, result);
}

fn update_market(
    mut state: ResMut<MarketState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    let now = time.elapsed();
    state.load.refresh(mutations.refresh_revision);
    let wanted = context.zip(state.query.clone());
    let net = client.0.clone();
    let (changed, result) = state
        .load
        .update(wanted, now, move |world, query| market(net, world, query));
    state.market.apply(changed, result);
}

fn update_assets(
    mut state: ResMut<AssetsState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    let now = time.elapsed();
    state.load.refresh(mutations.refresh_revision);
    let wanted = context.zip(state.query.clone());
    let net = client.0.clone();
    let (changed, result) = state
        .load
        .update(wanted, now, move |world, query| assets(net, world, query));
    state.assets.apply(changed, result);
}

fn update_industry(
    mut state: ResMut<IndustryState>,
    client: Res<NetworkClient>,
    session: Res<GameSession>,
    time: Res<Time<Real>>,
    mutations: Res<Mutations>,
) {
    let context = Some(session.key);
    state.update(
        &client.0,
        context,
        time.elapsed(),
        mutations.refresh_revision,
    );
}

fn apply_society(
    session: &mut SocietyState,
    context_changed: bool,
    result: Option<Result<SocietyView, String>>,
) {
    // Preserve cached public identities while refreshing a detail view.
    if context_changed {
        session.society = SocietyData {
            account: session.society.account,
            ..Default::default()
        };
        session.directory = Default::default();
        session.society_assets_next = None;
        session.society_asset_loaded = None;
        session.declaration_history.clear();
        session.declaration_history_next = None;
        session.declaration_history_key = None;
        session.society_error = None;
    }
    if let Some(result) = result {
        match result {
            Ok(view) => {
                let mut snapshot = view.snapshot;
                let previous = &mut session.society.directory;
                snapshot.directory.sovereignties = std::mem::take(&mut previous.sovereignties)
                    .into_iter()
                    .chain(snapshot.directory.sovereignties)
                    .collect();
                snapshot.directory.organizations = std::mem::take(&mut previous.organizations)
                    .into_iter()
                    .chain(snapshot.directory.organizations)
                    .collect();
                snapshot.directory.players = std::mem::take(&mut previous.players)
                    .into_iter()
                    .chain(snapshot.directory.players)
                    .collect();
                session.society = snapshot;
                session.society_assets_next = view.assets_next;
                session.society_asset_loaded = view.loaded_asset;
                session.declaration_history = view.history;
                session.declaration_history_next = view.history_next;
                session.declaration_history_key = view.history_key;
                session.society_error = None;
            }
            Err(error) => {
                session.society_error = Some(error);
                session.society.assets.clear();
                session.society.gas_accounts.clear();
                session.society.directory.access_profiles.clear();
                session.society.directory.access_bindings.clear();
                session.society.directory.diplomacy.trust.clear();
            }
        }
    }
}

async fn wallet(client: OsgNetClient, world: Id, query: WalletQuery) -> Result<WalletView, String> {
    let (account, history, owners, fx_history) = tokio::try_join!(
        call(client.wallet_balance(world, query.owner)),
        call(client.wallet_history(world, query.owner, query.before, query.limit)),
        call(client.list_wallets(world)),
        call(client.trade_history(world, osg_model::market::Instrument::Fx, None, 60)),
    )?;
    let mut balances = owners;
    balances.retain(|balance| balance.owner != query.owner);
    balances.insert(0, account.balance);
    Ok(WalletView {
        fx_trades: fx_history.items,
        owner: Some(query.owner),
        balances,
        entries: history.items,
        next_before: history.next,
        next_charge_ms: account.next_charge_ms,
        market_uec_per_lat: account.market_uec_per_lat,
    })
}

async fn market(client: OsgNetClient, world: Id, query: MarketQuery) -> Result<MarketView, String> {
    use osg_model::market::Instrument;
    let (account, book, orders, history, stations) = tokio::try_join!(
        call(client.wallet_balance(world, query.owner)),
        call(client.order_book(world, query.instrument.clone(), 50)),
        call(client.list_orders(
            world,
            query.owner,
            Some(query.instrument.clone()),
            query.order_status,
            query.orders_after,
            128
        )),
        call(client.trade_history(world, query.instrument.clone(), query.before, query.limit)),
        call(client.list_market_stations(world, query.stations_after, 128)),
    )?;
    let (stock, offers, offers_next) =
        if let Instrument::Commodity { station, item, .. } = &query.instrument {
            let (stock, offers) = tokio::try_join!(
                call(client.storage_stock(world, query.owner, *station)),
                call(client.compare_commodity_offers(world, item.clone(), query.offers_after, 128)),
            )?;
            (stock, offers.items, offers.next)
        } else {
            (Vec::new(), Vec::new(), None)
        };
    Ok(MarketView {
        offers,
        offers_next,
        instrument: query.instrument,
        owner: Some(query.owner),
        stations: stations.items,
        stations_next: stations.next,
        stock,
        available_uec: account
            .balance
            .uec
            .saturating_sub(account.balance.reserved_uec),
        available_lat: account
            .balance
            .lat
            .saturating_sub(account.balance.reserved_lat),
        reserved_uec: account.balance.reserved_uec,
        reserved_lat: account.balance.reserved_lat,
        lat_restricted: account.balance.lat_restricted,
        last_price: book.last_price,
        backstop_price: book.backstop_price,
        bids: book.bids,
        asks: book.asks,
        orders: orders.items,
        orders_next: orders.next,
        trades: history.items,
        next_before: history.next,
    })
}

async fn assets(client: OsgNetClient, world: Id, query: AssetsQuery) -> Result<AssetsView, String> {
    let (assets, goods) = tokio::try_join!(
        call(client.list_assets(
            world,
            query.search.clone(),
            query.owner,
            query.after,
            query.limit
        )),
        call(client.goods_totals(
            world,
            query.search.clone(),
            query.owner,
            query.goods_after.clone(),
            query.limit
        )),
    )?;
    let (sources, sources_next) = if let Some(item) = &query.item {
        let page = call(client.stock_locations(
            world,
            item.clone(),
            query.owner,
            query.sources_after.clone(),
            query.limit,
        ))
        .await?;
        (page.items, page.next)
    } else {
        (Vec::new(), None)
    };
    Ok(AssetsView {
        query,
        assets: assets.items,
        goods: goods.items,
        sources,
        next: assets.next,
        goods_next: goods.next,
        sources_next,
        total_assets: assets.total.unwrap_or(0),
        total_goods: goods.total.unwrap_or(0),
    })
}

async fn society(
    client: OsgNetClient,
    world: Id,
    query: SocietyQuery,
) -> Result<SocietyView, String> {
    use osg_model::rpc::IdentityRecord;
    use ownership::{AssetAffiliation, Principal};
    let me = call(client.my_affiliation(world)).await?;
    let account = me.account;
    let mut snapshot = SocietyData {
        account,
        ..Default::default()
    };
    snapshot.directory.players.insert(account, me);
    let mut wanted = std::collections::BTreeSet::new();
    wanted.extend(query.selected);
    let (assets, gas, profiles, diplomacy, standings) = tokio::try_join!(
        async {
            if query.assets {
                call(client.list_assets(world, String::new(), None, query.assets_after, 128)).await
            } else {
                Ok(osg_model::rpc::Page {
                    items: Vec::new(),
                    next: None,
                    total: None,
                })
            }
        },
        async {
            if query.gas {
                call(client.gas_balances(world)).await
            } else {
                Ok(Vec::new())
            }
        },
        async {
            if query.profiles {
                call(client.list_access_profiles(world)).await
            } else {
                Ok(Vec::new())
            }
        },
        call(client.diplomacy(world, query.selected.unwrap_or(Principal::Player(account)))),
        call(client.standings(world)),
    )?;
    snapshot.gas_accounts = gas;
    wanted.extend(snapshot.gas_accounts.iter().map(|entry| entry.owner));
    snapshot.directory.access_profiles = profiles
        .into_iter()
        .map(|profile| (profile.id, profile))
        .collect();
    snapshot.directory.diplomacy = diplomacy;
    snapshot.directory.standings = standings;
    if let Some(target) = query.selected {
        let report = call(client.resolve_standing(world, target)).await?;
        match report.source {
            ownership::StandingSource::Declaration { source, .. }
            | ownership::StandingSource::Override { source, .. } => {
                wanted.insert(source);
            }
            ownership::StandingSource::MutualDefence { ally, .. } => {
                wanted.insert(ally);
            }
            _ => {}
        }
        snapshot.standing_report = Some(report);
    }
    for &(source, target) in snapshot.directory.standings.keys() {
        wanted.extend([source, target]);
    }
    for declaration in snapshot.directory.diplomacy.declarations.values() {
        wanted.extend([declaration.source, declaration.target]);
    }
    for agreement in snapshot.directory.diplomacy.agreements.values() {
        wanted.extend([agreement.from, agreement.to]);
    }
    for (&(source, _), trustees) in &snapshot.directory.diplomacy.trust {
        wanted.insert(source);
        wanted.extend(trustees);
    }
    for asset in assets.items {
        wanted.insert(asset.owner);
        snapshot.assets.push(AssetAffiliation {
            entity: asset.id,
            name: asset.name,
            owner: asset.owner,
            access: Default::default(),
            can_manage: asset.can_manage,
        });
    }
    if let Some(asset) = query.asset {
        let detail = call(client.asset_access(world, asset)).await?;
        wanted.insert(detail.asset.owner);
        wanted.extend(
            detail
                .asset
                .access
                .grants
                .iter()
                .map(|grant| grant.principal),
        );
        snapshot.assets.retain(|entry| entry.entity != asset);
        snapshot.assets.push(detail.asset);
        if let Some(binding) = detail.binding {
            snapshot.directory.access_bindings.insert(asset, binding);
        }
        if let Some(profile) = detail.profile {
            snapshot
                .directory
                .access_profiles
                .insert(profile.id, profile);
        }
    }
    // Resolve the selected identities and their organization/sovereignty ancestry.
    let mut resolved = std::collections::BTreeSet::new();
    wanted.insert(Principal::Player(account));
    for _ in 0..3 {
        let unresolved: Vec<_> = wanted.difference(&resolved).copied().collect();
        if unresolved.is_empty() {
            break;
        }
        for chunk in unresolved.chunks(128) {
            for entry in call(client.resolve_identities(world, chunk.to_vec())).await? {
                resolved.insert(entry.principal());
                match entry {
                    IdentityRecord::Sovereignty(value) => {
                        snapshot.directory.sovereignties.insert(value.id, value);
                    }
                    IdentityRecord::Organization(value) => {
                        wanted.insert(Principal::Sovereignty(value.sovereignty));
                        snapshot.directory.organizations.insert(value.id, value);
                    }
                    IdentityRecord::Player(value) => {
                        wanted.extend(value.organization.map(Principal::Organization));
                        snapshot.directory.players.insert(value.account, value);
                    }
                }
            }
            resolved.extend(chunk);
        }
    }
    let history = if let Some((source, category, target)) = query.declaration_history {
        Some(
            call(client.declaration_history(
                world,
                source,
                category,
                target,
                query.history_before,
                32,
            ))
            .await?,
        )
    } else {
        None
    };
    Ok(SocietyView {
        history_key: query.declaration_history,
        history: history
            .as_ref()
            .map_or_else(Vec::new, |page| page.items.clone()),
        history_next: history.and_then(|page| page.next),
        snapshot,
        assets_next: assets.next,
        loaded_asset: query.asset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_dispatch_waits_for_running_and_ignores_input_queue_status() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<ClientPhase>()
            .init_resource::<Mutations>()
            .init_resource::<CommandState>()
            .insert_resource(SessionInfo {
                status: "input queue busy".into(),
                ..Default::default()
            })
            .configure_sets(
                Update,
                ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
            )
            .add_systems(Update, update_mutations.in_set(ClientSystems::Gameplay));
        let world = Id([9; 16]);
        let (send, receive) = oneshot::channel();
        send.send(Ok(())).unwrap();
        app.world_mut().resource_mut::<Mutations>().jobs.push(Job {
            key: SessionKey {
                world,
                generation: 1,
            },
            id: Id([8; 16]),
            receive,
        });
        app.update();
        assert!(app.world().resource::<CommandState>().results.is_empty());
        assert_eq!(app.world().resource::<Mutations>().jobs.len(), 1);

        app.insert_resource(GameSession::test(world, 1));
        app.world_mut()
            .resource_mut::<NextState<ClientPhase>>()
            .set(ClientPhase::Running);
        app.update();
        assert_eq!(app.world().resource::<CommandState>().results.len(), 1);
        assert!(app.world().resource::<Mutations>().jobs.is_empty());
    }

    #[test]
    fn mutation_completion_refreshes_domains_and_discards_stale_generations() {
        let world = Id([1; 16]);
        let current_id = Id([2; 16]);
        let stale_id = Id([3; 16]);
        let (current_send, current_receive) = oneshot::channel();
        let (stale_send, stale_receive) = oneshot::channel();
        current_send.send(Ok(())).unwrap();
        stale_send.send(Ok(())).unwrap();

        let mut app = App::new();
        app.insert_resource(GameSession::test(world, 2))
            .init_resource::<CommandState>()
            .insert_resource(Mutations {
                jobs: vec![
                    Job {
                        key: SessionKey {
                            world,
                            generation: 1,
                        },
                        id: stale_id,
                        receive: stale_receive,
                    },
                    Job {
                        key: SessionKey {
                            world,
                            generation: 2,
                        },
                        id: current_id,
                        receive: current_receive,
                    },
                ],
                refresh_revision: 0,
            })
            .add_systems(Update, update_mutations);
        app.update();

        let results = &app.world().resource::<CommandState>().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, current_id);
        assert!(results[0].error.is_none());
        let revision = app.world().resource::<Mutations>().refresh_revision;
        assert_eq!(revision, 1);

        let mut load = Load::<u8, u8>::default();
        let (_, pending) = oneshot::channel();
        load.pending = Some(pending);
        load.next = Duration::from_secs(10);
        load.refresh(revision);
        assert!(load.pending.is_none());
        assert_eq!(load.next, Duration::ZERO);
        load.next = Duration::from_secs(20);
        load.refresh(revision);
        assert_eq!(load.next, Duration::from_secs(20));

        app.update();
        assert_eq!(
            app.world().resource::<Mutations>().refresh_revision,
            revision
        );
    }

    #[test]
    fn directory_keeps_content_until_replacement_and_clears_on_session_change() {
        use ownership::PlayerAffiliation;

        let account = Id([1; 16]);
        let mut session = SocietyState::default();
        session.society.account = account;
        session.society.directory.players.insert(
            account,
            PlayerAffiliation {
                account,
                name: "Current identity".into(),
                organization: None,
            },
        );

        apply_society(&mut session, false, None);
        assert_eq!(
            session.society.directory.players[&account].name,
            "Current identity"
        );

        apply_society(&mut session, false, Some(Err("Offline".into())));
        assert_eq!(
            session.society.directory.players[&account].name,
            "Current identity"
        );
        assert_eq!(session.society_error.as_deref(), Some("Offline"));

        let mut replacement = session.society.clone();
        replacement
            .directory
            .players
            .get_mut(&account)
            .unwrap()
            .name = "Updated identity".into();
        apply_society(
            &mut session,
            false,
            Some(Ok(SocietyView {
                snapshot: replacement,
                assets_next: None,
                loaded_asset: None,
                history: Vec::new(),
                history_key: None,
                history_next: None,
            })),
        );
        assert_eq!(
            session.society.directory.players[&account].name,
            "Updated identity"
        );
        assert!(session.society_error.is_none());

        apply_society(&mut session, true, None);
        assert!(session.society.directory.players.is_empty());
    }

    #[test]
    fn query_changes_discard_pending_work_and_refreshes_are_coalesced() {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut load = Load::<u8, u8>::default();
        let world = Id([1; 16]);
        let (old_send, old_receive) = oneshot::channel::<u8>();
        let (changed, result) = load.update(
            Some((
                SessionKey {
                    world,
                    generation: 1,
                },
                10,
            )),
            Duration::ZERO,
            |_, _| async { old_receive.await.map_err(|_| "cancelled".into()) },
        );
        assert!(changed && result.is_none());
        let (changed, result) = load.update(
            Some((
                SessionKey {
                    world,
                    generation: 1,
                },
                10,
            )),
            Duration::ZERO,
            |_, _| {
                panic!("an in-flight query must coalesce refreshes");
                #[allow(unreachable_code)]
                async {
                    Ok(0)
                }
            },
        );
        assert!(!changed && result.is_none());

        load.update(
            Some((
                SessionKey {
                    world,
                    generation: 2,
                },
                20,
            )),
            Duration::ZERO,
            |_, value| async move { Ok(value) },
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let (_, result) = load.update(
                Some((
                    SessionKey {
                        world,
                        generation: 2,
                    },
                    20,
                )),
                Duration::ZERO,
                |_, _| async { panic!("a pending refresh must not launch again") },
            );
            if let Some(result) = result {
                assert_eq!(result.unwrap(), 20);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let _ = old_send.send(10);
        let (_, result) = load.update(
            Some((
                SessionKey {
                    world,
                    generation: 2,
                },
                20,
            )),
            Duration::from_millis(999),
            |_, _| async { panic!("refresh interval has not elapsed") },
        );
        assert!(result.is_none());
        let (changed, _) = load.update(None, Duration::from_secs(1), |_, _| async {
            panic!("closed screens must not refresh")
        });
        assert!(changed && load.pending.is_none());
    }
}
