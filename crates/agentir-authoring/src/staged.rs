//! Deterministic bounded staged-program expansion without manual graph indices.

use super::{
    AuthoringError, AuthoringErrorCode, GRAPH_SCHEMA, GraphOpcode, GraphOperand, GraphOperation,
    GraphProposal, MAX_OPERATIONS, failure, require_keys, require_object, valid_binding,
    with_repair_hint,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Exact schema identifier for the staged structural builder.
pub const STAGED_SCHEMA: &str = "agentir.elementwise_staged.v1";

/// Exact JSON Schema document for the staged structural builder.
pub const STAGED_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-staged-v1.schema.json");

/// Self-contained model instruction for the staged surface.
pub const STAGED_MODEL_INSTRUCTION: &str = r#"Return exactly one JSON object with this wire shape:
{"schema":"agentir.elementwise_staged.v1","stages":5,"seed":{"kind":"tensor","name":"x0"},"body":[{"bind":"$state","op":"add","operands":[{"kind":"state_prev"},{"kind":"state_lag","stages":3,"initial":[{"kind":"tensor","name":"x9"},{"kind":"tensor","name":"x8"},{"kind":"tensor","name":"x7"},{"kind":"tensor","name":"x6"}]}]}],"state":"$state"}
body has one to eight ordered operations and is expanded for a positive stages count, with at most 128 expanded operations. Every bind is a unique $ identifier. stage_local names only an earlier body binding; state_prev is seed at stage 0 and the prior state afterward. state_lag.initial is the complete explicit warmup prefix and may be longer than its positive stages lag: with four initial values and lag 3, stages 0/1/2/3 use all four values, then stage 4 uses state_(4-3)=state_1. scalar_cycle/tensor_cycle select prefix + ((stage*stride+offset)%count), with positive count. state names the body binding used as every stage state and the final yield. Use exact add/mul/fma arity and operand order; never replace fma. Return only this bounded structural payload, never types, inputs, source, compiler IDs, hashes, guards, certificates, bytecode, or backend settings."#;

/// One symbolic operand in a staged body template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StagedOperand {
    /// Direct scalar capture.
    Scalar {
        /// Exact name.
        name: String,
    },
    /// Direct tensor capture.
    Tensor {
        /// Exact name.
        name: String,
    },
    /// A result bound earlier in the current stage.
    StageLocal {
        /// Body binding beginning with `$`.
        name: String,
    },
    /// State result of the immediately preceding stage, or the seed at stage zero.
    StatePrev,
    /// State result a fixed number of stages back with explicit initial values.
    StateLag {
        /// Positive stage distance.
        stages: usize,
        /// Scalar/tensor warmup prefix before the lag recurrence begins.
        ///
        /// Its length may exceed `stages`, which represents a delayed
        /// recurrence such as four explicit warmup values followed by
        /// `state_(i-3)`.
        initial: Vec<GraphOperand>,
    },
    /// Scalar capture selected by `(stage * stride + offset) % count`.
    ScalarCycle {
        /// Capture prefix.
        prefix: String,
        /// Positive cycle size.
        count: usize,
        /// Stage multiplier.
        stride: usize,
        /// Added offset.
        offset: usize,
    },
    /// Tensor capture selected by `(stage * stride + offset) % count`.
    TensorCycle {
        /// Capture prefix.
        prefix: String,
        /// Positive cycle size.
        count: usize,
        /// Stage multiplier.
        stride: usize,
        /// Added offset.
        offset: usize,
    },
}

/// One named operation in a staged body template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedOperation {
    /// Unique body binding beginning with `$`.
    pub bind: String,
    /// Exact elementwise opcode.
    pub op: GraphOpcode,
    /// Ordered symbolic operands.
    pub operands: Vec<StagedOperand>,
}

/// Complete bounded staged-program request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedProposal {
    /// Must equal [`STAGED_SCHEMA`].
    pub schema: String,
    /// Positive number of stages.
    pub stages: usize,
    /// Scalar or tensor state before stage zero.
    pub seed: GraphOperand,
    /// Ordered stage body, repeated without mutation.
    pub body: Vec<StagedOperation>,
    /// Body binding that becomes the state and final yield.
    pub state: String,
}

