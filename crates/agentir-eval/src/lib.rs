//! Deterministic CPU reference interpreter for SpecIR, ImplIR and guarded candidates.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    candidate::{CandidateForest, DifferentialValidation, GuardPredicate},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{BufferId, CandidateId, CandidateRevisionId, ImplValueId, MemoryGuardId, ValueId},
    impl_ir::{ImplProgram, impl_as_program},
    ir::{ConstantValue, Opcode, Operation, Program, Region, RegionValue, ValueOrigin},
    memory::{MemoryRevision, MemoryStatus},
    memory_ir::{
        AccessMode, AliasRelation, BufferAccessKind, MEMORY_TRACE_CODEC_VERSION, MemoryBuffer,
        Ownership, ReuseDecision, verify_memory_program,
    },
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    types::{DimExpr, ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value as JsonValue, json};
use std::collections::{BTreeMap, BTreeSet};

/// Scalar runtime value supported by the reference interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarValue {
    /// Boolean scalar.
    Bool(bool),
    /// Signed 32-bit scalar or logical index.
    I32(i32),
    /// IEEE-754 binary32 scalar.
    F32(f32),
}

/// Row-major dense tensor value.
#[derive(Clone, Debug, PartialEq)]
pub struct DenseTensor {
    /// Element type.
    pub element_type: ScalarType,
    /// Concrete runtime shape.
    pub shape: Vec<usize>,
    /// Flattened row-major elements.
    pub elements: Vec<ScalarValue>,
}

/// Scalar or dense tensor value evaluated by the CPU oracle.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeValue {
    /// Scalar value.
    Scalar(ScalarValue),
    /// Dense tensor value.
    Tensor(DenseTensor),
}

/// Output bundle returned by an evaluation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Named JSON-compatible outputs.
    pub outputs: BTreeMap<String, JsonValue>,
    /// Concrete symbolic dimensions inferred from tensor inputs.
    pub dimensions: BTreeMap<String, usize>,
}

/// One deterministic high-level MemoryIR execution-trace event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTraceEvent {
    /// Zero-based stable trace sequence.
    pub sequence: u64,
    /// Stable event kind.
    pub kind: String,
    /// Related abstract buffer, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer: Option<BufferId>,
    /// Deterministic human-readable detail without addresses or timing.
    pub detail: String,
}

/// Exact MemoryIR evaluation result with branch outcomes and deterministic trace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryEvaluationResult {
    /// Named semantic outputs, identical to the anchored ImplIR oracle.
    pub evaluation: EvaluationResult,
    /// Actual compiler-owned guard decisions taken by this evaluation.
    pub guard_outcomes: BTreeMap<MemoryGuardId, bool>,
    /// Memory trace codec version, independent of MemoryIR semantics.
    pub trace_codec_version: u32,
    /// Deterministic high-level allocation/access/reuse trace.
    pub trace: Vec<MemoryTraceEvent>,
}

fn mismatch(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::EvaluationInputMismatch, message)
}

fn parse_scalar(ty: ScalarType, value: &JsonValue) -> AgentResult<ScalarValue> {
    match ty {
        ScalarType::Bool => value
            .as_bool()
            .map(ScalarValue::Bool)
            .ok_or_else(|| mismatch("expected a boolean input")),
        ScalarType::I32 | ScalarType::Index => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(ScalarValue::I32)
            .ok_or_else(|| mismatch("expected an integer input in i32 range")),
        ScalarType::F32 => value
            .as_f64()
            .map(|value| ScalarValue::F32(value as f32))
            .ok_or_else(|| mismatch("expected a numeric f32 input")),
    }
}

fn flatten_json(
    value: &JsonValue,
    depth: usize,
    shape: &mut Vec<usize>,
    scalars: &mut Vec<JsonValue>,
    max_elements: u64,
    max_depth: u64,
) -> AgentResult<()> {
    BudgetCheck::ensure(
        ResourceKind::JsonNestingDepth,
        max_depth,
        u64::try_from(depth).unwrap_or(u64::MAX).saturating_add(1),
        "evaluation tensor JSON",
    )?;
    let array = value
        .as_array()
        .ok_or_else(|| mismatch("tensor input must use nested JSON arrays"))?;
    if shape.len() == depth {
        shape.push(array.len());
    } else if shape[depth] != array.len() {
        return Err(mismatch("tensor input is not rectangular"));
    }
    if array.first().is_some_and(JsonValue::is_array) {
        for child in array {
            flatten_json(child, depth + 1, shape, scalars, max_elements, max_depth)?;
        }
    } else {
        if array.iter().any(JsonValue::is_array) {
            return Err(mismatch("tensor input mixes scalar and array elements"));
        }
        let attempted = u64::try_from(scalars.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(array.len()).unwrap_or(u64::MAX));
        BudgetCheck::ensure(
            ResourceKind::EvaluationTensorElements,
            max_elements,
            attempted,
            "evaluation input tensor",
        )?;
        scalars.extend(array.iter().cloned());
    }
    Ok(())
}

fn bind_dimension(
    expression: &DimExpr,
    actual: usize,
    dimensions: &mut BTreeMap<String, usize>,
) -> AgentResult<()> {
    match expression {
        DimExpr::Static(expected) => {
            if usize::try_from(*expected).ok() == Some(actual) {
                Ok(())
            } else {
                Err(mismatch(format!(
                    "static dimension mismatch: expected {expected}, got {actual}"
                )))
            }
        }
        DimExpr::Symbol(symbol) => match dimensions.get(symbol) {
            Some(expected) if *expected != actual => Err(mismatch(format!(
                "symbolic dimension `{symbol}` was {expected}, got {actual}"
            ))),
            Some(_) => Ok(()),
            None => {
                dimensions.insert(symbol.clone(), actual);
                Ok(())
            }
        },
        DimExpr::Affine {
            coefficient,
            symbol,
            constant,
        } => {
            let actual = i64::try_from(actual).map_err(|_| mismatch("dimension exceeds i64"))?;
            if *coefficient == 0 {
                return (actual == *constant)
                    .then_some(())
                    .ok_or_else(|| mismatch("affine constant dimension mismatch"));
            }
            let numerator = actual - constant;
            if numerator % coefficient != 0 || numerator / coefficient < 0 {
                return Err(mismatch(format!(
                    "cannot solve affine dimension `{expression}` for extent {actual}"
                )));
            }
            let symbol_value = usize::try_from(numerator / coefficient)
                .map_err(|_| mismatch("affine dimension result is out of range"))?;
            match dimensions.get(symbol) {
                Some(expected) if *expected != symbol_value => Err(mismatch(format!(
                    "affine dimension `{symbol}` was {expected}, inferred {symbol_value}"
                ))),
                Some(_) => Ok(()),
                None => {
                    dimensions.insert(symbol.clone(), symbol_value);
                    Ok(())
                }
            }
        }
    }
}

fn parse_input(
    ty: &Type,
    value: &JsonValue,
    dimensions: &mut BTreeMap<String, usize>,
    limits: &ResourceLimits,
) -> AgentResult<RuntimeValue> {
    match ty {
        Type::Scalar(scalar) => parse_scalar(*scalar, value).map(RuntimeValue::Scalar),
        Type::Tensor { element, shape } => {
            let mut concrete_shape = Vec::new();
            let mut scalars = Vec::new();
            flatten_json(
                value,
                0,
                &mut concrete_shape,
                &mut scalars,
                limits.evaluation_tensor_elements,
                limits.json_nesting_depth,
            )?;
            if concrete_shape.len() != shape.0.len() {
                return Err(mismatch(format!(
                    "tensor rank mismatch: expected {}, got {}",
                    shape.0.len(),
                    concrete_shape.len()
                )));
            }
            for (expression, actual) in shape.0.iter().zip(&concrete_shape) {
                bind_dimension(expression, *actual, dimensions)?;
            }
            let elements = scalars
                .iter()
                .map(|scalar| parse_scalar(*element, scalar))
                .collect::<AgentResult<Vec<_>>>()?;
            Ok(RuntimeValue::Tensor(DenseTensor {
                element_type: *element,
                shape: concrete_shape,
                elements,
            }))
        }
    }
}

