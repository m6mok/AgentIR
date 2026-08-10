//! Transport-independent reference compiler core for AgentIR through Stage 2C.
//!
//! The crate owns canonical SpecIR state, type and shape checking, atomic
//! ActionIR transactions, immutable revisions, typed holes, ImplIR candidates,
//! speculative proof debt, guarded fallback and continuation frames. Network
//! and command-line transports intentionally live in separate crates.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod actions;
pub mod backend;
pub mod backend_ir;
pub mod candidate;
pub mod canonical;
pub mod constraints;
pub mod continuation;
pub mod cpu;
pub mod cpu_measurement;
pub mod diagnostics;
pub mod equality;
pub mod holes;
pub mod ids;
pub mod impl_ir;
pub mod ir;
pub mod memory;
pub mod memory_ir;
pub mod obligations;
pub mod persistence;
pub mod resources;
pub mod revision;
pub mod schedule;
pub mod schedule_ir;
pub mod semantic;
pub mod shapes;
pub mod spec;
pub mod target;
pub mod transaction;
pub mod types;
pub mod workspace;

pub use actions::{Action, Transaction};
pub use diagnostics::{AgentError, AgentResult, ErrorCode};
pub use ids::{
    ArtifactId, BackendPlanId, BackendRevisionId, CandidateId, CandidateRevisionId,
    CpuMeasurementId, HoleId, MemoryPlanId, MemoryRevisionId, ProposalId, RevisionId,
    SchedulePlanId, ScheduleRevisionId, TargetManifestId, TargetManifestRevisionId, WorkspaceId,
};
pub use workspace::Workspace;
