//! Typed, transport-independent Stage 6A evaluation records.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::acquisition::{
    MeasurementAcquisitionCheckpoint, MeasurementAcquisitionPlan, MeasurementAcquisitionResult,
    MeasurementAcquisitionSession, MeasurementAcquisitionTrace,
};
use crate::learned::{
    DatasetSplit, InferenceRecord, LearnedModelArtifact, RankingDataset, RankingInput,
    TrainingConfiguration, TrainingRun,
};
use crate::measured::{
    MeasuredObjectiveDescriptor, MeasuredRecommendation, MeasuredSearchRunRecord,
    MeasurementCohort, MeasurementCohortRecord,
};
use crate::ranking::{
    EvaluationChoiceSet, FeatureSchema, RankingPolicyDescriptor, RankingTrace, SelectionOutcome,
};
use crate::recovery::{
    MeasurementAcquisitionPreparedSlot, MeasurementAcquisitionPublicationSnapshot,
    MeasurementAcquisitionReconciliationResult, MeasurementAcquisitionRecoveryCheckpoint,
    MeasurementAcquisitionRecoveryJournal, MeasurementAcquisitionRecoveryWorkCounters,
    MeasurementAcquisitionRetryAuthorization,
};
use crate::repairs::RepairDescriptor;
use crate::search::{
    SearchCheckpoint, SearchEdge, SearchNode, SearchObjectiveDescriptor, SearchPlan,
    SearchRejection, SearchResult, SearchRunRecord, SearchTrace, SearchWorkCounters,
};

/// Stable task identity assigned by the harness.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvaluationTaskId(pub String);

/// Stable policy version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PolicyVersion(pub String);

/// Workload category represented by a corpus task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    /// Stable wire variant.
    SpecIrConstruction,
    /// Stable wire variant.
    HoleRepair,
    /// Stable wire variant.
    ConstraintDischarge,
    /// Stable wire variant.
    ExactCandidateRewrite,
    /// Stable wire variant.
    SpeculativeProposalRepair,
    /// Stable wire variant.
    EqualityExpansionMaterialization,
    /// Stable wire variant.
    FreshMemoryCreation,
    /// Stable wire variant.
    LegalStaticReuse,
    /// Stable wire variant.
    GuardedReuse,
    /// Stable wire variant.
    ScheduleSerialBaseline,
    /// Stable wire variant.
    ExactTilingRemainder,
    /// Stable wire variant.
    LegalFusion,
    /// Stable wire variant.
    VectorizationUnrolling,
    /// Stable wire variant.
    IllegalScheduleRepair,
    /// Stable wire variant.
    BackendLowering,
    /// Stable wire variant.
    UnsupportedBackendRepair,
    /// Stable wire variant.
    DeterministicArtifactEmission,
    /// Stable wire variant.
    EqualityToArtifact,
    /// Stable wire variant.
    ArchiveMigrationReplay,
    /// Stable wire variant.
    EndToEndSaxpy,
}

/// Human-readable objective and stable machine tags.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskObjective {
    /// Stable serialized contract field.
    pub summary: String,
    #[serde(default)]
    /// Stable serialized contract field.
    pub tags: Vec<String>,
}

/// Deterministic initial state and ordered production-protocol actions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskInitialState {
    /// Stable serialized contract field.
    pub source_archive_hash: Option<String>,
    /// Stable serialized contract field.
    pub production_requests: Vec<Value>,
    /// Stable serialized contract field.
    pub runtime_inputs: BTreeMap<String, Value>,
}

/// Compiler-owned completion predicate for an episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskSuccessCriterion {
    /// Stable wire variant.
    AllActionsAccepted,
    /// Stable wire variant.
    JsonPointerEquals {
        /// JSON pointer evaluated against the final compiler response.
        pointer: String,
        /// Exact expected JSON value.
        expected: Value,
    },
    /// Stable wire variant.
    RejectionThenAccepted {
        /// Stable compiler code that must precede an accepted repair.
        rejection_code: String,
    },
}

/// Fixed task budgets; these values participate in `corpus_hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBudget {
    /// Stable serialized contract field.
    pub max_actions: u64,
    /// Stable serialized contract field.
    pub max_rejections: u64,
    /// Stable serialized contract field.
    pub max_context_bytes: u64,
    /// Stable serialized contract field.
    pub max_deterministic_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub max_wall_time_ms: Option<u64>,
}

