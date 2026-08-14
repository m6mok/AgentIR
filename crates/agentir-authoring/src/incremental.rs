//! Atomic small-transaction authoring with symbolic local bindings.

use super::{
    AuthoringError, AuthoringErrorCode, GRAPH_SCHEMA, GraphOpcode, GraphOperand, GraphOperation,
    GraphProposal, MAX_OPERATIONS, failure, require_keys, require_object, valid_binding,
    with_repair_hint,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Exact schema identifier for one incremental authoring transaction.
pub const TRANSACTION_SCHEMA: &str = "agentir.elementwise_transaction.v1";

/// Exact schema identifier for one complete incremental model payload.
pub const INCREMENTAL_BATCH_SCHEMA: &str = "agentir.elementwise_incremental_batch.v1";

/// Exact JSON Schema document for one incremental transaction.
pub const TRANSACTION_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-transaction-v1.schema.json");

/// Exact JSON Schema document for a complete incremental batch.
pub const INCREMENTAL_BATCH_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-incremental-batch-v1.schema.json");

/// Self-contained model instruction for the incremental batch surface.
pub const INCREMENTAL_BATCH_MODEL_INSTRUCTION: &str = r#"Return exactly one JSON object with this wire shape:
{"schema":"agentir.elementwise_incremental_batch.v1","transactions":[{"schema":"agentir.elementwise_transaction.v1","base_operations":0,"operations":[{"bind":"$ax","op":"mul","operands":[{"kind":"scalar","name":"a"},{"kind":"tensor","name":"x"}]}]},{"schema":"agentir.elementwise_transaction.v1","base_operations":1,"operations":[{"bind":"$out","op":"add","operands":[{"kind":"local","name":"$ax"},{"kind":"tensor","name":"y"}]}]}],"yield":"$out"}
Each transaction contains one to eight operations and base_operations is the exact number of operations accepted before that transaction: start at 0 and increase it by the prior transaction lengths with no gaps, duplicates, or reordering. Every bind is a unique identifier beginning with $ for this payload. A local operand names only a binding accepted earlier in the same payload, including an earlier operation in the current transaction. yield is one known binding. Use op add/mul/fma with exactly 2/2/3 ordered operands and never replace fma with mul plus add. Return only the batch; the server owns types, inputs, compiler IDs, hashes, guards, certificates, intent checking, and publication."#;

const MAX_TRANSACTION_OPERATIONS: usize = 8;

/// One operand in an incremental transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IncrementalOperand {
    /// Server-declared scalar capture.
    Scalar {
        /// Exact scalar name.
        name: String,
    },
    /// Server-declared tensor capture.
    Tensor {
        /// Exact tensor name.
        name: String,
    },
    /// A symbolic result from an earlier accepted operation.
    Local {
        /// Authoring-local binding beginning with `$`.
        name: String,
    },
}

/// One symbolically bound operation in a transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalOperation {
    /// New globally unique authoring-local binding beginning with `$`.
    pub bind: String,
    /// Exact elementwise opcode.
    pub op: GraphOpcode,
    /// Ordered operands.
    pub operands: Vec<IncrementalOperand>,
}

/// One atomic edit against an explicit single-session operation-count base.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalTransaction {
    /// Must equal [`TRANSACTION_SCHEMA`].
    pub schema: String,
    /// Exact accepted operation count on which this edit is based.
    ///
    /// This is a local optimistic-concurrency cursor, not a compiler revision,
    /// persistent ID, or hash. Because every accepted edit is non-empty and the
    /// adapter is single-session, equality with the current count detects stale,
    /// duplicated, gapped, and reordered edits deterministically.
    pub base_operations: usize,
    /// One to eight topologically ordered operations.
    pub operations: Vec<IncrementalOperation>,
}

/// One complete incremental model payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalBatch {
    /// Must equal [`INCREMENTAL_BATCH_SCHEMA`].
    pub schema: String,
    /// Ordered atomic transactions forming one final graph proposal.
    pub transactions: Vec<IncrementalTransaction>,
    /// Previously bound result that becomes the graph yield.
    pub r#yield: String,
}

/// Compiler-owned receipt after one accepted incremental transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IncrementalReceipt {
    /// New total operation count.
    pub operation_count: usize,
    /// Bindings introduced by this transaction in source order.
    pub accepted_bindings: Vec<String>,
}

/// Server-owned incremental graph session.
#[derive(Clone, Debug)]
pub struct IncrementalSession {
    scalars: BTreeSet<String>,
    tensors: BTreeSet<String>,
    operations: Vec<GraphOperation>,
    bindings: BTreeMap<String, usize>,
}

