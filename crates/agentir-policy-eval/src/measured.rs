//! Stage 7B measurement-aware offline recommendation contracts.
//!
//! This module consumes immutable, compiler-published hardware measurement
//! records. It never acquires measurements, opens a device, changes compiler
//! state, or alters the frozen Stage 7A search contracts.

use crate::{
    hashing::domain_hash_cleared,
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, EvaluationTaskId},
    search::{SearchNodeId, SearchSession},
};
use agentir_core::{
    Workspace,
    backend::measurement_hash,
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig, HardwareMeasurementRecord},
    ids::MeasurementId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Measurement-cohort hash domain.
pub const MEASUREMENT_COHORT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.measurement_cohort.v1\0";
/// Measured-objective hash domain.
pub const MEASURED_OBJECTIVE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.measured_objective.v1\0";
/// Measured-recommendation hash domain.
pub const MEASURED_RECOMMENDATION_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.measured_recommendation.v1\0";

/// Reference accepted at the measurement trust boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MeasurementReference {
    /// Compiler-assigned persistent measurement identity.
    Id(MeasurementId),
    /// Exact hash of an existing compiler-owned record.
    Hash(String),
}

/// Versioned validation policy for one frozen cohort.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementValidationPolicy {
    /// Only records produced by the real benchmark protocol are accepted.
    HardwareExecutedV1,
    /// Explicitly marked synthetic records used only by tests/studies.
    SyntheticFixtureV1,
}

impl MeasurementValidationPolicy {
    fn accepts(self, status: &str) -> bool {
        match self {
            Self::HardwareExecutedV1 => status == "offline_validated_and_device_executed",
            Self::SyntheticFixtureV1 => matches!(
                status,
                "synthetic_test_data_not_performance_evidence"
                    | "fixture_record_not_hardware_executed"
            ),
        }
    }
}

/// Deterministic cross-record aggregation contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAggregationMethod {
    /// One and only one measurement record is required per artifact.
    SingleRecordSummaryV1,
    /// Lower median of ordered record summaries; even counts select the lower member.
    MedianOfRecordSummariesV1,
}

/// One verified compiler-owned record retained in a cohort snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementCohortRecord {
    /// Compiler-assigned record identity.
    pub measurement_id: MeasurementId,
    /// Complete immutable production record.
    pub record: HardwareMeasurementRecord,
}

/// Immutable, canonical, same-device/build/input measurement cohort v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementCohort {
    /// Cohort schema version.
    pub version: u32,
    /// Exact evaluation corpus anchor.
    pub corpus_hash: String,
    /// Exact task anchor.
    pub task_id: EvaluationTaskId,
    /// Exact Stage 7A root anchor.
    pub initial_anchor_hash: String,
    /// Canonically ordered verified records.
    pub records: Vec<MeasurementCohortRecord>,
    /// Canonically ordered exact measurement hashes.
    pub measurement_hashes: Vec<String>,
    /// Canonically ordered measured artifact hashes.
    pub artifact_hashes: Vec<String>,
    /// Immutable target anchor shared by all records.
    pub target_hash: String,
    /// Immutable compiler-build anchor shared by all records.
    pub compiler_build_hash: String,
    /// Immutable device anchor shared by all records.
    pub device_fingerprint_hash: String,
    /// Runtime version shared by all records.
    pub runtime_version: String,
    /// Exact benchmark configuration shared by all records.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Validation policy applied to every record.
    pub validation_policy: MeasurementValidationPolicy,
    /// Exact record count required for every artifact.
    pub records_per_artifact: u64,
    /// Cross-record aggregation contract.
    pub aggregation_method: MeasurementAggregationMethod,
    /// Independent cohort hash.
    pub measurement_cohort_hash: String,
}

