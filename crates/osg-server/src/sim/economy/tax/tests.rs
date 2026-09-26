use super::super::*;
use osg_model::{
    Id,
    market::{Instrument, MarketCommand, Side},
};

#[test]
fn treaty_tariff_is_collected_on_transfers_and_rounds_up() {
    use osg_model::diplomacy::{Agreement, AgreementStatus, AgreementTerm};
    let mut world = World::new();
    let payer = Id([1; 16]);
    let recipient = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[payer, recipient]);
    let from = Principal::Player(payer);
    let to = Principal::Player(recipient);
    let id = Id::new();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.directory)
        .0
        .diplomacy
        .agreements
        .insert(
            id,
            Agreement {
                id,
                from,
                to,
                title: "Tariff".into(),
                terms: vec![AgreementTerm::Tariff { basis_points: 200 }],
                note: String::new(),
                status: AgreementStatus::Active,
                revision: 2,
            },
        );
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(from, Currency::Uec, 100, now)
        .unwrap();
    let directory = (&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0
        .clone();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .transfer(Some(&directory), from, to, Currency::Uec, 100, false, now)
        .unwrap();
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(economy.balances[&from].uec, 2);
    assert_eq!(economy.balances[&to].uec, 98);
}

#[test]
fn transfer_receipts_are_taxed_and_micro_transfers_round_up() {
    let mut world = World::new();
    let payer = Id([1; 16]);
    let recipient = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[payer, recipient]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    {
        let records = &mut world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.directory)
            .0
            .sovereignties;
        let mut record = records.get(&sovereignty).unwrap().clone();
        record.officers.insert(payer);
        records.insert(sovereignty, record);
    }
    let tax = WalletCommand::SetTurnoverTax {
        sovereignty,
        basis_points: 1000,
    };
    assert!(wallet(&mut world, recipient, tax.clone()).is_err());
    wallet(&mut world, payer, tax).unwrap();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(
            Principal::Player(payer),
            Currency::Uec,
            100 * MONEY_SCALE,
            now,
        )
        .unwrap();
    for _ in 0..3 {
        wallet(
            &mut world,
            payer,
            WalletCommand::Transfer {
                from: Principal::Player(payer),
                to: Principal::Player(recipient),
                currency: Currency::Uec,
                amount: 1,
            },
        )
        .unwrap();
    }
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(economy.balances[&Principal::Player(recipient)].uec, 0);
    assert_eq!(
        economy.balances[&Principal::Sovereignty(sovereignty)].uec,
        3
    );
}

#[test]
fn fx_buyer_pays_tax_at_placement_and_fills_deliver_full_amounts() {
    let mut world = World::new();
    let buyer = Id([1; 16]);
    let seller = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[buyer, seller]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    let now = osg_model::calendar::now_unix_ms();
    let mut economy = world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy);
    economy.turnover_taxes.insert(sovereignty, 1000);
    economy
        .issue(
            Principal::Player(buyer),
            Currency::Uec,
            100 * MONEY_SCALE,
            now,
        )
        .unwrap();
    economy
        .issue(
            Principal::Player(seller),
            Currency::Lat,
            10 * MONEY_SCALE,
            now,
        )
        .unwrap();
    drop(economy);
    let treasury = Principal::Sovereignty(sovereignty);
    for (account, side) in [(buyer, Side::Buy), (seller, Side::Sell)] {
        crate::sim::society::submit(
            &mut world,
            account,
            crate::sim::society::Action::Market(
                Id::new(),
                MarketCommand::Limit {
                    instrument: Instrument::Fx,
                    owner: Principal::Player(account),
                    side,
                    quantity: 10 * MONEY_SCALE,
                    price: 4 * MONEY_SCALE,
                },
            ),
        )
        .unwrap();
        if side == Side::Buy {
            let economy = &world
                .resource::<crate::sim::society::SocietyState>()
                .economy;
            assert!(economy.exchange.trades.values().next().is_none());
            assert_eq!(economy.available(treasury, Currency::Uec), 4 * MONEY_SCALE);
            assert_eq!(
                economy.reserved(Principal::Player(buyer), Currency::Uec),
                40 * MONEY_SCALE
            );
            assert_eq!(
                economy.available(Principal::Player(buyer), Currency::Uec),
                56 * MONEY_SCALE
            );
        }
    }
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(
        economy.balances[&Principal::Player(buyer)].lat,
        10 * MONEY_SCALE
    );
    assert_eq!(
        economy.balances[&Principal::Player(seller)].uec,
        40 * MONEY_SCALE
    );
    assert_eq!(economy.available(treasury, Currency::Uec), 4 * MONEY_SCALE);
    assert_eq!(
        economy
            .balances
            .values()
            .map(|balance| balance.uec)
            .sum::<u64>(),
        100 * MONEY_SCALE
    );
    assert_eq!(
        economy
            .balances
            .values()
            .map(|balance| balance.lat)
            .sum::<u64>(),
        10 * MONEY_SCALE
    );
}

