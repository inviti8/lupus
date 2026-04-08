//! lupus — pathfinder AI daemon library crate.
//!
//! This is the library half of the lupus daemon. It declares all the
//! daemon's modules as `pub` so they can be reached from:
//!   - The main binary at `src/main.rs` (`use lupus::agent::Agent;`)
//!   - Examples in `examples/` (`use lupus::agent::inference::*;`)
//!   - Sibling binaries in `src/bin/`
//!   - Integration tests in `tests/`
//!
//! The binary entry point is in `src/main.rs`. The library has no
//! standalone purpose — it's just the module organization that makes
//! the daemon's internals reachable from the rest of the crate.

pub mod agent;
pub mod config;
pub mod crawler;
pub mod daemon;
pub mod error;
pub mod index;
pub mod ipfs;
pub mod protocol;
pub mod security;
pub mod server;
pub mod tools;