/// Strictly decodes one staged builder request.
pub fn parse_staged(input: &str) -> Result<StagedProposal, AuthoringError> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        staged_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_staged.v1 object"),
            json!(error.to_string()),
        )
    })?;
    validate_staged_shape(&value)?;
    serde_json::from_value(value).map_err(|error| {
        staged_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_staged.v1 object"),
            json!(error.to_string()),
        )
    })
}

/// Deterministically expands a staged request into the ordinary graph contract.
pub fn compile_staged(source: &StagedProposal) -> Result<GraphProposal, AuthoringError> {
    compile_staged_inner(source).map_err(staged_hint)
}

fn compile_staged_inner(source: &StagedProposal) -> Result<GraphProposal, AuthoringError> {
    if source.schema != STAGED_SCHEMA {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(STAGED_SCHEMA),
            json!(source.schema),
        ));
    }
    if source.stages == 0 {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.stages",
            json!({"min":1}),
            json!(source.stages),
        ));
    }
    if source.body.is_empty() || source.body.len() > 8 {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.body",
            json!({"min":1,"max":8}),
            json!({"length":source.body.len()}),
        ));
    }
    let Some(expanded_operations) = source.stages.checked_mul(source.body.len()) else {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.stages",
            json!({"expanded_max":MAX_OPERATIONS}),
            json!({"stages":source.stages,"body":source.body.len(),"expanded":"overflow"}),
        ));
    };
    if expanded_operations > MAX_OPERATIONS {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.stages",
            json!({"expanded_max":MAX_OPERATIONS}),
            json!({"stages":source.stages,"body":source.body.len(),"expanded":expanded_operations}),
        ));
    }
    if matches!(source.seed, GraphOperand::Local { .. }) {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.seed",
            json!("scalar or tensor"),
            json!(source.seed),
        ));
    }
    let mut body_bindings = BTreeMap::new();
    for (index, operation) in source.body.iter().enumerate() {
        if !valid_binding(&operation.bind)
            || body_bindings
                .insert(operation.bind.clone(), index)
                .is_some()
        {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.body[{index}].bind"),
                json!("unique identifier beginning with $"),
                json!(operation.bind),
            ));
        }
        if operation.operands.len() != operation.op.arity() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.body[{index}].operands"),
                json!({"length":operation.op.arity()}),
                json!({"length":operation.operands.len()}),
            ));
        }
        for (operand_index, operand) in operation.operands.iter().enumerate() {
            let operand_path = format!("$.body[{index}].operands[{operand_index}]");
            if let StagedOperand::StageLocal { name } = operand
                && body_bindings.get(name).is_none_or(|prior| *prior >= index)
            {
                return Err(failure(
                    AuthoringErrorCode::ValidationRejected,
                    operand_path,
                    json!("earlier body binding"),
                    json!(name),
                ));
            }
            if let StagedOperand::StateLag { stages, initial } = operand {
                if *stages == 0 {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!("{operand_path}.stages"),
                        json!("positive integer"),
                        json!(stages),
                    ));
                }
                if initial.len() < *stages {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        operand_path,
                        json!({"minimum_length":stages}),
                        json!({"length":initial.len()}),
                    ));
                }
                if let Some((initial_index, value)) = initial
                    .iter()
                    .enumerate()
                    .find(|(_, value)| matches!(value, GraphOperand::Local { .. }))
                {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!("{operand_path}.initial[{initial_index}]"),
                        json!("scalar or tensor warmup value"),
                        json!(value),
                    ));
                }
            }
            if matches!(
                operand,
                StagedOperand::ScalarCycle { count: 0, .. }
                    | StagedOperand::TensorCycle { count: 0, .. }
            ) {
                return Err(failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("{operand_path}.count"),
                    json!("positive integer"),
                    json!(0),
                ));
            }
        }
    }
    let Some(state_offset) = body_bindings.get(&source.state).copied() else {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.state",
            json!({"body_binding":body_bindings.keys().collect::<Vec<_>>()}),
            json!(source.state),
        ));
    };
    let mut operations = Vec::with_capacity(expanded_operations);
    let mut states = Vec::with_capacity(source.stages);
    for stage in 0..source.stages {
        let base = operations.len();
        for operation in &source.body {
            let operands = operation
                .operands
                .iter()
                .map(|operand| match operand {
                    StagedOperand::Scalar { name } => GraphOperand::Scalar { name: name.clone() },
                    StagedOperand::Tensor { name } => GraphOperand::Tensor { name: name.clone() },
                    StagedOperand::StageLocal { name } => GraphOperand::Local {
                        operation: base + body_bindings[name],
                    },
                    StagedOperand::StatePrev => {
                        if stage == 0 {
                            source.seed.clone()
                        } else {
                            GraphOperand::Local {
                                operation: states[stage - 1],
                            }
                        }
                    }
                    StagedOperand::StateLag { stages, initial } => {
                        if stage < initial.len() {
                            initial[stage].clone()
                        } else {
                            GraphOperand::Local {
                                operation: states[stage - stages],
                            }
                        }
                    }
                    StagedOperand::ScalarCycle {
                        prefix,
                        count,
                        stride,
                        offset,
                    } => GraphOperand::Scalar {
                        name: format!("{prefix}{}", cycle_index(stage, *stride, *offset, *count)),
                    },
                    StagedOperand::TensorCycle {
                        prefix,
                        count,
                        stride,
                        offset,
                    } => GraphOperand::Tensor {
                        name: format!("{prefix}{}", cycle_index(stage, *stride, *offset, *count)),
                    },
                })
                .collect();
            operations.push(GraphOperation {
                op: operation.op,
                operands,
            });
        }
        states.push(base + state_offset);
    }
    Ok(GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: *states.last().expect("positive stages"),
    })
}

