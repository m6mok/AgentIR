//! Deterministic CPU reference interpreter for AgentIR SpecIR.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::ValueId,
    ir::{ConstantValue, Opcode, Operation, Program, Region, RegionValue, ValueOrigin},
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
) -> AgentResult<()> {
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
            flatten_json(child, depth + 1, shape, scalars)?;
        }
    } else {
        if array.iter().any(JsonValue::is_array) {
            return Err(mismatch("tensor input mixes scalar and array elements"));
        }
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
) -> AgentResult<RuntimeValue> {
    match ty {
        Type::Scalar(scalar) => parse_scalar(*scalar, value).map(RuntimeValue::Scalar),
        Type::Tensor { element, shape } => {
            let mut concrete_shape = Vec::new();
            let mut scalars = Vec::new();
            flatten_json(value, 0, &mut concrete_shape, &mut scalars)?;
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
            let value = scalar_json(&elements[*cursor])?;
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
        )
        .map_err(|error| {
            error
                .with_detail("parameter", name.clone())
                .with_detail("expected_type", definition.ty.to_string())
        })?;
        parameters.insert(value_id.clone(), value);
    }
    let mut evaluator = Evaluator {
        program,
        parameters,
        memo: BTreeMap::new(),
        visiting: BTreeSet::new(),
    };
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
