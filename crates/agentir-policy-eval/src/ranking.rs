//! Deterministic Stage 6B ranking over compiler-generated continuation choices.
//!
//! This module has no correctness authority. It canonicalizes the exact visible
//! choice set, validates policy preferences, resolves deterministic ties, and
//! records an independently hashed transcript before a selected production
//! action is submitted to the compiler.

use crate::{
    hashing::{domain_hash, domain_hash_cleared},
    model::{
        CompilerOutcome, EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult,
        EvaluationRun, PolicyKind,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Choice-set hash domain.
pub const CHOICE_SET_HASH_DOMAIN: &[u8] = b"agentir.evaluation.choice_set.v1\0";
/// Stable semantic choice-identity hash domain.
pub const CHOICE_ID_HASH_DOMAIN: &[u8] = b"agentir.evaluation.choice_id.v1\0";
/// Visible feature-schema hash domain.
pub const FEATURE_SCHEMA_HASH_DOMAIN: &[u8] = b"agentir.evaluation.feature_schema.v1\0";
/// Ranking-policy hash domain.
pub const RANKING_POLICY_HASH_DOMAIN: &[u8] = b"agentir.evaluation.ranking_policy.v1\0";
/// Ranking-trace hash domain.
pub const RANKING_TRACE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.ranking_trace.v1\0";
/// Explicit-selection hash domain.
pub const SELECTION_HASH_DOMAIN: &[u8] = b"agentir.evaluation.selection.v1\0";

/// Stable harness-assigned identity for one compiler-generated choice.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvaluationChoiceId(pub String);

/// Compiler layer that generated a choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceOrigin {
    /// SpecIR typed-hole continuation.
    SpecIr,
    /// CandidateForest exact rewrite continuation.
    Candidate,
    /// Equality expansion or materialization continuation.
    Equality,
    /// MemoryIR allocation or reuse continuation.
    Memory,
    /// ScheduleIR transform continuation.
    Schedule,
    /// BackendIR lowering continuation.
    Backend,
    /// Compiler-generated repair continuation.
    Repair,
}

/// Stable semantic category of an available action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceCategory {
    /// Fill a typed SpecIR hole.
    FillHole,
    /// Apply one exact known rewrite.
    ExactRewrite,
    /// Expand an exact equality member.
    EqualityExpand,
    /// Materialize an exact equality member.
    EqualityMaterialize,
    /// Retain conservative fresh storage.
    MemoryFresh,
    /// Apply statically proved storage reuse.
    MemoryStaticReuse,
    /// Apply guarded storage reuse with exact fallback.
    MemoryGuardedReuse,
    /// Apply an exact schedule tile.
    ScheduleTile,
    /// Apply restricted schedule fusion.
    ScheduleFusion,
    /// Select a schedule vector width.
    ScheduleVectorize,
    /// Select a schedule unroll factor.
    ScheduleUnroll,
    /// Retain the conservative serial schedule.
    ScheduleSerial,
    /// Select a supported backend lowering.
    BackendLowering,
    /// Apply a structured compiler repair.
    Repair,
}

/// Typed action carried by a ranked compiler choice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChoiceAction {
    /// Exact production JSON request constructed from compiler-owned descriptors.
    ProductionRequest {
        /// Request passed through `agentir-protocol::Engine` after selection.
        request: Value,
    },
}

impl ChoiceAction {
    /// Returns the exact production request represented by this choice.
    #[must_use]
    pub const fn request(&self) -> &Value {
        match self {
            Self::ProductionRequest { request } => request,
        }
    }
}

/// Visible hard requirements for selecting a compiler-generated action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoicePreconditions {
    /// Exact base identifiers and independent compiler hashes.
    pub bases: BTreeMap<String, String>,
    /// Required target capabilities.
    pub required_capabilities: Vec<String>,
    /// Stable hard-condition descriptions.
    pub hard_conditions: Vec<String>,
}

/// Versioned visible feature value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FeatureValue {
    /// Signed integral value.
    Integer(i64),
    /// Boolean value.
    Boolean(bool),
    /// Stable string or enum value.
    Text(String),
    /// Ordered stable strings.
    TextList(Vec<String>),
}

/// Exact policy-visible features for one choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceVisibleFeatures {
    /// Values keyed by stable feature name.
    pub values: BTreeMap<String, FeatureValue>,
}

/// Feature-schema version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureSchemaVersion(pub u32);

/// Stable primitive type for a visible feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureType {
    /// Signed integral feature.
    Integer,
    /// Boolean feature.
    Boolean,
    /// Stable textual feature.
    Text,
    /// Ordered textual list feature.
    TextList,
}

/// One ordered, visible feature definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureDefinition {
    /// Stable feature name.
    pub name: String,
    /// Feature primitive type.
    pub feature_type: FeatureType,
    /// Visibility contract; Stage 6B uses `policy_visible`.
    pub visibility: String,
    /// Deterministic normalization rule.
    pub normalization: String,
}

/// Versioned ordered visible feature schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSchema {
    /// Schema version.
    pub version: FeatureSchemaVersion,
    /// Ordered exact definitions.
    pub definitions: Vec<FeatureDefinition>,
    /// Independently derived schema hash.
    pub feature_schema_hash: String,
}

/// One compiler-generated ranked action candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationChoice {
    /// Stable choice identity.
    pub id: EvaluationChoiceId,
    /// Compiler layer of origin.
    pub origin: ChoiceOrigin,
    /// Stable action category.
    pub category: ChoiceCategory,
    /// Exact production action.
    pub action: ChoiceAction,
    /// Visible hard requirements.
    pub preconditions: ChoicePreconditions,
    /// Visible deterministic structural features.
    pub visible_features: ChoiceVisibleFeatures,
    /// Compiler-generated explanation.
    pub explanation: String,
    /// Compiler enumeration order, starting at zero.
    pub compiler_order: u64,
    /// Visible proof-frontier or obligation effect; never a policy claim.
    pub proof_effect: String,
}

