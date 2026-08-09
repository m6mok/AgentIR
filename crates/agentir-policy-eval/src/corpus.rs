//! Built-in immutable, offline Stage 6A task corpus.

use crate::{
    hashing::corpus_hash,
    model::{
        EvaluationCorpus, EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult,
        EvaluationTask, EvaluationTaskId, PolicyKind, TaskBudget, TaskCategory, TaskInitialState,
        TaskObjective, TaskSuccessCriterion,
    },
};
use serde_json::Value;
use std::collections::BTreeMap;

const SAXPY: &str = include_str!("../../../examples/saxpy.jsonl");
const CANDIDATE_REWRITE: &str = include_str!("../../../examples/candidate_rewrite.jsonl");
const SPECULATIVE: &str = include_str!("../../../examples/speculative_promote.jsonl");
const EQUALITY: &str = include_str!("../../../examples/equality_materialize.jsonl");
const MEMORY_FRESH: &str = include_str!("../../../examples/memory_fresh.jsonl");
const MEMORY_REUSE: &str = include_str!("../../../examples/memory_reuse.jsonl");
const MEMORY_GUARDED: &str = include_str!("../../../examples/memory_guarded_reuse.jsonl");
const SCHEDULE_SERIAL: &str = include_str!("../../../examples/schedule_serial.jsonl");
const SCHEDULE_TILED: &str = include_str!("../../../examples/schedule_tiled.jsonl");
const SCHEDULE_FUSED: &str = include_str!("../../../examples/schedule_fused.jsonl");
const SCHEDULE_VECTOR: &str = include_str!("../../../examples/schedule_vectorized.jsonl");
const BACKEND: &str = include_str!("../../../examples/backend_saxpy_wgsl.jsonl");
const BACKEND_REJECTED: &str = include_str!("../../../examples/backend_rejected_reduce.jsonl");
const EQUALITY_ARTIFACT: &str = include_str!("../../../examples/equality_to_artifact.jsonl");

fn parse_script(script: &str) -> EvaluationResult<Vec<Value>> {
    script
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationCorpusNotFound,
                    format!("built-in task JSON is invalid: {error}"),
                )
            })
        })
        .collect()
}

fn task(
    id: &str,
    category: TaskCategory,
    size: &str,
    summary: &str,
    script: &str,
    criterion: TaskSuccessCriterion,
) -> EvaluationResult<EvaluationTask> {
    let requests = parse_script(script)?;
    let max_actions = u64::try_from(requests.len())
        .unwrap_or(u64::MAX)
        .saturating_add(4);
    Ok(EvaluationTask {
        id: EvaluationTaskId(id.to_owned()),
        corpus_version: "stage6a-v1".to_owned(),
        category,
        size: size.to_owned(),
        objective: TaskObjective {
            summary: summary.to_owned(),
            tags: vec!["offline".to_owned(), "compiler_owned_success".to_owned()],
        },
        initial_state: TaskInitialState {
            source_archive_hash: None,
            production_requests: requests,
            runtime_inputs: BTreeMap::new(),
        },
        success_criterion: criterion,
        allowed_modes: vec![PolicyKind::Free, PolicyKind::Menu, PolicyKind::Hybrid],
        budget: TaskBudget {
            max_actions,
            max_rejections: 4,
            max_context_bytes: 8 * 1024 * 1024,
            max_deterministic_tokens: None,
            max_wall_time_ms: None,
        },
        required_final_state: summary.to_owned(),
        reference_output: None,
        selected_target: None,
        metadata: BTreeMap::from([
            ("network".to_owned(), "forbidden".to_owned()),
            ("device".to_owned(), "not_required".to_owned()),
        ]),
    })
}

