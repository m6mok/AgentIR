//! Bounded JSONL transport for external Stage 6A agents.

use crate::{
    acquisition::{
        MeasurementAcquisitionCatalog, MeasurementAcquisitionCheckpoint,
        MeasurementAcquisitionExecutor, MeasurementAcquisitionFailurePolicy,
        MeasurementAcquisitionOrderingPolicy, MeasurementAcquisitionPlan,
        MeasurementAcquisitionPlanRequest, MeasurementAcquisitionSession,
        MeasurementAcquisitionStatus, SyntheticMeasurementAcquisitionExecutor,
        SyntheticMeasurementAcquisitionStore, WgpuMeasurementAcquisitionExecutor,
    },
    engine::{EvaluationHarness, RankingSubmission, external_policy, scripted_policy},
    measured::{
        MeasuredMetric, MeasuredObjectiveDescriptor, MeasurementAggregationMethod,
        MeasurementCohort, MeasurementCohortRequest, MeasurementReference,
        MeasurementValidationPolicy, measured_recommendation,
        measurement_cohort_from_verified_records, measurement_cohort_from_workspace,
    },
    model::{EvaluationDiagnostic, EvaluationTaskId, PolicyDecision, PolicyKind, TokenUsage},
    ranking::{RankingDecision, aggregate_ranking_metrics, feature_schema_v1, scripted_ranker},
    search::{
        ObjectiveDirection, SearchLimits, SearchObjectiveComponent, SearchObjectiveComponentKind,
        SearchObjectiveDescriptor, SearchPlan, SearchRanker, SearchSession, replay_search,
    },
};
use agentir_core::Workspace;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquisitionBenchmarkConfigRequest {
    warmups: u32,
    iterations: u32,
    input_distribution: String,
    tensor_dimensions: Vec<u64>,
}