fn concrete_dimension(
    expression: &DimExpr,
    dimensions: &BTreeMap<String, usize>,
) -> AgentResult<u64> {
    match expression {
        DimExpr::Static(value) => Ok(*value),
        DimExpr::Symbol(symbol) => dimensions
            .get(symbol)
            .copied()
            .map(|value| u64::try_from(value).unwrap_or(u64::MAX))
            .ok_or_else(|| mismatch(format!("symbolic dimension `{symbol}` is unbound"))),
        DimExpr::Affine {
            coefficient,
            symbol,
            constant,
        } => {
            let symbol = dimensions
                .get(symbol)
                .copied()
                .ok_or_else(|| mismatch(format!("symbolic dimension `{symbol}` is unbound")))?;
            let value = i128::from(*coefficient)
                .checked_mul(i128::try_from(symbol).unwrap_or(i128::MAX))
                .and_then(|value| value.checked_add(i128::from(*constant)))
                .ok_or_else(|| mismatch("affine evaluation dimension overflow"))?;
            if value < 0 {
                return Err(mismatch("affine evaluation dimension is negative"));
            }
            u64::try_from(value).map_err(|_| mismatch("evaluation dimension exceeds u64"))
        }
    }
}

fn preflight_evaluation(
    program: &Program,
    dimensions: &BTreeMap<String, usize>,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    for constraint in &program.constraints {
        match constraint {
            agentir_core::shapes::ShapeConstraint::Equal { left, right } => {
                if left.0.len() != right.0.len() {
                    return Err(mismatch("runtime constraint ranks differ"));
                }
                for (left, right) in left.0.iter().zip(&right.0) {
                    let left = concrete_dimension(left, dimensions)?;
                    let right = concrete_dimension(right, dimensions)?;
                    if left != right {
                        return Err(
                            mismatch("runtime inputs violate an accepted shape equality")
                                .with_types(left, right),
                        );
                    }
                }
            }
            agentir_core::shapes::ShapeConstraint::NonNegative { .. } => {}
        }
    }
    let mut total = 0_u64;
    for definition in program.values.values() {
        let Type::Tensor { shape, .. } = &definition.ty else {
            continue;
        };
        let elements = shape.0.iter().try_fold(1_u64, |count, dimension| {
            count
                .checked_mul(concrete_dimension(dimension, dimensions)?)
                .ok_or_else(|| mismatch("evaluation tensor element count overflow"))
        })?;
        BudgetCheck::against(
            limits,
            ResourceKind::EvaluationTensorElements,
            elements,
            format!("evaluation value `{}`", definition.id),
        )?;
        total = total.saturating_add(elements);
    }
    BudgetCheck::against(
        limits,
        ResourceKind::TotalEvaluationElements,
        total,
        "evaluation graph materialization",
    )
}

fn constant_value(value: &ConstantValue) -> AgentResult<RuntimeValue> {
    let scalar = match value {
        ConstantValue::Bool { value } => ScalarValue::Bool(*value),
        ConstantValue::I32 { value } => ScalarValue::I32(*value),
        ConstantValue::F32 { .. } => ScalarValue::F32(value.as_f32().ok_or_else(|| {
            AgentError::new(ErrorCode::TransactionRejected, "invalid canonical f32 bits")
        })?),
    };
    Ok(RuntimeValue::Scalar(scalar))
}

fn scalar_binary(
    opcode: Opcode,
    left: &ScalarValue,
    right: &ScalarValue,
) -> AgentResult<ScalarValue> {
    match (opcode, left, right) {
        (Opcode::Add, ScalarValue::I32(left), ScalarValue::I32(right)) => left
            .checked_add(*right)
            .map(ScalarValue::I32)
            .ok_or_else(|| {
                AgentError::new(ErrorCode::TransactionRejected, "i32 addition overflow")
            }),
        (Opcode::Sub, ScalarValue::I32(left), ScalarValue::I32(right)) => left
            .checked_sub(*right)
            .map(ScalarValue::I32)
            .ok_or_else(|| {
                AgentError::new(ErrorCode::TransactionRejected, "i32 subtraction overflow")
            }),
        (Opcode::Mul, ScalarValue::I32(left), ScalarValue::I32(right)) => left
            .checked_mul(*right)
            .map(ScalarValue::I32)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::TransactionRejected,
                    "i32 multiplication overflow",
                )
            }),
        (Opcode::Div, ScalarValue::I32(_), ScalarValue::I32(0))
        | (Opcode::Div, ScalarValue::F32(_), ScalarValue::F32(0.0)) => Err(AgentError::new(
            ErrorCode::DivisionByZero,
            "Stage 1 reference semantics reject division by zero",
        )),
        (Opcode::Div, ScalarValue::I32(left), ScalarValue::I32(right)) => left
            .checked_div(*right)
            .map(ScalarValue::I32)
            .ok_or_else(|| {
                AgentError::new(ErrorCode::TransactionRejected, "i32 division overflow")
            }),
        (Opcode::Add, ScalarValue::F32(left), ScalarValue::F32(right)) => {
            Ok(ScalarValue::F32(left + right))
        }
        (Opcode::Sub, ScalarValue::F32(left), ScalarValue::F32(right)) => {
            Ok(ScalarValue::F32(left - right))
        }
        (Opcode::Mul, ScalarValue::F32(left), ScalarValue::F32(right)) => {
            Ok(ScalarValue::F32(left * right))
        }
        (Opcode::Div, ScalarValue::F32(left), ScalarValue::F32(right)) => {
            Ok(ScalarValue::F32(left / right))
        }
        _ => Err(AgentError::new(
            ErrorCode::TypeMismatch,
            format!("cannot apply `{opcode}` to runtime scalar values"),
        )),
    }
}

fn elementwise_binary(
    opcode: Opcode,
    left: &RuntimeValue,
    right: &RuntimeValue,
) -> AgentResult<RuntimeValue> {
    match (left, right) {
        (RuntimeValue::Scalar(left), RuntimeValue::Scalar(right)) => {
            scalar_binary(opcode, left, right).map(RuntimeValue::Scalar)
        }
        (RuntimeValue::Tensor(left), RuntimeValue::Tensor(right))
            if left.shape == right.shape && left.element_type == right.element_type =>
        {
            let elements = left
                .elements
                .iter()
                .zip(&right.elements)
                .map(|(left, right)| scalar_binary(opcode, left, right))
                .collect::<AgentResult<Vec<_>>>()?;
            Ok(RuntimeValue::Tensor(DenseTensor {
                element_type: left.element_type,
                shape: left.shape.clone(),
                elements,
            }))
        }
        _ => Err(AgentError::new(
            ErrorCode::TypeMismatch,
            "runtime elementwise operands differ",
        )),
    }
}

fn scalar_fma(
    left: &ScalarValue,
    right: &ScalarValue,
    addend: &ScalarValue,
) -> AgentResult<ScalarValue> {
    match (left, right, addend) {
        (ScalarValue::F32(left), ScalarValue::F32(right), ScalarValue::F32(addend)) => {
            Ok(ScalarValue::F32(left.mul_add(*right, *addend)))
        }
        (ScalarValue::I32(left), ScalarValue::I32(right), ScalarValue::I32(addend)) => left
            .checked_mul(*right)
            .and_then(|value| value.checked_add(*addend))
            .map(ScalarValue::I32)
            .ok_or_else(|| AgentError::new(ErrorCode::TransactionRejected, "i32 fma overflow")),
        _ => Err(AgentError::new(
            ErrorCode::TypeMismatch,
            "fma runtime types differ",
        )),
    }
}

