//! Episode state machine, scripted policies, replay, metrics, and archives.

use crate::{
    acquisition::{
        MeasurementAcquisitionArchiveBundle, measurement_acquisition_checkpoint_hash,
        measurement_acquisition_result_hash, measurement_acquisition_trace_hash,
    },
    corpus::builtin_corpus,
    hashing::{
        aggregate_hash, archive_hash, corpus_hash, episode_hash, evaluation_hash, observation_hash,
        policy_hash,
    },
    learned::{
        DatasetSplit, InferenceRecord, LearnedModelArtifact, LearnedRankingLimits, RankingDataset,
        RankingInput, TrainingConfiguration, TrainingRun, dataset_split_hash, inference_hash,
        learned_model_hash, ranking_dataset_hash, ranking_input_hash, training_configuration_hash,
        training_run_hash, validate_dataset, validate_model_artifact, validate_split,
        validate_training_configuration_contract, validate_training_run, verify_inference,
    },
    measured::{
        MeasuredObjectiveDescriptor, MeasuredRecommendation, MeasuredSearchRunRecord,
        MeasuredSearchWorkCounters, MeasurementCohort, measured_objective_hash,
        measured_recommendation, measured_recommendation_hash, measurement_cohort_hash,
        verify_measurement_cohort,
    },
    model::{
        CompilerOutcome, ContextMeasurement, EpisodeResult, EpisodeStatus, EpisodeStep,
        EvaluationAggregate, EvaluationArchive, EvaluationComparison, EvaluationContinuation,
        EvaluationCorpus, EvaluationDiagnostic, EvaluationEpisode, EvaluationErrorCode,
        EvaluationManifest, EvaluationObservation, EvaluationResult, EvaluationRun, EvaluationTask,
        EvaluationTaskId, LearningEpisodeStatus, MeasuredSearchHistoryStatus,
        MeasurementAcquisitionHistoryStatus, MeasurementAcquisitionRecoveryHistoryStatus,
        PolicyCapabilities, PolicyDecision, PolicyDescriptor, PolicyKind, PolicyOrigin,
        PolicyVersion, RejectionClassification, RepairCycle, SearchHistoryStatus, SemanticResult,
        TaskBudget, TaskSuccessCriterion, TokenUsage, UsageTrust,
    },
    ranking::{
        ChoiceCategory, ChoiceOrigin, ChoicePreconditions, EvaluationChoiceSet, FeatureSchema,
        RankingDecision, RankingLimits, RankingPolicyDescriptor, RankingTrace, SelectionOutcome,
        build_choice_set, compiler_choice, feature_schema_v1, rank_choices, record_selection,
        scripted_ranker,
    },
    recovery::{
        MeasurementAcquisitionRecoveryArchiveBundle, measurement_acquisition_reconciliation_hash,
        measurement_acquisition_recovery_checkpoint_hash,
        measurement_acquisition_recovery_journal_hash,
    },
    search::{
        SearchCheckpoint, SearchLimits, SearchRanker, SearchSession, replay_search,
        search_checkpoint_hash, search_edge_hash, search_node_hash, search_objective_hash,
        search_plan_hash, search_result_hash, search_trace_hash,
    },
};
use agentir_core::backend::compiler_build_hash;
use agentir_protocol::Engine as CompilerEngine;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

/// Hard operational limits for the Stage 6A harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationLimits {
    /// Stable operational hard limit.
    pub corpora: u64,
    /// Stable operational hard limit.
    pub tasks: u64,
    /// Stable operational hard limit.
    pub task_bytes: u64,
    /// Stable operational hard limit.
    pub runs: u64,
    /// Stable operational hard limit.
    pub episodes: u64,
    /// Stable operational hard limit.
    pub episode_steps: u64,
    /// Stable operational hard limit.
    pub observation_bytes: u64,
    /// Stable operational hard limit.
    pub menu_choices: u64,
    /// Stable operational hard limit.
    pub submitted_action_bytes: u64,
    /// Stable operational hard limit.
    pub transcript_bytes: u64,
    /// Stable operational hard limit.
    pub decisions: u64,
    /// Stable operational hard limit.
    pub rejections: u64,
    /// Stable operational hard limit.
    pub repair_cycles: u64,
    /// Stable operational hard limit.
    pub token_count: u64,
    /// Stable operational hard limit.
    pub cumulative_context_bytes: u64,
    /// Stable operational hard limit.
    pub active_runs: u64,
    /// Stable operational hard limit.
    pub aggregates: u64,
    /// Stable operational hard limit.
    pub comparisons: u64,
    /// Stable operational hard limit.
    pub archive_bytes: u64,
    /// Stable operational hard limit.
    pub replay_work_units: u64,
    /// Stable operational hard limit.
    pub generated_property_cases: u64,
}

impl Default for EvaluationLimits {
    fn default() -> Self {
        Self {
            corpora: 8,
            tasks: 1_000,
            task_bytes: 8 * 1024 * 1024,
            runs: 128,
            episodes: 4_096,
            episode_steps: 10_000,
            observation_bytes: 4 * 1024 * 1024,
            menu_choices: 256,
            submitted_action_bytes: 4 * 1024 * 1024,
            transcript_bytes: 64 * 1024 * 1024,
            decisions: 10_000,
            rejections: 1_000,
            repair_cycles: 1_000,
            token_count: 100_000_000,
            cumulative_context_bytes: 256 * 1024 * 1024,
            active_runs: 32,
            aggregates: 1_024,
            comparisons: 1_024,
            archive_bytes: 256 * 1024 * 1024,
            replay_work_units: 1_000_000,
            generated_property_cases: 10_000,
        }
    }
}

fn limit(actual: u64, maximum: u64, resource: &str) -> EvaluationResult<()> {
    if actual > maximum {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationBudgetExceeded,
            format!("evaluation resource `{resource}` exceeded"),
        )
        .expected_actual(json!(maximum), json!(actual))
        .repair(format!("reduce {resource} below {maximum}")));
    }
    Ok(())
}

/// Creates one of the five deterministic CI baseline descriptors.
pub fn scripted_policy(name: &str) -> EvaluationResult<PolicyDescriptor> {
    let (kind, escape) = match name {
        "free_reference_v1" => (PolicyKind::Free, false),
        "menu_first_valid_v1" | "menu_goal_directed_v1" => (PolicyKind::Menu, false),
        "hybrid_menu_preferred_v1" => (PolicyKind::Hybrid, false),
        "hybrid_bounded_escape_v1" => (PolicyKind::Hybrid, true),
        _ => {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationPolicyNotFound,
                format!("scripted policy `{name}` does not exist"),
            ));
        }
    };
    descriptor(kind, name, PolicyOrigin::Scripted, escape)
}

/// Creates a bounded external policy descriptor without provider SDK state.
pub fn external_policy(kind: PolicyKind, name: &str) -> EvaluationResult<PolicyDescriptor> {
    descriptor(
        kind,
        name,
        PolicyOrigin::External,
        matches!(kind, PolicyKind::Hybrid),
    )
}

/// Creates an evaluation policy bound to one exact Stage 6B ranking descriptor.
pub fn ranked_policy(
    ranker: &RankingPolicyDescriptor,
    origin: PolicyOrigin,
) -> EvaluationResult<PolicyDescriptor> {
    let mut policy = descriptor(
        ranker.base_interaction_mode,
        &ranker.name,
        origin,
        ranker.allowed_escape,
    )?;
    policy.configuration.insert(
        "ranking_policy_hash".to_owned(),
        json!(ranker.ranking_policy_hash),
    );
    policy.configuration.insert(
        "feature_schema_hash".to_owned(),
        json!(ranker.feature_schema_hash),
    );
    policy
        .configuration
        .insert("ranked".to_owned(), json!(true));
    if let Some(seed) = ranker.configuration.get("seed") {
        policy
            .configuration
            .insert("ranking_seed".to_owned(), seed.clone());
    }
    policy.policy_hash.clear();
    policy.policy_hash = policy_hash(&policy)?;
    Ok(policy)
}

fn descriptor(
    kind: PolicyKind,
    name: &str,
    origin: PolicyOrigin,
    escape: bool,
) -> EvaluationResult<PolicyDescriptor> {
    let (action_surface, menu_selection, typed_escape) = match kind {
        PolicyKind::Free => ("production_action_schema", false, false),
        PolicyKind::Menu => ("compiler_generated_choices_only", true, false),
        PolicyKind::Hybrid => ("compiler_choices_with_bounded_escape", true, true),
    };
    let mut descriptor = PolicyDescriptor {
        kind,
        name: name.to_owned(),
        version: PolicyVersion("1".to_owned()),
        configuration: BTreeMap::from([
            ("bounded_escape".to_owned(), json!(escape)),
            ("continuation_order".to_owned(), json!("task_script_v1")),
        ]),
        capabilities: PolicyCapabilities {
            observation_fields: vec![
                "task_objective".to_owned(),
                "visible_workspace_summary".to_owned(),
                "diagnostics".to_owned(),
                "remaining_budget".to_owned(),
                "allowed_action_schema".to_owned(),
            ],
            action_surface: action_surface.to_owned(),
            menu_selection,
            typed_escape,
        },
        accounting_method: "utf8_bytes_div4_v1".to_owned(),
        external_model_id: None,
        prompt_hash: None,
        origin,
        policy_hash: String::new(),
    };
    descriptor.policy_hash = policy_hash(&descriptor)?;
    Ok(descriptor)
}

fn response_value(engine: &mut CompilerEngine, action: &Value) -> EvaluationResult<Value> {
    let line = serde_json::to_string(action).map_err(|error| {
        EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTranscriptInvalid,
            format!("submitted production action cannot be encoded: {error}"),
        )
    })?;
    serde_json::from_str(&engine.process_line(&line)).map_err(|error| {
        EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTranscriptInvalid,
            format!("production compiler returned invalid JSON: {error}"),
        )
    })
}

fn error_code(response: &Value) -> Option<String> {
    response
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn classify(code: Option<&str>) -> Option<RejectionClassification> {
    let code = code?;
    Some(match code {
        "INVALID_REQUEST" => RejectionClassification::MalformedRequest,
        "BASE_REVISION_CONFLICT"
        | "REWRITE_PRECONDITION_FAILED"
        | "MEMORY_HASH_MISMATCH"
        | "SCHEDULE_HASH_MISMATCH"
        | "BACKEND_HASH_MISMATCH" => RejectionClassification::StaleBaseOrHash,
        "UNKNOWN_REFERENCE"
        | "REVISION_NOT_FOUND"
        | "CANDIDATE_NOT_FOUND"
        | "MEMORY_PLAN_NOT_FOUND"
        | "SCHEDULE_PLAN_NOT_FOUND" => RejectionClassification::InvalidReference,
        "TYPE_MISMATCH" | "SHAPE_MISMATCH" | "IMPL_VERIFICATION_FAILED" => {
            RejectionClassification::TypeError
        }
        "OPEN_HOLE" | "CANDIDATE_HAS_PROOF_DEBT" | "EQUIVALENCE_NOT_PROVED" => {
            RejectionClassification::OpenObligation
        }
        "RESOURCE_LIMIT_EXCEEDED" | "MEMORY_RESOURCE_LIMIT" | "SCHEDULE_RESOURCE_LIMIT" => {
            RejectionClassification::ResourceLimit
        }
        "UNSUPPORTED_BACKEND_LOWERING" => RejectionClassification::UnsupportedBackendLowering,
        "DEVICE_UNAVAILABLE" => RejectionClassification::DeviceUnavailable,
        "UNKNOWN_OPCODE" | "UNSUPPORTED_REFINEMENT" | "TRANSLATION_UNSUPPORTED" => {
            RejectionClassification::UnsupportedAction
        }
        _ => RejectionClassification::OtherCompilerRejection,
    })
}

fn progress_action(action: &Value) -> bool {
    action
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            let operation = command
                .rsplit_once('.')
                .map_or(command, |(_, operation)| operation);
            [
                "apply",
                "create",
                "freeze",
                "seal",
                "lower",
                "emit",
                "materialize",
                "translation_check",
                "open",
            ]
            .contains(&operation)
        })
}

fn visible_hashes(previous: Option<&CompilerOutcome>) -> BTreeMap<String, String> {
    let Some(Value::Object(result)) = previous.map(|outcome| &outcome.response) else {
        return BTreeMap::new();
    };
    let source = result
        .get("result")
        .and_then(Value::as_object)
        .unwrap_or(result);
    source
        .iter()
        .filter(|(key, value)| {
            (key.ends_with("hash") || key.ends_with("revision")) && value.is_string()
        })
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_owned()))
        .collect()
}

fn action_schema(kind: PolicyKind) -> Value {
    match kind {
        PolicyKind::Free => json!({
            "kind": "production_request",
            "required": ["command", "request_id"],
            "references": "only IDs and hashes visible in prior compiler outcomes"
        }),
        PolicyKind::Menu => json!({
            "kind": "menu_choice",
            "required": ["choice_id"],
            "arbitrary_action": false
        }),
        PolicyKind::Hybrid => json!({
            "one_of": [
                {"kind": "menu_choice", "required": ["choice_id"]},
                {"kind": "typed_escape", "required": ["action"], "max_actions": 1}
            ],
            "verification": "production compiler transaction path"
        }),
    }
}

/// Exact bounded input for one explicit Stage 6B ranking decision.
#[derive(Clone, Debug, PartialEq)]
pub struct RankingSubmission {
    /// Episode identity.
    pub episode_id: String,
    /// Exact current step identity.
    pub step_id: String,
    /// Exact observation anchor.
    pub observation_hash: String,
    /// Exact compiler-generated choice-set anchor.
    pub choice_set_hash: String,
    /// Exact visible feature-schema anchor.
    pub feature_schema_hash: String,
    /// Exact policy descriptor anchor.
    pub ranking_policy_hash: String,
    /// Policy-owned scores or explicit visible selection.
    pub decision: RankingDecision,
    /// Optional untrusted usage provenance.
    pub usage: Option<TokenUsage>,
    /// Optional opaque external correlation identity.
    pub correlation_id: Option<String>,
}

/// Stage 6C artifacts attached atomically to the current evaluation archive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LearnedArchiveBundle {
    /// Immutable datasets.
    pub datasets: Vec<RankingDataset>,
    /// Stable group splits.
    pub splits: Vec<DatasetSplit>,
    /// Deterministic training configurations.
    pub configurations: Vec<TrainingConfiguration>,
    /// Deterministic training runs.
    pub training_runs: Vec<TrainingRun>,
    /// Fixed-point learned model artifacts.
    pub models: Vec<LearnedModelArtifact>,
    /// Exact policy-visible inference inputs.
    pub inputs: Vec<RankingInput>,
    /// Exact fixed-point inference records.
    pub inferences: Vec<InferenceRecord>,
    /// Learned policies referenced by inference records.
    pub policies: Vec<RankingPolicyDescriptor>,
}

fn ranked_policy_hash(policy: &PolicyDescriptor) -> Option<&str> {
    policy
        .configuration
        .get("ranking_policy_hash")
        .and_then(Value::as_str)
}