impl IncrementalSession {
    /// Creates an empty session over server-declared capture names.
    #[must_use]
    pub fn new(
        scalars: impl IntoIterator<Item = String>,
        tensors: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            scalars: scalars.into_iter().collect(),
            tensors: tensors.into_iter().collect(),
            operations: Vec::new(),
            bindings: BTreeMap::new(),
        }
    }

    /// Applies one transaction atomically or leaves the session unchanged.
    pub fn apply(
        &mut self,
        transaction: &IncrementalTransaction,
    ) -> Result<IncrementalReceipt, AuthoringError> {
        self.apply_at(transaction, "$")
    }

    pub(crate) fn apply_at(
        &mut self,
        transaction: &IncrementalTransaction,
        root: &str,
    ) -> Result<IncrementalReceipt, AuthoringError> {
        if transaction.schema != TRANSACTION_SCHEMA {
            return Err(incremental_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{root}.schema"),
                json!(TRANSACTION_SCHEMA),
                json!(transaction.schema),
            ));
        }
        if transaction.base_operations != self.operations.len() {
            return Err(incremental_failure(
                AuthoringErrorCode::ValidationRejected,
                format!("{root}.base_operations"),
                json!(self.operations.len()),
                json!(transaction.base_operations),
            ));
        }
        if transaction.operations.is_empty()
            || transaction.operations.len() > MAX_TRANSACTION_OPERATIONS
        {
            return Err(incremental_failure(
                AuthoringErrorCode::ValidationRejected,
                format!("{root}.operations"),
                json!({"min":1,"max":MAX_TRANSACTION_OPERATIONS}),
                json!({"length":transaction.operations.len()}),
            ));
        }
        let Some(projected_operations) = self
            .operations
            .len()
            .checked_add(transaction.operations.len())
        else {
            return Err(incremental_failure(
                AuthoringErrorCode::ValidationRejected,
                format!("{root}.operations"),
                json!({"total_max":MAX_OPERATIONS}),
                json!("operation count overflow"),
            ));
        };
        if projected_operations > MAX_OPERATIONS {
            return Err(incremental_failure(
                AuthoringErrorCode::ValidationRejected,
                format!("{root}.operations"),
                json!({"total_max":MAX_OPERATIONS}),
                json!({"total":projected_operations}),
            ));
        }

        let mut operations = self.operations.clone();
        let mut bindings = self.bindings.clone();
        let mut accepted_bindings = Vec::with_capacity(transaction.operations.len());
        for (offset, source) in transaction.operations.iter().enumerate() {
            let path = format!("{root}.operations[{offset}]");
            if !valid_binding(&source.bind) || bindings.contains_key(&source.bind) {
                return Err(incremental_failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("{path}.bind"),
                    json!("new identifier beginning with $"),
                    json!(source.bind),
                ));
            }
            if source.operands.len() != source.op.arity() {
                return Err(incremental_failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("{path}.operands"),
                    json!({"length":source.op.arity()}),
                    json!({"length":source.operands.len()}),
                ));
            }
            let mut operands = Vec::with_capacity(source.operands.len());
            for (operand_index, operand) in source.operands.iter().enumerate() {
                let operand_path = format!("{path}.operands[{operand_index}]");
                operands.push(match operand {
                    IncrementalOperand::Scalar { name } if self.scalars.contains(name) => {
                        GraphOperand::Scalar { name: name.clone() }
                    }
                    IncrementalOperand::Tensor { name } if self.tensors.contains(name) => {
                        GraphOperand::Tensor { name: name.clone() }
                    }
                    IncrementalOperand::Local { name } if bindings.contains_key(name) => {
                        GraphOperand::Local {
                            operation: bindings[name],
                        }
                    }
                    IncrementalOperand::Scalar { name } => {
                        return Err(incremental_failure(
                            AuthoringErrorCode::ValidationRejected,
                            operand_path,
                            json!({"declared_scalar":self.scalars}),
                            json!(name),
                        ));
                    }
                    IncrementalOperand::Tensor { name } => {
                        return Err(incremental_failure(
                            AuthoringErrorCode::ValidationRejected,
                            operand_path,
                            json!({"declared_tensor":self.tensors}),
                            json!(name),
                        ));
                    }
                    IncrementalOperand::Local { name } => {
                        return Err(incremental_failure(
                            AuthoringErrorCode::ValidationRejected,
                            operand_path,
                            json!({"known_prior_binding":bindings.keys().collect::<Vec<_>>()}),
                            json!(name),
                        ));
                    }
                });
            }
            let operation = operations.len();
            operations.push(GraphOperation {
                op: source.op,
                operands,
            });
            bindings.insert(source.bind.clone(), operation);
            accepted_bindings.push(source.bind.clone());
        }
        self.operations = operations;
        self.bindings = bindings;
        Ok(IncrementalReceipt {
            operation_count: self.operations.len(),
            accepted_bindings,
        })
    }

    /// Finishes the session by yielding one accepted symbolic binding.
    pub fn finish(&self, binding: &str) -> Result<GraphProposal, AuthoringError> {
        self.finish_at(binding, "$.yield")
    }

    pub(crate) fn finish_at(
        &self,
        binding: &str,
        path: &str,
    ) -> Result<GraphProposal, AuthoringError> {
        let Some(operation) = self.bindings.get(binding) else {
            return Err(incremental_failure(
                AuthoringErrorCode::ValidationRejected,
                path,
                json!({"known_binding":self.bindings.keys().collect::<Vec<_>>()}),
                json!(binding),
            ));
        };
        Ok(GraphProposal {
            schema: GRAPH_SCHEMA.to_owned(),
            operations: self.operations.clone(),
            r#yield: *operation,
        })
    }

    /// Returns the current accepted operation count.
    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

