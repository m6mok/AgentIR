//! Stage 7E integrated autotuning campaign contracts.
//!
//! A campaign composes the frozen Stage 7A search, Stage 7C acquisition,
//! Stage 7D recovery, and Stage 7B measured-recommendation records. It is an
//! evaluation-only lifecycle and has no compiler correctness or live artifact
//! publication authority.

use crate::{
    acquisition::{
        MeasurementAcquisitionCheckpoint, MeasurementAcquisitionPlan,
        MeasurementAcquisitionSession, MeasurementAcquisitionStoppingReason,
    },
    hashing::{domain_hash, domain_hash_cleared},
    measured::{
        MeasuredMetric, MeasuredObjectiveDescriptor, MeasuredRecommendation,
        MeasurementAggregationMethod, MeasurementCohort, MeasurementValidationPolicy,
    },
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, EvaluationTaskId},
    recovery::{MeasurementAcquisitionRecoveryCheckpoint, MeasurementAcquisitionRecoveryJournal},
    search::{SearchCheckpoint, SearchRanker, SearchSession, SearchStatus},
};
use agentir_core::backend_ir::HardwareBenchmarkConfig;
use serde::{Deserialize, Serialize};

/// Autotuning-campaign plan hash domain.
pub const AUTOTUNING_CAMPAIGN_PLAN_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.autotuning_campaign_plan.v1\0";
/// Autotuning-campaign session hash domain.
pub const AUTOTUNING_CAMPAIGN_SESSION_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.autotuning_campaign_session.v1\0";
/// Autotuning-campaign checkpoint hash domain.
pub const AUTOTUNING_CAMPAIGN_CHECKPOINT_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.autotuning_campaign_checkpoint.v1\0";
/// Autotuning-campaign trace hash domain.
pub const AUTOTUNING_CAMPAIGN_TRACE_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.autotuning_campaign_trace.v1\0";
/// Autotuning-campaign result hash domain.
pub const AUTOTUNING_CAMPAIGN_RESULT_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.autotuning_campaign_result.v1\0";

/// Operational Stage 7E limits excluded from every campaign semantic identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutotuningCampaignLimits {
    /// Maximum retained campaign sessions.
    pub retained_sessions: u64,
    /// Maximum distinct terminal artifacts materialized by one campaign.
    pub terminal_artifacts: u64,
    /// Maximum accepted campaign state transitions.
    pub transitions: u64,
    /// Maximum semantic campaign trace events.
    pub trace_events: u64,
    /// Maximum retained Stage 7A-7D record references.
    pub retained_substage_references: u64,
    /// Maximum encoded campaign-checkpoint bytes.
    pub checkpoint_bytes: u64,
    /// Maximum deterministic resume/replay work.
    pub resume_replay_work: u64,
    /// Maximum production requests attributed to campaign search.
    pub production_requests: u64,
    /// Maximum encoded evaluation archive v8 bytes.
    pub archive_v8_bytes: u64,
    /// Maximum total deterministic campaign semantic work.
    pub total_semantic_work: u64,
}

impl Default for AutotuningCampaignLimits {
    fn default() -> Self {
        Self {
            retained_sessions: 1_024,
            terminal_artifacts: 1_024,
            transitions: 100_000,
            trace_events: 100_000,
            retained_substage_references: 1_000_000,
            checkpoint_bytes: 256 * 1024 * 1024,
            resume_replay_work: 10_000_000,
            production_requests: 10_000_000,
            archive_v8_bytes: 256 * 1024 * 1024,
            total_semantic_work: 10_000_000,
        }
    }
}

/// V1 deterministic terminal-artifact materialization policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutotuningTerminalArtifactPolicy {
    /// Retain every distinct proved/offline-valid terminal, ordered by artifact hash.
    AllDistinctProvedOfflineValidByArtifactHashV1,
}

/// V1 recovery-policy anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutotuningRecoveryPolicy {
    /// Durable prepare and explicit retry under the frozen Stage 7D v1 contract.
    DurableSingleWriterExplicitRetryV1,
}

