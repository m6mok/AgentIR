//! Deterministic ScheduleIR-to-bytecode lowering and safe scalar CPU execution.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    cpu::{
        CpuArtifactAnchor, CpuArtifactDraft, CpuArtifactHash, CpuArtifactPackage, CpuBinding,
        CpuConstant, CpuExtent, CpuFunction, CpuInstruction, CpuOutput, CpuScalarFunction,
        CpuScalarInstruction, CpuScalarOpcode, CpuScalarOperand, CpuValueType, verify_cpu_artifact,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{CpuArtifactId, ImplOperationId, ImplValueId},
    impl_ir::{ImplOperation, ImplProgram, ImplRegion, ImplRegionValue, ImplValueOrigin},
    ir::{ConstantValue, Opcode},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    schedule::{SchedulePlan, ScheduleRevision, ScheduleStatus},
    schedule_ir::BindingLevel,
    target::{CPU_SCALAR_V1, TargetManifest},
    types::{DimExpr, ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn lowering_error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::UnsupportedCpuLowering, message).with_repair(
        "select cpu_scalar_v1, a serial schedule, and the one-dimensional f32 elementwise subset",
    )
}

fn extent(ty: &Type) -> AgentResult<CpuExtent> {
    let Type::Tensor { element, shape } = ty else {
        return Err(lowering_error("CPU tensor lowering expected a tensor type"));
    };
    if *element != ScalarType::F32 || shape.0.len() != 1 {
        return Err(lowering_error(
            "cpu_scalar_v1 supports only one-dimensional f32 tensors",
        ));
    }
    match &shape.0[0] {
        DimExpr::Static(value) => Ok(CpuExtent::Static { value: *value }),
        DimExpr::Symbol(name) => Ok(CpuExtent::Symbol { name: name.clone() }),
        DimExpr::Affine { .. } => Err(lowering_error(
            "cpu_scalar_v1 does not lower affine runtime extents",
        )),
    }
}

fn value_type(ty: &Type) -> AgentResult<CpuValueType> {
    match ty {
        Type::Scalar(ScalarType::F32) => Ok(CpuValueType::F32),
        Type::Tensor {
            element: ScalarType::F32,
            shape,
        } if shape.0.len() == 1 => Ok(CpuValueType::F32Tensor1d),
        _ => Err(lowering_error(
            "cpu_scalar_v1 supports only scalar f32 and one-dimensional f32 tensors",
        )),
    }
}

fn visit_value(
    program: &ImplProgram,
    value: &ImplValueId,
    seen: &mut BTreeSet<ImplOperationId>,
    order: &mut Vec<ImplOperationId>,
) -> AgentResult<()> {
    let definition = program.values.get(value).ok_or_else(|| {
        lowering_error(format!(
            "ImplIR value `{value}` is absent during CPU lowering"
        ))
    })?;
    let ImplValueOrigin::Operation(operation_id) = &definition.origin;
    if seen.contains(operation_id) {
        return Ok(());
    }
    let operation = program
        .operations
        .get(operation_id)
        .ok_or_else(|| lowering_error(format!("ImplIR operation `{operation_id}` is absent")))?;
    for operand in &operation.operands {
        visit_value(program, operand, seen, order)?;
    }
    if let Some(region) = &operation.region {
        for capture in &region.captures {
            visit_value(program, capture, seen, order)?;
        }
    }
    seen.insert(operation_id.clone());
    order.push(operation_id.clone());
    Ok(())
}

fn reachable_order(program: &ImplProgram) -> AgentResult<Vec<ImplOperationId>> {
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for output in program.outputs.values() {
        visit_value(program, &output.value, &mut seen, &mut order)?;
    }
    for parameter in program.parameters.values() {
        visit_value(program, parameter, &mut seen, &mut order)?;
    }
    Ok(order)
}