fn compiler_generated_continuations(
    previous: Option<&CompilerOutcome>,
    next: &Value,
    ordinal: u64,
) -> Vec<EvaluationContinuation> {
    let Some(result) = previous
        .and_then(|outcome| outcome.response.get("result"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let workspace = next
        .get("workspace")
        .and_then(Value::as_str)
        .unwrap_or("w1");
    let mut choices = Vec::new();

    if let (Some(candidate), Some(revision), Some(before), Some(matches)) = (
        result.get("candidate").and_then(Value::as_str),
        result.get("candidate_revision").and_then(Value::as_str),
        result
            .get("expected_before_impl_hash")
            .and_then(Value::as_str),
        result.get("matches").and_then(Value::as_array),
    ) {
        for (index, matched) in matches.iter().enumerate() {
            let Some(rule) = matched.get("rule").and_then(Value::as_str) else {
                continue;
            };
            let Some(target) = matched.get("target").and_then(Value::as_str) else {
                continue;
            };
            if matched.get("applicability").and_then(Value::as_str) != Some("applicable") {
                continue;
            }
            choices.push(EvaluationContinuation {
                choice_id: format!("compiler-candidate-{ordinal}-{index}"),
                description: format!("exact compiler rewrite {rule} at {target}"),
                action: json!({
                    "command": "candidate.apply",
                    "request_id": format!("ranked-candidate-{ordinal}-{index}"),
                    "workspace": workspace,
                    "candidate": candidate,
                    "base_candidate_revision": revision,
                    "actions": [{
                        "kind": "apply_known_rewrite",
                        "rule": rule,
                        "target": target,
                        "expected_before_impl_hash": before
                    }]
                }),
            });
        }
    }

    if let (Some(plan), Some(revision), Some(memory_hash), Some(impl_hash), Some(reuses)) = (
        result.get("memory_plan").and_then(Value::as_str),
        result.get("memory_revision").and_then(Value::as_str),
        result.get("expected_memory_hash").and_then(Value::as_str),
        result.get("expected_impl_hash").and_then(Value::as_str),
        result.get("reuse_choices").and_then(Value::as_array),
    ) {
        let mut fresh_results = BTreeSet::new();
        for reuse in reuses {
            let (Some(input), Some(output)) = (
                reuse.get("input").and_then(Value::as_str),
                reuse.get("result").and_then(Value::as_str),
            ) else {
                continue;
            };
            if reuse
                .get("fresh_fallback")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && fresh_results.insert(output)
            {
                choices.push(EvaluationContinuation {
                    choice_id: format!("compiler-memory-fresh-{ordinal}-{output}"),
                    description: format!("retain compiler fresh buffer for {output}"),
                    action: json!({
                        "command": "memory.apply",
                        "request_id": format!("ranked-memory-fresh-{ordinal}-{output}"),
                        "workspace": workspace,
                        "memory_plan": plan,
                        "base_memory_revision": revision,
                        "expected_memory_hash": memory_hash,
                        "expected_impl_hash": impl_hash,
                        "actions": [{"kind":"choose_fresh_buffer","result":output}]
                    }),
                });
            }
            choices.push(EvaluationContinuation {
                choice_id: format!("compiler-memory-reuse-{ordinal}-{input}-{output}"),
                description: if reuse
                    .get("statically_applicable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    format!("compiler-proved static reuse {input} for {output}")
                } else {
                    format!("compiler-visible reuse attempt {input} for {output}")
                },
                action: json!({
                    "command": "memory.apply",
                    "request_id": format!("ranked-memory-reuse-{ordinal}-{input}-{output}"),
                    "workspace": workspace,
                    "memory_plan": plan,
                    "base_memory_revision": revision,
                    "expected_memory_hash": memory_hash,
                    "expected_impl_hash": impl_hash,
                    "actions": [{"kind":"request_in_place_reuse","input":input,"result":output}]
                }),
            });
        }
    }

    if let (Some(plan), Some(revision), Some(schedule_hash), Some(memory_hash), Some(target_hash)) = (
        result.get("schedule_plan").and_then(Value::as_str),
        result.get("schedule_revision").and_then(Value::as_str),
        result.get("expected_schedule_hash").and_then(Value::as_str),
        result.get("expected_memory_hash").and_then(Value::as_str),
        result.get("expected_target_hash").and_then(Value::as_str),
    ) {
        let base = json!({
            "command": "schedule.apply",
            "workspace": workspace,
            "schedule_plan": plan,
            "base_schedule_revision": revision,
            "expected_schedule_hash": schedule_hash,
            "expected_memory_hash": memory_hash,
            "expected_target_hash": target_hash
        });
        let axes = result
            .get("eligible_axes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let factors = result
            .get("factors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for axis in &axes {
            for factor in &factors {
                let mut action = base.clone();
                action["request_id"] = json!(format!("ranked-tile-{}-{}", ordinal, choices.len()));
                action["actions"] =
                    json!([{"kind":"tile_axes","axes":[axis],"tile_sizes":[factor]}]);
                choices.push(EvaluationContinuation {
                    choice_id: format!("compiler-schedule-tile-{ordinal}-{}", choices.len()),
                    description: format!("compiler legal tile {factor} for axis {axis}"),
                    action,
                });
            }
            for width in result
                .get("vector_widths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut action = base.clone();
                action["request_id"] =
                    json!(format!("ranked-vector-{}-{}", ordinal, choices.len()));
                action["actions"] = json!([{"kind":"vectorize_axis","axis":axis,"width":width}]);
                choices.push(EvaluationContinuation {
                    choice_id: format!("compiler-schedule-vector-{ordinal}-{}", choices.len()),
                    description: format!("compiler-supported vector width {width} for axis {axis}"),
                    action,
                });
            }
            for factor in result
                .get("unroll_factors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut action = base.clone();
                action["request_id"] =
                    json!(format!("ranked-unroll-{}-{}", ordinal, choices.len()));
                action["actions"] = json!([{"kind":"unroll_axis","axis":axis,"factor":factor}]);
                choices.push(EvaluationContinuation {
                    choice_id: format!("compiler-schedule-unroll-{ordinal}-{}", choices.len()),
                    description: format!("compiler-supported unroll {factor} for axis {axis}"),
                    action,
                });
            }
        }
        for pair in result
            .get("fusion_pairs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(pair) = pair.as_array() else {
                continue;
            };
            if pair.len() != 2 {
                continue;
            }
            let mut action = base.clone();
            action["request_id"] = json!(format!("ranked-fusion-{}-{}", ordinal, choices.len()));
            action["actions"] =
                json!([{"kind":"fuse_operations","producer":pair[0],"consumer":pair[1]}]);
            choices.push(EvaluationContinuation {
                choice_id: format!("compiler-schedule-fusion-{ordinal}-{}", choices.len()),
                description: "compiler-verified restricted fusion".to_owned(),
                action,
            });
        }
    }

    if result.get("purpose").and_then(Value::as_str) == Some("fill_hole") {
        let hole = result
            .get("focus")
            .and_then(|focus| focus.get("hole"))
            .and_then(Value::as_str);
        let revision = result.get("revision").and_then(Value::as_str);
        let values = result
            .get("slots")
            .and_then(Value::as_array)
            .and_then(|slots| {
                slots
                    .iter()
                    .find_map(|slot| slot.pointer("/domain/values").and_then(Value::as_array))
            });
        if let (Some(hole), Some(revision), Some(values)) = (hole, revision, values) {
            for value in values {
                let Some(value) = value.as_str() else {
                    continue;
                };
                choices.push(EvaluationContinuation {
                    choice_id: format!("compiler-hole-{ordinal}-{value}"),
                    description: format!("compiler-compatible value {value} for hole {hole}"),
                    action: json!({
                        "command":"spec.apply",
                        "request_id":format!("ranked-hole-{ordinal}-{value}"),
                        "workspace":workspace,
                        "base_revision":revision,
                        "actions":[{"kind":"fill_hole","hole":hole,"value":value}]
                    }),
                });
            }
        }
    }
    choices
}

fn choice_origin_category(action: &Value) -> (ChoiceOrigin, ChoiceCategory) {
    let command = action.get("command").and_then(Value::as_str).unwrap_or("");
    let kind = action
        .pointer("/actions/0/kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    match (command, kind) {
        ("spec.apply", "fill_hole") => (ChoiceOrigin::SpecIr, ChoiceCategory::FillHole),
        ("candidate.apply", _) => (ChoiceOrigin::Candidate, ChoiceCategory::ExactRewrite),
        ("equality.expand", _) => (ChoiceOrigin::Equality, ChoiceCategory::EqualityExpand),
        ("equality.materialize", _) => {
            (ChoiceOrigin::Equality, ChoiceCategory::EqualityMaterialize)
        }
        ("memory.apply", "choose_fresh_buffer") => {
            (ChoiceOrigin::Memory, ChoiceCategory::MemoryFresh)
        }
        ("memory.apply", "request_guarded_reuse") => {
            (ChoiceOrigin::Memory, ChoiceCategory::MemoryGuardedReuse)
        }
        ("memory.apply", _) => (ChoiceOrigin::Memory, ChoiceCategory::MemoryStaticReuse),
        ("schedule.apply", "tile_axes" | "split_axis") => {
            (ChoiceOrigin::Schedule, ChoiceCategory::ScheduleTile)
        }
        ("schedule.apply", "fuse_operations") => {
            (ChoiceOrigin::Schedule, ChoiceCategory::ScheduleFusion)
        }
        ("schedule.apply", "vectorize_axis") => {
            (ChoiceOrigin::Schedule, ChoiceCategory::ScheduleVectorize)
        }
        ("schedule.apply", "unroll_axis") => {
            (ChoiceOrigin::Schedule, ChoiceCategory::ScheduleUnroll)
        }
        ("schedule.apply", "choose_serial") => {
            (ChoiceOrigin::Schedule, ChoiceCategory::ScheduleSerial)
        }
        ("backend.lower" | "artifact.emit", _) => {
            (ChoiceOrigin::Backend, ChoiceCategory::BackendLowering)
        }
        _ => (ChoiceOrigin::Repair, ChoiceCategory::Repair),
    }
}

fn structural_target(action: &Value) -> String {
    [
        "target",
        "axis",
        "result",
        "node",
        "schedule_revision",
        "candidate_revision",
    ]
    .into_iter()
    .find_map(|field| {
        action
            .pointer(&format!("/actions/0/{field}"))
            .or_else(|| action.get(field))
            .and_then(Value::as_str)
    })
    .unwrap_or("frame")
    .to_owned()
}

fn evaluation_choice_set_from_observation(
    observation: &EvaluationObservation,
) -> EvaluationResult<EvaluationChoiceSet> {
    let schema = feature_schema_v1()?;
    let choices = observation
        .continuation_frame
        .iter()
        .map(|continuation| {
            let (origin, category) = choice_origin_category(&continuation.action);
            compiler_choice(
                origin,
                category,
                continuation.action.clone(),
                ChoicePreconditions {
                    bases: observation.selected_revisions_and_hashes.clone(),
                    required_capabilities: Vec::new(),
                    hard_conditions: vec!["production verifier acceptance required".to_owned()],
                },
                continuation.description.clone(),
                if matches!(
                    category,
                    ChoiceCategory::ExactRewrite | ChoiceCategory::EqualityMaterialize
                ) {
                    "advance"
                } else {
                    "unchanged_or_compiler_owned"
                },
                structural_target(&continuation.action),
            )
        })
        .collect::<EvaluationResult<Vec<_>>>()?;
    build_choice_set(
        observation.observation_hash.clone(),
        &schema,
        choices,
        &RankingLimits::default(),
    )
}

/// Stateful in-memory Stage 6A harness.
pub struct EvaluationHarness {
    corpus: EvaluationCorpus,
    runs: BTreeMap<String, EvaluationRun>,
    sessions: BTreeMap<String, CompilerEngine>,
    ranking_policy_registry: BTreeMap<String, RankingPolicyDescriptor>,
    limits: EvaluationLimits,
    next_run: u64,
    comparison_count: u64,
}

impl std::fmt::Debug for EvaluationHarness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvaluationHarness")
            .field("corpus", &self.corpus)
            .field("runs", &self.runs)
            .field("limits", &self.limits)
            .field("next_run", &self.next_run)
            .finish_non_exhaustive()
    }
}

impl EvaluationHarness {
    /// Creates a harness with the immutable built-in corpus.
    pub fn new() -> EvaluationResult<Self> {
        Self::with_corpus_and_limits(builtin_corpus()?, EvaluationLimits::default())
    }