impl From<AcquisitionBenchmarkConfigRequest> for agentir_core::backend_ir::HardwareBenchmarkConfig {
    fn from(request: AcquisitionBenchmarkConfigRequest) -> Self {
        Self {
            warmups: request.warmups,
            iterations: request.iterations,
            input_distribution: request.input_distribution,
            tensor_dimensions: request.tensor_dimensions,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum EvaluationRequest {
    #[serde(rename = "evaluation.measurement_acquisition.start")]
    MeasurementAcquisitionStart {
        request_id: String,
        task: EvaluationTaskId,
        root_anchor_hash: String,
        artifact_hashes: Vec<String>,
        benchmark_config: AcquisitionBenchmarkConfigRequest,
        records_per_artifact: u64,
        validation_policy: MeasurementValidationPolicy,
        checkpoint_cadence_slots: u64,
    },
    #[serde(rename = "evaluation.measurement_acquisition.advance")]
    MeasurementAcquisitionAdvance {
        request_id: String,
        session: String,
        measurement_acquisition_plan_hash: String,
        maximum_slots: u64,
    },
    #[serde(rename = "evaluation.measurement_acquisition.status")]
    MeasurementAcquisitionStatus {
        request_id: String,
        session: String,
        measurement_acquisition_plan_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.checkpoint")]
    MeasurementAcquisitionCheckpoint {
        request_id: String,
        session: String,
        measurement_acquisition_plan_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.resume")]
    MeasurementAcquisitionResume {
        request_id: String,
        measurement_acquisition_checkpoint_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.cancel")]
    MeasurementAcquisitionCancel {
        request_id: String,
        session: String,
        measurement_acquisition_plan_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.result")]
    MeasurementAcquisitionResult {
        request_id: String,
        session: String,
        measurement_acquisition_plan_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.replay")]
    MeasurementAcquisitionReplay {
        request_id: String,
        session: String,
        measurement_acquisition_result_hash: String,
    },
    #[serde(rename = "evaluation.measurement_acquisition.create_cohort")]
    MeasurementAcquisitionCreateCohort {
        request_id: String,
        session: String,
        measurement_acquisition_result_hash: String,
        aggregation_method: MeasurementAggregationMethod,
    },
    #[serde(rename = "evaluation.measurement_cohort.create")]
    MeasurementCohortCreate {
        request_id: String,
        task: EvaluationTaskId,
        initial_anchor_hash: String,
        measurements: Vec<MeasurementReference>,
        validation_policy: MeasurementValidationPolicy,
        records_per_artifact: u64,
        aggregation_method: MeasurementAggregationMethod,
    },
    #[serde(rename = "evaluation.measurement_cohort.query")]
    MeasurementCohortQuery {
        request_id: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.start")]
    MeasuredSearchStart {
        request_id: String,
        task: EvaluationTaskId,
        corpus_hash: String,
        measurement_cohort_hash: String,
        metric: MeasuredMetric,
        indifference_band_ppm: u64,
        ranking_policy: String,
        #[serde(default)]
        seed: u64,
        beam_width: u64,
        maximum_semantic_depth: u64,
        maximum_children_retained_per_node: u64,
        checkpoint_cadence_work_units: u64,
    },
    #[serde(rename = "evaluation.measured_search.advance")]
    MeasuredSearchAdvance {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
        maximum_work_units: u64,
    },
    #[serde(rename = "evaluation.measured_search.status")]
    MeasuredSearchStatus {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.checkpoint")]
    MeasuredSearchCheckpoint {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.resume")]
    MeasuredSearchResume {
        request_id: String,
        checkpoint: Box<crate::search::SearchCheckpoint>,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.cancel")]
    MeasuredSearchCancel {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.result")]
    MeasuredSearchResult {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
    #[serde(rename = "evaluation.measured_search.replay")]
    MeasuredSearchReplay {
        request_id: String,
        search: String,
        measured_objective_hash: String,
        measurement_cohort_hash: String,
    },
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
            Self::MeasurementAcquisitionStart { request_id, .. }
            | Self::MeasurementAcquisitionAdvance { request_id, .. }
            | Self::MeasurementAcquisitionStatus { request_id, .. }
            | Self::MeasurementAcquisitionCheckpoint { request_id, .. }
            | Self::MeasurementAcquisitionResume { request_id, .. }
            | Self::MeasurementAcquisitionCancel { request_id, .. }
            | Self::MeasurementAcquisitionResult { request_id, .. }
            | Self::MeasurementAcquisitionReplay { request_id, .. }
            | Self::MeasurementAcquisitionCreateCohort { request_id, .. }
            | Self::MeasurementCohortCreate { request_id, .. }
            | Self::MeasurementCohortQuery { request_id, .. }
            | Self::MeasuredSearchStart { request_id, .. }
            | Self::MeasuredSearchAdvance { request_id, .. }
            | Self::MeasuredSearchStatus { request_id, .. }
            | Self::MeasuredSearchCheckpoint { request_id, .. }
            | Self::MeasuredSearchResume { request_id, .. }
            | Self::MeasuredSearchCancel { request_id, .. }
            | Self::MeasuredSearchResult { request_id, .. }
            | Self::MeasuredSearchReplay { request_id, .. }
            | Self::SearchStart { request_id, .. }
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

#[derive(Debug)]
enum AcquisitionProtocolExecutor {
    Hardware(WgpuMeasurementAcquisitionExecutor),
    Synthetic(Box<SyntheticMeasurementAcquisitionExecutor>),
}

/// Stateful one-line-in/one-line-out evaluation protocol engine.
#[derive(Debug)]
pub struct EvaluationProtocol {
    harness: EvaluationHarness,
    searches: BTreeMap<String, SearchSession>,
    search_rankers: BTreeMap<String, SearchRanker>,
    measurement_workspace: Option<Workspace>,
    measurement_cohorts: BTreeMap<String, MeasurementCohort>,
    measured_objectives: BTreeMap<String, MeasuredObjectiveDescriptor>,
    measured_search_anchors: BTreeMap<String, (String, String)>,
    acquisition_catalog: Option<MeasurementAcquisitionCatalog>,
    acquisition_plans: BTreeMap<String, MeasurementAcquisitionPlan>,
    acquisition_sessions: BTreeMap<String, MeasurementAcquisitionSession>,
    acquisition_checkpoints: BTreeMap<String, MeasurementAcquisitionCheckpoint>,
    acquisition_results: BTreeMap<String, crate::acquisition::MeasurementAcquisitionResult>,
    acquisition_executor: AcquisitionProtocolExecutor,
    synthetic_acquisition_store: SyntheticMeasurementAcquisitionStore,
    max_request_bytes: u64,
}

impl EvaluationProtocol {
    /// Creates an engine with the built-in corpus and default hard limits.
    pub fn new() -> Result<Self, EvaluationDiagnostic> {
        Ok(Self {
            harness: EvaluationHarness::new()?,
            searches: BTreeMap::new(),
            search_rankers: BTreeMap::new(),
            measurement_workspace: None,
            measurement_cohorts: BTreeMap::new(),
            measured_objectives: BTreeMap::new(),
            measured_search_anchors: BTreeMap::new(),
            acquisition_catalog: None,
            acquisition_plans: BTreeMap::new(),
            acquisition_sessions: BTreeMap::new(),
            acquisition_checkpoints: BTreeMap::new(),
            acquisition_results: BTreeMap::new(),
            acquisition_executor: AcquisitionProtocolExecutor::Hardware(
                WgpuMeasurementAcquisitionExecutor { adapter_index: 0 },
            ),
            synthetic_acquisition_store: SyntheticMeasurementAcquisitionStore::default(),
            max_request_bytes: 4 * 1024 * 1024,
        })
    }

    /// Creates an evaluation protocol bound to one already verified production workspace.
    ///
    /// JSONL clients may reference only measurement IDs or hashes from this workspace;
    /// timing summaries and device metadata remain unrepresentable in requests.
    pub fn with_measurement_workspace(workspace: Workspace) -> Result<Self, EvaluationDiagnostic> {
        let mut protocol = Self::new()?;
        protocol.measurement_workspace = Some(workspace);
        Ok(protocol)
    }

    /// Creates a study/test-only protocol with an explicit synthetic catalog.
    pub fn with_synthetic_acquisition_catalog(
        catalog: MeasurementAcquisitionCatalog,
    ) -> Result<Self, EvaluationDiagnostic> {
        if !catalog.synthetic_fixture {
            return Err(EvaluationDiagnostic::new(
                crate::model::EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode,
                "synthetic protocol requires an explicitly labelled fixture catalog",
            ));
        }
        let mut protocol = Self::new()?;
        protocol.acquisition_catalog = Some(catalog);
        protocol.acquisition_executor = AcquisitionProtocolExecutor::Synthetic(Box::default());
        Ok(protocol)
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
            EvaluationRequest::MeasurementAcquisitionStart {
                task,
                root_anchor_hash,
                artifact_hashes,
                benchmark_config,
                records_per_artifact,
                validation_policy,
                checkpoint_cadence_slots,
                ..
            } => {
                self.harness.task(&task)?;
                let catalog = if let Some(catalog) = &self.acquisition_catalog {
                    if catalog.root_anchor_hash != root_anchor_hash {
                        return Err(EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                            "acquisition root differs from the server-owned fixture catalog",
                        ));
                    }
                    catalog.clone()
                } else {
                    let workspace = self.measurement_workspace.as_ref().ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                            "acquisition protocol has no verified production workspace",
                        )
                    })?;
                    MeasurementAcquisitionCatalog::from_workspace(
                        workspace,
                        root_anchor_hash.clone(),
                    )?
                };
                let plan = MeasurementAcquisitionPlan::new(
                    &catalog,
                    MeasurementAcquisitionPlanRequest {
                        corpus_hash: self.harness.corpus().corpus_hash.clone(),
                        task_id: task,
                        root_anchor_hash,
                        artifact_hashes,
                        benchmark_config: benchmark_config.into(),
                        records_per_artifact,
                        validation_policy,
                        ordering_policy:
                            MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
                        failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
                        checkpoint_cadence_slots,
                    },
                )?;
                let session = match &mut self.acquisition_executor {
                    AcquisitionProtocolExecutor::Hardware(executor) => {
                        let workspace = self.measurement_workspace.as_ref().ok_or_else(|| {
                            EvaluationDiagnostic::new(
                                crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                                "hardware acquisition has no production workspace",
                            )
                        })?;
                        MeasurementAcquisitionSession::start(
                            plan.clone(),
                            &catalog,
                            Some(workspace),
                            executor,
                        )?
                    }
                    AcquisitionProtocolExecutor::Synthetic(executor) => {
                        MeasurementAcquisitionSession::start(
                            plan.clone(),
                            &catalog,
                            None,
                            executor.as_mut(),
                        )?
                    }
                };
                let session_id = session.session_id.clone();
                let plan_hash = plan.measurement_acquisition_plan_hash.clone();
                self.acquisition_catalog = Some(catalog);
                self.acquisition_plans.insert(plan_hash.clone(), plan);
                self.acquisition_sessions
                    .insert(session_id.clone(), session.clone());
                Ok(json!({
                    "session": session_id,
                    "status": session.status,
                    "measurement_acquisition_plan_hash": plan_hash,
                    "next_slot": session.next_slot,
                    "total_slots": session.slots.len(),
                }))
            }
            EvaluationRequest::MeasurementAcquisitionAdvance {
                session,
                measurement_acquisition_plan_hash,
                maximum_slots,
                ..
            } => {
                let catalog = self.acquisition_catalog.clone().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                        "acquisition catalog is missing",
                    )
                })?;
                let mut staged_session = acquisition_session(
                    &self.acquisition_sessions,
                    &session,
                    &measurement_acquisition_plan_hash,
                )?
                .clone();
                match &mut self.acquisition_executor {
                    AcquisitionProtocolExecutor::Hardware(executor) => {
                        let mut staged_workspace = self.measurement_workspace.clone().ok_or_else(|| {
                            EvaluationDiagnostic::new(
                                crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                                "hardware acquisition workspace is missing",
                            )
                        })?;
                        let read_workspace = staged_workspace.clone();
                        staged_session.advance(
                            &mut staged_workspace,
                            &catalog,
                            Some(&read_workspace),
                            executor,
                            maximum_slots,
                        )?;
                        self.measurement_workspace = Some(staged_workspace);
                    }
                    AcquisitionProtocolExecutor::Synthetic(executor) => {
                        let mut staged_store = self.synthetic_acquisition_store.clone();
                        staged_session.advance(
                            &mut staged_store,
                            &catalog,
                            None,
                            executor.as_mut(),
                            maximum_slots,
                        )?;
                        self.synthetic_acquisition_store = staged_store;
                    }
                }
                let status = staged_session.status;
                let next_slot = staged_session.next_slot;
                self.acquisition_sessions
                    .insert(session.clone(), staged_session);
                Ok(json!({"session":session,"status":status,"next_slot":next_slot}))
            }
            EvaluationRequest::MeasurementAcquisitionStatus {
                session,
                measurement_acquisition_plan_hash,
                ..
            } => {
                let retained = acquisition_session(
                    &self.acquisition_sessions,
                    &session,
                    &measurement_acquisition_plan_hash,
                )?;
                Ok(json!({
                    "session":session,
                    "status":retained.status,
                    "next_slot":retained.next_slot,
                    "total_slots":retained.slots.len(),
                    "work":retained.work,
                }))
            }
            EvaluationRequest::MeasurementAcquisitionCheckpoint {
                session,
                measurement_acquisition_plan_hash,
                ..
            } => {
                let checkpoint = acquisition_session(
                    &self.acquisition_sessions,
                    &session,
                    &measurement_acquisition_plan_hash,
                )?
                .checkpoint()?;
                let hash = checkpoint.measurement_acquisition_checkpoint_hash.clone();
                self.acquisition_checkpoints
                    .insert(hash.clone(), checkpoint);
                Ok(json!({"session":session,"measurement_acquisition_checkpoint_hash":hash}))
            }
            EvaluationRequest::MeasurementAcquisitionResume {
                measurement_acquisition_checkpoint_hash,
                ..
            } => {
                let checkpoint = self
                    .acquisition_checkpoints
                    .get(&measurement_acquisition_checkpoint_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt,
                            "acquisition checkpoint does not exist",
                        )
                    })?
                    .clone();
                let catalog = self.acquisition_catalog.clone().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                        "acquisition catalog is missing",
                    )
                })?;
                let resumed = match &mut self.acquisition_executor {
                    AcquisitionProtocolExecutor::Hardware(executor) => {
                        let workspace = self.measurement_workspace.as_ref().ok_or_else(|| {
                            EvaluationDiagnostic::new(
                                crate::model::EvaluationErrorCode::EvaluationAcquisitionCheckpointStale,
                                "production workspace is missing",
                            )
                        })?;
                        let current = executor.preflight(
                            Some(workspace),
                            &catalog,
                            &checkpoint.session.plan,
                        )?;
                        MeasurementAcquisitionSession::resume(
                            &checkpoint,
                            workspace,
                            &catalog,
                            &current,
                        )?
                    }
                    AcquisitionProtocolExecutor::Synthetic(executor) => {
                        let current = executor.as_mut().preflight(
                            None,
                            &catalog,
                            &checkpoint.session.plan,
                        )?;
                        MeasurementAcquisitionSession::resume(
                            &checkpoint,
                            &self.synthetic_acquisition_store,
                            &catalog,
                            &current,
                        )?
                    }
                };
                let session = resumed.session_id.clone();
                let status = resumed.status;
                self.acquisition_sessions.insert(session.clone(), resumed);
                Ok(json!({"session":session,"status":status,"hardware_benchmark_calls":0}))
            }
            EvaluationRequest::MeasurementAcquisitionCancel {
                session,
                measurement_acquisition_plan_hash,
                ..
            } => {
                let retained = self.acquisition_sessions.get_mut(&session).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "acquisition session does not exist",
                    )
                })?;
                if retained.plan.measurement_acquisition_plan_hash
                    != measurement_acquisition_plan_hash
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionPlanCorrupt,
                        "acquisition plan hash is stale",
                    ));
                }
                let status = retained.cancel()?;
                Ok(json!({"session":session,"status":status}))
            }
            EvaluationRequest::MeasurementAcquisitionResult {
                session,
                measurement_acquisition_plan_hash,
                ..
            } => {
                let result = acquisition_session(
                    &self.acquisition_sessions,
                    &session,
                    &measurement_acquisition_plan_hash,
                )?
                .result()?;
                let hash = result.measurement_acquisition_result_hash.clone();
                self.acquisition_results.insert(hash, result.clone());
                serde_json::to_value(result).map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::MeasurementAcquisitionReplay {
                session,
                measurement_acquisition_result_hash,
                ..
            } => {
                let retained = self.acquisition_sessions.get(&session).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "acquisition session does not exist",
                    )
                })?;
                let catalog = self.acquisition_catalog.as_ref().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid,
                        "acquisition catalog is missing",
                    )
                })?;
                let replayed = match &self.acquisition_executor {
                    AcquisitionProtocolExecutor::Hardware(_) => retained.replay(
                        self.measurement_workspace.as_ref().ok_or_else(|| {
                            EvaluationDiagnostic::new(
                                crate::model::EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                                "production workspace is missing",
                            )
                        })?,
                        catalog,
                    )?,
                    AcquisitionProtocolExecutor::Synthetic(_) => {
                        retained.replay(&self.synthetic_acquisition_store, catalog)?
                    }
                };
                if replayed.measurement_acquisition_result_hash
                    != measurement_acquisition_result_hash
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionResultCorrupt,
                        "acquisition replay result hash mismatch",
                    ));
                }
                Ok(json!({
                    "session":session,
                    "measurement_acquisition_result_hash":measurement_acquisition_result_hash,
                    "replayed":true,
                    "benchmark_calls":0,
                    "device_calls":0,
                }))
            }
            EvaluationRequest::MeasurementAcquisitionCreateCohort {
                session,
                measurement_acquisition_result_hash,
                aggregation_method,
                ..
            } => {
                let retained = self.acquisition_sessions.get(&session).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "acquisition session does not exist",
                    )
                })?;
                let result = retained.result()?;
                if result.status != MeasurementAcquisitionStatus::Complete
                    || result.measurement_acquisition_result_hash
                        != measurement_acquisition_result_hash
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationAcquisitionUnequalRecords,
                        "only a complete exact acquisition result can create a cohort",
                    ));
                }
                let request = MeasurementCohortRequest {
                    corpus_hash: retained.plan.corpus_hash.clone(),
                    task_id: retained.plan.task_id.clone(),
                    initial_anchor_hash: retained.plan.root_anchor_hash.clone(),
                    validation_policy: retained.plan.validation_policy,
                    records_per_artifact: retained.plan.records_per_artifact,
                    aggregation_method,
                };
                let cohort = match &self.acquisition_executor {
                    AcquisitionProtocolExecutor::Hardware(_) => {
                        let workspace = self.measurement_workspace.as_ref().ok_or_else(|| {
                            EvaluationDiagnostic::new(
                                crate::model::EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                                "production workspace is missing",
                            )
                        })?;
                        let references = result
                            .measurement_ids
                            .iter()
                            .cloned()
                            .map(MeasurementReference::Id)
                            .collect::<Vec<_>>();
                        measurement_cohort_from_workspace(workspace, &references, request)?
                    }
                    AcquisitionProtocolExecutor::Synthetic(_) => {
                        let records = result
                            .measurement_ids
                            .iter()
                            .map(|id| {
                                self.synthetic_acquisition_store
                                    .records
                                    .get(id)
                                    .cloned()
                                    .map(|record| crate::measured::MeasurementCohortRecord {
                                        measurement_id: id.clone(),
                                        record,
                                    })
                                    .ok_or_else(|| {
                                        EvaluationDiagnostic::new(
                                            crate::model::EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing,
                                            "synthetic completed measurement is missing",
                                        )
                                    })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        measurement_cohort_from_verified_records(records, request)?
                    }
                };
                let hash = cohort.measurement_cohort_hash.clone();
                self.measurement_cohorts.insert(hash.clone(), cohort);
                Ok(json!({
                    "measurement_acquisition_result_hash":measurement_acquisition_result_hash,
                    "measurement_cohort_hash":hash,
                    "hardware_calls":0,
                }))
            }
            EvaluationRequest::MeasurementCohortCreate {
                task,
                initial_anchor_hash,
                measurements,
                validation_policy,
                records_per_artifact,
                aggregation_method,
                ..
            } => {
                let workspace = self.measurement_workspace.as_ref().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationMeasurementNotFound,
                        "evaluation protocol has no verified production measurement workspace",
                    )
                })?;
                self.harness.task(&task)?;
                let cohort = measurement_cohort_from_workspace(
                    workspace,
                    &measurements,
                    MeasurementCohortRequest {
                        corpus_hash: self.harness.corpus().corpus_hash.clone(),
                        task_id: task,
                        initial_anchor_hash,
                        validation_policy,
                        records_per_artifact,
                        aggregation_method,
                    },
                )?;
                let hash = cohort.measurement_cohort_hash.clone();
                self.measurement_cohorts.insert(hash.clone(), cohort);
                Ok(json!({"measurement_cohort_hash": hash, "hardware_calls": 0}))
            }
            EvaluationRequest::MeasurementCohortQuery {
                measurement_cohort_hash,
                ..
            } => serde_json::to_value(
                self.measurement_cohorts
                    .get(&measurement_cohort_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationMeasurementNotFound,
                            "measurement cohort does not exist",
                        )
                    })?,
            )
            .map_err(|error| serialization_error(&error)),
            EvaluationRequest::MeasuredSearchStart {
                task,
                corpus_hash,
                measurement_cohort_hash,
                metric,
                indifference_band_ppm,
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
                        "measured search start corpus hash is stale",
                    ));
                }
                let cohort = self
                    .measurement_cohorts
                    .get(&measurement_cohort_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationMeasurementNotFound,
                            "measured search cohort does not exist",
                        )
                    })?
                    .clone();
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
                if cohort.task_id != task
                    || cohort.initial_anchor_hash != objective.initial_anchor_hash
                    || cohort.corpus_hash != corpus_hash
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                        "measurement cohort does not match the exact Stage 7A root",
                    ));
                }
                let measured = MeasuredObjectiveDescriptor::new(
                    &cohort,
                    metric,
                    indifference_band_ppm,
                    objective.search_objective_hash.clone(),
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
                let measured_hash = measured.measured_objective_hash.clone();
                self.measured_objectives
                    .insert(measured_hash.clone(), measured);
                self.measured_search_anchors.insert(
                    search.clone(),
                    (measurement_cohort_hash.clone(), measured_hash.clone()),
                );
                self.search_rankers.insert(search.clone(), ranker);
                self.searches.insert(search.clone(), session.clone());
                Ok(json!({
                    "search": search,
                    "status": session.status,
                    "search_objective_hash": session.objective.search_objective_hash,
                    "search_plan_hash": session.plan.search_plan_hash,
                    "measurement_cohort_hash": measurement_cohort_hash,
                    "measured_objective_hash": measured_hash,
                    "hardware_calls": 0,
                }))
            }
            EvaluationRequest::MeasuredSearchAdvance {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                maximum_work_units,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let ranker = self.search_rankers.get(&search).cloned().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "measured search ranker is missing",
                    )
                })?;
                let session = self.searches.get_mut(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                let status =
                    session.advance(maximum_work_units, &ranker, &SearchLimits::default())?;
                Ok(json!({
                    "search": search,
                    "status": status,
                    "semantic_work": session.work.semantic_expansions,
                    "hardware_calls": 0,
                }))
            }
            EvaluationRequest::MeasuredSearchStatus {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let session = self.searches.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                Ok(json!({
                    "search": search,
                    "status": session.status,
                    "semantic_work": session.work.semantic_expansions,
                    "terminal_candidates": session.terminal_candidates.len(),
                    "hardware_calls": 0,
                }))
            }
            EvaluationRequest::MeasuredSearchCheckpoint {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let session = self.searches.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                Ok(json!({
                    "checkpoint": session.checkpoint(&SearchLimits::default())?,
                    "measurement_cohort_hash": measurement_cohort_hash,
                    "measured_objective_hash": measured_objective_hash,
                    "hardware_calls": 0,
                }))
            }
            EvaluationRequest::MeasuredSearchResume {
                checkpoint,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                let cohort = self
                    .measurement_cohorts
                    .get(&measurement_cohort_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationMeasurementNotFound,
                            "resume cohort does not exist",
                        )
                    })?;
                let objective = self
                    .measured_objectives
                    .get(&measured_objective_hash)
                    .ok_or_else(|| {
                        EvaluationDiagnostic::new(
                            crate::model::EvaluationErrorCode::EvaluationMeasuredObjectiveInvalid,
                            "resume measured objective does not exist",
                        )
                    })?;
                objective.validate(cohort)?;
                if objective.structural_fallback_objective_hash != checkpoint.search_objective_hash
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationMeasuredAnchorStale,
                        "resume checkpoint differs from the measured objective root",
                    ));
                }
                if checkpoint.session.ranking_policy.kind
                    == crate::ranking::RankingPolicyKind::LearnedLinear
                {
                    return Err(EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchUnsupportedSurface,
                        "learned measured resume requires an archive-retained model",
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
                self.measured_search_anchors.insert(
                    search.clone(),
                    (measurement_cohort_hash, measured_objective_hash),
                );
                self.search_rankers.insert(search.clone(), ranker);
                self.searches.insert(search.clone(), session.clone());
                Ok(json!({
                    "search": search,
                    "status": session.status,
                    "semantic_work": session.work.semantic_expansions,
                    "hardware_calls": 0,
                }))
            }
            EvaluationRequest::MeasuredSearchCancel {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let ranker = self.search_rankers.get(&search).cloned().ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "measured search ranker is missing",
                    )
                })?;
                let session = self.searches.get_mut(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                session.request_cancellation();
                session.advance(1, &ranker, &SearchLimits::default())?;
                Ok(json!({"search": search, "status": session.status, "hardware_calls": 0}))
            }
            EvaluationRequest::MeasuredSearchResult {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let session = self.searches.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                let cohort = &self.measurement_cohorts[&measurement_cohort_hash];
                let objective = &self.measured_objectives[&measured_objective_hash];
                serde_json::to_value(measured_recommendation(session, cohort, objective)?)
                    .map_err(|error| serialization_error(&error))
            }
            EvaluationRequest::MeasuredSearchReplay {
                search,
                measured_objective_hash,
                measurement_cohort_hash,
                ..
            } => {
                validate_measured_search_request(
                    &self.measured_search_anchors,
                    &search,
                    &measurement_cohort_hash,
                    &measured_objective_hash,
                )?;
                let session = self.searches.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationRunNotFound,
                        "measured search does not exist",
                    )
                })?;
                let ranker = self.search_rankers.get(&search).ok_or_else(|| {
                    EvaluationDiagnostic::new(
                        crate::model::EvaluationErrorCode::EvaluationSearchPlanInvalid,
                        "measured search ranker is missing",
                    )
                })?;
                replay_search(session, ranker, &SearchLimits::default())?;
                let recommendation = measured_recommendation(
                    session,
                    &self.measurement_cohorts[&measurement_cohort_hash],
                    &self.measured_objectives[&measured_objective_hash],
                )?;
                Ok(json!({
                    "search": search,
                    "replayed": true,
                    "measured_recommendation_hash": recommendation.measured_recommendation_hash,
                    "benchmark_calls": 0,
                    "device_calls": 0,
                    "network_calls": 0,
                    "training_calls": 0,
                }))
            }
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