fn scalar_operand(
    operand: &ImplRegionValue,
    arguments: &BTreeMap<String, u32>,
    locals: &BTreeMap<String, u32>,
    registers: &BTreeMap<ImplValueId, u32>,
) -> AgentResult<CpuScalarOperand> {
    match operand {
        ImplRegionValue::Argument(name) => arguments
            .get(name)
            .copied()
            .map(|index| CpuScalarOperand::Argument { index }),
        ImplRegionValue::Local(name) => locals
            .get(name)
            .copied()
            .map(|register| CpuScalarOperand::Local { register }),
        ImplRegionValue::Capture(value) => registers
            .get(value)
            .copied()
            .map(|register| CpuScalarOperand::Capture { register }),
    }
    .ok_or_else(|| lowering_error("CPU scalar region operand is unavailable"))
}

fn lower_scalar_region(
    region: &ImplRegion,
    registers: &BTreeMap<ImplValueId, u32>,
) -> AgentResult<CpuScalarFunction> {
    if region
        .arguments
        .iter()
        .any(|argument| argument.ty != Type::Scalar(ScalarType::F32))
        || region.yield_type != Type::Scalar(ScalarType::F32)
    {
        return Err(lowering_error(
            "cpu_scalar_v1 elementwise regions must use only scalar f32 values",
        ));
    }
    let arguments = region
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            Ok((
                argument.name.clone(),
                u32::try_from(index).map_err(|_| lowering_error("too many region arguments"))?,
            ))
        })
        .collect::<AgentResult<BTreeMap<_, _>>>()?;
    let mut locals = BTreeMap::new();
    let mut instructions = Vec::new();
    for operation in &region.operations {
        if operation.result_type != Type::Scalar(ScalarType::F32)
            || !operation.attributes.is_empty()
        {
            return Err(lowering_error(
                "cpu_scalar_v1 region operations must be attribute-free scalar f32 arithmetic",
            ));
        }
        let opcode = match operation.opcode {
            Opcode::Add => CpuScalarOpcode::AddF32,
            Opcode::Mul => CpuScalarOpcode::MulF32,
            Opcode::Fma => CpuScalarOpcode::FmaF32,
            _ => {
                return Err(lowering_error(format!(
                    "region opcode `{}` is outside cpu_scalar_v1",
                    operation.opcode
                )));
            }
        };
        let output = u32::try_from(instructions.len())
            .map_err(|_| lowering_error("too many CPU scalar instructions"))?;
        let operands = operation
            .operands
            .iter()
            .map(|operand| scalar_operand(operand, &arguments, &locals, registers))
            .collect::<AgentResult<Vec<_>>>()?;
        instructions.push(CpuScalarInstruction {
            output,
            opcode,
            operands,
        });
        if locals.insert(operation.result.clone(), output).is_some() {
            return Err(lowering_error("duplicate ImplIR scalar local result"));
        }
    }
    let result = scalar_operand(&region.yield_value, &arguments, &locals, registers)?;
    Ok(CpuScalarFunction {
        arguments: u32::try_from(region.arguments.len())
            .map_err(|_| lowering_error("too many CPU scalar arguments"))?,
        instructions,
        result,
    })
}

fn operation_result(operation: &ImplOperation) -> AgentResult<&ImplValueId> {
    if operation.results.len() != 1 || operation.result_types.len() != 1 {
        return Err(lowering_error(
            "cpu_scalar_v1 requires every reachable operation to have one result",
        ));
    }
    Ok(&operation.results[0])
}

fn validate_schedule_subset(
    schedule: &ScheduleRevision,
    target: &TargetManifest,
) -> AgentResult<()> {
    if target.profile != CPU_SCALAR_V1 {
        return Err(lowering_error(
            "CPU artifact emission requires the cpu_scalar_v1 TargetManifest",
        ));
    }
    if !matches!(
        schedule.status,
        ScheduleStatus::ResourceValid | ScheduleStatus::Sealed
    ) || !schedule.program.splits.is_empty()
        || !schedule.program.tiles.is_empty()
        || !schedule.program.fusion_groups.is_empty()
        || !schedule.program.vectorizations.is_empty()
        || !schedule.program.unrolls.is_empty()
        || schedule.program.axes.values().any(|axis| {
            axis.binding
                .as_ref()
                .is_some_and(|binding| binding.level != BindingLevel::Serial)
        })
    {
        return Err(lowering_error(
            "cpu_scalar_v1 requires an untransformed deterministic serial schedule",
        ));
    }
    Ok(())
}

