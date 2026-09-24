//! Language-neutral ship syscalls. No allocator, runtime model, or serialization.
#![no_std]

/// Shared compatibility version for firmware, connections, and saved worlds.
pub const GAME_VERSION: u16 = 56;

pub mod abi;
pub mod beacons;
pub mod sdk;
pub mod services;
pub mod world;