/// Immutable integrated campaign plan v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutotuningCampaignPlan {
    /// Plan schema version.
    pub version: u32,
    /// Exact evaluation corpus anchor.
    pub corpus_hash: String,
    /// Exact task identity.
    pub task_id: EvaluationTaskId,
    /// Exact Stage 7A initial/root anchor.
    pub initial_anchor_hash: String,
    /// Frozen Stage 7A objective anchor.
    pub search_objective_hash: String,
    /// Frozen Stage 7A plan anchor.
    pub search_plan_hash: String,
    /// Frozen Stage 6B/6C ranking policy anchor.
    pub ranking_policy_hash: String,
    /// Optional frozen learned-model anchor.
    pub learned_model_hash: Option<String>,
    /// Exact server-validated acquisition benchmark configuration.
    pub benchmark_config: HardwareBenchmarkConfig,
    /// Positive production records requested for every artifact.
    pub records_per_artifact: u64,
    /// Validation policy inherited by Stage 7C and Stage 7B.
    pub validation_policy: MeasurementValidationPolicy,
    /// Positive Stage 7C checkpoint cadence.
    pub acquisition_checkpoint_cadence_slots: u64,
    /// Frozen recovery policy/version.
    pub recovery_policy: AutotuningRecoveryPolicy,
    /// Terminal-only measured metric.
    pub measured_metric: MeasuredMetric,
    /// Stage 7B cross-record aggregation method.
    pub aggregation_method: MeasurementAggregationMethod,
    /// Checked integer indifference band.
    pub indifference_band_ppm: u64,
    /// Deterministic terminal artifact policy.
    pub terminal_artifact_policy: AutotuningTerminalArtifactPolicy,
    /// Explicit operational cap, semantic because it can truncate the artifact set.
    pub terminal_artifact_cap: u64,
    /// Independent campaign-plan hash.
    pub autotuning_campaign_plan_hash: String,
}

impl AutotuningCampaignPlan {
    /// Constructs a plan anchored to one pristine Stage 7A session and exact ranker.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search: &SearchSession,
        ranker: &SearchRanker,
        benchmark_config: HardwareBenchmarkConfig,
        records_per_artifact: u64,
        validation_policy: MeasurementValidationPolicy,
        acquisition_checkpoint_cadence_slots: u64,
        measured_metric: MeasuredMetric,
        aggregation_method: MeasurementAggregationMethod,
        indifference_band_ppm: u64,
        terminal_artifact_cap: u64,
    ) -> EvaluationResult<Self> {
        if search.work.semantic_expansions != 0
            || !matches!(search.status, SearchStatus::Running | SearchStatus::Bounded)
            || search.plan.ranking_policy_hash != ranker.descriptor().ranking_policy_hash
            || records_per_artifact == 0
            || acquisition_checkpoint_cadence_slots == 0
            || terminal_artifact_cap == 0
            || indifference_band_ppm > 1_000_000
            || benchmark_config.iterations == 0
            || benchmark_config.tensor_dimensions.is_empty()
            || benchmark_config.tensor_dimensions.contains(&0)
            || benchmark_config.input_distribution.is_empty()
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignPlanInvalid,
                "campaign plan inputs or frozen Stage 7A anchors are invalid",
            ));
        }
        let mut plan = Self {
            version: 1,
            corpus_hash: search.objective.corpus_hash.clone(),
            task_id: search.task_id.clone(),
            initial_anchor_hash: search.objective.initial_anchor_hash.clone(),
            search_objective_hash: search.objective.search_objective_hash.clone(),
            search_plan_hash: search.plan.search_plan_hash.clone(),
            ranking_policy_hash: search.plan.ranking_policy_hash.clone(),
            learned_model_hash: search.plan.learned_model_hash.clone(),
            benchmark_config,
            records_per_artifact,
            validation_policy,
            acquisition_checkpoint_cadence_slots,
            recovery_policy: AutotuningRecoveryPolicy::DurableSingleWriterExplicitRetryV1,
            measured_metric,
            aggregation_method,
            indifference_band_ppm,
            terminal_artifact_policy:
                AutotuningTerminalArtifactPolicy::AllDistinctProvedOfflineValidByArtifactHashV1,
            terminal_artifact_cap,
            autotuning_campaign_plan_hash: String::new(),
        };
        plan.autotuning_campaign_plan_hash = autotuning_campaign_plan_hash(&plan)?;
        Ok(plan)
    }

    /// Verifies the complete retained plan and immutable Stage 7A/ranker anchors.
    pub fn verify(&self, search: &SearchSession, ranker: &SearchRanker) -> EvaluationResult<()> {
        if self.version != 1
            || self.corpus_hash != search.objective.corpus_hash
            || self.task_id != search.task_id
            || self.initial_anchor_hash != search.objective.initial_anchor_hash
            || self.search_objective_hash != search.objective.search_objective_hash
            || self.search_plan_hash != search.plan.search_plan_hash
            || self.ranking_policy_hash != ranker.descriptor().ranking_policy_hash
            || self.learned_model_hash.as_deref() != ranker.learned_model_hash()
            || self.records_per_artifact == 0
            || self.acquisition_checkpoint_cadence_slots == 0
            || self.terminal_artifact_cap == 0
            || self.indifference_band_ppm > 1_000_000
            || self.autotuning_campaign_plan_hash != autotuning_campaign_plan_hash(self)?
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignPlanCorrupt,
                "campaign plan is corrupt or stale for its Stage 7A anchors",
            ));
        }
        Ok(())
    }
}

