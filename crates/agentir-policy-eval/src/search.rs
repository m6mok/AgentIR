//! Reproducible bounded offline search over production-generated choice sets.
//!
//! Search is evaluation-only. It reconstructs every branch in an isolated
//! [`EvaluationHarness`], reruns the Stage 6B/6C ranker, and submits actions
//! through the existing production verifier. Search records never advance a
//! compiler proof frontier and are never published as compiler state.

use crate::{
    continuation::FrameCompleteness,
    engine::{EvaluationHarness, EvaluationLimits, ranked_policy},
    hashing::{domain_hash, domain_hash_cleared},
    learned::{InferenceRecord, LearnedModelArtifact, LearnedRankingLimits, infer, ranking_input},
    model::{
        CompilerOutcome, EvaluationCorpus, EvaluationDiagnostic, EvaluationErrorCode,
        EvaluationResult, EvaluationTask, EvaluationTaskId, PolicyDecision, PolicyKind,
        PolicyOrigin, RejectionClassification, TaskCategory,
    },
    ranking::{
        EvaluationChoiceId, EvaluationChoiceSet, FeatureSchema, FixedScore, RankedChoice,
        RankingDecision, RankingLimits, RankingPolicyDescriptor, RankingPolicyKind, RankingTrace,
        TieBreakRule, rank_choices, scripted_ranking_decision,
    },
};
use agentir_core::backend::compiler_build_hash;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Search-objective hash domain.
pub const SEARCH_OBJECTIVE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_objective.v1\0";
/// Search-plan hash domain.
pub const SEARCH_PLAN_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_plan.v1\0";
/// Search-node hash domain.
pub const SEARCH_NODE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_node.v1\0";
/// Search-edge hash domain.
pub const SEARCH_EDGE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_edge.v1\0";
/// Search-checkpoint hash domain.
pub const SEARCH_CHECKPOINT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_checkpoint.v1\0";
/// Search-trace hash domain.
pub const SEARCH_TRACE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_trace.v1\0";
/// Search-result hash domain.
pub const SEARCH_RESULT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_result.v1\0";
/// Search-local compiler-state observation hash domain.
pub const SEARCH_STATE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_state.v1\0";
/// Search root-anchor hash domain.
pub const SEARCH_ROOT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_root.v1\0";
/// Search repair hash domain.
pub const SEARCH_REPAIR_HASH_DOMAIN: &[u8] = b"agentir.evaluation.search_repair.v1\0";

const MAX_BEAM_WIDTH: u64 = 64;
const MAX_SEMANTIC_DEPTH: u64 = 256;
const MAX_CHILDREN_PER_NODE: u64 = 1_000;

/// Direction of one ordered structural objective component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveDirection {
    /// Larger checked integer values are preferred.
    Maximize,
    /// Smaller checked integer values are preferred.
    Minimize,
}

/// Offline structural metric permitted in Stage 7A objectives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchObjectiveComponentKind {
    /// Whether the production episode completed successfully.
    CompilerTerminalSuccess,
    /// Whether the evaluation task criterion was satisfied.
    TaskCriterionSuccess,
    /// Count of accepted progress-producing production actions.
    AcceptedProgressCount,
    /// Policy-visible open obligation count.
    OpenObligationCount,
    /// Count of compiler-rejected actions on the trajectory.
    RejectionCount,
    /// Count of closed typed repair cycles on the trajectory.
    RepairCount,
    /// Deterministic semantic expansion units on the trajectory.
    DeterministicWorkUnits,
    /// Sum of fixed-point Stage 6B/6C ranking scores.
    AccumulatedRankingScore,
    /// Number of selected production actions.
    TrajectoryLength,
}

/// One ordered component of a lexicographic objective.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchObjectiveComponent {
    /// Structural metric.
    pub kind: SearchObjectiveComponentKind,
    /// Comparison direction.
    pub direction: ObjectiveDirection,
}

/// Terminal eligibility contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalEligibilityRule {
    /// Only a production-completed evaluation episode is terminal eligible.
    ProductionEpisodeComplete,
}

/// Treatment of the evaluation task-success predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSuccessTreatment {
    /// Success is retained as a distinct objective component when requested.
    StructuralComponentOnly,
}

/// Treatment of rejected production actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionTreatment {
    /// Record the edge and leaf but never advance the rejected branch.
    RecordTerminalLeaf,
}

/// Treatment of a search that ends without exhausting its envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteSearchTreatment {
    /// Publish only a bounded-frontier recommendation, never a complete result.
    BoundedFrontierRecommendation,
}

/// Versioned, independently hashed Stage 7A objective descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchObjectiveDescriptor {
    /// Objective schema version. Stage 7A uses one.
    pub version: u32,
    /// Exact corpus anchor.
    pub corpus_hash: String,
    /// Exact task identity.
    pub task_id: EvaluationTaskId,
    /// Exact immutable task-definition hash.
    pub task_hash: String,
    /// Exact initial observation/workspace anchor.
    pub initial_anchor_hash: String,
    /// Ordered lexicographic integer components.
    pub components: Vec<SearchObjectiveComponent>,
    /// Explicit comparison rule identifier.
    pub comparison_rule: String,
    /// Terminal eligibility.
    pub terminal_eligibility: TerminalEligibilityRule,
    /// Task-success handling.
    pub task_success_treatment: TaskSuccessTreatment,
    /// Compiler-rejection handling.
    pub rejection_treatment: RejectionTreatment,
    /// Incomplete-search handling.
    pub incomplete_search_treatment: IncompleteSearchTreatment,
    /// Operational display label excluded from objective identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operational_label: Option<String>,
    /// Independent objective hash.
    pub search_objective_hash: String,
}

impl SearchObjectiveDescriptor {
    /// Constructs and validates a v1 structural objective for one exact task.
    pub fn new(
        corpus: &EvaluationCorpus,
        task: &EvaluationTask,
        components: Vec<SearchObjectiveComponent>,
    ) -> EvaluationResult<Self> {
        validate_objective_components(&components)?;
        let task_hash = domain_hash(SEARCH_ROOT_HASH_DOMAIN, task)?;
        let initial_anchor_hash = domain_hash(
            SEARCH_ROOT_HASH_DOMAIN,
            &(
                &corpus.corpus_hash,
                &task.id,
                &task_hash,
                &task.initial_state,
                compiler_build_hash(),
            ),
        )?;
        let mut objective = Self {
            version: 1,
            corpus_hash: corpus.corpus_hash.clone(),
            task_id: task.id.clone(),
            task_hash,
            initial_anchor_hash,
            components,
            comparison_rule: "ordered_lexicographic_checked_i64_v1".to_owned(),
            terminal_eligibility: TerminalEligibilityRule::ProductionEpisodeComplete,
            task_success_treatment: TaskSuccessTreatment::StructuralComponentOnly,
            rejection_treatment: RejectionTreatment::RecordTerminalLeaf,
            incomplete_search_treatment: IncompleteSearchTreatment::BoundedFrontierRecommendation,
            operational_label: None,
            search_objective_hash: String::new(),
        };
        objective.search_objective_hash = search_objective_hash(&objective)?;
        Ok(objective)
    }

    /// Verifies the complete retained descriptor and exact current anchors.
    pub fn validate(
        &self,
        corpus: &EvaluationCorpus,
        task: &EvaluationTask,
    ) -> EvaluationResult<()> {
        let expected = Self::new(corpus, task, self.components.clone())?;
        if self.version != 1
            || self.comparison_rule != "ordered_lexicographic_checked_i64_v1"
            || self.corpus_hash != expected.corpus_hash
            || self.task_id != expected.task_id
            || self.task_hash != expected.task_hash
            || self.initial_anchor_hash != expected.initial_anchor_hash
            || self.search_objective_hash != search_objective_hash(self)?
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchObjectiveInvalid,
                "search objective is corrupt or incompatible with the exact root",
            ));
        }
        validate_objective_components(&self.components)
    }
}

fn validate_objective_components(components: &[SearchObjectiveComponent]) -> EvaluationResult<()> {
    if components.is_empty() {
        return Err(search_error(
            EvaluationErrorCode::EvaluationSearchObjectiveInvalid,
            "search objective requires at least one interpretable component",
        ));
    }
    let mut seen = BTreeSet::new();
    for component in components {
        if !seen.insert(component.kind) {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchObjectiveInvalid,
                "search objective contains a duplicate component",
            ));
        }
        let required = match component.kind {
            SearchObjectiveComponentKind::CompilerTerminalSuccess
            | SearchObjectiveComponentKind::TaskCriterionSuccess
            | SearchObjectiveComponentKind::AcceptedProgressCount
            | SearchObjectiveComponentKind::AccumulatedRankingScore => ObjectiveDirection::Maximize,
            SearchObjectiveComponentKind::OpenObligationCount
            | SearchObjectiveComponentKind::RejectionCount
            | SearchObjectiveComponentKind::RepairCount
            | SearchObjectiveComponentKind::DeterministicWorkUnits
            | SearchObjectiveComponentKind::TrajectoryLength => ObjectiveDirection::Minimize,
        };
        if component.direction != required {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchObjectiveInvalid,
                "search objective component has an incompatible direction",
            ));
        }
    }
    Ok(())
}

/// Supported Stage 7A search algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchAlgorithmKind {
    /// Deterministic level-synchronous bounded beam search.
    DeterministicBeam,
}

/// Explicit duplicate-state policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateStatePolicy {
    /// Preserve every parent edge and mark later nodes with their representative.
    PreserveProvenanceCanonicalRepresentative,
}

/// Deterministic expansion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionOrder {
    /// Expand a depth layer in total frontier order.
    LevelSynchronousFrontierOrder,
}