    /// Creates a harness with explicit corpus and operational limits.
    pub fn with_corpus_and_limits(
        corpus: EvaluationCorpus,
        limits: EvaluationLimits,
    ) -> EvaluationResult<Self> {
        limit(1, limits.corpora, "corpora")?;
        limit(
            u64::try_from(corpus.tasks.len()).unwrap_or(u64::MAX),
            limits.tasks,
            "tasks",
        )?;
        limit(
            u64::try_from(serde_json::to_vec(&corpus).unwrap_or_default().len())
                .unwrap_or(u64::MAX),
            limits.task_bytes,
            "task_bytes",
        )?;
        let calculated = corpus_hash(&corpus)?;
        if calculated != corpus.corpus_hash {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "corpus hash does not match its ordered task definitions",
            )
            .expected_actual(json!(calculated), json!(corpus.corpus_hash)));
        }
        Ok(Self {
            corpus,
            runs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            ranking_policy_registry: BTreeMap::new(),
            limits,
            next_run: 0,
            comparison_count: 0,
        })
    }

    /// Returns the immutable corpus.
    #[must_use]
    pub const fn corpus(&self) -> &EvaluationCorpus {
        &self.corpus
    }

    /// Returns a retained run.
    pub fn run(&self, run_id: &str) -> EvaluationResult<&EvaluationRun> {
        self.runs.get(run_id).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRunNotFound,
                format!("evaluation run `{run_id}` does not exist"),
            )
        })
    }

    /// Iterates retained run IDs in deterministic lexical order.
    pub fn run_ids(&self) -> impl Iterator<Item = &str> {
        self.runs.keys().map(String::as_str)
    }

    /// Returns one immutable task definition.
    pub fn task(&self, task_id: &EvaluationTaskId) -> EvaluationResult<&EvaluationTask> {
        self.corpus
            .tasks
            .iter()
            .find(|task| &task.id == task_id)
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationTaskNotFound,
                    format!("evaluation task `{}` does not exist", task_id.0),
                )
            })
    }

    /// Returns the immutable Stage 6B visible feature schema.
    pub fn ranking_feature_schema(&self) -> EvaluationResult<FeatureSchema> {
        feature_schema_v1()
    }

    /// Returns all deterministic scripted ranking descriptors in stable order.
    pub fn ranking_policies(&self, seed: u64) -> EvaluationResult<Vec<RankingPolicyDescriptor>> {
        [
            "lexicographic_choice_v1",
            "first_progress_choice_v1",
            "goal_directed_rule_v1",
            "proof_frontier_first_v1",
            "min_context_choice_v1",
            "seeded_uniform_choice_v1",
            "hybrid_ranked_escape_v1",
        ]
        .into_iter()
        .map(|name| scripted_ranker(name, &feature_schema_v1()?, seed))
        .collect()
    }

    /// Registers one exact non-scripted ranking descriptor for later dispatch/archive replay.
    pub fn register_ranking_policy(
        &mut self,
        policy: RankingPolicyDescriptor,
    ) -> EvaluationResult<()> {
        if policy.ranking_policy_hash != crate::ranking::ranking_policy_hash(&policy)? {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                "ranking policy registry rejected an invalid descriptor hash",
            ));
        }
        if let Some(retained) = self
            .ranking_policy_registry
            .get(&policy.ranking_policy_hash)
            && retained != &policy
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                "ranking policy hash is already registered with different content",
            ));
        }
        self.ranking_policy_registry
            .insert(policy.ranking_policy_hash.clone(), policy);
        Ok(())
    }

    /// Rebuilds the exact current compiler-generated choice set for an episode.
    pub fn ranked_choice_set(&self, episode_id: &str) -> EvaluationResult<EvaluationChoiceSet> {
        let observation = self.next_observation(episode_id)?;
        if observation.choice_set_hash.is_none() {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationChoiceSetNotFound,
                "episode policy is not bound to a ranking descriptor",
            ));
        }
        let set = evaluation_choice_set_from_observation(&observation)?;
        if observation.choice_set_hash.as_deref() != Some(set.choice_set_hash.as_str()) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationChoiceSetMismatch,
                "observation and rebuilt exact choice set differ",
            ));
        }
        Ok(set)
    }

    /// Applies one policy-owned ranking and then explicitly dispatches its selection.
    pub fn rank_episode(
        &mut self,
        submission: RankingSubmission,
    ) -> EvaluationResult<(RankingTrace, SelectionOutcome)> {
        let (run, index) = self.locate_episode(&submission.episode_id)?;
        let seed = run
            .policy
            .configuration
            .get("ranking_seed")
            .and_then(Value::as_u64)
            .unwrap_or(run.episodes[index].deterministic_seed);
        let policy = self
            .ranking_policies(seed)?
            .into_iter()
            .find(|policy| policy.ranking_policy_hash == submission.ranking_policy_hash)
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationRankingPolicyNotFound,
                    "ranking policy hash is not registered for this deterministic seed",
                )
            })?;
        self.rank_episode_with_descriptor(submission, &policy)
    }

    /// Applies an exact learned or external descriptor and dispatches once only after validation.
    pub fn rank_episode_with_descriptor(
        &mut self,
        submission: RankingSubmission,
        policy: &RankingPolicyDescriptor,
    ) -> EvaluationResult<(RankingTrace, SelectionOutcome)> {
        let observation = self.next_observation(&submission.episode_id)?;
        if observation.step_id != submission.step_id
            || observation.observation_hash != submission.observation_hash
            || observation.choice_set_hash.as_deref() != Some(&submission.choice_set_hash)
            || observation.feature_schema_hash.as_deref() != Some(&submission.feature_schema_hash)
            || policy.ranking_policy_hash != submission.ranking_policy_hash
            || policy.ranking_policy_hash != crate::ranking::ranking_policy_hash(policy)?
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationChoiceSetMismatch,
                "rank request is stale or mismatched with the exact observation frame",
            )
            .repair("request evaluation.episode.next and rank its exact returned frame"));
        }
        let choice_set = self.ranked_choice_set(&submission.episode_id)?;
        let (run, _) = self.locate_episode(&submission.episode_id)?;
        if ranked_policy_hash(&run.policy) != Some(&submission.ranking_policy_hash) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                "run is not bound to the submitted ranking policy",
            ));
        }
        let trace = rank_choices(
            &choice_set,
            policy,
            submission.decision,
            &RankingLimits::default(),
        )?;
        let production_decision = if trace.selection_source
            == crate::ranking::SelectionSource::HybridEscape
        {
            let RankingDecision::HybridEscape { action } = &trace.decision else {
                unreachable!("hybrid escape source has hybrid escape decision")
            };
            PolicyDecision::Action {
                action: action.clone(),
                escape: true,
            }
        } else {
            let selected_id = trace.selected_choice.as_ref().ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationSelectionInvalid,
                    "ranked visible selection is missing its choice identity",
                )
            })?;
            let selected = choice_set
                .choices
                .iter()
                .find(|choice| &choice.id == selected_id)
                .ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationSelectionNotInChoiceSet,
                        "ranked selection is outside the exact choice set",
                    )
                })?;
            let continuation_index = usize::try_from(selected.compiler_order).unwrap_or(usize::MAX);
            let menu_choice = observation
                .continuation_frame
                .get(continuation_index)
                .ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationSelectionInvalid,
                        "ranked compiler order is outside the observation frame",
                    )
                })?
                .choice_id
                .clone();
            PolicyDecision::MenuChoice {
                choice_id: menu_choice,
            }
        };
        let outcome = self.submit(
            &submission.episode_id,
            &submission.step_id,
            &submission.observation_hash,
            production_decision,
            submission.usage,
            submission.correlation_id,
        )?;
        let selection = record_selection(&choice_set, &trace, outcome)?;
        let (run, index) = self.locate_episode(&submission.episode_id)?;
        let run_id = run.id.clone();
        let run = self
            .runs
            .get_mut(&run_id)
            .expect("located run remains present");
        let episode = &mut run.episodes[index];
        let step = episode.steps.last_mut().ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSelectionInvalid,
                "production selection did not append an episode step",
            )
        })?;
        step.ranking_trace = Some(trace.clone());
        step.selection = Some(selection.clone());
        if episode.result.is_some() {
            episode.episode_hash = Some(episode_hash(episode)?);
            if run.episodes.iter().all(|episode| episode.result.is_some()) {
                run.evaluation_hash = Some(evaluation_hash(run)?);
            }
        }
        Ok((trace, selection))
    }

    /// Starts a lazy external or scripted run over selected tasks and seeds.
    pub fn start_run(
        &mut self,
        policy: PolicyDescriptor,
        task_ids: &[EvaluationTaskId],
        seeds: &[u64],
    ) -> EvaluationResult<String> {
        limit(
            u64::try_from(self.runs.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            self.limits.runs,
            "runs",
        )?;
        let active = self
            .runs
            .values()
            .filter(|run| run.episodes.iter().any(|episode| episode.result.is_none()))
            .count();
        limit(
            u64::try_from(active).unwrap_or(u64::MAX).saturating_add(1),
            self.limits.active_runs,
            "active_runs",
        )?;
        let selected: Vec<EvaluationTaskId> = if task_ids.is_empty() {
            self.corpus
                .tasks
                .iter()
                .map(|task| task.id.clone())
                .collect()
        } else {
            task_ids.to_vec()
        };
        for task_id in &selected {
            let task = self.task(task_id)?;
            if !task.allowed_modes.contains(&policy.kind) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationPolicyViolation,
                    format!("task `{}` does not allow this policy mode", task_id.0),
                ));
            }
        }
        let seeds = if seeds.is_empty() {
            vec![0]
        } else {
            seeds.to_vec()
        };
        let projected = u64::try_from(selected.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::try_from(seeds.len()).unwrap_or(u64::MAX));
        limit(projected, self.limits.episodes, "episodes")?;
        self.next_run = self.next_run.saturating_add(1);
        let run_id = format!("run{}", self.next_run);
        let mut episodes = Vec::new();
        for seed in &seeds {
            for task_id in &selected {
                let id = format!("{}-ep{}", run_id, episodes.len().saturating_add(1));
                self.sessions.insert(id.clone(), CompilerEngine::new());
                episodes.push(EvaluationEpisode {
                    id,
                    run_id: run_id.clone(),
                    task_id: task_id.clone(),
                    policy_hash: policy.policy_hash.clone(),
                    deterministic_seed: *seed,
                    status: EpisodeStatus::Ready,
                    steps: Vec::new(),
                    result: None,
                    episode_hash: None,
                });
            }
        }
        self.runs.insert(
            run_id.clone(),
            EvaluationRun {
                id: run_id.clone(),
                corpus_hash: self.corpus.corpus_hash.clone(),
                policy,
                compiler_build_hash: compiler_build_hash().to_string(),
                seeds,
                episodes,
                cancelled: false,
                evaluation_hash: None,
            },
        );
        Ok(run_id)
    }

    /// Starts a run whose menu observations carry exact Stage 6B choice-set anchors.
    pub fn start_ranked_run(
        &mut self,
        ranking_policy_name: &str,
        task_ids: &[EvaluationTaskId],
        seeds: &[u64],
    ) -> EvaluationResult<String> {
        let seed = seeds.first().copied().unwrap_or(0);
        let ranker = scripted_ranker(ranking_policy_name, &feature_schema_v1()?, seed)?;
        let policy = ranked_policy(&ranker, PolicyOrigin::External)?;
        self.start_run(policy, task_ids, seeds)
    }

    fn locate_episode(&self, episode_id: &str) -> EvaluationResult<(&EvaluationRun, usize)> {
        self.runs
            .values()
            .find_map(|run| {
                run.episodes
                    .iter()
                    .position(|episode| episode.id == episode_id)
                    .map(|index| (run, index))
            })
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationEpisodeNotFound,
                    format!("evaluation episode `{episode_id}` does not exist"),
                )
            })
    }

    /// Builds exactly one policy-visible observation without exposing hidden state.
    pub fn next_observation(&self, episode_id: &str) -> EvaluationResult<EvaluationObservation> {
        let (run, index) = self.locate_episode(episode_id)?;
        let episode = &run.episodes[index];
        if !matches!(
            episode.status,
            EpisodeStatus::Ready | EpisodeStatus::Running
        ) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAlreadyComplete,
                format!("episode `{episode_id}` is not active"),
            ));
        }
        let task = self.task(&episode.task_id)?;
        let ordinal = u64::try_from(episode.steps.len()).unwrap_or(u64::MAX);
        let next = task
            .initial_state
            .production_requests
            .get(episode.steps.len())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAlreadyComplete,
                    "the task has no remaining production action",
                )
            })?;
        let previous = episode.steps.last().map(|step| &step.outcome);
        let ranked = ranked_policy_hash(&run.policy).is_some();
        let continuation_frame = if matches!(run.policy.kind, PolicyKind::Menu | PolicyKind::Hybrid)
        {
            let generated = if ranked {
                compiler_generated_continuations(previous, next, ordinal)
            } else {
                Vec::new()
            };
            if generated.is_empty() {
                vec![EvaluationContinuation {
                    choice_id: format!("choice-{ordinal}"),
                    description: format!(
                        "compiler-generated {} action",
                        next.get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("production")
                    ),
                    action: next.clone(),
                }]
            } else {
                generated
            }
        } else {
            Vec::new()
        };
        limit(
            u64::try_from(continuation_frame.len()).unwrap_or(u64::MAX),
            self.limits.menu_choices,
            "menu_choices",
        )?;
        let used_context = episode.steps.iter().fold(0_u64, |total, step| {
            total.saturating_add(step.context.observation_bytes)
        });
        let used_rejections = episode
            .steps
            .iter()
            .filter(|step| !step.outcome.accepted)
            .count();
        let mut observation = EvaluationObservation {
            run_id: run.id.clone(),
            episode_id: episode.id.clone(),
            step_id: format!("step{ordinal}"),
            interaction_mode: run.policy.kind,
            task_id: task.id.clone(),
            task_objective: task.objective.clone(),
            visible_workspace_summary: previous.map_or_else(
                || json!({"state": "empty"}),
                |outcome| outcome.response.clone(),
            ),
            selected_revisions_and_hashes: visible_hashes(previous),
            open_obligations: Vec::new(),
            diagnostics: previous
                .filter(|outcome| !outcome.accepted)
                .map(|outcome| vec![outcome.response.clone()])
                .unwrap_or_default(),
            continuation_frame,
            choice_set_hash: None,
            feature_schema_hash: None,
            remaining_budget: TaskBudget {
                max_actions: task.budget.max_actions.saturating_sub(ordinal),
                max_rejections: task
                    .budget
                    .max_rejections
                    .saturating_sub(u64::try_from(used_rejections).unwrap_or(u64::MAX)),
                max_context_bytes: task.budget.max_context_bytes.saturating_sub(used_context),
                max_deterministic_tokens: task.budget.max_deterministic_tokens,
                max_wall_time_ms: task.budget.max_wall_time_ms,
            },
            previous_compiler_outcome: previous.map(|outcome| outcome.response.clone()),
            allowed_action_schema: action_schema(run.policy.kind),
            context_bytes: 0,
            deterministic_tokens: None,
            observation_hash: String::new(),
        };
        let bytes = u64::try_from(serde_json::to_vec(&observation).unwrap_or_default().len())
            .unwrap_or(u64::MAX);
        limit(bytes, self.limits.observation_bytes, "observation_bytes")?;
        limit(
            used_context.saturating_add(bytes),
            task.budget
                .max_context_bytes
                .min(self.limits.cumulative_context_bytes),
            "cumulative_context_bytes",
        )?;
        observation.context_bytes = bytes;
        observation.deterministic_tokens = Some(bytes.saturating_add(3) / 4);
        observation.observation_hash = observation_hash(&observation)?;
        if ranked {
            let choice_set = evaluation_choice_set_from_observation(&observation)?;
            observation.choice_set_hash = Some(choice_set.choice_set_hash);
            observation.feature_schema_hash = Some(choice_set.feature_schema_hash);
        }
        Ok(observation)
    }

    /// Atomically validates one decision, invokes the production compiler, and records the step.
    pub fn submit(
        &mut self,
        episode_id: &str,
        step_id: &str,
        expected_observation_hash: &str,
        decision: PolicyDecision,
        usage: Option<TokenUsage>,
        correlation_id: Option<String>,
    ) -> EvaluationResult<CompilerOutcome> {
        let observation = self.next_observation(episode_id)?;
        if observation.step_id != step_id
            || observation.observation_hash != expected_observation_hash
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationObservationMismatch,
                "submitted step or observation hash is stale",
            )
            .expected_actual(
                json!({"step_id": observation.step_id, "observation_hash": observation.observation_hash}),
                json!({"step_id": step_id, "observation_hash": expected_observation_hash}),
            )
            .repair("request evaluation.episode.next and submit against the returned observation"));
        }
        let (run, index) = self.locate_episode(episode_id)?;
        let run_id = run.id.clone();
        let policy = run.policy.clone();
        let task = self.task(&run.episodes[index].task_id)?.clone();
        let action = match (&policy.kind, &decision) {
            (PolicyKind::Menu | PolicyKind::Hybrid, PolicyDecision::MenuChoice { choice_id }) => {
                observation
                    .continuation_frame
                    .iter()
                    .find(|choice| &choice.choice_id == choice_id)
                    .map(|choice| choice.action.clone())
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationMenuChoiceInvalid,
                            format!("menu choice `{choice_id}` was not compiler generated"),
                        )
                    })?
            }
            (
                PolicyKind::Free,
                PolicyDecision::Action {
                    action,
                    escape: false,
                },
            ) => action.clone(),
            (
                PolicyKind::Hybrid,
                PolicyDecision::Action {
                    action,
                    escape: true,
                },
            ) => {
                if !policy.capabilities.typed_escape {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationPolicyViolation,
                        "hybrid escape is disabled by this policy descriptor",
                    ));
                }
                action.clone()
            }
            (_, PolicyDecision::Finish) => {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationPolicyViolation,
                    "task success and finish are compiler-owned; submit the remaining action",
                ));
            }
            _ => {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationPolicyViolation,
                    "decision is outside the policy action surface",
                ));
            }
        };
        let action_bytes = u64::try_from(serde_json::to_vec(&action).unwrap_or_default().len())
            .unwrap_or(u64::MAX);
        limit(
            action_bytes,
            self.limits.submitted_action_bytes,
            "submitted_action_bytes",
        )?;
        let current_steps = u64::try_from(run.episodes[index].steps.len()).unwrap_or(u64::MAX);
        limit(
            current_steps.saturating_add(1),
            task.budget
                .max_actions
                .min(self.limits.decisions)
                .min(self.limits.episode_steps),
            "decisions",
        )?;
        let transcript_bytes = run.episodes[index].steps.iter().fold(0_u64, |total, step| {
            total.saturating_add(
                u64::try_from(serde_json::to_vec(step).unwrap_or_default().len())
                    .unwrap_or(u64::MAX),
            )
        });
        limit(
            transcript_bytes
                .saturating_add(action_bytes)
                .saturating_add(observation.context_bytes),
            self.limits.transcript_bytes,
            "transcript_bytes",
        )?;
        if let Some(report) = &usage {
            limit(
                report.input_tokens.unwrap_or(0),
                self.limits.token_count,
                "input_tokens",
            )?;
            limit(
                report.output_tokens.unwrap_or(0),
                self.limits.token_count,
                "output_tokens",
            )?;
        }

        let response = response_value(
            self.sessions.get_mut(episode_id).ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationEpisodeNotFound,
                    "episode compiler session is missing",
                )
            })?,
            &action,
        )?;
        let accepted = response.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let code = error_code(&response);
        let outcome = CompilerOutcome {
            accepted,
            progress_producing: accepted && progress_action(&action),
            rejection: if accepted {
                None
            } else {
                classify(code.as_deref())
            },
            compiler_error_code: code,
            response,
        };
        let decision_bytes = u64::try_from(serde_json::to_vec(&decision).unwrap_or_default().len())
            .unwrap_or(u64::MAX);
        let run = self
            .runs
            .get_mut(&run_id)
            .expect("located run remains present");
        let episode = &mut run.episodes[index];
        let previous_context = episode.steps.iter().fold(0_u64, |total, step| {
            total.saturating_add(step.context.observation_bytes)
        });
        let ordinal = u64::try_from(episode.steps.len()).unwrap_or(u64::MAX);
        let observation_bytes = observation.context_bytes;
        let deterministic_tokens = observation.deterministic_tokens;
        episode.steps.push(EpisodeStep {
            ordinal,
            observation,
            decision,
            outcome: outcome.clone(),
            context: ContextMeasurement {
                observation_bytes,
                decision_bytes,
                cumulative_context_bytes: previous_context,
                deterministic_tokens,
                reported_usage: usage,
            },
            external_request_correlation_id: correlation_id,
            ranking_trace: None,
            selection: None,
        });
        let last = episode.steps.last_mut().expect("step was appended");
        last.context.cumulative_context_bytes =
            previous_context.saturating_add(last.observation.context_bytes);
        episode.status = EpisodeStatus::Running;
        let rejection_count = episode
            .steps
            .iter()
            .filter(|step| !step.outcome.accepted)
            .count();
        if u64::try_from(rejection_count).unwrap_or(u64::MAX) > task.budget.max_rejections {
            episode.status = EpisodeStatus::BudgetExhausted;
        }
        if episode.steps.len() == task.initial_state.production_requests.len()
            || matches!(episode.status, EpisodeStatus::BudgetExhausted)
        {
            finalize_episode(episode, &task)?;
            run.evaluation_hash = if run.episodes.iter().all(|episode| episode.result.is_some()) {
                Some(evaluation_hash(run)?)
            } else {
                None
            };
        }
        Ok(outcome)
    }

    /// Executes a complete deterministic scripted run without model/provider calls.
    pub fn run_scripted(
        &mut self,
        policy_name: &str,
        task_ids: &[EvaluationTaskId],
        seeds: &[u64],
    ) -> EvaluationResult<String> {
        let policy = scripted_policy(policy_name)?;
        let run_id = self.start_run(policy.clone(), task_ids, seeds)?;
        let episode_ids: Vec<String> = self
            .run(&run_id)?
            .episodes
            .iter()
            .map(|episode| episode.id.clone())
            .collect();
        for episode_id in episode_ids {
            loop {
                let done = {
                    let (run, index) = self.locate_episode(&episode_id)?;
                    run.episodes[index].result.is_some()
                };
                if done {
                    break;
                }
                let observation = self.next_observation(&episode_id)?;
                let ordinal = observation
                    .step_id
                    .trim_start_matches("step")
                    .parse::<u64>()
                    .unwrap_or(0);
                let decision = match policy.kind {
                    PolicyKind::Free => PolicyDecision::Action {
                        action: self
                            .task(&observation.task_id)?
                            .initial_state
                            .production_requests[usize::try_from(ordinal).unwrap_or(usize::MAX)]
                        .clone(),
                        escape: false,
                    },
                    PolicyKind::Hybrid
                        if policy
                            .configuration
                            .get("bounded_escape")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            && ordinal % 3 == 2 =>
                    {
                        PolicyDecision::Action {
                            action: observation.continuation_frame[0].action.clone(),
                            escape: true,
                        }
                    }
                    PolicyKind::Menu | PolicyKind::Hybrid => PolicyDecision::MenuChoice {
                        choice_id: observation.continuation_frame[0].choice_id.clone(),
                    },
                };
                self.submit(
                    &episode_id,
                    &observation.step_id,
                    &observation.observation_hash,
                    decision,
                    None,
                    None,
                )?;
            }
        }
        Ok(run_id)
    }

    /// Cancels all still-active episodes in a run.
    pub fn cancel_run(&mut self, run_id: &str) -> EvaluationResult<()> {
        let run = self.runs.get_mut(run_id).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRunNotFound,
                format!("run `{run_id}` does not exist"),
            )
        })?;
        run.cancelled = true;
        for episode in &mut run.episodes {
            if episode.result.is_none() {
                episode.status = EpisodeStatus::Cancelled;
            }
        }
        Ok(())
    }

    /// Replays a completed run using only recorded decisions and production compiler calls.
    pub fn replay_run(&self, run_id: &str) -> EvaluationResult<()> {
        let run = self.run(run_id)?;
        let work = run.episodes.iter().fold(0_u64, |total, episode| {
            total.saturating_add(u64::try_from(episode.steps.len()).unwrap_or(u64::MAX))
        });
        limit(work, self.limits.replay_work_units, "replay_work_units")?;
        replay_recorded_run(run, &[])
    }

    /// Recomputes raw statistical metrics from completed episodes.
    pub fn aggregate(&self, run_id: &str) -> EvaluationResult<EvaluationAggregate> {
        limit(
            u64::try_from(self.runs.len()).unwrap_or(u64::MAX),
            self.limits.aggregates,
            "aggregates",
        )?;
        let run = self.run(run_id)?;
        if run.episodes.iter().any(|episode| episode.result.is_none()) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMetricInvalid,
                "aggregate requires every episode to have a compiler-derived result",
            ));
        }
        let results: Vec<&EpisodeResult> = run
            .episodes
            .iter()
            .filter_map(|episode| episode.result.as_ref())
            .collect();
        let count = u64::try_from(results.len()).unwrap_or(u64::MAX);
        let successes = u64::try_from(results.iter().filter(|result| result.success).count())
            .unwrap_or(u64::MAX);
        let decisions: Vec<u64> = results
            .iter()
            .map(|result| result.total_decisions)
            .collect();
        let rejections: Vec<u64> = results
            .iter()
            .map(|result| result.rejected_decisions)
            .collect();
        let repairs: Vec<u64> = results
            .iter()
            .map(|result| u64::try_from(result.repair_cycles.len()).unwrap_or(u64::MAX))
            .collect();
        let contexts: Vec<u64> = results.iter().map(|result| result.context_bytes).collect();
        let accepted = results.iter().fold(0_u64, |total, result| {
            total.saturating_add(result.accepted_decisions)
        });
        let deterministic_tokens = results.iter().try_fold(0_u64, |total, result| {
            result
                .deterministic_tokens
                .map(|tokens| total.saturating_add(tokens))
        });
        let mut aggregate = EvaluationAggregate {
            run_id: run_id.to_owned(),
            count,
            successes,
            success_rate: ratio(successes, count),
            median_decisions: percentile(&decisions, 50),
            p95_decisions: percentile(&decisions, 95),
            median_rejections: percentile(&rejections, 50),
            p95_rejections: percentile(&rejections, 95),
            median_repair_cycles: percentile(&repairs, 50),
            p95_repair_cycles: percentile(&repairs, 95),
            median_context_bytes: percentile(&contexts, 50),
            p95_context_bytes: percentile(&contexts, 95),
            accepted_actions_per_token: deterministic_tokens
                .filter(|tokens| *tokens > 0)
                .map(|tokens| accepted as f64 / tokens as f64),
            accepted_actions_per_decision: Some(ratio(accepted, decisions.iter().sum())),
            budget_exhaustion_rate: ratio(
                u64::try_from(
                    results
                        .iter()
                        .filter(|result| result.budget_exhausted)
                        .count(),
                )
                .unwrap_or(u64::MAX),
                count,
            ),
            semantic_correctness_rate: ratio(
                u64::try_from(
                    results
                        .iter()
                        .filter(|result| result.semantic.criterion_satisfied)
                        .count(),
                )
                .unwrap_or(u64::MAX),
                count,
            ),
            aggregate_hash: String::new(),
        };
        aggregate.aggregate_hash = aggregate_hash(&aggregate)?;
        Ok(aggregate)
    }

    /// Compares only runs with matching corpus, compiler build, seeds, and device provenance.
    pub fn compare(&mut self, run_ids: &[String]) -> EvaluationResult<EvaluationComparison> {
        self.comparison_count = self.comparison_count.saturating_add(1);
        limit(
            self.comparison_count,
            self.limits.comparisons,
            "comparisons",
        )?;
        let first = self.run(run_ids.first().ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationComparisonInvalid,
                "comparison requires at least one run",
            )
        })?)?;
        let corpus_hash = first.corpus_hash.clone();
        let build = first.compiler_build_hash.clone();
        let seeds = first.seeds.clone();
        for run_id in run_ids {
            let run = self.run(run_id)?;
            if run.corpus_hash != corpus_hash
                || run.compiler_build_hash != build
                || run.seeds != seeds
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationComparisonInvalid,
                    "runs differ in corpus, compiler build, or deterministic seed set",
                ));
            }
        }
        let ranking_anchors = run_ids
            .iter()
            .map(|run_id| {
                self.run(run_id).map(|run| {
                    (
                        run.policy.capabilities.typed_escape,
                        run.episodes
                            .iter()
                            .flat_map(|episode| episode.steps.iter())
                            .filter_map(|step| {
                                step.ranking_trace.as_ref().map(|trace| {
                                    (
                                        trace.choice_set_hash.clone(),
                                        trace.feature_schema_hash.clone(),
                                    )
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect::<EvaluationResult<Vec<_>>>()?;
        let has_ranking = ranking_anchors
            .iter()
            .any(|(_, anchors)| !anchors.is_empty());
        if has_ranking && ranking_anchors.windows(2).any(|pair| pair[0] != pair[1]) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRankingComparisonInvalid,
                "ranked runs differ in exact choice-set sequence, feature schema, or escape surface",
            ));
        }
        let device_sets: Vec<BTreeSet<String>> = run_ids
            .iter()
            .map(|run_id| {
                self.run(run_id).map(|run| {
                    run.episodes
                        .iter()
                        .filter_map(|episode| {
                            episode
                                .result
                                .as_ref()?
                                .performance
                                .as_ref()
                                .map(|observation| observation.device_fingerprint_hash.clone())
                        })
                        .collect()
                })
            })
            .collect::<EvaluationResult<_>>()?;
        if device_sets.windows(2).any(|pair| pair[0] != pair[1]) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationComparisonInvalid,
                "device performance observations have incompatible fingerprints",
            ));
        }
        Ok(EvaluationComparison {
            run_ids: run_ids.to_vec(),
            corpus_hash,
            compiler_build_hash: build,
            aggregates: run_ids
                .iter()
                .map(|run_id| self.aggregate(run_id))
                .collect::<EvaluationResult<Vec<_>>>()?,
        })
    }

    /// Builds a separate checksummed evaluation archive.
    pub fn archive(&self, run_ids: &[String]) -> EvaluationResult<EvaluationArchive> {
        let runs = run_ids
            .iter()
            .map(|id| self.run(id).cloned())
            .collect::<EvaluationResult<Vec<_>>>()?;
        let aggregates = run_ids
            .iter()
            .map(|id| self.aggregate(id))
            .collect::<EvaluationResult<Vec<_>>>()?;
        let ranking_statuses = runs
            .iter()
            .flat_map(|run| run.episodes.iter())
            .map(|episode| {
                (
                    episode.id.clone(),
                    if episode
                        .steps
                        .iter()
                        .any(|step| step.ranking_trace.is_some())
                    {
                        crate::model::RankingEpisodeStatus::Ranked
                    } else {
                        crate::model::RankingEpisodeStatus::Unranked
                    },
                )
            })
            .collect();
        let has_ranked = runs.iter().any(|run| {
            run.episodes.iter().any(|episode| {
                episode
                    .steps
                    .iter()
                    .any(|step| step.ranking_trace.is_some())
            })
        });
        let feature_schemas = if has_ranked {
            vec![feature_schema_v1()?]
        } else {
            Vec::new()
        };
        let mut policy_by_hash = BTreeMap::new();
        policy_by_hash.extend(self.ranking_policy_registry.clone());
        let mut choice_set_by_hash = BTreeMap::new();
        for run in &runs {
            let seed = run
                .policy
                .configuration
                .get("ranking_seed")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            for policy in self.ranking_policies(seed)? {
                policy_by_hash.insert(policy.ranking_policy_hash.clone(), policy);
            }
            for step in run.episodes.iter().flat_map(|episode| &episode.steps) {
                if let Some(trace) = &step.ranking_trace {
                    let set = evaluation_choice_set_from_observation(&step.observation)?;
                    if set.choice_set_hash != trace.choice_set_hash {
                        return Err(EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationChoiceSetMismatch,
                            "recorded ranking trace differs from its observation choice set",
                        ));
                    }
                    choice_set_by_hash.insert(set.choice_set_hash.clone(), set);
                }
            }
        }
        policy_by_hash.retain(|hash, _| {
            runs.iter().any(|run| {
                run.episodes
                    .iter()
                    .flat_map(|episode| &episode.steps)
                    .any(|step| {
                        step.ranking_trace
                            .as_ref()
                            .is_some_and(|trace| &trace.ranking_policy_hash == hash)
                    })
            })
        });
        let learning_statuses = runs
            .iter()
            .flat_map(|run| run.episodes.iter())
            .map(|episode| (episode.id.clone(), LearningEpisodeStatus::Unlearned))
            .collect();
        let mut archive = EvaluationArchive {
            manifest: EvaluationManifest {
                format: "agentir.evaluation.archive".to_owned(),
                version: 7,
                corpus_version: self.corpus.version.clone(),
                corpus_hash: self.corpus.corpus_hash.clone(),
                compiler_build_hash: compiler_build_hash().to_string(),
                source_workspace_hashes: Vec::new(),
                aggregation_configuration: BTreeMap::from([
                    ("percentile".to_owned(), json!("nearest_rank_v1")),
                    ("weighted_overall_score".to_owned(), json!(false)),
                ]),
            },
            corpus: self.corpus.clone(),
            runs,
            aggregates,
            feature_schemas,
            ranking_policies: policy_by_hash.into_values().collect(),
            choice_sets: choice_set_by_hash.into_values().collect(),
            ranking_statuses,
            ranking_datasets: Vec::new(),
            dataset_splits: Vec::new(),
            training_configurations: Vec::new(),
            training_runs: Vec::new(),
            learned_models: Vec::new(),
            ranking_inputs: Vec::new(),
            inference_records: Vec::new(),
            learning_statuses,
            search_history_status: SearchHistoryStatus::NoSearchHistory,
            search_objectives: Vec::new(),
            search_plans: Vec::new(),
            search_runs: Vec::new(),
            search_nodes: Vec::new(),
            search_edges: Vec::new(),
            search_checkpoints: Vec::new(),
            search_traces: Vec::new(),
            search_results: Vec::new(),
            search_rejections: Vec::new(),
            search_work_counters: Vec::new(),
            measured_search_history_status: MeasuredSearchHistoryStatus::NoMeasuredSearchHistory,
            measurement_cohorts: Vec::new(),
            measured_objectives: Vec::new(),
            measured_search_runs: Vec::new(),
            measured_recommendations: Vec::new(),
            measurement_acquisition_history_status:
                MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory,
            measurement_acquisition_plans: Vec::new(),
            measurement_acquisition_sessions: Vec::new(),
            measurement_acquisition_checkpoints: Vec::new(),
            measurement_acquisition_traces: Vec::new(),
            measurement_acquisition_results: Vec::new(),
            measurement_acquisition_records: Vec::new(),
            measurement_acquisition_cohort_links: BTreeMap::new(),
            measurement_acquisition_replay_statuses: BTreeMap::new(),
            measurement_acquisition_recovery_history_status:
                MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory,
            measurement_acquisition_recovery_journals: Vec::new(),
            measurement_acquisition_prepared_slots: Vec::new(),
            measurement_acquisition_publication_snapshots: Vec::new(),
            measurement_acquisition_reconciliation_results: Vec::new(),
            measurement_acquisition_retry_authorizations: Vec::new(),
            measurement_acquisition_recovery_checkpoints: Vec::new(),
            measurement_acquisition_recovery_work_counters: Vec::new(),
            measurement_acquisition_recovery_replay_statuses: BTreeMap::new(),
            archive_hash: String::new(),
        };
        archive.archive_hash = archive_hash(&archive)?;
        Ok(archive)
    }

    /// Atomically saves a separate evaluation archive.
    pub fn save_archive(&self, path: &Path, run_ids: &[String]) -> EvaluationResult<String> {
        let archive = self.archive(run_ids)?;
        let bytes = serde_json::to_vec(&archive).map_err(|error| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                format!("evaluation archive encoding failed: {error}"),
            )
        })?;
        limit(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            self.limits.archive_bytes,
            "evaluation_archive_bytes",
        )?;
        let temporary = path.with_extension("agentir-evaluation.tmp");
        fs::write(&temporary, bytes).map_err(|error| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                format!("evaluation archive temporary write failed: {error}"),
            )
        })?;
        fs::rename(&temporary, path).map_err(|error| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                format!("evaluation archive atomic rename failed: {error}"),
            )
        })?;
        Ok(archive.archive_hash)
    }

    /// Loads and verifies a separate archive without model, network, device, or benchmark calls.
    pub fn load_archive(
        path: &Path,
        limits: &EvaluationLimits,
    ) -> EvaluationResult<EvaluationArchive> {
        let bytes = fs::read(path).map_err(|error| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                format!("evaluation archive read failed: {error}"),
            )
        })?;
        limit(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            limits.archive_bytes,
            "evaluation_archive_bytes",
        )?;
        let archive: EvaluationArchive = serde_json::from_slice(&bytes).map_err(|error| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                format!("evaluation archive decode failed: {error}"),
            )
        })?;
        let archive = match archive.manifest.version {
            1 => migrate_archive_v6_to_v7(&migrate_archive_v5_to_v6(&migrate_archive_v4_to_v5(
                &migrate_archive_v3_to_v4(&migrate_archive_v2_to_v3(&migrate_archive_v1_to_v2(
                    &archive,
                )?)?)?,
            )?)?)?,
            2 => migrate_archive_v6_to_v7(&migrate_archive_v5_to_v6(&migrate_archive_v4_to_v5(
                &migrate_archive_v3_to_v4(&migrate_archive_v2_to_v3(&archive)?)?,
            )?)?)?,
            3 => migrate_archive_v6_to_v7(&migrate_archive_v5_to_v6(&migrate_archive_v4_to_v5(
                &migrate_archive_v3_to_v4(&archive)?,
            )?)?)?,
            4 => migrate_archive_v6_to_v7(&migrate_archive_v5_to_v6(&migrate_archive_v4_to_v5(
                &archive,
            )?)?)?,
            5 => migrate_archive_v6_to_v7(&migrate_archive_v5_to_v6(&archive)?)?,
            6 => migrate_archive_v6_to_v7(&archive)?,
            _ => archive,
        };
        verify_archive(&archive)?;
        for run in &archive.runs {
            replay_recorded_run(run, &archive.ranking_policies)?;
        }
        Ok(archive)
    }

    /// Atomically imports fully verified completed runs from an evaluation archive.
    pub fn import_archive(&mut self, path: &Path) -> EvaluationResult<EvaluationArchive> {
        let archive = Self::load_archive(path, &self.limits)?;
        if archive.corpus.corpus_hash != self.corpus.corpus_hash {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "loaded evaluation corpus does not match the active harness corpus",
            )
            .expected_actual(
                json!(self.corpus.corpus_hash),
                json!(archive.corpus.corpus_hash),
            ));
        }
        let mut staged = self.runs.clone();
        for run in &archive.runs {
            if staged.insert(run.id.clone(), run.clone()).is_some() {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    format!("evaluation run `{}` is already retained", run.id),
                ));
            }
        }
        limit(
            u64::try_from(staged.len()).unwrap_or(u64::MAX),
            self.limits.runs,
            "runs",
        )?;
        self.next_run = staged
            .keys()
            .filter_map(|id| id.strip_prefix("run")?.parse::<u64>().ok())
            .max()
            .unwrap_or(self.next_run)
            .max(self.next_run);
        self.runs = staged;
        Ok(archive)
    }
}

