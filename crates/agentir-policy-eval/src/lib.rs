//! Reproducible Stage 6A evaluation of free, menu, and hybrid agent policies.
//!
//! Evaluation records are a separate non-correctness layer. Every submitted
//! action is executed by the production JSON protocol engine, while success,
//! rejection classes, budgets, hashes, replay, and aggregates remain
//! harness-owned. The crate contains no provider SDK, HTTP client, credentials,
//! GPU driver, process execution, ranking, or artifact selection.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod corpus;
pub mod engine;
pub mod hashing;
pub mod model;
pub mod protocol;

pub use corpus::builtin_corpus;
pub use engine::{
    EvaluationHarness, EvaluationLimits, external_policy, scripted_policy, verify_archive,
};
pub use model::*;
pub use protocol::EvaluationProtocol;