/// Integrated campaign lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutotuningCampaignStatus {
    /// Campaign exists but search has not advanced.
    Planned,
    /// Frozen Stage 7A search is running.
    Searching,
    /// Search stopped successfully or at its deterministic bound.
    SearchComplete,
    /// No proved/offline-valid terminal artifact was eligible.
    NoEligibleTerminal,
    /// A canonical Stage 7C acquisition plan exists.
    AcquisitionPrepared,
    /// One or more canonical acquisition slots remain.
    Acquiring,
    /// Stage 7D reconciliation or explicit retry is required.
    RecoveryRequired,
    /// Complete Stage 7C result is retained.
    AcquisitionComplete,
    /// Verified Stage 7B cohort is retained.
    CohortReady,
    /// Measured recommendation is retained and awaiting finalization.
    RecommendationReady,
    /// Immutable final campaign result is retained.
    Complete,
    /// Explicit cooperative cancellation was accepted.
    Cancelled,
    /// A typed external prerequisite blocks progress.
    Blocked,
    /// A typed internal/sub-stage failure stopped progress.
    Failed,
}

/// Typed campaign stopping/finalization reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutotuningCampaignStoppingReason {
    /// Full integrated lifecycle completed.
    CampaignComplete,
    /// Search produced no eligible terminal artifacts.
    NoEligibleTerminal,
    /// Search was cancelled or failed and acquisition was not started.
    SearchDidNotComplete,
    /// Acquisition requires Stage 7D recovery.
    RecoveryRequired,
    /// Acquisition stopped with its exact Stage 7C reason.
    AcquisitionStopped(MeasurementAcquisitionStoppingReason),
    /// Explicit campaign cancellation.
    Cancelled,
    /// A typed prerequisite blocked progress.
    Blocked,
    /// A typed sub-stage failure stopped progress.
    Failed,
}

/// Deterministic campaign work counters, excluded from semantic identities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutotuningCampaignWorkCounters {
    /// Accepted state transitions.
    pub transitions: u64,
    /// Stage 7A semantic expansion units requested through the campaign.
    pub search_expansions: u64,
    /// Terminal search nodes inspected.
    pub terminal_nodes_inspected: u64,
    /// Distinct eligible terminal artifacts retained.
    pub terminal_artifacts_retained: u64,
    /// Stage 7C canonical slots explicitly executed.
    pub acquisition_slots_executed: u64,
    /// Stage 7D durable preparations.
    pub durable_preparations: u64,
    /// Zero-device reconciliation operations.
    pub reconciliations: u64,
    /// Explicit retry authorizations.
    pub retry_authorizations: u64,
    /// Cohort creation operations.
    pub cohorts_created: u64,
    /// Measured recommendations computed.
    pub recommendations_computed: u64,
    /// Campaign checkpoints encoded.
    pub checkpoints: u64,
    /// Campaign resumes verified.
    pub resumes: u64,
    /// Full zero-device campaign replays.
    pub replays: u64,
    /// Production verifier requests attributed to Stage 7A.
    pub production_requests: u64,
    /// Hardware calls made only by explicit campaign execution.
    pub hardware_calls: u64,
    /// Hardware calls during replay; valid campaign replay requires zero.
    pub replay_hardware_calls: u64,
}