/// Exact server-owned request for freezing one measurement cohort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementCohortRequest {
    /// Exact evaluation corpus anchor.
    pub corpus_hash: String,
    /// Exact task anchor.
    pub task_id: EvaluationTaskId,
    /// Exact Stage 7A root anchor.
    pub initial_anchor_hash: String,
    /// Validation policy applied to resolved production records.
    pub validation_policy: MeasurementValidationPolicy,
    /// Exact record count required for every artifact.
    pub records_per_artifact: u64,
    /// Cross-record aggregation contract.
    pub aggregation_method: MeasurementAggregationMethod,
}

/// Resolves references only against one production workspace and freezes a cohort.
pub fn measurement_cohort_from_workspace(
    workspace: &Workspace,
    references: &[MeasurementReference],
    request: MeasurementCohortRequest,
) -> EvaluationResult<MeasurementCohort> {
    let store = workspace.measurement_store();
    let mut resolved = Vec::with_capacity(references.len());
    for reference in references {
        let (id, record) = match reference {
            MeasurementReference::Id(id) => store.records.get_key_value(id),
            MeasurementReference::Hash(hash) => store
                .records
                .iter()
                .find(|(_, record)| record.measurement_hash.as_str() == hash),
        }
        .ok_or_else(|| {
            measured_error(
                EvaluationErrorCode::EvaluationMeasurementNotFound,
                "measurement reference does not resolve in the production workspace",
            )
        })?;
        let package_valid = workspace.artifact_store().packages.values().any(|package| {
            package.artifact_hash == record.artifact_hash
                && package.status == ArtifactStatus::Validated
        });
        if !package_valid {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasuredTerminalUnavailable,
                "measurement artifact is not a retained offline-valid artifact",
            ));
        }
        resolved.push(MeasurementCohortRecord {
            measurement_id: id.clone(),
            record: record.clone(),
        });
    }
    measurement_cohort_from_verified_records(resolved, request)
}

/// Freezes already server-resolved production-format records through the same
/// Stage 7B eligibility and canonicalization path.
pub fn measurement_cohort_from_verified_records(
    mut resolved: Vec<MeasurementCohortRecord>,
    request: MeasurementCohortRequest,
) -> EvaluationResult<MeasurementCohort> {
    let MeasurementCohortRequest {
        corpus_hash,
        task_id,
        initial_anchor_hash,
        validation_policy,
        records_per_artifact,
        aggregation_method,
    } = request;
    if resolved.is_empty() || records_per_artifact == 0 {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasurementMissing,
            "measurement cohort requires records and a positive per-artifact count",
        ));
    }
    if aggregation_method == MeasurementAggregationMethod::SingleRecordSummaryV1
        && records_per_artifact != 1
    {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationUnequalMeasurementRecords,
            "single-record aggregation requires exactly one record per artifact",
        ));
    }
    let mut seen_hashes = BTreeSet::new();
    for entry in &resolved {
        let calculated = measurement_hash(&entry.record).map_err(|error| {
            measured_error(
                EvaluationErrorCode::EvaluationMeasurementCorrupt,
                format!("production measurement cannot be verified: {error}"),
            )
        })?;
        if calculated != entry.record.measurement_hash
            || !seen_hashes.insert(entry.record.measurement_hash.to_string())
        {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasurementDuplicate,
                "duplicate or corrupt measurement hashes are forbidden in a cohort",
            ));
        }
        if !validation_policy.accepts(&entry.record.validation_status) {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasurementValidationInvalid,
                "measurement validation status is not accepted by the cohort policy",
            )
            .expected_actual(
                json!(validation_policy),
                json!(entry.record.validation_status),
            ));
        }
    }
    resolved.sort_by(|left, right| {
        left.record
            .measurement_hash
            .cmp(&right.record.measurement_hash)
    });
    let first = &resolved[0].record;
    for record in resolved.iter().skip(1).map(|entry| &entry.record) {
        if let Some((code, field)) = cohort_compatibility_mismatch(first, record) {
            return Err(measured_error(
                code,
                format!("measurement cohort mixes incompatible {field} records"),
            ));
        }
    }
    let mut counts = BTreeMap::<String, u64>::new();
    for entry in &resolved {
        let count = counts
            .entry(entry.record.artifact_hash.to_string())
            .or_default();
        *count = count.checked_add(1).ok_or_else(measured_overflow)?;
    }
    if counts.values().any(|count| *count != records_per_artifact) {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationUnequalMeasurementRecords,
            "each artifact must have exactly the declared record count",
        )
        .expected_actual(json!(records_per_artifact), json!(counts)));
    }
    let mut cohort = MeasurementCohort {
        version: 1,
        corpus_hash,
        task_id,
        initial_anchor_hash,
        measurement_hashes: resolved
            .iter()
            .map(|entry| entry.record.measurement_hash.to_string())
            .collect(),
        artifact_hashes: counts.into_keys().collect(),
        target_hash: first.target_hash.to_string(),
        compiler_build_hash: first.compiler_build_hash.to_string(),
        device_fingerprint_hash: first.device_fingerprint_hash.to_string(),
        runtime_version: first.runtime_version.clone(),
        benchmark_config: first.config.clone(),
        validation_policy,
        records_per_artifact,
        aggregation_method,
        records: resolved,
        measurement_cohort_hash: String::new(),
    };
    cohort.measurement_cohort_hash = measurement_cohort_hash(&cohort)?;
    Ok(cohort)
}

