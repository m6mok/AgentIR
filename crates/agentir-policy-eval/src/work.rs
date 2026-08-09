//! Deterministic non-semantic work-unit accounting.

use crate::model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult};
use crate::{model::EvaluationArchive, ranking::EvaluationChoiceSet};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Deterministic operation counts retained only as evaluation/study data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitCounters {
    /// Compiler descriptor queries.
    pub descriptor_query: u64,
    /// Visible feature extraction operations.
    pub feature_extraction: u64,
    /// Stable identity assignments.
    pub stable_id_assignment: u64,
    /// Sorting and deduplication operations.
    pub sorting_deduplication: u64,
    /// Canonical encoding operations.
    pub canonical_encoding: u64,
    /// Domain-separated hashing operations.
    pub hashing: u64,
    /// Fixed-point score validation operations.
    pub score_validation: u64,
    /// Deterministic tie-resolution operations.
    pub tie_resolution: u64,
    /// Explicit production dispatches.
    pub production_dispatch: u64,
    /// Production compiler verification operations.
    pub compiler_verification: u64,
    /// Transcript publications.
    pub transcript_publication: u64,
    /// Archive parse operations.
    pub archive_parse: u64,
    /// Archive structural verification operations.
    pub archive_structural_verification: u64,
    /// Replay operations.
    pub replay: u64,
    /// Aggregate recomputation operations.
    pub aggregate_recomputation: u64,
}

impl WorkUnitCounters {
    /// Returns the checked sum of all work categories.
    pub fn total(&self) -> EvaluationResult<u64> {
        [
            self.descriptor_query,
            self.feature_extraction,
            self.stable_id_assignment,
            self.sorting_deduplication,
            self.canonical_encoding,
            self.hashing,
            self.score_validation,
            self.tie_resolution,
            self.production_dispatch,
            self.compiler_verification,
            self.transcript_publication,
            self.archive_parse,
            self.archive_structural_verification,
            self.replay,
            self.aggregate_recomputation,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationWorkUnitOverflow,
                "evaluation work-unit counter overflow",
            )
        })
    }

    /// Rejects a total above an operational work-unit limit.
    pub fn validate_limit(&self, maximum: u64) -> EvaluationResult<()> {
        let total = self.total()?;
        if total > maximum {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationWorkUnitLimitExceeded,
                "evaluation work-unit limit exceeded",
            )
            .expected_actual(json!(maximum), json!(total))
            .repair("reduce the bounded input or raise the non-semantic evaluation limit"));
        }
        Ok(())
    }
}

/// Computes deterministic Stage 6B ranking/dispatch work for study attribution.
#[must_use]
pub fn ranking_dispatch_work_units(choice_set: &EvaluationChoiceSet) -> WorkUnitCounters {
    let choices = u64::try_from(choice_set.choices.len()).unwrap_or(u64::MAX);
    let features = choice_set.choices.iter().fold(0_u64, |total, choice| {
        total
            .saturating_add(u64::try_from(choice.visible_features.values.len()).unwrap_or(u64::MAX))
    });
    WorkUnitCounters {
        descriptor_query: 1,
        feature_extraction: features,
        stable_id_assignment: choices,
        sorting_deduplication: choices,
        canonical_encoding: choices.saturating_add(3),
        hashing: choices.saturating_add(3),
        score_validation: choices,
        tie_resolution: choices.saturating_sub(1),
        production_dispatch: 1,
        compiler_verification: 1,
        transcript_publication: 1,
        ..WorkUnitCounters::default()
    }
}

/// Computes deterministic archive/replay/aggregate work for study attribution.
#[must_use]
pub fn archive_work_units(archive: &EvaluationArchive) -> WorkUnitCounters {
    let steps = archive
        .runs
        .iter()
        .flat_map(|run| &run.episodes)
        .fold(0_u64, |total, episode| {
            total.saturating_add(u64::try_from(episode.steps.len()).unwrap_or(u64::MAX))
        });
    let structural_records = u64::try_from(
        archive
            .runs
            .len()
            .saturating_add(archive.aggregates.len())
            .saturating_add(archive.feature_schemas.len())
            .saturating_add(archive.ranking_policies.len())
            .saturating_add(archive.choice_sets.len())
            .saturating_add(archive.ranking_datasets.len())
            .saturating_add(archive.dataset_splits.len())
            .saturating_add(archive.training_configurations.len())
            .saturating_add(archive.training_runs.len())
            .saturating_add(archive.learned_models.len())
            .saturating_add(archive.ranking_inputs.len())
            .saturating_add(archive.inference_records.len()),
    )
    .unwrap_or(u64::MAX);
    WorkUnitCounters {
        archive_parse: u64::try_from(
            serde_json::to_vec(archive).map_or(usize::MAX, |bytes| bytes.len()),
        )
        .unwrap_or(u64::MAX),
        archive_structural_verification: structural_records,
        replay: steps,
        aggregate_recomputation: u64::try_from(archive.runs.len()).unwrap_or(u64::MAX),
        canonical_encoding: structural_records,
        hashing: structural_records,
        ..WorkUnitCounters::default()
    }
}
