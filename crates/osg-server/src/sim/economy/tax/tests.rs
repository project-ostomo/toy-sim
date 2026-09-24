use super::*;
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
        .resource_mut::<Directory>()
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
        .resource_mut::<Economy>()
        .issue(from, Currency::Uec, 100, now)
        .unwrap();
    let directory = world.resource::<Directory>().0.clone();
    world
        .resource_mut::<Economy>()
        .transfer(Some(&directory), from, to, Currency::Uec, 100, false, now)
        .unwrap();
    let economy = world.resource::<Economy>();
    assert_eq!(economy.balances[&from].uec, 2);
    assert_eq!(economy.balances[&to].uec, 98);
    economy.validate(&directory).unwrap();
}

#[test]
fn every_receipt_is_taxed_and_micro_transfers_round_up() {
    let mut world = World::new();
    let payer = Id([1; 16]);
    let recipient = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[payer, recipient]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    world
        .resource_mut::<Directory>()
        .0
        .sovereignties
        .get_mut(&sovereignty)
        .unwrap()
        .officers
        .insert(payer);
    let tax = WalletCommand::SetTurnoverTax {
        sovereignty,
        basis_points: 1000,
    };
    assert!(super::super::apply(&mut world, recipient, Id::new(), tax.clone()).is_err());
    super::super::apply(&mut world, payer, Id::new(), tax).unwrap();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<Economy>()
        .issue(
            Principal::Player(payer),
            Currency::Uec,
            100 * MONEY_SCALE,
            now,
        )
        .unwrap();
    for _ in 0..3 {
        super::super::apply(
            &mut world,
            payer,
            Id::new(),
            WalletCommand::Transfer {
                from: Principal::Player(payer),
                to: Principal::Player(recipient),
                currency: Currency::Uec,
                amount: 1,
            },
        )
        .unwrap();
    }
    let economy = world.resource::<Economy>();
    assert_eq!(economy.balances[&Principal::Player(recipient)].uec, 0);
    assert_eq!(
        economy.balances[&Principal::Sovereignty(sovereignty)].uec,
        3
    );
    economy.validate(&world.resource::<Directory>().0).unwrap();
}

#[test]
fn fx_taxes_both_currency_receipts_and_conserves_totals() {
    let mut world = World::new();
    let buyer = Id([1; 16]);
    let seller = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[buyer, seller]);
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    let now = osg_model::calendar::now_unix_ms();
    let mut economy = world.resource_mut::<Economy>();
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
    for (account, side) in [(buyer, Side::Buy), (seller, Side::Sell)] {
        super::super::exchange::apply(
            &mut world,
            account,
            Id::new(),
            MarketCommand::Limit {
                instrument: Instrument::Fx,
                owner: Principal::Player(account),
                side,
                quantity: 10 * MONEY_SCALE,
                price: 4 * MONEY_SCALE,
            },
        )
        .unwrap();
    }
    let economy = world.resource::<Economy>();
    assert_eq!(
        economy.balances[&Principal::Player(buyer)].lat,
        9 * MONEY_SCALE
    );
    assert_eq!(
        economy.balances[&Principal::Player(seller)].uec,
        36 * MONEY_SCALE
    );
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
    economy.validate(&world.resource::<Directory>().0).unwrap();
}

#[test]
fn restricted_receipt_and_backstop_conversion_are_both_taxed() {
    let mut world = World::new();
    let sender = Id([1; 16]);
    let recipient = Id([2; 16]);
    crate::sim::identity::initialize(&mut world, &[sender, recipient]);
    let use_id = crate::sim::ownership::sovereignty_id("USE");
    let organization = world
        .resource::<Directory>()
        .0
        .organizations
        .values()
        .find(|organization| organization.sovereignty == use_id)
        .unwrap()
        .id;
    crate::sim::ownership::affiliate(&mut world, recipient, Some(organization)).unwrap();
    let now = osg_model::calendar::now_unix_ms();
    world
        .resource_mut::<Economy>()
        .turnover_taxes
        .insert(use_id, 1000);
    world
        .resource_mut::<Economy>()
        .issue(
            Principal::Player(sender),
            Currency::Lat,
            10 * MONEY_SCALE,
            now,
        )
        .unwrap();
    super::super::apply(
        &mut world,
        sender,
        Id::new(),
        WalletCommand::Transfer {
            from: Principal::Player(sender),
            to: Principal::Player(recipient),
            currency: Currency::Lat,
            amount: 10 * MONEY_SCALE,
        },
    )
    .unwrap();
    let economy = world.resource::<Economy>();
    assert_eq!(economy.balances[&Principal::Player(recipient)].lat, 0);
    assert_eq!(
        economy.balances[&Principal::Player(recipient)].uec,
        25_920_000
    );
    assert_eq!(
        economy.balances[&Principal::Sovereignty(use_id)].lat,
        10 * MONEY_SCALE
    );
    assert_eq!(
        economy.balances[&Principal::Sovereignty(use_id)].uec,
        2_880_000
    );
    economy.validate(&world.resource::<Directory>().0).unwrap();
}
