//! Shared authoritative ship definitions and interpreted hardware execution.
pub mod appearance;
pub mod catalogue;
pub mod design;
pub mod runtime;
pub use catalogue::*;
pub use design::*;
pub use runtime::*;

pub const EXAMPLE_CONTROLLER: &[u8] = include_bytes!("../data/example-controller.wasm");
pub const CHATTER_CONTROLLER: &[u8] = include_bytes!("../data/chatter-controller.wasm");

pub mod devices;
pub use devices::*;

pub mod thermal;

pub mod weapons;

pub mod missiles;

mod cargo;
pub mod industry;
pub use cargo::aggregate_stacks;

pub mod reactors;

pub mod utilities;

pub mod attachments;
pub use attachments::*;
pub mod collision;

pub mod quantities;
pub use quantities::{StochasticBalance, StochasticRound};