/// Exact ordered compiler-generated choice frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationChoiceSet {
    /// Observation that exposed the frame.
    pub observation_hash: String,
    /// Feature schema used by every choice.
    pub feature_schema_hash: String,
    /// Ordered choices.
    pub choices: Vec<EvaluationChoice>,
    /// Independent hash of the ordered visible frame and actions.
    pub choice_set_hash: String,
}

/// Deterministic score representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FixedScore {
    /// Signed fixed-point units; higher values rank first.
    pub units: i64,
    /// Decimal scale version. Stage 6B v1 uses one million units.
    pub scale: i64,
}

impl FixedScore {
    /// Fixed-point scale used by the v1 contract.
    pub const SCALE_V1: i64 = 1_000_000;

    /// Constructs one checked v1 score.
    pub fn v1(units: i64, maximum_magnitude: i64) -> EvaluationResult<Self> {
        if units == i64::MIN || units.unsigned_abs() > maximum_magnitude.unsigned_abs() {
            return Err(diagnostic(
                EvaluationErrorCode::EvaluationScoreInvalid,
                "fixed-point score exceeds the configured magnitude",
            )
            .expected_actual(json!(maximum_magnitude), json!(units)));
        }
        Ok(Self {
            units,
            scale: Self::SCALE_V1,
        })
    }
}

/// Deterministic final tie-break contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TieBreakRule {
    /// Higher score, then compiler order, then stable choice identity.
    CompilerOrderThenChoiceId,
}

/// Source of the final explicit selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    /// First choice in validated ranking.
    RankedTopOne,
    /// External policy explicitly selected a visible choice.
    ExplicitOverride,
    /// Bounded hybrid typed escape outside the choice set.
    HybridEscape,
}

/// Kind of ranking policy implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingPolicyKind {
    /// Lexical stable-choice baseline.
    LexicographicChoice,
    /// First compiler-order action that visibly produces progress.
    FirstProgressChoice,
    /// Deterministic task-goal rules.
    GoalDirectedRule,
    /// Prefer visible proof-frontier progress.
    ProofFrontierFirst,
    /// Prefer smaller deterministic context.
    MinContextChoice,
    /// Fixed-seed uniform permutation.
    SeededUniformChoice,
    /// Ranked menu with bounded hybrid escape.
    HybridRankedEscape,
    /// Deterministic offline fixed-point linear model.
    LearnedLinear,
    /// External policy-owned ranking.
    External,
}

/// Stable descriptor for one ranking policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankingPolicyDescriptor {
    /// Stable policy name.
    pub name: String,
    /// Stable policy version.
    pub version: String,
    /// Ranking implementation kind.
    pub kind: RankingPolicyKind,
    /// Underlying Stage 6A interaction mode.
    pub base_interaction_mode: PolicyKind,
    /// Required exact visible feature schema.
    pub feature_schema_hash: String,
    /// Deterministic configuration.
    pub configuration: BTreeMap<String, Value>,
    /// Score representation identifier.
    pub score_representation: String,
    /// Deterministic tie-break rule.
    pub tie_break_rule: TieBreakRule,
    /// Maximum frame size.
    pub maximum_choices: u64,
    /// Whether bounded hybrid escape is permitted.
    pub allowed_escape: bool,
    /// Optional opaque model provenance.
    pub opaque_model_identifier: Option<String>,
    /// Optional prompt/template hash.
    pub prompt_template_hash: Option<String>,
    /// Independent ranking-policy hash.
    pub ranking_policy_hash: String,
}

/// Policy preferences submitted for an exact visible frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RankingDecision {
    /// Exact score for every visible choice.
    Scores {
        /// Score entries; validation rejects missing or duplicate identities.
        scores: Vec<ChoiceScore>,
    },
    /// Explicit visible choice without score invention.
    Select {
        /// Selected choice identity.
        choice_id: EvaluationChoiceId,
    },
    /// Bounded hybrid typed escape outside the compiler-generated frame.
    HybridEscape {
        /// Exact untrusted production request to decode and verify.
        action: Value,
    },
}

/// One submitted score entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceScore {
    /// Visible choice identity.
    pub choice_id: EvaluationChoiceId,
    /// Deterministic fixed-point score.
    pub score: FixedScore,
}

/// One choice in the deterministic final ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedChoice {
    /// Choice identity.
    pub choice_id: EvaluationChoiceId,
    /// Submitted or scripted score, if ranking used scores.
    pub score: Option<FixedScore>,
    /// Zero-based final position.
    pub rank: u64,
    /// Original compiler position.
    pub compiler_order: u64,
}

/// Complete replayable policy-owned ranking record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankingTrace {
    /// Observation anchor.
    pub observation_hash: String,
    /// Exact choice-set anchor.
    pub choice_set_hash: String,
    /// Visible schema anchor.
    pub feature_schema_hash: String,
    /// Ranking-policy anchor.
    pub ranking_policy_hash: String,
    /// Exact submitted preferences.
    pub decision: RankingDecision,
    /// Applied tie-break contract.
    pub tie_break_rule: TieBreakRule,
    /// Final deterministic order.
    pub ranking: Vec<RankedChoice>,
    /// Explicit selected visible choice.
    pub selected_choice: Option<EvaluationChoiceId>,
    /// Selection source.
    pub selection_source: SelectionSource,
    /// Independent ranking trace hash.
    pub ranking_trace_hash: String,
}

/// Explicit compiler dispatch outcome after ranking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionDecision {
    /// Exact choice-set anchor, absent only for bounded hybrid escape.
    pub choice_set_hash: Option<String>,
    /// Selected visible choice, absent for escape.
    pub selected_choice: Option<EvaluationChoiceId>,
    /// Exact escaped action, present only for hybrid escape.
    pub escape_action: Option<Value>,
    /// Selection source.
    pub source: SelectionSource,
}

/// Replayable selection plus production compiler outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionOutcome {
    /// Explicit selection.
    pub decision: SelectionDecision,
    /// Outcome returned by the production verifier/transaction path.
    pub compiler_outcome: CompilerOutcome,
    /// Independent selection hash.
    pub selection_hash: String,
}

