use crate::state::{Celestial, Contact, Outgoing, OwnedShip, SessionInfo, ViewObservation};
use bevy::prelude::*;
use toy_sim_model::*;

#[derive(Resource, Default)]
pub(super) struct Subscriptions {
    focused: Option<Id>,
    view: Option<(u64, Id, GroupId)>,
    revision: u64,
}

#[derive(Resource, Default)]
pub(super) struct Selection {
    pub target: Option<SelectedTarget>,
    pub ship: Option<EntityId>,
    pub view: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SelectedTarget {
    Contact(ContactRef),
    Celestial(Id),
    Beacon(Id),
}

impl Selection {
    pub(super) fn contact(&self) -> Option<ContactRef> {
        match self.target {
            Some(SelectedTarget::Contact(reference)) => Some(reference),
            _ => None,
        }
    }

    pub(super) fn celestial(&self) -> Option<Id> {
        match self.target {
            Some(SelectedTarget::Celestial(id)) => Some(id),
            _ => None,
        }
    }
}

pub(super) fn synchronize(
    session: Res<SessionInfo>,
    mut subscriptions: ResMut<Subscriptions>,
    mut selection: ResMut<Selection>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<&OwnedShip>,
    contacts: Query<&Contact>,
    views: Query<&ViewObservation>,
    celestials: Query<&Celestial>,
) {
    if selection
        .celestial()
        .is_some_and(|selected| !celestials.iter().any(|body| body.0.entity == selected))
    {
        selection.target = None;
    }
    if selection
        .contact()
        .is_some_and(|selected| !contacts.iter().any(|contact| contact.1 == selected))
    {
        selection.target = None;
    }
    if selection.ship.is_none_or(|selected| {
        !ships.iter().any(|ship| ship.0.ship == selected)
            && !session
                .industry
                .snapshot
                .hangar
                .as_ref()
                .is_some_and(|hangar| {
                    hangar
                        .ships
                        .iter()
                        .any(|entry| entry.inventory.entity == selected && entry.can_focus)
                })
    }) {
        selection.ship = ships
            .iter()
            .filter(|ship| {
                ship.0.can_control
                    && !matches!(
                        ship.0.presence,
                        travel::Presence::Destroyed | travel::Presence::StoredInWreck(_)
                    )
            })
            .map(|ship| ship.0.ship)
            .min();
    }
    if selection
        .view
        .is_none_or(|selected| !views.iter().any(|view| view.0.id == selected))
    {
        selection.view = views.iter().map(|view| view.0.id).min();
    }
    if subscriptions.focused != selection.ship {
        if let Some(previous) = subscriptions.focused {
            outgoing.push(Action::InstrumentUnsubscribe { ship: previous });
        }
        subscriptions.focused = selection.ship;
        if let Some(ship) = selection.ship {
            outgoing.push(Action::InstrumentSubscribe { ship });
        }
    }
    let view_id = selection.view.unwrap_or(1);
    let current_view = views.iter().find(|view| view.0.id == view_id);
    let group = current_view
        .map(|view| view.0.group)
        .filter(|group| *group != PUBLIC_GROUP && session.groups.contains(group))
        .or_else(|| {
            session
                .groups
                .iter()
                .copied()
                .find(|group| *group != PUBLIC_GROUP)
        });
    if let (Some(group), Some(ship)) = (group, selection.ship) {
        let requested = (view_id, ship, group);
        if subscriptions.view != Some(requested) {
            subscriptions.revision = subscriptions
                .revision
                .max(current_view.map_or(0, |view| view.0.revision))
                .checked_add(1)
                .expect("view subscription revision exhausted");
            outgoing.push(Action::Subscribe(ViewSubscription {
                id: view_id,
                revision: subscriptions.revision,
                group,
                focused_ship: Some(ship),
                query: TrackQuery {
                    sphere: Some((GalacticPosition::ZERO, 1e8)),
                    limit: 256,
                    work: 300_000,
                    ..Default::default()
                },
            }));
            subscriptions.view = Some(requested);
        }
    }
}