/// One immutable, versioned evaluation task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationTask {
    /// Stable serialized contract field.
    pub id: EvaluationTaskId,
    /// Stable serialized contract field.
    pub corpus_version: String,
    /// Stable serialized contract field.
    pub category: TaskCategory,
    /// Stable serialized contract field.
    pub size: String,
    /// Stable serialized contract field.
    pub objective: TaskObjective,
    /// Stable serialized contract field.
    pub initial_state: TaskInitialState,
    /// Stable serialized contract field.
    pub success_criterion: TaskSuccessCriterion,
    /// Stable serialized contract field.
    pub allowed_modes: Vec<PolicyKind>,
    /// Stable serialized contract field.
    pub budget: TaskBudget,
    /// Stable serialized contract field.
    pub required_final_state: String,
    /// Stable serialized contract field.
    pub reference_output: Option<Value>,
    /// Stable serialized contract field.
    pub selected_target: Option<String>,
    /// Stable serialized contract field.
    pub metadata: BTreeMap<String, String>,
}

/// Immutable ordered task corpus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationCorpus {
    /// Stable serialized contract field.
    pub name: String,
    /// Stable serialized contract field.
    pub version: String,
    /// Stable serialized contract field.
    pub tasks: Vec<EvaluationTask>,
    /// Stable serialized contract field.
    pub corpus_hash: String,
}

/// Evaluated interaction surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    /// Stable wire variant.
    Free,
    /// Stable wire variant.
    Menu,
    /// Stable wire variant.
    Hybrid,
}

/// Fields and escape rights visible to a policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyCapabilities {
    /// Stable serialized contract field.
    pub observation_fields: Vec<String>,
    /// Stable serialized contract field.
    pub action_surface: String,
    /// Stable serialized contract field.
    pub menu_selection: bool,
    /// Stable serialized contract field.
    pub typed_escape: bool,
}

/// Provenance of a policy implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOrigin {
    /// Stable wire variant.
    Scripted,
    /// Offline deterministic learned policy.
    Learned,
    /// Stable wire variant.
    External,
}

/// Stable descriptor hashed independently from compiler state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDescriptor {
    /// Stable serialized contract field.
    pub kind: PolicyKind,
    /// Stable serialized contract field.
    pub name: String,
    /// Stable serialized contract field.
    pub version: PolicyVersion,
    /// Stable serialized contract field.
    pub configuration: BTreeMap<String, Value>,
    /// Stable serialized contract field.
    pub capabilities: PolicyCapabilities,
    /// Stable serialized contract field.
    pub accounting_method: String,
    /// Stable serialized contract field.
    pub external_model_id: Option<String>,
    /// Stable serialized contract field.
    pub prompt_hash: Option<String>,
    /// Stable serialized contract field.
    pub origin: PolicyOrigin,
    /// Stable serialized contract field.
    pub policy_hash: String,
}

/// Compiler-generated choice in one bounded continuation frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationContinuation {
    /// Stable serialized contract field.
    pub choice_id: String,
    /// Stable serialized contract field.
    pub description: String,
    /// Stable serialized contract field.
    pub action: Value,
}

/// Exact policy-visible state for one step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationObservation {
    /// Stable serialized contract field.
    pub run_id: String,
    /// Stable serialized contract field.
    pub episode_id: String,
    /// Stable serialized contract field.
    pub step_id: String,
    /// Stable serialized contract field.
    pub interaction_mode: PolicyKind,
    /// Stable serialized contract field.
    pub task_id: EvaluationTaskId,
    /// Stable serialized contract field.
    pub task_objective: TaskObjective,
    /// Stable serialized contract field.
    pub visible_workspace_summary: Value,
    /// Stable serialized contract field.
    pub selected_revisions_and_hashes: BTreeMap<String, String>,
    /// Stable serialized contract field.
    pub open_obligations: Vec<Value>,
    /// Stable serialized contract field.
    pub diagnostics: Vec<Value>,
    /// Stable serialized contract field.
    pub continuation_frame: Vec<EvaluationContinuation>,
    /// Exact Stage 6B choice-set identity when ranking is requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_set_hash: Option<String>,
    /// Exact Stage 6B visible feature-schema identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_schema_hash: Option<String>,
    /// Stable serialized contract field.
    pub remaining_budget: TaskBudget,
    /// Stable serialized contract field.
    pub previous_compiler_outcome: Option<Value>,
    /// Stable serialized contract field.
    pub allowed_action_schema: Value,
    /// Stable serialized contract field.
    pub context_bytes: u64,
    /// Stable serialized contract field.
    pub deterministic_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub observation_hash: String,
}

