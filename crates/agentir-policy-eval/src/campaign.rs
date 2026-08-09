//! Stage 7E integrated autotuning campaign contracts.
//!
//! A campaign composes the frozen Stage 7A search, Stage 7C acquisition,
//! Stage 7D recovery, and Stage 7B measured-recommendation records. It is an
//! evaluation-only lifecycle and has no compiler correctness or live artifact
//! publication authority.

use crate::{
    acquisition::{
        MeasurementAcquisitionCatalog, MeasurementAcquisitionCheckpoint,
        MeasurementAcquisitionExecutor, MeasurementAcquisitionFailurePolicy,
        MeasurementAcquisitionLimits, MeasurementAcquisitionOrderingPolicy,
        MeasurementAcquisitionPlan, MeasurementAcquisitionPlanRequest,
        MeasurementAcquisitionSession, MeasurementAcquisitionStatus,
        MeasurementAcquisitionStoppingReason, MeasurementAcquisitionStore,
    },
    hashing::{domain_hash, domain_hash_cleared},
    measured::{
        MeasuredMetric, MeasuredObjectiveDescriptor, MeasuredRecommendation,
        MeasurementAggregationMethod, MeasurementCohort, MeasurementCohortRecord,
        MeasurementCohortRequest, MeasurementValidationPolicy, measured_recommendation,
        measurement_cohort_from_verified_records, terminal_artifact_hash,
        verify_measurement_cohort,
    },
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, EvaluationTaskId},
    recovery::{
        MeasurementAcquisitionRecoveryAnchors, MeasurementAcquisitionRecoveryCheckpoint,
        MeasurementAcquisitionRecoveryFaultBoundary, MeasurementAcquisitionRecoveryJournal,
        MeasurementAcquisitionRecoveryLimits, MeasurementAcquisitionRecoveryWorkCounters,
        ReconciliationOutcome, RecoveryStatus,
    },
    search::{
        SearchCheckpoint, SearchLimits, SearchRanker, SearchSession, SearchStatus, replay_search,
    },
};
use agentir_core::backend_ir::{ArtifactStatus, HardwareBenchmarkConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

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

impl AutotuningCampaignHistoryStatus {
    /// Returns whether an older archive contains no Stage 7E history.
    #[must_use]
    pub fn is_no_history(&self) -> bool {
        *self == Self::NoCampaignHistory
    }
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

impl AutotuningCampaignSession {
    /// Starts a campaign without advancing search or touching hardware.
    pub fn start(
        search: SearchSession,
        ranker: SearchRanker,
        plan: AutotuningCampaignPlan,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<Self> {
        plan.verify(&search, &ranker)?;
        campaign_limit(1, limits.retained_sessions, "retained_sessions")?;
        let mut trace = AutotuningCampaignTrace {
            version: 1,
            autotuning_campaign_plan_hash: plan.autotuning_campaign_plan_hash.clone(),
            events: vec![AutotuningCampaignTraceEvent {
                sequence: 0,
                kind: "campaign_started_without_hardware".to_owned(),
                from: AutotuningCampaignStatus::Planned,
                to: AutotuningCampaignStatus::Planned,
                substage_hash: Some(plan.search_plan_hash.clone()),
            }],
            autotuning_campaign_trace_hash: String::new(),
        };
        trace.autotuning_campaign_trace_hash = autotuning_campaign_trace_hash(&trace)?;
        let mut session = Self {
            version: 1,
            campaign_id: format!(
                "autotuning-campaign-{}",
                &plan.autotuning_campaign_plan_hash[..16]
            ),
            plan,
            search,
            ranker,
            terminal_artifact_hashes: Vec::new(),
            acquisition_plan: None,
            acquisition_session: None,
            recovery_journals: Vec::new(),
            cohort: None,
            measured_objective: None,
            recommendation: None,
            status: AutotuningCampaignStatus::Planned,
            stopping_reason: None,
            trace,
            work: AutotuningCampaignWorkCounters::default(),
            result: None,
            autotuning_campaign_session_hash: String::new(),
        };
        session.refresh_hash()?;
        session.verify_structure(limits)?;
        Ok(session)
    }

    /// Advances only the frozen Stage 7A search and performs zero hardware work.
    pub fn advance_search(
        &mut self,
        base_campaign_hash: &str,
        maximum_work_units: u64,
        search_limits: &SearchLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if !matches!(
            self.status,
            AutotuningCampaignStatus::Planned | AutotuningCampaignStatus::Searching
        ) {
            return Err(invalid_transition(self.status, "advance_search"));
        }
        let mut staged = self.clone();
        let previous_search_work = staged.search.work.semantic_expansions;
        staged
            .search
            .advance(maximum_work_units, &staged.ranker, search_limits)?;
        let delta = staged
            .search
            .work
            .semantic_expansions
            .checked_sub(previous_search_work)
            .ok_or_else(campaign_overflow)?;
        staged.work.search_expansions = campaign_add(staged.work.search_expansions, delta)?;
        staged.work.production_requests = staged.search.work.production_requests_submitted;
        campaign_limit(
            staged.work.production_requests,
            limits.production_requests,
            "production_requests",
        )?;
        let (next, reason) = match staged.search.status {
            SearchStatus::Running => (AutotuningCampaignStatus::Searching, None),
            SearchStatus::Complete | SearchStatus::Bounded => {
                (AutotuningCampaignStatus::SearchComplete, None)
            }
            SearchStatus::Cancelled => (
                AutotuningCampaignStatus::Cancelled,
                Some(AutotuningCampaignStoppingReason::SearchDidNotComplete),
            ),
            SearchStatus::Failed => (
                AutotuningCampaignStatus::Failed,
                Some(AutotuningCampaignStoppingReason::SearchDidNotComplete),
            ),
        };
        let substage_hash = staged
            .search
            .result
            .as_ref()
            .map(|result| result.search_result_hash.clone());
        staged.transition("search_advanced", next, substage_hash, limits)?;
        staged.stopping_reason = reason;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }

    /// Materializes the canonical eligible terminal set and Stage 7C plan.
    ///
    /// This operation performs no preflight, benchmark, device, or executor call.
    pub fn prepare_acquisition(
        &mut self,
        base_campaign_hash: &str,
        catalog: &MeasurementAcquisitionCatalog,
        acquisition_limits: &MeasurementAcquisitionLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if self.status != AutotuningCampaignStatus::SearchComplete {
            return Err(invalid_transition(self.status, "prepare_acquisition"));
        }
        if !matches!(
            self.search.status,
            SearchStatus::Complete | SearchStatus::Bounded
        ) {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                "failed, cancelled, or running search cannot begin acquisition",
            ));
        }
        let mut staged = self.clone();
        let terminal_ids = staged
            .search
            .nodes
            .values()
            .filter(|node| node.terminal)
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        staged.work.terminal_nodes_inspected = campaign_add(
            staged.work.terminal_nodes_inspected,
            campaign_count(terminal_ids.len())?,
        )?;
        let mut artifacts = BTreeSet::new();
        for terminal in terminal_ids {
            let Some(hash) = terminal_artifact_hash(&staged.search, &terminal)? else {
                continue;
            };
            if catalog.artifacts.get(&hash).is_some_and(|artifact| {
                artifact.offline_valid
                    && matches!(
                        artifact.status,
                        ArtifactStatus::Validated | ArtifactStatus::Sealed
                    )
            }) {
                artifacts.insert(hash);
            }
        }
        let cap =
            usize::try_from(staged.plan.terminal_artifact_cap).map_err(|_| campaign_overflow())?;
        let artifact_hashes = artifacts.into_iter().take(cap).collect::<Vec<_>>();
        campaign_limit(
            campaign_count(artifact_hashes.len())?,
            limits.terminal_artifacts,
            "terminal_artifacts",
        )?;
        staged.work.terminal_artifacts_retained = campaign_add(
            staged.work.terminal_artifacts_retained,
            campaign_count(artifact_hashes.len())?,
        )?;
        staged.terminal_artifact_hashes.clone_from(&artifact_hashes);
        if artifact_hashes.is_empty() {
            staged.stopping_reason = Some(AutotuningCampaignStoppingReason::NoEligibleTerminal);
            staged.transition(
                "no_eligible_terminal_artifact",
                AutotuningCampaignStatus::NoEligibleTerminal,
                staged
                    .search
                    .result
                    .as_ref()
                    .map(|result| result.search_result_hash.clone()),
                limits,
            )?;
            staged.result = Some(staged.result_projection(
                AutotuningCampaignStatus::NoEligibleTerminal,
                AutotuningCampaignStoppingReason::NoEligibleTerminal,
            )?);
            staged.refresh_hash()?;
            staged.verify_structure(limits)?;
            *self = staged;
            return Ok(self.status);
        }
        let acquisition_plan = MeasurementAcquisitionPlan::new_with_limits(
            catalog,
            MeasurementAcquisitionPlanRequest {
                corpus_hash: staged.plan.corpus_hash.clone(),
                task_id: staged.plan.task_id.clone(),
                root_anchor_hash: staged.plan.initial_anchor_hash.clone(),
                artifact_hashes,
                benchmark_config: staged.plan.benchmark_config.clone(),
                records_per_artifact: staged.plan.records_per_artifact,
                validation_policy: staged.plan.validation_policy,
                ordering_policy: MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
                failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
                checkpoint_cadence_slots: staged.plan.acquisition_checkpoint_cadence_slots,
            },
            acquisition_limits,
        )?;
        let plan_hash = acquisition_plan.measurement_acquisition_plan_hash.clone();
        staged.acquisition_plan = Some(acquisition_plan);
        staged.transition(
            "acquisition_plan_materialized_without_hardware",
            AutotuningCampaignStatus::AcquisitionPrepared,
            Some(plan_hash),
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }
}

impl AutotuningCampaignSession {
    /// Creates an exact campaign checkpoint without executor or device access.
    pub fn checkpoint(
        &mut self,
        base_campaign_hash: &str,
        search_limits: &SearchLimits,
        recovery_limits: &MeasurementAcquisitionRecoveryLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignCheckpoint> {
        self.verify_base(base_campaign_hash, limits)?;
        let mut staged = self.clone();
        staged.work.checkpoints = campaign_add(staged.work.checkpoints, 1)?;
        let search_checkpoint = Some(staged.search.checkpoint(search_limits)?);
        let acquisition_checkpoint = staged
            .acquisition_session
            .as_ref()
            .map(MeasurementAcquisitionSession::checkpoint)
            .transpose()?;
        let recovery_checkpoint = if let (Some(journal), Some(session)) = (
            staged.recovery_journals.last_mut(),
            staged.acquisition_session.as_ref(),
        ) {
            Some(journal.checkpoint(session, recovery_limits)?)
        } else {
            None
        };
        staged.refresh_hash()?;
        let mut checkpoint = AutotuningCampaignCheckpoint {
            version: 1,
            prefix: "agentir-autotuning-campaign-checkpoint-v1".to_owned(),
            autotuning_campaign_plan_hash: staged.plan.autotuning_campaign_plan_hash.clone(),
            autotuning_campaign_session_hash: staged.autotuning_campaign_session_hash.clone(),
            session: Box::new(staged.clone()),
            search_checkpoint,
            acquisition_checkpoint,
            recovery_checkpoint,
            autotuning_campaign_checkpoint_hash: String::new(),
        };
        checkpoint.autotuning_campaign_checkpoint_hash =
            autotuning_campaign_checkpoint_hash(&checkpoint)?;
        let bytes = serde_json::to_vec(&checkpoint).map_err(|error| {
            campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                format!("campaign checkpoint encoding failed: {error}"),
            )
        })?;
        campaign_limit(
            campaign_count(bytes.len())?,
            limits.checkpoint_bytes,
            "checkpoint_bytes",
        )?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(checkpoint)
    }

    /// Restores and revalidates a checkpoint with zero hardware calls.
    pub fn resume<S: MeasurementAcquisitionStore>(
        checkpoint: &AutotuningCampaignCheckpoint,
        store: &S,
        catalog: Option<&MeasurementAcquisitionCatalog>,
        search_limits: &SearchLimits,
        recovery_limits: &MeasurementAcquisitionRecoveryLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<Self> {
        if checkpoint.version != 1
            || checkpoint.prefix != "agentir-autotuning-campaign-checkpoint-v1"
            || checkpoint.autotuning_campaign_checkpoint_hash
                != autotuning_campaign_checkpoint_hash(checkpoint)?
            || checkpoint.autotuning_campaign_plan_hash
                != checkpoint.session.plan.autotuning_campaign_plan_hash
            || checkpoint.autotuning_campaign_session_hash
                != checkpoint.session.autotuning_campaign_session_hash
            || checkpoint.autotuning_campaign_session_hash
                != autotuning_campaign_session_hash(&checkpoint.session)?
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                "campaign checkpoint prefix, version, digest, or anchors are corrupt",
            ));
        }
        let mut resumed = (*checkpoint.session).clone();
        if let Some(search_checkpoint) = &checkpoint.search_checkpoint {
            let restored = SearchSession::resume(
                search_checkpoint,
                &resumed.search.corpus,
                &resumed.ranker,
                search_limits,
            )?;
            if restored != resumed.search {
                return Err(campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                    "campaign Stage 7A checkpoint differs from its session",
                ));
            }
        }
        if let Some(acquisition_checkpoint) = &checkpoint.acquisition_checkpoint {
            let catalog = catalog.ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignAnchorStale,
                    "campaign resume requires its acquisition catalog",
                )
            })?;
            let retained = resumed.acquisition_session.as_ref().ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                    "campaign acquisition checkpoint has no retained session",
                )
            })?;
            let restored = MeasurementAcquisitionSession::resume(
                acquisition_checkpoint,
                store,
                catalog,
                &retained.preflight,
            )?;
            if restored.plan != retained.plan
                || restored.slots != retained.slots
                || restored.next_slot != retained.next_slot
            {
                return Err(campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                    "campaign Stage 7C checkpoint differs from its retained state",
                ));
            }
        }
        if let Some(recovery_checkpoint) = &checkpoint.recovery_checkpoint {
            let catalog = catalog.ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignAnchorStale,
                    "campaign recovery resume requires its acquisition catalog",
                )
            })?;
            let session = resumed.acquisition_session.as_ref().ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                    "campaign recovery checkpoint has no Stage 7C session",
                )
            })?;
            let restored = MeasurementAcquisitionRecoveryJournal::restore_checkpoint(
                recovery_checkpoint,
                session,
                store,
                catalog,
                recovery_limits,
            )?;
            if resumed
                .recovery_journals
                .last()
                .is_none_or(|journal| journal != &restored)
            {
                return Err(campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                    "campaign Stage 7D checkpoint differs from its retained journal",
                ));
            }
        }
        resumed.work.resumes = campaign_add(resumed.work.resumes, 1)?;
        resumed.refresh_hash()?;
        resumed.verify_structure(limits)?;
        Ok(resumed)
    }

    /// Replays all retained Stage 7A-7D records and Stage 7B selection with zero hardware calls.
    pub fn replay<S: MeasurementAcquisitionStore>(
        &self,
        store: &S,
        catalog: Option<&MeasurementAcquisitionCatalog>,
        search_limits: &SearchLimits,
        recovery_limits: &MeasurementAcquisitionRecoveryLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignResult> {
        self.verify_structure(limits)?;
        replay_search(&self.search, &self.ranker, search_limits)?;
        let mut replay_work = campaign_add(
            campaign_count(self.search.nodes.len())?,
            campaign_count(self.search.edges.len())?,
        )?;
        if let Some(acquisition) = &self.acquisition_session {
            let catalog = catalog.ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignAnchorStale,
                    "campaign replay requires its acquisition catalog",
                )
            })?;
            if acquisition.status == MeasurementAcquisitionStatus::Running {
                acquisition.verify(store, catalog)?;
            } else {
                acquisition.replay(store, catalog)?;
            }
            replay_work = campaign_add(replay_work, campaign_count(acquisition.slots.len())?)?;
            for journal in &self.recovery_journals {
                journal.replay(acquisition, store, catalog, recovery_limits)?;
                replay_work = campaign_add(replay_work, campaign_count(journal.trace.len())?)?;
            }
        }
        campaign_limit(replay_work, limits.resume_replay_work, "resume_replay_work")?;
        if self.work.replay_hardware_calls != 0 {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAcquisitionReplayHardwareWork,
                "campaign replay attempted hardware work",
            ));
        }
        if let Some(cohort) = &self.cohort {
            verify_measurement_cohort(cohort)?;
            let objective = self.measured_objective.as_ref().ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationMeasuredObjectiveCorrupt,
                    "campaign measured objective is missing",
                )
            })?;
            objective.validate(cohort)?;
            let replayed = measured_recommendation(&self.search, cohort, objective)?;
            if self.recommendation.as_ref() != Some(&replayed) {
                return Err(campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignReplayMismatch,
                    "campaign measured recommendation differs during replay",
                ));
            }
        }
        let result = self.result.clone().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                "campaign replay requires a terminal campaign result",
            )
        })?;
        if result.autotuning_campaign_result_hash != autotuning_campaign_result_hash(&result)? {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignReplayMismatch,
                "campaign result hash differs during replay",
            ));
        }
        Ok(result)
    }

    /// Cancels a non-terminal campaign at a deterministic sub-stage boundary.
    pub fn cancel(
        &mut self,
        base_campaign_hash: &str,
        search_limits: &SearchLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if matches!(
            self.status,
            AutotuningCampaignStatus::Complete
                | AutotuningCampaignStatus::NoEligibleTerminal
                | AutotuningCampaignStatus::Cancelled
                | AutotuningCampaignStatus::Failed
        ) {
            return Err(invalid_transition(self.status, "cancel"));
        }
        let mut staged = self.clone();
        if staged.search.status == SearchStatus::Running {
            staged.search.request_cancellation();
            staged.search.advance(1, &staged.ranker, search_limits)?;
        }
        if let Some(acquisition) = staged.acquisition_session.as_mut()
            && acquisition.status == MeasurementAcquisitionStatus::Running
        {
            acquisition.cancel()?;
        }
        staged.stopping_reason = Some(AutotuningCampaignStoppingReason::Cancelled);
        staged.transition(
            "campaign_cancelled_at_substage_boundary",
            AutotuningCampaignStatus::Cancelled,
            None,
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }

    fn verify_base(
        &self,
        base_campaign_hash: &str,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<()> {
        self.verify_structure(limits)?;
        if self.autotuning_campaign_session_hash != base_campaign_hash {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignAnchorStale,
                "campaign base session hash is stale",
            )
            .expected_actual(
                json!(self.autotuning_campaign_session_hash),
                json!(base_campaign_hash),
            ));
        }
        Ok(())
    }

    /// Verifies the campaign hash, trace, plan, canonical references, and limits.
    pub fn verify_structure(&self, limits: &AutotuningCampaignLimits) -> EvaluationResult<()> {
        self.plan.verify(&self.search, &self.ranker)?;
        if self.version != 1
            || self.campaign_id
                != format!(
                    "autotuning-campaign-{}",
                    &self.plan.autotuning_campaign_plan_hash[..16]
                )
            || self.trace.version != 1
            || self.trace.autotuning_campaign_plan_hash != self.plan.autotuning_campaign_plan_hash
            || self.trace.autotuning_campaign_trace_hash
                != autotuning_campaign_trace_hash(&self.trace)?
            || self
                .trace
                .events
                .iter()
                .enumerate()
                .any(|(index, event)| event.sequence != u64::try_from(index).unwrap_or(u64::MAX))
            || self.autotuning_campaign_session_hash != autotuning_campaign_session_hash(self)?
            || self
                .terminal_artifact_hashes
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignCheckpointCorrupt,
                "campaign session hash, trace, or canonical references are corrupt",
            ));
        }
        campaign_limit(
            campaign_count(self.trace.events.len())?,
            limits.trace_events,
            "trace_events",
        )?;
        campaign_limit(self.work.transitions, limits.transitions, "transitions")?;
        campaign_limit(
            campaign_count(self.terminal_artifact_hashes.len())?,
            limits.terminal_artifacts,
            "terminal_artifacts",
        )?;
        let substage_references = campaign_add(
            campaign_add(
                campaign_count(self.search.nodes.len())?,
                campaign_count(self.search.edges.len())?,
            )?,
            campaign_count(self.recovery_journals.len())?,
        )?;
        campaign_limit(
            substage_references,
            limits.retained_substage_references,
            "retained_substage_references",
        )?;
        campaign_limit(
            self.work.semantic_total()?,
            limits.total_semantic_work,
            "total_semantic_work",
        )?;
        if let Some(result) = &self.result
            && result.autotuning_campaign_result_hash != autotuning_campaign_result_hash(result)?
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignReplayMismatch,
                "campaign result hash is corrupt",
            ));
        }
        Ok(())
    }

    fn transition(
        &mut self,
        kind: &str,
        to: AutotuningCampaignStatus,
        substage_hash: Option<String>,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<()> {
        let next_sequence = campaign_count(self.trace.events.len())?;
        campaign_limit(
            campaign_add(next_sequence, 1)?,
            limits.trace_events,
            "trace_events",
        )?;
        campaign_limit(
            campaign_add(self.work.transitions, 1)?,
            limits.transitions,
            "transitions",
        )?;
        let from = self.status;
        self.trace.events.push(AutotuningCampaignTraceEvent {
            sequence: next_sequence,
            kind: kind.to_owned(),
            from,
            to,
            substage_hash,
        });
        self.trace.autotuning_campaign_trace_hash = autotuning_campaign_trace_hash(&self.trace)?;
        self.status = to;
        self.work.transitions = campaign_add(self.work.transitions, 1)?;
        Ok(())
    }

    fn result_projection(
        &self,
        status: AutotuningCampaignStatus,
        stopping_reason: AutotuningCampaignStoppingReason,
    ) -> EvaluationResult<AutotuningCampaignResult> {
        let search_result_hash = self
            .search
            .result
            .as_ref()
            .map(|result| result.search_result_hash.clone())
            .ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationSearchIncomplete,
                    "campaign result requires a stopped Stage 7A result",
                )
            })?;
        let acquisition_result_hash = self
            .acquisition_session
            .as_ref()
            .filter(|session| session.status != MeasurementAcquisitionStatus::Running)
            .map(MeasurementAcquisitionSession::result)
            .transpose()?
            .map(|result| result.measurement_acquisition_result_hash);
        let mut result = AutotuningCampaignResult {
            version: 1,
            autotuning_campaign_plan_hash: self.plan.autotuning_campaign_plan_hash.clone(),
            autotuning_campaign_trace_hash: self.trace.autotuning_campaign_trace_hash.clone(),
            status,
            stopping_reason,
            search_result_hash,
            terminal_artifact_hashes: self.terminal_artifact_hashes.clone(),
            measurement_acquisition_plan_hash: self
                .acquisition_plan
                .as_ref()
                .map(|plan| plan.measurement_acquisition_plan_hash.clone()),
            measurement_acquisition_result_hash: acquisition_result_hash,
            recovery_journal_hashes: self
                .recovery_journals
                .iter()
                .map(|journal| {
                    journal
                        .measurement_acquisition_recovery_journal_hash
                        .clone()
                })
                .collect(),
            measurement_cohort_hash: self
                .cohort
                .as_ref()
                .map(|cohort| cohort.measurement_cohort_hash.clone()),
            measured_objective_hash: self
                .measured_objective
                .as_ref()
                .map(|objective| objective.measured_objective_hash.clone()),
            measured_recommendation_hash: self
                .recommendation
                .as_ref()
                .map(|recommendation| recommendation.measured_recommendation_hash.clone()),
            recommended_artifact_hash: self
                .recommendation
                .as_ref()
                .and_then(|recommendation| recommendation.artifact_hash.clone()),
            provenance: "integrated Stage 7A-7E evaluation campaign; no live artifact publication, correctness advancement, exactly-once hardware claim, proven-fastest claim, portability claim, statistical-significance claim, or global-optimality claim".to_owned(),
            work: self.work.clone(),
            autotuning_campaign_result_hash: String::new(),
        };
        result.autotuning_campaign_result_hash = autotuning_campaign_result_hash(&result)?;
        Ok(result)
    }

    fn refresh_hash(&mut self) -> EvaluationResult<()> {
        self.autotuning_campaign_session_hash = autotuning_campaign_session_hash(self)?;
        Ok(())
    }
}