/// Strictly decodes one incremental transaction.
pub fn parse_transaction(input: &str) -> Result<IncrementalTransaction, AuthoringError> {
    let value = parse_incremental_value(input, "strict agentir.elementwise_transaction.v1 object")?;
    validate_transaction_shape(&value, "$")?;
    serde_json::from_value(value).map_err(|error| {
        incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_transaction.v1 object"),
            json!(error.to_string()),
        )
    })
}

/// Strictly decodes one complete incremental batch.
pub fn parse_incremental_batch(input: &str) -> Result<IncrementalBatch, AuthoringError> {
    let value = parse_incremental_value(
        input,
        "strict agentir.elementwise_incremental_batch.v1 object",
    )?;
    validate_batch_shape(&value)?;
    serde_json::from_value(value).map_err(|error| {
        incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_incremental_batch.v1 object"),
            json!(error.to_string()),
        )
    })
}

/// Compiles a complete batch through one private incremental session.
///
/// Transactions remain atomic individually. If any transaction or the final
/// yield is rejected, the private session is dropped and no partial prefix is
/// observable or publishable.
pub fn compile_incremental_batch(
    source: &IncrementalBatch,
    scalars: impl IntoIterator<Item = String>,
    tensors: impl IntoIterator<Item = String>,
) -> Result<GraphProposal, AuthoringError> {
    if source.schema != INCREMENTAL_BATCH_SCHEMA {
        return Err(incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(INCREMENTAL_BATCH_SCHEMA),
            json!(source.schema),
        ));
    }
    if source.transactions.is_empty() || source.transactions.len() > MAX_OPERATIONS {
        return Err(incremental_failure(
            AuthoringErrorCode::ValidationRejected,
            "$.transactions",
            json!({"min":1,"max":MAX_OPERATIONS}),
            json!({"length":source.transactions.len()}),
        ));
    }
    let mut session = IncrementalSession::new(scalars, tensors);
    for (index, transaction) in source.transactions.iter().enumerate() {
        session.apply_at(transaction, &format!("$.transactions[{index}]"))?;
    }
    session.finish_at(&source.r#yield, "$.yield")
}

fn parse_incremental_value(input: &str, expected: &str) -> Result<Value, AuthoringError> {
    serde_json::from_str(input).map_err(|error| {
        incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!(expected),
            json!(error.to_string()),
        )
    })
}

fn validate_batch_shape(value: &Value) -> Result<(), AuthoringError> {
    let root = require_object(value, "$").map_err(incremental_hint)?;
    require_keys(root, "$", &["schema", "transactions", "yield"]).map_err(incremental_hint)?;
    expect_schema(root, "$", INCREMENTAL_BATCH_SCHEMA)?;
    let transactions = expect_array(root.get("transactions"), "$.transactions")?;
    if transactions.is_empty() || transactions.len() > MAX_OPERATIONS {
        return Err(incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.transactions",
            json!({"min_items":1,"max_items":MAX_OPERATIONS}),
            json!({"length":transactions.len()}),
        ));
    }
    for (index, transaction) in transactions.iter().enumerate() {
        validate_transaction_shape(transaction, &format!("$.transactions[{index}]"))?;
    }
    expect_string(root.get("yield"), "$.yield")?;
    Ok(())
}