fn finalize_episode(
    episode: &mut EvaluationEpisode,
    task: &EvaluationTask,
) -> EvaluationResult<()> {
    let semantic = match &task.success_criterion {
        TaskSuccessCriterion::AllActionsAccepted => {
            episode.steps.iter().all(|step| step.outcome.accepted)
        }
        TaskSuccessCriterion::JsonPointerEquals { pointer, expected } => {
            episode
                .steps
                .last()
                .and_then(|step| step.outcome.response.pointer(pointer))
                == Some(expected)
        }
        TaskSuccessCriterion::RejectionThenAccepted { rejection_code } => {
            episode.steps.iter().enumerate().any(|(index, step)| {
                step.outcome.compiler_error_code.as_deref() == Some(rejection_code)
                    && episode
                        .steps
                        .iter()
                        .skip(index.saturating_add(1))
                        .any(|later| later.outcome.accepted)
            })
        }
    };
    let repair_cycles = repair_cycles(&episode.steps);
    let accepted = u64::try_from(
        episode
            .steps
            .iter()
            .filter(|step| step.outcome.accepted)
            .count(),
    )
    .unwrap_or(u64::MAX);
    let rejected = u64::try_from(episode.steps.len())
        .unwrap_or(u64::MAX)
        .saturating_sub(accepted);
    let context = episode.steps.iter().fold(0_u64, |total, step| {
        total.saturating_add(step.context.observation_bytes)
    });
    let deterministic_tokens = episode.steps.iter().try_fold(0_u64, |total, step| {
        step.context
            .deterministic_tokens
            .map(|tokens| total.saturating_add(tokens))
    });
    let provider_input = sum_reported(&episode.steps, true);
    let provider_output = sum_reported(&episode.steps, false);
    let budget_exhausted = matches!(episode.status, EpisodeStatus::BudgetExhausted);
    episode.result = Some(EpisodeResult {
        success: semantic && !budget_exhausted,
        semantic: SemanticResult {
            criterion_satisfied: semantic,
            checked_by: "agentir_policy_eval::task_success_v1".to_owned(),
            final_response: episode
                .steps
                .last()
                .map(|step| step.outcome.response.clone()),
        },
        total_decisions: u64::try_from(episode.steps.len()).unwrap_or(u64::MAX),
        accepted_decisions: accepted,
        rejected_decisions: rejected,
        no_op_decisions: u64::try_from(
            episode
                .steps
                .iter()
                .filter(|step| step.outcome.accepted && !step.outcome.progress_producing)
                .count(),
        )
        .unwrap_or(u64::MAX),
        repair_cycles,
        context_bytes: context,
        deterministic_tokens,
        provider_input_tokens: provider_input,
        provider_output_tokens: provider_output,
        budget_exhausted,
        performance: None,
    });
    episode.status = if semantic && !budget_exhausted {
        EpisodeStatus::Succeeded
    } else if budget_exhausted {
        EpisodeStatus::BudgetExhausted
    } else {
        EpisodeStatus::Failed
    };
    episode.episode_hash = Some(episode_hash(episode)?);
    Ok(())
}