/// Revalidates a retained cohort without hardware work.
pub fn verify_measurement_cohort(cohort: &MeasurementCohort) -> EvaluationResult<()> {
    if cohort.version != 1 || cohort.records.is_empty() || cohort.records_per_artifact == 0 {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasurementCohortCorrupt,
            "measurement cohort version or cardinality is invalid",
        ));
    }
    let mut hashes = Vec::new();
    let mut artifacts = BTreeMap::<String, u64>::new();
    let first = &cohort.records[0].record;
    if first.target_hash.as_str() != cohort.target_hash
        || first.compiler_build_hash.as_str() != cohort.compiler_build_hash
        || first.device_fingerprint_hash.as_str() != cohort.device_fingerprint_hash
        || first.runtime_version != cohort.runtime_version
        || first.config != cohort.benchmark_config
    {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasurementCohortCorrupt,
            "retained cohort top-level anchors differ from its first record",
        ));
    }
    for entry in &cohort.records {
        let calculated = measurement_hash(&entry.record).map_err(|error| {
            measured_error(
                EvaluationErrorCode::EvaluationMeasurementCorrupt,
                format!("retained measurement cannot be verified: {error}"),
            )
        })?;
        if calculated != entry.record.measurement_hash {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasurementCorrupt,
                "retained measurement hash is corrupt",
            ));
        }
        if !cohort
            .validation_policy
            .accepts(&entry.record.validation_status)
        {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasurementValidationInvalid,
                "retained measurement validation status is not accepted",
            ));
        }
        if let Some((code, field)) = cohort_compatibility_mismatch(first, &entry.record) {
            return Err(measured_error(
                code,
                format!("retained measurement cohort mixes incompatible {field} records"),
            ));
        }
        hashes.push(entry.record.measurement_hash.to_string());
        let count = artifacts
            .entry(entry.record.artifact_hash.to_string())
            .or_default();
        *count = count.checked_add(1).ok_or_else(measured_overflow)?;
    }
    let mut canonical_hashes = hashes.clone();
    canonical_hashes.sort();
    if canonical_hashes.windows(2).any(|pair| pair[0] == pair[1])
        || hashes != canonical_hashes
        || canonical_hashes != cohort.measurement_hashes
        || artifacts.keys().cloned().collect::<Vec<_>>() != cohort.artifact_hashes
        || artifacts
            .values()
            .any(|count| *count != cohort.records_per_artifact)
        || cohort.measurement_cohort_hash != measurement_cohort_hash(cohort)?
    {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasurementCohortCorrupt,
            "measurement cohort canonical order, counts, or hash is corrupt",
        ));
    }
    Ok(())
}

