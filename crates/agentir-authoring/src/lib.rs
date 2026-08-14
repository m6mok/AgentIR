//! Strict graph, incremental, and staged authoring surfaces over the production
//! AgentIR verifier.
//!
//! Every model-authored surface deterministically lowers to one ordinary
//! [`GraphProposal`]. The server-owned task envelope supplies parameter order,
//! types, inputs, and exact intent. No compiler identifier or hash is accepted
//! from the caller, and no workspace is opened until lowering and exact intent
//! comparison have succeeded.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    actions::{Action, RegionArgumentSpec, RegionOpSpec, RegionSpec},
    types::{DimExpr, ScalarType, Shape, Type},
};
use agentir_protocol::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod framed;
mod incremental;
mod staged;

pub use framed::{
    AUTHORING_FRAME_JSON_SCHEMA, AUTHORING_FRAME_SCHEMA, AuthoringFrame, AuthoringFrameBlueprint,
    FRAMED_STAGED_JSON_SCHEMA, FRAMED_STAGED_MODEL_INSTRUCTION, FRAMED_STAGED_SCHEMA,
    FrameOpcodeMenu, FrameRole, FrameSlot, FramedOperationChoice, FramedStagedProposal,
    PublicAuthoringDeclarations, PublicProblemShape, RecommendationError, SurfaceRecommendation,
    build_authoring_frame, compile_framed_staged, parse_framed_staged, recommend_surface,
    verify_authoring_frame,
};

pub use incremental::{
    INCREMENTAL_BATCH_JSON_SCHEMA, INCREMENTAL_BATCH_MODEL_INSTRUCTION, INCREMENTAL_BATCH_SCHEMA,
    IncrementalBatch, IncrementalOperand, IncrementalOperation, IncrementalReceipt,
    IncrementalSession, IncrementalTransaction, TRANSACTION_JSON_SCHEMA, TRANSACTION_SCHEMA,
    compile_incremental_batch, parse_incremental_batch, parse_transaction,
};
pub use staged::{
    STAGED_JSON_SCHEMA, STAGED_MODEL_INSTRUCTION, STAGED_SCHEMA, StagedOperand, StagedOperation,
    StagedProposal, compile_staged, parse_staged,
};

/// Exact schema identifier accepted for model-authored graphs.
pub const GRAPH_SCHEMA: &str = "agentir.elementwise_graph.v1";

/// Exact JSON Schema document for model-authored graphs.
pub const GRAPH_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-graph-v1.schema.json");

/// Exact schema identifier accepted for server-owned task envelopes.
pub const TASK_SCHEMA: &str = "agentir.elementwise_authoring_task.v1";

/// Self-contained default instruction for a model producing one graph proposal.
///
/// Integrations should provide this text together with exactly one authorized
/// public task. The explicit field shapes are intentional: model trials showed
/// that names alone invite plausible but invalid `opcode`, shorthand operand,
/// and object-valued `yield` variants.
pub const DEFAULT_MODEL_INSTRUCTION: &str = r#"Return exactly one JSON object with this wire shape:
{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"a"},{"kind":"tensor","name":"x"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"tensor","name":"y"}]}],"yield":1}
Use the top-level keys schema, operations, and yield exactly; yield is a zero-based integer operation index, never an object. Every operation uses the key op, never opcode. The only supported op values are add, mul, and fma. Every operand has a kind: scalar/tensor operands also have name, while local operands also have the zero-based prior integer operation index in operation. Do not use shorthand operand objects. Preserve every required operation, dependency, operand, and their exact order. Use exactly two operands for add/mul and exactly three for fma. Never replace fma with mul plus add. Return only the JSON graph object; the server owns capability preflight, types, inputs, compiler IDs, hashes, intent checking, and publication."#;

/// Graph-surface model instruction, named consistently with the other surfaces.
pub const GRAPH_MODEL_INSTRUCTION: &str = DEFAULT_MODEL_INSTRUCTION;

const MAX_OPERATIONS: usize = 128;

/// One supported model-authored payload family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthoringSurface {
    /// One graph with explicit zero-based local operation indices.
    Graph,
    /// One complete sequence of atomic symbolic transactions.
    IncrementalBatch,
    /// One bounded structural body expanded over a fixed number of stages.
    Staged,
}

impl AuthoringSurface {
    /// Returns the exact top-level schema identifier for this surface.
    #[must_use]
    pub const fn schema(self) -> &'static str {
        match self {
            Self::Graph => GRAPH_SCHEMA,
            Self::IncrementalBatch => INCREMENTAL_BATCH_SCHEMA,
            Self::Staged => STAGED_SCHEMA,
        }
    }

    /// Returns the exact machine-readable JSON Schema document.
    #[must_use]
    pub const fn json_schema(self) -> &'static str {
        match self {
            Self::Graph => GRAPH_JSON_SCHEMA,
            Self::IncrementalBatch => INCREMENTAL_BATCH_JSON_SCHEMA,
            Self::Staged => STAGED_JSON_SCHEMA,
        }
    }

    /// Returns the short model instruction, including one literal wire example.
    #[must_use]
    pub const fn model_instruction(self) -> &'static str {
        match self {
            Self::Graph => GRAPH_MODEL_INSTRUCTION,
            Self::IncrementalBatch => INCREMENTAL_BATCH_MODEL_INSTRUCTION,
            Self::Staged => STAGED_MODEL_INSTRUCTION,
        }
    }
}