#[allow(clippy::float_cmp)] // `compare` follows exact IEEE predicate semantics.
fn compare_scalar(
    predicate: &str,
    left: &ScalarValue,
    right: &ScalarValue,
) -> AgentResult<ScalarValue> {
    let result = match (left, right) {
        (ScalarValue::Bool(left), ScalarValue::Bool(right)) => match predicate {
            "eq" => left == right,
            "ne" => left != right,
            _ => {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "bool comparison supports eq/ne",
                ));
            }
        },
        (ScalarValue::I32(left), ScalarValue::I32(right)) => match predicate {
            "eq" => left == right,
            "ne" => left != right,
            "lt" => left < right,
            "le" => left <= right,
            "gt" => left > right,
            "ge" => left >= right,
            _ => {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "unknown comparison predicate",
                ));
            }
        },
        (ScalarValue::F32(left), ScalarValue::F32(right)) => match predicate {
            "eq" => left == right,
            "ne" => left != right,
            "lt" => left < right,
            "le" => left <= right,
            "gt" => left > right,
            "ge" => left >= right,
            _ => {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "unknown comparison predicate",
                ));
            }
        },
        _ => {
            return Err(AgentError::new(
                ErrorCode::TypeMismatch,
                "comparison runtime types differ",
            ));
        }
    };
    Ok(ScalarValue::Bool(result))
}

fn cast_scalar(value: &ScalarValue, target: ScalarType) -> AgentResult<ScalarValue> {
    match (value, target) {
        (ScalarValue::Bool(value), ScalarType::Bool) => Ok(ScalarValue::Bool(*value)),
        (ScalarValue::I32(value), ScalarType::I32 | ScalarType::Index) => {
            Ok(ScalarValue::I32(*value))
        }
        (ScalarValue::F32(value), ScalarType::F32) => Ok(ScalarValue::F32(*value)),
        (ScalarValue::I32(value), ScalarType::F32) => Ok(ScalarValue::F32(*value as f32)),
        (ScalarValue::F32(value), ScalarType::I32 | ScalarType::Index) => {
            if value.is_finite() && *value >= i32::MIN as f32 && *value <= i32::MAX as f32 {
                Ok(ScalarValue::I32(*value as i32))
            } else {
                Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "f32 value is out of i32 cast range",
                ))
            }
        }
        _ => Err(AgentError::new(
            ErrorCode::TypeMismatch,
            "unsupported explicit cast",
        )),
    }
}

fn eval_primitive(
    opcode: Opcode,
    operands: &[RuntimeValue],
    attributes: &BTreeMap<String, JsonValue>,
) -> AgentResult<RuntimeValue> {
    match opcode {
        Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div => {
            elementwise_binary(opcode, &operands[0], &operands[1])
        }
        Opcode::Fma => match (&operands[0], &operands[1], &operands[2]) {
            (
                RuntimeValue::Scalar(left),
                RuntimeValue::Scalar(right),
                RuntimeValue::Scalar(addend),
            ) => scalar_fma(left, right, addend).map(RuntimeValue::Scalar),
            (
                RuntimeValue::Tensor(left),
                RuntimeValue::Tensor(right),
                RuntimeValue::Tensor(addend),
            ) if left.shape == right.shape && left.shape == addend.shape => {
                let elements = left
                    .elements
                    .iter()
                    .zip(&right.elements)
                    .zip(&addend.elements)
                    .map(|((left, right), addend)| scalar_fma(left, right, addend))
                    .collect::<AgentResult<Vec<_>>>()?;
                Ok(RuntimeValue::Tensor(DenseTensor {
                    element_type: left.element_type,
                    shape: left.shape.clone(),
                    elements,
                }))
            }
            _ => Err(AgentError::new(
                ErrorCode::TypeMismatch,
                "runtime fma operands differ",
            )),
        },
        Opcode::Compare => {
            let predicate = attributes
                .get("predicate")
                .and_then(JsonValue::as_str)
                .unwrap_or("eq");
            match (&operands[0], &operands[1]) {
                (RuntimeValue::Scalar(left), RuntimeValue::Scalar(right)) => {
                    compare_scalar(predicate, left, right).map(RuntimeValue::Scalar)
                }
                (RuntimeValue::Tensor(left), RuntimeValue::Tensor(right))
                    if left.shape == right.shape =>
                {
                    let elements = left
                        .elements
                        .iter()
                        .zip(&right.elements)
                        .map(|(left, right)| compare_scalar(predicate, left, right))
                        .collect::<AgentResult<Vec<_>>>()?;
                    Ok(RuntimeValue::Tensor(DenseTensor {
                        element_type: ScalarType::Bool,
                        shape: left.shape.clone(),
                        elements,
                    }))
                }
                _ => Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "runtime compare operands differ",
                )),
            }
        }
        Opcode::Select => match (&operands[0], &operands[1], &operands[2]) {
            (RuntimeValue::Scalar(ScalarValue::Bool(condition)), yes, no) => {
                Ok(if *condition { yes.clone() } else { no.clone() })
            }
            (
                RuntimeValue::Tensor(condition),
                RuntimeValue::Tensor(yes),
                RuntimeValue::Tensor(no),
            ) if condition.shape == yes.shape && yes.shape == no.shape => {
                let elements = condition
                    .elements
                    .iter()
                    .zip(&yes.elements)
                    .zip(&no.elements)
                    .map(|((condition, yes), no)| match condition {
                        ScalarValue::Bool(value) => {
                            Ok(if *value { yes.clone() } else { no.clone() })
                        }
                        _ => Err(AgentError::new(
                            ErrorCode::TypeMismatch,
                            "select condition is not bool",
                        )),
                    })
                    .collect::<AgentResult<Vec<_>>>()?;
                Ok(RuntimeValue::Tensor(DenseTensor {
                    element_type: yes.element_type,
                    shape: yes.shape.clone(),
                    elements,
                }))
            }
            _ => Err(AgentError::new(
                ErrorCode::TypeMismatch,
                "runtime select operands differ",
            )),
        },
        Opcode::Cast => {
            let target = attributes
                .get("target_type")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    AgentError::new(ErrorCode::TypeMismatch, "cast target_type missing")
                })?
                .parse::<ScalarType>()
                .map_err(|message| AgentError::new(ErrorCode::TypeMismatch, message))?;
            match &operands[0] {
                RuntimeValue::Scalar(value) => cast_scalar(value, target).map(RuntimeValue::Scalar),
                RuntimeValue::Tensor(tensor) => {
                    let elements = tensor
                        .elements
                        .iter()
                        .map(|value| cast_scalar(value, target))
                        .collect::<AgentResult<Vec<_>>>()?;
                    Ok(RuntimeValue::Tensor(DenseTensor {
                        element_type: target,
                        shape: tensor.shape.clone(),
                        elements,
                    }))
                }
            }
        }
        _ => Err(AgentError::new(
            ErrorCode::UnknownOpcode,
            format!("`{opcode}` is not a primitive evaluator opcode"),
        )),
    }
}

struct Evaluator<'a> {
    program: &'a Program,
    parameters: BTreeMap<ValueId, RuntimeValue>,
    memo: BTreeMap<ValueId, RuntimeValue>,
    visiting: BTreeSet<ValueId>,
}