fn replay_recorded_run(
    run: &EvaluationRun,
    retained_policies: &[RankingPolicyDescriptor],
) -> EvaluationResult<()> {
    let schema = feature_schema_v1()?;
    let ranking_seed = run
        .policy
        .configuration
        .get("ranking_seed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut ranking_policies = retained_policies.to_vec();
    let scripted = [
        "lexicographic_choice_v1",
        "first_progress_choice_v1",
        "goal_directed_rule_v1",
        "proof_frontier_first_v1",
        "min_context_choice_v1",
        "seeded_uniform_choice_v1",
        "hybrid_ranked_escape_v1",
    ]
    .into_iter()
    .map(|name| scripted_ranker(name, &schema, ranking_seed))
    .collect::<EvaluationResult<Vec<_>>>()?;
    for policy in scripted {
        if !ranking_policies
            .iter()
            .any(|retained| retained.ranking_policy_hash == policy.ranking_policy_hash)
        {
            ranking_policies.push(policy);
        }
    }
    for episode in &run.episodes {
        let mut compiler = CompilerEngine::new();
        for (index, recorded) in episode.steps.iter().enumerate() {
            if recorded.ordinal != u64::try_from(index).unwrap_or(u64::MAX) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationEventOrderInvalid,
                    "episode step order is not contiguous",
                ));
            }
            if let Some(trace) = &recorded.ranking_trace {
                let choice_set = evaluation_choice_set_from_observation(&recorded.observation)?;
                let policy = ranking_policies
                    .iter()
                    .find(|policy| policy.ranking_policy_hash == trace.ranking_policy_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationRankingPolicyNotFound,
                            "recorded ranking policy is unavailable during replay",
                        )
                    })?;
                crate::ranking::replay_ranking_trace(
                    &choice_set,
                    policy,
                    trace,
                    &RankingLimits::default(),
                )?;
            }
            let action = match &recorded.decision {
                PolicyDecision::MenuChoice { choice_id } => recorded
                    .observation
                    .continuation_frame
                    .iter()
                    .find(|choice| &choice.choice_id == choice_id)
                    .map(|choice| choice.action.clone()),
                PolicyDecision::Action { action, .. } => Some(action.clone()),
                PolicyDecision::Finish => None,
            }
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationTranscriptInvalid,
                    "recorded decision cannot be resolved to a production action",
                )
            })?;
            let response = response_value(&mut compiler, &action)?;
            if response != recorded.outcome.response {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationReplayMismatch,
                    "production compiler outcome differs during replay",
                )
                .expected_actual(recorded.outcome.response.clone(), response));
            }
            if let Some(selection) = &recorded.selection {
                if selection.compiler_outcome != recorded.outcome
                    || selection.selection_hash != crate::ranking::selection_hash(selection)?
                {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationRankingReplayMismatch,
                        "recorded selection differs during replay",
                    ));
                }
            }
        }
        let calculated = episode_hash(episode)?;
        if episode.episode_hash.as_deref() != Some(calculated.as_str()) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "episode hash differs during replay",
            ));
        }
    }
    Ok(())
}

fn sum_reported(steps: &[EpisodeStep], input: bool) -> Option<u64> {
    steps.iter().try_fold(0_u64, |total, step| {
        let report = step.context.reported_usage.as_ref()?;
        if !matches!(report.trust, UsageTrust::ProviderReported) {
            return Some(total);
        }
        let value = if input {
            report.input_tokens
        } else {
            report.output_tokens
        }?;
        Some(total.saturating_add(value))
    })
}

fn repair_cycles(steps: &[EpisodeStep]) -> Vec<RepairCycle> {
    let mut cycles = Vec::new();
    let mut start = None;
    let mut first_code: Option<&str> = None;
    let mut repeated = false;
    for (index, step) in steps.iter().enumerate() {
        if !step.outcome.accepted {
            if start.is_none() {
                start = Some(index);
                first_code = step.outcome.compiler_error_code.as_deref();
            } else if step.outcome.compiler_error_code.as_deref() == first_code {
                repeated = true;
            }
        } else if step.outcome.progress_producing {
            if let Some(begin) = start.take() {
                cycles.push(RepairCycle {
                    start_step: u64::try_from(begin).unwrap_or(u64::MAX),
                    end_step: u64::try_from(index).unwrap_or(u64::MAX),
                    length: u64::try_from(index.saturating_sub(begin)).unwrap_or(u64::MAX),
                    repeated_identical_rejection: repeated,
                });
                first_code = None;
                repeated = false;
            }
        }
    }
    if let Some(begin) = start {
        let end = steps.len().saturating_sub(1);
        cycles.push(RepairCycle {
            start_step: u64::try_from(begin).unwrap_or(u64::MAX),
            end_step: u64::try_from(end).unwrap_or(u64::MAX),
            length: u64::try_from(steps.len().saturating_sub(begin)).unwrap_or(u64::MAX),
            repeated_identical_rejection: repeated,
        });
    }
    cycles
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = percentile.saturating_mul(sorted.len()).saturating_add(99) / 100;
    sorted[rank
        .max(1)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1))]
}

/// Verifies every independent hash and archive structural invariant.
pub fn verify_archive(archive: &EvaluationArchive) -> EvaluationResult<()> {
    if archive.manifest.format != "agentir.evaluation.archive"
        || !matches!(archive.manifest.version, 1..=7)
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "unsupported evaluation archive format or version",
        ));
    }
    if archive.archive_hash != archive_hash(archive)?
        || archive.corpus.corpus_hash != corpus_hash(&archive.corpus)?
        || archive.manifest.corpus_hash != archive.corpus.corpus_hash
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationHashMismatch,
            "evaluation archive envelope or corpus hash mismatch",
        ));
    }
    let mut episode_ids = BTreeSet::new();
    for run in &archive.runs {
        if run.policy.policy_hash != policy_hash(&run.policy)? {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "policy hash mismatch",
            ));
        }
        for episode in &run.episodes {
            if !episode_ids.insert(episode.id.clone()) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationTranscriptInvalid,
                    "duplicated episode ID in evaluation archive",
                ));
            }
            if episode.episode_hash.as_deref() != Some(episode_hash(episode)?.as_str()) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationHashMismatch,
                    "episode hash mismatch",
                ));
            }
            for (index, step) in episode.steps.iter().enumerate() {
                if step.ordinal != u64::try_from(index).unwrap_or(u64::MAX)
                    || step.observation.observation_hash != observation_hash(&step.observation)?
                    || step.context.observation_bytes != step.observation.context_bytes
                    || step.context.deterministic_tokens != step.observation.deterministic_tokens
                {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationEventOrderInvalid,
                        "step order, observation hash, or budget accounting mismatch",
                    ));
                }
                match (&step.ranking_trace, &step.selection) {
                    (None, None) => {}
                    (Some(trace), Some(selection)) => {
                        if !matches!(archive.manifest.version, 2..=7)
                            || trace.ranking_trace_hash
                                != crate::ranking::ranking_trace_hash(trace)?
                            || selection.selection_hash
                                != crate::ranking::selection_hash(selection)?
                            || selection.compiler_outcome != step.outcome
                        {
                            return Err(EvaluationDiagnostic::new(
                                EvaluationErrorCode::EvaluationRankingTraceInvalid,
                                "ranking/selection record is invalid or mismatched",
                            ));
                        }
                    }
                    _ => {
                        return Err(EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationRankingTraceInvalid,
                            "ranking trace and selection record must occur together",
                        ));
                    }
                }
            }
        }
        if run.evaluation_hash.as_deref() != Some(evaluation_hash(run)?.as_str()) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "evaluation hash mismatch",
            ));
        }
    }
    if archive.manifest.version <= 6 && has_recovery_fields(archive) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "evaluation archives v1-v6 cannot contain Stage 7D recovery fields",
        ));
    }
    if archive.manifest.version == 1 {
        if !archive.feature_schemas.is_empty()
            || !archive.ranking_policies.is_empty()
            || !archive.choice_sets.is_empty()
            || !archive.ranking_statuses.is_empty()
            || has_learned_fields(archive)
            || has_search_fields(archive)
            || archive.search_history_status != SearchHistoryStatus::Unspecified
            || has_measured_fields(archive)
            || archive.measured_search_history_status != MeasuredSearchHistoryStatus::Unspecified
            || has_acquisition_fields(archive)
            || archive.measurement_acquisition_history_status
                != MeasurementAcquisitionHistoryStatus::Unspecified
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "evaluation archive v1 cannot contain Stage 6B fields",
            ));
        }
    } else {
        verify_archive_v2_ranking(archive, &episode_ids)?;
        if archive.manifest.version == 2 {
            if has_learned_fields(archive)
                || !archive.learning_statuses.is_empty()
                || has_search_fields(archive)
                || archive.search_history_status != SearchHistoryStatus::Unspecified
                || has_measured_fields(archive)
                || archive.measured_search_history_status
                    != MeasuredSearchHistoryStatus::Unspecified
                || has_acquisition_fields(archive)
                || archive.measurement_acquisition_history_status
                    != MeasurementAcquisitionHistoryStatus::Unspecified
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "evaluation archive v2 cannot contain Stage 6C fields",
                ));
            }
        } else {
            verify_archive_v3_learning(archive, &episode_ids)?;
            if archive.manifest.version == 3 {
                if has_search_fields(archive)
                    || archive.search_history_status != SearchHistoryStatus::Unspecified
                    || has_measured_fields(archive)
                    || archive.measured_search_history_status
                        != MeasuredSearchHistoryStatus::Unspecified
                    || has_acquisition_fields(archive)
                    || archive.measurement_acquisition_history_status
                        != MeasurementAcquisitionHistoryStatus::Unspecified
                {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationArchiveInvalid,
                        "evaluation archive v3 cannot contain Stage 7A fields",
                    ));
                }
            } else if archive.manifest.version == 4 {
                verify_archive_v4_search(archive)?;
                if has_measured_fields(archive)
                    || archive.measured_search_history_status
                        != MeasuredSearchHistoryStatus::Unspecified
                    || has_acquisition_fields(archive)
                    || archive.measurement_acquisition_history_status
                        != MeasurementAcquisitionHistoryStatus::Unspecified
                {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationArchiveInvalid,
                        "evaluation archive v4 cannot contain Stage 7B fields",
                    ));
                }
            } else {
                verify_archive_v4_search(archive)?;
                verify_archive_v5_measured(archive)?;
                if archive.manifest.version == 5 {
                    if has_acquisition_fields(archive)
                        || archive.measurement_acquisition_history_status
                            != MeasurementAcquisitionHistoryStatus::Unspecified
                    {
                        return Err(EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationArchiveInvalid,
                            "evaluation archive v5 cannot contain Stage 7C fields",
                        ));
                    }
                } else if archive.manifest.version == 6 {
                    verify_archive_v6_acquisition(archive)?;
                } else {
                    verify_archive_v6_acquisition(archive)?;
                    verify_archive_v7_recovery(archive)?;
                }
            }
        }
    }
    for aggregate in &archive.aggregates {
        if aggregate.aggregate_hash != aggregate_hash(aggregate)? {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "aggregate hash mismatch",
            ));
        }
    }
    let mut recomputer = EvaluationHarness::with_corpus_and_limits(
        archive.corpus.clone(),
        EvaluationLimits::default(),
    )?;
    for run in &archive.runs {
        recomputer.runs.insert(run.id.clone(), run.clone());
    }
    for aggregate in &archive.aggregates {
        if &recomputer.aggregate(&aggregate.run_id)? != aggregate {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMetricInvalid,
                "stored aggregate differs from exact episode recomputation",
            ));
        }
    }
    Ok(())
}

/// Pure explicit migration from immutable evaluation archive v1 to v2.
pub fn migrate_archive_v1_to_v2(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 1 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 1",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 2;
    migrated.feature_schemas.clear();
    migrated.ranking_policies.clear();
    migrated.choice_sets.clear();
    migrated.ranking_statuses = migrated
        .runs
        .iter()
        .flat_map(|run| run.episodes.iter())
        .map(|episode| {
            (
                episode.id.clone(),
                crate::model::RankingEpisodeStatus::Unranked,
            )
        })
        .collect();
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

/// Pure explicit migration from immutable evaluation archive v2 to v3.
pub fn migrate_archive_v2_to_v3(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 2 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 2",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 3;
    migrated.ranking_datasets.clear();
    migrated.dataset_splits.clear();
    migrated.training_configurations.clear();
    migrated.training_runs.clear();
    migrated.learned_models.clear();
    migrated.ranking_inputs.clear();
    migrated.inference_records.clear();
    migrated.learning_statuses = migrated
        .runs
        .iter()
        .flat_map(|run| run.episodes.iter())
        .map(|episode| (episode.id.clone(), LearningEpisodeStatus::Unlearned))
        .collect();
    migrated.search_history_status = SearchHistoryStatus::Unspecified;
    migrated.search_objectives.clear();
    migrated.search_plans.clear();
    migrated.search_runs.clear();
    migrated.search_nodes.clear();
    migrated.search_edges.clear();
    migrated.search_checkpoints.clear();
    migrated.search_traces.clear();
    migrated.search_results.clear();
    migrated.search_rejections.clear();
    migrated.search_work_counters.clear();
    migrated.measured_search_history_status = MeasuredSearchHistoryStatus::Unspecified;
    migrated.measurement_cohorts.clear();
    migrated.measured_objectives.clear();
    migrated.measured_search_runs.clear();
    migrated.measured_recommendations.clear();
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

/// Pure explicit migration from immutable evaluation archive v3 to v4.
pub fn migrate_archive_v3_to_v4(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 3 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 3",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 4;
    migrated.search_history_status = SearchHistoryStatus::NoSearchHistory;
    migrated.search_objectives.clear();
    migrated.search_plans.clear();
    migrated.search_runs.clear();
    migrated.search_nodes.clear();
    migrated.search_edges.clear();
    migrated.search_checkpoints.clear();
    migrated.search_traces.clear();
    migrated.search_results.clear();
    migrated.search_rejections.clear();
    migrated.search_work_counters.clear();
    migrated.measured_search_history_status = MeasuredSearchHistoryStatus::Unspecified;
    migrated.measurement_cohorts.clear();
    migrated.measured_objectives.clear();
    migrated.measured_search_runs.clear();
    migrated.measured_recommendations.clear();
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

/// Pure explicit migration from immutable evaluation archive v4 to v5.
pub fn migrate_archive_v4_to_v5(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 4 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 4",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 5;
    migrated.measured_search_history_status = MeasuredSearchHistoryStatus::NoMeasuredSearchHistory;
    migrated.measurement_cohorts.clear();
    migrated.measured_objectives.clear();
    migrated.measured_search_runs.clear();
    migrated.measured_recommendations.clear();
    clear_acquisition_fields(&mut migrated);
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

/// Pure explicit migration from immutable evaluation archive v5 to v6.
pub fn migrate_archive_v5_to_v6(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 5 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 5",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 6;
    clear_acquisition_fields(&mut migrated);
    migrated.measurement_acquisition_history_status =
        MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory;
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

/// Pure explicit migration from immutable evaluation archive v6 to v7.
pub fn migrate_archive_v6_to_v7(source: &EvaluationArchive) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 6 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "evaluation archive migration requires exact source version 6",
        ));
    }
    verify_archive(source)?;
    let mut migrated = source.clone();
    migrated.manifest.version = 7;
    clear_recovery_fields(&mut migrated);
    migrated.measurement_acquisition_recovery_history_status =
        MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory;
    migrated.archive_hash.clear();
    migrated.archive_hash = archive_hash(&migrated)?;
    verify_archive(&migrated)?;
    Ok(migrated)
}

fn clear_acquisition_fields(archive: &mut EvaluationArchive) {
    archive.measurement_acquisition_history_status =
        MeasurementAcquisitionHistoryStatus::Unspecified;
    archive.measurement_acquisition_plans.clear();
    archive.measurement_acquisition_sessions.clear();
    archive.measurement_acquisition_checkpoints.clear();
    archive.measurement_acquisition_traces.clear();
    archive.measurement_acquisition_results.clear();
    archive.measurement_acquisition_records.clear();
    archive.measurement_acquisition_cohort_links.clear();
    archive.measurement_acquisition_replay_statuses.clear();
    clear_recovery_fields(archive);
}

fn clear_recovery_fields(archive: &mut EvaluationArchive) {
    archive.measurement_acquisition_recovery_history_status =
        MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory;
    archive.measurement_acquisition_recovery_journals.clear();
    archive.measurement_acquisition_prepared_slots.clear();
    archive
        .measurement_acquisition_publication_snapshots
        .clear();
    archive
        .measurement_acquisition_reconciliation_results
        .clear();
    archive.measurement_acquisition_retry_authorizations.clear();
    archive.measurement_acquisition_recovery_checkpoints.clear();
    archive
        .measurement_acquisition_recovery_work_counters
        .clear();
    archive
        .measurement_acquisition_recovery_replay_statuses
        .clear();
}

/// Atomically attaches verified Stage 6C artifacts to evaluation archive v4.
pub fn attach_learning_artifacts(
    source: &EvaluationArchive,
    bundle: LearnedArchiveBundle,
) -> EvaluationResult<EvaluationArchive> {
    if !matches!(source.manifest.version, 4..=7) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "learned artifacts require evaluation archive v4, v5, v6, or v7",
        ));
    }
    verify_archive(source)?;
    let mut archive = source.clone();
    archive.ranking_datasets.extend(bundle.datasets);
    archive.dataset_splits.extend(bundle.splits);
    archive
        .training_configurations
        .extend(bundle.configurations);
    archive.training_runs.extend(bundle.training_runs);
    archive.learned_models.extend(bundle.models);
    archive.ranking_inputs.extend(bundle.inputs);
    archive.inference_records.extend(bundle.inferences);
    archive.ranking_policies.extend(bundle.policies);
    sort_stage6c_artifacts(&mut archive);
    let learned_frames = archive
        .inference_records
        .iter()
        .map(|record| {
            (
                record.choice_set_hash.as_str(),
                record.ranking_policy_hash.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    archive.learning_statuses = archive
        .runs
        .iter()
        .flat_map(|run| run.episodes.iter())
        .map(|episode| {
            let learned = episode.steps.iter().any(|step| {
                step.ranking_trace.as_ref().is_some_and(|trace| {
                    learned_frames.contains(&(
                        trace.choice_set_hash.as_str(),
                        trace.ranking_policy_hash.as_str(),
                    ))
                })
            });
            (
                episode.id.clone(),
                if learned {
                    LearningEpisodeStatus::Learned
                } else {
                    LearningEpisodeStatus::Unlearned
                },
            )
        })
        .collect();
    archive.archive_hash.clear();
    archive.archive_hash = archive_hash(&archive)?;
    verify_archive(&archive)?;
    Ok(archive)
}

/// Atomically attaches stopped, structurally valid Stage 7A sessions and checkpoints.
pub fn attach_search_artifacts(
    source: &EvaluationArchive,
    artifacts: &[(SearchSession, SearchCheckpoint)],
) -> EvaluationResult<EvaluationArchive> {
    if !matches!(source.manifest.version, 4..=7) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "search artifacts require evaluation archive v4, v5, v6, or v7",
        ));
    }
    verify_archive(source)?;
    let mut archive = source.clone();
    for (session, checkpoint) in artifacts {
        if session.status == crate::search::SearchStatus::Running
            || checkpoint.session != *session
            || checkpoint.search_checkpoint_hash != search_checkpoint_hash(checkpoint)?
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSearchIncomplete,
                "only stopped search sessions with exact final checkpoints can be archived",
            ));
        }
        let mut record = session.run_record();
        record.checkpoint_hashes = vec![checkpoint.search_checkpoint_hash.clone()];
        if !archive
            .feature_schemas
            .iter()
            .any(|schema| schema.feature_schema_hash == session.ranking_policy.feature_schema_hash)
        {
            let schema = feature_schema_v1()?;
            if schema.feature_schema_hash != session.ranking_policy.feature_schema_hash {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
                    "search ranking policy references an unavailable visible feature schema",
                ));
            }
            archive.feature_schemas.push(schema);
        }
        if !archive
            .ranking_policies
            .iter()
            .any(|policy| policy.ranking_policy_hash == session.ranking_policy.ranking_policy_hash)
        {
            archive
                .ranking_policies
                .push(session.ranking_policy.clone());
        }
        archive.search_objectives.push(session.objective.clone());
        archive.search_plans.push(session.plan.clone());
        archive.search_runs.push(record);
        archive.search_nodes.extend(session.nodes.values().cloned());
        archive.search_edges.extend(session.edges.values().cloned());
        archive.search_checkpoints.push(checkpoint.clone());
        if let Some(trace) = &session.trace {
            archive.search_traces.push(trace.clone());
        }
        if let Some(result) = &session.result {
            archive.search_results.push(result.clone());
        }
        if let Some(rejection) = &session.rejection {
            archive.search_rejections.push(rejection.clone());
        }
        archive.search_work_counters.push(session.work.clone());
    }
    archive.search_history_status = if archive.search_runs.is_empty() {
        SearchHistoryStatus::NoSearchHistory
    } else {
        SearchHistoryStatus::SearchHistoryPresent
    };
    archive
        .search_objectives
        .sort_by(|left, right| left.search_objective_hash.cmp(&right.search_objective_hash));
    archive
        .search_objectives
        .dedup_by(|left, right| left.search_objective_hash == right.search_objective_hash);
    archive
        .search_plans
        .sort_by(|left, right| left.search_plan_hash.cmp(&right.search_plan_hash));
    archive
        .search_plans
        .dedup_by(|left, right| left.search_plan_hash == right.search_plan_hash);
    archive
        .feature_schemas
        .sort_by(|left, right| left.feature_schema_hash.cmp(&right.feature_schema_hash));
    archive
        .feature_schemas
        .dedup_by(|left, right| left.feature_schema_hash == right.feature_schema_hash);
    archive
        .ranking_policies
        .sort_by(|left, right| left.ranking_policy_hash.cmp(&right.ranking_policy_hash));
    archive
        .ranking_policies
        .dedup_by(|left, right| left.ranking_policy_hash == right.ranking_policy_hash);
    archive
        .search_runs
        .sort_by(|left, right| left.search_run_id.cmp(&right.search_run_id));
    archive
        .search_nodes
        .sort_by(|left, right| left.id.cmp(&right.id));
    archive
        .search_edges
        .sort_by(|left, right| left.id.cmp(&right.id));
    archive
        .search_traces
        .sort_by(|left, right| left.search_trace_hash.cmp(&right.search_trace_hash));
    archive
        .search_results
        .sort_by(|left, right| left.search_result_hash.cmp(&right.search_result_hash));
    archive.archive_hash.clear();
    archive.archive_hash = archive_hash(&archive)?;
    verify_archive(&archive)?;
    Ok(archive)
}