/// Trust level for externally supplied token counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTrust {
    /// Stable wire variant.
    Deterministic,
    /// Stable wire variant.
    ProviderReported,
    /// Stable wire variant.
    AgentSelfReported,
}

/// Optional token report. Missing counts remain `None`, never zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Stable serialized contract field.
    pub input_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub output_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub trust: UsageTrust,
    /// Stable serialized contract field.
    pub tokenizer: Option<String>,
}

/// Byte/token context accounting for one step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMeasurement {
    /// Stable serialized contract field.
    pub observation_bytes: u64,
    /// Stable serialized contract field.
    pub decision_bytes: u64,
    /// Stable serialized contract field.
    pub cumulative_context_bytes: u64,
    /// Stable serialized contract field.
    pub deterministic_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub reported_usage: Option<TokenUsage>,
}

/// Agent decision accepted by the evaluation protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Stable wire variant.
    MenuChoice {
        /// Compiler-generated choice identity from the observation.
        choice_id: String,
    },
    /// Stable wire variant.
    Action {
        /// Typed production protocol request.
        action: Value,
        /// Whether this is a bounded hybrid escape.
        escape: bool,
    },
    /// Stable wire variant.
    Finish,
}

/// Stable compiler-owned rejection classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionClassification {
    /// Stable wire variant.
    MalformedRequest,
    /// Stable wire variant.
    StaleBaseOrHash,
    /// Stable wire variant.
    InvalidReference,
    /// Stable wire variant.
    TypeError,
    /// Stable wire variant.
    OpenObligation,
    /// Stable wire variant.
    UnsupportedAction,
    /// Stable wire variant.
    ResourceLimit,
    /// Stable wire variant.
    UnsupportedBackendLowering,
    /// Stable wire variant.
    DeviceUnavailable,
    /// Stable wire variant.
    PolicyViolation,
    /// Stable wire variant.
    EvaluationBudgetExceeded,
    /// Stable wire variant.
    OtherCompilerRejection,
}

/// Outcome produced by the production compiler path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompilerOutcome {
    /// Stable serialized contract field.
    pub accepted: bool,
    /// Stable serialized contract field.
    pub progress_producing: bool,
    /// Stable serialized contract field.
    pub response: Value,
    /// Stable serialized contract field.
    pub rejection: Option<RejectionClassification>,
    /// Stable serialized contract field.
    pub compiler_error_code: Option<String>,
}

/// One closed repair interval after a rejected decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairCycle {
    /// Stable serialized contract field.
    pub start_step: u64,
    /// Stable serialized contract field.
    pub end_step: u64,
    /// Stable serialized contract field.
    pub length: u64,
    /// Stable serialized contract field.
    pub repeated_identical_rejection: bool,
}

/// One observation/decision/compiler-outcome triplet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeStep {
    /// Stable serialized contract field.
    pub ordinal: u64,
    /// Stable serialized contract field.
    pub observation: EvaluationObservation,
    /// Stable serialized contract field.
    pub decision: PolicyDecision,
    /// Stable serialized contract field.
    pub outcome: CompilerOutcome,
    /// Stable serialized contract field.
    pub context: ContextMeasurement,
    /// Stable serialized contract field.
    pub external_request_correlation_id: Option<String>,
    /// Optional Stage 6B policy-owned ranking trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_trace: Option<RankingTrace>,
    /// Optional Stage 6B explicit compiler selection outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionOutcome>,
}

/// Episode lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeStatus {
    /// Stable wire variant.
    Ready,
    /// Stable wire variant.
    Running,
    /// Stable wire variant.
    Succeeded,
    /// Stable wire variant.
    Failed,
    /// Stable wire variant.
    BudgetExhausted,
    /// Stable wire variant.
    Cancelled,
}

/// Compiler-derived semantic completion result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticResult {
    /// Stable serialized contract field.
    pub criterion_satisfied: bool,
    /// Stable serialized contract field.
    pub checked_by: String,
    /// Stable serialized contract field.
    pub final_response: Option<Value>,
}

/// Optional same-device, confidence-only performance observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceObservation {
    /// Stable serialized contract field.
    pub artifact_hash: String,
    /// Stable serialized contract field.
    pub measurement_hash: String,
    /// Stable serialized contract field.
    pub device_fingerprint_hash: String,
    /// Stable serialized contract field.
    pub tensor_dimensions: BTreeMap<String, u64>,
    /// Stable serialized contract field.
    pub guard_outcomes: BTreeMap<String, u64>,
    /// Stable serialized contract field.
    pub min_ns: u64,
    /// Stable serialized contract field.
    pub median_ns: u64,
    /// Stable serialized contract field.
    pub p95_ns: u64,
    /// Stable serialized contract field.
    pub max_ns: u64,
    /// Stable serialized contract field.
    pub validation_status: String,
}