/// Deterministic frontier tie rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTieRule {
    /// Objective vector, ranking score, state hash, then search-local node ID.
    ObjectiveRankingStateNode,
}

/// Search repair handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRepairPolicy {
    /// Retain compiler repair accounting but generate no search action.
    RecordOnly,
}

/// Search terminal policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTerminalPolicy {
    /// Explore the bounded envelope and recommend the highest ordered observed terminal.
    HighestObservedEligible,
}

/// Versioned deterministic search-plan contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPlan {
    /// Plan schema version.
    pub version: u32,
    /// Algorithm kind.
    pub algorithm: SearchAlgorithmKind,
    /// Algorithm semantic version.
    pub algorithm_version: String,
    /// Supported interaction surface.
    pub interaction_surface: PolicyKind,
    /// Exact ranking policy anchor.
    pub ranking_policy_hash: String,
    /// Optional exact learned-model anchor.
    pub learned_model_hash: Option<String>,
    /// Exact objective anchor.
    pub search_objective_hash: String,
    /// Exact initial root anchor.
    pub initial_anchor_hash: String,
    /// Maximum retained nodes per depth layer.
    pub beam_width: u64,
    /// Maximum semantic edge depth from the root.
    pub maximum_semantic_depth: u64,
    /// Maximum ranked children inspected per expanded node.
    pub maximum_children_retained_per_node: u64,
    /// Expansion ordering.
    pub expansion_order: ExpansionOrder,
    /// Duplicate-state handling.
    pub duplicate_state_policy: DuplicateStatePolicy,
    /// Compiler-rejection handling.
    pub rejection_policy: RejectionTreatment,
    /// Typed-repair handling.
    pub repair_policy: SearchRepairPolicy,
    /// Terminal recommendation handling.
    pub terminal_policy: SearchTerminalPolicy,
    /// Total tie rule.
    pub tie_rule: SearchTieRule,
    /// Deterministic automatic checkpoint cadence in semantic parent expansions.
    pub checkpoint_cadence_work_units: u64,
    /// Independent search-plan hash.
    pub search_plan_hash: String,
}

impl SearchPlan {
    /// Constructs the supported `deterministic_beam_v1` menu plan.
    pub fn deterministic_beam_v1(
        objective: &SearchObjectiveDescriptor,
        ranker: &SearchRanker,
        beam_width: u64,
        maximum_semantic_depth: u64,
        maximum_children_retained_per_node: u64,
        checkpoint_cadence_work_units: u64,
    ) -> EvaluationResult<Self> {
        let descriptor = ranker.descriptor();
        let mut plan = Self {
            version: 1,
            algorithm: SearchAlgorithmKind::DeterministicBeam,
            algorithm_version: "deterministic_beam_v1".to_owned(),
            interaction_surface: descriptor.base_interaction_mode,
            ranking_policy_hash: descriptor.ranking_policy_hash.clone(),
            learned_model_hash: ranker.learned_model_hash().map(str::to_owned),
            search_objective_hash: objective.search_objective_hash.clone(),
            initial_anchor_hash: objective.initial_anchor_hash.clone(),
            beam_width,
            maximum_semantic_depth,
            maximum_children_retained_per_node,
            expansion_order: ExpansionOrder::LevelSynchronousFrontierOrder,
            duplicate_state_policy: DuplicateStatePolicy::PreserveProvenanceCanonicalRepresentative,
            rejection_policy: RejectionTreatment::RecordTerminalLeaf,
            repair_policy: SearchRepairPolicy::RecordOnly,
            terminal_policy: SearchTerminalPolicy::HighestObservedEligible,
            tie_rule: SearchTieRule::ObjectiveRankingStateNode,
            checkpoint_cadence_work_units,
            search_plan_hash: String::new(),
        };
        plan.validate(objective, ranker)?;
        plan.search_plan_hash = search_plan_hash(&plan)?;
        Ok(plan)
    }

    /// Validates the supported Stage 7A plan and all ranker anchors.
    pub fn validate(
        &self,
        objective: &SearchObjectiveDescriptor,
        ranker: &SearchRanker,
    ) -> EvaluationResult<()> {
        validate_ranker(ranker)?;
        if self.version != 1
            || self.algorithm != SearchAlgorithmKind::DeterministicBeam
            || self.algorithm_version != "deterministic_beam_v1"
            || self.interaction_surface != PolicyKind::Menu
            || self.beam_width == 0
            || self.beam_width > MAX_BEAM_WIDTH
            || self.maximum_semantic_depth > MAX_SEMANTIC_DEPTH
            || self.maximum_children_retained_per_node == 0
            || self.maximum_children_retained_per_node > MAX_CHILDREN_PER_NODE
            || self.checkpoint_cadence_work_units == 0
            || self.search_objective_hash != objective.search_objective_hash
            || self.initial_anchor_hash != objective.initial_anchor_hash
            || self.ranking_policy_hash != ranker.descriptor().ranking_policy_hash
            || self.learned_model_hash.as_deref() != ranker.learned_model_hash()
            || !self.search_plan_hash.is_empty() && self.search_plan_hash != search_plan_hash(self)?
        {
            let code = if self.interaction_surface == PolicyKind::Menu {
                EvaluationErrorCode::EvaluationSearchPlanInvalid
            } else {
                EvaluationErrorCode::EvaluationSearchUnsupportedSurface
            };
            return Err(search_error(
                code,
                "unsupported, corrupt, or incompatible search plan",
            ));
        }
        Ok(())
    }
}

/// Runtime Stage 6B/6C ranker supplied to search and replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchRanker {
    /// Existing deterministic Stage 6B scripted ranker.
    Scripted {
        /// Exact ranking descriptor.
        descriptor: RankingPolicyDescriptor,
    },
    /// Existing deterministic Stage 6C learned ranker and immutable model.
    Learned {
        /// Exact learned ranking descriptor.
        descriptor: RankingPolicyDescriptor,
        /// Exact immutable learned model.
        model: Box<LearnedModelArtifact>,
        /// Exact visible Stage 6B schema.
        schema: FeatureSchema,
    },
}

impl SearchRanker {
    /// Returns the exact ranking descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &RankingPolicyDescriptor {
        match self {
            Self::Scripted { descriptor } | Self::Learned { descriptor, .. } => descriptor,
        }
    }

    /// Returns the learned-model anchor when applicable.
    #[must_use]
    pub fn learned_model_hash(&self) -> Option<&str> {
        match self {
            Self::Scripted { .. } => None,
            Self::Learned { model, .. } => Some(&model.learned_model_hash),
        }
    }

    fn decision(
        &self,
        choice_set: &EvaluationChoiceSet,
        task: &EvaluationTask,
    ) -> EvaluationResult<(RankingDecision, Option<InferenceRecord>)> {
        match self {
            Self::Scripted { descriptor } => Ok((
                scripted_ranking_decision(descriptor, choice_set, &RankingLimits::default())?,
                None,
            )),
            Self::Learned {
                descriptor,
                model,
                schema,
            } => {
                let input = ranking_input(
                    choice_set,
                    schema,
                    FrameCompleteness::Complete,
                    PolicyKind::Menu,
                    false,
                    BTreeMap::from([
                        ("task_id".to_owned(), task.id.0.clone()),
                        ("corpus_version".to_owned(), task.corpus_version.clone()),
                        (
                            "task_category".to_owned(),
                            task_category_name(task.category),
                        ),
                        ("task_size".to_owned(), task.size.clone()),
                    ]),
                )?;
                let (decision, record) = infer(
                    &input,
                    schema,
                    model,
                    descriptor,
                    &LearnedRankingLimits::default(),
                )?;
                Ok((decision, Some(record)))
            }
        }
    }
}

fn task_category_name(category: TaskCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Search-local deterministic node identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SearchNodeId(pub String);

/// Search-local deterministic edge identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SearchEdgeId(pub String);

/// Interpretable ordered integer objective value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchObjectiveValue {
    /// Values in exact descriptor component order.
    pub values: Vec<i64>,
}

/// One search graph node with exact trajectory provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchNode {
    /// Search-local identity.
    pub id: SearchNodeId,
    /// Parent node, absent for the root.
    pub parent: Option<SearchNodeId>,
    /// Incoming edge, absent for the root.
    pub parent_edge: Option<SearchEdgeId>,
    /// Semantic edge depth from the root.
    pub depth: u64,
    /// Exact replay-derived compiler/evaluation state identity.
    pub compiler_state_hash: String,
    /// Ordered objective vector computed after the production outcome.
    pub objective: SearchObjectiveValue,
    /// Checked accumulated Stage 6B/6C fixed-point ranking score.
    pub accumulated_ranking_score: i64,
    /// Accepted progress actions on this trajectory.
    pub accepted_progress_count: u64,
    /// Compiler rejections on this trajectory.
    pub rejection_count: u64,
    /// Repair cycles on this trajectory.
    pub repair_count: u64,
    /// Whether the production evaluation episode is complete.
    pub terminal: bool,
    /// Whether the evaluation task criterion succeeded at this terminal.
    pub task_success: bool,
    /// Canonical earlier node for the same state, while this node preserves its parent.
    pub duplicate_of: Option<SearchNodeId>,
    /// Independent node hash.
    pub search_node_hash: String,
}

