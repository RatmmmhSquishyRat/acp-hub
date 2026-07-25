//! In-process Hub engine plus embedded-library RPC client.
//!
//! [`CoreHub`] is daemon-internal: it owns the registry, runtime cache, agent
//! handles, and the single projection store. [`HubClient`] is the public
//! embedded-library entry point and forwards every method over JSON-RPC.

mod client;
mod conversation;
mod dispatch;
mod lifecycle;
mod prompt;
mod registry;
mod state;
mod types;
mod wait;

pub use client::HubClient;
pub use state::CoreHub;
pub use types::*;
pub use wait::wait_run_via_client;

#[cfg(test)]
mod tests;