fn cohort_compatibility_mismatch(
    first: &HardwareMeasurementRecord,
    record: &HardwareMeasurementRecord,
) -> Option<(EvaluationErrorCode, &'static str)> {
    if record.device_fingerprint_hash != first.device_fingerprint_hash {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedDevice,
            "device",
        ))
    } else if record.target_hash != first.target_hash {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedTarget,
            "target",
        ))
    } else if record.compiler_build_hash != first.compiler_build_hash {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedBuild,
            "compiler build",
        ))
    } else if record.runtime_version != first.runtime_version {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedRuntime,
            "runtime",
        ))
    } else if record.config.input_distribution != first.config.input_distribution
        || record.config.tensor_dimensions != first.config.tensor_dimensions
    {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedInput,
            "input",
        ))
    } else if record.config != first.config {
        Some((
            EvaluationErrorCode::EvaluationMeasurementMixedConfig,
            "benchmark config",
        ))
    } else {
        None
    }
}

/// Hardware metric permitted by Stage 7B v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredMetric {
    /// Median nanoseconds from each production record.
    MedianNs,
    /// P95 nanoseconds from each production record.
    P95Ns,
}

/// Measured objective direction. V1 permits only minimize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredDirection {
    /// Smaller integer values are preferred.
    Minimize,
}

/// Eligibility of an unmeasured terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredEligibilityPolicy {
    /// Exclude it with a typed reason; never substitute a sentinel value.
    RequireEligibleMeasurement,
}

/// Terminal-only application of the measured objective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSelectionSemantics {
    /// Stage 7A orders intermediate nodes; measurement selects only terminals.
    PostSearchMeasuredTerminalsOnly,
}

/// Immutable hardware objective descriptor v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredObjectiveDescriptor {
    /// Descriptor schema version.
    pub version: u32,
    /// Exact cohort anchor.
    pub measurement_cohort_hash: String,
    /// Hardware summary metric.
    pub metric: MeasuredMetric,
    /// Cross-record aggregation method.
    pub aggregation_method: MeasurementAggregationMethod,
    /// V1 comparison direction.
    pub direction: MeasuredDirection,
    /// Exact required records per artifact.
    pub records_per_artifact: u64,
    /// Integer parts-per-million indifference band, at most one million.
    pub indifference_band_ppm: u64,
    /// Missing-measurement behavior.
    pub eligibility_policy: MeasuredEligibilityPolicy,
    /// Terminal-only semantics.
    pub terminal_selection_semantics: TerminalSelectionSemantics,
    /// Exact frozen Stage 7A structural fallback objective hash.
    pub structural_fallback_objective_hash: String,
    /// Independent measured-objective hash.
    pub measured_objective_hash: String,
}

impl MeasuredObjectiveDescriptor {
    /// Constructs one validated v1 descriptor.
    pub fn new(
        cohort: &MeasurementCohort,
        metric: MeasuredMetric,
        indifference_band_ppm: u64,
        structural_fallback_objective_hash: String,
    ) -> EvaluationResult<Self> {
        verify_measurement_cohort(cohort)?;
        if indifference_band_ppm > 1_000_000 {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasuredObjectiveInvalid,
                "indifference band must be between zero and one million ppm",
            ));
        }
        let mut descriptor = Self {
            version: 1,
            measurement_cohort_hash: cohort.measurement_cohort_hash.clone(),
            metric,
            aggregation_method: cohort.aggregation_method,
            direction: MeasuredDirection::Minimize,
            records_per_artifact: cohort.records_per_artifact,
            indifference_band_ppm,
            eligibility_policy: MeasuredEligibilityPolicy::RequireEligibleMeasurement,
            terminal_selection_semantics:
                TerminalSelectionSemantics::PostSearchMeasuredTerminalsOnly,
            structural_fallback_objective_hash,
            measured_objective_hash: String::new(),
        };
        descriptor.measured_objective_hash = measured_objective_hash(&descriptor)?;
        Ok(descriptor)
    }

    /// Verifies the descriptor and exact cohort anchor.
    pub fn validate(&self, cohort: &MeasurementCohort) -> EvaluationResult<()> {
        if self.version != 1
            || self.measurement_cohort_hash != cohort.measurement_cohort_hash
            || self.aggregation_method != cohort.aggregation_method
            || self.records_per_artifact != cohort.records_per_artifact
            || self.indifference_band_ppm > 1_000_000
            || self.measured_objective_hash != measured_objective_hash(self)?
        {
            return Err(measured_error(
                EvaluationErrorCode::EvaluationMeasuredObjectiveCorrupt,
                "measured objective is corrupt or stale for its cohort",
            ));
        }
        Ok(())
    }
}

