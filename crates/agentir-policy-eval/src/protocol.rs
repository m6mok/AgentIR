//! Bounded JSONL transport for external Stage 6A agents.

use crate::{
    engine::{EvaluationHarness, RankingSubmission, external_policy, scripted_policy},
    model::{EvaluationDiagnostic, EvaluationTaskId, PolicyDecision, PolicyKind, TokenUsage},
    ranking::{RankingDecision, aggregate_ranking_metrics, feature_schema_v1, scripted_ranker},
    search::{
        ObjectiveDirection, SearchLimits, SearchObjectiveComponent, SearchObjectiveComponentKind,
        SearchObjectiveDescriptor, SearchPlan, SearchRanker, SearchSession, replay_search,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum EvaluationRequest {
    #[serde(rename = "evaluation.search.start")]
    SearchStart {
        request_id: String,
        task: EvaluationTaskId,
        corpus_hash: String,
        ranking_policy: String,
        #[serde(default)]
        seed: u64,
        beam_width: u64,
        maximum_semantic_depth: u64,
        maximum_children_retained_per_node: u64,
        checkpoint_cadence_work_units: u64,
    },
    #[serde(rename = "evaluation.search.advance")]
    SearchAdvance {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
        maximum_work_units: u64,
    },
    #[serde(rename = "evaluation.search.status")]
    SearchStatus {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
    },
    #[serde(rename = "evaluation.search.checkpoint")]
    SearchCheckpoint {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
    },
    #[serde(rename = "evaluation.search.resume")]
    SearchResume {
        request_id: String,
        checkpoint: Box<crate::search::SearchCheckpoint>,
    },
    #[serde(rename = "evaluation.search.cancel")]
    SearchCancel {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
    },
    #[serde(rename = "evaluation.search.result")]
    SearchResult {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
    },
    #[serde(rename = "evaluation.search.replay")]
    SearchReplay {
        request_id: String,
        search: String,
        search_objective_hash: String,
        search_plan_hash: String,
    },
    #[serde(rename = "evaluation.ranking.policy.list")]
    RankingPolicyList {
        request_id: String,
        #[serde(default)]
        seed: u64,
    },
    #[serde(rename = "evaluation.ranking.policy.query")]
    RankingPolicyQuery {
        request_id: String,
        policy: String,
        #[serde(default)]
        seed: u64,
    },
    #[serde(rename = "evaluation.choice_set.query")]
    ChoiceSetQuery { request_id: String, episode: String },
    #[serde(rename = "evaluation.corpus.list")]
    CorpusList { request_id: String },
    #[serde(rename = "evaluation.task.query")]
    TaskQuery {
        request_id: String,
        task: EvaluationTaskId,
    },
    #[serde(rename = "evaluation.run.start")]
    RunStart {
        request_id: String,
        policy: String,
        #[serde(default)]
        kind: Option<PolicyKind>,
        #[serde(default)]
        tasks: Vec<EvaluationTaskId>,
        #[serde(default)]
        seeds: Vec<u64>,
        #[serde(default)]
        scripted: bool,
    },
    #[serde(rename = "evaluation.run.status")]
    RunStatus { request_id: String, run: String },
    #[serde(rename = "evaluation.run.cancel")]
    RunCancel { request_id: String, run: String },
    #[serde(rename = "evaluation.episode.query")]
    EpisodeQuery { request_id: String, episode: String },
    #[serde(rename = "evaluation.episode.next")]
    EpisodeNext { request_id: String, episode: String },
    #[serde(rename = "evaluation.episode.submit")]
    EpisodeSubmit {
        request_id: String,
        run: String,
        episode: String,
        step: String,
        observation_hash: String,
        decision: PolicyDecision,
        #[serde(default)]
        usage: Option<TokenUsage>,
        #[serde(default)]
        external_request_correlation_id: Option<String>,
    },
    #[serde(rename = "evaluation.episode.rank")]
    EpisodeRank {
        request_id: String,
        run: String,
        episode: String,
        step: String,
        observation_hash: String,
        choice_set_hash: String,
        feature_schema_hash: String,
        ranking_policy_hash: String,
        decision: RankingDecision,
        #[serde(default)]
        usage: Option<TokenUsage>,
        #[serde(default)]
        external_request_correlation_id: Option<String>,
    },
    #[serde(rename = "evaluation.ranking.trace.query")]
    RankingTraceQuery { request_id: String, episode: String },
    #[serde(rename = "evaluation.ranking.aggregate")]
    RankingAggregate { request_id: String, run: String },
    #[serde(rename = "evaluation.ranking.compare")]
    RankingCompare {
        request_id: String,
        runs: Vec<String>,
    },
    #[serde(rename = "evaluation.episode.finish")]
    EpisodeFinish { request_id: String, episode: String },
    #[serde(rename = "evaluation.transcript.query")]
    TranscriptQuery { request_id: String, episode: String },
    #[serde(rename = "evaluation.aggregate")]
    Aggregate { request_id: String, run: String },
    #[serde(rename = "evaluation.compare")]
    Compare {
        request_id: String,
        runs: Vec<String>,
    },
    #[serde(rename = "evaluation.archive.save")]
    ArchiveSave {
        request_id: String,
        path: String,
        runs: Vec<String>,
    },
    #[serde(rename = "evaluation.archive.load")]
    ArchiveLoad { request_id: String, path: String },
    #[serde(rename = "evaluation.replay")]
    Replay { request_id: String, run: String },
}

impl EvaluationRequest {
    fn request_id(&self) -> &str {
        match self {
            Self::SearchStart { request_id, .. }
            | Self::SearchAdvance { request_id, .. }
            | Self::SearchStatus { request_id, .. }
            | Self::SearchCheckpoint { request_id, .. }
            | Self::SearchResume { request_id, .. }
            | Self::SearchCancel { request_id, .. }
            | Self::SearchResult { request_id, .. }
            | Self::SearchReplay { request_id, .. }
            | Self::RankingPolicyList { request_id, .. }
            | Self::RankingPolicyQuery { request_id, .. }
            | Self::ChoiceSetQuery { request_id, .. }
            | Self::CorpusList { request_id }
            | Self::TaskQuery { request_id, .. }
            | Self::RunStart { request_id, .. }
            | Self::RunStatus { request_id, .. }
            | Self::RunCancel { request_id, .. }
            | Self::EpisodeQuery { request_id, .. }
            | Self::EpisodeNext { request_id, .. }
            | Self::EpisodeSubmit { request_id, .. }
            | Self::EpisodeRank { request_id, .. }
            | Self::RankingTraceQuery { request_id, .. }
            | Self::RankingAggregate { request_id, .. }
            | Self::RankingCompare { request_id, .. }
            | Self::EpisodeFinish { request_id, .. }
            | Self::TranscriptQuery { request_id, .. }
            | Self::Aggregate { request_id, .. }
            | Self::Compare { request_id, .. }
            | Self::ArchiveSave { request_id, .. }
            | Self::ArchiveLoad { request_id, .. }
            | Self::Replay { request_id, .. } => request_id,
        }
    }
}

/// Stateful one-line-in/one-line-out evaluation protocol engine.
#[derive(Debug)]
pub struct EvaluationProtocol {
    harness: EvaluationHarness,
    searches: BTreeMap<String, SearchSession>,
    search_rankers: BTreeMap<String, SearchRanker>,
    max_request_bytes: u64,
}

impl EvaluationProtocol {
    /// Creates an engine with the built-in corpus and default hard limits.
    pub fn new() -> Result<Self, EvaluationDiagnostic> {
        Ok(Self {
            harness: EvaluationHarness::new()?,
            searches: BTreeMap::new(),
            search_rankers: BTreeMap::new(),
            max_request_bytes: 4 * 1024 * 1024,
        })
    }

    /// Maximum encoded bytes accepted for one physical JSONL line.
    #[must_use]
    pub const fn max_request_bytes(&self) -> u64 {
        self.max_request_bytes
    }

    /// Processes one JSONL request and always returns one structured response.
    #[must_use]
    pub fn process_line(&mut self, line: &str) -> String {
        let request_id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("request_id")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        if u64::try_from(line.len()).unwrap_or(u64::MAX) > self.max_request_bytes {
            return response(
                &request_id,
                Err(EvaluationDiagnostic::new(
                    crate::model::EvaluationErrorCode::EvaluationBudgetExceeded,
                    "evaluation JSONL request exceeds byte limit",
                )),
            );
        }
        let request = match serde_json::from_str::<EvaluationRequest>(line) {
            Ok(request) => request,
            Err(error) => {
                return response(
                    &request_id,
                    Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationTranscriptInvalid,
                        format!("invalid evaluation JSONL request: {error}"),
                    )),
                );
            }
        };
        let request_id = request.request_id().to_owned();
        let result = self.handle(request);
        response(&request_id, result)
    }

    fn handle(&mut self, request: EvaluationRequest) -> Result<Value, EvaluationDiagnostic> {
        match request {
            EvaluationRequest::SearchStart {
                task,
                corpus_hash,
                ranking_policy,
                seed,
                beam_width,
                maximum_semantic_depth,
                maximum_children_retained_per_node,
                checkpoint_cadence_work_units,
                ..
            } => {
                if corpus_hash != self.harness.corpus().corpus_hash {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchRootStale,
                        "search start corpus hash is stale",
                    ));
                }
                let task_definition = self.harness.task(&task)?.clone();
                let descriptor = scripted_ranker(&ranking_policy, &feature_schema_v1()?, seed)?;
                let ranker = SearchRanker::Scripted { descriptor };
                let objective = SearchObjectiveDescriptor::new(
                    self.harness.corpus(),
                    &task_definition,
                    vec![
                        SearchObjectiveComponent {
                            kind: SearchObjectiveComponentKind::TaskCriterionSuccess,
                            direction: ObjectiveDirection::Maximize,
                        },
                        SearchObjectiveComponent {
                            kind: SearchObjectiveComponentKind::CompilerTerminalSuccess,
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
                let plan = SearchPlan::deterministic_beam_v1(
                    &objective,
                    &ranker,
                    beam_width,
                    maximum_semantic_depth,
                    maximum_children_retained_per_node,
                    checkpoint_cadence_work_units,
                )?;
                let session = SearchSession::start(
                    self.harness.corpus().clone(),
                    task,
                    objective,
                    plan,
                    &ranker,
                )?;
                let search = session.search_run_id.clone();
                let response = json!({
                    "search": search,
                    "status": session.status,
                    "initial_anchor_hash": session.objective.initial_anchor_hash,
                    "search_objective_hash": session.objective.search_objective_hash,
                    "search_plan_hash": session.plan.search_plan_hash,
                    "ranking_policy_hash": session.plan.ranking_policy_hash,
                });
                self.search_rankers.insert(search.clone(), ranker);
                self.searches.insert(search, session);
                Ok(response)
            }
            EvaluationRequest::SearchAdvance {
                search,
                search_objective_hash,
                search_plan_hash,
                maximum_work_units,
                ..
            } => {
                validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                let ranker = self.search_rankers.get(&search).cloned().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "search ranker is missing",
                    )
                })?;
                let session = self.searches.get_mut(&search).expect("validated search");
                let status =
                    session.advance(maximum_work_units, &ranker, &SearchLimits::default())?;
                Ok(json!({
                    "search": search,
                    "status": status,
                    "semantic_work": session.work.semantic_expansions,
                    "nodes": session.nodes.len(),
                    "edges": session.edges.len(),
                }))
            }
            EvaluationRequest::SearchStatus {
                search,
                search_objective_hash,
                search_plan_hash,
                ..
            } => {
                let session = validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                Ok(json!({
                    "search": search,
                    "status": session.status,
                    "frontier": session.frontier,
                    "semantic_work": session.work.semantic_expansions,
                    "terminal_candidates": session.terminal_candidates.len(),
                }))
            }
            EvaluationRequest::SearchCheckpoint {
                search,
                search_objective_hash,
                search_plan_hash,
                ..
            } => {
                let session = validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                serde_json::to_value(session.checkpoint(&SearchLimits::default())?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::SearchResume { checkpoint, .. } => {
                if checkpoint.session.ranking_policy.kind
                    == crate::ranking::RankingPolicyKind::LearnedLinear
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchUnsupportedSurface,
                        "JSONL learned-search resume requires an archive-retained model",
                    ));
                }
                let ranker = SearchRanker::Scripted {
                    descriptor: checkpoint.session.ranking_policy.clone(),
                };
                let session = SearchSession::resume(
                    &checkpoint,
                    self.harness.corpus(),
                    &ranker,
                    &SearchLimits::default(),
                )?;
                let search = session.search_run_id.clone();
                self.search_rankers.insert(search.clone(), ranker);
                self.searches.insert(search.clone(), session.clone());
                Ok(json!({
                    "search": search,
                    "status": session.status,
                    "semantic_work": session.work.semantic_expansions,
                    "search_checkpoint_hash": checkpoint.search_checkpoint_hash,
                }))
            }
            EvaluationRequest::SearchCancel {
                search,
                search_objective_hash,
                search_plan_hash,
                ..
            } => {
                validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                let ranker = self.search_rankers.get(&search).cloned().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "search ranker is missing",
                    )
                })?;
                let session = self.searches.get_mut(&search).expect("validated search");
                session.request_cancellation();
                session.advance(1, &ranker, &SearchLimits::default())?;
                Ok(json!({"search": search, "status": session.status}))
            }
            EvaluationRequest::SearchResult {
                search,
                search_objective_hash,
                search_plan_hash,
                ..
            } => {
                let session = validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                let result = session.result.as_ref().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchIncomplete,
                        "running search cannot publish a result",
                    )
                })?;
                serde_json::to_value(result).map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::SearchReplay {
                search,
                search_objective_hash,
                search_plan_hash,
                ..
            } => {
                let session = validate_search_request(
                    self.searches.get(&search),
                    &search_objective_hash,
                    &search_plan_hash,
                )?;
                let ranker = self.search_rankers.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "search ranker is missing",
                    )
                })?;
                replay_search(session, ranker, &SearchLimits::default())?;
                Ok(json!({
                    "search": search,
                    "replayed": true,
                    "training_calls": 0,
                    "network_calls": 0,
                    "provider_calls": 0,
                    "device_calls": 0,
                }))
            }
            EvaluationRequest::RankingPolicyList { seed, .. } => {
                serde_json::to_value(self.harness.ranking_policies(seed)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::RankingPolicyQuery { policy, seed, .. } => {
                serde_json::to_value(scripted_ranker(&policy, &feature_schema_v1()?, seed)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::ChoiceSetQuery { episode, .. } => {
                serde_json::to_value(self.harness.ranked_choice_set(&episode)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::CorpusList { .. } => Ok(json!({
                "name": self.harness.corpus().name,
                "version": self.harness.corpus().version,
                "corpus_hash": self.harness.corpus().corpus_hash,
                "tasks": self.harness.corpus().tasks.iter().map(|task| &task.id).collect::<Vec<_>>()
            })),
            EvaluationRequest::TaskQuery { task, .. } => {
                serde_json::to_value(self.harness.task(&task)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::RunStart {
                policy,
                kind,
                tasks,
                seeds,
                scripted,
                ..
            } => {
                let ranking_policy = self
                    .harness
                    .ranking_policies(seeds.first().copied().unwrap_or(0))?
                    .into_iter()
                    .find(|ranker| ranker.name == policy);
                let run = if let Some(ranker) = ranking_policy {
                    if scripted {
                        return Err(EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationRankingPolicyInvalid,
                            "ranked external loop must explicitly submit each selection",
                        ));
                    }
                    self.harness
                        .start_ranked_run(&ranker.name, &tasks, &seeds)?
                } else if scripted {
                    let _ = scripted_policy(&policy)?;
                    self.harness.run_scripted(&policy, &tasks, &seeds)?
                } else {
                    let descriptor = external_policy(kind.unwrap_or(PolicyKind::Hybrid), &policy)?;
                    self.harness.start_run(descriptor, &tasks, &seeds)?
                };
                Ok(json!({"run": run, "status": if scripted {"completed"} else {"ready"}}))
            }
            EvaluationRequest::RunStatus { run, .. } => {
                serde_json::to_value(self.harness.run(&run)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::RunCancel { run, .. } => {
                self.harness.cancel_run(&run)?;
                Ok(json!({"run": run, "status": "cancelled"}))
            }
            EvaluationRequest::EpisodeQuery { episode, .. }
            | EvaluationRequest::TranscriptQuery { episode, .. } => {
                let episode = self
                    .harness
                    .run_ids()
                    .find_map(|run_id| {
                        self.harness
                            .run(run_id)
                            .ok()?
                            .episodes
                            .iter()
                            .find(|candidate| candidate.id == episode)
                    })
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationEpisodeNotFound,
                            format!("episode `{episode}` does not exist"),
                        )
                    })?;
                serde_json::to_value(episode).map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::EpisodeNext { episode, .. } => {
                serde_json::to_value(self.harness.next_observation(&episode)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::EpisodeSubmit {
                run,
                episode,
                step,
                observation_hash,
                decision,
                usage,
                external_request_correlation_id,
                ..
            } => {
                if !self
                    .harness
                    .run(&run)?
                    .episodes
                    .iter()
                    .any(|candidate| candidate.id == episode)
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationEpisodeNotFound,
                        "episode does not belong to the submitted run",
                    ));
                }
                serde_json::to_value(self.harness.submit(
                    &episode,
                    &step,
                    &observation_hash,
                    decision,
                    usage,
                    external_request_correlation_id,
                )?)
                .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::EpisodeRank {
                run,
                episode,
                step,
                observation_hash,
                choice_set_hash,
                feature_schema_hash,
                ranking_policy_hash,
                decision,
                usage,
                external_request_correlation_id,
                ..
            } => {
                if !self
                    .harness
                    .run(&run)?
                    .episodes
                    .iter()
                    .any(|candidate| candidate.id == episode)
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationEpisodeNotFound,
                        "episode does not belong to the submitted run",
                    ));
                }
                let (trace, selection) = self.harness.rank_episode(RankingSubmission {
                    episode_id: episode,
                    step_id: step,
                    observation_hash,
                    choice_set_hash,
                    feature_schema_hash,
                    ranking_policy_hash,
                    decision,
                    usage,
                    correlation_id: external_request_correlation_id,
                })?;
                Ok(json!({"ranking_trace": trace, "selection": selection}))
            }
            EvaluationRequest::RankingTraceQuery { episode, .. } => {
                let episode = self
                    .harness
                    .run_ids()
                    .find_map(|run_id| {
                        self.harness
                            .run(run_id)
                            .ok()?
                            .episodes
                            .iter()
                            .find(|candidate| candidate.id == episode)
                    })
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationEpisodeNotFound,
                            "ranking episode does not exist",
                        )
                    })?;
                Ok(json!({
                    "episode": episode.id,
                    "traces": episode.steps.iter().filter_map(|step| step.ranking_trace.as_ref()).collect::<Vec<_>>()
                }))
            }
            EvaluationRequest::RankingAggregate { run, .. } => {
                Ok(json!(aggregate_ranking_metrics(self.harness.run(&run)?)))
            }
            EvaluationRequest::RankingCompare { runs, .. } => {
                serde_json::to_value(self.harness.compare(&runs)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::EpisodeFinish { episode, .. } => {
                let observation = self.harness.next_observation(&episode);
                match observation {
                    Err(error)
                        if error.code
                            == crate::model::EvaluationErrorCode::EvaluationAlreadyComplete =>
                    {
                        Ok(json!({"episode": episode, "status": "already_complete"}))
                    }
                    _ => Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationPolicyViolation,
                        "episode.finish cannot supply success; complete compiler-owned actions",
                    )),
                }
            }
            EvaluationRequest::Aggregate { run, .. } => {
                serde_json::to_value(self.harness.aggregate(&run)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::Compare { runs, .. } => {
                serde_json::to_value(self.harness.compare(&runs)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::ArchiveSave { path, runs, .. } => Ok(json!({
                "path": path,
                "archive_hash": self.harness.save_archive(Path::new(&path), &runs)?
            })),
            EvaluationRequest::ArchiveLoad { path, .. } => {
                let archive = self.harness.import_archive(Path::new(&path))?;
                Ok(json!({
                    "path": path,
                    "archive_hash": archive.archive_hash,
                    "runs": archive.runs.len(),
                    "verified": true
                }))
            }
            EvaluationRequest::Replay { run, .. } => {
                self.harness.replay_run(&run)?;
                Ok(json!({"run": run, "replayed": true, "external_calls": 0, "device_calls": 0}))
            }
        }
    }
}

fn validate_search_request<'a>(
    session: Option<&'a SearchSession>,
    objective_hash: &str,
    plan_hash: &str,
) -> Result<&'a SearchSession, EvaluationDiagnostic> {
    let session = session.ok_or_else(|| {
        EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationRunNotFound,
            "evaluation search does not exist",
        )
    })?;
    if session.objective.search_objective_hash != objective_hash
        || session.plan.search_plan_hash != plan_hash
    {
        return Err(EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationSearchCheckpointStale,
            "search request objective or plan hash is stale",
        ));
    }
    Ok(session)
}

impl Default for EvaluationProtocol {
    fn default() -> Self {
        Self::new().expect("built-in Stage 6A corpus is valid")
    }
}

fn serialization_error(error: &serde_json::Error) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(
        crate::model::EvaluationErrorCode::EvaluationTranscriptInvalid,
        format!("evaluation response serialization failed: {error}"),
    )
}

fn response(request_id: &str, result: Result<Value, EvaluationDiagnostic>) -> String {
    let envelope = match result {
        Ok(result) => json!({
            "ok": true,
            "request_id": request_id,
            "result": result,
            "diagnostics": []
        }),
        Err(error) => json!({
            "ok": false,
            "request_id": request_id,
            "error": error,
            "diagnostics": []
        }),
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        "{\"ok\":false,\"request_id\":\"unknown\",\"error\":{\"code\":\"EVALUATION_TRANSCRIPT_INVALID\",\"message\":\"response serialization failed\"},\"diagnostics\":[]}".to_owned()
    })
}