/// Final metrics and semantic result for one episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeResult {
    /// Stable serialized contract field.
    pub success: bool,
    /// Stable serialized contract field.
    pub semantic: SemanticResult,
    /// Stable serialized contract field.
    pub total_decisions: u64,
    /// Stable serialized contract field.
    pub accepted_decisions: u64,
    /// Stable serialized contract field.
    pub rejected_decisions: u64,
    /// Stable serialized contract field.
    pub no_op_decisions: u64,
    /// Stable serialized contract field.
    pub repair_cycles: Vec<RepairCycle>,
    /// Stable serialized contract field.
    pub context_bytes: u64,
    /// Stable serialized contract field.
    pub deterministic_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub provider_input_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub provider_output_tokens: Option<u64>,
    /// Stable serialized contract field.
    pub budget_exhausted: bool,
    /// Stable serialized contract field.
    pub performance: Option<PerformanceObservation>,
}

/// One replayable episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationEpisode {
    /// Stable serialized contract field.
    pub id: String,
    /// Stable serialized contract field.
    pub run_id: String,
    /// Stable serialized contract field.
    pub task_id: EvaluationTaskId,
    /// Stable serialized contract field.
    pub policy_hash: String,
    /// Stable serialized contract field.
    pub deterministic_seed: u64,
    /// Stable serialized contract field.
    pub status: EpisodeStatus,
    /// Stable serialized contract field.
    pub steps: Vec<EpisodeStep>,
    /// Stable serialized contract field.
    pub result: Option<EpisodeResult>,
    /// Stable serialized contract field.
    pub episode_hash: Option<String>,
}

/// Ordered collection of episodes evaluated under one policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRun {
    /// Stable serialized contract field.
    pub id: String,
    /// Stable serialized contract field.
    pub corpus_hash: String,
    /// Stable serialized contract field.
    pub policy: PolicyDescriptor,
    /// Stable serialized contract field.
    pub compiler_build_hash: String,
    /// Stable serialized contract field.
    pub seeds: Vec<u64>,
    /// Stable serialized contract field.
    pub episodes: Vec<EvaluationEpisode>,
    /// Stable serialized contract field.
    pub cancelled: bool,
    /// Stable serialized contract field.
    pub evaluation_hash: Option<String>,
}

/// Statistical aggregate with raw, non-weighted metrics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationAggregate {
    /// Stable serialized contract field.
    pub run_id: String,
    /// Stable serialized contract field.
    pub count: u64,
    /// Stable serialized contract field.
    pub successes: u64,
    /// Stable serialized contract field.
    pub success_rate: f64,
    /// Stable serialized contract field.
    pub median_decisions: u64,
    /// Stable serialized contract field.
    pub p95_decisions: u64,
    /// Stable serialized contract field.
    pub median_rejections: u64,
    /// Stable serialized contract field.
    pub p95_rejections: u64,
    /// Stable serialized contract field.
    pub median_repair_cycles: u64,
    /// Stable serialized contract field.
    pub p95_repair_cycles: u64,
    /// Stable serialized contract field.
    pub median_context_bytes: u64,
    /// Stable serialized contract field.
    pub p95_context_bytes: u64,
    /// Stable serialized contract field.
    pub accepted_actions_per_token: Option<f64>,
    /// Stable serialized contract field.
    pub accepted_actions_per_decision: Option<f64>,
    /// Stable serialized contract field.
    pub budget_exhaustion_rate: f64,
    /// Stable serialized contract field.
    pub semantic_correctness_rate: f64,
    /// Stable serialized contract field.
    pub aggregate_hash: String,
}

/// Fair comparison of compatible aggregates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationComparison {
    /// Stable serialized contract field.
    pub run_ids: Vec<String>,
    /// Stable serialized contract field.
    pub corpus_hash: String,
    /// Stable serialized contract field.
    pub compiler_build_hash: String,
    /// Stable serialized contract field.
    pub aggregates: Vec<EvaluationAggregate>,
}

/// Reproducibility metadata for a separate evaluation artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationManifest {
    /// Stable serialized contract field.
    pub format: String,
    /// Stable serialized contract field.
    pub version: u32,
    /// Stable serialized contract field.
    pub corpus_version: String,
    /// Stable serialized contract field.
    pub corpus_hash: String,
    /// Stable serialized contract field.
    pub compiler_build_hash: String,
    /// Stable serialized contract field.
    pub source_workspace_hashes: Vec<String>,
    /// Stable serialized contract field.
    pub aggregation_configuration: BTreeMap<String, Value>,
}