#[test]
fn restricted_receipt_is_taxed_once_and_automatic_sell_has_no_tax() {
    let mut world = World::new();
    let sender = Id([1; 16]);
    let recipient = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[sender, recipient]);
    let use_id = crate::sim::ownership::sovereignty_id("USE");
    let organization = (&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0
        .organizations
        .values()
        .find(|organization| organization.sovereignty == use_id)
        .unwrap()
        .id;
    crate::sim::ownership::affiliate(&mut world, recipient, Some(organization)).unwrap();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .turnover_taxes
        .insert(use_id, 1000);
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(
            Principal::Player(sender),
            Currency::Lat,
            10 * MONEY_SCALE,
            now,
        )
        .unwrap();
    wallet(
        &mut world,
        sender,
        WalletCommand::Transfer {
            from: Principal::Player(sender),
            to: Principal::Player(recipient),
            currency: Currency::Lat,
            amount: 10 * MONEY_SCALE,
        },
    )
    .unwrap();
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    assert_eq!(economy.balances[&Principal::Player(recipient)].lat, 0);
    assert_eq!(
        economy.balances[&Principal::Player(recipient)].uec,
        28_800_000
    );
    assert_eq!(
        economy.balances[&Principal::Sovereignty(use_id)].lat,
        10 * MONEY_SCALE
    );
    assert_eq!(economy.balances[&Principal::Sovereignty(use_id)].uec, 0);
}

#[test]
fn partial_fills_and_cancellation_keep_the_original_placement_tax() {
    use crate::sim::society::{SocietyState, submit};

    let mut world = World::new();
    let buyer = Id([1; 16]);
    let seller = Id([2; 16]);
    let buyer_owner = Principal::Player(buyer);
    let seller_owner = Principal::Player(seller);
    crate::sim::identity::initialize(&mut world, &[buyer, seller]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    let treasury = Principal::Sovereignty(sovereignty);
    let now = osg_model::calendar::now_unix_ms();
    {
        let mut society = world.resource_mut::<SocietyState>();
        society.economy.turnover_taxes.insert(sovereignty, 1000);
        society
            .economy
            .issue(buyer_owner, Currency::Uec, 100, now)
            .unwrap();
        society
            .economy
            .issue(seller_owner, Currency::Uec, 10, now)
            .unwrap();
        society
            .economy
            .issue(seller_owner, Currency::Lat, 10, now)
            .unwrap();
    }
    let bid = Id::new();
    submit(
        &mut world,
        buyer,
        crate::sim::society::Action::Market(
            bid,
            MarketCommand::Limit {
                instrument: Instrument::Fx,
                owner: buyer_owner,
                side: Side::Buy,
                quantity: 10,
                price: 4 * MONEY_SCALE,
            },
        ),
    )
    .unwrap();
    assert_eq!(
        world
            .resource::<SocietyState>()
            .economy
            .available(treasury, Currency::Uec),
        4
    );

    // A later rate change does not charge the maker again on either partial fill.
    // Incoming sell orders pay no tax.
    world
        .resource_mut::<SocietyState>()
        .economy
        .turnover_taxes
        .insert(sovereignty, 2000);
    for _ in 0..2 {
        submit(
            &mut world,
            seller,
            MarketCommand::Immediate {
                instrument: Instrument::Fx,
                owner: seller_owner,
                side: Side::Sell,
                quantity: 2,
                price: 4 * MONEY_SCALE,
            }
            .into(),
        )
        .unwrap();
    }
    submit(
        &mut world,
        buyer,
        MarketCommand::Cancel { order: bid }.into(),
    )
    .unwrap();
    let economy = &world.resource::<SocietyState>().economy;
    assert_eq!(economy.available(treasury, Currency::Uec), 4);
    assert_eq!(economy.available(buyer_owner, Currency::Uec), 80);
    assert_eq!(economy.available(buyer_owner, Currency::Lat), 4);
    assert_eq!(economy.reserved(buyer_owner, Currency::Uec), 0);
    assert_eq!(economy.exchange.trades.len(), 2);
    assert_eq!(
        economy
            .entries
            .query((buyer_owner, 0)..=(buyer_owner, u64::MAX))
            .filter(|entry| entry.kind == EntryKind::Tax)
            .count(),
        1
    );
}

#[test]
fn rejected_order_charges_nothing_and_unfilled_immediate_order_pays_rounded_tax() {
    use crate::sim::society::{SocietyState, submit};

    let mut world = World::new();
    let account = Id([1; 16]);
    let owner = Principal::Player(account);
    crate::sim::identity::initialize(&mut world, &[account]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    let now = osg_model::calendar::now_unix_ms();
    {
        let mut society = world.resource_mut::<SocietyState>();
        society.economy.turnover_taxes.insert(sovereignty, 1);
        society.economy.issue(owner, Currency::Uec, 1, now).unwrap();
    }
    let order = MarketCommand::Immediate {
        instrument: Instrument::Fx,
        owner,
        side: Side::Buy,
        quantity: 1,
        price: MONEY_SCALE,
    };
    let before = postcard::to_stdvec(&world.resource::<SocietyState>().economy).unwrap();
    assert!(submit(&mut world, account, order.clone().into()).is_err());
    assert_eq!(
        postcard::to_stdvec(&world.resource::<SocietyState>().economy).unwrap(),
        before
    );

    world
        .resource_mut::<SocietyState>()
        .economy
        .issue(owner, Currency::Uec, 1, now)
        .unwrap();
    submit(&mut world, account, order.into()).unwrap();
    let society = world.resource::<SocietyState>();
    assert_eq!(society.economy.available(owner, Currency::Uec), 1);
    assert_eq!(
        society
            .economy
            .available(Principal::Sovereignty(sovereignty), Currency::Uec),
        1
    );
    assert_eq!(society.economy.exchange.trades.len(), 0);
    assert!(society.economy.exchange.orders.is_empty());
}

fn wallet(world: &mut World, account: Id, command: WalletCommand) -> Result<()> {
    crate::sim::society::submit(world, account, command.into())
}