/// Lowers one proved serial schedule into compiler-owned portable scalar bytecode.
pub fn lower_schedule(
    schedule_plan: &SchedulePlan,
    schedule: &ScheduleRevision,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<CpuArtifactDraft> {
    validate_schedule_subset(schedule, target)?;
    if schedule_plan
        .revisions
        .get(&schedule.id)
        .is_none_or(|retained| retained.schedule_hash != schedule.schedule_hash)
    {
        return Err(lowering_error(
            "selected schedule revision is not retained by its plan",
        ));
    }
    let order = reachable_order(implementation)?;
    let parameter_names = implementation
        .parameters
        .iter()
        .map(|(name, value)| (value.clone(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut registers = BTreeMap::new();
    let mut bindings = Vec::new();
    let mut constants = Vec::new();
    let mut instructions = Vec::new();
    for operation_id in order {
        let operation = &implementation.operations[&operation_id];
        let result = operation_result(operation)?.clone();
        let result_type = value_type(&operation.result_types[0])?;
        let register = u32::try_from(registers.len())
            .map_err(|_| lowering_error("too many CPU bytecode registers"))?;
        let encoded = match operation.opcode {
            Opcode::Parameter => {
                let name = parameter_names.get(&result).ok_or_else(|| {
                    lowering_error("reachable CPU parameter lacks an external name")
                })?;
                bindings.push(CpuBinding {
                    name: name.clone(),
                    register,
                    value_type: result_type,
                    extent: operation.result_types[0]
                        .shape()
                        .map(|_| extent(&operation.result_types[0]))
                        .transpose()?,
                });
                None
            }
            Opcode::Constant => {
                let ConstantValue::F32 { bits } =
                    implementation.constants.get(&result).ok_or_else(|| {
                        lowering_error("reachable CPU constant lacks its compiler-owned payload")
                    })?
                else {
                    return Err(lowering_error("cpu_scalar_v1 supports only f32 constants"));
                };
                let index = u32::try_from(constants.len())
                    .map_err(|_| lowering_error("too many CPU constants"))?;
                constants.push(CpuConstant {
                    index,
                    bits: bits.clone(),
                });
                Some(CpuInstruction::ConstantF32 {
                    output: register,
                    constant: index,
                })
            }
            Opcode::Add | Opcode::Mul => {
                if result_type != CpuValueType::F32 {
                    return Err(lowering_error(
                        "cpu_scalar_v1 top-level arithmetic requires scalar f32 operands",
                    ));
                }
                let [lhs, rhs] = operation.operands.as_slice() else {
                    return Err(lowering_error("CPU binary operation arity is invalid"));
                };
                let lhs = *registers
                    .get(lhs)
                    .ok_or_else(|| lowering_error("CPU operand is not ordered before use"))?;
                let rhs = *registers
                    .get(rhs)
                    .ok_or_else(|| lowering_error("CPU operand is not ordered before use"))?;
                Some(if operation.opcode == Opcode::Add {
                    CpuInstruction::AddF32 {
                        output: register,
                        lhs,
                        rhs,
                    }
                } else {
                    CpuInstruction::MulF32 {
                        output: register,
                        lhs,
                        rhs,
                    }
                })
            }
            Opcode::Fma => {
                if result_type != CpuValueType::F32 {
                    return Err(lowering_error(
                        "cpu_scalar_v1 top-level fma requires scalar f32 operands",
                    ));
                }
                let [a, b, c] = operation.operands.as_slice() else {
                    return Err(lowering_error("CPU fma arity is invalid"));
                };
                Some(CpuInstruction::FmaF32 {
                    output: register,
                    a: *registers
                        .get(a)
                        .ok_or_else(|| lowering_error("CPU fma operand is unavailable"))?,
                    b: *registers
                        .get(b)
                        .ok_or_else(|| lowering_error("CPU fma operand is unavailable"))?,
                    c: *registers
                        .get(c)
                        .ok_or_else(|| lowering_error("CPU fma operand is unavailable"))?,
                })
            }
            Opcode::Map => {
                let [input] = operation.operands.as_slice() else {
                    return Err(lowering_error("CPU map arity is invalid"));
                };
                Some(CpuInstruction::MapF32 {
                    output: register,
                    input: *registers
                        .get(input)
                        .ok_or_else(|| lowering_error("CPU map input is unavailable"))?,
                    extent: extent(&operation.result_types[0])?,
                    body: lower_scalar_region(
                        operation
                            .region
                            .as_ref()
                            .ok_or_else(|| lowering_error("CPU map region is absent"))?,
                        &registers,
                    )?,
                })
            }
            Opcode::ZipMap => Some(CpuInstruction::ZipMapF32 {
                output: register,
                inputs: operation
                    .operands
                    .iter()
                    .map(|input| {
                        registers
                            .get(input)
                            .copied()
                            .ok_or_else(|| lowering_error("CPU zip_map input is unavailable"))
                    })
                    .collect::<AgentResult<Vec<_>>>()?,
                extent: extent(&operation.result_types[0])?,
                body: lower_scalar_region(
                    operation
                        .region
                        .as_ref()
                        .ok_or_else(|| lowering_error("CPU zip_map region is absent"))?,
                    &registers,
                )?,
            }),
            _ => {
                return Err(lowering_error(format!(
                    "opcode `{}` is outside cpu_scalar_v1",
                    operation.opcode
                )));
            }
        };
        registers.insert(result, register);
        if let Some(instruction) = encoded {
            instructions.push(instruction);
        }
    }
    bindings.sort_by(|left, right| left.name.cmp(&right.name));
    let outputs = implementation
        .outputs
        .values()
        .map(|output| {
            Ok(CpuOutput {
                name: output.name.clone(),
                register: *registers
                    .get(&output.value)
                    .ok_or_else(|| lowering_error("CPU output is unavailable"))?,
                value_type: value_type(&output.ty)?,
            })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    let anchor = &schedule_plan.anchor;
    Ok(CpuArtifactDraft {
        anchor: CpuArtifactAnchor {
            spec_revision: anchor.spec_revision.clone(),
            spec_hash: anchor.spec_hash.clone(),
            impl_hash: anchor.impl_hash.clone(),
            memory_hash: anchor.memory_hash.clone(),
            memory_plan: anchor.memory_plan.clone(),
            memory_revision: anchor.memory_revision.clone(),
            target_hash: anchor.target_hash.clone(),
            target_manifest: anchor.target_manifest.clone(),
            target_revision: anchor.target_revision.clone(),
            schedule_hash: schedule.schedule_hash.clone(),
            schedule_plan: schedule_plan.id.clone(),
            schedule_revision: schedule.id.clone(),
        },
        bindings,
        constants,
        functions: vec![CpuFunction {
            name: "main".to_owned(),
            register_count: u32::try_from(registers.len())
                .map_err(|_| lowering_error("too many CPU registers"))?,
            instructions,
        }],
        outputs,
    })
}

#[derive(Clone, Debug)]
enum RuntimeValue {
    F32(f32),
    Tensor(Vec<f32>),
}

fn execution_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn f32_number(value: f32) -> AgentResult<Value> {
    Number::from_f64(f64::from(value))
        .map(Value::Number)
        .ok_or_else(|| {
            execution_error(
                ErrorCode::CpuExecutionInputMismatch,
                "CPU execution produced a non-finite f32 output",
            )
        })
}

fn constant_f32(bits: &str) -> AgentResult<f32> {
    let raw = u32::from_str_radix(bits.trim_start_matches("0x"), 16).map_err(|_| {
        execution_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU constant bits are invalid",
        )
    })?;
    Ok(f32::from_bits(raw))
}

fn scalar_source(
    source: &CpuScalarOperand,
    arguments: &[f32],
    locals: &[f32],
    registers: &[Option<RuntimeValue>],
) -> AgentResult<f32> {
    match source {
        CpuScalarOperand::Argument { index } => arguments
            .get(usize::try_from(*index).map_err(|_| {
                execution_error(
                    ErrorCode::CpuExecutionOverflow,
                    "CPU argument index overflow",
                )
            })?)
            .copied(),
        CpuScalarOperand::Local { register } => locals
            .get(usize::try_from(*register).map_err(|_| {
                execution_error(ErrorCode::CpuExecutionOverflow, "CPU local index overflow")
            })?)
            .copied(),
        CpuScalarOperand::Capture { register } => registers
            .get(usize::try_from(*register).map_err(|_| {
                execution_error(
                    ErrorCode::CpuExecutionOverflow,
                    "CPU capture index overflow",
                )
            })?)
            .and_then(Option::as_ref)
            .and_then(|value| match value {
                RuntimeValue::F32(value) => Some(*value),
                RuntimeValue::Tensor(_) => None,
            }),
    }
    .ok_or_else(|| {
        execution_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU scalar operand is unavailable",
        )
    })
}

fn execute_scalar(
    body: &CpuScalarFunction,
    arguments: &[f32],
    registers: &[Option<RuntimeValue>],
    instruction_counter: &mut u64,
) -> AgentResult<f32> {
    let mut locals = Vec::with_capacity(body.instructions.len());
    for instruction in &body.instructions {
        let operands = instruction
            .operands
            .iter()
            .map(|operand| scalar_source(operand, arguments, &locals, registers))
            .collect::<AgentResult<Vec<_>>>()?;
        let result = match instruction.opcode {
            CpuScalarOpcode::AddF32 => operands[0] + operands[1],
            CpuScalarOpcode::MulF32 => operands[0] * operands[1],
            CpuScalarOpcode::FmaF32 => operands[0].mul_add(operands[1], operands[2]),
        };
        *instruction_counter = instruction_counter.checked_add(1).ok_or_else(|| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU instruction counter overflow",
            )
        })?;
        locals.push(result);
    }
    scalar_source(&body.result, arguments, &locals, registers)
}

fn resolve_extent(extent: &CpuExtent, dimensions: &BTreeMap<String, usize>) -> AgentResult<usize> {
    match extent {
        CpuExtent::Static { value } => usize::try_from(*value).map_err(|_| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU static extent exceeds usize",
            )
        }),
        CpuExtent::Symbol { name } => dimensions.get(name).copied().ok_or_else(|| {
            execution_error(
                ErrorCode::CpuExecutionInputMismatch,
                format!("runtime dimension `{name}` is unbound"),
            )
        }),
    }
}

