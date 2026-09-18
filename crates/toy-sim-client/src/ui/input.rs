use super::Selection;
use crate::state::{Outgoing, OwnedShip};
use bevy::prelude::*;
use toy_sim_model::{ShipCommand, ShipTelemetry};
use toy_sim_ui::bevy_egui::EguiContexts;

#[derive(Resource, Default)]
pub(super) struct FlightControls {
    pub throttle: f64,
    pub steering: [f64; 3],
}

pub(super) fn release_manual(
    flight: &mut FlightControls,
    outgoing: &mut Outgoing,
    ship: &ShipTelemetry,
) {
    let throttle = flight.throttle;
    outgoing.ship(
        ship,
        ShipCommand::Manual {
            throttle,
            steering: [0.; 3],
        },
    );
    *flight = FlightControls::default();
}

pub(super) fn manual(
    mut flight: ResMut<FlightControls>,
    selection: Res<Selection>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<&OwnedShip>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut contexts: EguiContexts,
) {
    let captured = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.egui_wants_keyboard_input());
    let Some(ship) = ships
        .iter()
        .map(|ship| &ship.0)
        .find(|ship| Some(ship.ship) == selection.ship)
    else {
        return;
    };
    let previous = (flight.throttle, flight.steering);
    flight.steering = [0.; 3];
    if !captured {
        let axis = |positive, negative| {
            f64::from(keys.pressed(positive)) - f64::from(keys.pressed(negative))
        };
        flight.throttle = (flight.throttle
            + axis(KeyCode::ShiftLeft, KeyCode::ControlLeft) * time.delta_secs_f64() / 2.)
            .clamp(0., 1.);
        flight.steering = [
            axis(KeyCode::KeyS, KeyCode::KeyW),
            axis(KeyCode::KeyA, KeyCode::KeyD),
            axis(KeyCode::KeyQ, KeyCode::KeyE),
        ];
    }
    let next = (flight.throttle, flight.steering);
    if previous != next {
        outgoing.ship(
            ship,
            ShipCommand::Manual {
                throttle: next.0,
                steering: next.1,
            },
        );
    }
}
