use super::instruments;
use crate::state::{Celestial, Contact, Outgoing, OwnedShip, SessionInfo, ViewObservation};
use bevy::prelude::*;
use toy_sim_model::*;

#[derive(Resource, Default)]
pub(super) struct Subscriptions {
    initial: bool,
    focused: Option<Id>,
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

#[derive(Message)]
pub(super) struct FocusRequest {
    pub view: Option<u64>,
    pub target: SelectedTarget,
}

pub(super) fn synchronize(
    session: Res<SessionInfo>,
    mut subscriptions: ResMut<Subscriptions>,
    mut flight: ResMut<instruments::FlightControls>,
    mut navigation: ResMut<instruments::NavigationControls>,
    mut contact_controls: ResMut<instruments::ContactControls>,
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
    if selection.contact().is_some_and(|selected| {
        !contacts
            .iter()
            .any(|contact| contact.1 == selected && instruments::is_ship_contact(&contact.0))
    }) {
        selection.target = None;
    }
    if selection
        .ship
        .is_none_or(|selected| !ships.iter().any(|ship| ship.0.ship == selected))
    {
        selection.ship = ships.iter().map(|ship| ship.0.ship).min();
    }
    if selection
        .view
        .is_none_or(|selected| !views.iter().any(|view| view.0.id == selected))
    {
        selection.view = views.iter().map(|view| view.0.id).min();
    }
    if subscriptions.focused != selection.ship {
        if let Some(previous) = subscriptions.focused {
            if let Some(ship) = ships.iter().find(|ship| ship.0.ship == previous) {
                instruments::release_manual(&mut flight, &mut outgoing, &ship.0);
            }
            outgoing.push(Action::InstrumentUnsubscribe { ship: previous });
        }
        *flight = instruments::FlightControls::default();
        *navigation = instruments::NavigationControls::default();
        *contact_controls = instruments::ContactControls::default();
        subscriptions.focused = selection.ship;
        if let Some(ship) = selection.ship {
            outgoing.push(Action::InstrumentSubscribe { ship });
        }
    }
    if !subscriptions.initial {
        if let (Some(group), Some(ship)) = (
            session.groups.iter().find(|group| **group != PUBLIC_GROUP),
            selection.ship,
        ) {
            outgoing.push(Action::Subscribe(ViewSubscription {
                id: 1,
                revision: 1,
                group: *group,
                focused_ship: Some(ship),
                query: TrackQuery {
                    sphere: Some((GalacticPosition::ZERO, 1e8)),
                    limit: 256,
                    work: 300_000,
                    ..Default::default()
                },
            }));
            subscriptions.initial = true;
        }
    }
}

pub(super) fn reset_focus_requests(
    _: On<crate::state::SessionReset>,
    mut requests: ResMut<Messages<FocusRequest>>,
) {
    requests.clear();
}