fn register(registers: &[Option<RuntimeValue>], index: u32) -> AgentResult<&RuntimeValue> {
    registers
        .get(usize::try_from(index).map_err(|_| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU register index overflow",
            )
        })?)
        .and_then(Option::as_ref)
        .ok_or_else(|| {
            execution_error(ErrorCode::CpuArtifactInvalid, "CPU register is unavailable")
        })
}

fn scalar_register(registers: &[Option<RuntimeValue>], index: u32) -> AgentResult<f32> {
    match register(registers, index)? {
        RuntimeValue::F32(value) => Ok(*value),
        RuntimeValue::Tensor(_) => Err(execution_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU scalar instruction read a tensor register",
        )),
    }
}

fn tensor_register(registers: &[Option<RuntimeValue>], index: u32) -> AgentResult<&[f32]> {
    match register(registers, index)? {
        RuntimeValue::Tensor(values) => Ok(values),
        RuntimeValue::F32(_) => Err(execution_error(
            ErrorCode::CpuArtifactInvalid,
            "CPU tensor instruction read a scalar register",
        )),
    }
}

/// Non-correctness execution counters emitted without timing data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuExecutionObservation {
    /// Number of top-level and scalar-body instructions interpreted.
    pub instructions_executed: u64,
    /// Number of tensor elements produced by elementwise instructions.
    pub elements_executed: u64,
    /// Number of bytecode functions entered.
    pub function_calls: u64,
}