fn validate_transaction_shape(value: &Value, root_path: &str) -> Result<(), AuthoringError> {
    let root = require_object(value, root_path).map_err(incremental_hint)?;
    require_keys(
        root,
        root_path,
        &["schema", "base_operations", "operations"],
    )
    .map_err(incremental_hint)?;
    expect_schema(root, root_path, TRANSACTION_SCHEMA)?;
    expect_usize(
        root.get("base_operations"),
        &format!("{root_path}.base_operations"),
    )?;
    let operations = expect_array(root.get("operations"), &format!("{root_path}.operations"))?;
    if operations.is_empty() || operations.len() > MAX_TRANSACTION_OPERATIONS {
        return Err(incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            format!("{root_path}.operations"),
            json!({"min_items":1,"max_items":MAX_TRANSACTION_OPERATIONS}),
            json!({"length":operations.len()}),
        ));
    }
    for (index, operation) in operations.iter().enumerate() {
        validate_operation_shape(operation, &format!("{root_path}.operations[{index}]"))?;
    }
    Ok(())
}

fn validate_operation_shape(value: &Value, path: &str) -> Result<(), AuthoringError> {
    let operation = require_object(value, path).map_err(incremental_hint)?;
    require_keys(operation, path, &["bind", "op", "operands"]).map_err(incremental_hint)?;
    expect_string(operation.get("bind"), &format!("{path}.bind"))?;
    expect_opcode(operation.get("op"), &format!("{path}.op"))?;
    let operands = expect_array(operation.get("operands"), &format!("{path}.operands"))?;
    for (index, operand) in operands.iter().enumerate() {
        validate_operand_shape(operand, &format!("{path}.operands[{index}]"))?;
    }
    Ok(())
}

fn validate_operand_shape(value: &Value, path: &str) -> Result<(), AuthoringError> {
    let operand = require_object(value, path).map_err(incremental_hint)?;
    match operand.get("kind").and_then(Value::as_str) {
        Some("scalar" | "tensor" | "local") => {
            require_keys(operand, path, &["kind", "name"]).map_err(incremental_hint)?;
            expect_string(operand.get("name"), &format!("{path}.name"))?;
        }
        _ => {
            return Err(incremental_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.kind"),
                json!(["scalar", "tensor", "local"]),
                operand.get("kind").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    Ok(())
}

fn expect_schema(
    object: &serde_json::Map<String, Value>,
    root: &str,
    expected: &str,
) -> Result<(), AuthoringError> {
    if object.get("schema").and_then(Value::as_str) != Some(expected) {
        return Err(incremental_failure(
            AuthoringErrorCode::SchemaRejected,
            format!("{root}.schema"),
            json!(expected),
            object.get("schema").cloned().unwrap_or(Value::Null),
        ));
    }
    Ok(())
}

fn expect_array<'a>(value: Option<&'a Value>, path: &str) -> Result<&'a [Value], AuthoringError> {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            incremental_failure(
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
    Err(incremental_failure(
        AuthoringErrorCode::SchemaRejected,
        path,
        json!("string"),
        value.cloned().unwrap_or(Value::Null),
    ))
}

fn expect_usize(value: Option<&Value>, path: &str) -> Result<(), AuthoringError> {
    if value
        .and_then(Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
        .is_some()
    {
        return Ok(());
    }
    Err(incremental_failure(
        AuthoringErrorCode::SchemaRejected,
        path,
        json!("non-negative platform-sized integer"),
        value.cloned().unwrap_or(Value::Null),
    ))
}

fn expect_opcode(value: Option<&Value>, path: &str) -> Result<(), AuthoringError> {
    if matches!(value.and_then(Value::as_str), Some("add" | "mul" | "fma")) {
        return Ok(());
    }
    Err(incremental_failure(
        AuthoringErrorCode::SchemaRejected,
        path,
        json!(["add", "mul", "fma"]),
        value.cloned().unwrap_or(Value::Null),
    ))
}

fn incremental_failure(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    expected: Value,
    actual: Value,
) -> AuthoringError {
    with_repair_hint(
        failure(code, path, expected, actual),
        INCREMENTAL_BATCH_MODEL_INSTRUCTION,
    )
}

fn incremental_hint(error: AuthoringError) -> AuthoringError {
    with_repair_hint(error, INCREMENTAL_BATCH_MODEL_INSTRUCTION)
}