/// Separate current evaluation archive v7; never embedded in workspace archive v9.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationArchive {
    /// Stable serialized contract field.
    pub manifest: EvaluationManifest,
    /// Stable serialized contract field.
    pub corpus: EvaluationCorpus,
    /// Stable serialized contract field.
    pub runs: Vec<EvaluationRun>,
    /// Stable serialized contract field.
    pub aggregates: Vec<EvaluationAggregate>,
    /// Stage 6B visible schemas. Empty and omitted for archive v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_schemas: Vec<FeatureSchema>,
    /// Stage 6B ranking descriptors. Empty and omitted for archive v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranking_policies: Vec<RankingPolicyDescriptor>,
    /// Stage 6B exact choice sets. Empty and omitted for archive v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choice_sets: Vec<EvaluationChoiceSet>,
    /// Explicit v1-migration status for every episode in archive v2.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ranking_statuses: BTreeMap<String, RankingEpisodeStatus>,
    /// Stage 6C immutable learned-ranking datasets. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranking_datasets: Vec<RankingDataset>,
    /// Stage 6C stable group splits. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dataset_splits: Vec<DatasetSplit>,
    /// Stage 6C deterministic training configurations. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub training_configurations: Vec<TrainingConfiguration>,
    /// Stage 6C deterministic training provenance. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub training_runs: Vec<TrainingRun>,
    /// Stage 6C fixed-point model artifacts. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub learned_models: Vec<LearnedModelArtifact>,
    /// Stage 6C exact policy-visible inputs needed for inference replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranking_inputs: Vec<RankingInput>,
    /// Stage 6C exact retained inference records. Empty in v1/v2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inference_records: Vec<InferenceRecord>,
    /// Explicit learned/unlearned classification for every v3 episode.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub learning_statuses: BTreeMap<String, LearningEpisodeStatus>,
    /// Explicit v4 classification of search-history presence.
    #[serde(default, skip_serializing_if = "SearchHistoryStatus::is_unspecified")]
    pub search_history_status: SearchHistoryStatus,
    /// Stage 7A exact objective descriptors. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_objectives: Vec<SearchObjectiveDescriptor>,
    /// Stage 7A deterministic search plans. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_plans: Vec<SearchPlan>,
    /// Stage 7A search-run summaries. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_runs: Vec<SearchRunRecord>,
    /// Stage 7A search nodes. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_nodes: Vec<SearchNode>,
    /// Stage 7A search edges. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_edges: Vec<SearchEdge>,
    /// Stage 7A resumable checkpoints. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_checkpoints: Vec<SearchCheckpoint>,
    /// Stage 7A replayable semantic traces. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_traces: Vec<SearchTrace>,
    /// Stage 7A non-authoritative results. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_results: Vec<SearchResult>,
    /// Stage 7A structured search rejections. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_rejections: Vec<SearchRejection>,
    /// Stage 7A deterministic non-correctness work counters. Empty in v1/v2/v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_work_counters: Vec<SearchWorkCounters>,
    /// Explicit v5 classification of measured-search-history presence.
    #[serde(
        default,
        skip_serializing_if = "MeasuredSearchHistoryStatus::is_unspecified"
    )]
    pub measured_search_history_status: MeasuredSearchHistoryStatus,
    /// Stage 7B frozen verified measurement cohorts. Empty in v1–v4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_cohorts: Vec<MeasurementCohort>,
    /// Stage 7B hardware objective descriptors. Empty in v1–v4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measured_objectives: Vec<MeasuredObjectiveDescriptor>,
    /// Stage 7B measured-search run anchors. Empty in v1–v4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measured_search_runs: Vec<MeasuredSearchRunRecord>,
    /// Stage 7B non-authoritative measured recommendations. Empty in v1–v4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measured_recommendations: Vec<MeasuredRecommendation>,
    /// Explicit v6 classification of acquisition-history presence.
    #[serde(
        default,
        skip_serializing_if = "MeasurementAcquisitionHistoryStatus::is_unspecified"
    )]
    pub measurement_acquisition_history_status: MeasurementAcquisitionHistoryStatus,
    /// Stage 7C immutable acquisition plans. Empty in v1-v5.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_plans: Vec<MeasurementAcquisitionPlan>,
    /// Stage 7C terminal or resumable sessions. Empty in v1-v5.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_sessions: Vec<MeasurementAcquisitionSession>,
    /// Stage 7C slot-boundary checkpoints. Empty in v1-v5.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_checkpoints: Vec<MeasurementAcquisitionCheckpoint>,
    /// Stage 7C semantic traces. Empty in v1-v5.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_traces: Vec<MeasurementAcquisitionTrace>,
    /// Stage 7C terminal results. Empty in v1-v5.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_results: Vec<MeasurementAcquisitionResult>,
    /// Complete production-format records referenced by Stage 7C results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_records: Vec<MeasurementCohortRecord>,
    /// Explicit result-to-Stage-7B-cohort handoff links.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurement_acquisition_cohort_links: BTreeMap<String, String>,
    /// Exact zero-device replay status by acquisition result hash.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurement_acquisition_replay_statuses: BTreeMap<String, bool>,
    /// Explicit v7 classification of durable-recovery-history presence.
    #[serde(
        default,
        skip_serializing_if = "MeasurementAcquisitionRecoveryHistoryStatus::is_no_history"
    )]
    pub measurement_acquisition_recovery_history_status:
        MeasurementAcquisitionRecoveryHistoryStatus,
    /// Stage 7D durable recovery journals. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_recovery_journals: Vec<MeasurementAcquisitionRecoveryJournal>,
    /// Flattened Stage 7D prepared-slot records. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_prepared_slots: Vec<MeasurementAcquisitionPreparedSlot>,
    /// Flattened server-owned publication snapshots. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_publication_snapshots:
        Vec<MeasurementAcquisitionPublicationSnapshot>,
    /// Flattened Stage 7D reconciliation results. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_reconciliation_results:
        Vec<MeasurementAcquisitionReconciliationResult>,
    /// Explicit retry authorizations. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_retry_authorizations: Vec<MeasurementAcquisitionRetryAuthorization>,
    /// Durable recovery checkpoints. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_recovery_checkpoints: Vec<MeasurementAcquisitionRecoveryCheckpoint>,
    /// Flattened deterministic recovery work counters. Empty in v1-v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_acquisition_recovery_work_counters:
        Vec<MeasurementAcquisitionRecoveryWorkCounters>,
    /// Exact zero-device replay status by recovery journal hash.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurement_acquisition_recovery_replay_statuses: BTreeMap<String, bool>,
    /// Stable serialized contract field.
    pub archive_hash: String,
}