/// Raw ranking-specific metrics without a weighted overall score.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RankingMetrics {
    /// Number of ranked frames.
    pub choice_set_count: u64,
    /// Mean frame size.
    pub mean_choice_set_size: f64,
    /// Median frame size.
    pub median_choice_set_size: u64,
    /// P95 frame size.
    pub p95_choice_set_size: u64,
    /// Accepted selected visible actions.
    pub top1_accepted: u64,
    /// Accepted selected visible action rate.
    pub top1_accepted_rate: f64,
    /// Progress-producing selected visible actions.
    pub top1_progress_producing: u64,
    /// Progress-producing selected visible action rate.
    pub top1_progress_producing_rate: f64,
    /// Median zero-based compiler-order position selected.
    pub median_selected_position: u64,
    /// P95 zero-based compiler-order position selected.
    pub p95_selected_position: u64,
    /// Number of score ties.
    pub tie_count: u64,
    /// Fraction of ranked frames containing a tie.
    pub tie_frequency: f64,
    /// Number of equal-score collisions.
    pub score_collision_count: u64,
    /// Fraction of ranked frames containing equal scores.
    pub score_collision_frequency: f64,
    /// Explicit choice overrides.
    pub explicit_overrides: u64,
    /// Explicit override rate.
    pub explicit_override_rate: f64,
    /// Hybrid escapes.
    pub hybrid_escapes: u64,
    /// Hybrid escape rate.
    pub hybrid_escape_rate: f64,
    /// Accepted hybrid escapes.
    pub accepted_hybrid_escapes: u64,
    /// Accepted escape rate among escapes.
    pub accepted_escape_rate: f64,
    /// Rejected ranked selections.
    pub ranked_rejections: u64,
    /// Rejection rate after ranked selection.
    pub rejection_rate_after_ranked_selection: f64,
    /// Repair cycles observed after ranked selections.
    pub repair_cycles_after_ranked_selection: u64,
    /// Mean deterministic observation context bytes per visible choice.
    pub context_bytes_per_choice: f64,
    /// Tokens reported per ranked decision when known.
    pub tokens_per_ranked_decision: Option<f64>,
    /// Completed episode rate.
    pub completion_rate: f64,
    /// Budget exhaustion rate.
    pub budget_exhaustion_rate: f64,
    /// Compiler-owned task-criterion rate.
    pub semantic_correctness_rate: f64,
}

/// Limits specific to the non-correctness ranking layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankingLimits {
    /// Choices per frame.
    pub choices_per_frame: u64,
    /// Bytes per choice.
    pub choice_bytes: u64,
    /// Bytes per exact choice set.
    pub choice_set_bytes: u64,
    /// Feature definitions.
    pub feature_definitions: u64,
    /// Features per choice.
    pub features_per_choice: u64,
    /// Score entries.
    pub score_entries: u64,
    /// Maximum absolute fixed-point score.
    pub score_magnitude: i64,
    /// Ranking traces per run/archive.
    pub ranking_traces: u64,
    /// Ranking transcript bytes.
    pub ranking_transcript_bytes: u64,
}

