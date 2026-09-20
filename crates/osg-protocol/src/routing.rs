use super::*;

pub fn validate_request(request: &osg_model::routing::Request) -> Result<()> {
    ensure!(request.id != 0, "invalid route request id");
    ensure!(request.preferences.valid(), "invalid planning preference");
    ensure!(
        request.orders.len() <= osg_model::routing::MAX_ORDERS,
        "too many requested waypoints"
    );
    for order in &request.orders {
        validate_order(order)?;
    }
    Ok(())
}

pub fn validate_status(status: &osg_model::routing::Status) -> Result<()> {
    use osg_model::routing::{MAX_ORDERS, Status};

    match status {
        Status::Unknown => {}
        Status::Pending { progress } => ensure!(
            progress
                .total
                .is_none_or(|total| progress.completed <= total),
            "invalid route progress"
        ),
        Status::Failed { reason } => ensure!(
            !reason.trim().is_empty() && reason.len() <= 1024,
            "invalid route failure"
        ),
        Status::Ready { plan } => {
            ensure!(
                plan.orders.len() <= MAX_ORDERS,
                "too many planned waypoints"
            );
            ensure!(plan.fuel_budget.valid(), "invalid route fuel budget");
            ensure!(
                plan.estimated_loss_ppm.is_finite()
                    && (0. ..=1_000_000.).contains(&plan.estimated_loss_ppm)
                    && plan.exotic_fuel_kg.is_finite()
                    && plan.exotic_fuel_kg >= 0.
                    && plan.beacon_assumptions.len() <= MAX_ORDERS,
                "invalid route risk or exotic fuel estimate"
            );
            for order in &plan.orders {
                validate_queued_order(order)?;
                ensure!(
                    !matches!(
                        order.action,
                        travel::Order::TravelTo(_) | travel::Order::TravelToSystem(_)
                    ),
                    "planned route contains an unexpanded destination"
                );
                ensure!(
                    order
                        .estimated_propellant_kg
                        .is_none_or(|kg| kg.is_finite() && kg >= 0.),
                    "invalid route propellant estimate"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::routing::{Plan, Request, Status};

    #[test]
    fn preview_poll_and_commit_round_trip_with_explicit_authority() {
        let ship = Id::new();
        let input = InputFrame {
            world: Id::new(),
            sequence: 4,
            actions: vec![
                (
                    Id::new(),
                    Action::RouteRequest {
                        ship,
                        authority_revision: 8,
                        request: Request {
                            id: 9,
                            orders: vec![travel::Order::Dock(Id::new())],
                            preferences: Default::default(),
                        },
                    },
                ),
                (
                    Id::new(),
                    Action::RoutePoll {
                        ship,
                        authority_revision: 8,
                        id: 9,
                    },
                ),
                (
                    Id::new(),
                    Action::RouteCancel {
                        ship,
                        authority_revision: 8,
                        id: 10,
                    },
                ),
                (
                    Id::new(),
                    Action::Ship {
                        ship,
                        authority_revision: 8,
                        command: ShipCommand::UseRoute {
                            id: 9,
                            expected_revision: 3,
                            engage: true,
                        },
                    },
                ),
            ],
        };
        let decoded = decode(&encode(&Message::Input(input.clone())).unwrap()).unwrap();
        assert_eq!(decoded, Message::Input(input));
    }

    #[test]
    fn validates_preview_payloads_and_rejects_unexpanded_or_oversized_plans() {
        let mut request = Request {
            id: 1,
            orders: vec![travel::Order::TravelTo(travel::Destination::Galactic(
                GalacticPosition::ZERO,
            ))],
            preferences: Default::default(),
        };
        validate_request(&request).unwrap();
        request.id = 0;
        assert!(validate_request(&request).is_err());
        request.id = 1;
        request.preferences.max_loss_ppm = f64::NAN;
        assert!(validate_request(&request).is_err());
        request.preferences.max_loss_ppm = 100.;
        request.orders.resize(257, travel::Order::Undock);
        assert!(validate_request(&request).is_err());

        let mut plan = Plan {
            planned_tick: 5,
            travel_revision: 2,
            topology_revision: 7,
            orders: vec![
                travel::Order::Sublight(travel::Destination::Galactic(GalacticPosition::ZERO))
                    .into(),
            ],
            fuel_budget: Default::default(),
            estimated_loss_ppm: 100.,
            exotic_fuel_kg: 1.,
            beacon_assumptions: Vec::new(),
        };
        validate_status(&Status::Ready { plan: plan.clone() }).unwrap();
        plan.estimated_loss_ppm = 1_000_001.;
        assert!(validate_status(&Status::Ready { plan: plan.clone() }).is_err());
        plan.estimated_loss_ppm = 100.;
        plan.orders[0].action =
            travel::Order::TravelTo(travel::Destination::Galactic(GalacticPosition::ZERO));
        assert!(validate_status(&Status::Ready { plan: plan.clone() }).is_err());
        plan.orders[0].action = travel::Order::Undock;
        plan.orders[0].estimated_propellant_kg = Some(f64::NAN);
        assert!(validate_status(&Status::Ready { plan: plan.clone() }).is_err());
        plan.orders[0].estimated_propellant_kg = Some(0.);
        plan.orders.resize(257, travel::Order::Undock.into());
        assert!(validate_status(&Status::Ready { plan }).is_err());
    }
}