/// One parsed model payload without an additional test or transport envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoringPayload {
    /// An explicit graph proposal.
    Graph(GraphProposal),
    /// A complete sequence of symbolic incremental transactions.
    IncrementalBatch(IncrementalBatch),
    /// A bounded staged structural proposal.
    Staged(StagedProposal),
}

impl AuthoringPayload {
    /// Returns this payload's authoring family.
    #[must_use]
    pub const fn surface(&self) -> AuthoringSurface {
        match self {
            Self::Graph(_) => AuthoringSurface::Graph,
            Self::IncrementalBatch(_) => AuthoringSurface::IncrementalBatch,
            Self::Staged(_) => AuthoringSurface::Staged,
        }
    }
}

/// Opcode subset supported by the elementwise authoring SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphOpcode {
    /// Exact binary addition.
    Add,
    /// Exact binary multiplication.
    Mul,
    /// Exact fused multiply-add; never interchangeable with multiplication plus addition.
    Fma,
}

impl GraphOpcode {
    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Add | Self::Mul => 2,
            Self::Fma => 3,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Mul => "mul",
            Self::Fma => "fma",
        }
    }
}

/// One typed operand reference in a graph-only proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphOperand {
    /// Server-declared scalar capture by name.
    Scalar {
        /// Exact scalar name from the task envelope.
        name: String,
    },
    /// Server-declared tensor element by name.
    Tensor {
        /// Exact tensor name from the task envelope.
        name: String,
    },
    /// Result of an earlier operation by zero-based index.
    Local {
        /// Zero-based operation index; must be less than the current operation.
        operation: usize,
    },
}

/// One ordered operation in a graph-only proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphOperation {
    /// Exact opcode.
    pub op: GraphOpcode,
    /// Ordered operands.
    pub operands: Vec<GraphOperand>,
}

/// Complete model-authored graph. It cannot represent IDs, hashes, types, or inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphProposal {
    /// Must equal GRAPH_SCHEMA.
    pub schema: String,
    /// Topologically ordered operations.
    pub operations: Vec<GraphOperation>,
    /// Zero-based operation index yielded as the program output.
    pub r#yield: usize,
}

/// Server-owned authoring task and exact intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoringTask {
    /// Must equal TASK_SCHEMA.
    pub schema: String,
    /// Stable server-owned task identity.
    pub task_id: String,
    /// Symbolic one-dimensional extent.
    pub dimension: String,
    /// Ordered f32 scalar captures.
    pub scalars: Vec<String>,
    /// Ordered one-dimensional f32 tensors.
    pub tensors: Vec<String>,
    /// Server-owned runtime inputs used for independent execution checks.
    pub inputs: BTreeMap<String, Value>,
    /// Exact ordered graph that must match before any workspace is created.
    pub intent: GraphProposal,
}

/// Stable local authoring diagnostic class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthoringErrorCode {
    /// JSON or strict schema decoding failed.
    SchemaRejected,
    /// A typed field or reference is invalid for the server-owned task.
    ValidationRejected,
    /// A graph differs from exact server-owned intent.
    IntentRejected,
    /// The production AgentIR verifier or lowering pipeline rejected a request.
    CompilerRejected,
    /// Reference, portable, and native outputs did not agree.
    ExecutionMismatch,
}

/// One path-specific authoring failure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuthoringError {
    /// Stable diagnostic class.
    pub code: AuthoringErrorCode,
    /// JSONPath-like location of the failure.
    pub path: String,
    /// Expected contract at path.
    pub expected: Value,
    /// Actual value or diagnostic.
    pub actual: Value,
    /// Complete model-facing contract supplied for one-shot schema repair.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair_hint: Option<String>,
}

impl fmt::Display for AuthoringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} at {}: expected {}, actual {}",
            self.code, self.path, self.expected, self.actual
        )
    }
}

impl std::error::Error for AuthoringError {}

/// One server-side production protocol exchange retained for audit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuthoringTranscriptEntry {
    /// Production request with compiler identities filled from prior responses.
    pub request: Value,
    /// Complete production response envelope.
    pub response: Value,
}