impl Default for RankingLimits {
    fn default() -> Self {
        Self {
            choices_per_frame: 1_000,
            choice_bytes: 1024 * 1024,
            choice_set_bytes: 16 * 1024 * 1024,
            feature_definitions: 256,
            features_per_choice: 256,
            score_entries: 1_000,
            score_magnitude: 1_000_000_000_000,
            ranking_traces: 100_000,
            ranking_transcript_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Returns the exact Stage 6B visible feature schema v1.
pub fn feature_schema_v1() -> EvaluationResult<FeatureSchema> {
    let definitions = [
        ("action_category", FeatureType::Text, "stable_enum"),
        ("compiler_layer", FeatureType::Text, "stable_enum"),
        ("structural_target", FeatureType::Text, "compiler_locator"),
        ("operand_arity", FeatureType::Integer, "non_negative_i64"),
        ("result_arity", FeatureType::Integer, "non_negative_i64"),
        ("visible_type_class", FeatureType::Text, "stable_enum"),
        (
            "tensor_rank",
            FeatureType::Integer,
            "negative_one_if_not_tensor",
        ),
        (
            "static_extent_count",
            FeatureType::Integer,
            "non_negative_i64",
        ),
        (
            "symbolic_extent_count",
            FeatureType::Integer,
            "non_negative_i64",
        ),
        ("open_obligation_delta", FeatureType::Integer, "signed_i64"),
        ("proof_frontier_effect", FeatureType::Text, "stable_enum"),
        (
            "estimated_action_bytes",
            FeatureType::Integer,
            "canonical_json_bytes",
        ),
        (
            "continuation_depth",
            FeatureType::Integer,
            "non_negative_i64",
        ),
        ("fallback_required", FeatureType::Boolean, "boolean"),
        ("escape_used", FeatureType::Boolean, "boolean"),
        (
            "target_capabilities",
            FeatureType::TextList,
            "sorted_unique",
        ),
        (
            "analytical_resource_estimate",
            FeatureType::Text,
            "visible_canonical_json",
        ),
    ]
    .into_iter()
    .map(|(name, feature_type, normalization)| FeatureDefinition {
        name: name.to_owned(),
        feature_type,
        visibility: "policy_visible".to_owned(),
        normalization: normalization.to_owned(),
    })
    .collect::<Vec<_>>();
    let mut schema = FeatureSchema {
        version: FeatureSchemaVersion(1),
        definitions,
        feature_schema_hash: String::new(),
    };
    schema.feature_schema_hash = feature_schema_hash(&schema)?;
    Ok(schema)
}

/// Computes the feature-schema hash without trusting the stored hash.
pub fn feature_schema_hash(schema: &FeatureSchema) -> EvaluationResult<String> {
    domain_hash_cleared(FEATURE_SCHEMA_HASH_DOMAIN, schema, |model| {
        model.feature_schema_hash.clear();
    })
}

/// Computes the exact ordered choice-set hash.
pub fn choice_set_hash(choice_set: &EvaluationChoiceSet) -> EvaluationResult<String> {
    domain_hash_cleared(CHOICE_SET_HASH_DOMAIN, choice_set, |model| {
        model.choice_set_hash.clear();
    })
}

/// Computes the ranking-policy hash.
pub fn ranking_policy_hash(policy: &RankingPolicyDescriptor) -> EvaluationResult<String> {
    domain_hash_cleared(RANKING_POLICY_HASH_DOMAIN, policy, |model| {
        model.ranking_policy_hash.clear();
    })
}

/// Computes the ranking-trace hash.
pub fn ranking_trace_hash(trace: &RankingTrace) -> EvaluationResult<String> {
    domain_hash_cleared(RANKING_TRACE_HASH_DOMAIN, trace, |model| {
        model.ranking_trace_hash.clear();
    })
}

/// Computes the explicit selection hash.
pub fn selection_hash(selection: &SelectionOutcome) -> EvaluationResult<String> {
    domain_hash_cleared(SELECTION_HASH_DOMAIN, selection, |model| {
        model.selection_hash.clear();
    })
}

/// Constructs a checked exact ordered choice set and assigns stable identities.
pub fn build_choice_set(
    observation_hash: impl Into<String>,
    schema: &FeatureSchema,
    mut choices: Vec<EvaluationChoice>,
    limits: &RankingLimits,
) -> EvaluationResult<EvaluationChoiceSet> {
    validate_schema(schema, limits)?;
    limit(
        choices.len(),
        limits.choices_per_frame,
        EvaluationErrorCode::EvaluationRankingBudgetExceeded,
        "choices_per_frame",
    )?;
    for (index, choice) in choices.iter_mut().enumerate() {
        choice.compiler_order = u64::try_from(index).unwrap_or(u64::MAX);
        choice.id = stable_choice_id(choice)?;
        validate_choice_features(choice, schema)?;
        limit(
            choice.visible_features.values.len(),
            limits.features_per_choice,
            EvaluationErrorCode::EvaluationRankingBudgetExceeded,
            "features_per_choice",
        )?;
        let bytes = serde_json::to_vec(choice).map_err(|error| encoding_error(&error))?;
        limit(
            bytes.len(),
            limits.choice_bytes,
            EvaluationErrorCode::EvaluationRankingBudgetExceeded,
            "choice_bytes",
        )?;
    }
    let mut ids = BTreeSet::new();
    if choices.iter().any(|choice| !ids.insert(choice.id.clone())) {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationChoiceSetMismatch,
            "stable choice identities are not unique in the exact frame",
        ));
    }
    let mut set = EvaluationChoiceSet {
        observation_hash: observation_hash.into(),
        feature_schema_hash: schema.feature_schema_hash.clone(),
        choices,
        choice_set_hash: String::new(),
    };
    let bytes = serde_json::to_vec(&set).map_err(|error| encoding_error(&error))?;
    limit(
        bytes.len(),
        limits.choice_set_bytes,
        EvaluationErrorCode::EvaluationRankingBudgetExceeded,
        "choice_set_bytes",
    )?;
    set.choice_set_hash = choice_set_hash(&set)?;
    Ok(set)
}

fn stable_choice_id(choice: &EvaluationChoice) -> EvaluationResult<EvaluationChoiceId> {
    let action = semantic_choice_action(&choice.action);
    let identity = (
        choice.origin,
        choice.category,
        action,
        &choice.preconditions,
    );
    let hash = domain_hash(CHOICE_ID_HASH_DOMAIN, &identity)?;
    Ok(EvaluationChoiceId(format!("ec{}", &hash[..24])))
}

fn semantic_choice_action(action: &ChoiceAction) -> ChoiceAction {
    match action {
        ChoiceAction::ProductionRequest { request } => ChoiceAction::ProductionRequest {
            request: strip_transport_metadata(request),
        },
    }
}

fn strip_transport_metadata(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(name, _)| !is_transport_metadata(name))
                .map(|(name, value)| (name.clone(), strip_transport_metadata(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(strip_transport_metadata).collect()),
        _ => value.clone(),
    }
}

fn is_transport_metadata(name: &str) -> bool {
    matches!(
        name,
        "request_id"
            | "correlation_id"
            | "external_correlation_id"
            | "timestamp"
            | "timestamp_ns"
            | "latency"
            | "latency_ns"
            | "latency_ms"
            | "hostname"
            | "provider_session"
            | "provider_session_id"
    )
}

fn validate_schema(schema: &FeatureSchema, limits: &RankingLimits) -> EvaluationResult<()> {
    limit(
        schema.definitions.len(),
        limits.feature_definitions,
        EvaluationErrorCode::EvaluationRankingBudgetExceeded,
        "feature_definitions",
    )?;
    if schema.version != FeatureSchemaVersion(1)
        || schema.feature_schema_hash != feature_schema_hash(schema)?
        || schema != &feature_schema_v1()?
    {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
            "visible feature schema identity is invalid",
        ));
    }
    let mut names = BTreeSet::new();
    if schema
        .definitions
        .iter()
        .any(|definition| !names.insert(&definition.name))
    {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
            "visible feature names must be unique",
        ));
    }
    Ok(())
}

