use super::*;

fn owner(n: u8) -> Principal {
    Principal::Player(osg_model::Id([n; 16]))
}

#[test]
fn daily_compounding_retains_eighty_percent_in_both_calendar_year_lengths() {
    // 1970 and leap year 1972; exemption is retained throughout compounding.
    for (start, days) in [(0, 365), (730, 366)] {
        let mut economy = Economy::at(start * DAY_MS);
        economy
            .issue(
                owner(1),
                Currency::Uec,
                DEMURRAGE_EXEMPTION + 1_000_000 * MONEY_SCALE,
                start * DAY_MS,
            )
            .unwrap();
        economy.settle((start + days) * DAY_MS);
        let expected = DEMURRAGE_EXEMPTION + 800_000 * MONEY_SCALE;
        assert!(economy.balances[&owner(1)].uec.abs_diff(expected) <= days as u64);
        assert_eq!(economy.entries.len(), days as usize + 1);
        let balance = economy.balances[&owner(1)].uec;
        economy.settle((start + days) * DAY_MS);
        economy.settle(start * DAY_MS);
        assert_eq!(economy.balances[&owner(1)].uec, balance);
    }
}

#[test]
fn threshold_rounding_and_checkpoint_catchup() {
    let mut economy = Economy::at(0);
    economy
        .issue(owner(1), Currency::Uec, DEMURRAGE_EXEMPTION, 0)
        .unwrap();
    economy.settle(DAY_MS);
    assert_eq!(economy.balances[&owner(1)].uec, DEMURRAGE_EXEMPTION);
    economy.issue(owner(1), Currency::Uec, 1, DAY_MS).unwrap();
    economy.settle(2 * DAY_MS);
    assert_eq!(economy.balances[&owner(1)].uec, DEMURRAGE_EXEMPTION);
    let bytes = postcard::to_stdvec(&economy).unwrap();
    let mut restored: Economy = postcard::from_bytes(&bytes).unwrap();
    restored.settle(365 * DAY_MS);
    for day in 3..=365 {
        economy.settle(day * DAY_MS);
    }
    assert_eq!(
        postcard::to_stdvec(&economy).unwrap(),
        postcard::to_stdvec(&restored).unwrap()
    );
}

#[test]
fn transfers_are_atomic_and_conversion_preserves_lat_in_reserve() {
    let mut economy = Economy::at(0);
    economy
        .issue(owner(1), Currency::Lat, 10 * MONEY_SCALE, 0)
        .unwrap();
    let before = postcard::to_stdvec(&economy).unwrap();
    assert!(
        economy
            .transfer(
                None,
                owner(1),
                owner(2),
                Currency::Lat,
                11 * MONEY_SCALE,
                true,
                0
            )
            .is_err()
    );
    assert_eq!(postcard::to_stdvec(&economy).unwrap(), before);
    economy
        .transfer(
            None,
            owner(1),
            owner(2),
            Currency::Lat,
            10 * MONEY_SCALE,
            true,
            0,
        )
        .unwrap();
    assert_eq!(economy.balances[&owner(1)].lat, 0);
    assert_eq!(economy.balances[&owner(2)].uec, 32 * MONEY_SCALE);
    let reserve = Principal::Sovereignty(super::super::ownership::sovereignty_id("USE"));
    assert_eq!(economy.balances[&reserve].lat, 10 * MONEY_SCALE);
    economy
        .transfer(None, reserve, owner(2), Currency::Lat, MONEY_SCALE, true, 0)
        .unwrap();
    assert_eq!(economy.balances[&reserve].lat, 10 * MONEY_SCALE);
}

#[test]
fn failed_service_conversion_restores_reservation_and_ledger() {
    let mut economy = Economy::at(0);
    economy
        .issue(owner(1), Currency::Lat, MONEY_SCALE, 0)
        .unwrap();
    economy.issue(owner(2), Currency::Uec, u64::MAX, 0).unwrap();
    let id = osg_model::Id::new();
    economy.service_holds.insert(
        id,
        osg_model::industry::ServicePayment {
            payer: owner(1),
            operator: owner(2),
            currency: Currency::Lat,
            amount: MONEY_SCALE,
            charged: false,
        },
    );
    let before = postcard::to_stdvec(&economy).unwrap();
    let result = economy.transaction(|transaction| {
        transaction.release_service_hold(id).unwrap();
        transaction.transfer(
            None,
            owner(1),
            owner(2),
            Currency::Lat,
            MONEY_SCALE,
            true,
            0,
        )
    });
    assert!(result.is_err());
    assert_eq!(postcard::to_stdvec(&economy).unwrap(), before);
}