/// Typed reason why a terminal artifact is unavailable or ineligible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredUnavailableReason {
    /// Terminal trajectory did not publish a compiler-owned artifact hash.
    NoTerminalArtifact,
    /// Cohort contains no record for the terminal artifact.
    MissingMeasurement,
    /// Records exist but fail the exact required count.
    UnequalRecordCount,
}

/// One terminal alternative with its exact eligibility result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredAlternative {
    /// Stage 7A terminal node.
    pub terminal_node: SearchNodeId,
    /// Compiler-owned artifact hash, when published.
    pub artifact_hash: Option<String>,
    /// Exact measurement hashes used by aggregation.
    pub measurement_hashes: Vec<String>,
    /// Aggregated integer objective, absent when unavailable.
    pub objective_value_ns: Option<u64>,
    /// Typed absence instead of a numerical sentinel.
    pub unavailable: Option<MeasuredUnavailableReason>,
}

/// Classification of the selected value relative to eligible alternatives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredTieClassification {
    /// Only one eligible value or every other value is outside the band.
    Distinct,
    /// At least one alternative is exactly equal.
    ExactTie,
    /// At least one non-equal alternative lies within the ppm band.
    WithinIndifferenceBand,
}

/// Measured recommendation lifecycle result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredRecommendationStatus {
    /// One eligible terminal was selected under the exact descriptor/cohort.
    Selected,
    /// Search completed or bounded without a comparable measured terminal.
    NoComparableMeasuredTerminal,
}

/// Immutable, non-authoritative measured recommendation v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredRecommendation {
    /// Recommendation schema version.
    pub version: u32,
    /// Frozen Stage 7A result anchor.
    pub search_result_hash: String,
    /// Exact measured objective anchor.
    pub measured_objective_hash: String,
    /// Exact cohort anchor.
    pub measurement_cohort_hash: String,
    /// Selected terminal node.
    pub selected_terminal_node: Option<SearchNodeId>,
    /// Selected artifact hash.
    pub artifact_hash: Option<String>,
    /// Exact measurement hashes used for the selected value.
    pub measurement_hashes: Vec<String>,
    /// Selected integer objective value.
    pub objective_value_ns: Option<u64>,
    /// Noise-aware tie classification.
    pub tie_classification: Option<MeasuredTieClassification>,
    /// Eligible alternatives in deterministic measured order.
    pub eligible_alternatives: Vec<MeasuredAlternative>,
    /// Rejected/ineligible alternatives in deterministic node order.
    pub ineligible_alternatives: Vec<MeasuredAlternative>,
    /// Completion classification.
    pub status: MeasuredRecommendationStatus,
    /// Exact non-authoritative terminology.
    pub provenance: String,
    /// Independent recommendation hash.
    pub measured_recommendation_hash: String,
}

/// Deterministic Stage 7B work accounting, excluded from semantic hashes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredSearchWorkCounters {
    /// Production records inspected.
    pub measurement_records_inspected: u64,
    /// Eligible artifact aggregations performed.
    pub objective_aggregations: u64,
    /// Indifference comparisons performed.
    pub indifference_comparisons: u64,
    /// Exact offline recommendation replays.
    pub recommendation_replays: u64,
    /// Hardware/device calls; verified records require this to remain zero.
    pub hardware_calls: u64,
}