fn validate_choice_features(
    choice: &EvaluationChoice,
    schema: &FeatureSchema,
) -> EvaluationResult<()> {
    if choice.visible_features.values.len() != schema.definitions.len() {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
            "choice features do not exactly match the visible schema",
        )
        .expected_actual(
            json!(schema.definitions.len()),
            json!(choice.visible_features.values.len()),
        ));
    }
    for definition in &schema.definitions {
        let value = choice
            .visible_features
            .values
            .get(&definition.name)
            .ok_or_else(|| {
                diagnostic(
                    EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
                    "choice is missing a required visible feature",
                )
                .expected_actual(json!(definition.name), Value::Null)
            })?;
        let type_matches = matches!(
            (definition.feature_type, value),
            (FeatureType::Integer, FeatureValue::Integer(_))
                | (FeatureType::Boolean, FeatureValue::Boolean(_))
                | (FeatureType::Text, FeatureValue::Text(_))
                | (FeatureType::TextList, FeatureValue::TextList(_))
        );
        if !type_matches {
            return Err(diagnostic(
                EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
                "visible feature value has the wrong primitive type",
            )
            .expected_actual(json!(definition.feature_type), json!(value)));
        }
        let normalized = match (definition.normalization.as_str(), value) {
            ("non_negative_i64" | "canonical_json_bytes", FeatureValue::Integer(value)) => {
                *value >= 0
            }
            ("negative_one_if_not_tensor", FeatureValue::Integer(value)) => *value >= -1,
            ("sorted_unique", FeatureValue::TextList(values)) => {
                values.windows(2).all(|pair| pair[0] < pair[1])
            }
            (
                "signed_i64"
                | "stable_enum"
                | "compiler_locator"
                | "boolean"
                | "visible_canonical_json",
                _,
            ) => true,
            _ => false,
        };
        if !normalized {
            return Err(diagnostic(
                EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
                "visible feature violates its declared normalization",
            )
            .expected_actual(json!(definition.normalization), json!(value)));
        }
    }
    Ok(())
}

/// Returns one of the deterministic scripted Stage 6B ranking descriptors.
pub fn scripted_ranker(
    name: &str,
    schema: &FeatureSchema,
    seed: u64,
) -> EvaluationResult<RankingPolicyDescriptor> {
    let (kind, base, escape) = match name {
        "lexicographic_choice_v1" => (
            RankingPolicyKind::LexicographicChoice,
            PolicyKind::Menu,
            false,
        ),
        "first_progress_choice_v1" => (
            RankingPolicyKind::FirstProgressChoice,
            PolicyKind::Menu,
            false,
        ),
        "goal_directed_rule_v1" => (RankingPolicyKind::GoalDirectedRule, PolicyKind::Menu, false),
        "proof_frontier_first_v1" => (
            RankingPolicyKind::ProofFrontierFirst,
            PolicyKind::Menu,
            false,
        ),
        "min_context_choice_v1" => (RankingPolicyKind::MinContextChoice, PolicyKind::Menu, false),
        "seeded_uniform_choice_v1" => (
            RankingPolicyKind::SeededUniformChoice,
            PolicyKind::Menu,
            false,
        ),
        "hybrid_ranked_escape_v1" => (
            RankingPolicyKind::HybridRankedEscape,
            PolicyKind::Hybrid,
            true,
        ),
        _ => {
            return Err(diagnostic(
                EvaluationErrorCode::EvaluationRankingPolicyNotFound,
                "ranking policy does not exist",
            ));
        }
    };
    let mut descriptor = RankingPolicyDescriptor {
        name: name.to_owned(),
        version: "1".to_owned(),
        kind,
        base_interaction_mode: base,
        feature_schema_hash: schema.feature_schema_hash.clone(),
        configuration: BTreeMap::from([("seed".to_owned(), json!(seed))]),
        score_representation: "signed_fixed_point_i64_scale_1000000_v1".to_owned(),
        tie_break_rule: TieBreakRule::CompilerOrderThenChoiceId,
        maximum_choices: 1_000,
        allowed_escape: escape,
        opaque_model_identifier: None,
        prompt_template_hash: None,
        ranking_policy_hash: String::new(),
    };
    descriptor.ranking_policy_hash = ranking_policy_hash(&descriptor)?;
    Ok(descriptor)
}

