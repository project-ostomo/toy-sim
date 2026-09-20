//! Language-neutral ship syscalls. No allocator, runtime model, or serialization.
#![no_std]

pub mod abi;
pub mod sdk;
pub mod services;
pub mod world;
pub mod world_intel;