impl AutotuningCampaignWorkCounters {
    /// Returns a checked total of deterministic semantic work counters.
    pub fn semantic_total(&self) -> EvaluationResult<u64> {
        [
            self.transitions,
            self.search_expansions,
            self.terminal_nodes_inspected,
            self.terminal_artifacts_retained,
            self.acquisition_slots_executed,
            self.durable_preparations,
            self.reconciliations,
            self.retry_authorizations,
            self.cohorts_created,
            self.recommendations_computed,
            self.production_requests,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(campaign_overflow)
    }
}

/// One deterministic campaign lifecycle transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutotuningCampaignTraceEvent {
    /// Zero-based contiguous event sequence.
    pub sequence: u64,
    /// Stable transition kind.
    pub kind: String,
    /// State before the transition.
    pub from: AutotuningCampaignStatus,
    /// State after the transition.
    pub to: AutotuningCampaignStatus,
    /// Exact referenced sub-stage hash when applicable.
    pub substage_hash: Option<String>,
}

/// Immutable replayable campaign trace v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutotuningCampaignTrace {
    /// Trace schema version.
    pub version: u32,
    /// Exact campaign plan anchor.
    pub autotuning_campaign_plan_hash: String,
    /// Ordered deterministic events.
    pub events: Vec<AutotuningCampaignTraceEvent>,
    /// Independent trace hash.
    pub autotuning_campaign_trace_hash: String,
}

/// Complete serializable integrated campaign state v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutotuningCampaignSession {
    /// Session schema version.
    pub version: u32,
    /// Deterministic campaign identity derived from the plan.
    pub campaign_id: String,
    /// Immutable campaign plan.
    pub plan: AutotuningCampaignPlan,
    /// Exact frozen Stage 7A state.
    pub search: SearchSession,
    /// Exact Stage 6B/6C ranker required for replay.
    pub ranker: SearchRanker,
    /// Canonical eligible terminal artifact hashes.
    pub terminal_artifact_hashes: Vec<String>,
    /// Stage 7C plan when materialized.
    pub acquisition_plan: Option<MeasurementAcquisitionPlan>,
    /// Stage 7C session after explicit preflight.
    pub acquisition_session: Option<MeasurementAcquisitionSession>,
    /// Ordered Stage 7D journals, one per prepared canonical slot.
    pub recovery_journals: Vec<MeasurementAcquisitionRecoveryJournal>,
    /// Complete Stage 7B cohort after acquisition handoff.
    pub cohort: Option<MeasurementCohort>,
    /// Stage 7B measured objective.
    pub measured_objective: Option<MeasuredObjectiveDescriptor>,
    /// Stage 7B non-authoritative recommendation.
    pub recommendation: Option<MeasuredRecommendation>,
    /// Current campaign state.
    pub status: AutotuningCampaignStatus,
    /// Terminal or blocking reason when applicable.
    pub stopping_reason: Option<AutotuningCampaignStoppingReason>,
    /// Deterministic campaign trace.
    pub trace: AutotuningCampaignTrace,
    /// Operational deterministic work accounting.
    pub work: AutotuningCampaignWorkCounters,
    /// Final result once complete or terminal-without-acquisition.
    pub result: Option<AutotuningCampaignResult>,
    /// Independent current session hash.
    pub autotuning_campaign_session_hash: String,
}

/// Exact restartable campaign checkpoint v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutotuningCampaignCheckpoint {
    /// Checkpoint schema version.
    pub version: u32,
    /// Stable payload prefix.
    pub prefix: String,
    /// Exact plan anchor.
    pub autotuning_campaign_plan_hash: String,
    /// Exact base session anchor.
    pub autotuning_campaign_session_hash: String,
    /// Complete campaign state snapshot.
    pub session: Box<AutotuningCampaignSession>,
    /// Optional exact current Stage 7A checkpoint.
    pub search_checkpoint: Option<SearchCheckpoint>,
    /// Optional exact current Stage 7C checkpoint.
    pub acquisition_checkpoint: Option<MeasurementAcquisitionCheckpoint>,
    /// Optional exact current Stage 7D checkpoint.
    pub recovery_checkpoint: Option<MeasurementAcquisitionRecoveryCheckpoint>,
    /// Independent checkpoint hash.
    pub autotuning_campaign_checkpoint_hash: String,
}

