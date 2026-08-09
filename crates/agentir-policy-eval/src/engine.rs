//! Episode state machine, scripted policies, replay, metrics, and archives.

use crate::{
    corpus::builtin_corpus,
    hashing::{
        aggregate_hash, archive_hash, corpus_hash, episode_hash, evaluation_hash, observation_hash,
        policy_hash,
    },
    model::{
        CompilerOutcome, ContextMeasurement, EpisodeResult, EpisodeStatus, EpisodeStep,
        EvaluationAggregate, EvaluationArchive, EvaluationComparison, EvaluationContinuation,
        EvaluationCorpus, EvaluationDiagnostic, EvaluationEpisode, EvaluationErrorCode,
        EvaluationManifest, EvaluationObservation, EvaluationResult, EvaluationRun, EvaluationTask,
        EvaluationTaskId, PolicyCapabilities, PolicyDecision, PolicyDescriptor, PolicyKind,
        PolicyOrigin, PolicyVersion, RejectionClassification, RepairCycle, SemanticResult,
        TaskBudget, TaskSuccessCriterion, TokenUsage, UsageTrust,
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

/// Stateful in-memory Stage 6A harness.
pub struct EvaluationHarness {
    corpus: EvaluationCorpus,
    runs: BTreeMap<String, EvaluationRun>,
    sessions: BTreeMap<String, CompilerEngine>,
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
        let continuation_frame = if matches!(run.policy.kind, PolicyKind::Menu | PolicyKind::Hybrid)
        {
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
            Vec::new()
        };
        limit(
            u64::try_from(continuation_frame.len()).unwrap_or(u64::MAX),
            self.limits.menu_choices,
            "menu_choices",
        )?;
        let previous = episode.steps.last().map(|step| &step.outcome);
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
        replay_recorded_run(run)
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
        let mut archive = EvaluationArchive {
            manifest: EvaluationManifest {
                format: "agentir.evaluation.archive".to_owned(),
                version: 1,
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
        verify_archive(&archive)?;
        for run in &archive.runs {
            replay_recorded_run(run)?;
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

fn replay_recorded_run(run: &EvaluationRun) -> EvaluationResult<()> {
    for episode in &run.episodes {
        let mut compiler = CompilerEngine::new();
        for (index, recorded) in episode.steps.iter().enumerate() {
            if recorded.ordinal != u64::try_from(index).unwrap_or(u64::MAX) {
                return Err(EvaluationDiagnostic::new(
                    EvaluationErrorCode::EvaluationEventOrderInvalid,
                    "episode step order is not contiguous",
                ));
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
    if archive.manifest.format != "agentir.evaluation.archive" || archive.manifest.version != 1 {
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
            if !episode_ids.insert(&episode.id) {
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
            }
        }
        if run.evaluation_hash.as_deref() != Some(evaluation_hash(run)?.as_str()) {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationHashMismatch,
                "evaluation hash mismatch",
            ));
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