fn cycle_index(stage: usize, stride: usize, offset: usize, count: usize) -> usize {
    let value = ((stage as u128) * (stride as u128) + (offset as u128)) % (count as u128);
    usize::try_from(value).expect("cycle result is less than usize count")
}

fn validate_staged_shape(value: &Value) -> Result<(), AuthoringError> {
    let root = require_object(value, "$").map_err(staged_hint)?;
    require_keys(root, "$", &["schema", "stages", "seed", "body", "state"]).map_err(staged_hint)?;
    if root.get("schema").and_then(Value::as_str) != Some(STAGED_SCHEMA) {
        return Err(staged_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(STAGED_SCHEMA),
            root.get("schema").cloned().unwrap_or(Value::Null),
        ));
    }
    let stages = expect_usize(root.get("stages"), "$.stages")?;
    if stages == 0 {
        return Err(staged_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.stages",
            json!({"minimum":1}),
            json!(stages),
        ));
    }
    validate_graph_operand_shape(root.get("seed").unwrap_or(&Value::Null), "$.seed")?;
    let body = expect_array(root.get("body"), "$.body")?;
    if body.is_empty() || body.len() > 8 {
        return Err(staged_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.body",
            json!({"min_items":1,"max_items":8}),
            json!({"length":body.len()}),
        ));
    }
    for (index, operation) in body.iter().enumerate() {
        validate_staged_operation_shape(operation, &format!("$.body[{index}]"))?;
    }
    expect_string(root.get("state"), "$.state")?;
    Ok(())
}

fn validate_staged_operation_shape(value: &Value, path: &str) -> Result<(), AuthoringError> {
    let operation = require_object(value, path).map_err(staged_hint)?;
    require_keys(operation, path, &["bind", "op", "operands"]).map_err(staged_hint)?;
    expect_string(operation.get("bind"), &format!("{path}.bind"))?;
    if !matches!(
        operation.get("op").and_then(Value::as_str),
        Some("add" | "mul" | "fma")
    ) {
        return Err(staged_failure(
            AuthoringErrorCode::SchemaRejected,
            format!("{path}.op"),
            json!(["add", "mul", "fma"]),
            operation.get("op").cloned().unwrap_or(Value::Null),
        ));
    }
    let operands = expect_array(operation.get("operands"), &format!("{path}.operands"))?;
    for (index, operand) in operands.iter().enumerate() {
        validate_staged_operand_shape(operand, &format!("{path}.operands[{index}]"))?;
    }
    Ok(())
}