/// Archive anchor for one completed measured-search recommendation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredSearchRunRecord {
    /// Frozen Stage 7A result.
    pub search_result_hash: String,
    /// Frozen cohort.
    pub measurement_cohort_hash: String,
    /// Frozen measured objective.
    pub measured_objective_hash: String,
    /// Recommendation, when produced.
    pub measured_recommendation_hash: String,
    /// Exact offline replay status.
    pub replay_verified: bool,
    /// Deterministic non-correctness work accounting.
    pub work: MeasuredSearchWorkCounters,
}

/// Computes a measured recommendation after Stage 7A has stopped.
pub fn measured_recommendation(
    session: &SearchSession,
    cohort: &MeasurementCohort,
    objective: &MeasuredObjectiveDescriptor,
) -> EvaluationResult<MeasuredRecommendation> {
    verify_measurement_cohort(cohort)?;
    objective.validate(cohort)?;
    let result = session.result.as_ref().ok_or_else(|| {
        measured_error(
            EvaluationErrorCode::EvaluationSearchIncomplete,
            "measured recommendation requires a stopped Stage 7A result",
        )
    })?;
    if result.search_objective_hash != objective.structural_fallback_objective_hash
        || session.objective.initial_anchor_hash != cohort.initial_anchor_hash
        || session.objective.corpus_hash != cohort.corpus_hash
        || session.task_id != cohort.task_id
    {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasuredAnchorStale,
            "search, cohort, and measured objective anchors are stale",
        ));
    }
    let mut by_artifact = BTreeMap::<String, Vec<&MeasurementCohortRecord>>::new();
    for record in &cohort.records {
        by_artifact
            .entry(record.record.artifact_hash.to_string())
            .or_default()
            .push(record);
    }
    let mut terminals = session
        .nodes
        .values()
        .filter(|node| node.terminal)
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    terminals.sort();
    let mut eligible = Vec::new();
    let mut ineligible = Vec::new();
    for terminal in terminals {
        let artifact = terminal_artifact_hash(session, &terminal)?;
        let Some(artifact_hash) = artifact else {
            ineligible.push(unavailable_alternative(
                terminal,
                None,
                MeasuredUnavailableReason::NoTerminalArtifact,
            ));
            continue;
        };
        let Some(records) = by_artifact.get(&artifact_hash) else {
            ineligible.push(unavailable_alternative(
                terminal,
                Some(artifact_hash),
                MeasuredUnavailableReason::MissingMeasurement,
            ));
            continue;
        };
        if u64::try_from(records.len()).unwrap_or(u64::MAX) != objective.records_per_artifact {
            ineligible.push(unavailable_alternative(
                terminal,
                Some(artifact_hash),
                MeasuredUnavailableReason::UnequalRecordCount,
            ));
            continue;
        }
        let value = aggregate_records(records, objective.metric, objective.aggregation_method)?;
        eligible.push(MeasuredAlternative {
            terminal_node: terminal,
            artifact_hash: Some(artifact_hash),
            measurement_hashes: records
                .iter()
                .map(|entry| entry.record.measurement_hash.to_string())
                .collect(),
            objective_value_ns: Some(value),
            unavailable: None,
        });
    }
    eligible.sort_by(|left, right| {
        left.objective_value_ns
            .cmp(&right.objective_value_ns)
            .then_with(|| left.artifact_hash.cmp(&right.artifact_hash))
            .then_with(|| left.terminal_node.cmp(&right.terminal_node))
    });
    let selected_index = if let Some(first) = eligible.first() {
        let first_value = first.objective_value_ns.unwrap_or_default();
        let mut tied = Vec::new();
        for (index, alternative) in eligible.iter().enumerate() {
            let value = alternative.objective_value_ns.unwrap_or_default();
            if within_indifference_band(first_value, value, objective.indifference_band_ppm)? {
                tied.push(index);
            }
        }
        tied.into_iter().min_by(|left, right| {
            eligible[*left]
                .artifact_hash
                .cmp(&eligible[*right].artifact_hash)
                .then_with(|| {
                    eligible[*left]
                        .terminal_node
                        .cmp(&eligible[*right].terminal_node)
                })
        })
    } else {
        None
    };
    let (selected_node, artifact_hash, hashes, value, tie, status) =
        if let Some(index) = selected_index {
            let selected = &eligible[index];
            let selected_value = selected.objective_value_ns.unwrap_or_default();
            let mut exact = false;
            let mut band = false;
            for (other_index, other) in eligible.iter().enumerate() {
                if other_index == index {
                    continue;
                }
                let other_value = other.objective_value_ns.unwrap_or_default();
                if selected_value == other_value {
                    exact = true;
                } else if within_indifference_band(
                    selected_value,
                    other_value,
                    objective.indifference_band_ppm,
                )? {
                    band = true;
                }
            }
            (
                Some(selected.terminal_node.clone()),
                selected.artifact_hash.clone(),
                selected.measurement_hashes.clone(),
                selected.objective_value_ns,
                Some(if exact {
                    MeasuredTieClassification::ExactTie
                } else if band {
                    MeasuredTieClassification::WithinIndifferenceBand
                } else {
                    MeasuredTieClassification::Distinct
                }),
                MeasuredRecommendationStatus::Selected,
            )
        } else {
            (
                None,
                None,
                Vec::new(),
                None,
                None,
                MeasuredRecommendationStatus::NoComparableMeasuredTerminal,
            )
        };
    let mut recommendation = MeasuredRecommendation {
        version: 1,
        search_result_hash: result.search_result_hash.clone(),
        measured_objective_hash: objective.measured_objective_hash.clone(),
        measurement_cohort_hash: cohort.measurement_cohort_hash.clone(),
        selected_terminal_node: selected_node,
        artifact_hash,
        measurement_hashes: hashes,
        objective_value_ns: value,
        tie_classification: tie,
        eligible_alternatives: eligible,
        ineligible_alternatives: ineligible,
        status,
        provenance: format!(
            "selected under descriptor {} from cohort {}; not proven fastest",
            objective.measured_objective_hash, cohort.measurement_cohort_hash
        ),
        measured_recommendation_hash: String::new(),
    };
    recommendation.measured_recommendation_hash = measured_recommendation_hash(&recommendation)?;
    Ok(recommendation)
}