/// Successful publication and execution evidence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuthoringResult {
    /// Server-owned task identity.
    pub task_id: String,
    /// Compiler-assigned workspace identity.
    pub workspace: String,
    /// Compiler-assigned frozen SpecIR revision.
    pub revision: String,
    /// Frozen revision content hash.
    pub content_hash: String,
    /// Frozen semantic specification hash.
    pub spec_hash: String,
    /// Compiler-assigned portable CPU artifact identity.
    pub cpu_artifact: String,
    /// Compiler-owned portable CPU artifact hash.
    pub cpu_artifact_hash: String,
    /// Reference/portable/native-agreed named outputs.
    pub outputs: Value,
    /// Whether native execution was included.
    pub native_checked: bool,
    /// Model-visible authoring calls represented by this result.
    pub model_visible_calls: u64,
    /// Production protocol requests executed in one persistent server session.
    pub internal_agentir_requests: u64,
    /// Full server-owned request/response audit trail.
    pub transcript: Vec<AuthoringTranscriptEntry>,
}

/// Whether to include isolated native execution after portable validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Check reference and portable CPU execution.
    Portable,
    /// Also launch the isolated native CPU worker and require exact output equality.
    Native,
}

/// Stateful local authoring gateway. One instance retains one production engine session.
#[derive(Debug, Default)]
pub struct AuthoringGateway {
    engine: Engine,
}

impl AuthoringGateway {
    /// Creates an empty gateway.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates exact intent, publishes one frozen SpecIR, and checks CPU execution.
    ///
    /// Intent and schema failures occur before workspace.open, so they consume no
    /// compiler IDs or workspace state.
    pub fn publish(
        &mut self,
        task: &AuthoringTask,
        proposal: &GraphProposal,
        mode: ExecutionMode,
    ) -> Result<AuthoringResult, AuthoringError> {
        validate_task(task)?;
        validate_graph(proposal, task, "$")?;
        if proposal != &task.intent {
            return Err(intent_mismatch(&task.intent, proposal));
        }

        self.publish_validated(task, proposal, mode)
    }

    /// Lowers any supported authoring payload, checks exact intent, and uses the
    /// same production publication and execution pipeline as [`Self::publish`].
    ///
    /// Parsing is intentionally separate so callers can retain the original
    /// structured diagnostic. Lowering, task-relative graph validation, and
    /// exact hidden-intent comparison all finish before `workspace.open`.
    pub fn publish_payload(
        &mut self,
        task: &AuthoringTask,
        payload: &AuthoringPayload,
        mode: ExecutionMode,
    ) -> Result<AuthoringResult, AuthoringError> {
        let proposal = compile_authoring_payload(task, payload)?;
        if proposal != task.intent {
            return Err(intent_mismatch(&task.intent, &proposal));
        }

        self.publish_validated(task, &proposal, mode)
    }