/// Durable recovery presence recorded explicitly by evaluation archive v7.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionRecoveryHistoryStatus {
    /// Pure migration retained no synthetic recovery history.
    #[default]
    NoRecoveryHistory,
    /// One or more verified recovery journals are retained.
    RecoveryHistoryPresent,
}

impl MeasurementAcquisitionRecoveryHistoryStatus {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_no_history(&self) -> bool {
        matches!(self, Self::NoRecoveryHistory)
    }
}

/// Measurement-acquisition presence recorded explicitly by evaluation archive v6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementAcquisitionHistoryStatus {
    /// Legacy decoding default; forbidden in a verified v6 archive.
    #[default]
    Unspecified,
    /// Pure migration retained no synthetic acquisition history.
    NoAcquisitionHistory,
    /// One or more verified acquisition results are retained.
    AcquisitionHistoryPresent,
}

impl MeasurementAcquisitionHistoryStatus {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// Measured-search presence recorded explicitly by evaluation archive v5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredSearchHistoryStatus {
    /// Legacy decoding default; forbidden in a verified v5 archive.
    #[default]
    Unspecified,
    /// Pure migration retained no synthetic measured-search history.
    NoMeasuredSearchHistory,
    /// One or more verified measured recommendations are retained.
    MeasuredSearchHistoryPresent,
}

impl MeasuredSearchHistoryStatus {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// Search-history presence recorded explicitly by evaluation archive v4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchHistoryStatus {
    /// Legacy decoding default; forbidden in a verified v4 archive.
    #[default]
    Unspecified,
    /// Pure migration retained no synthetic search history.
    NoSearchHistory,
    /// One or more exact Stage 7A runs are retained.
    SearchHistoryPresent,
}

impl SearchHistoryStatus {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// Ranking presence recorded explicitly by evaluation archive v2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingEpisodeStatus {
    /// Legacy or ordinary Stage 6A episode with no invented ranking records.
    Unranked,
    /// Episode containing validated ranking and selection records.
    Ranked,
}