/// Named deterministic CPU execution result and observation counters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuExecutionResult {
    /// Executed compiler-owned package.
    pub cpu_artifact: CpuArtifactId,
    /// Exact package identity checked before execution.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Named JSON outputs.
    pub outputs: BTreeMap<String, Value>,
    /// Non-correctness runtime observation data.
    pub observation: CpuExecutionObservation,
}

/// Executes one structurally verified compiler-published CPU package safely and deterministically.
pub fn execute(
    package: &CpuArtifactPackage,
    inputs: &BTreeMap<String, Value>,
    limits: &ResourceLimits,
) -> AgentResult<CpuExecutionResult> {
    verify_cpu_artifact(package)?;
    let expected_names = package
        .bindings
        .iter()
        .map(|binding| binding.name.clone())
        .collect::<BTreeSet<_>>();
    let actual_names = inputs.keys().cloned().collect::<BTreeSet<_>>();
    if expected_names != actual_names {
        return Err(execution_error(
            ErrorCode::CpuExecutionInputMismatch,
            "CPU runtime input names differ from the compiler-owned binding interface",
        )
        .with_detail("expected_names", json!(expected_names))
        .with_detail("actual_names", json!(actual_names)));
    }
    let function = &package.functions[0];
    let register_count = usize::try_from(function.register_count).map_err(|_| {
        execution_error(
            ErrorCode::CpuExecutionOverflow,
            "CPU register count exceeds usize",
        )
    })?;
    let mut registers = vec![None; register_count];
    let mut dimensions = BTreeMap::new();
    let mut total_elements = 0_u64;
    for binding in &package.bindings {
        let input = &inputs[&binding.name];
        let value = match binding.value_type {
            CpuValueType::F32 => {
                let number = input.as_f64().ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionInputMismatch,
                        format!("input `{}` must be scalar f32", binding.name),
                    )
                })?;
                let value = number as f32;
                if !value.is_finite() {
                    return Err(execution_error(
                        ErrorCode::CpuExecutionInputMismatch,
                        format!("input `{}` is outside finite f32", binding.name),
                    ));
                }
                total_elements = total_elements.checked_add(1).ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU input element count overflow",
                    )
                })?;
                RuntimeValue::F32(value)
            }
            CpuValueType::F32Tensor1d => {
                let items = input.as_array().ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionInputMismatch,
                        format!(
                            "input `{}` must be a one-dimensional f32 tensor",
                            binding.name
                        ),
                    )
                })?;
                let length = items.len();
                match binding.extent.as_ref().ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuArtifactInvalid,
                        "CPU tensor binding lacks an extent",
                    )
                })? {
                    CpuExtent::Static { value } => {
                        if usize::try_from(*value).ok() != Some(length) {
                            return Err(execution_error(
                                ErrorCode::CpuExecutionInputMismatch,
                                format!("input `{}` has incompatible length", binding.name),
                            )
                            .with_types(*value, u64::try_from(length).unwrap_or(u64::MAX)));
                        }
                    }
                    CpuExtent::Symbol { name } => {
                        if let Some(previous) = dimensions.insert(name.clone(), length) {
                            if previous != length {
                                return Err(execution_error(
                                    ErrorCode::CpuExecutionInputMismatch,
                                    format!("runtime dimension `{name}` is inconsistent"),
                                ));
                            }
                        }
                    }
                }
                let values = items
                    .iter()
                    .map(|item| {
                        let number = item.as_f64().ok_or_else(|| {
                            execution_error(
                                ErrorCode::CpuExecutionInputMismatch,
                                format!("input `{}` contains a non-number", binding.name),
                            )
                        })?;
                        let value = number as f32;
                        if !value.is_finite() {
                            return Err(execution_error(
                                ErrorCode::CpuExecutionInputMismatch,
                                format!(
                                    "input `{}` contains a value outside finite f32",
                                    binding.name
                                ),
                            ));
                        }
                        Ok(value)
                    })
                    .collect::<AgentResult<Vec<_>>>()?;
                total_elements = total_elements
                    .checked_add(u64::try_from(values.len()).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        execution_error(
                            ErrorCode::CpuExecutionOverflow,
                            "CPU input element count overflow",
                        )
                    })?;
                RuntimeValue::Tensor(values)
            }
        };
        let slot = usize::try_from(binding.register).map_err(|_| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU binding register overflow",
            )
        })?;
        registers[slot] = Some(value);
    }
    let mut projected_elements = total_elements;
    let mut projected_work = u64::try_from(function.instructions.len()).map_err(|_| {
        execution_error(
            ErrorCode::CpuExecutionOverflow,
            "CPU top-level instruction count exceeds u64",
        )
    })?;
    let mut projected_buffers = package
        .bindings
        .iter()
        .filter(|binding| binding.value_type == CpuValueType::F32Tensor1d)
        .count();
    for instruction in &function.instructions {
        match instruction {
            CpuInstruction::ConstantF32 { .. }
            | CpuInstruction::AddF32 { .. }
            | CpuInstruction::MulF32 { .. }
            | CpuInstruction::FmaF32 { .. } => {
                projected_elements = projected_elements.checked_add(1).ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU projected scalar storage overflow",
                    )
                })?;
            }
            CpuInstruction::MapF32 { extent, body, .. }
            | CpuInstruction::ZipMapF32 { extent, body, .. } => {
                let length = u64::try_from(resolve_extent(extent, &dimensions)?).map_err(|_| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU projected tensor extent exceeds u64",
                    )
                })?;
                projected_elements = projected_elements.checked_add(length).ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU projected tensor storage overflow",
                    )
                })?;
                let body_work = length
                    .checked_mul(u64::try_from(body.instructions.len()).map_err(|_| {
                        execution_error(
                            ErrorCode::CpuExecutionOverflow,
                            "CPU scalar body instruction count exceeds u64",
                        )
                    })?)
                    .ok_or_else(|| {
                        execution_error(
                            ErrorCode::CpuExecutionOverflow,
                            "CPU projected instruction work overflow",
                        )
                    })?;
                projected_work = projected_work.checked_add(body_work).ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU projected instruction work overflow",
                    )
                })?;
                projected_buffers = projected_buffers.checked_add(1).ok_or_else(|| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU projected tensor buffer count overflow",
                    )
                })?;
            }
        }
    }
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionBuffers,
        u64::try_from(projected_buffers).unwrap_or(u64::MAX),
        "CPU execution tensor registers",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionElements,
        projected_elements,
        "CPU execution retained values",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionElements,
        projected_work,
        "CPU execution instruction work",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ExecutionBytes,
        projected_elements.checked_mul(4).ok_or_else(|| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU input byte size overflow",
            )
        })?,
        "CPU execution retained values",
    )?;
    let mut observation = CpuExecutionObservation {
        instructions_executed: 0,
        elements_executed: 0,
        function_calls: 1,
    };
    for instruction in &function.instructions {
        let result = match instruction {
            CpuInstruction::ConstantF32 { constant, .. } => RuntimeValue::F32(constant_f32(
                &package.constants[usize::try_from(*constant).map_err(|_| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU constant index overflow",
                    )
                })?]
                .bits,
            )?),
            CpuInstruction::AddF32 { lhs, rhs, .. } => RuntimeValue::F32(
                scalar_register(&registers, *lhs)? + scalar_register(&registers, *rhs)?,
            ),
            CpuInstruction::MulF32 { lhs, rhs, .. } => RuntimeValue::F32(
                scalar_register(&registers, *lhs)? * scalar_register(&registers, *rhs)?,
            ),
            CpuInstruction::FmaF32 { a, b, c, .. } => {
                RuntimeValue::F32(scalar_register(&registers, *a)?.mul_add(
                    scalar_register(&registers, *b)?,
                    scalar_register(&registers, *c)?,
                ))
            }
            CpuInstruction::MapF32 {
                input,
                extent,
                body,
                ..
            } => {
                let input = tensor_register(&registers, *input)?;
                let length = resolve_extent(extent, &dimensions)?;
                if input.len() != length {
                    return Err(execution_error(
                        ErrorCode::CpuExecutionInputMismatch,
                        "CPU map input length differs from validated runtime extent",
                    ));
                }
                let mut output = Vec::new();
                output.try_reserve_exact(length).map_err(|_| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU map output allocation failed",
                    )
                })?;
                for &value in input {
                    output.push(execute_scalar(
                        body,
                        &[value],
                        &registers,
                        &mut observation.instructions_executed,
                    )?);
                }
                observation.elements_executed = observation
                    .elements_executed
                    .checked_add(u64::try_from(length).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        execution_error(
                            ErrorCode::CpuExecutionOverflow,
                            "CPU element counter overflow",
                        )
                    })?;
                RuntimeValue::Tensor(output)
            }
            CpuInstruction::ZipMapF32 {
                inputs,
                extent,
                body,
                ..
            } => {
                let tensors = inputs
                    .iter()
                    .map(|input| tensor_register(&registers, *input))
                    .collect::<AgentResult<Vec<_>>>()?;
                let length = resolve_extent(extent, &dimensions)?;
                if tensors.iter().any(|tensor| tensor.len() != length) {
                    return Err(execution_error(
                        ErrorCode::CpuExecutionInputMismatch,
                        "CPU zip_map inputs have incompatible runtime shapes",
                    ));
                }
                let mut output = Vec::new();
                output.try_reserve_exact(length).map_err(|_| {
                    execution_error(
                        ErrorCode::CpuExecutionOverflow,
                        "CPU zip_map output allocation failed",
                    )
                })?;
                for index in 0..length {
                    let arguments = tensors
                        .iter()
                        .map(|tensor| tensor[index])
                        .collect::<Vec<_>>();
                    output.push(execute_scalar(
                        body,
                        &arguments,
                        &registers,
                        &mut observation.instructions_executed,
                    )?);
                }
                observation.elements_executed = observation
                    .elements_executed
                    .checked_add(u64::try_from(length).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        execution_error(
                            ErrorCode::CpuExecutionOverflow,
                            "CPU element counter overflow",
                        )
                    })?;
                RuntimeValue::Tensor(output)
            }
        };
        observation.instructions_executed = observation
            .instructions_executed
            .checked_add(1)
            .ok_or_else(|| {
                execution_error(
                    ErrorCode::CpuExecutionOverflow,
                    "CPU instruction counter overflow",
                )
            })?;
        let slot = usize::try_from(instruction.output()).map_err(|_| {
            execution_error(
                ErrorCode::CpuExecutionOverflow,
                "CPU output register overflow",
            )
        })?;
        registers[slot] = Some(result);
    }
    let mut outputs = BTreeMap::new();
    for output in &package.outputs {
        let value = match register(&registers, output.register)? {
            RuntimeValue::F32(value) => f32_number(*value)?,
            RuntimeValue::Tensor(values) => Value::Array(
                values
                    .iter()
                    .copied()
                    .map(f32_number)
                    .collect::<AgentResult<Vec<_>>>()?,
            ),
        };
        outputs.insert(output.name.clone(), value);
    }
    Ok(CpuExecutionResult {
        cpu_artifact: package.id.clone(),
        cpu_artifact_hash: package.cpu_artifact_hash.clone(),
        outputs,
        observation,
    })
}