    fn publish_validated(
        &mut self,
        task: &AuthoringTask,
        proposal: &GraphProposal,
        mode: ExecutionMode,
    ) -> Result<AuthoringResult, AuthoringError> {
        let actions = compile_actions(task, proposal);
        let mut transcript = Vec::new();
        let open = call(
            &mut self.engine,
            json!({"command":"workspace.open","request_id":"authoring.open"}),
            &mut transcript,
        )?;
        let workspace = string_at(
            &open,
            "workspace",
            "$.agentir.authoring.open.result.workspace",
        )?;
        let base_revision = string_at(
            &open,
            "revision",
            "$.agentir.authoring.open.result.revision",
        )?;
        let build = call(
            &mut self.engine,
            json!({
                "command":"spec.apply",
                "request_id":"authoring.publish",
                "workspace":workspace,
                "base_revision":base_revision,
                "actions":actions,
            }),
            &mut transcript,
        )?;
        let revision = string_at(
            &build,
            "revision",
            "$.agentir.authoring.publish.result.revision",
        )?;
        let content_hash = string_at(
            &build,
            "content_hash",
            "$.agentir.authoring.publish.result.content_hash",
        )?;
        let spec_hash = string_at(
            &build,
            "spec_hash",
            "$.agentir.authoring.publish.result.spec_hash",
        )?;

        let reference = call(
            &mut self.engine,
            json!({
                "command":"program.evaluate",
                "request_id":"authoring.reference",
                "workspace":workspace,
                "revision":revision,
                "inputs":task.inputs,
            }),
            &mut transcript,
        )?;
        let candidate = call(
            &mut self.engine,
            json!({
                "command":"candidate.create",
                "request_id":"authoring.candidate",
                "workspace":workspace,
                "spec_revision":revision,
            }),
            &mut transcript,
        )?;
        let candidate_id = string_at(
            &candidate,
            "candidate",
            "$.agentir.authoring.candidate.result.candidate",
        )?;
        let candidate_revision = string_at(
            &candidate,
            "candidate_revision",
            "$.agentir.authoring.candidate.result.candidate_revision",
        )?;
        let memory = call(
            &mut self.engine,
            json!({
                "command":"memory.create",
                "request_id":"authoring.memory",
                "workspace":workspace,
                "candidate":candidate_id,
                "candidate_revision":candidate_revision,
            }),
            &mut transcript,
        )?;
        let memory_query = object_at(&memory, "query", "$.agentir.authoring.memory.result.query")?;
        let memory_plan = string_at(
            memory_query,
            "memory_plan",
            "$.agentir.authoring.memory.result.query.memory_plan",
        )?;
        let memory_revision = string_at(
            memory_query,
            "memory_revision",
            "$.agentir.authoring.memory.result.query.memory_revision",
        )?;
        let target = call(
            &mut self.engine,
            json!({
                "command":"target.create",
                "request_id":"authoring.target",
                "workspace":workspace,
                "profile":"cpu_scalar_v1",
            }),
            &mut transcript,
        )?;
        let target_query = object_at(&target, "query", "$.agentir.authoring.target.result.query")?;
        let target_manifest = string_at(
            target_query,
            "target_manifest",
            "$.agentir.authoring.target.result.query.target_manifest",
        )?;
        let target_revision = string_at(
            target_query,
            "target_revision",
            "$.agentir.authoring.target.result.query.target_revision",
        )?;
        let schedule = call(
            &mut self.engine,
            json!({
                "command":"schedule.create",
                "request_id":"authoring.schedule",
                "workspace":workspace,
                "memory_plan":memory_plan,
                "memory_revision":memory_revision,
                "target_manifest":target_manifest,
                "target_revision":target_revision,
            }),
            &mut transcript,
        )?;
        let schedule_query = object_at(
            &schedule,
            "query",
            "$.agentir.authoring.schedule.result.query",
        )?;
        let schedule_plan = string_at(
            schedule_query,
            "schedule_plan",
            "$.agentir.authoring.schedule.result.query.schedule_plan",
        )?;
        let schedule_revision = string_at(
            schedule_query,
            "schedule_revision",
            "$.agentir.authoring.schedule.result.query.schedule_revision",
        )?;
        let schedule_hash = string_at(
            schedule_query,
            "schedule_hash",
            "$.agentir.authoring.schedule.result.query.schedule_hash",
        )?;
        let emitted = call(
            &mut self.engine,
            json!({
                "command":"cpu_artifact.emit",
                "request_id":"authoring.emit",
                "workspace":workspace,
                "schedule_plan":schedule_plan,
                "schedule_revision":schedule_revision,
                "expected_schedule_hash":schedule_hash,
            }),
            &mut transcript,
        )?;
        let artifact_query = object_at(&emitted, "query", "$.agentir.authoring.emit.result.query")?;
        let cpu_artifact = string_at(
            artifact_query,
            "cpu_artifact",
            "$.agentir.authoring.emit.result.query.cpu_artifact",
        )?;
        let cpu_artifact_hash = string_at(
            artifact_query,
            "cpu_artifact_hash",
            "$.agentir.authoring.emit.result.query.cpu_artifact_hash",
        )?;
        call(
            &mut self.engine,
            json!({
                "command":"cpu_artifact.check",
                "request_id":"authoring.check",
                "workspace":workspace,
                "cpu_artifact":cpu_artifact,
                "expected_cpu_artifact_hash":cpu_artifact_hash,
            }),
            &mut transcript,
        )?;
        let portable = call(
            &mut self.engine,
            json!({
                "command":"cpu_artifact.execute",
                "request_id":"authoring.portable",
                "workspace":workspace,
                "cpu_artifact":cpu_artifact,
                "expected_cpu_artifact_hash":cpu_artifact_hash,
                "inputs":task.inputs,
            }),
            &mut transcript,
        )?;
        let reference_outputs = value_at(
            &reference,
            "outputs",
            "$.agentir.authoring.reference.result.outputs",
        )?;
        let portable_outputs = value_at(
            &portable,
            "outputs",
            "$.agentir.authoring.portable.result.outputs",
        )?;
        if reference_outputs != portable_outputs {
            return Err(failure(
                AuthoringErrorCode::ExecutionMismatch,
                "$.outputs.portable",
                reference_outputs.clone(),
                portable_outputs.clone(),
            ));
        }
        if mode == ExecutionMode::Native {
            let native = call(
                &mut self.engine,
                json!({
                    "command":"cpu_native.execute",
                    "request_id":"authoring.native",
                    "workspace":workspace,
                    "cpu_artifact":cpu_artifact,
                    "expected_cpu_artifact_hash":cpu_artifact_hash,
                    "inputs":task.inputs,
                }),
                &mut transcript,
            )?;
            let native_outputs = value_at(
                &native,
                "outputs",
                "$.agentir.authoring.native.result.outputs",
            )?;
            if reference_outputs != native_outputs {
                return Err(failure(
                    AuthoringErrorCode::ExecutionMismatch,
                    "$.outputs.native",
                    reference_outputs.clone(),
                    native_outputs.clone(),
                ));
            }
        }
        let internal_agentir_requests = u64::try_from(transcript.len()).unwrap_or(u64::MAX);
        Ok(AuthoringResult {
            task_id: task.task_id.clone(),
            workspace,
            revision,
            content_hash,
            spec_hash,
            cpu_artifact,
            cpu_artifact_hash,
            outputs: reference_outputs.clone(),
            native_checked: mode == ExecutionMode::Native,
            model_visible_calls: 1,
            internal_agentir_requests,
            transcript,
        })
    }
}