impl Evaluator<'_> {
    fn value(&mut self, id: &ValueId) -> AgentResult<RuntimeValue> {
        let id = self.program.resolve_filled_value(id).clone();
        if let Some(value) = self.memo.get(&id).or_else(|| self.parameters.get(&id)) {
            return Ok(value.clone());
        }
        if !self.visiting.insert(id.clone()) {
            return Err(AgentError::new(
                ErrorCode::TransactionRejected,
                format!("cycle detected while evaluating `{id}`"),
            ));
        }
        let definition = self.program.values.get(&id).ok_or_else(|| {
            AgentError::new(
                ErrorCode::UnknownReference,
                format!("value `{id}` is absent"),
            )
        })?;
        let result = match &definition.origin {
            ValueOrigin::Hole(hole) => {
                return Err(AgentError::new(
                    ErrorCode::OpenHole,
                    format!("hole `{hole}` is open"),
                ));
            }
            ValueOrigin::Operation(operation) => {
                let operation = self.program.operations.get(operation).ok_or_else(|| {
                    AgentError::new(ErrorCode::UnknownReference, "defining operation is absent")
                })?;
                match operation.opcode {
                    Opcode::Parameter => self.parameters.get(&id).cloned().ok_or_else(|| {
                        mismatch(format!("input for parameter value `{id}` is missing"))
                    })?,
                    Opcode::Constant => {
                        constant_value(self.program.constants.get(&id).ok_or_else(|| {
                            AgentError::new(
                                ErrorCode::UnknownReference,
                                "constant payload is absent",
                            )
                        })?)?
                    }
                    _ => self.operation(operation)?,
                }
            }
        };
        self.visiting.remove(&id);
        self.memo.insert(id, result.clone());
        Ok(result)
    }

    fn region(&mut self, region: &Region, arguments: &[ScalarValue]) -> AgentResult<ScalarValue> {
        let arguments: BTreeMap<_, _> = region
            .arguments
            .iter()
            .zip(arguments)
            .map(|(argument, value)| (argument.name.clone(), value.clone()))
            .collect();
        let mut locals = BTreeMap::<String, ScalarValue>::new();
        let mut captures = BTreeMap::new();
        for capture in &region.captures {
            let RuntimeValue::Scalar(value) = self.value(capture)? else {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "Stage 1 region capture must be scalar",
                ));
            };
            captures.insert(capture.clone(), value);
        }
        let resolve = |reference: &RegionValue,
                       locals: &BTreeMap<String, ScalarValue>|
         -> AgentResult<ScalarValue> {
            match reference {
                RegionValue::Argument(name) => arguments.get(name).cloned(),
                RegionValue::Local(name) => locals.get(name).cloned(),
                RegionValue::Capture(id) => captures.get(id).cloned(),
            }
            .ok_or_else(|| AgentError::new(ErrorCode::InvalidRegion, "region value is unavailable"))
        };
        for operation in &region.operations {
            let operands = operation
                .operands
                .iter()
                .map(|operand| resolve(operand, &locals).map(RuntimeValue::Scalar))
                .collect::<AgentResult<Vec<_>>>()?;
            let RuntimeValue::Scalar(result) =
                eval_primitive(operation.opcode, &operands, &operation.attributes)?
            else {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "region operation yielded a tensor",
                ));
            };
            locals.insert(operation.result.clone(), result);
        }
        resolve(&region.yield_value, &locals)
    }

    fn operation(&mut self, operation: &Operation) -> AgentResult<RuntimeValue> {
        let operands = operation
            .operands
            .iter()
            .map(|operand| self.value(operand))
            .collect::<AgentResult<Vec<_>>>()?;
        match operation.opcode {
            Opcode::Map => {
                let RuntimeValue::Tensor(input) = &operands[0] else {
                    return Err(AgentError::new(
                        ErrorCode::TypeMismatch,
                        "map input is not a tensor",
                    ));
                };
                let region = operation.region.as_ref().ok_or_else(|| {
                    AgentError::new(ErrorCode::InvalidRegion, "map region is absent")
                })?;
                let elements = input
                    .elements
                    .iter()
                    .map(|element| self.region(region, std::slice::from_ref(element)))
                    .collect::<AgentResult<Vec<_>>>()?;
                Ok(RuntimeValue::Tensor(DenseTensor {
                    element_type: region.yield_type.element_type(),
                    shape: input.shape.clone(),
                    elements,
                }))
            }
            Opcode::ZipMap => {
                let tensors = operands
                    .iter()
                    .map(|operand| match operand {
                        RuntimeValue::Tensor(tensor) => Ok(tensor),
                        RuntimeValue::Scalar(_) => Err(AgentError::new(
                            ErrorCode::TypeMismatch,
                            "zip_map input is not a tensor",
                        )),
                    })
                    .collect::<AgentResult<Vec<_>>>()?;
                let region = operation.region.as_ref().ok_or_else(|| {
                    AgentError::new(ErrorCode::InvalidRegion, "zip_map region is absent")
                })?;
                let length = tensors.first().map_or(0, |tensor| tensor.elements.len());
                if tensors.iter().any(|tensor| {
                    tensor.shape != tensors[0].shape || tensor.elements.len() != length
                }) {
                    return Err(AgentError::new(
                        ErrorCode::TypeMismatch,
                        "zip_map runtime tensor shapes differ",
                    ));
                }
                let mut elements = Vec::with_capacity(length);
                for index in 0..length {
                    let arguments = tensors
                        .iter()
                        .map(|tensor| tensor.elements[index].clone())
                        .collect::<Vec<_>>();
                    elements.push(self.region(region, &arguments)?);
                }
                Ok(RuntimeValue::Tensor(DenseTensor {
                    element_type: region.yield_type.element_type(),
                    shape: tensors[0].shape.clone(),
                    elements,
                }))
            }
            Opcode::Reduce => {
                let RuntimeValue::Tensor(input) = &operands[0] else {
                    return Err(AgentError::new(
                        ErrorCode::TypeMismatch,
                        "reduce input is not a tensor",
                    ));
                };
                let RuntimeValue::Scalar(mut accumulator) = operands[1].clone() else {
                    return Err(AgentError::new(
                        ErrorCode::TypeMismatch,
                        "reduce identity is not scalar",
                    ));
                };
                let region = operation.region.as_ref().ok_or_else(|| {
                    AgentError::new(ErrorCode::InvalidRegion, "reduce region is absent")
                })?;
                for element in &input.elements {
                    accumulator = self.region(region, &[accumulator, element.clone()])?;
                }
                Ok(RuntimeValue::Scalar(accumulator))
            }
            _ => eval_primitive(operation.opcode, &operands, &operation.attributes),
        }
    }
}

fn scalar_json(value: &ScalarValue) -> AgentResult<JsonValue> {
    match value {
        ScalarValue::Bool(value) => Ok(JsonValue::Bool(*value)),
        ScalarValue::I32(value) => Ok(json!(value)),
        ScalarValue::F32(value) => Number::from_f64(f64::from(*value))
            .map(JsonValue::Number)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::TransactionRejected,
                    "non-finite f32 cannot be encoded as JSON",
                )
            }),
    }
}

fn tensor_json(tensor: &DenseTensor) -> AgentResult<JsonValue> {
    fn build(
        shape: &[usize],
        elements: &[ScalarValue],
        cursor: &mut usize,
    ) -> AgentResult<JsonValue> {
        if shape.is_empty() {
            let value = elements.get(*cursor).ok_or_else(|| {
                AgentError::new(
                    ErrorCode::TransactionRejected,
                    "tensor shape exceeds available runtime elements",
                )
            })?;
            let value = scalar_json(value)?;
            *cursor += 1;
            return Ok(value);
        }
        (0..shape[0])
            .map(|_| build(&shape[1..], elements, cursor))
            .collect::<AgentResult<Vec<_>>>()
            .map(JsonValue::Array)
    }
    build(&tensor.shape, &tensor.elements, &mut 0)
}

fn runtime_json(value: &RuntimeValue) -> AgentResult<JsonValue> {
    match value {
        RuntimeValue::Scalar(value) => scalar_json(value),
        RuntimeValue::Tensor(tensor) => tensor_json(tensor),
    }
}

/// Evaluates a frozen, complete SpecIR program using strict CPU semantics.
pub fn evaluate(
    program: &Program,
    inputs: &BTreeMap<String, JsonValue>,
) -> AgentResult<EvaluationResult> {
    evaluate_with_limits(program, inputs, &ResourceLimits::default())
}

/// Evaluates using explicit resource limits that are not part of program semantics.
pub fn evaluate_with_limits(
    program: &Program,
    inputs: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
) -> AgentResult<EvaluationResult> {
    let (mut evaluator, dimensions) = prepare_evaluator(program, inputs, limits)?;
    let outputs = program
        .outputs
        .iter()
        .map(|(name, value)| {
            let runtime = evaluator.value(value)?;
            runtime_json(&runtime).map(|value| (name.clone(), value))
        })
        .collect::<AgentResult<BTreeMap<_, _>>>()?;
    Ok(EvaluationResult {
        outputs,
        dimensions,
    })
}