impl AutotuningCampaignSession {
    /// Explicitly preflights, durably prepares, and executes one canonical slot.
    ///
    /// This is the only campaign API that accepts an executor or may perform
    /// device/benchmark work.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_prepared<S, E>(
        &mut self,
        base_campaign_hash: &str,
        store: &mut S,
        catalog: &MeasurementAcquisitionCatalog,
        workspace: Option<&agentir_core::Workspace>,
        executor: &mut E,
        fault: Option<MeasurementAcquisitionRecoveryFaultBoundary>,
        recovery_limits: &MeasurementAcquisitionRecoveryLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus>
    where
        S: MeasurementAcquisitionStore,
        E: MeasurementAcquisitionExecutor,
    {
        self.verify_base(base_campaign_hash, limits)?;
        if !matches!(
            self.status,
            AutotuningCampaignStatus::AcquisitionPrepared
                | AutotuningCampaignStatus::Acquiring
                | AutotuningCampaignStatus::RecoveryRequired
        ) {
            return Err(invalid_transition(self.status, "execute_prepared"));
        }
        if self.status == AutotuningCampaignStatus::RecoveryRequired
            && self
                .recovery_journals
                .last()
                .is_none_or(|journal| journal.status != RecoveryStatus::RetryAuthorized)
        {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryRetryNotAuthorized,
                "indeterminate campaign slot requires reconciliation and explicit retry",
            ));
        }
        let mut staged = self.clone();
        let mut staged_store = store.clone();
        if staged.acquisition_session.is_none() {
            let plan = staged.acquisition_plan.clone().ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                    "campaign acquisition plan is missing",
                )
            })?;
            staged.acquisition_session = Some(MeasurementAcquisitionSession::start(
                plan, catalog, workspace, executor,
            )?);
        }
        let session = staged.acquisition_session.as_mut().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                "campaign acquisition session is missing",
            )
        })?;
        let needs_journal = staged.recovery_journals.last().is_none_or(|journal| {
            !matches!(
                journal.status,
                RecoveryStatus::Prepared | RecoveryStatus::RetryAuthorized
            ) || journal.slot_index != session.next_slot
        });
        if needs_journal {
            let journal = MeasurementAcquisitionRecoveryJournal::prepare(
                session,
                &staged_store,
                catalog,
                recovery_limits,
            )?;
            staged.recovery_journals.push(journal);
            staged.work.durable_preparations = campaign_add(staged.work.durable_preparations, 1)?;
        }
        let journal = staged.recovery_journals.last_mut().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryNotPrepared,
                "campaign durable recovery journal is missing",
            )
        })?;
        let before_calls = journal.work.device_calls;
        let status = journal.execute(
            session,
            &mut staged_store,
            catalog,
            workspace,
            executor,
            fault,
            recovery_limits,
        )?;
        staged.work.acquisition_slots_executed =
            campaign_add(staged.work.acquisition_slots_executed, 1)?;
        staged.work.hardware_calls = campaign_add(
            staged.work.hardware_calls,
            journal
                .work
                .device_calls
                .checked_sub(before_calls)
                .ok_or_else(campaign_overflow)?,
        )?;
        let next = if matches!(status, RecoveryStatus::IndeterminateAfterCrash) {
            staged.stopping_reason = Some(AutotuningCampaignStoppingReason::RecoveryRequired);
            AutotuningCampaignStatus::RecoveryRequired
        } else if session.status == MeasurementAcquisitionStatus::Complete {
            staged.stopping_reason = None;
            AutotuningCampaignStatus::AcquisitionComplete
        } else if session.status == MeasurementAcquisitionStatus::Running {
            staged.stopping_reason = None;
            AutotuningCampaignStatus::Acquiring
        } else {
            staged.stopping_reason = session
                .stopping_reason
                .clone()
                .map(AutotuningCampaignStoppingReason::AcquisitionStopped);
            AutotuningCampaignStatus::Failed
        };
        let journal_hash = journal
            .measurement_acquisition_recovery_journal_hash
            .clone();
        staged.transition(
            "explicit_prepared_slot_execution",
            next,
            Some(journal_hash),
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *store = staged_store;
        *self = staged;
        Ok(self.status)
    }

    /// Reconciles the latest indeterminate attempt without hardware work.
    ///
    /// A true `authorize_retry` is a separate explicit Stage 7D authorization
    /// after a zero-publication observation; it never runs the retry itself.
    pub fn reconcile<S: MeasurementAcquisitionStore>(
        &mut self,
        base_campaign_hash: &str,
        store: &S,
        catalog: &MeasurementAcquisitionCatalog,
        authorize_retry: bool,
        recovery_limits: &MeasurementAcquisitionRecoveryLimits,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if self.status != AutotuningCampaignStatus::RecoveryRequired {
            return Err(invalid_transition(self.status, "reconcile"));
        }
        let mut staged = self.clone();
        let session = staged.acquisition_session.as_mut().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                "campaign recovery has no Stage 7C session",
            )
        })?;
        let journal = staged.recovery_journals.last_mut().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryNotPrepared,
                "campaign recovery journal is missing",
            )
        })?;
        let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(session);
        let result = journal.reconcile(session, store, catalog, &anchors, recovery_limits)?;
        staged.work.reconciliations = campaign_add(staged.work.reconciliations, 1)?;
        let mut event = "campaign_reconciliation_observed";
        if result.outcome == ReconciliationOutcome::NoPublicationObserved && authorize_retry {
            journal.authorize_retry(session, store, catalog, recovery_limits)?;
            staged.work.retry_authorizations = campaign_add(staged.work.retry_authorizations, 1)?;
            event = "campaign_reconciliation_and_explicit_retry_authorization";
        }
        let next = if session.status == MeasurementAcquisitionStatus::Complete {
            staged.stopping_reason = None;
            AutotuningCampaignStatus::AcquisitionComplete
        } else if journal.status == RecoveryStatus::Reconciled {
            staged.stopping_reason = None;
            AutotuningCampaignStatus::Acquiring
        } else {
            staged.stopping_reason = Some(AutotuningCampaignStoppingReason::RecoveryRequired);
            AutotuningCampaignStatus::RecoveryRequired
        };
        staged.transition(
            event,
            next,
            Some(result.measurement_acquisition_reconciliation_hash),
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }

    /// Creates the exact Stage 7B cohort from one complete Stage 7C result.
    pub fn create_cohort<S: MeasurementAcquisitionStore>(
        &mut self,
        base_campaign_hash: &str,
        store: &S,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if self.status != AutotuningCampaignStatus::AcquisitionComplete {
            return Err(invalid_transition(self.status, "create_cohort"));
        }
        let mut staged = self.clone();
        let acquisition = staged.acquisition_session.as_ref().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
                "complete campaign acquisition session is missing",
            )
        })?;
        let result = acquisition.result()?;
        if result.status != MeasurementAcquisitionStatus::Complete {
            return Err(campaign_error(
                EvaluationErrorCode::EvaluationAcquisitionUnequalRecords,
                "campaign cohort requires one complete Stage 7C result",
            ));
        }
        let records = result
            .measurement_ids
            .iter()
            .map(|id| {
                store
                    .get(id)
                    .cloned()
                    .map(|record| MeasurementCohortRecord {
                        measurement_id: id.clone(),
                        record,
                    })
                    .ok_or_else(|| {
                        campaign_error(
                            EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                            "campaign acquisition measurement is missing",
                        )
                    })
            })
            .collect::<EvaluationResult<Vec<_>>>()?;
        let cohort = measurement_cohort_from_verified_records(
            records,
            MeasurementCohortRequest {
                corpus_hash: staged.plan.corpus_hash.clone(),
                task_id: staged.plan.task_id.clone(),
                initial_anchor_hash: staged.plan.initial_anchor_hash.clone(),
                validation_policy: staged.plan.validation_policy,
                records_per_artifact: staged.plan.records_per_artifact,
                aggregation_method: staged.plan.aggregation_method,
            },
        )?;
        let cohort_hash = cohort.measurement_cohort_hash.clone();
        staged.cohort = Some(cohort);
        staged.work.cohorts_created = campaign_add(staged.work.cohorts_created, 1)?;
        staged.transition(
            "stage7b_cohort_created",
            AutotuningCampaignStatus::CohortReady,
            Some(cohort_hash),
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }

    /// Computes the exact terminal-only Stage 7B recommendation without hardware work.
    pub fn recommend(
        &mut self,
        base_campaign_hash: &str,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignStatus> {
        self.verify_base(base_campaign_hash, limits)?;
        if self.status != AutotuningCampaignStatus::CohortReady {
            return Err(invalid_transition(self.status, "recommend"));
        }
        let mut staged = self.clone();
        let cohort = staged.cohort.as_ref().ok_or_else(|| {
            campaign_error(
                EvaluationErrorCode::EvaluationMeasurementCohortCorrupt,
                "campaign cohort is missing",
            )
        })?;
        let objective = MeasuredObjectiveDescriptor::new(
            cohort,
            staged.plan.measured_metric,
            staged.plan.indifference_band_ppm,
            staged.plan.search_objective_hash.clone(),
        )?;
        let recommendation = measured_recommendation(&staged.search, cohort, &objective)?;
        let recommendation_hash = recommendation.measured_recommendation_hash.clone();
        staged.measured_objective = Some(objective);
        staged.recommendation = Some(recommendation);
        staged.work.recommendations_computed =
            campaign_add(staged.work.recommendations_computed, 1)?;
        staged.transition(
            "stage7b_recommendation_created",
            AutotuningCampaignStatus::RecommendationReady,
            Some(recommendation_hash),
            limits,
        )?;
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(self.status)
    }

    /// Finalizes a recommendation-ready campaign without publishing an artifact.
    pub fn finalize(
        &mut self,
        base_campaign_hash: &str,
        limits: &AutotuningCampaignLimits,
    ) -> EvaluationResult<AutotuningCampaignResult> {
        self.verify_base(base_campaign_hash, limits)?;
        if self.status == AutotuningCampaignStatus::NoEligibleTerminal {
            return self.result.clone().ok_or_else(|| {
                campaign_error(
                    EvaluationErrorCode::EvaluationAutotuningCampaignReplayMismatch,
                    "no-terminal campaign result is missing",
                )
            });
        }
        if self.status != AutotuningCampaignStatus::RecommendationReady {
            return Err(invalid_transition(self.status, "result"));
        }
        let mut staged = self.clone();
        staged.stopping_reason = Some(AutotuningCampaignStoppingReason::CampaignComplete);
        staged.transition(
            "campaign_finalized_without_live_publication",
            AutotuningCampaignStatus::Complete,
            staged
                .recommendation
                .as_ref()
                .map(|value| value.measured_recommendation_hash.clone()),
            limits,
        )?;
        let result = staged.result_projection(
            AutotuningCampaignStatus::Complete,
            AutotuningCampaignStoppingReason::CampaignComplete,
        )?;
        staged.result = Some(result.clone());
        staged.refresh_hash()?;
        staged.verify_structure(limits)?;
        *self = staged;
        Ok(result)
    }
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
    model.search.work = crate::search::SearchWorkCounters::default();
    if let Some(acquisition) = model.acquisition_session.as_mut() {
        acquisition.work = crate::acquisition::MeasurementAcquisitionWorkCounters::default();
    }
    for journal in &mut model.recovery_journals {
        journal.work = MeasurementAcquisitionRecoveryWorkCounters::default();
        journal.attempt_device_calls.clear();
        for reconciliation in &mut journal.reconciliation_results {
            reconciliation.work = MeasurementAcquisitionRecoveryWorkCounters::default();
        }
    }
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

fn campaign_add(left: u64, right: u64) -> EvaluationResult<u64> {
    left.checked_add(right).ok_or_else(campaign_overflow)
}

fn campaign_count(value: usize) -> EvaluationResult<u64> {
    u64::try_from(value).map_err(|_| campaign_overflow())
}

fn campaign_limit(actual: u64, maximum: u64, resource: &str) -> EvaluationResult<()> {
    if actual > maximum {
        return Err(campaign_error(
            EvaluationErrorCode::EvaluationAutotuningCampaignLimitExceeded,
            format!("autotuning campaign resource `{resource}` exceeded"),
        )
        .expected_actual(json!(maximum), json!(actual)));
    }
    Ok(())
}

fn invalid_transition(status: AutotuningCampaignStatus, operation: &str) -> EvaluationDiagnostic {
    campaign_error(
        EvaluationErrorCode::EvaluationAutotuningCampaignInvalidTransition,
        format!("campaign operation `{operation}` is invalid from state {status:?}"),
    )
}
