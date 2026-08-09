//! Reproducible Stage 6A–7C policy evaluation, ranking, search, and acquisition.
//!
//! Evaluation records are a separate non-correctness layer. Every submitted
//! action is executed by the production JSON protocol engine, while success,
//! rejection classes, budgets, hashes, replay, and aggregates remain
//! harness-owned. The crate contains no provider SDK, HTTP client, credentials,
//! GPU driver, process execution, hardware-driven tuning, or automatic artifact
//! publication. Stage 7A search is bounded, deterministic, and evaluation-only.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod acquisition;
pub mod continuation;
pub mod corpus;
pub mod engine;
pub mod hashing;
pub mod learned;
pub mod measured;
pub mod model;
pub mod protocol;
pub mod ranking;
pub mod repairs;
pub mod search;
pub mod work;

pub use acquisition::*;
pub use continuation::*;
pub use corpus::{builtin_corpus, builtin_ranked_corpus};
pub use engine::{
    EvaluationHarness, EvaluationLimits, LearnedArchiveBundle, RankingSubmission,
    attach_learning_artifacts, attach_measured_search_artifacts,
    attach_measurement_acquisition_artifacts, attach_search_artifacts, external_policy,
    migrate_archive_v1_to_v2, migrate_archive_v2_to_v3, migrate_archive_v3_to_v4,
    migrate_archive_v4_to_v5, migrate_archive_v5_to_v6, ranked_policy, scripted_policy,
    verify_archive,
};
pub use learned::*;
pub use measured::*;
pub use model::*;
pub use protocol::EvaluationProtocol;
pub use ranking::*;
pub use repairs::*;
pub use search::*;
pub use work::*;