/// Atomically attaches verified Stage 7B cohorts, objectives, and replayed recommendations.
pub fn attach_measured_search_artifacts(
    source: &EvaluationArchive,
    artifacts: &[(
        MeasurementCohort,
        MeasuredObjectiveDescriptor,
        MeasuredRecommendation,
    )],
) -> EvaluationResult<EvaluationArchive> {
    if !matches!(source.manifest.version, 5..=7) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "measured search artifacts require evaluation archive v5, v6, or v7",
        ));
    }
    verify_archive(source)?;
    let sessions = source
        .search_checkpoints
        .iter()
        .filter_map(|checkpoint| {
            checkpoint
                .session
                .result
                .as_ref()
                .map(|result| (result.search_result_hash.as_str(), &checkpoint.session))
        })
        .collect::<BTreeMap<_, _>>();
    let mut archive = source.clone();
    for (cohort, objective, retained) in artifacts {
        verify_measurement_cohort(cohort)?;
        objective.validate(cohort)?;
        let session = sessions
            .get(retained.search_result_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                    "recommendation references a missing Stage 7A result",
                )
            })?;
        let replayed = measured_recommendation(session, cohort, objective)?;
        if &replayed != retained
            || retained.measured_recommendation_hash != measured_recommendation_hash(retained)?
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMeasuredRecommendationCorrupt,
                "recommendation differs from exact offline replay",
            ));
        }
        archive.measurement_cohorts.push(cohort.clone());
        archive.measured_objectives.push(objective.clone());
        archive.measured_recommendations.push(retained.clone());
        archive.measured_search_runs.push(MeasuredSearchRunRecord {
            search_result_hash: retained.search_result_hash.clone(),
            measurement_cohort_hash: cohort.measurement_cohort_hash.clone(),
            measured_objective_hash: objective.measured_objective_hash.clone(),
            measured_recommendation_hash: retained.measured_recommendation_hash.clone(),
            replay_verified: true,
            work: MeasuredSearchWorkCounters {
                measurement_records_inspected: u64::try_from(cohort.records.len())
                    .unwrap_or(u64::MAX),
                objective_aggregations: u64::try_from(retained.eligible_alternatives.len())
                    .unwrap_or(u64::MAX),
                indifference_comparisons: u64::try_from(
                    retained.eligible_alternatives.len().saturating_sub(1),
                )
                .unwrap_or(u64::MAX),
                recommendation_replays: 1,
                hardware_calls: 0,
            },
        });
    }
    archive.measurement_cohorts.sort_by(|left, right| {
        left.measurement_cohort_hash
            .cmp(&right.measurement_cohort_hash)
    });
    archive
        .measurement_cohorts
        .dedup_by(|left, right| left.measurement_cohort_hash == right.measurement_cohort_hash);
    archive.measured_objectives.sort_by(|left, right| {
        left.measured_objective_hash
            .cmp(&right.measured_objective_hash)
    });
    archive
        .measured_objectives
        .dedup_by(|left, right| left.measured_objective_hash == right.measured_objective_hash);
    archive.measured_recommendations.sort_by(|left, right| {
        left.measured_recommendation_hash
            .cmp(&right.measured_recommendation_hash)
    });
    archive.measured_search_runs.sort_by(|left, right| {
        left.measured_recommendation_hash
            .cmp(&right.measured_recommendation_hash)
    });
    archive.measured_search_history_status = if archive.measured_search_runs.is_empty() {
        MeasuredSearchHistoryStatus::NoMeasuredSearchHistory
    } else {
        MeasuredSearchHistoryStatus::MeasuredSearchHistoryPresent
    };
    archive.archive_hash.clear();
    archive.archive_hash = archive_hash(&archive)?;
    verify_archive(&archive)?;
    Ok(archive)
}

/// Atomically attaches replay-verified Stage 7C acquisition history to archive v6.
pub fn attach_measurement_acquisition_artifacts(
    source: &EvaluationArchive,
    bundle: MeasurementAcquisitionArchiveBundle,
) -> EvaluationResult<EvaluationArchive> {
    if !matches!(source.manifest.version, 6 | 7) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "measurement acquisition artifacts require evaluation archive v6 or v7",
        ));
    }
    verify_archive(source)?;
    if bundle.sessions.len() != bundle.checkpoints.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "acquisition bundle requires one exact checkpoint per session",
        ));
    }
    let records = bundle
        .records
        .iter()
        .map(|entry| (entry.measurement_id.clone(), &entry.record))
        .collect::<BTreeMap<_, _>>();
    if records.len() != bundle.records.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
            "acquisition bundle contains duplicate measurement IDs",
        ));
    }
    let mut archive = source.clone();
    for (session, checkpoint) in bundle.sessions.iter().zip(&bundle.checkpoints) {
        if session.status == crate::acquisition::MeasurementAcquisitionStatus::Running
            || checkpoint.session.as_ref() != session
            || checkpoint.measurement_acquisition_checkpoint_hash
                != measurement_acquisition_checkpoint_hash(checkpoint)?
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                "only stopped acquisition sessions with exact checkpoints may be archived",
            ));
        }
        let result = session.result()?;
        if result.measurement_acquisition_result_hash
            != measurement_acquisition_result_hash(&result)?
            || result.work.replay_hardware_calls != 0
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionResultCorrupt,
                "acquisition result failed zero-device offline replay validation",
            ));
        }
        for (id, hash) in result
            .measurement_ids
            .iter()
            .zip(&result.measurement_hashes)
        {
            let record = records.get(id).ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "acquisition bundle omits a completed production measurement",
                )
            })?;
            if record.measurement_hash.as_str() != hash
                || agentir_core::backend::measurement_hash(record)
                    .map_err(|error| {
                        EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                            error.to_string(),
                        )
                    })?
                    .as_str()
                    != hash
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "acquisition bundle measurement hash is corrupt",
                ));
            }
        }
        archive
            .measurement_acquisition_plans
            .push(session.plan.clone());
        archive
            .measurement_acquisition_sessions
            .push(session.clone());
        archive
            .measurement_acquisition_checkpoints
            .push(checkpoint.clone());
        archive
            .measurement_acquisition_traces
            .push(session.trace.clone());
        archive.measurement_acquisition_results.push(result.clone());
        archive
            .measurement_acquisition_replay_statuses
            .insert(result.measurement_acquisition_result_hash, true);
    }
    archive
        .measurement_acquisition_records
        .extend(bundle.records);
    archive
        .measurement_acquisition_cohort_links
        .extend(bundle.cohort_links);
    archive
        .measurement_acquisition_plans
        .sort_by(|left, right| {
            left.measurement_acquisition_plan_hash
                .cmp(&right.measurement_acquisition_plan_hash)
        });
    archive
        .measurement_acquisition_plans
        .dedup_by(|left, right| {
            left.measurement_acquisition_plan_hash == right.measurement_acquisition_plan_hash
        });
    archive
        .measurement_acquisition_sessions
        .sort_by(|left, right| left.session_id.cmp(&right.session_id));
    archive
        .measurement_acquisition_checkpoints
        .sort_by(|left, right| {
            left.measurement_acquisition_checkpoint_hash
                .cmp(&right.measurement_acquisition_checkpoint_hash)
        });
    archive
        .measurement_acquisition_traces
        .sort_by(|left, right| {
            left.measurement_acquisition_trace_hash
                .cmp(&right.measurement_acquisition_trace_hash)
        });
    archive
        .measurement_acquisition_results
        .sort_by(|left, right| {
            left.measurement_acquisition_result_hash
                .cmp(&right.measurement_acquisition_result_hash)
        });
    archive
        .measurement_acquisition_records
        .sort_by(|left, right| {
            left.record
                .measurement_hash
                .cmp(&right.record.measurement_hash)
        });
    archive.measurement_acquisition_history_status =
        if archive.measurement_acquisition_results.is_empty() {
            MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory
        } else {
            MeasurementAcquisitionHistoryStatus::AcquisitionHistoryPresent
        };
    archive.archive_hash.clear();
    archive.archive_hash = archive_hash(&archive)?;
    verify_archive(&archive)?;
    Ok(archive)
}

/// Atomically attaches verified Stage 7D recovery history to archive v7.
pub fn attach_measurement_acquisition_recovery_artifacts(
    source: &EvaluationArchive,
    bundle: MeasurementAcquisitionRecoveryArchiveBundle,
) -> EvaluationResult<EvaluationArchive> {
    if source.manifest.version != 7 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "measurement acquisition recovery artifacts require evaluation archive v7",
        ));
    }
    verify_archive(source)?;
    if bundle.journals.len() != bundle.checkpoints.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "recovery bundle requires one exact checkpoint per journal",
        ));
    }
    let mut archive = source.clone();
    archive
        .measurement_acquisition_records
        .extend(bundle.records);
    let records = verified_acquisition_records(&archive)?;
    for (journal, checkpoint) in bundle.journals.iter().zip(&bundle.checkpoints) {
        verify_recovery_journal_archive(journal, &records)?;
        if checkpoint.measurement_acquisition_recovery_checkpoint_hash
            != measurement_acquisition_recovery_checkpoint_hash(checkpoint)?
            || checkpoint.journal.as_ref() != journal
            || checkpoint.measurement_acquisition_recovery_journal_hash
                != journal.measurement_acquisition_recovery_journal_hash
            || checkpoint.session.session_id != journal.session_id
            || checkpoint.session.plan.measurement_acquisition_plan_hash
                != journal.measurement_acquisition_plan_hash
            || !bundle
                .replay_statuses
                .get(&journal.measurement_acquisition_recovery_journal_hash)
                .copied()
                .unwrap_or(false)
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
                "recovery checkpoint, session, or zero-device replay status is invalid",
            ));
        }
        archive
            .measurement_acquisition_recovery_journals
            .push(journal.clone());
        archive
            .measurement_acquisition_prepared_slots
            .extend(journal.prepared_slots.clone());
        archive
            .measurement_acquisition_publication_snapshots
            .extend(
                journal
                    .prepared_slots
                    .iter()
                    .map(|prepared| prepared.publication_snapshot.clone()),
            );
        archive
            .measurement_acquisition_reconciliation_results
            .extend(journal.reconciliation_results.clone());
        archive
            .measurement_acquisition_retry_authorizations
            .extend(journal.retry_authorizations.clone());
        archive
            .measurement_acquisition_recovery_checkpoints
            .push(checkpoint.clone());
        archive
            .measurement_acquisition_recovery_work_counters
            .push(journal.work.clone());
    }
    archive
        .measurement_acquisition_recovery_replay_statuses
        .extend(bundle.replay_statuses);
    archive
        .measurement_acquisition_recovery_journals
        .sort_by(|left, right| left.recovery_journal_id.cmp(&right.recovery_journal_id));
    archive.measurement_acquisition_prepared_slots = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .flat_map(|journal| journal.prepared_slots.clone())
        .collect();
    archive
        .measurement_acquisition_prepared_slots
        .sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
    archive.measurement_acquisition_publication_snapshots = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .flat_map(|journal| {
            journal
                .prepared_slots
                .iter()
                .map(|prepared| prepared.publication_snapshot.clone())
        })
        .collect();
    archive.measurement_acquisition_reconciliation_results = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .flat_map(|journal| journal.reconciliation_results.clone())
        .collect();
    archive
        .measurement_acquisition_reconciliation_results
        .sort_by(|left, right| left.reconciliation_id.cmp(&right.reconciliation_id));
    archive.measurement_acquisition_retry_authorizations = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .flat_map(|journal| journal.retry_authorizations.clone())
        .collect();
    archive
        .measurement_acquisition_retry_authorizations
        .sort_by(|left, right| left.authorized_attempt_id.cmp(&right.authorized_attempt_id));
    archive.measurement_acquisition_recovery_work_counters = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .map(|journal| journal.work.clone())
        .collect();
    archive
        .measurement_acquisition_recovery_checkpoints
        .sort_by(|left, right| {
            left.measurement_acquisition_recovery_checkpoint_hash
                .cmp(&right.measurement_acquisition_recovery_checkpoint_hash)
        });
    archive
        .measurement_acquisition_records
        .sort_by(|left, right| left.measurement_id.cmp(&right.measurement_id));
    archive.measurement_acquisition_recovery_history_status =
        if archive.measurement_acquisition_recovery_journals.is_empty() {
            MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory
        } else {
            MeasurementAcquisitionRecoveryHistoryStatus::RecoveryHistoryPresent
        };
    archive.archive_hash.clear();
    archive.archive_hash = archive_hash(&archive)?;
    verify_archive(&archive)?;
    Ok(archive)
}

