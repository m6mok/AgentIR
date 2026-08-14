// The checked suffixes below are JSONPath fields, never filesystem extensions.
#![allow(clippy::case_sensitive_file_extension_comparisons)]

use super::{
    AnyResult, FORMAT_VERSION, MAX_PROVIDER_RESPONSE_BYTES, PrivateCorpusTask, SurfaceName,
    ratio_micros,
};
use agentir_authoring::{
    AuthoringError, AuthoringErrorCode, AuthoringGateway, AuthoringPayload, ExecutionMode,
    GraphProposal, compile_authoring_payload, parse_authoring_payload,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StableDiagnostic {
    pub(crate) taxonomy: String,
    sdk_code: Option<String>,
    pub(crate) path: String,
    expected: Value,
    actual: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct GradeRecord {
    format: String,
    format_version: u32,
    phase: String,
    surface: SurfaceName,
    raw_response_bytes: usize,
    pub(crate) authored_json_bytes: Option<usize>,
    pub(crate) authored_operation_count: Option<usize>,
    pub(crate) expanded_graph_operation_count: Option<usize>,
    pub(crate) staged_compression_ratio_micros: Option<u64>,
    framing_success: bool,
    json_success: bool,
    pub(crate) strict_schema_success: bool,
    pub(crate) local_compile_success: bool,
    task_reference_success: bool,
    pub(crate) exact_intent_success: bool,
    pub(crate) publication_success: bool,
    pub(crate) portable_execution_success: bool,
    pub(crate) native_execution_success: Option<bool>,
    deterministic_replay_success: bool,
    pub(crate) model_failure: bool,
    pub(crate) harness_error: bool,
    pub(crate) final_success: bool,
    pub(crate) diagnostic: Option<StableDiagnostic>,
    spec_hash: Option<String>,
    cpu_artifact_hash: Option<String>,
    outputs: Option<Value>,
}

impl GradeRecord {
    fn empty(phase: &str, surface: SurfaceName, bytes: usize) -> Self {
        Self {
            format: "agentir.authoring_eval.grade".to_owned(),
            format_version: FORMAT_VERSION,
            phase: phase.to_owned(),
            surface,
            raw_response_bytes: bytes,
            authored_json_bytes: None,
            authored_operation_count: None,
            expanded_graph_operation_count: None,
            staged_compression_ratio_micros: None,
            framing_success: false,
            json_success: false,
            strict_schema_success: false,
            local_compile_success: false,
            task_reference_success: false,
            exact_intent_success: false,
            publication_success: false,
            portable_execution_success: false,
            native_execution_success: None,
            deterministic_replay_success: false,
            model_failure: true,
            harness_error: false,
            final_success: false,
            diagnostic: None,
            spec_hash: None,
            cpu_artifact_hash: None,
            outputs: None,
        }
    }
}

pub(crate) fn grade_response(
    raw: &str,
    phase: &str,
    surface: SurfaceName,
    task: &PrivateCorpusTask,
    native: bool,
) -> GradeRecord {
    match grade_response_inner(raw, phase, surface, task, native) {
        Ok(grade) => grade,
        Err(error) => {
            let mut grade = GradeRecord::empty(phase, surface, raw.len());
            grade.model_failure = false;
            grade.harness_error = true;
            grade.diagnostic = Some(simple_diagnostic(
                "HARNESS_ERROR",
                "$",
                json!("successful deterministic grading"),
                json!(error.to_string()),
            ));
            grade
        }
    }
}

fn grade_response_inner(
    raw: &str,
    phase: &str,
    surface: SurfaceName,
    task: &PrivateCorpusTask,
    native: bool,
) -> AnyResult<GradeRecord> {
    let mut grade = GradeRecord::empty(phase, surface, raw.len());
    if raw.len() > MAX_PROVIDER_RESPONSE_BYTES {
        grade.diagnostic = Some(simple_diagnostic(
            "RESPONSE_LIMIT",
            "$",
            json!({"maximum_bytes":MAX_PROVIDER_RESPONSE_BYTES}),
            json!({"actual_bytes":raw.len()}),
        ));
        return Ok(grade);
    }
    if raw.trim().is_empty() {
        grade.diagnostic = Some(simple_diagnostic(
            "EMPTY_RESPONSE",
            "$",
            json!("one JSON object"),
            Value::Null,
        ));
        return Ok(grade);
    }
    let mut stream = serde_json::Deserializer::from_str(raw).into_iter::<Value>();
    let value = match stream.next() {
        Some(Ok(value)) => value,
        Some(Err(error)) => {
            grade.diagnostic = Some(simple_diagnostic(
                "NON_JSON",
                "$",
                json!("one JSON object"),
                json!(error.to_string()),
            ));
            return Ok(grade);
        }
        None => return Err("non-empty input produced no JSON stream item".into()),
    };
    let consumed = stream.byte_offset();
    if !raw[consumed..].trim().is_empty() {
        grade.json_success = true;
        grade.diagnostic = Some(simple_diagnostic(
            "EXTRA_TEXT",
            "$",
            json!("no text outside one JSON object"),
            json!(bounded_text(&raw[consumed..], 512)),
        ));
        return Ok(grade);
    }
    grade.framing_success = value.is_object();
    grade.json_success = true;
    grade.authored_json_bytes = Some(serde_json::to_vec(&value)?.len());
    if !value.is_object() {
        grade.diagnostic = Some(simple_diagnostic(
            "WRONG_SCHEMA",
            "$",
            json!("JSON object"),
            value,
        ));
        return Ok(grade);
    }
    let payload = match parse_authoring_payload(raw, Some(surface.sdk())) {
        Ok(payload) => payload,
        Err(error) => {
            grade.diagnostic = Some(classify_sdk_error(&error, raw, surface, None));
            return Ok(grade);
        }
    };
    grade.strict_schema_success = true;
    grade.authored_operation_count = Some(authored_operation_count(&payload));
    let proposal = match compile_authoring_payload(&task.server_task, &payload) {
        Ok(proposal) => proposal,
        Err(error) => {
            grade.diagnostic = Some(classify_sdk_error(&error, raw, surface, Some(&payload)));
            return Ok(grade);
        }
    };
    grade.local_compile_success = true;
    grade.task_reference_success = true;
    grade.expanded_graph_operation_count = Some(proposal.operations.len());
    if surface == SurfaceName::Staged {
        grade.staged_compression_ratio_micros = Some(ratio_micros(
            authored_operation_count(&payload),
            proposal.operations.len(),
        ));
    }
    if proposal != task.server_task.intent {
        grade.diagnostic = Some(intent_diagnostic(&task.server_task.intent, &proposal));
        return Ok(grade);
    }
    grade.exact_intent_success = true;
    let reparsed = parse_authoring_payload(raw, Some(surface.sdk()))?;
    let relowered = compile_authoring_payload(&task.server_task, &reparsed)?;
    if proposal != relowered || serde_json::to_vec(&proposal)? != serde_json::to_vec(&relowered)? {
        return Err("repeated parse, lowering, or serialization diverged".into());
    }
    grade.deterministic_replay_success = true;
    let mode = if native {
        ExecutionMode::Native
    } else {
        ExecutionMode::Portable
    };
    match AuthoringGateway::new().publish_payload(&task.server_task, &payload, mode) {
        Ok(result) => {
            grade.publication_success = true;
            grade.portable_execution_success = true;
            grade.native_execution_success = native.then_some(result.native_checked);
            grade.spec_hash = Some(result.spec_hash);
            grade.cpu_artifact_hash = Some(result.cpu_artifact_hash);
            grade.outputs = Some(result.outputs);
            grade.model_failure = false;
            grade.final_success = true;
        }
        Err(error) => {
            grade.model_failure = matches!(
                error.code,
                AuthoringErrorCode::SchemaRejected
                    | AuthoringErrorCode::ValidationRejected
                    | AuthoringErrorCode::IntentRejected
            );
            grade.harness_error = !grade.model_failure;
            grade.diagnostic = Some(classify_sdk_error(&error, raw, surface, Some(&payload)));
        }
    }
    Ok(grade)
}

fn authored_operation_count(payload: &AuthoringPayload) -> usize {
    match payload {
        AuthoringPayload::Graph(graph) => graph.operations.len(),
        AuthoringPayload::IncrementalBatch(batch) => batch
            .transactions
            .iter()
            .map(|transaction| transaction.operations.len())
            .sum(),
        AuthoringPayload::Staged(staged) => staged.body.len(),
    }
}

fn classify_sdk_error(
    error: &AuthoringError,
    raw: &str,
    surface: SurfaceName,
    payload: Option<&AuthoringPayload>,
) -> StableDiagnostic {
    let taxonomy = match error.code {
        AuthoringErrorCode::SchemaRejected => classify_schema_error(error, surface),
        AuthoringErrorCode::ValidationRejected => {
            classify_validation_error(error, raw, surface, payload)
        }
        AuthoringErrorCode::IntentRejected => {
            if error.path == "$.operations" {
                "INTENT_LENGTH_MISMATCH"
            } else if error.path == "$.yield" {
                "INTENT_YIELD_MISMATCH"
            } else if error.path.ends_with(".op") {
                "INTENT_OPCODE_MISMATCH"
            } else {
                "INTENT_OPERAND_MISMATCH"
            }
        }
        AuthoringErrorCode::CompilerRejected => "COMPILER_REJECTION",
        AuthoringErrorCode::ExecutionMismatch => "EXECUTION_MISMATCH",
    };
    StableDiagnostic {
        taxonomy: taxonomy.to_owned(),
        sdk_code: serde_json::to_value(error.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned)),
        path: error.path.clone(),
        expected: error.expected.clone(),
        actual: error.actual.clone(),
    }
}

fn classify_schema_error(error: &AuthoringError, surface: SurfaceName) -> &'static str {
    if error.expected == json!("no unknown field") {
        "UNKNOWN_FIELD"
    } else if error.path.ends_with(".op") {
        "UNSUPPORTED_OPCODE"
    } else if error.path.ends_with(".yield") && error.actual.is_null() {
        "MISSING_YIELD"
    } else if error.path == "$.state" {
        "UNKNOWN_STATE"
    } else if error.path.ends_with(".schema") || error.path == "$" {
        "WRONG_SCHEMA"
    } else if error.path.ends_with(".operations") && error.actual.get("length") == Some(&json!(0)) {
        "EMPTY_PROGRAM"
    } else if error.path == "$.stages" && error.actual == json!(0) {
        "ZERO_STAGE"
    } else if error.path.ends_with(".count") && error.actual == json!(0) {
        "ZERO_CYCLE"
    } else if error.path.ends_with(".body") && surface == SurfaceName::Staged {
        "BODY_LIMIT"
    } else if error.path.ends_with(".operations") || error.path.ends_with(".transactions") {
        "OPERATION_LIMIT"
    } else {
        "WRONG_FIELD_TYPE"
    }
}

