use crate::{EntityId, GalacticPosition, Id};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Presence {
    Space,
    Docked { host: EntityId, bay: u32 },
    SlipTransit(Id),
    StoredInWreck(EntityId),
    Destroyed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axes {
    Galactic,
    BodyFixed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reference {
    Celestial(EntityId),
    Beacon(EntityId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    Beacon(EntityId),
    Galactic(GalacticPosition),
    Relative {
        reference: Reference,
        offset: GalacticPosition,
        axes: Axes,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    TravelTo(Destination),
    Dock(EntityId),
    Undock,
    WaitUntil(u64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Leg {
    Sublight(Destination),
    Gate { entry: EntityId, exit: EntityId },
    Slip { destination: GalacticPosition },
    Dock(EntityId),
    Undock,
    WaitUntil(u64),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Idle,
    Planning,
    Active,
    Paused,
    Blocked(String),
    Completed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TravelState {
    pub revision: u64,
    pub orders: Vec<Order>,
    pub order: usize,
    pub legs: Vec<Leg>,
    pub leg: usize,
    pub status: Status,
    pub estimated_arrival_tick: Option<u64>,
}