/// Decodes a strict server-owned task envelope.
pub fn parse_task(input: &str) -> Result<AuthoringTask, AuthoringError> {
    serde_json::from_str(input).map_err(|error| {
        failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_authoring_task.v1 object"),
            json!(error.to_string()),
        )
    })
}

/// Decodes a strict graph-only proposal.
pub fn parse_proposal(input: &str) -> Result<GraphProposal, AuthoringError> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        proposal_schema_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_graph.v1 object"),
            json!(error.to_string()),
        )
    })?;
    validate_proposal_shape(&value).map_err(with_proposal_repair_hint)?;
    serde_json::from_value(value).map_err(|error| {
        proposal_schema_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_graph.v1 object"),
            json!(error.to_string()),
        )
    })
}

/// Strictly parses one bare model payload.
///
/// When `surface` is `None`, dispatch uses only the exact top-level `schema`
/// value. It never attempts one dialect and then falls back to another.
pub fn parse_authoring_payload(
    input: &str,
    surface: Option<AuthoringSurface>,
) -> Result<AuthoringPayload, AuthoringError> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        let mut diagnostic = failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("one strict authoring JSON object"),
            json!(error.to_string()),
        );
        if let Some(surface) = surface {
            diagnostic.repair_hint = Some(surface.model_instruction().to_owned());
        }
        diagnostic
    })?;
    let root = require_object(&value, "$")?;
    let actual_schema = root.get("schema").and_then(Value::as_str).ok_or_else(|| {
        failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(surface.map_or_else(
                || vec![GRAPH_SCHEMA, INCREMENTAL_BATCH_SCHEMA, STAGED_SCHEMA],
                |selected| vec![selected.schema()]
            )),
            root.get("schema").cloned().unwrap_or(Value::Null),
        )
    })?;
    let selected = if let Some(selected) = surface {
        if actual_schema != selected.schema() {
            return Err(with_repair_hint(
                failure(
                    AuthoringErrorCode::SchemaRejected,
                    "$.schema",
                    json!(selected.schema()),
                    json!(actual_schema),
                ),
                selected.model_instruction(),
            ));
        }
        selected
    } else {
        match actual_schema {
            GRAPH_SCHEMA => AuthoringSurface::Graph,
            INCREMENTAL_BATCH_SCHEMA => AuthoringSurface::IncrementalBatch,
            STAGED_SCHEMA => AuthoringSurface::Staged,
            _ => {
                return Err(failure(
                    AuthoringErrorCode::SchemaRejected,
                    "$.schema",
                    json!([GRAPH_SCHEMA, INCREMENTAL_BATCH_SCHEMA, STAGED_SCHEMA]),
                    json!(actual_schema),
                ));
            }
        }
    };

    match selected {
        AuthoringSurface::Graph => parse_proposal(input).map(AuthoringPayload::Graph),
        AuthoringSurface::IncrementalBatch => {
            parse_incremental_batch(input).map(AuthoringPayload::IncrementalBatch)
        }
        AuthoringSurface::Staged => parse_staged(input).map(AuthoringPayload::Staged),
    }
}

/// Deterministically lowers one parsed authoring payload into the ordinary
/// graph contract and validates every task-relative capture and local edge.
pub fn compile_authoring_payload(
    task: &AuthoringTask,
    payload: &AuthoringPayload,
) -> Result<GraphProposal, AuthoringError> {
    validate_task(task)?;
    let proposal = match payload {
        AuthoringPayload::Graph(proposal) => proposal.clone(),
        AuthoringPayload::IncrementalBatch(batch) => compile_incremental_batch(
            batch,
            task.scalars.iter().cloned(),
            task.tensors.iter().cloned(),
        )?,
        AuthoringPayload::Staged(staged) => compile_staged(staged)?,
    };
    validate_graph(&proposal, task, "$")
        .map_err(|error| with_repair_hint(error, payload.surface().model_instruction()))?;
    Ok(proposal)
}

pub(crate) fn failure(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    expected: Value,
    actual: Value,
) -> AuthoringError {
    AuthoringError {
        code,
        path: path.into(),
        expected,
        actual,
        repair_hint: None,
    }
}

pub(crate) fn with_repair_hint(
    mut error: AuthoringError,
    instruction: &'static str,
) -> AuthoringError {
    error.repair_hint = Some(instruction.to_owned());
    error
}

fn proposal_schema_failure(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    expected: Value,
    actual: Value,
) -> AuthoringError {
    with_proposal_repair_hint(failure(code, path, expected, actual))
}

fn with_proposal_repair_hint(mut error: AuthoringError) -> AuthoringError {
    error = with_repair_hint(error, DEFAULT_MODEL_INSTRUCTION);
    error
}

