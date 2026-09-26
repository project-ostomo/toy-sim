use super::*;

fn owner(n: u8) -> Principal {
    Principal::Player(Id([n; 16]))
}

fn world() -> World {
    let mut world = World::new();
    crate::sim::identity::initialize(&mut world, &[Id([1; 16]), Id([2; 16]), Id([3; 16])]);
    let mut economy = world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy);
    economy.licences.extend([owner(1), owner(2), owner(3)]);
    let now = osg_model::calendar::now_unix_ms();
    economy
        .issue(owner(1), Currency::Uec, 100 * MONEY_SCALE, now)
        .unwrap();
    economy
        .issue(owner(2), Currency::Uec, 100 * MONEY_SCALE, now)
        .unwrap();
    economy
        .issue(owner(3), Currency::Lat, 10 * MONEY_SCALE, now)
        .unwrap();
    world
}

fn place(world: &mut World, account: u8, side: Side, quantity: u64, price: u64) -> Id {
    let id = Id::new();
    crate::sim::society::submit(
        world,
        Id([account; 16]),
        crate::sim::society::Action::Market(
            id,
            MarketCommand::Limit {
                instrument: Instrument::Fx,
                owner: owner(account),
                side,
                quantity,
                price,
            },
        ),
    )
    .unwrap();
    id
}

#[test]
fn price_time_priority_partial_fills_and_reservations_conserve_money() {
    let mut world = world();
    let first = place(&mut world, 1, Side::Buy, 4 * MONEY_SCALE, 4 * MONEY_SCALE);
    let second = place(&mut world, 2, Side::Buy, 4 * MONEY_SCALE, 4 * MONEY_SCALE);
    place(&mut world, 3, Side::Sell, 6 * MONEY_SCALE, 3 * MONEY_SCALE);
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert!(!economy.exchange.orders.contains_key(&first));
    assert_eq!(economy.exchange.orders[&second].remaining, 2 * MONEY_SCALE);
    assert_eq!(
        economy.exchange.orders[&second].original_quantity,
        4 * MONEY_SCALE
    );
    assert_eq!(
        economy.exchange.orders[&second].filled_quantity,
        2 * MONEY_SCALE
    );
    assert_eq!(economy.balances[&owner(1)].lat, 4 * MONEY_SCALE);
    assert_eq!(economy.balances[&owner(2)].lat, 2 * MONEY_SCALE);
    assert_eq!(economy.balances[&owner(3)].uec, 24 * MONEY_SCALE);
    assert_eq!(economy.reserved(owner(2), Currency::Uec), 8 * MONEY_SCALE);
    assert_eq!(
        economy
            .balances
            .values()
            .map(|b| b.uec + b.reserved_uec)
            .sum::<u64>(),
        200 * MONEY_SCALE
    );
    assert_eq!(
        economy
            .balances
            .values()
            .map(|b| b.lat + b.reserved_lat)
            .sum::<u64>(),
        10 * MONEY_SCALE
    );
}

#[test]
fn partially_filled_taker_preserves_original_and_executed_quantity() {
    let mut world = world();
    place(&mut world, 3, Side::Sell, MONEY_SCALE, 4 * MONEY_SCALE);
    let bid = place(&mut world, 1, Side::Buy, 3 * MONEY_SCALE, 4 * MONEY_SCALE);
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let order = &economy.exchange.orders[&bid];
    assert_eq!(order.original_quantity, 3 * MONEY_SCALE);
    assert_eq!(order.filled_quantity, MONEY_SCALE);
    assert_eq!(order.remaining, 2 * MONEY_SCALE);
}

#[test]
fn completed_and_cancelled_orders_survive_economy_persistence() {
    let mut world = world();
    let bid = place(&mut world, 1, Side::Buy, 3 * MONEY_SCALE, 4 * MONEY_SCALE);
    let sold = place(&mut world, 3, Side::Sell, MONEY_SCALE, 4 * MONEY_SCALE);
    crate::sim::society::submit(
        &mut world,
        Id([1; 16]),
        crate::sim::society::Action::Market(Id::new(), MarketCommand::Cancel { order: bid }),
    )
    .unwrap();
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let completed = economy
        .exchange
        .history
        .values()
        .find(|order| order.id == sold)
        .unwrap();
    assert_eq!(completed.status, OrderStatus::Completed);
    assert_eq!(completed.original_quantity, MONEY_SCALE);
    assert_eq!(completed.filled_quantity, MONEY_SCALE);
    let cancelled = economy
        .exchange
        .history
        .values()
        .find(|order| order.id == bid)
        .unwrap();
    assert_eq!(cancelled.status, OrderStatus::Cancelled);
    assert_eq!(cancelled.original_quantity, 3 * MONEY_SCALE);
    assert_eq!(cancelled.filled_quantity, MONEY_SCALE);
    assert_eq!(cancelled.remaining, 0);
    assert!(cancelled.closed_ms.is_some());
    let bytes = postcard::to_stdvec(economy).unwrap();
    let restored: Economy = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(restored.exchange.history, economy.exchange.history);
}