fn acquisition_session<'a>(
    sessions: &'a BTreeMap<String, MeasurementAcquisitionSession>,
    session: &str,
    plan_hash: &str,
) -> Result<&'a MeasurementAcquisitionSession, EvaluationDiagnostic> {
    let retained = sessions.get(session).ok_or_else(|| {
        EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationRunNotFound,
            "measurement acquisition session does not exist",
        )
    })?;
    if retained.plan.measurement_acquisition_plan_hash != plan_hash {
        return Err(EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationAcquisitionPlanCorrupt,
            "measurement acquisition plan hash is stale",
        ));
    }
    Ok(retained)
}

fn validate_measured_search_request(
    anchors: &BTreeMap<String, (String, String)>,
    search: &str,
    cohort_hash: &str,
    objective_hash: &str,
) -> Result<(), EvaluationDiagnostic> {
    let retained = anchors.get(search).ok_or_else(|| {
        EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationRunNotFound,
            "evaluation measured search does not exist",
        )
    })?;
    if retained.0 != cohort_hash || retained.1 != objective_hash {
        return Err(EvaluationDiagnostic::new(
            crate::model::EvaluationErrorCode::EvaluationMeasuredAnchorStale,
            "measured search cohort or objective hash is stale",
        ));
    }
    Ok(())
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