/// Learned-ranking presence recorded explicitly by evaluation archive v3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningEpisodeStatus {
    /// Legacy, scripted, external, or otherwise unlearned episode.
    Unlearned,
    /// Episode with an exact retained learned inference record.
    Learned,
}

/// Stable evaluation-layer diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluationErrorCode {
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryNotPrepared,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryJournalCorrupt,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryAlreadyResolved,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryRetryNotAuthorized,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryAmbiguous,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryIncompatiblePublication,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoverySnapshotCorrupt,
    /// Stable Stage 7D wire variant.
    EvaluationAcquisitionRecoveryLimitExceeded,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionUnsupportedMode,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionArtifactSetInvalid,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionArtifactInvalid,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionMixedSpec,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionMixedTarget,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionDeviceUnavailable,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionDeviceChanged,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionCompilerBuildChanged,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionRuntimeChanged,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionBenchmarkConfigInvalid,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionPlanInvalid,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionPlanCorrupt,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionCheckpointCorrupt,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionCheckpointStale,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionTraceCorrupt,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionResultCorrupt,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionMeasurementMissing,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionMeasurementDuplicate,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionSlotOrderMismatch,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionUnequalRecords,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionBenchmarkFailed,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionIndeterminateAfterCrash,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionClientDataForbidden,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionLimitExceeded,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionCounterOverflow,
    /// Stable Stage 7C wire variant.
    EvaluationAcquisitionReplayHardwareWork,
    /// Stable wire variant.
    EvaluationCorpusNotFound,
    /// Stable wire variant.
    EvaluationTaskNotFound,
    /// Stable wire variant.
    EvaluationPolicyNotFound,
    /// Stable wire variant.
    EvaluationRunNotFound,
    /// Stable wire variant.
    EvaluationEpisodeNotFound,
    /// Stable wire variant.
    EvaluationStepNotFound,
    /// Stable wire variant.
    EvaluationObservationMismatch,
    /// Stable wire variant.
    EvaluationPolicyViolation,
    /// Stable wire variant.
    EvaluationMenuChoiceInvalid,
    /// Stable wire variant.
    EvaluationBudgetExceeded,
    /// Stable wire variant.
    EvaluationAlreadyComplete,
    /// Stable wire variant.
    EvaluationReplayMismatch,
    /// Stable wire variant.
    EvaluationTranscriptInvalid,
    /// Stable wire variant.
    EvaluationMetricInvalid,
    /// Stable wire variant.
    EvaluationComparisonInvalid,
    /// Stable wire variant.
    EvaluationHashMismatch,
    /// Stable wire variant.
    EvaluationArchiveInvalid,
    /// Stable wire variant.
    EvaluationChoiceSetNotFound,
    /// Stable wire variant.
    EvaluationChoiceNotFound,
    /// Stable wire variant.
    EvaluationChoiceSetMismatch,
    /// Stable wire variant.
    EvaluationFeatureSchemaNotFound,
    /// Stable wire variant.
    EvaluationFeatureSchemaMismatch,
    /// Stable wire variant.
    EvaluationRankingPolicyNotFound,
    /// Stable wire variant.
    EvaluationRankingPolicyInvalid,
    /// Stable wire variant.
    EvaluationRankingTraceInvalid,
    /// Stable wire variant.
    EvaluationScoreInvalid,
    /// Stable wire variant.
    EvaluationScoreCountMismatch,
    /// Stable wire variant.
    EvaluationTieBreakInvalid,
    /// Stable wire variant.
    EvaluationSelectionInvalid,
    /// Stable wire variant.
    EvaluationSelectionNotInChoiceSet,
    /// Stable wire variant.
    EvaluationRankingReplayMismatch,
    /// Stable wire variant.
    EvaluationRankingComparisonInvalid,
    /// Stable wire variant.
    EvaluationRankingBudgetExceeded,
    /// Stable wire variant.
    EvaluationArchiveMigrationInvalid,
    /// Stable wire variant.
    EvaluationContinuationCorrupt,
    /// Stable wire variant.
    EvaluationContinuationStale,
    /// Stable wire variant.
    EvaluationContinuationLimitExceeded,
    /// Stable wire variant.
    EvaluationTypedRepairInvalid,
    /// Stable wire variant.
    EvaluationTypedRepairStale,
    /// Stable wire variant.
    EvaluationWorkUnitOverflow,
    /// Stable wire variant.
    EvaluationWorkUnitLimitExceeded,
    /// Stable wire variant.
    EvaluationDatasetInvalid,
    /// Stable wire variant.
    EvaluationDatasetLeakage,
    /// Stable wire variant.
    EvaluationTrainingInvalid,
    /// Stable wire variant.
    EvaluationTrainingOverflow,
    /// Stable wire variant.
    EvaluationModelInvalid,
    /// Stable wire variant.
    EvaluationModelIncompatible,
    /// Stable wire variant.
    EvaluationInferenceInvalid,
    /// Stable wire variant.
    EvaluationEventOrderInvalid,
    /// Stable wire variant.
    EvaluationSearchObjectiveInvalid,
    /// Stable wire variant.
    EvaluationSearchPlanInvalid,
    /// Stable wire variant.
    EvaluationSearchRootStale,
    /// Stable wire variant.
    EvaluationSearchCheckpointStale,
    /// Stable wire variant.
    EvaluationSearchCheckpointCorrupt,
    /// Stable wire variant.
    EvaluationSearchFrontierCorrupt,
    /// Stable wire variant.
    EvaluationSearchUnsupportedSurface,
    /// Stable wire variant.
    EvaluationSearchLimitExceeded,
    /// Stable wire variant.
    EvaluationSearchReplayMismatch,
    /// Stable wire variant.
    EvaluationSearchIncomplete,
    /// Stable wire variant.
    EvaluationMeasuredMetricUnsupported,
    /// Stable wire variant.
    EvaluationMeasurementMissing,
    /// Stable wire variant.
    EvaluationMeasurementNotFound,
    /// Stable wire variant.
    EvaluationMeasurementDuplicate,
    /// Stable wire variant.
    EvaluationMeasurementMixedDevice,
    /// Stable wire variant.
    EvaluationMeasurementMixedTarget,
    /// Stable wire variant.
    EvaluationMeasurementMixedBuild,
    /// Stable wire variant.
    EvaluationMeasurementMixedRuntime,
    /// Stable wire variant.
    EvaluationMeasurementMixedConfig,
    /// Stable wire variant.
    EvaluationMeasurementMixedInput,
    /// Stable wire variant.
    EvaluationMeasurementValidationInvalid,
    /// Stable wire variant.
    EvaluationUnequalMeasurementRecords,
    /// Stable wire variant.
    EvaluationMeasuredTerminalUnavailable,
    /// Stable wire variant.
    EvaluationMeasurementCorrupt,
    /// Stable wire variant.
    EvaluationMeasurementCohortCorrupt,
    /// Stable wire variant.
    EvaluationMeasuredObjectiveInvalid,
    /// Stable wire variant.
    EvaluationMeasuredObjectiveCorrupt,
    /// Stable wire variant.
    EvaluationMeasuredRecommendationCorrupt,
    /// Stable wire variant.
    EvaluationMeasuredAnchorStale,
    /// Stable wire variant.
    EvaluationClientMeasurementDataForbidden,
    /// Stable wire variant.
    EvaluationMeasuredArithmeticOverflow,
    /// Stable wire variant.
    TokenAccountingUnavailable,
}