fn intent_mismatch(expected: &GraphProposal, actual: &GraphProposal) -> AuthoringError {
    let (path, expected, actual) = if expected.operations.len() != actual.operations.len() {
        (
            "$.operations".to_owned(),
            json!({"length":expected.operations.len()}),
            json!({"length":actual.operations.len()}),
        )
    } else if expected.r#yield != actual.r#yield {
        (
            "$.yield".to_owned(),
            json!(expected.r#yield),
            json!(actual.r#yield),
        )
    } else {
        expected
            .operations
            .iter()
            .zip(&actual.operations)
            .enumerate()
            .find_map(|(operation_index, (expected, actual))| {
                if expected.op != actual.op {
                    return Some((
                        format!("$.operations[{operation_index}].op"),
                        json!(expected.op),
                        json!(actual.op),
                    ));
                }
                expected
                    .operands
                    .iter()
                    .zip(&actual.operands)
                    .enumerate()
                    .find(|(_, (expected, actual))| expected != actual)
                    .map(|(operand_index, (expected, actual))| {
                        (
                            format!("$.operations[{operation_index}].operands[{operand_index}]"),
                            json!(expected),
                            json!(actual),
                        )
                    })
            })
            .expect("unequal validated graphs have a local mismatch")
    };
    AuthoringError {
        code: AuthoringErrorCode::IntentRejected,
        path,
        expected,
        actual,
        repair_hint: Some(
            "Change only the reported mismatch, then recheck the original task's indexing rules; do not rewrite unrelated operations."
                .to_owned(),
        ),
    }
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

pub(crate) fn valid_binding(binding: &str) -> bool {
    let Some(rest) = binding.strip_prefix('$') else {
        return false;
    };
    valid_name(rest)
}

fn validate_proposal_shape(value: &Value) -> Result<(), AuthoringError> {
    let root = require_object(value, "$")?;
    require_keys(root, "$", &["schema", "operations", "yield"])?;
    if root.get("schema").and_then(Value::as_str) != Some(GRAPH_SCHEMA) {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(GRAPH_SCHEMA),
            root.get("schema").cloned().unwrap_or(Value::Null),
        ));
    }
    let operations = root
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            failure(
                AuthoringErrorCode::SchemaRejected,
                "$.operations",
                json!("array"),
                root.get("operations").cloned().unwrap_or(Value::Null),
            )
        })?;
    if operations.is_empty() || operations.len() > MAX_OPERATIONS {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.operations",
            json!({"min_items":1,"max_items":MAX_OPERATIONS}),
            json!({"length":operations.len()}),
        ));
    }
    for (operation_index, operation) in operations.iter().enumerate() {
        let operation_path = format!("$.operations[{operation_index}]");
        let operation = require_object(operation, &operation_path)?;
        require_keys(operation, &operation_path, &["op", "operands"])?;
        if !matches!(
            operation.get("op").and_then(Value::as_str),
            Some("add" | "mul" | "fma")
        ) {
            return Err(failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{operation_path}.op"),
                json!(["add", "mul", "fma"]),
                operation.get("op").cloned().unwrap_or(Value::Null),
            ));
        }
        let operands = operation
            .get("operands")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                failure(
                    AuthoringErrorCode::SchemaRejected,
                    format!("{operation_path}.operands"),
                    json!("array"),
                    operation.get("operands").cloned().unwrap_or(Value::Null),
                )
            })?;
        for (operand_index, operand) in operands.iter().enumerate() {
            let operand_path = format!("{operation_path}.operands[{operand_index}]");
            let operand = require_object(operand, &operand_path)?;
            let kind = operand.get("kind").and_then(Value::as_str);
            match kind {
                Some("scalar" | "tensor") => {
                    require_keys(operand, &operand_path, &["kind", "name"])?;
                    if !operand.get("name").is_some_and(Value::is_string) {
                        return Err(failure(
                            AuthoringErrorCode::SchemaRejected,
                            format!("{operand_path}.name"),
                            json!("string"),
                            operand.get("name").cloned().unwrap_or(Value::Null),
                        ));
                    }
                }
                Some("local") => {
                    require_keys(operand, &operand_path, &["kind", "operation"])?;
                    if !operand.get("operation").is_some_and(Value::is_u64) {
                        return Err(failure(
                            AuthoringErrorCode::SchemaRejected,
                            format!("{operand_path}.operation"),
                            json!("non-negative integer"),
                            operand.get("operation").cloned().unwrap_or(Value::Null),
                        ));
                    }
                }
                _ => {
                    return Err(failure(
                        AuthoringErrorCode::SchemaRejected,
                        format!("{operand_path}.kind"),
                        json!(["scalar", "tensor", "local"]),
                        operand.get("kind").cloned().unwrap_or(Value::Null),
                    ));
                }
            }
        }
    }
    if !root.get("yield").is_some_and(Value::is_u64) {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.yield",
            json!("non-negative integer"),
            root.get("yield").cloned().unwrap_or(Value::Null),
        ));
    }
    Ok(())
}

pub(crate) fn require_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a serde_json::Map<String, Value>, AuthoringError> {
    value.as_object().ok_or_else(|| {
        failure(
            AuthoringErrorCode::SchemaRejected,
            path,
            json!("object"),
            value.clone(),
        )
    })
}

