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
            authority_revision,
            command: ShipCommand::SetThrottle(value),
        } = &action
        {
            if let Some((
                id,
                Action::Ship {
                    ship: last_ship,
                    authority_revision: last_revision,
                    command: ShipCommand::SetThrottle(last),
                },
            )) = self.actions.last_mut()
            {
                if ship == last_ship && authority_revision == last_revision {
                    *last = *value;
                    return *id;
                }
            }
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
    fn throttle_coalesces_without_crossing_control_transitions() {
        let mut queue = Outgoing::default();
        let action = |command| Action::Ship {
            ship: Id([1; 16]),
            authority_revision: 1,
            command,
        };
        let first = queue.push(action(ShipCommand::SetThrottle(0.2)));
        assert_eq!(queue.push(action(ShipCommand::SetThrottle(0.5))), first);
        queue.push(action(ShipCommand::SetAutopilot(true)));
        queue.push(action(ShipCommand::SetThrottle(0.8)));
        assert_eq!(queue.pending().len(), 3);
        assert!(matches!(
            queue.pending()[0].1,
            Action::Ship {
                command: ShipCommand::SetThrottle(0.5),
                ..
            }
        ));
    }

    #[test]
    fn backpressure_preserves_command_order_and_ids() {
        let mut queue = Outgoing::default();
        let ship = Id([1; 16]);
        let action = |command| Action::Ship {
            ship,
            authority_revision: 7,
            command,
        };
        let first = queue.push(action(ShipCommand::StartFiring));
        let second = queue.push(action(ShipCommand::StopFiring));
        let pending = queue.take();
        let third = queue.push(action(ShipCommand::UnmarkTarget));
        queue.restore(pending);

        assert_eq!(
            queue
                .pending()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![first, second, third]
        );
        assert!(matches!(
            queue.pending()[0].1,
            Action::Ship {
                command: ShipCommand::StartFiring,
                ..
            }
        ));
        assert!(matches!(
            queue.pending()[1].1,
            Action::Ship {
                command: ShipCommand::StopFiring,
                ..
            }
        ));
        assert!(matches!(
            queue.pending()[2].1,
            Action::Ship {
                command: ShipCommand::UnmarkTarget,
                ..
            }
        ));
    }
}
