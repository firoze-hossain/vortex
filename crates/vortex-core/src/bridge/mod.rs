//! vortex-core/src/bridge/mod.rs
//! Local HTTP/JSON Bridge Server connecting browsers & extensions to the running Vortex desktop engine.

pub mod server;

pub use server::{start_bridge_server, BridgeServerHandle};