pub(crate) fn require_keys(
    object: &serde_json::Map<String, Value>,
    path: &str,
    expected: &[&str],
) -> Result<(), AuthoringError> {
    for key in object.keys() {
        if !expected.contains(&key.as_str()) {
            return Err(failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.{key}"),
                json!("no unknown field"),
                object[key].clone(),
            ));
        }
    }
    for key in expected {
        if !object.contains_key(*key) {
            return Err(failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.{key}"),
                json!("required field"),
                Value::Null,
            ));
        }
    }
    Ok(())
}

fn validate_task(task: &AuthoringTask) -> Result<(), AuthoringError> {
    if task.schema != TASK_SCHEMA {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(TASK_SCHEMA),
            json!(task.schema),
        ));
    }
    if task.task_id.is_empty() {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.task_id",
            json!("non-empty server task ID"),
            json!(task.task_id),
        ));
    }
    if !valid_name(&task.dimension) {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.dimension",
            json!("identifier"),
            json!(task.dimension),
        ));
    }
    if task.tensors.is_empty() {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.tensors",
            json!("at least one tensor"),
            json!(task.tensors),
        ));
    }
    let mut names = BTreeSet::new();
    for (kind, values) in [("scalars", &task.scalars), ("tensors", &task.tensors)] {
        for (index, name) in values.iter().enumerate() {
            if !valid_name(name) || !names.insert(name) {
                return Err(failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("$.{kind}[{index}]"),
                    json!("globally unique identifier"),
                    json!(name),
                ));
            }
        }
    }
    let expected_names = task
        .scalars
        .iter()
        .chain(&task.tensors)
        .cloned()
        .collect::<BTreeSet<_>>();
    let actual_names = task.inputs.keys().cloned().collect::<BTreeSet<_>>();
    if actual_names != expected_names {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.inputs",
            json!(expected_names),
            json!(actual_names),
        ));
    }
    for scalar in &task.scalars {
        let value = &task.inputs[scalar];
        let finite = value
            .as_f64()
            .is_some_and(|number| (number as f32).is_finite());
        if !finite {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.inputs.{scalar}"),
                json!("finite f32 number"),
                value.clone(),
            ));
        }
    }
    let mut tensor_length = None;
    for tensor in &task.tensors {
        let value = &task.inputs[tensor];
        let Some(items) = value.as_array() else {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.inputs.{tensor}"),
                json!("array of finite f32 numbers"),
                value.clone(),
            ));
        };
        for (index, item) in items.iter().enumerate() {
            if !item
                .as_f64()
                .is_some_and(|number| (number as f32).is_finite())
            {
                return Err(failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("$.inputs.{tensor}[{index}]"),
                    json!("finite f32 number"),
                    item.clone(),
                ));
            }
        }
        if tensor_length.is_some_and(|length| length != items.len()) {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.inputs.{tensor}"),
                json!({"length":tensor_length}),
                json!({"length":items.len()}),
            ));
        }
        tensor_length = Some(items.len());
    }
    validate_graph(&task.intent, task, "$.intent")
}

fn validate_graph(
    graph: &GraphProposal,
    task: &AuthoringTask,
    root: &str,
) -> Result<(), AuthoringError> {
    if graph.schema != GRAPH_SCHEMA {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            format!("{root}.schema"),
            json!(GRAPH_SCHEMA),
            json!(graph.schema),
        ));
    }
    if graph.operations.is_empty() || graph.operations.len() > MAX_OPERATIONS {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            format!("{root}.operations"),
            json!({"min":1,"max":MAX_OPERATIONS}),
            json!({"length":graph.operations.len()}),
        ));
    }
    for (operation_index, operation) in graph.operations.iter().enumerate() {
        if operation.operands.len() != operation.op.arity() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("{root}.operations[{operation_index}].operands"),
                json!({"length":operation.op.arity()}),
                json!({"length":operation.operands.len()}),
            ));
        }
        for (operand_index, operand) in operation.operands.iter().enumerate() {
            let path = format!("{root}.operations[{operation_index}].operands[{operand_index}]");
            match operand {
                GraphOperand::Scalar { name } if !task.scalars.contains(name) => {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        path,
                        json!({"declared_scalar":task.scalars}),
                        json!(operand),
                    ));
                }
                GraphOperand::Tensor { name } if !task.tensors.contains(name) => {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        path,
                        json!({"declared_tensor":task.tensors}),
                        json!(operand),
                    ));
                }
                GraphOperand::Local { operation } if *operation >= operation_index => {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        path,
                        json!({"local_operation_less_than":operation_index}),
                        json!(operand),
                    ));
                }
                GraphOperand::Scalar { .. }
                | GraphOperand::Tensor { .. }
                | GraphOperand::Local { .. } => {}
            }
        }
    }
    if graph.r#yield >= graph.operations.len() {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            format!("{root}.yield"),
            json!({"operation_less_than":graph.operations.len()}),
            json!(graph.r#yield),
        ));
    }
    Ok(())
}