fn sort_stage6c_artifacts(archive: &mut EvaluationArchive) {
    archive.ranking_datasets.sort_by(|left, right| {
        left.manifest
            .ranking_dataset_hash
            .cmp(&right.manifest.ranking_dataset_hash)
    });
    archive
        .dataset_splits
        .sort_by(|left, right| left.dataset_split_hash.cmp(&right.dataset_split_hash));
    archive.training_configurations.sort_by(|left, right| {
        left.training_configuration_hash
            .cmp(&right.training_configuration_hash)
    });
    archive
        .training_runs
        .sort_by(|left, right| left.training_run_hash.cmp(&right.training_run_hash));
    archive
        .learned_models
        .sort_by(|left, right| left.learned_model_hash.cmp(&right.learned_model_hash));
    archive
        .ranking_inputs
        .sort_by(|left, right| left.ranking_input_hash.cmp(&right.ranking_input_hash));
    archive
        .inference_records
        .sort_by(|left, right| left.inference_hash.cmp(&right.inference_hash));
    archive
        .ranking_policies
        .sort_by(|left, right| left.ranking_policy_hash.cmp(&right.ranking_policy_hash));
}

fn has_learned_fields(archive: &EvaluationArchive) -> bool {
    !archive.ranking_datasets.is_empty()
        || !archive.dataset_splits.is_empty()
        || !archive.training_configurations.is_empty()
        || !archive.training_runs.is_empty()
        || !archive.learned_models.is_empty()
        || !archive.ranking_inputs.is_empty()
        || !archive.inference_records.is_empty()
}

fn has_search_fields(archive: &EvaluationArchive) -> bool {
    !archive.search_objectives.is_empty()
        || !archive.search_plans.is_empty()
        || !archive.search_runs.is_empty()
        || !archive.search_nodes.is_empty()
        || !archive.search_edges.is_empty()
        || !archive.search_checkpoints.is_empty()
        || !archive.search_traces.is_empty()
        || !archive.search_results.is_empty()
        || !archive.search_rejections.is_empty()
        || !archive.search_work_counters.is_empty()
}

fn has_measured_fields(archive: &EvaluationArchive) -> bool {
    !archive.measurement_cohorts.is_empty()
        || !archive.measured_objectives.is_empty()
        || !archive.measured_search_runs.is_empty()
        || !archive.measured_recommendations.is_empty()
}

fn has_acquisition_fields(archive: &EvaluationArchive) -> bool {
    !archive.measurement_acquisition_plans.is_empty()
        || !archive.measurement_acquisition_sessions.is_empty()
        || !archive.measurement_acquisition_checkpoints.is_empty()
        || !archive.measurement_acquisition_traces.is_empty()
        || !archive.measurement_acquisition_results.is_empty()
        || !archive.measurement_acquisition_records.is_empty()
        || !archive.measurement_acquisition_cohort_links.is_empty()
        || !archive.measurement_acquisition_replay_statuses.is_empty()
}

fn has_recovery_fields(archive: &EvaluationArchive) -> bool {
    !archive.measurement_acquisition_recovery_journals.is_empty()
        || !archive.measurement_acquisition_prepared_slots.is_empty()
        || !archive
            .measurement_acquisition_publication_snapshots
            .is_empty()
        || !archive
            .measurement_acquisition_reconciliation_results
            .is_empty()
        || !archive
            .measurement_acquisition_retry_authorizations
            .is_empty()
        || !archive
            .measurement_acquisition_recovery_checkpoints
            .is_empty()
        || !archive
            .measurement_acquisition_recovery_work_counters
            .is_empty()
        || !archive
            .measurement_acquisition_recovery_replay_statuses
            .is_empty()
}

fn verified_acquisition_records(
    archive: &EvaluationArchive,
) -> EvaluationResult<
    BTreeMap<agentir_core::ids::MeasurementId, agentir_core::backend_ir::HardwareMeasurementRecord>,
> {
    let mut records = BTreeMap::new();
    for entry in &archive.measurement_acquisition_records {
        let hash = agentir_core::backend::measurement_hash(&entry.record)
            .map_err(|error| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    error.to_string(),
                )
            })?
            .to_string();
        if hash != entry.record.measurement_hash.to_string()
            || records
                .insert(entry.measurement_id.clone(), entry.record.clone())
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
                "evaluation archive contains corrupt or duplicate acquisition records",
            ));
        }
    }
    Ok(records)
}

fn verify_recovery_journal_archive(
    journal: &crate::recovery::MeasurementAcquisitionRecoveryJournal,
    records: &BTreeMap<
        agentir_core::ids::MeasurementId,
        agentir_core::backend_ir::HardwareMeasurementRecord,
    >,
) -> EvaluationResult<()> {
    if journal.version != 1
        || journal.measurement_acquisition_recovery_journal_hash
            != measurement_acquisition_recovery_journal_hash(journal)?
        || journal.trace.iter().enumerate().any(|(index, event)| {
            event.sequence != u64::try_from(index).unwrap_or(u64::MAX)
                || event.slot_index != journal.slot_index
        })
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
            "archive v7 recovery journal hash or trace is corrupt",
        ));
    }
    let mut prepared_hashes = BTreeSet::new();
    for prepared in &journal.prepared_slots {
        prepared.verify()?;
        if !prepared_hashes.insert(prepared.measurement_acquisition_prepared_slot_hash.as_str())
            || prepared.measurement_acquisition_plan_hash
                != journal.measurement_acquisition_plan_hash
            || prepared.session_id != journal.session_id
            || prepared.slot_index != journal.slot_index
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
                "archive v7 prepared slot is duplicated or stale",
            ));
        }
        for anchor in &prepared.publication_snapshot.records {
            if records
                .get(&anchor.measurement_id)
                .is_none_or(|record| record.measurement_hash.as_str() != anchor.measurement_hash)
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionRecoverySnapshotCorrupt,
                    "archive v7 publication snapshot references a missing record",
                ));
            }
        }
    }
    let mut reconciliation_ids = BTreeSet::new();
    for result in &journal.reconciliation_results {
        if !reconciliation_ids.insert(result.reconciliation_id.as_str())
            || !prepared_hashes.contains(result.measurement_acquisition_prepared_slot_hash.as_str())
            || result.measurement_acquisition_reconciliation_hash
                != measurement_acquisition_reconciliation_hash(result)?
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
                "archive v7 reconciliation result is corrupt or duplicated",
            ));
        }
        if let (Some(id), Some(hash)) = (
            result.accepted_measurement_id.as_ref(),
            result.accepted_measurement_hash.as_deref(),
        ) && records
            .get(id)
            .is_none_or(|record| record.measurement_hash.as_str() != hash)
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                "archive v7 reconciliation references a missing accepted measurement",
            ));
        }
    }
    Ok(())
}

fn verify_archive_v7_recovery(archive: &EvaluationArchive) -> EvaluationResult<()> {
    match archive.measurement_acquisition_recovery_history_status {
        MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory => {
            if has_recovery_fields(archive)
                || (archive.measurement_acquisition_history_status
                    == MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory
                    && !archive.measurement_acquisition_records.is_empty())
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "archive declares no recovery history but retains Stage 7D records",
                ));
            }
            return Ok(());
        }
        MeasurementAcquisitionRecoveryHistoryStatus::RecoveryHistoryPresent => {}
    }
    let records = verified_acquisition_records(archive)?;
    let journals = archive
        .measurement_acquisition_recovery_journals
        .iter()
        .map(|journal| (journal.recovery_journal_id.as_str(), journal))
        .collect::<BTreeMap<_, _>>();
    if journals.len() != archive.measurement_acquisition_recovery_journals.len()
        || journals.len() != archive.measurement_acquisition_recovery_checkpoints.len()
        || journals.len() != archive.measurement_acquisition_recovery_work_counters.len()
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v7 recovery journal/checkpoint/work cardinality is invalid",
        ));
    }
    let mut prepared = Vec::new();
    let mut snapshots = Vec::new();
    let mut reconciliation = Vec::new();
    let mut authorizations = Vec::new();
    let mut work = Vec::new();
    for journal in journals.values() {
        verify_recovery_journal_archive(journal, &records)?;
        if !archive
            .measurement_acquisition_recovery_replay_statuses
            .get(&journal.measurement_acquisition_recovery_journal_hash)
            .copied()
            .unwrap_or(false)
            || journal.work.replay_hardware_calls != 0
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionReplayHardwareWork,
                "archive v7 recovery journal lacks zero-device replay proof",
            ));
        }
        prepared.extend(journal.prepared_slots.clone());
        snapshots.extend(
            journal
                .prepared_slots
                .iter()
                .map(|prepared| prepared.publication_snapshot.clone()),
        );
        reconciliation.extend(journal.reconciliation_results.clone());
        authorizations.extend(journal.retry_authorizations.clone());
        work.push(journal.work.clone());
    }
    prepared.sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
    reconciliation.sort_by(|left, right| left.reconciliation_id.cmp(&right.reconciliation_id));
    authorizations
        .sort_by(|left, right| left.authorized_attempt_id.cmp(&right.authorized_attempt_id));
    if archive.measurement_acquisition_prepared_slots != prepared
        || archive.measurement_acquisition_publication_snapshots != snapshots
        || archive.measurement_acquisition_reconciliation_results != reconciliation
        || archive.measurement_acquisition_retry_authorizations != authorizations
        || archive.measurement_acquisition_recovery_work_counters != work
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
            "archive v7 flattened recovery stores differ from their journals",
        ));
    }
    for checkpoint in &archive.measurement_acquisition_recovery_checkpoints {
        let journal = journals
            .get(checkpoint.journal.recovery_journal_id.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
                    "archive v7 recovery checkpoint references a missing journal",
                )
            })?;
        if checkpoint.measurement_acquisition_recovery_checkpoint_hash
            != measurement_acquisition_recovery_checkpoint_hash(checkpoint)?
            || checkpoint.journal.as_ref() != *journal
            || checkpoint.session.session_id != journal.session_id
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionRecoveryJournalCorrupt,
                "archive v7 recovery checkpoint is corrupt or stale",
            ));
        }
    }
    Ok(())
}

fn verify_archive_v6_acquisition(archive: &EvaluationArchive) -> EvaluationResult<()> {
    match archive.measurement_acquisition_history_status {
        MeasurementAcquisitionHistoryStatus::Unspecified => {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "evaluation archive v6 must classify acquisition history",
            ));
        }
        MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory => {
            let has_non_record_acquisition_fields =
                !archive.measurement_acquisition_plans.is_empty()
                    || !archive.measurement_acquisition_sessions.is_empty()
                    || !archive.measurement_acquisition_checkpoints.is_empty()
                    || !archive.measurement_acquisition_traces.is_empty()
                    || !archive.measurement_acquisition_results.is_empty()
                    || !archive.measurement_acquisition_cohort_links.is_empty()
                    || !archive.measurement_acquisition_replay_statuses.is_empty();
            if has_non_record_acquisition_fields
                || (archive.manifest.version == 6
                    && !archive.measurement_acquisition_records.is_empty())
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "archive declares no acquisition history but retains Stage 7C records",
                ));
            }
            return Ok(());
        }
        MeasurementAcquisitionHistoryStatus::AcquisitionHistoryPresent => {}
    }
    let plans = archive
        .measurement_acquisition_plans
        .iter()
        .map(|plan| {
            plan.verify()?;
            Ok((plan.measurement_acquisition_plan_hash.as_str(), plan))
        })
        .collect::<EvaluationResult<BTreeMap<_, _>>>()?;
    if plans.len() != archive.measurement_acquisition_plans.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionPlanCorrupt,
            "archive v6 contains duplicate acquisition plans",
        ));
    }
    let records = archive
        .measurement_acquisition_records
        .iter()
        .map(|entry| {
            let hash = agentir_core::backend::measurement_hash(&entry.record)
                .map_err(|error| {
                    EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        error.to_string(),
                    )
                })?
                .to_string();
            if hash != entry.record.measurement_hash.to_string() {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                    "archive v6 acquisition measurement hash is corrupt",
                ));
            }
            Ok((entry.measurement_id.clone(), &entry.record))
        })
        .collect::<EvaluationResult<BTreeMap<_, _>>>()?;
    if records.len() != archive.measurement_acquisition_records.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
            "archive v6 contains duplicate acquisition measurement IDs",
        ));
    }
    let traces = archive
        .measurement_acquisition_traces
        .iter()
        .map(|trace| {
            if trace.measurement_acquisition_trace_hash
                != measurement_acquisition_trace_hash(trace)?
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionTraceCorrupt,
                    "archive v6 acquisition trace hash is corrupt",
                ));
            }
            Ok((trace.measurement_acquisition_trace_hash.as_str(), trace))
        })
        .collect::<EvaluationResult<BTreeMap<_, _>>>()?;
    if traces.len() != archive.measurement_acquisition_traces.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationAcquisitionTraceCorrupt,
            "archive v6 contains duplicate acquisition traces",
        ));
    }
    let results = archive
        .measurement_acquisition_results
        .iter()
        .map(|result| {
            if result.measurement_acquisition_result_hash
                != measurement_acquisition_result_hash(result)?
                || !archive
                    .measurement_acquisition_replay_statuses
                    .get(&result.measurement_acquisition_result_hash)
                    .copied()
                    .unwrap_or(false)
                || !plans.contains_key(result.measurement_acquisition_plan_hash.as_str())
                || !traces.contains_key(result.measurement_acquisition_trace_hash.as_str())
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionResultCorrupt,
                    "archive v6 acquisition result is corrupt, stale, or unreplayed",
                ));
            }
            if result.measurement_ids.len() != result.measurement_hashes.len()
                || result
                    .measurement_hashes
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != result.measurement_hashes.len()
            {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate,
                    "archive v6 result measurement anchors are duplicated or misaligned",
                ));
            }
            for (id, hash) in result
                .measurement_ids
                .iter()
                .zip(&result.measurement_hashes)
            {
                if records
                    .get(id)
                    .is_none_or(|record| record.measurement_hash.as_str() != hash)
                {
                    return Err(EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                        "archive v6 result references a missing completed measurement",
                    ));
                }
            }
            Ok((result.measurement_acquisition_result_hash.as_str(), result))
        })
        .collect::<EvaluationResult<BTreeMap<_, _>>>()?;
    if results.len() != archive.measurement_acquisition_results.len()
        || archive.measurement_acquisition_sessions.len()
            != archive.measurement_acquisition_checkpoints.len()
        || archive.measurement_acquisition_sessions.len()
            != archive.measurement_acquisition_results.len()
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v6 acquisition session/checkpoint/result cardinality is invalid",
        ));
    }
    let sessions = archive
        .measurement_acquisition_sessions
        .iter()
        .map(|session| (session.session_id.as_str(), session))
        .collect::<BTreeMap<_, _>>();
    if sessions.len() != archive.measurement_acquisition_sessions.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v6 contains duplicate acquisition session IDs",
        ));
    }
    for session in sessions.values() {
        let result = session.result()?;
        if session.status == crate::acquisition::MeasurementAcquisitionStatus::Running
            || !plans.contains_key(session.plan.measurement_acquisition_plan_hash.as_str())
            || !traces.contains_key(session.trace.measurement_acquisition_trace_hash.as_str())
            || !results.contains_key(result.measurement_acquisition_result_hash.as_str())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionResultCorrupt,
                "archive v6 acquisition session is running, stale, or missing its result",
            ));
        }
    }
    for checkpoint in &archive.measurement_acquisition_checkpoints {
        if checkpoint.measurement_acquisition_checkpoint_hash
            != measurement_acquisition_checkpoint_hash(checkpoint)?
            || !plans.contains_key(checkpoint.measurement_acquisition_plan_hash.as_str())
            || sessions
                .get(checkpoint.session.session_id.as_str())
                .is_none_or(|session| *session != checkpoint.session.as_ref())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                "archive v6 acquisition checkpoint is corrupt",
            ));
        }
    }
    for (result_hash, cohort_hash) in &archive.measurement_acquisition_cohort_links {
        if !results.contains_key(result_hash.as_str())
            || !archive
                .measurement_cohorts
                .iter()
                .any(|cohort| cohort.measurement_cohort_hash == *cohort_hash)
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                "archive v6 acquisition cohort handoff link is stale",
            ));
        }
    }
    Ok(())
}

fn verify_archive_v5_measured(archive: &EvaluationArchive) -> EvaluationResult<()> {
    match archive.measured_search_history_status {
        MeasuredSearchHistoryStatus::Unspecified => {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "evaluation archive v5 must classify measured-search history",
            ));
        }
        MeasuredSearchHistoryStatus::NoMeasuredSearchHistory => {
            if has_measured_fields(archive) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "archive declares no measured-search history but retains Stage 7B records",
                ));
            }
            return Ok(());
        }
        MeasuredSearchHistoryStatus::MeasuredSearchHistoryPresent => {}
    }
    let mut cohorts = BTreeMap::new();
    for cohort in &archive.measurement_cohorts {
        verify_measurement_cohort(cohort)?;
        if cohort.measurement_cohort_hash != measurement_cohort_hash(cohort)?
            || cohorts
                .insert(cohort.measurement_cohort_hash.as_str(), cohort)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMeasurementCohortCorrupt,
                "archive v5 cohort is corrupt or duplicated",
            ));
        }
    }
    let mut objectives = BTreeMap::new();
    for objective in &archive.measured_objectives {
        let cohort = cohorts
            .get(objective.measurement_cohort_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                    "archive v5 objective references a missing cohort",
                )
            })?;
        objective.validate(cohort)?;
        if objective.measured_objective_hash != measured_objective_hash(objective)?
            || objectives
                .insert(objective.measured_objective_hash.as_str(), objective)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMeasuredObjectiveCorrupt,
                "archive v5 measured objective is corrupt or duplicated",
            ));
        }
    }
    let checkpoints = archive
        .search_checkpoints
        .iter()
        .filter_map(|checkpoint| {
            checkpoint
                .session
                .result
                .as_ref()
                .map(|result| (result.search_result_hash.as_str(), &checkpoint.session))
        })
        .collect::<BTreeMap<_, _>>();
    let recommendations = archive
        .measured_recommendations
        .iter()
        .map(|recommendation| {
            (
                recommendation.measured_recommendation_hash.as_str(),
                recommendation,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if recommendations.len() != archive.measured_recommendations.len()
        || archive.measured_search_runs.len() != archive.measured_recommendations.len()
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v5 measured run/recommendation cardinality is invalid",
        ));
    }
    for run in &archive.measured_search_runs {
        let cohort = cohorts
            .get(run.measurement_cohort_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                    "measured run references a missing cohort",
                )
            })?;
        let objective = objectives
            .get(run.measured_objective_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                    "measured run references a missing objective",
                )
            })?;
        let retained = recommendations
            .get(run.measured_recommendation_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredRecommendationCorrupt,
                    "measured run references a missing recommendation",
                )
            })?;
        let session = checkpoints
            .get(run.search_result_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                    "measured run references a missing Stage 7A result",
                )
            })?;
        let replayed = measured_recommendation(session, cohort, objective)?;
        if !run.replay_verified
            || run.work.hardware_calls != 0
            || retained.measured_recommendation_hash != measured_recommendation_hash(retained)?
            || &replayed != *retained
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationMeasuredRecommendationCorrupt,
                "archive v5 measured recommendation replay mismatch",
            ));
        }
    }
    Ok(())
}

