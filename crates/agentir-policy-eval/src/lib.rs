//! Reproducible Stage 6A/6B evaluation of interaction and ranking policies.
//!
//! Evaluation records are a separate non-correctness layer. Every submitted
//! action is executed by the production JSON protocol engine, while success,
//! rejection classes, budgets, hashes, replay, and aggregates remain
//! harness-owned. The crate contains no provider SDK, HTTP client, credentials,
//! GPU driver, process execution, learned ranking, tuning, or automatic
//! artifact selection.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod corpus;
pub mod engine;
pub mod hashing;
pub mod model;
pub mod protocol;
pub mod ranking;

pub use corpus::{builtin_corpus, builtin_ranked_corpus};
pub use engine::{
    EvaluationHarness, EvaluationLimits, RankingSubmission, external_policy,
    migrate_archive_v1_to_v2, ranked_policy, scripted_policy, verify_archive,
};
pub use model::*;
pub use protocol::EvaluationProtocol;
pub use ranking::*;