/// Structured deterministic evaluation failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationDiagnostic {
    /// Stable serialized contract field.
    pub code: EvaluationErrorCode,
    /// Stable serialized contract field.
    pub message: String,
    /// Stable serialized contract field.
    pub expected: Option<Value>,
    /// Stable serialized contract field.
    pub actual: Option<Value>,
    /// Stable serialized contract field.
    pub details: BTreeMap<String, Value>,
    /// Stable serialized contract field.
    pub repairs: Vec<String>,
    /// Bounded compiler-owned typed repair descriptors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed_repairs: Vec<RepairDescriptor>,
}

impl std::fmt::Display for EvaluationDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl EvaluationDiagnostic {
    pub(crate) fn new(code: EvaluationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            expected: None,
            actual: None,
            details: BTreeMap::new(),
            repairs: Vec::new(),
            typed_repairs: Vec::new(),
        }
    }

    pub(crate) fn expected_actual(mut self, expected: Value, actual: Value) -> Self {
        self.expected = Some(expected);
        self.actual = Some(actual);
        self
    }

    pub(crate) fn repair(mut self, repair: impl Into<String>) -> Self {
        self.repairs.push(repair.into());
        self
    }

    /// Attaches a compiler-owned typed repair descriptor.
    #[must_use]
    pub fn typed_repair(mut self, repair: RepairDescriptor) -> Self {
        self.typed_repairs.push(repair);
        self
    }
}

/// Evaluation result alias.
pub type EvaluationResult<T> = Result<T, EvaluationDiagnostic>;