#[test]
fn transaction_unwind_restores_successful_transfer() {
    let mut economy = Economy::at(0);
    economy.issue(owner(1), Currency::Uec, 100, 0).unwrap();
    let before = postcard::to_stdvec(&economy).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<()> = economy.transaction(|transaction| {
            transaction.transfer(None, owner(1), owner(2), Currency::Uec, 100, false, 0)?;
            panic!("interrupt settlement");
        });
    }));
    assert!(result.is_err());
    assert_eq!(postcard::to_stdvec(&economy).unwrap(), before);
}

#[test]
fn recipient_overflow_does_not_debit_source() {
    let mut economy = Economy::at(0);
    economy.issue(owner(1), Currency::Uec, 1, 0).unwrap();
    economy.issue(owner(2), Currency::Uec, u64::MAX, 0).unwrap();
    let before = postcard::to_stdvec(&economy).unwrap();
    assert!(
        economy
            .transfer(None, owner(1), owner(2), Currency::Uec, 1, false, 0)
            .is_err()
    );
    assert_eq!(postcard::to_stdvec(&economy).unwrap(), before);
}

#[test]
fn membership_does_not_grant_wallet_control_and_revocation_clears_snapshot() {
    let mut world = World::new();
    let account = osg_model::Id([1; 16]);
    let other = osg_model::Id([2; 16]);
    super::super::identity::initialize(&mut world, &[account, other]);
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<Economy>()
        .issue(owner(1), Currency::Uec, MONEY_SCALE, now)
        .unwrap();
    assert!(
        apply(
            &mut world,
            other,
            osg_model::Id::new(),
            WalletCommand::Transfer {
                from: owner(1),
                to: owner(2),
                currency: Currency::Uec,
                amount: MONEY_SCALE,
            }
        )
        .is_err()
    );
    let subscription = WalletQuery {
        owner: owner(1),
        before: None,
        limit: 10,
    };
    assert_eq!(snapshot(&world, account, &subscription).entries.len(), 1);
    let denied = snapshot(&world, other, &subscription);
    assert!(denied.error.is_some());
    assert!(denied.entries.is_empty());
    assert!(denied.balances.is_empty());
}

#[test]
fn successful_transfer_receipts_survive_restart() {
    let mut world = World::new();
    let account = osg_model::Id([1; 16]);
    super::super::identity::initialize(&mut world, &[account, osg_model::Id([2; 16])]);
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<Economy>()
        .issue(owner(1), Currency::Uec, 10 * MONEY_SCALE, now)
        .unwrap();
    let id = osg_model::Id::new();
    let command = WalletCommand::Transfer {
        from: owner(1),
        to: owner(2),
        currency: Currency::Uec,
        amount: MONEY_SCALE,
    };
    apply(&mut world, account, id, command.clone()).unwrap();
    let saved = postcard::to_stdvec(world.resource::<Economy>()).unwrap();
    world.insert_resource(postcard::from_bytes::<Economy>(&saved).unwrap());
    apply(&mut world, account, id, command).unwrap();
    assert_eq!(
        world.resource::<Economy>().balances[&owner(2)].uec,
        MONEY_SCALE
    );
    assert_eq!(
        postcard::to_stdvec(world.resource::<Economy>()).unwrap(),
        saved
    );
}

#[test]
fn world_checkpoint_restores_wallet_and_rejects_replayed_transfer() {
    let account = osg_model::Id([1; 16]);
    let other = osg_model::Id([2; 16]);
    let mut app = crate::scenario(&[account, other], Some(account), None).unwrap();
    let world = app.world_mut();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<Economy>()
        .issue(owner(1), Currency::Uec, 100_000 * MONEY_SCALE, now)
        .unwrap();
    let id = osg_model::Id::new();
    let command = WalletCommand::Transfer {
        from: owner(1),
        to: owner(2),
        currency: Currency::Uec,
        amount: MONEY_SCALE,
    };
    apply(world, account, id, command.clone()).unwrap();
    let order_id = osg_model::Id::new();
    exchange::apply(
        world,
        account,
        order_id,
        osg_model::market::MarketCommand::Limit {
            instrument: osg_model::market::Instrument::Fx,
            owner: owner(1),
            side: osg_model::market::Side::Buy,
            quantity: MONEY_SCALE,
            price: 4 * MONEY_SCALE,
        },
    )
    .unwrap();
    let money = postcard::to_stdvec(world.resource::<Economy>()).unwrap();
    let checkpoint = crate::persistence::world::capture(world).unwrap();
    world.resource_mut::<Economy>().balances.clear();
    crate::persistence::world::restore(world, &checkpoint).unwrap();
    apply(world, account, id, command).unwrap();
    assert!(
        world
            .resource::<Economy>()
            .exchange
            .orders
            .contains_key(&order_id)
    );
    assert_eq!(
        postcard::to_stdvec(world.resource::<Economy>()).unwrap(),
        money
    );
}