fn prepare_evaluator<'a>(
    program: &'a Program,
    inputs: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
) -> AgentResult<(Evaluator<'a>, BTreeMap<String, usize>)> {
    let open_holes: Vec<_> = program
        .holes
        .iter()
        .filter(|(_, hole)| hole.filled_with.is_none())
        .map(|(id, _)| id.to_string())
        .collect();
    if !open_holes.is_empty() {
        return Err(
            AgentError::new(ErrorCode::OpenHole, "program contains open holes")
                .with_detail("holes", json!(open_holes)),
        );
    }
    if !program.frozen {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "program must be complete and frozen before evaluation",
        ));
    }
    let expected_names: BTreeSet<_> = program.parameters.keys().cloned().collect();
    let actual_names: BTreeSet<_> = inputs.keys().cloned().collect();
    if expected_names != actual_names {
        return Err(
            mismatch("input names do not exactly match program parameters")
                .with_types(json!(expected_names), json!(actual_names)),
        );
    }
    let mut dimensions = BTreeMap::new();
    let mut parameters = BTreeMap::new();
    for (name, value_id) in &program.parameters {
        let definition = program.values.get(value_id).ok_or_else(|| {
            AgentError::new(ErrorCode::UnknownReference, "parameter value is absent")
        })?;
        let value = parse_input(
            &definition.ty,
            inputs.get(name).expect("input names were compared"),
            &mut dimensions,
            limits,
        )
        .map_err(|error| {
            error
                .with_detail("parameter", name.clone())
                .with_detail("expected_type", definition.ty.to_string())
        })?;
        parameters.insert(value_id.clone(), value);
    }
    preflight_evaluation(program, &dimensions, limits)?;
    Ok((
        Evaluator {
            program,
            parameters,
            memo: BTreeMap::new(),
            visiting: BTreeSet::new(),
        },
        dimensions,
    ))
}

/// Evaluates a verified separate ImplIR graph using the same strict reference semantics.
pub fn evaluate_impl_with_limits(
    program: &ImplProgram,
    inputs: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
) -> AgentResult<EvaluationResult> {
    evaluate_with_limits(&impl_as_program(program), inputs, limits)
}

fn memory_trace_push(
    trace: &mut Vec<MemoryTraceEvent>,
    kind: &str,
    buffer: Option<BufferId>,
    detail: impl Into<String>,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    let attempted = u64::try_from(trace.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryTraceEvents,
        attempted,
        "MemoryIR evaluation trace",
    )?;
    trace.push(MemoryTraceEvent {
        sequence: attempted - 1,
        kind: kind.to_owned(),
        buffer,
        detail: detail.into(),
    });
    let bytes = serde_json::to_vec(trace).map_err(|error| {
        AgentError::new(
            ErrorCode::PersistenceFormat,
            format!("MemoryIR trace encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryTraceBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "MemoryIR evaluation trace",
    )
}

#[derive(Clone, Copy, Debug)]
struct AbstractBufferState {
    initialized: bool,
    released: bool,
}

fn memory_element_bytes(element: ScalarType) -> u64 {
    match element {
        ScalarType::Bool => 1,
        ScalarType::I32 | ScalarType::F32 => 4,
        ScalarType::Index => 8,
    }
}

fn allocate_abstract_buffer(
    buffer: &MemoryBuffer,
    dimensions: &BTreeMap<String, usize>,
    states: &mut BTreeMap<BufferId, AbstractBufferState>,
    total_bytes: &mut u64,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    if buffer.alignment == 0
        || !buffer.alignment.is_power_of_two()
        || buffer.alignment < memory_element_bytes(buffer.element_type)
    {
        return Err(AgentError::new(
            ErrorCode::AlignmentUnsatisfied,
            "abstract memory allocation has an invalid alignment",
        )
        .with_detail("buffer", buffer.id.to_string())
        .with_detail("alignment", buffer.alignment));
    }
    let elements = buffer.shape.0.iter().try_fold(1_u64, |total, dimension| {
        let extent = concrete_dimension(dimension, dimensions)?;
        if extent == 0 {
            return Err(AgentError::new(
                ErrorCode::InvalidMemoryLayout,
                "zero-sized runtime buffer allocation is unsupported",
            )
            .with_detail("buffer", buffer.id.to_string()));
        }
        total.checked_mul(extent).ok_or_else(|| {
            AgentError::new(
                ErrorCode::MemoryResourceLimit,
                "runtime buffer element count overflowed u64",
            )
            .with_detail("buffer", buffer.id.to_string())
        })
    })?;
    let bytes = elements
        .checked_mul(memory_element_bytes(buffer.element_type))
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::MemoryResourceLimit,
                "runtime buffer allocation byte count overflowed u64",
            )
            .with_detail("buffer", buffer.id.to_string())
        })?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryAllocationBytesPerBuffer,
        bytes,
        format!("MemoryIR runtime buffer `{}`", buffer.id),
    )?;
    *total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
        AgentError::new(
            ErrorCode::MemoryResourceLimit,
            "total runtime abstract allocation bytes overflowed u64",
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryTotalAllocationBytes,
        *total_bytes,
        "MemoryIR runtime abstract allocation",
    )?;
    states.insert(
        buffer.id.clone(),
        AbstractBufferState {
            initialized: matches!(
                buffer.ownership,
                Ownership::ExternalBorrowed | Ownership::ConstantOwned
            ),
            released: false,
        },
    );
    Ok(())
}

fn execute_abstract_access(
    buffer: &MemoryBuffer,
    kind: BufferAccessKind,
    states: &mut BTreeMap<BufferId, AbstractBufferState>,
) -> AgentResult<()> {
    let state = states.get_mut(&buffer.id).ok_or_else(|| {
        AgentError::new(
            ErrorCode::InvalidMemoryAccess,
            "memory access references an unallocated abstract buffer",
        )
        .with_detail("buffer", buffer.id.to_string())
    })?;
    if state.released {
        return Err(AgentError::new(
            ErrorCode::LifetimeViolation,
            "memory access occurs after abstract buffer release",
        )
        .with_detail("buffer", buffer.id.to_string()));
    }
    let reads = matches!(kind, BufferAccessKind::Read | BufferAccessKind::ReadWrite);
    let writes = matches!(kind, BufferAccessKind::Write | BufferAccessKind::ReadWrite);
    if reads && !matches!(buffer.access, AccessMode::ReadOnly | AccessMode::ReadWrite) {
        return Err(AgentError::new(
            ErrorCode::InvalidMemoryAccess,
            "read is forbidden by the abstract buffer access mode",
        )
        .with_detail("buffer", buffer.id.to_string()));
    }
    if writes && !matches!(buffer.access, AccessMode::WriteOnly | AccessMode::ReadWrite) {
        return Err(AgentError::new(
            ErrorCode::InvalidMemoryAccess,
            "write is forbidden by the abstract buffer access mode",
        )
        .with_detail("buffer", buffer.id.to_string()));
    }
    if reads && !state.initialized {
        return Err(AgentError::new(
            ErrorCode::InvalidMemoryAccess,
            "read observes an uninitialized abstract buffer",
        )
        .with_detail("buffer", buffer.id.to_string()));
    }
    if writes {
        state.initialized = true;
    }
    Ok(())
}

