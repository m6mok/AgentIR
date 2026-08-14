use super::corpus::{audit_prompts, prompt_for};
use super::grader::{GradeRecord, StableDiagnostic, grade_response};
use super::report::{build_summary, write_summary};
use super::{
    AnyResult, Configuration, ExecutionPlan, ExperimentRecord, MAX_INITIAL_CALLS,
    MAX_PARALLEL_CALLS, MAX_PROVIDER_RESPONSE_BYTES, MAX_REPAIRS_PER_ATTEMPT, MAX_TOTAL_CALLS,
    PlanCell, PrivateCorpus, PrivateCorpusTask, PublicCorpus, append_event, atomic_json,
    atomic_write, attempt_directory, read_json, verify_loaded_artifacts,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProviderUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProviderResponse {
    status: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    response: String,
    #[serde(default)]
    latency_ms: Option<u64>,
    #[serde(default)]
    usage: Option<ProviderUsage>,
    #[serde(default)]
    provider_request_id: Option<String>,
    #[serde(default)]
    structured_output: Option<StructuredOutputMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StructuredOutputMetadata {
    capability: String,
    enforced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProviderMetadata {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<ProviderUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    structured_output: Option<StructuredOutputMetadata>,
}

#[derive(Clone, Debug, Serialize)]
struct RunnerBinding<'a> {
    task_id: &'a str,
    session_id: &'a str,
    phase: &'a str,
    surface_schema: &'a str,
    frame_hash: Option<&'a str>,
    previous_payload_sha256: Option<String>,
    diagnostic_code: Option<&'a str>,
    diagnostic_path: Option<&'a str>,
    repair_attempt: usize,
}

impl ProviderMetadata {
    pub(crate) fn completed(&self) -> bool {
        self.status == "completed"
    }

    pub(crate) fn input_tokens(&self) -> Option<u64> {
        self.usage.as_ref().and_then(|usage| usage.input_tokens)
    }

    pub(crate) fn output_tokens(&self) -> Option<u64> {
        self.usage.as_ref().and_then(|usage| usage.output_tokens)
    }
}

pub(crate) fn run_experiment(
    output: &Path,
    configuration: &Configuration,
    runner: &Path,
    parallel: usize,
) -> AnyResult<()> {
    if configuration.placeholders() {
        return Err("model or reasoning placeholders are unresolved; configure them and regenerate before external calls".into());
    }
    let public: PublicCorpus = read_json(&output.join("corpus.json"))?;
    let private: PrivateCorpus = read_json(&output.join("corpus-private.json"))?;
    let plan: ExecutionPlan = read_json(&output.join("execution-plan.json"))?;
    verify_loaded_artifacts(&public, &private, &plan)?;
    verify_runtime_configuration(configuration, &plan)?;
    if plan.planned_initial_calls > MAX_INITIAL_CALLS
        || plan.planned_maximum_total_calls > MAX_TOTAL_CALLS
        || parallel > MAX_PARALLEL_CALLS
    {
        return Err("execution plan exceeds enforced call limits".into());
    }
    let tasks = private
        .tasks
        .iter()
        .map(|task| (task.public.task_id.clone(), task.clone()))
        .collect::<BTreeMap<_, _>>();
    let cells = Arc::new(plan.cells.clone());
    let tasks = Arc::new(tasks);
    let next = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let event_lock = Arc::new(Mutex::new(()));
    let output = Arc::new(output.to_path_buf());
    let runner = Arc::new(runner.to_path_buf());
    let mut workers = Vec::new();
    for _ in 0..parallel {
        let cells = Arc::clone(&cells);
        let tasks = Arc::clone(&tasks);
        let next = Arc::clone(&next);
        let errors = Arc::clone(&errors);
        let event_lock = Arc::clone(&event_lock);
        let output = Arc::clone(&output);
        let runner = Arc::clone(&runner);
        let native = configuration.native;
        workers.push(thread::spawn(move || {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(cell) = cells.get(index) else {
                    break;
                };
                let task = &tasks[&cell.task_id];
                if let Err(error) = execute_cell(&output, &runner, cell, task, native, &event_lock)
                {
                    errors
                        .lock()
                        .expect("error lock")
                        .push(format!("{}: {error}", cell.cell_id));
                }
            }
        }));
    }
    for worker in workers {
        worker.join().map_err(|_| "runner worker panicked")?;
    }
    let errors = errors.lock().map_err(|_| "error lock poisoned")?;
    if !errors.is_empty() {
        return Err(format!(
            "{} harness cell errors: {}",
            errors.len(),
            errors.join("; ")
        )
        .into());
    }
    drop(errors);
    let summary = build_summary(output.as_path(), &public, &plan, "executed")?;
    write_summary(output.as_path(), &summary)?;
    let mut experiment: ExperimentRecord = read_json(&output.join("experiment.json"))?;
    experiment.external_calls_executed = true;
    experiment.runner_configured = true;
    atomic_json(&output.join("experiment.json"), &experiment)?;
    Ok(())
}

fn verify_runtime_configuration(
    configuration: &Configuration,
    plan: &ExecutionPlan,
) -> AnyResult<()> {
    let planned_models = plan
        .cells
        .iter()
        .map(|cell| cell.model.as_str())
        .collect::<BTreeSet<_>>();
    let configured_models = configuration
        .models
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let planned_reasoning = plan
        .cells
        .iter()
        .map(|cell| cell.reasoning_level.as_str())
        .collect::<BTreeSet<_>>();
    let configured_reasoning = configuration
        .reasoning_levels
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if planned_models != configured_models || planned_reasoning != configured_reasoning {
        return Err(
            "configured models or reasoning levels differ from execution-plan.json; regenerate before external calls"
                .into(),
        );
    }
    Ok(())
}

fn execute_cell(
    output: &Path,
    runner: &Path,
    cell: &PlanCell,
    task: &PrivateCorpusTask,
    native: bool,
    event_lock: &Mutex<()>,
) -> AnyResult<()> {
    let directory = attempt_directory(output, cell);
    std::fs::create_dir_all(&directory)?;
    let initial_path = directory.join("initial.txt");
    let initial_grade_path = directory.join("initial-grade.json");
    if initial_path.exists() {
        if !initial_grade_path.exists() {
            let raw = std::fs::read_to_string(&initial_path)?;
            let grade = grade_response(&raw, "initial", cell.surface, task, native);
            atomic_json(&initial_grade_path, &grade)?;
        }
    } else if directory.join("initial-provider-state.json").exists() {
        return Ok(());
    } else {
        let prompt = prompt_for(task, cell.surface);
        let request = runner_request(cell, task, "initial", &prompt, None, None);
        atomic_json(
            &directory.join("initial-provider-state.json"),
            &json!({"status":"prepared","cell_id":cell.cell_id}),
        )?;
        locked_event(
            output,
            event_lock,
            &json!({"event":"provider_prepared","cell_id":cell.cell_id,"phase":"initial"}),
        )?;
        let Some(raw) = invoke_and_record(runner, &request, &directory, "initial", &initial_path)?
        else {
            return Ok(());
        };
        let grade = grade_response(&raw, "initial", cell.surface, task, native);
        atomic_json(&initial_grade_path, &grade)?;
    }
    let initial_grade: GradeRecord = read_json(&initial_grade_path)?;
    if initial_grade.final_success || initial_grade.harness_error || MAX_REPAIRS_PER_ATTEMPT == 0 {
        return Ok(());
    }
    let repair_path = directory.join("repair.txt");
    let repair_grade_path = directory.join("repair-grade.json");
    if repair_path.exists() {
        if !repair_grade_path.exists() {
            let raw = std::fs::read_to_string(&repair_path)?;
            let grade = grade_response(&raw, "repair", cell.surface, task, native);
            atomic_json(&repair_grade_path, &grade)?;
        }
        return Ok(());
    }
    if directory.join("repair-provider-state.json").exists() {
        return Ok(());
    }
    let previous = std::fs::read_to_string(&initial_path)?;
    let diagnostic = initial_grade
        .diagnostic
        .as_ref()
        .ok_or("failed model grade has no local diagnostic")?;
    let prompt = repair_prompt(task, cell, &previous, diagnostic)?;
    atomic_write(&directory.join("repair-prompt.txt"), prompt.as_bytes())?;
    atomic_json(
        &directory.join("repair-provider-state.json"),
        &json!({"status":"prepared","cell_id":cell.cell_id}),
    )?;
    locked_event(
        output,
        event_lock,
        &json!({"event":"provider_prepared","cell_id":cell.cell_id,"phase":"repair"}),
    )?;
    let request = runner_request(
        cell,
        task,
        "repair",
        &prompt,
        Some(previous.as_bytes()),
        Some(diagnostic),
    );
    if let Some(raw) = invoke_and_record(runner, &request, &directory, "repair", &repair_path)? {
        let grade = grade_response(&raw, "repair", cell.surface, task, native);
        atomic_json(&repair_grade_path, &grade)?;
    }
    Ok(())
}

fn invoke_and_record(
    runner: &Path,
    request: &Value,
    directory: &Path,
    phase: &str,
    raw_path: &Path,
) -> AnyResult<Option<String>> {
    match invoke_runner(runner, request) {
        Ok(response) if response.status == "ok" => {
            atomic_write(raw_path, response.response.as_bytes())?;
            atomic_json(
                &directory.join(format!("{phase}-provider.json")),
                &ProviderMetadata {
                    status: "completed".to_owned(),
                    latency_ms: response.latency_ms,
                    usage: response.usage,
                    provider_request_id: response.provider_request_id,
                    error: None,
                    structured_output: Some(response.structured_output.unwrap_or_else(|| {
                        StructuredOutputMetadata {
                            capability: "unreported".to_owned(),
                            enforced: false,
                            schema_id: None,
                        }
                    })),
                },
            )?;
            Ok(Some(response.response))
        }
        Ok(response) => {
            atomic_json(
                &directory.join(format!("{phase}-provider.json")),
                &ProviderMetadata {
                    status: "provider_failure".to_owned(),
                    latency_ms: response.latency_ms,
                    usage: response.usage,
                    provider_request_id: response.provider_request_id,
                    error: Some(format!("runner status {}", response.status)),
                    structured_output: Some(response.structured_output.unwrap_or_else(|| {
                        StructuredOutputMetadata {
                            capability: "unreported".to_owned(),
                            enforced: false,
                            schema_id: None,
                        }
                    })),
                },
            )?;
            Ok(None)
        }
        Err(error) => {
            atomic_json(
                &directory.join(format!("{phase}-provider.json")),
                &ProviderMetadata {
                    status: "indeterminate".to_owned(),
                    latency_ms: None,
                    usage: None,
                    provider_request_id: None,
                    error: Some(error.to_string()),
                    structured_output: None,
                },
            )?;
            Ok(None)
        }
    }
}

fn runner_request(
    cell: &PlanCell,
    task: &PrivateCorpusTask,
    phase: &str,
    prompt: &str,
    previous_payload: Option<&[u8]>,
    diagnostic: Option<&StableDiagnostic>,
) -> Value {
    let schema: Value = serde_json::from_str(cell.surface.sdk().json_schema())
        .expect("embedded authoring schema is valid JSON");
    let binding = RunnerBinding {
        task_id: &task.public.task_id,
        session_id: &cell.cell_id,
        phase,
        surface_schema: cell.surface.sdk().schema(),
        frame_hash: None,
        previous_payload_sha256: previous_payload.map(sha256_hex),
        diagnostic_code: diagnostic.map(|item| item.taxonomy.as_str()),
        diagnostic_path: diagnostic.map(|item| item.path.as_str()),
        repair_attempt: usize::from(phase == "repair"),
    };
    json!({
        "format":"agentir.authoring_eval.runner_request.v2",
        "session_id":cell.cell_id,
        "phase":phase,
        "model":cell.model,
        "reasoning_level":cell.reasoning_level,
        "prompt":prompt,
        "response_contract":{
            "format":"agentir.authoring_eval.response_contract.v2",
            "schema_id":cell.surface.sdk().schema(),
            "schema_version":1,
            "json_schema":schema,
            "required_top_level_schema":cell.surface.sdk().schema(),
            "allow_extra_text":false,
            "maximum_output_bytes":MAX_PROVIDER_RESPONSE_BYTES,
            "structured_output_policy":"enforce_if_supported_and_report_capability",
            "binding":binding,
        }
    })
}

fn invoke_runner(runner: &Path, request: &Value) -> AnyResult<ProviderResponse> {
    let mut child = Command::new(runner)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("runner stdin unavailable")?
        .write_all(serde_json::to_string(request)?.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "runner outcome indeterminate: exit {} (stderr deliberately not recorded)",
            output.status
        )
        .into());
    }
    let response: ProviderResponse = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("runner outcome indeterminate: invalid envelope: {error}"))?;
    validate_runner_response_binding(request, &response)?;
    Ok(response)
}