fn verify_archive_v4_search(archive: &EvaluationArchive) -> EvaluationResult<()> {
    match archive.search_history_status {
        SearchHistoryStatus::Unspecified => {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "evaluation archive v4 must explicitly classify search-history presence",
            ));
        }
        SearchHistoryStatus::NoSearchHistory => {
            if has_search_fields(archive) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "evaluation archive v4 declares no search history but retains search fields",
                ));
            }
            return Ok(());
        }
        SearchHistoryStatus::SearchHistoryPresent => {
            if archive.search_runs.is_empty() || archive.search_checkpoints.is_empty() {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveInvalid,
                    "evaluation archive v4 search history requires runs and final checkpoints",
                ));
            }
        }
    }

    let mut objectives = BTreeMap::new();
    for objective in &archive.search_objectives {
        let task = archive
            .corpus
            .tasks
            .iter()
            .find(|task| task.id == objective.task_id)
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationSearchRootStale,
                    "archive v4 search objective task is missing",
                )
            })?;
        objective.validate(&archive.corpus, task)?;
        if objective.search_objective_hash != search_objective_hash(objective)?
            || objectives
                .insert(objective.search_objective_hash.clone(), objective)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSearchObjectiveInvalid,
                "archive v4 search objective is corrupt or duplicated",
            ));
        }
    }
    let mut plans = BTreeMap::new();
    for plan in &archive.search_plans {
        if plan.search_plan_hash != search_plan_hash(plan)?
            || !objectives.contains_key(&plan.search_objective_hash)
            || plans.insert(plan.search_plan_hash.clone(), plan).is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSearchPlanInvalid,
                "archive v4 search plan is corrupt, duplicated, or unanchored",
            ));
        }
    }
    let policies = archive
        .ranking_policies
        .iter()
        .map(|policy| (policy.ranking_policy_hash.as_str(), policy))
        .collect::<BTreeMap<_, _>>();
    let models = archive
        .learned_models
        .iter()
        .map(|model| (model.learned_model_hash.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let schemas = archive
        .feature_schemas
        .iter()
        .map(|schema| (schema.feature_schema_hash.as_str(), schema))
        .collect::<BTreeMap<_, _>>();
    let runs = archive
        .search_runs
        .iter()
        .map(|run| (run.search_run_id.as_str(), run))
        .collect::<BTreeMap<_, _>>();
    if runs.len() != archive.search_runs.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v4 contains duplicate search run IDs",
        ));
    }
    let mut checkpoint_hashes = BTreeSet::new();
    let mut expected_nodes = Vec::new();
    let mut expected_edges = Vec::new();
    let mut expected_traces = Vec::new();
    let mut expected_results = Vec::new();
    let mut expected_rejections = Vec::new();
    let mut expected_work = Vec::new();
    for checkpoint in &archive.search_checkpoints {
        if checkpoint.search_checkpoint_hash != search_checkpoint_hash(checkpoint)?
            || !checkpoint_hashes.insert(checkpoint.search_checkpoint_hash.as_str())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSearchCheckpointCorrupt,
                "archive v4 search checkpoint is corrupt or duplicated",
            ));
        }
        let plan = plans.get(&checkpoint.search_plan_hash).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationSearchPlanInvalid,
                "archive v4 checkpoint references a missing plan",
            )
        })?;
        let policy = policies
            .get(plan.ranking_policy_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationRankingPolicyNotFound,
                    "archive v4 search plan references a missing ranking policy",
                )
            })?;
        let ranker = if let Some(model_hash) = &plan.learned_model_hash {
            let model = models.get(model_hash.as_str()).ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationModelInvalid,
                    "archive v4 search plan references a missing learned model",
                )
            })?;
            let schema = schemas
                .get(policy.feature_schema_hash.as_str())
                .ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        EvaluationErrorCode::EvaluationFeatureSchemaNotFound,
                        "archive v4 learned search references a missing feature schema",
                    )
                })?;
            SearchRanker::Learned {
                descriptor: (*policy).clone(),
                model: Box::new((**model).clone()),
                schema: (*schema).clone(),
            }
        } else {
            SearchRanker::Scripted {
                descriptor: (*policy).clone(),
            }
        };
        let session = SearchSession::resume(
            checkpoint,
            &archive.corpus,
            &ranker,
            &SearchLimits::default(),
        )?;
        replay_search(&session, &ranker, &SearchLimits::default())?;
        let run = runs.get(session.search_run_id.as_str()).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "archive v4 checkpoint references a missing search run",
            )
        })?;
        if run.initial_anchor_hash != session.objective.initial_anchor_hash
            || run.search_objective_hash != session.objective.search_objective_hash
            || run.search_plan_hash != session.plan.search_plan_hash
            || run.status != session.status
            || run.semantic_work != session.work.semantic_expansions
            || run.checkpoint_hashes != vec![checkpoint.search_checkpoint_hash.clone()]
            || run.search_trace_hash
                != session
                    .trace
                    .as_ref()
                    .map(|trace| trace.search_trace_hash.clone())
            || run.search_result_hash
                != session
                    .result
                    .as_ref()
                    .map(|result| result.search_result_hash.clone())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationArchiveInvalid,
                "archive v4 search run differs from its exact final checkpoint",
            ));
        }
        expected_nodes.extend(session.nodes.into_values());
        expected_edges.extend(session.edges.into_values());
        expected_traces.extend(session.trace);
        expected_results.extend(session.result);
        expected_rejections.extend(session.rejection);
        expected_work.push(session.work);
    }
    if archive.search_checkpoints.len() != archive.search_runs.len() {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v4 requires exactly one final checkpoint per search run",
        ));
    }
    expected_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    expected_edges.sort_by(|left, right| left.id.cmp(&right.id));
    expected_traces.sort_by(|left, right| left.search_trace_hash.cmp(&right.search_trace_hash));
    expected_results.sort_by(|left, right| left.search_result_hash.cmp(&right.search_result_hash));
    if archive.search_nodes != expected_nodes
        || archive.search_edges != expected_edges
        || archive.search_traces != expected_traces
        || archive.search_results != expected_results
        || archive.search_rejections != expected_rejections
        || archive.search_work_counters != expected_work
        || archive
            .search_nodes
            .iter()
            .any(|node| node.search_node_hash != search_node_hash(node).unwrap_or_default())
        || archive
            .search_edges
            .iter()
            .any(|edge| edge.search_edge_hash != search_edge_hash(edge).unwrap_or_default())
        || archive
            .search_traces
            .iter()
            .any(|trace| trace.search_trace_hash != search_trace_hash(trace).unwrap_or_default())
        || archive.search_results.iter().any(|result| {
            result.search_result_hash != search_result_hash(result).unwrap_or_default()
        })
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveInvalid,
            "archive v4 flattened search graph, trace, result, rejection, or work store differs",
        ));
    }
    Ok(())
}

fn verify_archive_v3_learning(
    archive: &EvaluationArchive,
    episode_ids: &BTreeSet<String>,
) -> EvaluationResult<()> {
    if archive.learning_statuses.len() != episode_ids.len()
        || archive
            .learning_statuses
            .keys()
            .any(|episode| !episode_ids.contains(episode))
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "archive v3 must explicitly classify every episode as learned or unlearned",
        ));
    }
    let limits = LearnedRankingLimits::default();
    let schemas = archive
        .feature_schemas
        .iter()
        .map(|schema| (schema.feature_schema_hash.as_str(), schema))
        .collect::<BTreeMap<_, _>>();
    let policies = archive
        .ranking_policies
        .iter()
        .map(|policy| (policy.ranking_policy_hash.as_str(), policy))
        .collect::<BTreeMap<_, _>>();
    let source_episode_hashes = archive
        .runs
        .iter()
        .flat_map(|run| &run.episodes)
        .filter_map(|episode| episode.episode_hash.as_deref())
        .collect::<BTreeSet<_>>();
    let source_choice_set_hashes = archive
        .choice_sets
        .iter()
        .map(|choice_set| choice_set.choice_set_hash.as_str())
        .collect::<BTreeSet<_>>();
    let mut datasets = BTreeMap::new();
    for dataset in &archive.ranking_datasets {
        validate_dataset(dataset, &limits, true)?;
        if dataset.manifest.ranking_dataset_hash != ranking_dataset_hash(dataset)?
            || dataset.examples.iter().any(|example| {
                !source_episode_hashes.contains(example.source_episode_hash.as_str())
                    || !source_choice_set_hashes.contains(example.source_choice_set_hash.as_str())
            })
            || datasets
                .insert(dataset.manifest.ranking_dataset_hash.as_str(), dataset)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "archive v3 dataset is corrupt or duplicated",
            ));
        }
    }
    let mut splits = BTreeMap::new();
    for split in &archive.dataset_splits {
        let dataset = datasets
            .get(split.ranking_dataset_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationDatasetInvalid,
                    "archive v3 split references a missing dataset",
                )
            })?;
        validate_split(dataset, split)?;
        if split.dataset_split_hash != dataset_split_hash(split)?
            || splits
                .insert(split.dataset_split_hash.as_str(), split)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "archive v3 split is corrupt or duplicated",
            ));
        }
    }
    let mut configurations = BTreeMap::new();
    for configuration in &archive.training_configurations {
        validate_training_configuration_contract(configuration, &limits)?;
        if configuration.training_configuration_hash != training_configuration_hash(configuration)?
            || configurations
                .insert(
                    configuration.training_configuration_hash.as_str(),
                    configuration,
                )
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationTrainingInvalid,
                "archive v3 training configuration is corrupt or duplicated",
            ));
        }
    }
    let mut models = BTreeMap::new();
    for model in &archive.learned_models {
        let schema = schemas
            .get(model.feature_schema_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationModelIncompatible,
                    "archive v3 model references a missing schema",
                )
            })?;
        validate_model_artifact(model, schema, &limits)?;
        if model.learned_model_hash != learned_model_hash(model)?
            || models
                .insert(model.learned_model_hash.as_str(), model)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationModelInvalid,
                "archive v3 learned model is corrupt or duplicated",
            ));
        }
    }
    let mut inputs = BTreeMap::new();
    for input in &archive.ranking_inputs {
        if input.ranking_input_hash != ranking_input_hash(input)?
            || !schemas.contains_key(input.feature_schema_hash.as_str())
            || !archive
                .choice_sets
                .iter()
                .any(|set| set.choice_set_hash == input.choice_set_hash)
            || inputs
                .insert(input.ranking_input_hash.as_str(), input)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationInferenceInvalid,
                "archive v3 inference input is corrupt, duplicated, or unanchored",
            ));
        }
    }
    let mut inference_hashes = BTreeSet::new();
    for inference in &archive.inference_records {
        let input = inputs
            .get(inference.ranking_input_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationInferenceInvalid,
                    "archive v3 inference references a missing input",
                )
            })?;
        let model = models
            .get(inference.learned_model_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationInferenceInvalid,
                    "archive v3 inference references a missing model",
                )
            })?;
        let policy = policies
            .get(inference.ranking_policy_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationInferenceInvalid,
                    "archive v3 inference references a missing policy",
                )
            })?;
        let schema = schemas
            .get(input.feature_schema_hash.as_str())
            .expect("validated inference input schema remains present");
        verify_inference(input, schema, model, policy, inference, &limits)?;
        if inference.inference_hash != inference_hash(inference)?
            || !inference_hashes.insert(inference.inference_hash.as_str())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationInferenceInvalid,
                "archive v3 inference is corrupt or duplicated",
            ));
        }
    }
    let mut training_hashes = BTreeSet::new();
    for run in &archive.training_runs {
        let dataset = datasets
            .get(run.ranking_dataset_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationTrainingInvalid,
                    "archive v3 training run references a missing dataset",
                )
            })?;
        let split = splits.get(run.dataset_split_hash.as_str()).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationTrainingInvalid,
                "archive v3 training run references a missing split",
            )
        })?;
        let configuration = configurations
            .get(run.training_configuration_hash.as_str())
            .ok_or_else(|| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationTrainingInvalid,
                    "archive v3 training run references a missing configuration",
                )
            })?;
        let model = models.get(run.learned_model_hash.as_str()).ok_or_else(|| {
            EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationTrainingInvalid,
                "archive v3 training run references a missing model",
            )
        })?;
        validate_training_run(run, dataset, split, configuration, model)?;
        if run.training_run_hash != training_run_hash(run)?
            || !training_hashes.insert(run.training_run_hash.as_str())
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationTrainingInvalid,
                "archive v3 training run is corrupt or duplicated",
            ));
        }
    }
    let learned_frames = archive
        .inference_records
        .iter()
        .map(|record| {
            (
                record.choice_set_hash.as_str(),
                record.ranking_policy_hash.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    for run in &archive.runs {
        for episode in &run.episodes {
            let expected = if episode.steps.iter().any(|step| {
                step.ranking_trace.as_ref().is_some_and(|trace| {
                    learned_frames.contains(&(
                        trace.choice_set_hash.as_str(),
                        trace.ranking_policy_hash.as_str(),
                    ))
                })
            }) {
                LearningEpisodeStatus::Learned
            } else {
                LearningEpisodeStatus::Unlearned
            };
            if archive.learning_statuses.get(&episode.id) != Some(&expected) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
                    "archive v3 learned status does not match retained inference records",
                ));
            }
        }
    }
    Ok(())
}

fn verify_archive_v2_ranking(
    archive: &EvaluationArchive,
    episode_ids: &BTreeSet<String>,
) -> EvaluationResult<()> {
    let mut schemas = BTreeMap::new();
    for schema in &archive.feature_schemas {
        if schema.feature_schema_hash != crate::ranking::feature_schema_hash(schema)?
            || schemas
                .insert(schema.feature_schema_hash.clone(), schema)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationFeatureSchemaMismatch,
                "archive v2 feature schema is invalid or duplicated",
            ));
        }
    }
    let mut policies = BTreeMap::new();
    for policy in &archive.ranking_policies {
        if policy.ranking_policy_hash != crate::ranking::ranking_policy_hash(policy)?
            || !schemas.contains_key(&policy.feature_schema_hash)
            || policies
                .insert(policy.ranking_policy_hash.clone(), policy)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                "archive v2 ranking policy is invalid or duplicated",
            ));
        }
    }
    let mut choice_sets = BTreeMap::new();
    for choice_set in &archive.choice_sets {
        if choice_set.choice_set_hash != crate::ranking::choice_set_hash(choice_set)?
            || !schemas.contains_key(&choice_set.feature_schema_hash)
            || choice_sets
                .insert(choice_set.choice_set_hash.clone(), choice_set)
                .is_some()
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationChoiceSetMismatch,
                "archive v2 choice set is invalid or duplicated",
            ));
        }
    }
    if archive.ranking_statuses.len() != episode_ids.len()
        || archive
            .ranking_statuses
            .keys()
            .any(|episode| !episode_ids.contains(episode))
    {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
            "archive v2 must explicitly classify every episode as ranked or unranked",
        ));
    }
    for run in &archive.runs {
        for episode in &run.episodes {
            let has_ranking = episode
                .steps
                .iter()
                .any(|step| step.ranking_trace.is_some());
            let expected = if has_ranking {
                crate::model::RankingEpisodeStatus::Ranked
            } else {
                crate::model::RankingEpisodeStatus::Unranked
            };
            if archive.ranking_statuses.get(&episode.id) != Some(&expected) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationArchiveMigrationInvalid,
                    "episode ranking status does not match its exact transcript",
                ));
            }
            for step in &episode.steps {
                if let Some(trace) = &step.ranking_trace {
                    let choice_set = choice_sets.get(&trace.choice_set_hash).ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationChoiceSetNotFound,
                            "ranking trace choice set is missing from archive v2",
                        )
                    })?;
                    let policy = policies.get(&trace.ranking_policy_hash).ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            EvaluationErrorCode::EvaluationRankingPolicyNotFound,
                            "ranking trace policy is missing from archive v2",
                        )
                    })?;
                    crate::ranking::replay_ranking_trace(
                        choice_set,
                        policy,
                        trace,
                        &crate::ranking::RankingLimits::default(),
                    )?;
                }
            }
        }
    }
    Ok(())
}