/// Produces deterministic scripted scores for CI without claiming learned behavior.
pub fn scripted_ranking_decision(
    policy: &RankingPolicyDescriptor,
    choice_set: &EvaluationChoiceSet,
    limits: &RankingLimits,
) -> EvaluationResult<RankingDecision> {
    validate_policy_for_set(policy, choice_set)?;
    let seed = policy
        .configuration
        .get("seed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut scores = Vec::with_capacity(choice_set.choices.len());
    for choice in &choice_set.choices {
        let order = i64::try_from(choice.compiler_order).unwrap_or(i64::MAX);
        let action_bytes = feature_integer(choice, "estimated_action_bytes").unwrap_or(0);
        let proof = match feature_text(choice, "proof_frontier_effect") {
            Some("advance") => 1_i64,
            _ => 0,
        };
        let raw_units = match policy.kind {
            RankingPolicyKind::LexicographicChoice => lexical_score(&choice.id.0),
            RankingPolicyKind::FirstProgressChoice => -order,
            RankingPolicyKind::GoalDirectedRule => category_priority(choice.category) - order,
            RankingPolicyKind::ProofFrontierFirst => {
                proof.saturating_mul(1_000_000).saturating_sub(order)
            }
            RankingPolicyKind::MinContextChoice => action_bytes.saturating_neg(),
            RankingPolicyKind::SeededUniformChoice | RankingPolicyKind::HybridRankedEscape => {
                seeded_score(seed, &choice.id.0)
            }
            RankingPolicyKind::LearnedLinear | RankingPolicyKind::External => {
                return Err(diagnostic(
                    EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                    "learned and external rankers must submit their own preferences",
                ));
            }
        };
        let units = if limits.score_magnitude > 0 {
            raw_units % limits.score_magnitude
        } else {
            raw_units
        };
        scores.push(ChoiceScore {
            choice_id: choice.id.clone(),
            score: FixedScore::v1(units, limits.score_magnitude)?,
        });
    }
    Ok(RankingDecision::Scores { scores })
}

/// Validates preferences, applies deterministic tie-breaks, and hashes the trace.
pub fn rank_choices(
    choice_set: &EvaluationChoiceSet,
    policy: &RankingPolicyDescriptor,
    decision: RankingDecision,
    limits: &RankingLimits,
) -> EvaluationResult<RankingTrace> {
    validate_choice_set(choice_set)?;
    validate_policy_for_set(policy, choice_set)?;
    limit(
        choice_set.choices.len(),
        policy.maximum_choices,
        EvaluationErrorCode::EvaluationRankingBudgetExceeded,
        "ranking_policy_maximum_choices",
    )?;
    let (mut ranking, selected, source) = match &decision {
        RankingDecision::Scores { scores } => {
            limit(
                scores.len(),
                limits.score_entries,
                EvaluationErrorCode::EvaluationRankingBudgetExceeded,
                "score_entries",
            )?;
            if scores.len() != choice_set.choices.len() {
                return Err(diagnostic(
                    EvaluationErrorCode::EvaluationScoreCountMismatch,
                    "ranking must score every exact visible choice",
                )
                .expected_actual(json!(choice_set.choices.len()), json!(scores.len())));
            }
            let mut by_id = BTreeMap::new();
            for entry in scores {
                if entry.score.scale != FixedScore::SCALE_V1
                    || entry.score.units == i64::MIN
                    || entry.score.units.unsigned_abs() > limits.score_magnitude.unsigned_abs()
                {
                    return Err(diagnostic(
                        EvaluationErrorCode::EvaluationScoreInvalid,
                        "invalid fixed-point score representation or magnitude",
                    ));
                }
                if by_id.insert(entry.choice_id.clone(), entry.score).is_some() {
                    return Err(diagnostic(
                        EvaluationErrorCode::EvaluationScoreCountMismatch,
                        "duplicated score entry",
                    ));
                }
            }
            let mut ranked = choice_set
                .choices
                .iter()
                .map(|choice| {
                    by_id
                        .remove(&choice.id)
                        .map(|score| RankedChoice {
                            choice_id: choice.id.clone(),
                            score: Some(score),
                            rank: 0,
                            compiler_order: choice.compiler_order,
                        })
                        .ok_or_else(|| {
                            diagnostic(
                                EvaluationErrorCode::EvaluationChoiceNotFound,
                                "score entry is missing for a visible choice",
                            )
                        })
                })
                .collect::<EvaluationResult<Vec<_>>>()?;
            if !by_id.is_empty() {
                return Err(diagnostic(
                    EvaluationErrorCode::EvaluationSelectionNotInChoiceSet,
                    "score references a choice outside the exact frame",
                ));
            }
            ranked.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| left.compiler_order.cmp(&right.compiler_order))
                    .then_with(|| left.choice_id.cmp(&right.choice_id))
            });
            let selected = ranked
                .first()
                .map(|choice| choice.choice_id.clone())
                .ok_or_else(|| {
                    diagnostic(
                        EvaluationErrorCode::EvaluationSelectionInvalid,
                        "cannot rank an empty choice set",
                    )
                })?;
            (ranked, Some(selected), SelectionSource::RankedTopOne)
        }
        RankingDecision::Select { choice_id } => {
            let choice = choice_set
                .choices
                .iter()
                .find(|choice| &choice.id == choice_id)
                .ok_or_else(|| {
                    diagnostic(
                        EvaluationErrorCode::EvaluationSelectionNotInChoiceSet,
                        "explicit selection is outside the exact visible frame",
                    )
                })?;
            (
                vec![RankedChoice {
                    choice_id: choice.id.clone(),
                    score: None,
                    rank: 0,
                    compiler_order: choice.compiler_order,
                }],
                Some(choice.id.clone()),
                SelectionSource::ExplicitOverride,
            )
        }
        RankingDecision::HybridEscape { .. } => {
            if !policy.allowed_escape || policy.base_interaction_mode != PolicyKind::Hybrid {
                return Err(diagnostic(
                    EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                    "ranking policy does not permit hybrid escape",
                ));
            }
            (Vec::new(), None, SelectionSource::HybridEscape)
        }
    };
    for (index, choice) in ranking.iter_mut().enumerate() {
        choice.rank = u64::try_from(index).unwrap_or(u64::MAX);
    }
    let mut trace = RankingTrace {
        observation_hash: choice_set.observation_hash.clone(),
        choice_set_hash: choice_set.choice_set_hash.clone(),
        feature_schema_hash: choice_set.feature_schema_hash.clone(),
        ranking_policy_hash: policy.ranking_policy_hash.clone(),
        decision,
        tie_break_rule: policy.tie_break_rule,
        ranking,
        selected_choice: selected,
        selection_source: source,
        ranking_trace_hash: String::new(),
    };
    let bytes = serde_json::to_vec(&trace).map_err(|error| encoding_error(&error))?;
    limit(
        bytes.len(),
        limits.ranking_transcript_bytes,
        EvaluationErrorCode::EvaluationRankingBudgetExceeded,
        "ranking_transcript_bytes",
    )?;
    trace.ranking_trace_hash = ranking_trace_hash(&trace)?;
    Ok(trace)
}

/// Verifies a ranking trace by deterministic recomputation without a policy call.
pub fn replay_ranking_trace(
    choice_set: &EvaluationChoiceSet,
    policy: &RankingPolicyDescriptor,
    trace: &RankingTrace,
    limits: &RankingLimits,
) -> EvaluationResult<()> {
    let rebuilt = rank_choices(choice_set, policy, trace.decision.clone(), limits)?;
    if &rebuilt != trace {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationRankingReplayMismatch,
            "ranking trace differs from deterministic replay",
        )
        .expected_actual(json!(rebuilt), json!(trace)));
    }
    Ok(())
}