fn validate_runner_response_binding(request: &Value, response: &ProviderResponse) -> AnyResult<()> {
    let expected_session = request
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or("runner request has no session_id")?;
    let expected_phase = request
        .get("phase")
        .and_then(Value::as_str)
        .ok_or("runner request has no phase")?;
    if response.session_id.as_deref() != Some(expected_session)
        || response.phase.as_deref() != Some(expected_phase)
    {
        return Err("runner response is not bound to the exact task session and phase".into());
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn repair_prompt(
    task: &PrivateCorpusTask,
    cell: &PlanCell,
    previous: &str,
    diagnostic: &StableDiagnostic,
) -> AnyResult<String> {
    Ok(format!(
        "Your previous AgentIR authoring payload was rejected.\n\nPublic task:\n{}\n\nPrevious payload:\n{}\n\nDiagnostic:\n{}\n\nReturn the complete corrected payload in the same authoring format.\nChange the reported defect and recheck every reference governed by the same public rule. Preserve unrelated operations, operation order, operand order, exact fma operations, bindings, cycles, warmup semantics, and final yield.\nDo not request, reconstruct, or expose the hidden oracle.\n{}\nReturn only one JSON object.\n",
        task.public.public_specification,
        previous,
        serde_json::to_string(diagnostic)?,
        cell.surface.sdk().model_instruction()
    ))
}

fn locked_event(output: &Path, lock: &Mutex<()>, event: &Value) -> AnyResult<()> {
    let _guard = lock.lock().map_err(|_| "event lock poisoned")?;
    append_event(output, event)
}

pub(crate) fn replay(output: &Path, native: bool) -> AnyResult<()> {
    replay_saved(output, native, true)
}

pub(crate) fn verify_replay(output: &Path, native: bool) -> AnyResult<()> {
    replay_saved(output, native, false)
}

fn replay_saved(output: &Path, native: bool, update_reports: bool) -> AnyResult<()> {
    let public: PublicCorpus = read_json(&output.join("corpus.json"))?;
    let private: PrivateCorpus = read_json(&output.join("corpus-private.json"))?;
    let plan: ExecutionPlan = read_json(&output.join("execution-plan.json"))?;
    verify_loaded_artifacts(&public, &private, &plan)?;
    audit_prompts(&private.tasks)?;
    let tasks = private
        .tasks
        .iter()
        .map(|task| (task.public.task_id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut replayed = 0;
    for cell in &plan.cells {
        let directory = super::attempt_directory(output, cell);
        for (phase, raw_name, grade_name) in [
            ("initial", "initial.txt", "initial-grade.json"),
            ("repair", "repair.txt", "repair-grade.json"),
        ] {
            let raw_path = directory.join(raw_name);
            if !raw_path.exists() {
                continue;
            }
            let raw = std::fs::read_to_string(&raw_path)?;
            let replayed_grade =
                grade_response(&raw, phase, cell.surface, tasks[&*cell.task_id], native);
            let grade_path = directory.join(grade_name);
            if grade_path.exists() {
                let stored: GradeRecord = read_json(&grade_path)?;
                if stored != replayed_grade {
                    return Err(
                        format!("offline replay diverged at {} {phase}", cell.cell_id).into(),
                    );
                }
            } else if update_reports {
                atomic_json(&grade_path, &replayed_grade)?;
            } else {
                return Err(format!(
                    "saved raw response has no grade at {} {phase}",
                    cell.cell_id
                )
                .into());
            }
            replayed += 1;
        }
    }
    if update_reports {
        let status = if replayed == 0 { "dry_run" } else { "replayed" };
        let summary = build_summary(output, &public, &plan, status)?;
        write_summary(output, &summary)?;
        append_event(
            output,
            &json!({"event":"offline_replay","graded_responses":replayed}),
        )?;
        println!("offline replay graded {replayed} saved responses with zero model calls");
    } else {
        println!(
            "read-only offline replay verified {replayed} saved responses with zero model calls and no artifact writes"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval_harness::corpus::build_corpus;

    #[test]
    fn runner_v2_carries_exact_schema_and_initial_binding() {
        let task = build_corpus().unwrap().remove(0);
        let cell = PlanCell {
            cell_id: "session-1".to_owned(),
            model: "model".to_owned(),
            reasoning_level: "low".to_owned(),
            task_id: task.public.task_id.clone(),
            surface: super::super::SurfaceName::Staged,
            trial_index: 0,
        };
        let request = runner_request(&cell, &task, "initial", "prompt", None, None);
        assert_eq!(
            request["format"],
            "agentir.authoring_eval.runner_request.v2"
        );
        assert_eq!(
            request["response_contract"]["schema_id"],
            agentir_authoring::STAGED_SCHEMA
        );
        assert_eq!(
            request["response_contract"]["json_schema"],
            serde_json::from_str::<Value>(agentir_authoring::STAGED_JSON_SCHEMA).unwrap()
        );
        assert_eq!(
            request["response_contract"]["binding"]["task_id"],
            task.public.task_id
        );
        assert!(request["response_contract"]["binding"]["previous_payload_sha256"].is_null());
    }

    #[test]
    fn repair_binding_covers_session_task_payload_and_diagnostic() {
        let task = build_corpus().unwrap().remove(0);
        let cell = PlanCell {
            cell_id: "session-1".to_owned(),
            model: "model".to_owned(),
            reasoning_level: "low".to_owned(),
            task_id: task.public.task_id.clone(),
            surface: super::super::SurfaceName::Graph,
            trial_index: 0,
        };
        let diagnostic = grade_response(
            r#"{"schema":"agentir.elementwise_graph.v1","operations":[],"yield":0,"extra":1}"#,
            "initial",
            super::super::SurfaceName::Graph,
            &task,
            false,
        )
        .diagnostic
        .expect("local diagnostic");
        let request = runner_request(
            &cell,
            &task,
            "repair",
            "prompt",
            Some(b"raw payload"),
            Some(&diagnostic),
        );
        let binding = &request["response_contract"]["binding"];
        assert_eq!(binding["session_id"], "session-1");
        assert_eq!(binding["task_id"], task.public.task_id);
        assert_eq!(binding["surface_schema"], agentir_authoring::GRAPH_SCHEMA);
        assert_eq!(binding["diagnostic_code"], "UNKNOWN_FIELD");
        assert_eq!(binding["diagnostic_path"], "$.extra");
        assert_eq!(binding["repair_attempt"], 1);
        assert_eq!(
            binding["previous_payload_sha256"].as_str().unwrap().len(),
            64
        );

        let mismatched = ProviderResponse {
            status: "ok".to_owned(),
            session_id: Some("different-session".to_owned()),
            phase: Some("repair".to_owned()),
            response: "{}".to_owned(),
            latency_ms: None,
            usage: None,
            provider_request_id: None,
            structured_output: None,
        };
        assert!(validate_runner_response_binding(&request, &mismatched).is_err());
    }
}