/// One production-verified search graph edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchEdge {
    /// Search-local edge identity.
    pub id: SearchEdgeId,
    /// Parent node.
    pub parent: SearchNodeId,
    /// Child node.
    pub child: SearchNodeId,
    /// Exact choice-set anchor.
    pub choice_set_hash: String,
    /// Exact selected visible choice.
    pub selected_choice: EvaluationChoiceId,
    /// Selected choice's compiler order.
    pub compiler_order: u64,
    /// Rank within the validated Stage 6B order.
    pub rank: u64,
    /// Fixed-point score retained separately from objective values.
    pub ranking_score: Option<FixedScore>,
    /// Exact validated Stage 6B ranking trace.
    pub ranking_trace: RankingTrace,
    /// Optional exact Stage 6C inference record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference: Option<InferenceRecord>,
    /// Outcome returned by the production verifier/atomic transaction path.
    pub compiler_outcome: CompilerOutcome,
    /// Independent edge hash.
    pub search_edge_hash: String,
}

/// Exact level-synchronous frontier and deterministic next-work cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFrontier {
    /// Current depth being expanded.
    pub depth: u64,
    /// Remaining current-layer nodes in total order.
    pub pending: Vec<SearchNodeId>,
    /// Accepted next-layer candidates awaiting beam retention.
    pub next_candidates: Vec<SearchNodeId>,
    /// Number of current-layer nodes already expanded.
    pub expanded_in_layer: u64,
}

/// Stable terminal/search lifecycle classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    /// More deterministic work can be requested.
    Running,
    /// The complete configured finite envelope was exhausted.
    Complete,
    /// The algorithmic horizon stopped exploration.
    Bounded,
    /// Cooperative cancellation was observed between work units.
    Cancelled,
    /// A structured ranker, production, limit, or checkpoint failure stopped the run.
    Failed,
}

/// Stable stopping classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStoppingReason {
    /// No pending or next-layer node remains.
    EnvelopeExhausted,
    /// Maximum semantic depth was reached.
    SemanticDepthHorizon,
    /// Cooperative cancellation was requested.
    CooperativeCancellation,
    /// Structured failure stopped the run.
    StructuredFailure,
}

/// Deterministic search-specific work accounting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchWorkCounters {
    /// Continuation frames requested.
    pub continuation_frames_requested: u64,
    /// Continuation pages decoded.
    pub continuation_pages_decoded: u64,
    /// Choices inspected.
    pub choices_inspected: u64,
    /// Learned ranking invocations.
    pub ranking_inferences: u64,
    /// Ranking choices scored.
    pub ranking_choices_scored: u64,
    /// Fresh isolated branch harnesses initialized.
    pub branch_engines_initialized: u64,
    /// Production requests submitted, including deterministic branch replay.
    pub production_requests_submitted: u64,
    /// Accepted new child expansions.
    pub accepted_expansions: u64,
    /// Compiler-rejected new child expansions.
    pub compiler_rejected_expansions: u64,
    /// Choices dropped by child or beam policy.
    pub policy_rejected_expansions: u64,
    /// Typed repair attempts observed.
    pub repair_attempts: u64,
    /// Nodes created.
    pub nodes_created: u64,
    /// Edges created.
    pub edges_created: u64,
    /// Duplicate compiler states detected.
    pub duplicate_states_detected: u64,
    /// Total-order frontier comparisons.
    pub frontier_comparisons: u64,
    /// Successfully encoded automatic checkpoints.
    pub checkpoints_encoded: u64,
    /// Prior trajectory edges replayed in isolated engines.
    pub replayed_nodes: u64,
    /// Evaluation archive bytes inspected.
    pub archive_bytes_inspected: u64,
    /// Semantic parent expansion units.
    pub semantic_expansions: u64,
}

impl SearchWorkCounters {
    /// Checked sum of all deterministic search work counters.
    pub fn total(&self) -> EvaluationResult<u64> {
        [
            self.continuation_frames_requested,
            self.continuation_pages_decoded,
            self.choices_inspected,
            self.ranking_inferences,
            self.ranking_choices_scored,
            self.branch_engines_initialized,
            self.production_requests_submitted,
            self.accepted_expansions,
            self.compiler_rejected_expansions,
            self.policy_rejected_expansions,
            self.repair_attempts,
            self.nodes_created,
            self.edges_created,
            self.duplicate_states_detected,
            self.frontier_comparisons,
            self.checkpoints_encoded,
            self.replayed_nodes,
            self.archive_bytes_inspected,
            self.semantic_expansions,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| {
            search_error(
                EvaluationErrorCode::EvaluationWorkUnitOverflow,
                "search work counter overflow",
            )
        })
    }
}

/// Operational safety limits excluded from objective, plan, node, trace, and result identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    /// Maximum nodes retained in one session.
    pub nodes: u64,
    /// Maximum edges retained in one session.
    pub edges: u64,
    /// Maximum semantic parent expansions.
    pub semantic_expansions: u64,
    /// Maximum encoded checkpoint bytes.
    pub checkpoint_bytes: u64,
    /// Maximum isolated branch initializations.
    pub branch_engines: u64,
    /// Maximum production requests including branch replay.
    pub production_requests: u64,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            edges: 100_000,
            semantic_expansions: 100_000,
            checkpoint_bytes: 256 * 1024 * 1024,
            branch_engines: 1_000_000,
            production_requests: 10_000_000,
        }
    }
}

/// One deterministic semantic event in a replayable search trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchTraceEvent {
    /// A parent was selected for expansion.
    Expand {
        /// Parent node.
        node: SearchNodeId,
        /// Semantic work cursor before expansion.
        work_cursor: u64,
    },
    /// A child edge and node were published after production execution.
    Child {
        /// Edge identity.
        edge: SearchEdgeId,
        /// Child identity.
        node: SearchNodeId,
        /// Production acceptance.
        accepted: bool,
    },
    /// A layer was sorted and pruned to the plan beam width.
    RetainLayer {
        /// New layer depth.
        depth: u64,
        /// Retained total-order node identities.
        nodes: Vec<SearchNodeId>,
    },
}

/// Complete semantic search trace, independent of advance-call partitioning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchTrace {
    /// Root anchor.
    pub initial_anchor_hash: String,
    /// Objective anchor.
    pub search_objective_hash: String,
    /// Plan anchor.
    pub search_plan_hash: String,
    /// Exact deterministic events.
    pub events: Vec<SearchTraceEvent>,
    /// Final ordered node hashes.
    pub node_hashes: Vec<String>,
    /// Final ordered edge hashes.
    pub edge_hashes: Vec<String>,
    /// Final lifecycle status.
    pub status: SearchStatus,
    /// Independent trace hash.
    pub search_trace_hash: String,
}

/// Non-authoritative selected terminal or bounded-frontier recommendation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Root anchor.
    pub initial_anchor_hash: String,
    /// Objective anchor.
    pub search_objective_hash: String,
    /// Plan anchor.
    pub search_plan_hash: String,
    /// Trace anchor.
    pub search_trace_hash: String,
    /// Complete/bounded/cancelled/failed classification.
    pub status: SearchStatus,
    /// Exact stopping reason.
    pub stopping_reason: SearchStoppingReason,
    /// Highest-ranked observed eligible terminal, if any.
    pub selected_terminal_trajectory: Option<SearchNodeId>,
    /// Non-authoritative terminal or frontier recommendation.
    pub recommended_trajectory: Option<SearchNodeId>,
    /// Remaining bounded frontier in exact total order.
    pub bounded_frontier: Vec<SearchNodeId>,
    /// Explicit terminology contract; never `optimal` or `best`.
    pub recommendation_semantics: String,
    /// Independent result hash.
    pub search_result_hash: String,
}

/// Search-specific bounded repair category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRepairCode {
    /// Rebuild from the exact current root.
    StaleSearchRoot,
    /// Resume from a verified compatible checkpoint.
    StaleSearchCheckpoint,
    /// Recreate an objective against the exact task anchor.
    IncompatibleObjective,
    /// Recreate a supported deterministic plan.
    IncompatibleSearchPlan,
    /// Regenerate a structurally valid frontier.
    CorruptFrontier,
    /// Use the Stage 7A menu surface.
    UnsupportedSearchSurface,
    /// Advance or resume before requesting a publishable complete result.
    IncompleteSearchCannotPublish,
}

/// Independently hashed bounded search repair descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRepairDescriptor {
    /// Repair category.
    pub code: SearchRepairCode,
    /// Exact failing root/objective/plan/checkpoint anchor.
    pub failing_anchor: String,
    /// Maximum ordinary follow-up requests described by this repair.
    pub maximum_actions: u64,
    /// Independent repair hash.
    pub search_repair_hash: String,
}

/// Structured retained search rejection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRejection {
    /// Stable evaluation diagnostic code.
    pub code: EvaluationErrorCode,
    /// Exact failing anchor.
    pub failing_anchor: String,
    /// Optional bounded typed repair.
    pub repair: Option<SearchRepairDescriptor>,
}

/// Compact archive/run summary separate from compiler state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRunRecord {
    /// Stable deterministic run identity.
    pub search_run_id: String,
    /// Root anchor.
    pub initial_anchor_hash: String,
    /// Objective anchor.
    pub search_objective_hash: String,
    /// Plan anchor.
    pub search_plan_hash: String,
    /// Final lifecycle status.
    pub status: SearchStatus,
    /// Exact final semantic work cursor.
    pub semantic_work: u64,
    /// Automatic deterministic checkpoint chain.
    pub checkpoint_hashes: Vec<String>,
    /// Final trace anchor when terminal.
    pub search_trace_hash: Option<String>,
    /// Final result anchor when terminal.
    pub search_result_hash: Option<String>,
}

