//! ACP Hub — an ACP Client and Conductor that registers ACP Agent Endpoints,
//! manages conversations/messages/runs, sends prompts, captures Hub-owned
//! projection snapshots, searches them, and exposes CLI/MCP/library entry
//! points through an on-demand singleton daemon.
//!
//! See `doc/ssot/pillars/README.md` in the repository for the authoritative
//! project pillars.

pub mod acp;
mod bounded_transport;

#[cfg(feature = "test-flow-ledger")]
#[doc(hidden)]
pub mod test_flow_ledger {
    pub use crate::bounded_transport::{
        TestFlowLedgerEvent, pause_test_flow_acknowledgements, reset_test_flow_ledger,
        test_flow_ledger_snapshot,
    };
}
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