/// Returns the deterministic built-in corpus covering all twenty required categories.
pub fn builtin_corpus() -> EvaluationResult<EvaluationCorpus> {
    let accepted = TaskSuccessCriterion::AllActionsAccepted;
    let saxpy = TaskSuccessCriterion::JsonPointerEquals {
        pointer: "/result/outputs/out".to_owned(),
        expected: serde_json::json!([12.0, 24.0, 36.0, 48.0]),
    };
    let mut tasks = vec![
        task(
            "spec-construction-small",
            TaskCategory::SpecIrConstruction,
            "small",
            "construct and evaluate typed SpecIR",
            SAXPY,
            saxpy.clone(),
        )?,
        task(
            "hole-repair-small",
            TaskCategory::HoleRepair,
            "small",
            "repair a partial typed construction",
            SAXPY,
            saxpy.clone(),
        )?,
        task(
            "constraint-discharge-small",
            TaskCategory::ConstraintDischarge,
            "small",
            "discharge shape constraints",
            SAXPY,
            saxpy.clone(),
        )?,
        task(
            "candidate-rewrite-small",
            TaskCategory::ExactCandidateRewrite,
            "small",
            "apply and prove an exact rewrite",
            CANDIDATE_REWRITE,
            accepted.clone(),
        )?,
        task(
            "speculative-repair-small",
            TaskCategory::SpeculativeProposalRepair,
            "small",
            "validate a speculative proposal",
            SPECULATIVE,
            accepted.clone(),
        )?,
        task(
            "equality-materialize-medium",
            TaskCategory::EqualityExpansionMaterialization,
            "medium",
            "materialize an exact equality member",
            EQUALITY,
            accepted.clone(),
        )?,
        task(
            "memory-fresh-small",
            TaskCategory::FreshMemoryCreation,
            "small",
            "create fresh exact MemoryIR",
            MEMORY_FRESH,
            accepted.clone(),
        )?,
        task(
            "memory-reuse-medium",
            TaskCategory::LegalStaticReuse,
            "medium",
            "apply legal static reuse",
            MEMORY_REUSE,
            accepted.clone(),
        )?,
        task(
            "memory-guarded-medium",
            TaskCategory::GuardedReuse,
            "medium",
            "retain guarded reuse and exact fallback",
            MEMORY_GUARDED,
            accepted.clone(),
        )?,
        task(
            "schedule-serial-small",
            TaskCategory::ScheduleSerialBaseline,
            "small",
            "create the serial schedule baseline",
            SCHEDULE_SERIAL,
            accepted.clone(),
        )?,
        task(
            "schedule-tiled-medium",
            TaskCategory::ExactTilingRemainder,
            "medium",
            "apply exact tiling and remainder",
            SCHEDULE_TILED,
            accepted.clone(),
        )?,
        task(
            "schedule-fused-medium",
            TaskCategory::LegalFusion,
            "medium",
            "apply legal fusion",
            SCHEDULE_FUSED,
            accepted.clone(),
        )?,
        task(
            "schedule-vector-large",
            TaskCategory::VectorizationUnrolling,
            "large",
            "apply verified vectorization and unrolling",
            SCHEDULE_VECTOR,
            accepted.clone(),
        )?,
        task(
            "schedule-repair-medium",
            TaskCategory::IllegalScheduleRepair,
            "medium",
            "preserve state across illegal schedule repair",
            SCHEDULE_TILED,
            accepted.clone(),
        )?,
        task(
            "backend-lowering-medium",
            TaskCategory::BackendLowering,
            "medium",
            "lower proved ScheduleIR to BackendIR",
            BACKEND,
            accepted.clone(),
        )?,
        task(
            "backend-unsupported-small",
            TaskCategory::UnsupportedBackendRepair,
            "small",
            "classify unsupported backend lowering",
            BACKEND_REJECTED,
            TaskSuccessCriterion::RejectionThenAccepted {
                rejection_code: "UNSUPPORTED_BACKEND_LOWERING".to_owned(),
            },
        )?,
        task(
            "artifact-emission-medium",
            TaskCategory::DeterministicArtifactEmission,
            "medium",
            "emit a deterministic offline-valid artifact",
            BACKEND,
            accepted.clone(),
        )?,
        task(
            "equality-artifact-large",
            TaskCategory::EqualityToArtifact,
            "large",
            "materialize equality through artifact emission",
            EQUALITY_ARTIFACT,
            accepted.clone(),
        )?,
        task(
            "archive-replay-small",
            TaskCategory::ArchiveMigrationReplay,
            "small",
            "exercise deterministic production replay state",
            SAXPY,
            saxpy.clone(),
        )?,
        task(
            "saxpy-end-to-end-large",
            TaskCategory::EndToEndSaxpy,
            "large",
            "complete end-to-end SAXPY",
            SAXPY,
            saxpy,
        )?,
    ];
    if let Some(task) = tasks
        .iter_mut()
        .find(|task| task.id.0 == "backend-unsupported-small")
    {
        task.initial_state
            .production_requests
            .push(serde_json::json!({
                "command": "program.query",
                "request_id": "repair-preserved-spec",
                "workspace": "w1",
                "revision": "r2"
            }));
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let mut corpus = EvaluationCorpus {
        name: "agentir-stage6a-default".to_owned(),
        version: "stage6a-v1".to_owned(),
        tasks,
        corpus_hash: String::new(),
    };
    corpus.corpus_hash = corpus_hash(&corpus)?;
    Ok(corpus)
}
