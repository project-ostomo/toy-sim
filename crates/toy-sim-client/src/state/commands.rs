use bevy::prelude::*;
use toy_sim_model::{Action, Id, ShipCommand, ShipTelemetry};

#[derive(Resource, Default)]
pub(crate) struct Outgoing {
    actions: Vec<(Id, Action)>,
}

impl Outgoing {
    pub(crate) fn push(&mut self, action: Action) -> Id {
        if let Action::Ship {
            ship,
            command: ShipCommand::Manual { .. },
            ..
        } = &action
        {
            self.actions.retain(|(_, pending)| {
                !matches!(pending,
                    Action::Ship { ship: previous, command: ShipCommand::Manual { .. }, .. }
                        if previous == ship
                )
            });
        }
        let id = Id::new();
        self.actions.push((id, action));
        id
    }

    pub(crate) fn ship(&mut self, ship: &ShipTelemetry, command: ShipCommand) -> Id {
        self.push(Action::Ship {
            ship: ship.ship,
            authority_revision: ship.authority_revision,
            command,
        })
    }

    pub(crate) fn clear(&mut self) {
        self.actions.clear();
    }

    pub(super) fn take(&mut self) -> Vec<(Id, Action)> {
        std::mem::take(&mut self.actions)
    }

    pub(super) fn restore(&mut self, actions: Vec<(Id, Action)>) {
        self.actions.splice(0..0, actions);
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[(Id, Action)] {
        &self.actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backpressure_keeps_command_ids_and_coalesces_only_manual_inputs_for_the_same_ship() {
        let mut queue = Outgoing::default();
        let first = Id([1; 16]);
        let second = Id([2; 16]);
        let manual = |ship, throttle| Action::Ship {
            ship,
            authority_revision: 7,
            command: ShipCommand::Manual {
                throttle,
                steering: [0.; 3],
            },
        };
        queue.push(manual(first, 0.1));
        let other = queue.push(manual(second, 0.2));
        let discrete = queue.push(Action::Ship {
            ship: first,
            authority_revision: 7,
            command: ShipCommand::HoldFire,
        });
        queue.push(manual(first, 0.3));
        let pending = queue.take();
        queue.restore(pending);
        let latest = queue.push(manual(first, 0.4));
        assert_eq!(
            queue
                .pending()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![other, discrete, latest]
        );
        assert!(matches!(
            queue.pending()[2].1,
            Action::Ship {
                authority_revision: 7,
                command: ShipCommand::Manual { throttle: 0.4, .. },
                ..
            }
        ));
    }
}