pub(crate) fn terminal_artifact_hash(
    session: &SearchSession,
    terminal: &SearchNodeId,
) -> EvaluationResult<Option<String>> {
    let mut node = session.nodes.get(terminal).ok_or_else(|| {
        measured_error(
            EvaluationErrorCode::EvaluationMeasuredTerminalUnavailable,
            "terminal node is missing from the retained Stage 7A graph",
        )
    })?;
    let mut hashes = Vec::new();
    while let Some(edge_id) = &node.parent_edge {
        let edge = session.edges.get(edge_id).ok_or_else(|| {
            measured_error(
                EvaluationErrorCode::EvaluationMeasuredTerminalUnavailable,
                "terminal path references a missing Stage 7A edge",
            )
        })?;
        collect_named_string(
            &edge.compiler_outcome.response,
            "artifact_hash",
            &mut hashes,
        );
        node = session.nodes.get(&edge.parent).ok_or_else(|| {
            measured_error(
                EvaluationErrorCode::EvaluationMeasuredTerminalUnavailable,
                "terminal path references a missing Stage 7A parent",
            )
        })?;
    }
    hashes.sort();
    hashes.dedup();
    if hashes.len() > 1 {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasuredTerminalUnavailable,
            "terminal trajectory published multiple distinct artifact hashes",
        ));
    }
    Ok(hashes.pop())
}

fn collect_named_string(value: &Value, name: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(value) = map.get(name).and_then(Value::as_str) {
                output.push(value.to_owned());
            }
            for value in map.values() {
                collect_named_string(value, name, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_named_string(value, name, output);
            }
        }
        _ => {}
    }
}