/// Immutable final integrated campaign result v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutotuningCampaignResult {
    /// Result schema version.
    pub version: u32,
    /// Exact campaign plan anchor.
    pub autotuning_campaign_plan_hash: String,
    /// Final trace anchor.
    pub autotuning_campaign_trace_hash: String,
    /// Final campaign status.
    pub status: AutotuningCampaignStatus,
    /// Exact stopping reason.
    pub stopping_reason: AutotuningCampaignStoppingReason,
    /// Frozen Stage 7A result hash.
    pub search_result_hash: String,
    /// Canonical eligible terminal artifact set.
    pub terminal_artifact_hashes: Vec<String>,
    /// Stage 7C plan hash when acquisition existed.
    pub measurement_acquisition_plan_hash: Option<String>,
    /// Complete Stage 7C result hash when acquisition completed.
    pub measurement_acquisition_result_hash: Option<String>,
    /// Ordered Stage 7D journal hashes.
    pub recovery_journal_hashes: Vec<String>,
    /// Stage 7B cohort hash when acquisition completed.
    pub measurement_cohort_hash: Option<String>,
    /// Stage 7B objective hash when created.
    pub measured_objective_hash: Option<String>,
    /// Final Stage 7B recommendation hash when created.
    pub measured_recommendation_hash: Option<String>,
    /// Recommended artifact, never automatically published.
    pub recommended_artifact_hash: Option<String>,
    /// Explicit non-authoritative terminology.
    pub provenance: String,
    /// Operational work excluded from result identity.
    pub work: AutotuningCampaignWorkCounters,
    /// Independent campaign-result hash.
    pub autotuning_campaign_result_hash: String,
}

/// Stage 7E archive history presence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutotuningCampaignHistoryStatus {
    /// Pure v7-to-v8 migration retained no synthetic campaign history.
    #[default]
    NoCampaignHistory,
    /// One or more verified campaigns are retained.
    CampaignHistoryPresent,
}

/// Atomic Stage 7E archive attachment bundle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutotuningCampaignArchiveBundle {
    /// Retained terminal or restartable sessions.
    pub sessions: Vec<AutotuningCampaignSession>,
    /// Exact campaign checkpoints.
    pub checkpoints: Vec<AutotuningCampaignCheckpoint>,
    /// Final campaign results.
    pub results: Vec<AutotuningCampaignResult>,
    /// Exact zero-device replay status by result/session hash.
    pub replay_statuses: std::collections::BTreeMap<String, bool>,
}

/// Computes the independent campaign-plan hash.
pub fn autotuning_campaign_plan_hash(plan: &AutotuningCampaignPlan) -> EvaluationResult<String> {
    domain_hash_cleared(AUTOTUNING_CAMPAIGN_PLAN_HASH_DOMAIN, plan, |model| {
        model.autotuning_campaign_plan_hash.clear();
    })
}

/// Computes the current campaign-session hash with operational counters excluded.
pub fn autotuning_campaign_session_hash(
    session: &AutotuningCampaignSession,
) -> EvaluationResult<String> {
    let mut model = session.clone();
    model.autotuning_campaign_session_hash.clear();
    model.work = AutotuningCampaignWorkCounters::default();
    domain_hash(AUTOTUNING_CAMPAIGN_SESSION_HASH_DOMAIN, &model)
}

/// Computes the independent campaign-checkpoint hash.
pub fn autotuning_campaign_checkpoint_hash(
    checkpoint: &AutotuningCampaignCheckpoint,
) -> EvaluationResult<String> {
    domain_hash_cleared(
        AUTOTUNING_CAMPAIGN_CHECKPOINT_HASH_DOMAIN,
        checkpoint,
        |model| model.autotuning_campaign_checkpoint_hash.clear(),
    )
}

/// Computes the independent campaign-trace hash.
pub fn autotuning_campaign_trace_hash(trace: &AutotuningCampaignTrace) -> EvaluationResult<String> {
    domain_hash_cleared(AUTOTUNING_CAMPAIGN_TRACE_HASH_DOMAIN, trace, |model| {
        model.autotuning_campaign_trace_hash.clear();
    })
}

/// Computes the independent campaign-result hash with operational work excluded.
pub fn autotuning_campaign_result_hash(
    result: &AutotuningCampaignResult,
) -> EvaluationResult<String> {
    let mut model = result.clone();
    model.autotuning_campaign_result_hash.clear();
    model.work = AutotuningCampaignWorkCounters::default();
    domain_hash(AUTOTUNING_CAMPAIGN_RESULT_HASH_DOMAIN, &model)
}

pub(crate) fn campaign_error(
    code: EvaluationErrorCode,
    message: impl Into<String>,
) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}

pub(crate) fn campaign_overflow() -> EvaluationDiagnostic {
    campaign_error(
        EvaluationErrorCode::EvaluationAutotuningCampaignCounterOverflow,
        "checked autotuning campaign counter overflow",
    )
}