fn validate_staged_operand_shape(value: &Value, path: &str) -> Result<(), AuthoringError> {
    let operand = require_object(value, path).map_err(staged_hint)?;
    match operand.get("kind").and_then(Value::as_str) {
        Some("scalar" | "tensor" | "stage_local") => {
            require_keys(operand, path, &["kind", "name"]).map_err(staged_hint)?;
            expect_string(operand.get("name"), &format!("{path}.name"))?;
        }
        Some("state_prev") => {
            require_keys(operand, path, &["kind"]).map_err(staged_hint)?;
        }
        Some("state_lag") => {
            require_keys(operand, path, &["kind", "stages", "initial"]).map_err(staged_hint)?;
            expect_usize(operand.get("stages"), &format!("{path}.stages"))?;
            let initial = expect_array(operand.get("initial"), &format!("{path}.initial"))?;
            for (index, value) in initial.iter().enumerate() {
                validate_graph_operand_shape(value, &format!("{path}.initial[{index}]"))?;
            }
        }
        Some("scalar_cycle" | "tensor_cycle") => {
            require_keys(
                operand,
                path,
                &["kind", "prefix", "count", "stride", "offset"],
            )
            .map_err(staged_hint)?;
            expect_string(operand.get("prefix"), &format!("{path}.prefix"))?;
            expect_usize(operand.get("count"), &format!("{path}.count"))?;
            expect_usize(operand.get("stride"), &format!("{path}.stride"))?;
            expect_usize(operand.get("offset"), &format!("{path}.offset"))?;
        }
        _ => {
            return Err(staged_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.kind"),
                json!([
                    "scalar",
                    "tensor",
                    "stage_local",
                    "state_prev",
                    "state_lag",
                    "scalar_cycle",
                    "tensor_cycle"
                ]),
                operand.get("kind").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    Ok(())
}

fn validate_graph_operand_shape(value: &Value, path: &str) -> Result<(), AuthoringError> {
    let operand = require_object(value, path).map_err(staged_hint)?;
    match operand.get("kind").and_then(Value::as_str) {
        Some("scalar" | "tensor") => {
            require_keys(operand, path, &["kind", "name"]).map_err(staged_hint)?;
            expect_string(operand.get("name"), &format!("{path}.name"))?;
        }
        Some("local") => {
            require_keys(operand, path, &["kind", "operation"]).map_err(staged_hint)?;
            expect_usize(operand.get("operation"), &format!("{path}.operation"))?;
        }
        _ => {
            return Err(staged_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.kind"),
                json!(["scalar", "tensor", "local"]),
                operand.get("kind").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    Ok(())
}

fn expect_array<'a>(value: Option<&'a Value>, path: &str) -> Result<&'a [Value], AuthoringError> {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            staged_failure(
                AuthoringErrorCode::SchemaRejected,
                path,
                json!("array"),
                value.cloned().unwrap_or(Value::Null),
            )
        })
}

fn expect_string(value: Option<&Value>, path: &str) -> Result<(), AuthoringError> {
    if value.is_some_and(Value::is_string) {
        return Ok(());
    }
    Err(staged_failure(
        AuthoringErrorCode::SchemaRejected,
        path,
        json!("string"),
        value.cloned().unwrap_or(Value::Null),
    ))
}

fn expect_usize(value: Option<&Value>, path: &str) -> Result<usize, AuthoringError> {
    let Some(number) = value
        .and_then(Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
    else {
        return Err(staged_failure(
            AuthoringErrorCode::SchemaRejected,
            path,
            json!("non-negative platform-sized integer"),
            value.cloned().unwrap_or(Value::Null),
        ));
    };
    Ok(number)
}

fn staged_failure(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    expected: Value,
    actual: Value,
) -> AuthoringError {
    with_repair_hint(
        failure(code, path, expected, actual),
        STAGED_MODEL_INSTRUCTION,
    )
}

fn staged_hint(error: AuthoringError) -> AuthoringError {
    with_repair_hint(error, STAGED_MODEL_INSTRUCTION)
}