fn unavailable_alternative(
    terminal_node: SearchNodeId,
    artifact_hash: Option<String>,
    reason: MeasuredUnavailableReason,
) -> MeasuredAlternative {
    MeasuredAlternative {
        terminal_node,
        artifact_hash,
        measurement_hashes: Vec::new(),
        objective_value_ns: None,
        unavailable: Some(reason),
    }
}

fn aggregate_records(
    records: &[&MeasurementCohortRecord],
    metric: MeasuredMetric,
    method: MeasurementAggregationMethod,
) -> EvaluationResult<u64> {
    let mut values = records
        .iter()
        .map(|entry| match metric {
            MeasuredMetric::MedianNs => entry.record.median_ns,
            MeasuredMetric::P95Ns => entry.record.p95_ns,
        })
        .collect::<Vec<_>>();
    values.sort_unstable();
    match method {
        MeasurementAggregationMethod::SingleRecordSummaryV1 if values.len() == 1 => Ok(values[0]),
        MeasurementAggregationMethod::MedianOfRecordSummariesV1 if !values.is_empty() => {
            Ok(values[(values.len() - 1) / 2])
        }
        _ => Err(measured_error(
            EvaluationErrorCode::EvaluationUnequalMeasurementRecords,
            "aggregation method and record cardinality are incompatible",
        )),
    }
}

/// Checked integer ppm indifference comparison.
pub fn within_indifference_band(left: u64, right: u64, ppm: u64) -> EvaluationResult<bool> {
    if ppm > 1_000_000 {
        return Err(measured_error(
            EvaluationErrorCode::EvaluationMeasuredObjectiveInvalid,
            "indifference band exceeds one million ppm",
        ));
    }
    let difference = left.abs_diff(right);
    let scaled_difference = difference
        .checked_mul(1_000_000)
        .ok_or_else(measured_overflow)?;
    let allowance = left
        .min(right)
        .checked_mul(ppm)
        .ok_or_else(measured_overflow)?;
    Ok(scaled_difference <= allowance)
}

/// Computes the independent cohort hash.
pub fn measurement_cohort_hash(cohort: &MeasurementCohort) -> EvaluationResult<String> {
    domain_hash_cleared(MEASUREMENT_COHORT_HASH_DOMAIN, cohort, |model| {
        model.measurement_cohort_hash.clear();
    })
}

/// Computes the independent measured-objective hash.
pub fn measured_objective_hash(
    objective: &MeasuredObjectiveDescriptor,
) -> EvaluationResult<String> {
    domain_hash_cleared(MEASURED_OBJECTIVE_HASH_DOMAIN, objective, |model| {
        model.measured_objective_hash.clear();
    })
}

/// Computes the independent measured-recommendation hash.
pub fn measured_recommendation_hash(
    recommendation: &MeasuredRecommendation,
) -> EvaluationResult<String> {
    domain_hash_cleared(
        MEASURED_RECOMMENDATION_HASH_DOMAIN,
        recommendation,
        |model| model.measured_recommendation_hash.clear(),
    )
}

fn measured_overflow() -> EvaluationDiagnostic {
    measured_error(
        EvaluationErrorCode::EvaluationMeasuredArithmeticOverflow,
        "checked measured-objective arithmetic overflow",
    )
}

fn measured_error(code: EvaluationErrorCode, message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_median_and_indifference_are_exact() {
        assert!(within_indifference_band(1_000, 1_001, 1_000).unwrap());
        assert!(!within_indifference_band(1_000, 1_002, 1_000).unwrap());
        assert_eq!(
            within_indifference_band(0, u64::MAX, 1_000_000)
                .unwrap_err()
                .code,
            EvaluationErrorCode::EvaluationMeasuredArithmeticOverflow
        );
    }

    #[test]
    fn hash_domains_are_distinct() {
        assert_ne!(
            MEASUREMENT_COHORT_HASH_DOMAIN,
            MEASURED_OBJECTIVE_HASH_DOMAIN
        );
        assert_ne!(
            MEASURED_OBJECTIVE_HASH_DOMAIN,
            MEASURED_RECOMMENDATION_HASH_DOMAIN
        );
    }
}