fn classify_validation_error(
    error: &AuthoringError,
    raw: &str,
    surface: SurfaceName,
    payload: Option<&AuthoringPayload>,
) -> &'static str {
    if error.path.ends_with(".operands") && error.expected.get("length").is_some() {
        return "INVALID_ARITY";
    }
    if error.path.contains(".base_operations") {
        let expected = error.expected.as_u64().unwrap_or(0);
        let actual = error.actual.as_u64().unwrap_or(0);
        if actual < expected {
            return if raw
                .matches(&format!("\"base_operations\":{actual}"))
                .count()
                > 1
            {
                "DUPLICATE_BASE"
            } else {
                "STALE_BASE"
            };
        }
        return if surface == SurfaceName::IncrementalBatch
            && payload.is_some_and(|payload| has_reordered_transaction(payload, expected))
        {
            "REORDERED_TRANSACTION"
        } else {
            "BASE_GAP"
        };
    }
    if error.path.ends_with(".bind") {
        return if error
            .actual
            .as_str()
            .is_some_and(|binding| raw.matches(binding).count() > 1)
        {
            "DUPLICATE_BINDING"
        } else {
            "INVALID_BINDING"
        };
    }
    if error.path == "$.yield" {
        return if error.actual.is_null() {
            "MISSING_YIELD"
        } else {
            "UNKNOWN_YIELD"
        };
    }
    if error.path == "$.state" {
        return "UNKNOWN_STATE";
    }
    if error.path == "$.body" {
        return "BODY_LIMIT";
    }
    if error.path.ends_with(".stages") && error.actual == json!(0) {
        return "ZERO_LAG";
    }
    if error.path.contains(".initial[") {
        return "LOCAL_IN_WARMUP";
    }
    if error.path.contains(".body")
        && error.path.contains(".operands")
        && error.expected.get("minimum_length").is_some()
    {
        return "SHORT_WARMUP";
    }
    if error.path.ends_with(".count") {
        return "ZERO_CYCLE";
    }
    if error.path.contains(".body") && error.expected == json!("earlier body binding") {
        return "INVALID_STAGE_LOCAL";
    }
    if error.expected.get("declared_scalar").is_some()
        || error.expected.get("declared_tensor").is_some()
    {
        return "INVALID_CAPTURE";
    }
    if error.expected.get("known_prior_binding").is_some() {
        return if payload.is_some_and(|payload| binding_appears_later(payload, &error.actual)) {
            "FORWARD_REFERENCE"
        } else {
            "UNKNOWN_BINDING"
        };
    }
    if error.expected.get("local_operation_less_than").is_some() {
        return "INVALID_LOCAL_REFERENCE";
    }
    if error.path.ends_with(".operations") {
        return "OPERATION_LIMIT";
    }
    "INVALID_LOCAL_REFERENCE"
}