/// Recomputes raw ranking metrics from exact episode transcripts.
#[must_use]
pub fn aggregate_ranking_metrics(run: &EvaluationRun) -> RankingMetrics {
    let traces = run
        .episodes
        .iter()
        .flat_map(|episode| episode.steps.iter())
        .filter_map(|step| step.ranking_trace.as_ref().map(|trace| (step, trace)))
        .collect::<Vec<_>>();
    let frame_count = u64::try_from(traces.len()).unwrap_or(u64::MAX);
    let mut frame_sizes = Vec::new();
    let mut selected_positions = Vec::new();
    let mut accepted = 0_u64;
    let mut progress = 0_u64;
    let mut ties = 0_u64;
    let mut collisions = 0_u64;
    let mut overrides = 0_u64;
    let mut escapes = 0_u64;
    let mut accepted_escapes = 0_u64;
    let mut rejected = 0_u64;
    let mut total_context = 0_u64;
    let mut total_choices = 0_u64;
    let mut token_total = 0_u64;
    let mut tokens_known = true;
    for (step, trace) in traces {
        let size = u64::try_from(step.observation.continuation_frame.len()).unwrap_or(u64::MAX);
        frame_sizes.push(size);
        total_choices = total_choices.saturating_add(size);
        total_context = total_context.saturating_add(step.context.observation_bytes);
        selected_positions.push(
            trace
                .ranking
                .iter()
                .find(|choice| Some(&choice.choice_id) == trace.selected_choice.as_ref())
                .map_or(0, |choice| choice.compiler_order),
        );
        accepted = accepted.saturating_add(u64::from(step.outcome.accepted));
        progress = progress.saturating_add(u64::from(step.outcome.progress_producing));
        rejected = rejected.saturating_add(u64::from(!step.outcome.accepted));
        overrides = overrides.saturating_add(u64::from(matches!(
            trace.selection_source,
            SelectionSource::ExplicitOverride
        )));
        escapes = escapes.saturating_add(u64::from(matches!(
            trace.selection_source,
            SelectionSource::HybridEscape
        )));
        accepted_escapes = accepted_escapes.saturating_add(u64::from(
            matches!(trace.selection_source, SelectionSource::HybridEscape)
                && step.outcome.accepted,
        ));
        if let RankingDecision::Scores { scores } = &trace.decision {
            let distinct = scores
                .iter()
                .map(|entry| entry.score)
                .collect::<BTreeSet<_>>()
                .len();
            if distinct < scores.len() {
                ties = ties.saturating_add(1);
                collisions = collisions.saturating_add(
                    u64::try_from(scores.len().saturating_sub(distinct)).unwrap_or(u64::MAX),
                );
            }
        }
        match step.context.deterministic_tokens {
            Some(tokens) => token_total = token_total.saturating_add(tokens),
            None => tokens_known = false,
        }
    }
    let episodes = u64::try_from(run.episodes.len()).unwrap_or(u64::MAX);
    let completed = u64::try_from(
        run.episodes
            .iter()
            .filter(|episode| episode.result.is_some())
            .count(),
    )
    .unwrap_or(u64::MAX);
    let exhausted = u64::try_from(
        run.episodes
            .iter()
            .filter(|episode| {
                episode
                    .result
                    .as_ref()
                    .is_some_and(|result| result.budget_exhausted)
            })
            .count(),
    )
    .unwrap_or(u64::MAX);
    let correct = u64::try_from(
        run.episodes
            .iter()
            .filter(|episode| {
                episode
                    .result
                    .as_ref()
                    .is_some_and(|result| result.semantic.criterion_satisfied)
            })
            .count(),
    )
    .unwrap_or(u64::MAX);
    let repair_cycles = run.episodes.iter().fold(0_u64, |total, episode| {
        total.saturating_add(episode.result.as_ref().map_or(0, |result| {
            u64::try_from(result.repair_cycles.len()).unwrap_or(u64::MAX)
        }))
    });
    RankingMetrics {
        choice_set_count: frame_count,
        mean_choice_set_size: ratio(total_choices, frame_count),
        median_choice_set_size: percentile(&frame_sizes, 50),
        p95_choice_set_size: percentile(&frame_sizes, 95),
        top1_accepted: accepted,
        top1_accepted_rate: ratio(accepted, frame_count),
        top1_progress_producing: progress,
        top1_progress_producing_rate: ratio(progress, frame_count),
        median_selected_position: percentile(&selected_positions, 50),
        p95_selected_position: percentile(&selected_positions, 95),
        tie_count: ties,
        tie_frequency: ratio(ties, frame_count),
        score_collision_count: collisions,
        score_collision_frequency: ratio(collisions, frame_count),
        explicit_overrides: overrides,
        explicit_override_rate: ratio(overrides, frame_count),
        hybrid_escapes: escapes,
        hybrid_escape_rate: ratio(escapes, frame_count),
        accepted_hybrid_escapes: accepted_escapes,
        accepted_escape_rate: ratio(accepted_escapes, escapes),
        ranked_rejections: rejected,
        rejection_rate_after_ranked_selection: ratio(rejected, frame_count),
        repair_cycles_after_ranked_selection: repair_cycles,
        context_bytes_per_choice: ratio(total_context, total_choices),
        tokens_per_ranked_decision: tokens_known.then(|| ratio(token_total, frame_count)),
        completion_rate: ratio(completed, episodes),
        budget_exhaustion_rate: ratio(exhausted, episodes),
        semantic_correctness_rate: ratio(correct, episodes),
    }
}

/// Creates and hashes the selection record after production compiler execution.
pub fn record_selection(
    choice_set: &EvaluationChoiceSet,
    trace: &RankingTrace,
    compiler_outcome: CompilerOutcome,
) -> EvaluationResult<SelectionOutcome> {
    if trace.choice_set_hash != choice_set.choice_set_hash
        || (trace.selection_source != SelectionSource::HybridEscape
            && !choice_set
                .choices
                .iter()
                .any(|choice| Some(&choice.id) == trace.selected_choice.as_ref()))
    {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationSelectionInvalid,
            "ranking selection is not anchored to the exact choice set",
        ));
    }
    let mut outcome = SelectionOutcome {
        decision: SelectionDecision {
            choice_set_hash: Some(choice_set.choice_set_hash.clone()),
            selected_choice: trace.selected_choice.clone(),
            escape_action: match &trace.decision {
                RankingDecision::HybridEscape { action } => Some(action.clone()),
                _ => None,
            },
            source: trace.selection_source,
        },
        compiler_outcome,
        selection_hash: String::new(),
    };
    outcome.selection_hash = selection_hash(&outcome)?;
    Ok(outcome)
}