fn compile_actions(task: &AuthoringTask, graph: &GraphProposal) -> Vec<Action> {
    let scalar_bindings = task
        .scalars
        .iter()
        .enumerate()
        .map(|(index, name)| (name, format!("$authoring_scalar_{index}")))
        .collect::<BTreeMap<_, _>>();
    let tensor_bindings = task
        .tensors
        .iter()
        .enumerate()
        .map(|(index, name)| (name, format!("$authoring_tensor_{index}")))
        .collect::<BTreeMap<_, _>>();
    let tensor_arguments = task
        .tensors
        .iter()
        .enumerate()
        .map(|(index, name)| (name, format!("authoring_argument_{index}")))
        .collect::<BTreeMap<_, _>>();
    let mut actions = vec![Action::DefineDimension {
        bind: Some("$authoring_dimension".to_owned()),
        name: task.dimension.clone(),
        constraints: vec![format!("{} >= 0", task.dimension)],
    }];
    actions.extend(task.scalars.iter().map(|name| Action::CreateParameter {
        bind: scalar_bindings[name].clone(),
        name: name.clone(),
        ty: Type::Scalar(ScalarType::F32),
    }));
    actions.extend(task.tensors.iter().map(|name| Action::CreateParameter {
        bind: tensor_bindings[name].clone(),
        name: name.clone(),
        ty: Type::Tensor {
            element: ScalarType::F32,
            shape: Shape(vec![DimExpr::Symbol(task.dimension.clone())]),
        },
    }));
    let operations = graph
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| RegionOpSpec {
            bind: format!("$authoring_local_{index}"),
            opcode: operation.op.as_str().to_owned(),
            operands: operation
                .operands
                .iter()
                .map(|operand| match operand {
                    GraphOperand::Scalar { name } => scalar_bindings[name].clone(),
                    GraphOperand::Tensor { name } => tensor_arguments[name].clone(),
                    GraphOperand::Local { operation } => {
                        format!("$authoring_local_{operation}")
                    }
                })
                .collect(),
            attributes: BTreeMap::new(),
        })
        .collect();
    actions.push(Action::CreateOp {
        bind: "$authoring_output".to_owned(),
        opcode: "zip_map".to_owned(),
        operands: task
            .tensors
            .iter()
            .map(|name| tensor_bindings[name].clone())
            .collect(),
        attributes: BTreeMap::new(),
        region: Some(RegionSpec {
            arguments: task
                .tensors
                .iter()
                .map(|name| RegionArgumentSpec {
                    name: tensor_arguments[name].clone(),
                    ty: Type::Scalar(ScalarType::F32),
                })
                .collect(),
            captures: task
                .scalars
                .iter()
                .map(|name| scalar_bindings[name].clone())
                .collect(),
            operations,
            yield_value: format!("$authoring_local_{}", graph.r#yield),
        }),
    });
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: "$authoring_output".to_owned(),
    });
    actions.push(Action::FreezeSpec);
    actions
}

fn call(
    engine: &mut Engine,
    request: Value,
    transcript: &mut Vec<AuthoringTranscriptEntry>,
) -> Result<Value, AuthoringError> {
    let request_id = request
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let response_text = engine.process_line(&request.to_string());
    let response: Value = serde_json::from_str(&response_text).map_err(|error| {
        failure(
            AuthoringErrorCode::CompilerRejected,
            format!("$.agentir.{request_id}"),
            json!("valid production response envelope"),
            json!(error.to_string()),
        )
    })?;
    transcript.push(AuthoringTranscriptEntry {
        request,
        response: response.clone(),
    });
    if response.get("ok") != Some(&Value::Bool(true)) {
        return Err(failure(
            AuthoringErrorCode::CompilerRejected,
            format!("$.agentir.{request_id}"),
            json!({"ok":true}),
            response,
        ));
    }
    response.get("result").cloned().ok_or_else(|| {
        failure(
            AuthoringErrorCode::CompilerRejected,
            format!("$.agentir.{request_id}.result"),
            json!("result object"),
            response,
        )
    })
}

fn object_at<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a Value, AuthoringError> {
    value
        .get(key)
        .filter(|item| item.is_object())
        .ok_or_else(|| {
            failure(
                AuthoringErrorCode::CompilerRejected,
                path,
                json!("object"),
                value.get(key).cloned().unwrap_or(Value::Null),
            )
        })
}

fn string_at(value: &Value, key: &str, path: &str) -> Result<String, AuthoringError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            failure(
                AuthoringErrorCode::CompilerRejected,
                path,
                json!("string"),
                value.get(key).cloned().unwrap_or(Value::Null),
            )
        })
}

fn value_at<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a Value, AuthoringError> {
    value.get(key).ok_or_else(|| {
        failure(
            AuthoringErrorCode::CompilerRejected,
            path,
            json!("present value"),
            Value::Null,
        )
    })
}