fn has_reordered_transaction(payload: &AuthoringPayload, expected_at_failure: u64) -> bool {
    let AuthoringPayload::IncrementalBatch(batch) = payload else {
        return false;
    };
    let mut expected = 0_usize;
    for (index, transaction) in batch.transactions.iter().enumerate() {
        if transaction.base_operations != expected {
            return batch.transactions[index + 1..]
                .iter()
                .any(|later| later.base_operations as u64 == expected_at_failure);
        }
        expected += transaction.operations.len();
    }
    false
}

fn binding_appears_later(payload: &AuthoringPayload, actual: &Value) -> bool {
    let Some(binding) = actual.as_str() else {
        return false;
    };
    let AuthoringPayload::IncrementalBatch(batch) = payload else {
        return false;
    };
    batch
        .transactions
        .iter()
        .flat_map(|transaction| &transaction.operations)
        .any(|operation| operation.bind == binding)
}

fn intent_diagnostic(expected: &GraphProposal, actual: &GraphProposal) -> StableDiagnostic {
    if expected.operations.len() != actual.operations.len() {
        return simple_diagnostic(
            "INTENT_LENGTH_MISMATCH",
            "$.operations",
            json!({"length":expected.operations.len()}),
            json!({"length":actual.operations.len()}),
        );
    }
    if expected.r#yield != actual.r#yield {
        return simple_diagnostic(
            "INTENT_YIELD_MISMATCH",
            "$.yield",
            json!(expected.r#yield),
            json!(actual.r#yield),
        );
    }
    for (operation_index, (wanted, received)) in expected
        .operations
        .iter()
        .zip(&actual.operations)
        .enumerate()
    {
        if wanted.op != received.op {
            return simple_diagnostic(
                "INTENT_OPCODE_MISMATCH",
                &format!("$.operations[{operation_index}].op"),
                json!(wanted.op),
                json!(received.op),
            );
        }
        for (operand_index, (wanted, received)) in
            wanted.operands.iter().zip(&received.operands).enumerate()
        {
            if wanted != received {
                return simple_diagnostic(
                    "INTENT_OPERAND_MISMATCH",
                    &format!("$.operations[{operation_index}].operands[{operand_index}]"),
                    json!(wanted),
                    json!(received),
                );
            }
        }
    }
    simple_diagnostic(
        "HARNESS_ERROR",
        "$",
        json!("a local graph mismatch"),
        json!("graphs differ without a classifiable mismatch"),
    )
}

fn simple_diagnostic(
    taxonomy: &str,
    path: &str,
    expected: Value,
    actual: Value,
) -> StableDiagnostic {
    StableDiagnostic {
        taxonomy: taxonomy.to_owned(),
        sdk_code: None,
        path: path.to_owned(),
        expected,
        actual,
    }
}

fn bounded_text(text: &str, maximum: usize) -> String {
    text.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval_harness::corpus::build_corpus;
    use agentir_authoring::{GraphOperand, StagedOperand};

    #[test]
    fn grader_preserves_framing_and_local_intent_diagnostics() {
        let task = build_corpus().unwrap().remove(0);
        let empty = grade_response("", "initial", SurfaceName::Graph, &task, false);
        assert_eq!(empty.diagnostic.unwrap().taxonomy, "EMPTY_RESPONSE");
        let extra = format!(
            "{} trailing",
            serde_json::to_string(&task.graph_payload).unwrap()
        );
        let grade = grade_response(&extra, "initial", SurfaceName::Graph, &task, false);
        assert_eq!(grade.diagnostic.unwrap().taxonomy, "EXTRA_TEXT");
        let mut wrong = task.graph_payload.clone();
        wrong.operations[0].operands.swap(0, 1);
        let grade = grade_response(
            &serde_json::to_string(&wrong).unwrap(),
            "initial",
            SurfaceName::Graph,
            &task,
            false,
        );
        let diagnostic = grade.diagnostic.unwrap();
        assert_eq!(diagnostic.taxonomy, "INTENT_OPERAND_MISMATCH");
        assert_eq!(diagnostic.path, "$.operations[0].operands[0]");
        assert_ne!(
            diagnostic.expected,
            serde_json::to_value(&task.graph_payload).unwrap()
        );
    }

    #[test]
    fn exact_oracle_payload_passes_full_portable_pipeline() {
        let task = build_corpus().unwrap().remove(0);
        let grade = grade_response(
            &serde_json::to_string(&task.graph_payload).unwrap(),
            "initial",
            SurfaceName::Graph,
            &task,
            false,
        );
        assert!(grade.strict_schema_success);
        assert!(grade.exact_intent_success);
        assert!(grade.publication_success);
        assert!(grade.portable_execution_success);
        assert!(grade.final_success);
    }

    #[test]
    fn intent_diagnostic_never_contains_complete_oracle() {
        let task = build_corpus().unwrap().remove(2);
        let mut wrong = task.graph_payload.clone();
        wrong.operations[3].operands[0] = GraphOperand::Local { operation: 0 };
        let diagnostic = intent_diagnostic(&task.graph_payload, &wrong);
        assert!(diagnostic.path.starts_with("$.operations["));
        assert_ne!(
            diagnostic.expected,
            serde_json::to_value(task.graph_payload).unwrap()
        );
    }

    #[test]
    fn stable_taxonomy_covers_graph_incremental_and_staged_failures() {
        let tasks = build_corpus().unwrap();
        let graph_task = &tasks[0];
        let mut unknown = serde_json::to_value(&graph_task.graph_payload).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("compiler_id".to_owned(), json!("forbidden"));
        assert_taxonomy(
            &unknown.to_string(),
            SurfaceName::Graph,
            graph_task,
            "UNKNOWN_FIELD",
        );

        let mut unsupported = serde_json::to_value(&graph_task.graph_payload).unwrap();
        unsupported["operations"][0]["op"] = json!("div");
        assert_taxonomy(
            &unsupported.to_string(),
            SurfaceName::Graph,
            graph_task,
            "UNSUPPORTED_OPCODE",
        );

        let mut invalid_arity = serde_json::to_value(&graph_task.graph_payload).unwrap();
        invalid_arity["operations"][0]["operands"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_taxonomy(
            &invalid_arity.to_string(),
            SurfaceName::Graph,
            graph_task,
            "INVALID_ARITY",
        );

        let mut incremental = graph_task.incremental_payload.clone();
        incremental.transactions[1].base_operations += 2;
        assert_taxonomy(
            &serde_json::to_string(&incremental).unwrap(),
            SurfaceName::IncrementalBatch,
            graph_task,
            "BASE_GAP",
        );

        let staged_task = tasks
            .iter()
            .find(|task| !task.public.difficulty.recurrence_lags.is_empty())
            .unwrap();
        let mut zero_lag = staged_task.staged_payload.clone();
        let lag = zero_lag
            .body
            .iter_mut()
            .flat_map(|operation| &mut operation.operands)
            .find_map(|operand| match operand {
                StagedOperand::StateLag { stages, .. } => Some(stages),
                _ => None,
            })
            .unwrap();
        *lag = 0;
        assert_taxonomy(
            &serde_json::to_string(&zero_lag).unwrap(),
            SurfaceName::Staged,
            staged_task,
            "ZERO_LAG",
        );

        let mut unknown_state = staged_task.staged_payload.clone();
        unknown_state.state = "$missing".to_owned();
        assert_taxonomy(
            &serde_json::to_string(&unknown_state).unwrap(),
            SurfaceName::Staged,
            staged_task,
            "UNKNOWN_STATE",
        );
    }

    fn assert_taxonomy(raw: &str, surface: SurfaceName, task: &PrivateCorpusTask, expected: &str) {
        let grade = grade_response(raw, "initial", surface, task, false);
        assert_eq!(
            grade.diagnostic.unwrap().taxonomy,
            expected,
            "taxonomy for {surface:?}"
        );
    }
}
