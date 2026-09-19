//! Shared authoritative ship definitions and interpreted hardware execution.
pub mod catalogue;
pub mod design;
pub mod runtime;
pub use catalogue::*;
pub use design::*;
pub use runtime::*;

pub const EXAMPLE_CONTROLLER: &[u8] = include_bytes!("../data/example-controller.wasm");

pub mod devices;
pub use devices::*;

pub mod thermal;

pub mod weapons;

pub mod reactors;

pub mod utilities;

pub mod attachments;
pub use attachments::*;
pub mod collision;

pub mod quantities;
pub use quantities::{StochasticBalance, StochasticRound};