/// Evaluates one verified exact MemoryIR revision and emits a deterministic high-level trace.
///
/// Guard outcomes may be supplied by the caller's runtime binding contract. Missing outcomes are
/// true only for a statically proved `NoAlias` relation and false otherwise, selecting the lazy
/// exact fresh fallback without speculatively executing the reuse branch.
pub fn evaluate_memory_with_limits(
    revision: &MemoryRevision,
    implementation: &ImplProgram,
    inputs: &BTreeMap<String, JsonValue>,
    requested_guard_outcomes: &BTreeMap<MemoryGuardId, bool>,
    limits: &ResourceLimits,
) -> AgentResult<MemoryEvaluationResult> {
    if matches!(
        revision.status,
        MemoryStatus::Draft | MemoryStatus::WellTyped | MemoryStatus::Rejected
    ) {
        return Err(AgentError::new(
            ErrorCode::MemoryEquivalenceUnproved,
            "MemoryIR evaluation requires a proved, guarded, or sealed revision",
        ));
    }
    verify_memory_program(&revision.program, implementation, limits)?;
    let evaluation = evaluate_impl_with_limits(implementation, inputs, limits)?;
    let output_elements = evaluation.outputs.values().fold(0_u64, |total, value| {
        fn leaves(value: &JsonValue) -> u64 {
            value.as_array().map_or(1, |values| {
                values
                    .iter()
                    .fold(0_u64, |sum, value| sum.saturating_add(leaves(value)))
            })
        }
        total.saturating_add(leaves(value))
    });
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryEvaluationElements,
        output_elements,
        "MemoryIR reference evaluation",
    )?;

    let mut trace = Vec::new();
    let mut abstract_buffers = BTreeMap::new();
    let mut total_allocation_bytes = 0_u64;
    let fallback_buffers: BTreeSet<_> = revision
        .program
        .reuse_decisions
        .values()
        .filter_map(|decision| match decision {
            ReuseDecision::Guarded { fallback, .. } => Some(fallback.fresh_buffer.id.clone()),
            ReuseDecision::Fresh { .. } | ReuseDecision::InPlace { .. } => None,
        })
        .collect();
    for buffer in revision.program.buffers.values() {
        if fallback_buffers.contains(&buffer.id) {
            continue;
        }
        allocate_abstract_buffer(
            buffer,
            &evaluation.dimensions,
            &mut abstract_buffers,
            &mut total_allocation_bytes,
            limits,
        )?;
        let kind = match buffer.ownership {
            Ownership::ExternalBorrowed => "bind_external",
            Ownership::ConstantOwned => "bind_constant",
            Ownership::PlanOwned | Ownership::View => "allocate",
        };
        memory_trace_push(
            &mut trace,
            kind,
            Some(buffer.id.clone()),
            format!("{} {}", buffer.element_type, buffer.shape),
            limits,
        )?;
    }

    let mut guard_outcomes = BTreeMap::new();
    let mut fallback_results = BTreeMap::new();
    for (result, decision) in &revision.program.reuse_decisions {
        match decision {
            ReuseDecision::Fresh { buffer } => memory_trace_push(
                &mut trace,
                "fresh",
                Some(buffer.clone()),
                format!("result {result}"),
                limits,
            )?,
            ReuseDecision::InPlace { input, buffer, .. } => memory_trace_push(
                &mut trace,
                "reuse",
                Some(buffer.clone()),
                format!("input {input} -> result {result}"),
                limits,
            )?,
            ReuseDecision::Guarded {
                input,
                buffer,
                guard,
                fallback,
                ..
            } => {
                let statically_no_alias = revision.program.alias_facts.iter().any(|fact| {
                    ((fact.first == guard.primary_buffer && fact.second == guard.other_buffer)
                        || (fact.second == guard.primary_buffer
                            && fact.first == guard.other_buffer))
                        && fact.relation == AliasRelation::NoAlias
                });
                let outcome = requested_guard_outcomes
                    .get(&guard.id)
                    .copied()
                    .unwrap_or(statically_no_alias);
                guard_outcomes.insert(guard.id.clone(), outcome);
                memory_trace_push(
                    &mut trace,
                    "guard",
                    Some(buffer.clone()),
                    format!("{}={outcome}", guard.id),
                    limits,
                )?;
                if outcome {
                    memory_trace_push(
                        &mut trace,
                        "guarded_reuse",
                        Some(buffer.clone()),
                        format!("input {input} -> result {result}"),
                        limits,
                    )?;
                } else {
                    fallback_results.insert(result.clone(), fallback.fresh_buffer.id.clone());
                    allocate_abstract_buffer(
                        &fallback.fresh_buffer,
                        &evaluation.dimensions,
                        &mut abstract_buffers,
                        &mut total_allocation_bytes,
                        limits,
                    )?;
                    memory_trace_push(
                        &mut trace,
                        "fallback_allocate",
                        Some(fallback.fresh_buffer.id.clone()),
                        format!("lazy exact fallback for result {result}"),
                        limits,
                    )?;
                }
            }
        }
    }
    for operation_id in &revision.program.operation_order {
        let operation = &revision.program.operations[operation_id];
        for access in &operation.accesses {
            let fallback_read = operation.results.iter().any(|binding| {
                fallback_results.contains_key(binding.value())
                    && binding.buffer() == Some(&access.buffer)
            });
            let effective_kind = if fallback_read {
                BufferAccessKind::Read
            } else {
                access.kind
            };
            let buffer = &revision.program.buffers[&access.buffer];
            execute_abstract_access(buffer, effective_kind, &mut abstract_buffers)?;
            memory_trace_push(
                &mut trace,
                "access",
                Some(access.buffer.clone()),
                format!(
                    "{} {} {}",
                    operation.id,
                    match effective_kind {
                        BufferAccessKind::Read => "Read",
                        BufferAccessKind::Write => "Write",
                        BufferAccessKind::ReadWrite => "ReadWrite",
                    },
                    access.value
                ),
                limits,
            )?;
        }
        for binding in &operation.results {
            if let Some(buffer) = fallback_results.get(binding.value()) {
                let fallback = &revision.program.buffers[buffer];
                execute_abstract_access(fallback, BufferAccessKind::Write, &mut abstract_buffers)?;
                memory_trace_push(
                    &mut trace,
                    "access",
                    Some(buffer.clone()),
                    format!("{} Write {}", operation.id, binding.value()),
                    limits,
                )?;
            }
        }
        let logical_point = implementation
            .operation_order
            .iter()
            .position(|source| source == &operation.impl_operation)
            .and_then(|index| u64::try_from(index).ok())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        for buffer in revision.program.buffers.values() {
            if buffer.lifetime.deallocation_eligible
                && buffer.lifetime.last_use <= logical_point
                && !fallback_buffers.contains(&buffer.id)
                && abstract_buffers
                    .get(&buffer.id)
                    .is_some_and(|state| !state.released)
            {
                abstract_buffers
                    .get_mut(&buffer.id)
                    .expect("abstract allocation was checked")
                    .released = true;
                memory_trace_push(
                    &mut trace,
                    "release",
                    Some(buffer.id.clone()),
                    format!("after logical point {}", buffer.lifetime.last_use),
                    limits,
                )?;
            }
        }
    }
    for binding in revision.program.outputs.values() {
        if let Some(buffer) = binding.buffer() {
            let selected = fallback_results.get(binding.value()).unwrap_or(buffer);
            let state = abstract_buffers.get(selected).ok_or_else(|| {
                AgentError::new(
                    ErrorCode::InvalidMemoryAccess,
                    "MemoryIR output references an unallocated abstract buffer",
                )
                .with_detail("buffer", selected.to_string())
            })?;
            if state.released || !state.initialized {
                return Err(AgentError::new(
                    ErrorCode::LifetimeViolation,
                    "MemoryIR output buffer is uninitialized or already released",
                )
                .with_detail("buffer", selected.to_string()));
            }
        }
    }
    Ok(MemoryEvaluationResult {
        evaluation,
        guard_outcomes,
        trace_codec_version: MEMORY_TRACE_CODEC_VERSION,
        trace,
    })
}

/// Evaluates only the dependency cone of one ImplIR value.
///
/// Candidate-level guards use this entry point so the primary outputs and
/// fallback are never evaluated eagerly.
pub fn evaluate_impl_value_with_limits(
    program: &ImplProgram,
    value: &ImplValueId,
    inputs: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
) -> AgentResult<JsonValue> {
    let adapter = impl_as_program(program);
    let (mut evaluator, _dimensions) = prepare_evaluator(&adapter, inputs, limits)?;
    runtime_json(&evaluator.value(&ValueId::new(value.as_str()))?)
}

