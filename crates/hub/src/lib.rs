//! ACP Hub — an ACP Client and Conductor that registers ACP Agent Endpoints,
//! manages conversations/messages/runs, sends prompts, captures Hub-owned
//! projection snapshots, searches them, and exposes CLI/MCP/library entry
//! points through an on-demand singleton daemon.
//!
//! See `doc/pillars/README.md` for the authoritative spec.

pub mod acp;
pub mod callbacks;
pub mod conductor;
pub mod daemon;
pub mod endpoint;
pub mod error;
pub mod hub;
pub mod rpc;
pub mod runtime;
pub mod store;
pub mod transport;

pub use endpoint::{Registry, home_dir};
pub use error::HubError;