#[test]
fn immediate_remainders_and_history_retention_are_explicit() {
    let mut world = world();
    let id = Id::new();
    crate::sim::society::submit(
        &mut world,
        Id([1; 16]),
        crate::sim::society::Action::Market(
            id,
            MarketCommand::Immediate {
                instrument: Instrument::Fx,
                owner: owner(1),
                side: Side::Buy,
                quantity: MONEY_SCALE,
                price: 4 * MONEY_SCALE,
            },
        ),
    )
    .unwrap();
    let order = (&world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .exchange
        .history
        .values()
        .min_by_key(|order| order.sequence)
        .unwrap()
        .clone();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.filled_quantity, 0);
    let mut exchange = Exchange::default();
    for sequence in 0..=ORDER_HISTORY_LIMIT {
        let mut next = order.clone();
        next.sequence = sequence as u64;
        next.id = Id::new();
        exchange.archive(next, sequence as i64);
    }
    assert_eq!(exchange.history.len(), ORDER_HISTORY_LIMIT);
    assert_eq!(
        exchange
            .history
            .values()
            .min_by_key(|order| order.sequence)
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn separately_rounded_fills_stop_at_available_funds() {
    let mut world = world();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .transfer(
            None,
            owner(1),
            owner(2),
            Currency::Uec,
            100 * MONEY_SCALE - 9,
            false,
            now,
        )
        .unwrap();

    let first = place(&mut world, 3, Side::Sell, 1, 4_100_000);
    let second = place(&mut world, 3, Side::Sell, 1, 4_100_000);
    let bid = place(&mut world, 1, Side::Buy, 2, 4_100_000);

    // The combined quote is 9 micro UEC, but each individual fill costs 5.
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(economy.balances[&owner(1)].uec, 4);
    assert_eq!(economy.balances[&owner(1)].lat, 1);
    assert_eq!(economy.exchange.trades.len(), 1);
    assert!(!economy.exchange.orders.contains_key(&first));
    assert!(!economy.exchange.orders.contains_key(&bid));
    assert_eq!(economy.exchange.orders[&second].remaining, 1);
}

#[test]
fn restricted_receipts_take_better_bids_then_backstop() {
    let mut world = world();
    place(&mut world, 1, Side::Buy, 2 * MONEY_SCALE, 4 * MONEY_SCALE);
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .transfer(
            None,
            owner(3),
            owner(2),
            Currency::Lat,
            5 * MONEY_SCALE,
            true,
            now,
        )
        .unwrap();
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(economy.balances[&owner(2)].lat, 0);
    assert_eq!(economy.balances[&owner(2)].uec, 117_600_000);
    assert_eq!(economy.balances[&reserve_owner()].lat, 3 * MONEY_SCALE);
    assert_eq!(economy.exchange.trades.len(), 2);
    assert!(!economy.exchange.trades.get(&1).unwrap().backstop);
    assert!(economy.exchange.trades.get(&2).unwrap().backstop);
}

#[test]
fn reserved_money_cannot_be_spent_and_cancellation_requires_authority() {
    let mut world = world();
    let id = place(&mut world, 1, Side::Buy, 25 * MONEY_SCALE, 4 * MONEY_SCALE);
    assert!(
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .transfer(
                None,
                owner(1),
                owner(2),
                Currency::Uec,
                1,
                false,
                osg_model::calendar::now_unix_ms()
            )
            .is_err()
    );
    assert!(
        crate::sim::society::submit(
            &mut world,
            Id([2; 16]),
            crate::sim::society::Action::Market(Id::new(), MarketCommand::Cancel { order: id })
        )
        .is_err()
    );
    let receipt = Id::new();
    let command = MarketCommand::Cancel { order: id };
    crate::sim::society::submit(
        &mut world,
        Id([1; 16]),
        crate::sim::society::Action::Market(receipt, command.clone()),
    )
    .unwrap();
    let saved = postcard::to_stdvec(
        &world
            .resource::<crate::sim::society::SocietyState>()
            .economy,
    )
    .unwrap();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .economy = postcard::from_bytes::<Economy>(&saved).unwrap();
    assert!(
        crate::sim::society::submit(
            &mut world,
            Id([1; 16]),
            crate::sim::society::Action::Market(receipt, command)
        )
        .is_err()
    );
    assert_eq!(
        postcard::to_stdvec(
            &world
                .resource::<crate::sim::society::SocietyState>()
                .economy
        )
        .unwrap(),
        saved
    );
    assert_eq!(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .reserved(owner(1), Currency::Uec),
        0
    );
}

#[test]
fn overflow_leaves_all_balances_fills_and_orders_unchanged() {
    let mut world = world();
    place(&mut world, 1, Side::Buy, 2 * MONEY_SCALE, 4 * MONEY_SCALE);
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(
            owner(3),
            Currency::Uec,
            u64::MAX,
            osg_model::calendar::now_unix_ms(),
        )
        .unwrap();
    let before = postcard::to_stdvec(
        &world
            .resource::<crate::sim::society::SocietyState>()
            .economy,
    )
    .unwrap();
    assert!(
        crate::sim::society::submit(
            &mut world,
            Id([3; 16]),
            crate::sim::society::Action::Market(
                Id::new(),
                MarketCommand::Limit {
                    instrument: Instrument::Fx,
                    owner: owner(3),
                    side: Side::Sell,
                    quantity: MONEY_SCALE,
                    price: 3 * MONEY_SCALE,
                }
            )
        )
        .is_err()
    );
    assert_eq!(
        postcard::to_stdvec(
            &world
                .resource::<crate::sim::society::SocietyState>()
                .economy
        )
        .unwrap(),
        before
    );
}

#[test]
fn later_fill_failure_leaves_earlier_fills_and_full_history_unpublished() {
    let mut world = world();
    place(&mut world, 1, Side::Buy, MONEY_SCALE, 4 * MONEY_SCALE);
    place(&mut world, 2, Side::Buy, MONEY_SCALE, 4 * MONEY_SCALE);
    let now = osg_model::calendar::now_unix_ms();
    let mut economy = world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy);
    economy
        .issue(owner(3), Currency::Uec, u64::MAX - 6 * MONEY_SCALE, now)
        .unwrap();
    // Retention must not discard pre-plan history before commit.
    let template = economy.exchange.orders.values().next().unwrap().clone();
    for _ in 0..ORDER_HISTORY_LIMIT {
        let mut order = template.clone();
        order.id = Id::new();
        economy.exchange.archive(order, now);
    }
    let before = postcard::to_stdvec(&*economy).unwrap();
    drop(economy);

    let result = crate::sim::society::submit(
        &mut world,
        Id([3; 16]),
        crate::sim::society::Action::Market(
            Id::new(),
            MarketCommand::Immediate {
                instrument: Instrument::Fx,
                owner: owner(3),
                side: Side::Sell,
                quantity: 2 * MONEY_SCALE,
                price: 4 * MONEY_SCALE,
            },
        ),
    );
    assert!(result.is_err());
    assert_eq!(
        postcard::to_stdvec(
            &world
                .resource::<crate::sim::society::SocietyState>()
                .economy
        )
        .unwrap(),
        before
    );
}

#[test]
fn demurrage_exempts_reserved_buckets_and_queries_require_authority() {
    let mut economy = Economy::at(0);
    economy
        .issue(owner(1), Currency::Uec, 100_000 * MONEY_SCALE, 0)
        .unwrap();
    economy
        .execute_order(
            Instrument::Fx,
            None,
            Id::new(),
            owner(1),
            Side::Buy,
            25_000 * MONEY_SCALE,
            4 * MONEY_SCALE,
            true,
            0,
        )
        .unwrap();
    assert_eq!(
        economy.reserved(owner(1), Currency::Uec),
        100_000 * MONEY_SCALE
    );
    economy.settle(DAY_MS);
    assert_eq!(economy.exchange.orders.len(), 1);
    assert_eq!(economy.balances[&owner(1)].uec, 0);
    assert_eq!(
        economy.reserved(owner(1), Currency::Uec),
        100_000 * MONEY_SCALE
    );

    let world = world();
    assert!(
        crate::rpc::list_orders(
            &world,
            Id([2; 16]),
            owner(1),
            Some(Instrument::Fx),
            Some(OrderStatus::Open),
            None,
            100,
        )
        .is_err()
    );
}