/// Evaluates exact, speculative, or guarded candidate-level semantics.
pub fn evaluate_candidate_with_limits(
    forest: &CandidateForest,
    candidate: &CandidateId,
    revision: &CandidateRevisionId,
    inputs: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
) -> AgentResult<EvaluationResult> {
    fn evaluate_at(
        forest: &CandidateForest,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
        inputs: &BTreeMap<String, JsonValue>,
        limits: &ResourceLimits,
        depth: u64,
        visiting: &mut BTreeSet<(CandidateId, CandidateRevisionId)>,
    ) -> AgentResult<EvaluationResult> {
        BudgetCheck::against(
            limits,
            ResourceKind::FallbackDepth,
            depth,
            "candidate guarded evaluation before recursion",
        )?;
        if !visiting.insert((candidate.clone(), revision.clone())) {
            return Err(AgentError::new(
                ErrorCode::FallbackCycle,
                "candidate guarded fallback cycle detected",
            ));
        }
        let candidate_data = forest.candidates.get(candidate).ok_or_else(|| {
            AgentError::new(
                ErrorCode::CandidateNotFound,
                format!("candidate `{candidate}` does not exist"),
            )
        })?;
        let revision_data = candidate_data.revisions.get(revision).ok_or_else(|| {
            AgentError::new(
                ErrorCode::CandidateRevisionNotFound,
                format!("candidate revision `{revision}` does not exist"),
            )
        })?;
        let result = if let Some(fallback) = &revision_data.guarded_fallback {
            let guard = match &fallback.guard {
                GuardPredicate::I32NonZero { value } => {
                    let value = evaluate_impl_value_with_limits(
                        &revision_data.impl_program,
                        value,
                        inputs,
                        limits,
                    )?;
                    value.as_i64().ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::GuardInvalid,
                            "i32 non-zero guard did not evaluate to an integer",
                        )
                    })? != 0
                }
            };
            if guard {
                evaluate_impl_with_limits(&revision_data.impl_program, inputs, limits)
            } else {
                evaluate_at(
                    forest,
                    &fallback.fallback_candidate,
                    &fallback.fallback_revision,
                    inputs,
                    limits,
                    depth.saturating_add(1),
                    visiting,
                )
            }
        } else {
            evaluate_impl_with_limits(&revision_data.impl_program, inputs, limits)
        };
        visiting.remove(&(candidate.clone(), revision.clone()));
        result
    }
    evaluate_at(
        forest,
        candidate,
        revision,
        inputs,
        limits,
        0,
        &mut BTreeSet::new(),
    )
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