/// Constructs a choice from a compiler-owned descriptor and production request.
pub fn compiler_choice(
    origin: ChoiceOrigin,
    category: ChoiceCategory,
    request: Value,
    preconditions: ChoicePreconditions,
    explanation: impl Into<String>,
    proof_effect: impl Into<String>,
    structural_target: impl Into<String>,
) -> EvaluationResult<EvaluationChoice> {
    let action_bytes = i64::try_from(
        serde_json::to_vec(&request)
            .map_err(|error| encoding_error(&error))?
            .len(),
    )
    .unwrap_or(i64::MAX);
    let category_name = serde_json::to_value(category)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    let origin_name = serde_json::to_value(origin)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    let proof_effect = proof_effect.into();
    Ok(EvaluationChoice {
        id: EvaluationChoiceId(String::new()),
        origin,
        category,
        action: ChoiceAction::ProductionRequest { request },
        preconditions,
        visible_features: ChoiceVisibleFeatures {
            values: BTreeMap::from([
                (
                    "action_category".to_owned(),
                    FeatureValue::Text(category_name),
                ),
                ("compiler_layer".to_owned(), FeatureValue::Text(origin_name)),
                (
                    "structural_target".to_owned(),
                    FeatureValue::Text(structural_target.into()),
                ),
                ("operand_arity".to_owned(), FeatureValue::Integer(0)),
                ("result_arity".to_owned(), FeatureValue::Integer(0)),
                (
                    "visible_type_class".to_owned(),
                    FeatureValue::Text("unknown".to_owned()),
                ),
                ("tensor_rank".to_owned(), FeatureValue::Integer(-1)),
                ("static_extent_count".to_owned(), FeatureValue::Integer(0)),
                ("symbolic_extent_count".to_owned(), FeatureValue::Integer(0)),
                ("open_obligation_delta".to_owned(), FeatureValue::Integer(0)),
                (
                    "proof_frontier_effect".to_owned(),
                    FeatureValue::Text(proof_effect.clone()),
                ),
                (
                    "estimated_action_bytes".to_owned(),
                    FeatureValue::Integer(action_bytes),
                ),
                ("continuation_depth".to_owned(), FeatureValue::Integer(0)),
                ("fallback_required".to_owned(), FeatureValue::Boolean(false)),
                ("escape_used".to_owned(), FeatureValue::Boolean(false)),
                (
                    "target_capabilities".to_owned(),
                    FeatureValue::TextList(Vec::new()),
                ),
                (
                    "analytical_resource_estimate".to_owned(),
                    FeatureValue::Text("none".to_owned()),
                ),
            ]),
        },
        explanation: explanation.into(),
        compiler_order: 0,
        proof_effect,
    })
}

fn validate_choice_set(choice_set: &EvaluationChoiceSet) -> EvaluationResult<()> {
    if choice_set.choice_set_hash != choice_set_hash(choice_set)? {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationChoiceSetMismatch,
            "choice-set hash does not match the exact ordered choices",
        ));
    }
    for (index, choice) in choice_set.choices.iter().enumerate() {
        if choice.compiler_order != u64::try_from(index).unwrap_or(u64::MAX)
            || choice.id != stable_choice_id(choice)?
        {
            return Err(diagnostic(
                EvaluationErrorCode::EvaluationChoiceSetMismatch,
                "choice order or stable identity is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_policy_for_set(
    policy: &RankingPolicyDescriptor,
    choice_set: &EvaluationChoiceSet,
) -> EvaluationResult<()> {
    if policy.ranking_policy_hash != ranking_policy_hash(policy)?
        || policy.feature_schema_hash != choice_set.feature_schema_hash
        || policy.score_representation != "signed_fixed_point_i64_scale_1000000_v1"
        || policy.tie_break_rule != TieBreakRule::CompilerOrderThenChoiceId
    {
        return Err(diagnostic(
            EvaluationErrorCode::EvaluationRankingPolicyInvalid,
            "ranking policy descriptor is incompatible with the exact visible frame",
        ));
    }
    Ok(())
}

fn feature_integer(choice: &EvaluationChoice, name: &str) -> Option<i64> {
    match choice.visible_features.values.get(name) {
        Some(FeatureValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

fn feature_text<'a>(choice: &'a EvaluationChoice, name: &str) -> Option<&'a str> {
    match choice.visible_features.values.get(name) {
        Some(FeatureValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn category_priority(category: ChoiceCategory) -> i64 {
    match category {
        ChoiceCategory::ExactRewrite | ChoiceCategory::EqualityMaterialize => 9_000_000,
        ChoiceCategory::MemoryStaticReuse | ChoiceCategory::ScheduleFusion => 8_000_000,
        ChoiceCategory::ScheduleTile | ChoiceCategory::ScheduleVectorize => 7_000_000,
        ChoiceCategory::MemoryGuardedReuse | ChoiceCategory::ScheduleUnroll => 6_000_000,
        ChoiceCategory::FillHole | ChoiceCategory::BackendLowering | ChoiceCategory::Repair => {
            5_000_000
        }
        ChoiceCategory::EqualityExpand => 4_000_000,
        ChoiceCategory::MemoryFresh | ChoiceCategory::ScheduleSerial => 1_000_000,
    }
}

fn lexical_score(id: &str) -> i64 {
    id.bytes().take(7).fold(0_i64, |score, byte| {
        score.saturating_mul(257).saturating_sub(i64::from(byte))
    })
}

fn seeded_score(seed: u64, id: &str) -> i64 {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    for byte in id.bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        state ^= state >> 31;
    }
    i64::try_from(state & 0x3fff_ffff_ffff_ffff).unwrap_or(i64::MAX)
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn percentile(values: &[u64], percent: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = percent.saturating_mul(sorted.len()).saturating_add(99) / 100;
    sorted[rank
        .max(1)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1))]
}

fn limit(
    actual: usize,
    maximum: u64,
    code: EvaluationErrorCode,
    resource: &str,
) -> EvaluationResult<()> {
    let actual = u64::try_from(actual).unwrap_or(u64::MAX);
    if actual > maximum {
        return Err(diagnostic(code, "ranking resource limit exceeded")
            .expected_actual(json!(maximum), json!(actual))
            .repair(format!("reduce {resource} below {maximum}")));
    }
    Ok(())
}

fn diagnostic(code: EvaluationErrorCode, message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}

fn encoding_error(error: &serde_json::Error) -> EvaluationDiagnostic {
    diagnostic(
        EvaluationErrorCode::EvaluationRankingTraceInvalid,
        format!("ranking canonical encoding failed: {error}"),
    )
}