/// Complete serializable deterministic search state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchSession {
    /// Stable deterministic run identity.
    pub search_run_id: String,
    /// Exact objective contract.
    pub objective: SearchObjectiveDescriptor,
    /// Exact search plan.
    pub plan: SearchPlan,
    /// Exact Stage 6B/6C ranking descriptor used for every search frame.
    pub ranking_policy: RankingPolicyDescriptor,
    /// Exact immutable corpus retained for isolated branch reconstruction.
    pub corpus: EvaluationCorpus,
    /// Exact task identity.
    pub task_id: EvaluationTaskId,
    /// All nodes in search-local ID order.
    pub nodes: BTreeMap<SearchNodeId, SearchNode>,
    /// All edges in search-local ID order.
    pub edges: BTreeMap<SearchEdgeId, SearchEdge>,
    /// Exact next-work frontier.
    pub frontier: SearchFrontier,
    /// Ordered terminal candidates.
    pub terminal_candidates: Vec<SearchNodeId>,
    /// Canonical representative for each exact compiler-state observation.
    pub duplicate_state_index: BTreeMap<String, SearchNodeId>,
    /// Deterministic semantic trace events.
    pub trace_events: Vec<SearchTraceEvent>,
    /// Automatic checkpoint chain at plan cadence.
    pub checkpoint_hashes: Vec<String>,
    /// Deterministic work accounting.
    pub work: SearchWorkCounters,
    /// Current lifecycle status.
    pub status: SearchStatus,
    /// Cooperative cancellation request, checked only between units.
    pub cancellation_requested: bool,
    /// Structured retained rejection on failure.
    pub rejection: Option<SearchRejection>,
    /// Final semantic trace, if stopped.
    pub trace: Option<SearchTrace>,
    /// Final non-authoritative result, if stopped.
    pub result: Option<SearchResult>,
    next_node: u64,
    next_edge: u64,
}

/// Exact resumable search checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchCheckpoint {
    /// Checkpoint schema version.
    pub version: u32,
    /// Stable payload prefix.
    pub prefix: String,
    /// Root anchor.
    pub initial_anchor_hash: String,
    /// Objective anchor.
    pub search_objective_hash: String,
    /// Plan anchor.
    pub search_plan_hash: String,
    /// Ranking policy anchor.
    pub ranking_policy_hash: String,
    /// Learned model anchor when applicable.
    pub learned_model_hash: Option<String>,
    /// Exact deterministic next work cursor.
    pub next_work_cursor: u64,
    /// Complete resumable state.
    pub session: SearchSession,
    /// Independent checkpoint hash/digest.
    pub search_checkpoint_hash: String,
}

