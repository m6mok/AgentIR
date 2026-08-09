//! Bounded JSONL transport for external Stage 6A agents.

use crate::{
    engine::{EvaluationHarness, external_policy, scripted_policy},
    model::{EvaluationDiagnostic, EvaluationTaskId, PolicyDecision, PolicyKind, TokenUsage},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum EvaluationRequest {
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
            Self::CorpusList { request_id }
            | Self::TaskQuery { request_id, .. }
            | Self::RunStart { request_id, .. }
            | Self::RunStatus { request_id, .. }
            | Self::RunCancel { request_id, .. }
            | Self::EpisodeQuery { request_id, .. }
            | Self::EpisodeNext { request_id, .. }
            | Self::EpisodeSubmit { request_id, .. }
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
    max_request_bytes: u64,
}

impl EvaluationProtocol {
    /// Creates an engine with the built-in corpus and default hard limits.
    pub fn new() -> Result<Self, EvaluationDiagnostic> {
        Ok(Self {
            harness: EvaluationHarness::new()?,
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
                let run = if scripted {
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
