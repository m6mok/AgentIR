//! Transport-independent reference compiler core for AgentIR Stage 1.
//!
//! The crate owns canonical SpecIR state, type and shape checking, atomic
//! ActionIR transactions, immutable revisions, typed holes, proof obligations,
//! and continuation frames. Network and command-line transports intentionally
//! live in separate crates.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod actions;
pub mod canonical;
pub mod constraints;
pub mod continuation;
pub mod diagnostics;
pub mod holes;
pub mod ids;
pub mod ir;
pub mod obligations;
pub mod persistence;
pub mod resources;
pub mod revision;
pub mod semantic;
pub mod shapes;
pub mod spec;
pub mod transaction;
pub mod types;
pub mod workspace;

pub use actions::{Action, Transaction};
pub use diagnostics::{AgentError, AgentResult, ErrorCode};
pub use ids::{HoleId, RevisionId, WorkspaceId};
pub use workspace::Workspace;