impl SearchSession {
    /// Starts one exact immutable-root search without executing a branch action.
    pub fn start(
        corpus: EvaluationCorpus,
        task_id: EvaluationTaskId,
        objective: SearchObjectiveDescriptor,
        plan: SearchPlan,
        ranker: &SearchRanker,
    ) -> EvaluationResult<Self> {
        let task = corpus
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationTaskNotFound,
                    "search task does not exist in the exact corpus",
                )
            })?;
        objective.validate(&corpus, task).map_err(|_| {
            search_error(
                EvaluationErrorCode::EvaluationSearchRootStale,
                "search objective root is stale",
            )
        })?;
        plan.validate(&objective, ranker)?;
        validate_ranker(ranker)?;
        let root_state = domain_hash(
            SEARCH_STATE_HASH_DOMAIN,
            &(
                &objective.initial_anchor_hash,
                compiler_build_hash(),
                Vec::<(&PolicyDecision, &CompilerOutcome)>::new(),
            ),
        )?;
        let root_id = SearchNodeId("search-node-1".to_owned());
        let root_metrics = NodeStats::default();
        let mut root = SearchNode {
            id: root_id.clone(),
            parent: None,
            parent_edge: None,
            depth: 0,
            compiler_state_hash: root_state.clone(),
            objective: objective_value(&objective, &root_metrics)?,
            accumulated_ranking_score: 0,
            accepted_progress_count: 0,
            rejection_count: 0,
            repair_count: 0,
            terminal: task.initial_state.production_requests.is_empty(),
            task_success: false,
            duplicate_of: None,
            search_node_hash: String::new(),
        };
        root.search_node_hash = search_node_hash(&root)?;
        let run_id = format!(
            "search-run-{}",
            &domain_hash(
                SEARCH_ROOT_HASH_DOMAIN,
                &(
                    &objective.search_objective_hash,
                    &plan.search_plan_hash,
                    &ranker.descriptor().ranking_policy_hash,
                ),
            )?[..16]
        );
        let terminal = root.terminal;
        let mut nodes = BTreeMap::new();
        nodes.insert(root_id.clone(), root);
        let mut session = Self {
            search_run_id: run_id,
            objective,
            plan,
            ranking_policy: ranker.descriptor().clone(),
            corpus,
            task_id,
            nodes,
            edges: BTreeMap::new(),
            frontier: SearchFrontier {
                depth: 0,
                pending: if terminal {
                    Vec::new()
                } else {
                    vec![root_id.clone()]
                },
                next_candidates: Vec::new(),
                expanded_in_layer: 0,
            },
            terminal_candidates: if terminal {
                vec![root_id.clone()]
            } else {
                Vec::new()
            },
            duplicate_state_index: BTreeMap::from([(root_state, root_id)]),
            trace_events: Vec::new(),
            checkpoint_hashes: Vec::new(),
            work: SearchWorkCounters {
                nodes_created: 1,
                ..SearchWorkCounters::default()
            },
            status: SearchStatus::Running,
            cancellation_requested: false,
            rejection: None,
            trace: None,
            result: None,
            next_node: 2,
            next_edge: 1,
        };
        if terminal {
            session.finish(
                SearchStatus::Complete,
                SearchStoppingReason::EnvelopeExhausted,
            )?;
        } else if session.plan.maximum_semantic_depth == 0 {
            session.finish(
                SearchStatus::Bounded,
                SearchStoppingReason::SemanticDepthHorizon,
            )?;
        }
        Ok(session)
    }

    /// Requests cooperative cancellation at the next deterministic unit boundary.
    pub fn request_cancellation(&mut self) {
        if self.status == SearchStatus::Running {
            self.cancellation_requested = true;
        }
    }

    /// Advances by at most the requested number of semantic parent-expansion units.
    pub fn advance(
        &mut self,
        maximum_work_units: u64,
        ranker: &SearchRanker,
        limits: &SearchLimits,
    ) -> EvaluationResult<SearchStatus> {
        if self.status != SearchStatus::Running {
            return Ok(self.status);
        }
        self.validate_anchors(ranker)?;
        let mut advanced = 0_u64;
        while advanced < maximum_work_units && self.status == SearchStatus::Running {
            if self.cancellation_requested {
                self.finish(
                    SearchStatus::Cancelled,
                    SearchStoppingReason::CooperativeCancellation,
                )?;
                break;
            }
            if self.frontier.pending.is_empty() {
                self.finish_layer()?;
                if self.status != SearchStatus::Running {
                    break;
                }
                continue;
            }
            if self.work.semantic_expansions >= limits.semantic_expansions {
                self.fail_limit("semantic expansions", limits.semantic_expansions)?;
                break;
            }
            let node_id = self.frontier.pending.remove(0);
            if let Err(error) = self.expand_node(&node_id, ranker, limits) {
                self.fail(&error)?;
                break;
            }
            self.work.semantic_expansions = checked_add_u64(self.work.semantic_expansions, 1)?;
            self.frontier.expanded_in_layer = checked_add_u64(self.frontier.expanded_in_layer, 1)?;
            advanced = checked_add_u64(advanced, 1)?;
            if self.work.semantic_expansions % self.plan.checkpoint_cadence_work_units == 0 {
                match self.checkpoint(limits) {
                    Ok(checkpoint) => {
                        self.work.checkpoints_encoded =
                            checked_add_u64(self.work.checkpoints_encoded, 1)?;
                        self.checkpoint_hashes
                            .push(checkpoint.search_checkpoint_hash);
                    }
                    Err(error) => {
                        self.fail(&error)?;
                        break;
                    }
                }
            }
            if self.frontier.pending.is_empty() {
                self.finish_layer()?;
            }
        }
        Ok(self.status)
    }

    /// Encodes one exact resumable checkpoint without mutating the search state.
    pub fn checkpoint(&self, limits: &SearchLimits) -> EvaluationResult<SearchCheckpoint> {
        self.validate_structural()?;
        let mut checkpoint = SearchCheckpoint {
            version: 1,
            prefix: "agentir-search-checkpoint-v1".to_owned(),
            initial_anchor_hash: self.objective.initial_anchor_hash.clone(),
            search_objective_hash: self.objective.search_objective_hash.clone(),
            search_plan_hash: self.plan.search_plan_hash.clone(),
            ranking_policy_hash: self.plan.ranking_policy_hash.clone(),
            learned_model_hash: self.plan.learned_model_hash.clone(),
            next_work_cursor: self.work.semantic_expansions,
            session: self.clone(),
            search_checkpoint_hash: String::new(),
        };
        checkpoint.search_checkpoint_hash = search_checkpoint_hash(&checkpoint)?;
        let bytes = serde_json::to_vec(&checkpoint).map_err(|error| {
            search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                format!("search checkpoint encoding failed: {error}"),
            )
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.checkpoint_bytes {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                "search checkpoint byte limit exceeded",
            )
            .expected_actual(json!(limits.checkpoint_bytes), json!(bytes.len())));
        }
        Ok(checkpoint)
    }

    /// Verifies and resumes an exact checkpoint before any branch execution.
    pub fn resume(
        checkpoint: &SearchCheckpoint,
        corpus: &EvaluationCorpus,
        ranker: &SearchRanker,
        limits: &SearchLimits,
    ) -> EvaluationResult<Self> {
        if checkpoint.version != 1
            || checkpoint.prefix != "agentir-search-checkpoint-v1"
            || checkpoint.search_checkpoint_hash != search_checkpoint_hash(checkpoint)?
            || checkpoint.next_work_cursor != checkpoint.session.work.semantic_expansions
            || checkpoint.initial_anchor_hash != checkpoint.session.objective.initial_anchor_hash
            || checkpoint.search_objective_hash
                != checkpoint.session.objective.search_objective_hash
            || checkpoint.search_plan_hash != checkpoint.session.plan.search_plan_hash
            || checkpoint.ranking_policy_hash != ranker.descriptor().ranking_policy_hash
            || checkpoint.learned_model_hash.as_deref() != ranker.learned_model_hash()
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                "search checkpoint prefix, version, digest, cursor, or anchors are invalid",
            ));
        }
        let bytes = serde_json::to_vec(checkpoint).map_err(|error| {
            search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                format!("search checkpoint cannot be encoded: {error}"),
            )
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.checkpoint_bytes {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                "search checkpoint byte limit exceeded",
            ));
        }
        if &checkpoint.session.corpus != corpus {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointStale,
                "search checkpoint corpus/root anchor is stale",
            ));
        }
        let session = checkpoint.session.clone();
        session.validate_anchors(ranker)?;
        session.validate_structural()?;
        Ok(session)
    }

    /// Returns a compact archive/run record.
    #[must_use]
    pub fn run_record(&self) -> SearchRunRecord {
        SearchRunRecord {
            search_run_id: self.search_run_id.clone(),
            initial_anchor_hash: self.objective.initial_anchor_hash.clone(),
            search_objective_hash: self.objective.search_objective_hash.clone(),
            search_plan_hash: self.plan.search_plan_hash.clone(),
            status: self.status,
            semantic_work: self.work.semantic_expansions,
            checkpoint_hashes: self.checkpoint_hashes.clone(),
            search_trace_hash: self
                .trace
                .as_ref()
                .map(|trace| trace.search_trace_hash.clone()),
            search_result_hash: self
                .result
                .as_ref()
                .map(|result| result.search_result_hash.clone()),
        }
    }

    fn validate_anchors(&self, ranker: &SearchRanker) -> EvaluationResult<()> {
        let task = self.task()?;
        self.objective.validate(&self.corpus, task).map_err(|_| {
            search_error(
                EvaluationErrorCode::EvaluationSearchRootStale,
                "search root no longer matches the immutable task anchor",
            )
        })?;
        self.plan.validate(&self.objective, ranker)?;
        if self.ranking_policy != *ranker.descriptor() {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchPlanInvalid,
                "search checkpoint ranking descriptor differs from the exact runtime ranker",
            ));
        }
        validate_ranker(ranker)
    }

    fn task(&self) -> EvaluationResult<&EvaluationTask> {
        self.corpus
            .tasks
            .iter()
            .find(|task| task.id == self.task_id)
            .ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchRootStale,
                    "search task is missing from the retained corpus",
                )
            })
    }

    fn expand_node(
        &mut self,
        node_id: &SearchNodeId,
        ranker: &SearchRanker,
        limits: &SearchLimits,
    ) -> EvaluationResult<()> {
        let parent = self.nodes.get(node_id).cloned().ok_or_else(|| {
            search_error(
                EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                "frontier references a missing search node",
            )
        })?;
        self.trace_events.push(SearchTraceEvent::Expand {
            node: node_id.clone(),
            work_cursor: self.work.semantic_expansions,
        });
        let (harness, episode_id) = self.rebuild_branch(node_id, ranker, limits)?;
        let _observation = harness.next_observation(&episode_id)?;
        self.work.continuation_frames_requested =
            checked_add_u64(self.work.continuation_frames_requested, 1)?;
        self.work.continuation_pages_decoded =
            checked_add_u64(self.work.continuation_pages_decoded, 1)?;
        let choice_set = harness.ranked_choice_set(&episode_id)?;
        let choice_count = u64::try_from(choice_set.choices.len()).unwrap_or(u64::MAX);
        self.work.choices_inspected = checked_add_u64(self.work.choices_inspected, choice_count)?;
        self.work.ranking_choices_scored =
            checked_add_u64(self.work.ranking_choices_scored, choice_count)?;
        let task = self.task()?.clone();
        let (decision, inference) = ranker.decision(&choice_set, &task)?;
        if inference.is_some() {
            self.work.ranking_inferences = checked_add_u64(self.work.ranking_inferences, 1)?;
        }
        let trace = rank_choices(
            &choice_set,
            ranker.descriptor(),
            decision,
            &RankingLimits::default(),
        )?;
        let retain = usize::try_from(self.plan.maximum_children_retained_per_node)
            .unwrap_or(usize::MAX)
            .min(trace.ranking.len());
        self.work.policy_rejected_expansions = checked_add_u64(
            self.work.policy_rejected_expansions,
            u64::try_from(trace.ranking.len().saturating_sub(retain)).unwrap_or(u64::MAX),
        )?;
        for ranked in trace.ranking.iter().take(retain) {
            self.create_child(
                &parent,
                &choice_set,
                &trace,
                inference.clone(),
                ranked,
                ranker,
                limits,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn create_child(
        &mut self,
        parent: &SearchNode,
        choice_set: &EvaluationChoiceSet,
        trace: &RankingTrace,
        inference: Option<InferenceRecord>,
        ranked_choice: &RankedChoice,
        runtime_ranker: &SearchRanker,
        limits: &SearchLimits,
    ) -> EvaluationResult<()> {
        if u64::try_from(self.nodes.len()).unwrap_or(u64::MAX) >= limits.nodes
            || u64::try_from(self.edges.len()).unwrap_or(u64::MAX) >= limits.edges
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                "search graph node or edge limit exceeded before production execution",
            ));
        }
        let choice = choice_set
            .choices
            .iter()
            .find(|choice| choice.id == ranked_choice.choice_id)
            .ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationRankingReplayMismatch,
                    "ranked child is absent from the exact choice set",
                )
            })?;
        let (mut harness, episode_id) = self.rebuild_branch(&parent.id, runtime_ranker, limits)?;
        let observation = harness.next_observation(&episode_id)?;
        let menu_choice = observation
            .continuation_frame
            .get(usize::try_from(choice.compiler_order).unwrap_or(usize::MAX))
            .ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchReplayMismatch,
                    "ranked compiler order is outside the production continuation",
                )
            })?
            .choice_id
            .clone();
        self.check_production_limit(limits, 1)?;
        let outcome = harness.submit(
            &episode_id,
            &observation.step_id,
            &observation.observation_hash,
            PolicyDecision::MenuChoice {
                choice_id: menu_choice,
            },
            None,
            None,
        )?;
        self.work.production_requests_submitted =
            checked_add_u64(self.work.production_requests_submitted, 1)?;
        let episode = harness
            .run(harness.run_ids().next().unwrap_or_default())?
            .episodes
            .iter()
            .find(|episode| episode.id == episode_id)
            .ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchReplayMismatch,
                    "isolated branch episode disappeared after production execution",
                )
            })?;
        let result = episode.result.clone();
        let score = ranked_choice.score.map_or(0_i64, |score| score.units);
        let accumulated_score = parent
            .accumulated_ranking_score
            .checked_add(score)
            .ok_or_else(search_overflow)?;
        let accepted_progress_count = checked_add_u64(
            parent.accepted_progress_count,
            u64::from(outcome.accepted && outcome.progress_producing),
        )?;
        let rejection_count =
            checked_add_u64(parent.rejection_count, u64::from(!outcome.accepted))?;
        let repair_count = result.as_ref().map_or(parent.repair_count, |result| {
            u64::try_from(result.repair_cycles.len()).unwrap_or(u64::MAX)
        });
        let depth = checked_add_u64(parent.depth, 1)?;
        let stats = NodeStats {
            compiler_terminal_success: result.as_ref().is_some_and(|result| result.success),
            task_success: result
                .as_ref()
                .is_some_and(|result| result.semantic.criterion_satisfied),
            accepted_progress_count,
            open_obligation_count: 0,
            rejection_count,
            repair_count,
            deterministic_work_units: depth,
            accumulated_ranking_score: accumulated_score,
            trajectory_length: depth,
        };
        let state_hash = compiler_state_hash(&self.objective, episode)?;
        let duplicate_of = self.duplicate_state_index.get(&state_hash).cloned();
        if duplicate_of.is_some() {
            self.work.duplicate_states_detected =
                checked_add_u64(self.work.duplicate_states_detected, 1)?;
        }
        let node_id = SearchNodeId(format!("search-node-{}", self.next_node));
        let edge_id = SearchEdgeId(format!("search-edge-{}", self.next_edge));
        self.next_node = checked_add_u64(self.next_node, 1)?;
        self.next_edge = checked_add_u64(self.next_edge, 1)?;
        let mut node = SearchNode {
            id: node_id.clone(),
            parent: Some(parent.id.clone()),
            parent_edge: Some(edge_id.clone()),
            depth,
            compiler_state_hash: state_hash.clone(),
            objective: objective_value(&self.objective, &stats)?,
            accumulated_ranking_score: accumulated_score,
            accepted_progress_count,
            rejection_count,
            repair_count,
            terminal: result.is_some(),
            task_success: stats.task_success,
            duplicate_of,
            search_node_hash: String::new(),
        };
        node.search_node_hash = search_node_hash(&node)?;
        let mut edge = SearchEdge {
            id: edge_id.clone(),
            parent: parent.id.clone(),
            child: node_id.clone(),
            choice_set_hash: choice_set.choice_set_hash.clone(),
            selected_choice: ranked_choice.choice_id.clone(),
            compiler_order: choice.compiler_order,
            rank: ranked_choice.rank,
            ranking_score: ranked_choice.score,
            ranking_trace: trace.clone(),
            inference,
            compiler_outcome: outcome.clone(),
            search_edge_hash: String::new(),
        };
        edge.search_edge_hash = search_edge_hash(&edge)?;
        self.nodes.insert(node_id.clone(), node.clone());
        self.edges.insert(edge_id.clone(), edge);
        self.duplicate_state_index
            .entry(state_hash)
            .or_insert_with(|| node_id.clone());
        self.work.nodes_created = checked_add_u64(self.work.nodes_created, 1)?;
        self.work.edges_created = checked_add_u64(self.work.edges_created, 1)?;
        if outcome.accepted {
            self.work.accepted_expansions = checked_add_u64(self.work.accepted_expansions, 1)?;
            if node.terminal {
                self.terminal_candidates.push(node_id.clone());
            } else if node.duplicate_of.is_none() {
                self.frontier.next_candidates.push(node_id.clone());
            }
        } else {
            self.work.compiler_rejected_expansions =
                checked_add_u64(self.work.compiler_rejected_expansions, 1)?;
            if matches!(
                outcome.rejection,
                Some(RejectionClassification::PolicyViolation)
            ) {
                self.work.policy_rejected_expansions =
                    checked_add_u64(self.work.policy_rejected_expansions, 1)?;
            }
        }
        self.trace_events.push(SearchTraceEvent::Child {
            edge: edge_id,
            node: node_id,
            accepted: outcome.accepted,
        });
        Ok(())
    }

    fn rebuild_branch(
        &mut self,
        node_id: &SearchNodeId,
        ranker: &SearchRanker,
        limits: &SearchLimits,
    ) -> EvaluationResult<(EvaluationHarness, String)> {
        if self.work.branch_engines_initialized >= limits.branch_engines {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                "isolated branch-engine limit exceeded",
            ));
        }
        let task = self.task()?.clone();
        let policy = ranked_policy(
            ranker.descriptor(),
            match ranker {
                SearchRanker::Scripted { .. } => PolicyOrigin::Scripted,
                SearchRanker::Learned { .. } => PolicyOrigin::Learned,
            },
        )?;
        let mut harness = EvaluationHarness::with_corpus_and_limits(
            self.corpus.clone(),
            EvaluationLimits::default(),
        )?;
        harness.register_ranking_policy(ranker.descriptor().clone())?;
        let run_id = harness.start_run(policy, std::slice::from_ref(&self.task_id), &[0])?;
        let episode_id = harness.run(&run_id)?.episodes[0].id.clone();
        self.work.branch_engines_initialized =
            checked_add_u64(self.work.branch_engines_initialized, 1)?;
        let path = self.path_edges(node_id)?;
        self.check_production_limit(limits, u64::try_from(path.len()).unwrap_or(u64::MAX))?;
        for edge_id in path {
            let retained = self.edges.get(&edge_id).cloned().ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchReplayMismatch,
                    "trajectory references a missing retained edge",
                )
            })?;
            let observation = harness.next_observation(&episode_id)?;
            let choice_set = harness.ranked_choice_set(&episode_id)?;
            let (decision, inference) = ranker.decision(&choice_set, &task)?;
            let trace = rank_choices(
                &choice_set,
                ranker.descriptor(),
                decision,
                &RankingLimits::default(),
            )?;
            if trace != retained.ranking_trace
                || inference != retained.inference
                || choice_set.choice_set_hash != retained.choice_set_hash
            {
                return Err(search_error(
                    EvaluationErrorCode::EvaluationSearchReplayMismatch,
                    "branch ranking or choice set differs from retained provenance",
                ));
            }
            let menu_choice = observation
                .continuation_frame
                .get(usize::try_from(retained.compiler_order).unwrap_or(usize::MAX))
                .ok_or_else(|| {
                    search_error(
                        EvaluationErrorCode::EvaluationSearchReplayMismatch,
                        "retained compiler order is outside the replayed continuation",
                    )
                })?
                .choice_id
                .clone();
            let outcome = harness.submit(
                &episode_id,
                &observation.step_id,
                &observation.observation_hash,
                PolicyDecision::MenuChoice {
                    choice_id: menu_choice,
                },
                None,
                None,
            )?;
            if outcome != retained.compiler_outcome {
                return Err(search_error(
                    EvaluationErrorCode::EvaluationSearchReplayMismatch,
                    "production outcome differs during isolated branch replay",
                ));
            }
            self.work.production_requests_submitted =
                checked_add_u64(self.work.production_requests_submitted, 1)?;
            self.work.replayed_nodes = checked_add_u64(self.work.replayed_nodes, 1)?;
        }
        Ok((harness, episode_id))
    }

    fn path_edges(&self, node_id: &SearchNodeId) -> EvaluationResult<Vec<SearchEdgeId>> {
        let mut path = Vec::new();
        let mut current = self.nodes.get(node_id).ok_or_else(|| {
            search_error(
                EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                "search path starts at a missing node",
            )
        })?;
        let mut seen = BTreeSet::new();
        while let Some(edge) = &current.parent_edge {
            if !seen.insert(current.id.clone()) {
                return Err(search_error(
                    EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                    "cycle detected in search parent provenance",
                ));
            }
            path.push(edge.clone());
            let parent = current.parent.as_ref().ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                    "non-root search node is missing its parent",
                )
            })?;
            current = self.nodes.get(parent).ok_or_else(|| {
                search_error(
                    EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                    "search parent is missing",
                )
            })?;
        }
        path.reverse();
        Ok(path)
    }

    fn finish_layer(&mut self) -> EvaluationResult<()> {
        if !self.frontier.pending.is_empty() {
            return Ok(());
        }
        if self.frontier.next_candidates.is_empty() {
            self.finish(
                SearchStatus::Complete,
                SearchStoppingReason::EnvelopeExhausted,
            )?;
            return Ok(());
        }
        let objective = self.objective.clone();
        let nodes = &self.nodes;
        let comparisons = std::cell::Cell::new(0_u64);
        self.frontier.next_candidates.sort_by(|left, right| {
            comparisons.set(comparisons.get().saturating_add(1));
            compare_nodes(&objective, &nodes[left], &nodes[right])
        });
        self.work.frontier_comparisons =
            checked_add_u64(self.work.frontier_comparisons, comparisons.get())?;
        let beam = usize::try_from(self.plan.beam_width).unwrap_or(usize::MAX);
        let dropped = self.frontier.next_candidates.len().saturating_sub(beam);
        self.work.policy_rejected_expansions = checked_add_u64(
            self.work.policy_rejected_expansions,
            u64::try_from(dropped).unwrap_or(u64::MAX),
        )?;
        self.frontier.next_candidates.truncate(beam);
        let next_depth = checked_add_u64(self.frontier.depth, 1)?;
        let retained = std::mem::take(&mut self.frontier.next_candidates);
        self.trace_events.push(SearchTraceEvent::RetainLayer {
            depth: next_depth,
            nodes: retained.clone(),
        });
        self.frontier.depth = next_depth;
        self.frontier.expanded_in_layer = 0;
        self.frontier.pending = retained;
        if next_depth >= self.plan.maximum_semantic_depth {
            self.finish(
                SearchStatus::Bounded,
                SearchStoppingReason::SemanticDepthHorizon,
            )?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        status: SearchStatus,
        stopping_reason: SearchStoppingReason,
    ) -> EvaluationResult<()> {
        self.status = status;
        let mut node_hashes = self
            .nodes
            .values()
            .map(|node| node.search_node_hash.clone())
            .collect::<Vec<_>>();
        let mut edge_hashes = self
            .edges
            .values()
            .map(|edge| edge.search_edge_hash.clone())
            .collect::<Vec<_>>();
        node_hashes.sort();
        edge_hashes.sort();
        let mut trace = SearchTrace {
            initial_anchor_hash: self.objective.initial_anchor_hash.clone(),
            search_objective_hash: self.objective.search_objective_hash.clone(),
            search_plan_hash: self.plan.search_plan_hash.clone(),
            events: self.trace_events.clone(),
            node_hashes,
            edge_hashes,
            status,
            search_trace_hash: String::new(),
        };
        trace.search_trace_hash = search_trace_hash(&trace)?;
        let selected_terminal = self.highest_ordered(&self.terminal_candidates);
        let frontier_ids = self
            .frontier
            .pending
            .iter()
            .chain(&self.frontier.next_candidates)
            .cloned()
            .collect::<Vec<_>>();
        let recommended = selected_terminal
            .clone()
            .or_else(|| self.highest_ordered(&frontier_ids));
        let mut result = SearchResult {
            initial_anchor_hash: self.objective.initial_anchor_hash.clone(),
            search_objective_hash: self.objective.search_objective_hash.clone(),
            search_plan_hash: self.plan.search_plan_hash.clone(),
            search_trace_hash: trace.search_trace_hash.clone(),
            status,
            stopping_reason,
            selected_terminal_trajectory: selected_terminal,
            recommended_trajectory: recommended,
            bounded_frontier: frontier_ids,
            recommendation_semantics: if status == SearchStatus::Complete {
                "highest-ranked observed terminal under exact search plan"
            } else {
                "bounded frontier result"
            }
            .to_owned(),
            search_result_hash: String::new(),
        };
        result.search_result_hash = search_result_hash(&result)?;
        self.trace = Some(trace);
        self.result = Some(result);
        Ok(())
    }

    fn highest_ordered(&self, candidates: &[SearchNodeId]) -> Option<SearchNodeId> {
        candidates
            .iter()
            .filter_map(|id| self.nodes.get(id).map(|node| (id, node)))
            .min_by(|(_, left), (_, right)| compare_nodes(&self.objective, left, right))
            .map(|(id, _)| id.clone())
    }

    fn fail_limit(&mut self, resource: &str, maximum: u64) -> EvaluationResult<()> {
        self.fail(
            &search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                format!("search safety limit exceeded for {resource}"),
            )
            .expected_actual(json!(maximum), json!(self.work.semantic_expansions)),
        )
    }

    fn fail(&mut self, error: &EvaluationDiagnostic) -> EvaluationResult<()> {
        let anchor = self.plan.search_plan_hash.clone();
        self.rejection = Some(SearchRejection {
            code: error.code,
            failing_anchor: anchor.clone(),
            repair: Some(search_repair(
                SearchRepairCode::IncompleteSearchCannotPublish,
                anchor,
            )?),
        });
        self.finish(
            SearchStatus::Failed,
            SearchStoppingReason::StructuredFailure,
        )
    }

    fn check_production_limit(
        &self,
        limits: &SearchLimits,
        additional: u64,
    ) -> EvaluationResult<()> {
        let projected = self
            .work
            .production_requests_submitted
            .checked_add(additional)
            .ok_or_else(search_overflow)?;
        if projected > limits.production_requests {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchLimitExceeded,
                "search production-request safety limit exceeded before execution",
            ));
        }
        Ok(())
    }

    /// Performs cheap-to-expensive structural validation without branch execution.
    pub fn validate_structural(&self) -> EvaluationResult<()> {
        if self.nodes.is_empty()
            || self
                .nodes
                .values()
                .any(|node| node.search_node_hash != search_node_hash(node).unwrap_or_default())
            || self
                .edges
                .values()
                .any(|edge| edge.search_edge_hash != search_edge_hash(edge).unwrap_or_default())
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                "search node or edge hash mismatch",
            ));
        }
        let mut roots = 0_u64;
        for node in self.nodes.values() {
            match (&node.parent, &node.parent_edge) {
                (None, None) => roots = roots.saturating_add(1),
                (Some(parent), Some(edge)) => {
                    let retained = self.edges.get(edge).ok_or_else(|| {
                        search_error(
                            EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                            "search node references a missing edge",
                        )
                    })?;
                    if &retained.parent != parent || retained.child != node.id {
                        return Err(search_error(
                            EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                            "search parent/edge provenance is inconsistent",
                        ));
                    }
                }
                _ => {
                    return Err(search_error(
                        EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                        "search node has partial parent provenance",
                    ));
                }
            }
            self.path_edges(&node.id)?;
        }
        if roots != 1
            || self
                .frontier
                .pending
                .iter()
                .chain(&self.frontier.next_candidates)
                .any(|id| !self.nodes.contains_key(id))
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                "search root count or frontier membership is invalid",
            ));
        }
        let mut ordered = self.frontier.pending.clone();
        ordered.sort_by(|left, right| {
            compare_nodes(&self.objective, &self.nodes[left], &self.nodes[right])
        });
        if ordered != self.frontier.pending {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchFrontierCorrupt,
                "search frontier is not in deterministic total order",
            ));
        }
        if let Some(trace) = &self.trace
            && trace.search_trace_hash != search_trace_hash(trace)?
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                "search trace hash mismatch",
            ));
        }
        if let Some(result) = &self.result
            && result.search_result_hash != search_result_hash(result)?
        {
            return Err(search_error(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                "search result hash mismatch",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct NodeStats {
    compiler_terminal_success: bool,
    task_success: bool,
    accepted_progress_count: u64,
    open_obligation_count: u64,
    rejection_count: u64,
    repair_count: u64,
    deterministic_work_units: u64,
    accumulated_ranking_score: i64,
    trajectory_length: u64,
}

fn objective_value(
    descriptor: &SearchObjectiveDescriptor,
    stats: &NodeStats,
) -> EvaluationResult<SearchObjectiveValue> {
    let values = descriptor
        .components
        .iter()
        .map(|component| match component.kind {
            SearchObjectiveComponentKind::CompilerTerminalSuccess => {
                Ok(i64::from(stats.compiler_terminal_success))
            }
            SearchObjectiveComponentKind::TaskCriterionSuccess => Ok(i64::from(stats.task_success)),
            SearchObjectiveComponentKind::AcceptedProgressCount => {
                checked_i64(stats.accepted_progress_count)
            }
            SearchObjectiveComponentKind::OpenObligationCount => {
                checked_i64(stats.open_obligation_count)
            }
            SearchObjectiveComponentKind::RejectionCount => checked_i64(stats.rejection_count),
            SearchObjectiveComponentKind::RepairCount => checked_i64(stats.repair_count),
            SearchObjectiveComponentKind::DeterministicWorkUnits => {
                checked_i64(stats.deterministic_work_units)
            }
            SearchObjectiveComponentKind::AccumulatedRankingScore => {
                Ok(stats.accumulated_ranking_score)
            }
            SearchObjectiveComponentKind::TrajectoryLength => checked_i64(stats.trajectory_length),
        })
        .collect::<EvaluationResult<Vec<_>>>()?;
    Ok(SearchObjectiveValue { values })
}

fn compare_nodes(
    objective: &SearchObjectiveDescriptor,
    left: &SearchNode,
    right: &SearchNode,
) -> Ordering {
    for ((component, left), right) in objective
        .components
        .iter()
        .zip(&left.objective.values)
        .zip(&right.objective.values)
    {
        let ordering = match component.direction {
            ObjectiveDirection::Maximize => right.cmp(left),
            ObjectiveDirection::Minimize => left.cmp(right),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    right
        .accumulated_ranking_score
        .cmp(&left.accumulated_ranking_score)
        .then_with(|| left.compiler_state_hash.cmp(&right.compiler_state_hash))
        .then_with(|| left.id.cmp(&right.id))
}

fn compiler_state_hash(
    objective: &SearchObjectiveDescriptor,
    episode: &crate::model::EvaluationEpisode,
) -> EvaluationResult<String> {
    let accepted = episode
        .steps
        .iter()
        .filter(|step| step.outcome.accepted)
        .map(|step| (&step.decision, &step.outcome))
        .collect::<Vec<_>>();
    domain_hash(
        SEARCH_STATE_HASH_DOMAIN,
        &(
            &objective.initial_anchor_hash,
            compiler_build_hash(),
            accepted,
        ),
    )
}

fn validate_ranker(ranker: &SearchRanker) -> EvaluationResult<()> {
    let descriptor = ranker.descriptor();
    if descriptor.base_interaction_mode != PolicyKind::Menu
        || descriptor.allowed_escape
        || descriptor.tie_break_rule != TieBreakRule::CompilerOrderThenChoiceId
        || descriptor.ranking_policy_hash != crate::ranking::ranking_policy_hash(descriptor)?
    {
        return Err(search_error(
            EvaluationErrorCode::EvaluationSearchUnsupportedSurface,
            "Stage 7A search supports exact menu ranking without escape only",
        ));
    }
    match ranker {
        SearchRanker::Scripted { descriptor } => {
            if matches!(
                descriptor.kind,
                RankingPolicyKind::LearnedLinear | RankingPolicyKind::External
            ) {
                return Err(search_error(
                    EvaluationErrorCode::EvaluationSearchPlanInvalid,
                    "scripted search ranker has an incompatible ranking kind",
                ));
            }
        }
        SearchRanker::Learned {
            descriptor,
            model,
            schema,
        } => {
            if descriptor.kind != RankingPolicyKind::LearnedLinear
                || descriptor.feature_schema_hash != schema.feature_schema_hash
                || model.feature_schema_hash != schema.feature_schema_hash
                || descriptor.opaque_model_identifier.as_deref()
                    != Some(model.learned_model_hash.as_str())
                || model.learned_model_hash != crate::learned::learned_model_hash(model)?
            {
                return Err(search_error(
                    EvaluationErrorCode::EvaluationModelIncompatible,
                    "learned search ranker, model, and schema anchors are incompatible",
                ));
            }
        }
    }
    Ok(())
}

/// Replays a stopped search from its exact root without training or external/device calls.
pub fn replay_search(
    retained: &SearchSession,
    ranker: &SearchRanker,
    limits: &SearchLimits,
) -> EvaluationResult<()> {
    if retained.status == SearchStatus::Running {
        return Err(search_error(
            EvaluationErrorCode::EvaluationSearchIncomplete,
            "running search cannot be published or fully replayed",
        ));
    }
    let mut replayed = SearchSession::start(
        retained.corpus.clone(),
        retained.task_id.clone(),
        retained.objective.clone(),
        retained.plan.clone(),
        ranker,
    )?;
    let cancel_at = if retained.status == SearchStatus::Cancelled {
        Some(retained.work.semantic_expansions)
    } else {
        None
    };
    while replayed.status == SearchStatus::Running {
        if cancel_at == Some(replayed.work.semantic_expansions) {
            replayed.request_cancellation();
        }
        replayed.advance(1, ranker, limits)?;
    }
    if replayed.nodes != retained.nodes
        || replayed.edges != retained.edges
        || replayed.frontier != retained.frontier
        || replayed.trace != retained.trace
        || replayed.result != retained.result
        || replayed.status != retained.status
    {
        return Err(search_error(
            EvaluationErrorCode::EvaluationSearchReplayMismatch,
            "deterministic search replay differs from retained semantic records",
        ));
    }
    Ok(())
}

/// Computes the independent objective descriptor hash.
pub fn search_objective_hash(objective: &SearchObjectiveDescriptor) -> EvaluationResult<String> {
    let mut model = objective.clone();
    model.search_objective_hash.clear();
    model.operational_label = None;
    domain_hash(SEARCH_OBJECTIVE_HASH_DOMAIN, &model)
}

/// Computes the independent search plan hash.
pub fn search_plan_hash(plan: &SearchPlan) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_PLAN_HASH_DOMAIN, plan, |model| {
        model.search_plan_hash.clear();
    })
}

/// Computes the independent search node hash.
pub fn search_node_hash(node: &SearchNode) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_NODE_HASH_DOMAIN, node, |model| {
        model.search_node_hash.clear();
    })
}

/// Computes the independent search edge hash.
pub fn search_edge_hash(edge: &SearchEdge) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_EDGE_HASH_DOMAIN, edge, |model| {
        model.search_edge_hash.clear();
    })
}