fn solve_dimensions(program: &Program, state: &mut u64) -> AgentResult<BTreeMap<String, usize>> {
    let mut dimensions = program
        .dimension_names
        .keys()
        .map(|name| {
            let value = usize::try_from(next_random(state) % 3 + 1).expect("small extent");
            (name.clone(), value)
        })
        .collect::<BTreeMap<_, _>>();
    for _ in 0..program.constraints.len().saturating_add(1) {
        for constraint in &program.constraints {
            let agentir_core::shapes::ShapeConstraint::Equal { left, right } = constraint else {
                continue;
            };
            for (left, right) in left.0.iter().zip(&right.0) {
                match (left, right) {
                    (DimExpr::Symbol(symbol), DimExpr::Static(value))
                    | (DimExpr::Static(value), DimExpr::Symbol(symbol)) => {
                        dimensions.insert(
                            symbol.clone(),
                            usize::try_from(*value)
                                .map_err(|_| mismatch("constraint static extent exceeds usize"))?,
                        );
                    }
                    (DimExpr::Symbol(left), DimExpr::Symbol(right)) => {
                        let value = dimensions
                            .get(left)
                            .copied()
                            .or_else(|| dimensions.get(right).copied())
                            .unwrap_or(1);
                        dimensions.insert(left.clone(), value);
                        dimensions.insert(right.clone(), value);
                    }
                    (
                        DimExpr::Affine {
                            coefficient,
                            symbol,
                            constant,
                        },
                        DimExpr::Static(value),
                    )
                    | (
                        DimExpr::Static(value),
                        DimExpr::Affine {
                            coefficient,
                            symbol,
                            constant,
                        },
                    ) if *coefficient != 0 => {
                        let numerator = i128::from(*value) - i128::from(*constant);
                        let coefficient = i128::from(*coefficient);
                        if numerator >= 0 && numerator % coefficient == 0 {
                            dimensions.insert(
                                symbol.clone(),
                                usize::try_from(numerator / coefficient).map_err(|_| {
                                    mismatch("constraint affine extent exceeds usize")
                                })?,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(dimensions)
}

fn concrete_shape(ty: &Type, dimensions: &BTreeMap<String, usize>) -> AgentResult<Vec<usize>> {
    let Type::Tensor { shape, .. } = ty else {
        return Ok(Vec::new());
    };
    shape
        .0
        .iter()
        .map(|dimension| {
            usize::try_from(concrete_dimension(dimension, dimensions)?)
                .map_err(|_| mismatch("generated extent exceeds usize"))
        })
        .collect()
}

fn generated_scalar(ty: ScalarType, state: &mut u64) -> JsonValue {
    match ty {
        ScalarType::Bool => JsonValue::Bool(next_random(state) & 1 == 0),
        ScalarType::I32 | ScalarType::Index => {
            json!(i32::try_from(next_random(state) % 5).expect("small value") - 2)
        }
        ScalarType::F32 => {
            let choices = [-0.0_f32, 0.0, 0.5, -1.0, 2.0];
            let index =
                usize::try_from(next_random(state) % choices.len() as u64).expect("bounded index");
            json!(choices[index])
        }
    }
}

fn generated_tensor(element: ScalarType, shape: &[usize], state: &mut u64) -> JsonValue {
    if let Some((extent, rest)) = shape.split_first() {
        JsonValue::Array(
            (0..*extent)
                .map(|_| generated_tensor(element, rest, state))
                .collect(),
        )
    } else {
        generated_scalar(element, state)
    }
}

fn generate_inputs(
    program: &Program,
    state: &mut u64,
    limits: &ResourceLimits,
    accumulated_elements: &mut u64,
) -> AgentResult<BTreeMap<String, JsonValue>> {
    let dimensions = solve_dimensions(program, state)?;
    program
        .parameters
        .iter()
        .map(|(name, value)| {
            let ty = &program
                .values
                .get(value)
                .ok_or_else(|| mismatch("parameter value is missing"))?
                .ty;
            let input = match ty {
                Type::Scalar(scalar) => generated_scalar(*scalar, state),
                Type::Tensor { element, .. } => {
                    let shape = concrete_shape(ty, &dimensions)?;
                    let elements = shape.iter().try_fold(1_u64, |total, extent| {
                        total
                            .checked_mul(u64::try_from(*extent).unwrap_or(u64::MAX))
                            .ok_or_else(|| mismatch("generated tensor element count overflow"))
                    })?;
                    *accumulated_elements = accumulated_elements.saturating_add(elements);
                    BudgetCheck::against(
                        limits,
                        ResourceKind::DifferentialTensorElements,
                        *accumulated_elements,
                        "candidate differential inputs before tensor allocation",
                    )?;
                    generated_tensor(*element, &shape, state)
                }
            };
            Ok((name.clone(), input))
        })
        .collect()
}

fn exact_json(ty: &Type, left: &JsonValue, right: &JsonValue) -> bool {
    match ty {
        Type::Scalar(ScalarType::Bool) => left.as_bool() == right.as_bool(),
        Type::Scalar(ScalarType::I32 | ScalarType::Index) => left.as_i64() == right.as_i64(),
        Type::Scalar(ScalarType::F32) => {
            left.as_f64().map(|value| (value as f32).to_bits())
                == right.as_f64().map(|value| (value as f32).to_bits())
        }
        Type::Tensor { element, .. } => {
            let (Some(left), Some(right)) = (left.as_array(), right.as_array()) else {
                return false;
            };
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| {
                    exact_json(&Type::Scalar(*element), left, right)
                        || exact_json(
                            &Type::Tensor {
                                element: *element,
                                shape: agentir_core::types::Shape(Vec::new()),
                            },
                            left,
                            right,
                        )
                })
        }
    }
}

fn equivalent_results(
    program: &Program,
    left: &EvaluationResult,
    right: &EvaluationResult,
) -> bool {
    left.dimensions == right.dimensions
        && left.outputs.len() == right.outputs.len()
        && program.outputs.iter().all(|(name, value)| {
            let Some(ty) = program.values.get(value).map(|value| &value.ty) else {
                return false;
            };
            left.outputs
                .get(name)
                .zip(right.outputs.get(name))
                .is_some_and(|(left, right)| exact_json(ty, left, right))
        })
}

/// Runs fixed-seed bounded SpecIR/ImplIR differential validation.
///
/// Testing is confidence evidence only; callers must not use this result to prove
/// `EquivalentToSpec`.
pub fn differential_validate(
    spec: &Program,
    implementation: &ImplProgram,
    seed: u64,
    cases: u64,
    limits: &ResourceLimits,
) -> AgentResult<DifferentialValidation> {
    let generated_case_size = u64::try_from(spec.operations.len())
        .unwrap_or(u64::MAX)
        .saturating_add(u64::try_from(implementation.operations.len()).unwrap_or(u64::MAX));
    BudgetCheck::against(
        limits,
        ResourceKind::GeneratedCandidateCaseSize,
        generated_case_size,
        "candidate differential graph case before input generation",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::DifferentialCases,
        cases,
        "candidate differential validation before generation",
    )?;
    if cases == 0 {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "candidate differential validation requires at least one case",
        ));
    }
    let mut state = seed;
    let mut accumulated_elements = 0_u64;
    for case in 0..cases {
        let inputs = generate_inputs(spec, &mut state, limits, &mut accumulated_elements)?;
        let spec_result = evaluate_with_limits(spec, &inputs, limits);
        let impl_result = evaluate_impl_with_limits(implementation, &inputs, limits);
        let matches = match (&spec_result, &impl_result) {
            (Ok(left), Ok(right)) => equivalent_results(spec, left, right),
            (Err(left), Err(right)) => left.code == right.code,
            _ => false,
        };
        if !matches {
            return Ok(DifferentialValidation {
                seed,
                requested_cases: cases,
                executed_cases: case + 1,
                passed: false,
                counterexample: Some(json!({
                    "case": case,
                    "inputs": inputs,
                    "spec_result": spec_result.map_err(|error| error.code),
                    "impl_result": impl_result.map_err(|error| error.code),
                })),
            });
        }
    }
    Ok(DifferentialValidation {
        seed,
        requested_cases: cases,
        executed_cases: cases,
        passed: true,
        counterexample: None,
    })
}

/// Runs fixed-seed differential validation against candidate-level guarded semantics.
///
/// A successful result remains confidence evidence and never discharges proof debt.
pub fn differential_validate_candidate(
    spec: &Program,
    forest: &CandidateForest,
    candidate: &CandidateId,
    revision: &CandidateRevisionId,
    seed: u64,
    cases: u64,
    limits: &ResourceLimits,
) -> AgentResult<DifferentialValidation> {
    let implementation = forest
        .candidates
        .get(candidate)
        .and_then(|candidate| candidate.revisions.get(revision))
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::CandidateRevisionNotFound,
                "candidate differential revision does not exist",
            )
        })?;
    let generated_case_size = u64::try_from(spec.operations.len())
        .unwrap_or(u64::MAX)
        .saturating_add(
            u64::try_from(implementation.impl_program.operations.len()).unwrap_or(u64::MAX),
        );
    BudgetCheck::against(
        limits,
        ResourceKind::GeneratedSpeculativeCaseSize,
        generated_case_size,
        "speculative candidate differential graph case",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::DifferentialCases,
        cases,
        "candidate-level differential validation before generation",
    )?;
    if cases == 0 {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "candidate differential validation requires at least one case",
        ));
    }
    let mut state = seed;
    let mut accumulated_elements = 0_u64;
    for case in 0..cases {
        let mut inputs = generate_inputs(spec, &mut state, limits, &mut accumulated_elements)?;
        if case < 2 {
            if let Some(fallback) = &implementation.guarded_fallback {
                let GuardPredicate::I32NonZero { value } = &fallback.guard;
                if let Some((name, _)) = implementation
                    .impl_program
                    .parameters
                    .iter()
                    .find(|(_, parameter)| *parameter == value)
                {
                    inputs.insert(name.clone(), json!(i32::from(case != 0)));
                }
            }
        }
        let spec_result = evaluate_with_limits(spec, &inputs, limits);
        let candidate_result =
            evaluate_candidate_with_limits(forest, candidate, revision, &inputs, limits);
        let matches = match (&spec_result, &candidate_result) {
            (Ok(left), Ok(right)) => equivalent_results(spec, left, right),
            (Err(left), Err(right)) => left.code == right.code,
            _ => false,
        };
        if !matches {
            return Ok(DifferentialValidation {
                seed,
                requested_cases: cases,
                executed_cases: case + 1,
                passed: false,
                counterexample: Some(json!({
                    "case": case,
                    "inputs": inputs,
                    "spec_result": spec_result.map_err(|error| error.code),
                    "candidate_result": candidate_result.map_err(|error| error.code),
                })),
            });
        }
    }
    Ok(DifferentialValidation {
        seed,
        requested_cases: cases,
        executed_cases: cases,
        passed: true,
        counterexample: None,
    })
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    use agentir_core::{
        ids::{AliasDomainId, ImplValueId},
        memory_ir::{Lifetime, MemoryLayout, MemoryStride, MemoryStrides},
        types::Shape,
    };

    fn buffer(access: AccessMode) -> MemoryBuffer {
        MemoryBuffer {
            id: BufferId::new("buf1"),
            element_type: ScalarType::F32,
            shape: Shape(vec![DimExpr::Static(4)]),
            layout: MemoryLayout::ContiguousRowMajor,
            strides: MemoryStrides {
                entries: vec![MemoryStride::Static { value: 1 }],
            },
            address_space: agentir_core::memory_ir::AddressSpace::Global,
            access,
            alignment: 4,
            alias_domain: AliasDomainId::new("ad1"),
            lifetime: Lifetime {
                first_point: 1,
                uses: vec![2],
                last_use: 2,
                output_escape: false,
                external: false,
                deallocation_eligible: true,
            },
            ownership: Ownership::PlanOwned,
            external_binding: None,
            source_value: ImplValueId::new("iv1"),
            offset_elements: 0,
            provenance: "test".to_owned(),
        }
    }

    #[test]
    fn abstract_memory_machine_rejects_uninitialized_illegal_and_released_accesses() {
        let read_write = buffer(AccessMode::ReadWrite);
        let mut states = BTreeMap::from([(
            read_write.id.clone(),
            AbstractBufferState {
                initialized: false,
                released: false,
            },
        )]);
        assert_eq!(
            execute_abstract_access(&read_write, BufferAccessKind::Read, &mut states)
                .unwrap_err()
                .code,
            ErrorCode::InvalidMemoryAccess
        );

        let read_only = buffer(AccessMode::ReadOnly);
        states.insert(
            read_only.id.clone(),
            AbstractBufferState {
                initialized: true,
                released: false,
            },
        );
        assert_eq!(
            execute_abstract_access(&read_only, BufferAccessKind::Write, &mut states)
                .unwrap_err()
                .code,
            ErrorCode::InvalidMemoryAccess
        );

        states.insert(
            read_write.id.clone(),
            AbstractBufferState {
                initialized: true,
                released: true,
            },
        );
        assert_eq!(
            execute_abstract_access(&read_write, BufferAccessKind::Read, &mut states)
                .unwrap_err()
                .code,
            ErrorCode::LifetimeViolation
        );
    }
}