/// Computes the independent search checkpoint hash.
pub fn search_checkpoint_hash(checkpoint: &SearchCheckpoint) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_CHECKPOINT_HASH_DOMAIN, checkpoint, |model| {
        model.search_checkpoint_hash.clear();
    })
}

/// Computes the independent search trace hash.
pub fn search_trace_hash(trace: &SearchTrace) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_TRACE_HASH_DOMAIN, trace, |model| {
        model.search_trace_hash.clear();
    })
}

/// Computes the independent search result hash.
pub fn search_result_hash(result: &SearchResult) -> EvaluationResult<String> {
    domain_hash_cleared(SEARCH_RESULT_HASH_DOMAIN, result, |model| {
        model.search_result_hash.clear();
    })
}

/// Constructs an independently hashed bounded search repair.
pub fn search_repair(
    code: SearchRepairCode,
    failing_anchor: String,
) -> EvaluationResult<SearchRepairDescriptor> {
    let mut repair = SearchRepairDescriptor {
        code,
        failing_anchor,
        maximum_actions: 1,
        search_repair_hash: String::new(),
    };
    repair.search_repair_hash = domain_hash_cleared(SEARCH_REPAIR_HASH_DOMAIN, &repair, |model| {
        model.search_repair_hash.clear();
    })?;
    Ok(repair)
}

fn checked_add_u64(left: u64, right: u64) -> EvaluationResult<u64> {
    left.checked_add(right).ok_or_else(search_overflow)
}

fn checked_i64(value: u64) -> EvaluationResult<i64> {
    i64::try_from(value).map_err(|_| search_overflow())
}

fn search_overflow() -> EvaluationDiagnostic {
    search_error(
        EvaluationErrorCode::EvaluationWorkUnitOverflow,
        "checked search integer arithmetic overflow",
    )
}

fn search_error(code: EvaluationErrorCode, message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin_corpus, ranking::feature_schema_v1, ranking::scripted_ranker};

    fn setup(beam: u64, depth: u64) -> EvaluationResult<(SearchSession, SearchRanker)> {
        let corpus = builtin_corpus()?;
        let task = corpus
            .tasks
            .iter()
            .find(|task| task.id.0 == "candidate-rewrite-small")
            .expect("built-in task")
            .clone();
        let schema = feature_schema_v1()?;
        let ranker = SearchRanker::Scripted {
            descriptor: scripted_ranker("goal_directed_rule_v1", &schema, 0)?,
        };
        let objective = SearchObjectiveDescriptor::new(
            &corpus,
            &task,
            vec![
                SearchObjectiveComponent {
                    kind: SearchObjectiveComponentKind::TaskCriterionSuccess,
                    direction: ObjectiveDirection::Maximize,
                },
                SearchObjectiveComponent {
                    kind: SearchObjectiveComponentKind::RejectionCount,
                    direction: ObjectiveDirection::Minimize,
                },
                SearchObjectiveComponent {
                    kind: SearchObjectiveComponentKind::TrajectoryLength,
                    direction: ObjectiveDirection::Minimize,
                },
            ],
        )?;
        let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, beam, depth, 4, 2)?;
        let session = SearchSession::start(corpus, task.id, objective, plan, &ranker)?;
        Ok((session, ranker))
    }

    #[test]
    fn objective_rejects_empty_duplicate_and_wrong_direction() {
        let corpus = builtin_corpus().expect("corpus");
        let task = &corpus.tasks[0];
        assert_eq!(
            SearchObjectiveDescriptor::new(&corpus, task, Vec::new())
                .expect_err("empty objective")
                .code,
            EvaluationErrorCode::EvaluationSearchObjectiveInvalid
        );
        let component = SearchObjectiveComponent {
            kind: SearchObjectiveComponentKind::RejectionCount,
            direction: ObjectiveDirection::Minimize,
        };
        assert!(
            SearchObjectiveDescriptor::new(&corpus, task, vec![component.clone(), component])
                .is_err()
        );
        assert!(
            SearchObjectiveDescriptor::new(
                &corpus,
                task,
                vec![SearchObjectiveComponent {
                    kind: SearchObjectiveComponentKind::RejectionCount,
                    direction: ObjectiveDirection::Maximize,
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn operational_label_does_not_change_objective_hash() {
        let corpus = builtin_corpus().expect("corpus");
        let task = &corpus.tasks[0];
        let mut objective = SearchObjectiveDescriptor::new(
            &corpus,
            task,
            vec![SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::TaskCriterionSuccess,
                direction: ObjectiveDirection::Maximize,
            }],
        )
        .expect("objective");
        let before = objective.search_objective_hash.clone();
        objective.operational_label = Some("local display only".to_owned());
        assert_eq!(before, search_objective_hash(&objective).expect("hash"));
    }

    #[test]
    fn plan_boundaries_and_depth_zero_are_explicit() {
        let (session, ranker) = setup(1, 0).expect("setup");
        assert_eq!(session.status, SearchStatus::Bounded);
        let objective = &session.objective;
        assert!(SearchPlan::deterministic_beam_v1(objective, &ranker, 0, 1, 1, 1).is_err());
        SearchPlan::deterministic_beam_v1(objective, &ranker, MAX_BEAM_WIDTH, 1, 1, 1)
            .expect("exact maximum beam width");
        assert!(
            SearchPlan::deterministic_beam_v1(objective, &ranker, MAX_BEAM_WIDTH + 1, 1, 1, 1)
                .is_err()
        );
    }

    #[test]
    fn single_shot_and_every_unit_resume_are_semantically_identical() {
        let (mut single, ranker) = setup(2, 32).expect("setup");
        let limits = SearchLimits::default();
        single.advance(u64::MAX, &ranker, &limits).expect("single");
        for first_advance in 0..=single.work.semantic_expansions {
            let (mut partitioned, _) = setup(2, 32).expect("partition setup");
            partitioned
                .advance(first_advance, &ranker, &limits)
                .expect("first partition");
            let checkpoint = partitioned
                .checkpoint(&limits)
                .expect("partition checkpoint");
            partitioned =
                SearchSession::resume(&checkpoint, &partitioned.corpus.clone(), &ranker, &limits)
                    .expect("partition resume");
            partitioned
                .advance(u64::MAX, &ranker, &limits)
                .expect("final partition");
            assert_eq!(single.trace, partitioned.trace);
            assert_eq!(single.result, partitioned.result);
        }
        let (mut split, _) = setup(2, 32).expect("setup split");
        while split.status == SearchStatus::Running {
            split.advance(1, &ranker, &limits).expect("advance");
            if split.status == SearchStatus::Running {
                let checkpoint = split.checkpoint(&limits).expect("checkpoint");
                split = SearchSession::resume(&checkpoint, &split.corpus.clone(), &ranker, &limits)
                    .expect("resume");
            }
        }
        assert_eq!(single.nodes, split.nodes);
        assert_eq!(single.edges, split.edges);
        assert_eq!(single.trace, split.trace);
        assert_eq!(single.result, split.result);
        replay_search(&single, &ranker, &limits).expect("replay");
    }

    #[test]
    fn cancellation_and_complete_resume_are_idempotent() {
        let (mut session, ranker) = setup(1, 32).expect("setup");
        session.request_cancellation();
        session
            .advance(1, &ranker, &SearchLimits::default())
            .expect("cancel");
        assert_eq!(session.status, SearchStatus::Cancelled);
        let checkpoint = session
            .checkpoint(&SearchLimits::default())
            .expect("checkpoint");
        let resumed = SearchSession::resume(
            &checkpoint,
            &session.corpus,
            &ranker,
            &SearchLimits::default(),
        )
        .expect("resume");
        assert_eq!(resumed, session);
    }

    #[test]
    fn corrupt_checkpoint_and_frontier_reject_before_execution() {
        let (session, ranker) = setup(1, 32).expect("setup");
        let mut checkpoint = session
            .checkpoint(&SearchLimits::default())
            .expect("checkpoint");
        checkpoint.next_work_cursor += 1;
        assert_eq!(
            SearchSession::resume(
                &checkpoint,
                &session.corpus,
                &ranker,
                &SearchLimits::default()
            )
            .expect_err("corrupt")
            .code,
            EvaluationErrorCode::EvaluationSearchCheckpointCorrupt
        );
        let mut corrupt = session;
        corrupt
            .frontier
            .pending
            .push(SearchNodeId("search-node-missing".to_owned()));
        assert_eq!(
            corrupt.validate_structural().expect_err("frontier").code,
            EvaluationErrorCode::EvaluationSearchFrontierCorrupt
        );
    }

    #[test]
    fn checkpoint_exact_byte_limit_and_plus_one() {
        let (session, _) = setup(1, 32).expect("setup");
        let checkpoint = session
            .checkpoint(&SearchLimits::default())
            .expect("checkpoint");
        let bytes =
            u64::try_from(serde_json::to_vec(&checkpoint).expect("encode").len()).expect("length");
        let mut exact = SearchLimits {
            checkpoint_bytes: bytes,
            ..SearchLimits::default()
        };
        session.checkpoint(&exact).expect("exact limit");
        exact.checkpoint_bytes = bytes - 1;
        assert_eq!(
            session.checkpoint(&exact).expect_err("plus one").code,
            EvaluationErrorCode::EvaluationSearchLimitExceeded
        );
    }
}
