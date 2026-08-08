//! Deterministic ScheduleIR to BackendIR lowering and WGSL package emission.
//!
//! This crate is a compiler component: it performs no adapter discovery,
//! filesystem I/O, device allocation, or shader execution.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::{
    backend::{
        ArtifactHash, BackendAllocator, BackendHash, CompilerBuildHash, artifact_hash,
        compiler_build_hash,
    },
    backend_ir::{
        ARTIFACT_FORMAT_VERSION, ArtifactBindingLayout, ArtifactCertificate, ArtifactEntryPoint,
        ArtifactManifest, ArtifactModule, ArtifactPackage, ArtifactStatus, BackendBinding,
        BackendBindingAccess, BackendDispatch, BackendExpression, BackendExtent,
        BackendGuardBranch, BackendGuardPredicate, BackendKernel, BackendKind, BackendOutput,
        BackendParameterBlock, BackendParameterEntry, BackendParameterType, BackendProgram,
        BackendStatement, BackendValue, OfflineValidationReport,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{ArtifactId, BackendValueId, BufferId, ImplValueId, ScheduleNodeId},
    impl_ir::{ImplOperation, ImplProgram, ImplRegion, ImplRegionValue},
    ir::{ConstantValue, Opcode},
    memory::{MemoryRevision, MemoryStatus},
    memory_ir::{
        AccessMode, AddressSpace, MemoryBinding, MemoryLayout, MemoryProgram, ReuseDecision,
    },
    schedule::{SchedulePlan, ScheduleRevision, ScheduleStatus},
    schedule_ir::BindingLevel,
    target::{TargetManifest, WEBGPU_WGSL_V1},
    types::{DimExpr, ScalarType, Type},
};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Deterministic WGSL emitter implementation version.
pub const WGSL_EMITTER_VERSION: u32 = 1;
/// Offline Naga validation adapter version.
pub const WGSL_VALIDATOR_VERSION: u32 = 1;

fn lowering_error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::UnsupportedBackendLowering, message).with_repair(
        "preserve the legal ScheduleIR revision or select a supported elementwise schedule",
    )
}

fn extent_from_dimension(dimension: &DimExpr) -> AgentResult<BackendExtent> {
    match dimension {
        DimExpr::Static(value) => Ok(BackendExtent::Static { value: *value }),
        DimExpr::Symbol(name) => Ok(BackendExtent::Symbol { name: name.clone() }),
        DimExpr::Affine {
            coefficient: 1,
            symbol,
            constant: 0,
        } => Ok(BackendExtent::Symbol {
            name: symbol.clone(),
        }),
        DimExpr::Affine { .. } => Err(lowering_error(
            "WGSL v1 supports only static or one-symbol N extents",
        )),
    }
}

fn operation_tensor_type(
    operation: &ImplOperation,
) -> AgentResult<(ScalarType, agentir_core::types::Shape)> {
    operation
        .result_types
        .iter()
        .find_map(|ty| match ty {
            Type::Tensor { element, shape } => Some((*element, shape.clone())),
            Type::Scalar(_) => None,
        })
        .ok_or_else(|| lowering_error("backend lowering requires one tensor result"))
}

fn validate_subset(
    schedule: &ScheduleRevision,
    memory: &MemoryRevision,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<()> {
    if target.profile != WEBGPU_WGSL_V1 {
        return Err(AgentError::new(
            ErrorCode::BackendTargetMismatch,
            "backend.lower requires the webgpu_wgsl_v1 TargetManifest",
        )
        .with_types(WEBGPU_WGSL_V1, target.profile.clone())
        .with_repair("create webgpu_wgsl_v1 and schedule against that immutable target"));
    }
    if !matches!(
        schedule.status,
        ScheduleStatus::Proved | ScheduleStatus::ResourceValid | ScheduleStatus::Sealed
    ) || !matches!(
        memory.status,
        MemoryStatus::Proved | MemoryStatus::Guarded | MemoryStatus::Sealed
    ) {
        return Err(AgentError::new(
            ErrorCode::BackendEquivalenceUnproved,
            "backend lowering requires proved ScheduleIR and MemoryIR revisions",
        ));
    }
    for node in schedule.program.nodes.values() {
        let operation = implementation
            .operations
            .get(&node.impl_operation)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::BackendScheduleMismatch,
                    "schedule node references a missing ImplIR operation",
                )
            })?;
        if !matches!(operation.opcode, Opcode::Map | Opcode::ZipMap) {
            return Err(lowering_error(format!(
                "opcode `{}` is outside the WebGPU/WGSL v1 kernel subset",
                operation.opcode
            ))
            .with_detail("operation", operation.id.to_string())
            .with_detail("opcode", operation.opcode.to_string()));
        }
        let (element, shape) = operation_tensor_type(operation)?;
        if element != ScalarType::F32 || shape.0.len() != 1 {
            return Err(lowering_error(
                "WGSL v1 storage kernels require dense one-dimensional f32 tensors",
            ));
        }
        extent_from_dimension(&shape.0[0])?;
        if operation.region.is_none() {
            return Err(lowering_error(
                "map/zip_map lowering requires a verified pure region",
            ));
        }
    }
    for buffer in memory.program.buffers.values() {
        if buffer.element_type != ScalarType::F32
            || buffer.shape.0.len() != 1
            || buffer.layout != MemoryLayout::ContiguousRowMajor
            || !matches!(
                buffer.address_space,
                AddressSpace::Global | AddressSpace::Constant
            )
        {
            return Err(lowering_error(
                "WGSL v1 supports only dense one-dimensional f32 global/constant buffers",
            )
            .with_detail("buffer", buffer.id.to_string()));
        }
    }
    if schedule
        .program
        .vectorizations
        .iter()
        .any(|vector| ![1, 2, 4].contains(&vector.width))
    {
        return Err(AgentError::new(
            ErrorCode::BackendVectorUnsupported,
            "WGSL v1 supports only vector widths 1, 2, and 4",
        ));
    }
    Ok(())
}

fn scalar_parameter_type(ty: &Type) -> AgentResult<BackendParameterType> {
    match ty {
        Type::Scalar(ScalarType::F32) => Ok(BackendParameterType::F32),
        Type::Scalar(ScalarType::I32) => Ok(BackendParameterType::I32),
        _ => Err(lowering_error(
            "WGSL v1 scalar captures support only f32 and i32 parameters",
        )),
    }
}

fn parameter_block(
    implementation: &ImplProgram,
    extent: &BackendExtent,
    storage_binding_count: u32,
) -> AgentResult<BackendParameterBlock> {
    let mut entries = Vec::new();
    for (name, value) in &implementation.parameters {
        let ty = &implementation.values[value].ty;
        if matches!(ty, Type::Tensor { .. }) {
            continue;
        }
        let parameter_type = scalar_parameter_type(ty)?;
        entries.push(BackendParameterEntry {
            name: name.clone(),
            ty: parameter_type,
            offset: u64::try_from(entries.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(4),
            size: 4,
        });
    }
    if let BackendExtent::Symbol { name } = extent {
        if !entries.iter().any(|entry| entry.name == *name) {
            entries.push(BackendParameterEntry {
                name: name.clone(),
                ty: BackendParameterType::U32,
                offset: u64::try_from(entries.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(4),
                size: 4,
            });
        }
    }
    let raw = u64::try_from(entries.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(4);
    let byte_size = if raw == 0 {
        0
    } else {
        raw.saturating_add(15) / 16 * 16
    };
    Ok(BackendParameterBlock {
        group: 0,
        binding: storage_binding_count,
        entries,
        byte_size,
    })
}

fn buffer_access(access: AccessMode) -> BackendBindingAccess {
    match access {
        AccessMode::ReadOnly => BackendBindingAccess::Read,
        AccessMode::WriteOnly => BackendBindingAccess::Write,
        AccessMode::ReadWrite => BackendBindingAccess::ReadWrite,
    }
}

fn group_nodes(schedule: &ScheduleRevision) -> Vec<Vec<ScheduleNodeId>> {
    let mut by_fusion = BTreeMap::<u64, Vec<ScheduleNodeId>>::new();
    for group in &schedule.program.fusion_groups {
        by_fusion.insert(group.id, group.members.clone());
    }
    let mut visited = BTreeSet::new();
    let mut groups = Vec::new();
    for node_id in &schedule.program.node_order {
        if visited.contains(node_id) {
            continue;
        }
        let node = &schedule.program.nodes[node_id];
        if let Some(group_id) = node.fusion_group {
            let members = by_fusion
                .get(&group_id)
                .cloned()
                .unwrap_or_else(|| vec![node_id.clone()]);
            visited.extend(members.iter().cloned());
            groups.push(members);
        } else {
            visited.insert(node_id.clone());
            groups.push(vec![node_id.clone()]);
        }
    }
    groups
}

struct ExpressionBuilder<'a> {
    allocator: &'a mut BackendAllocator,
    values: BTreeMap<BackendValueId, BackendValue>,
}

impl ExpressionBuilder<'_> {
    fn push(&mut self, expression: BackendExpression, ty: ScalarType) -> BackendValueId {
        let id = self.allocator.value();
        self.values.insert(
            id.clone(),
            BackendValue {
                id: id.clone(),
                expression,
                ty,
            },
        );
        id
    }
}

fn binding_for_value<'a>(
    value: &ImplValueId,
    memory: &'a MemoryProgram,
    binding_numbers: &BTreeMap<BufferId, u32>,
) -> AgentResult<(&'a BufferId, u32)> {
    let MemoryBinding::Buffer { buffer, .. } = memory.bindings.get(value).ok_or_else(|| {
        AgentError::new(
            ErrorCode::BackendMemoryConflict,
            "ImplIR tensor value has no MemoryIR binding",
        )
    })?
    else {
        return Err(AgentError::new(
            ErrorCode::BackendMemoryConflict,
            "tensor kernel operand unexpectedly remained scalar SSA",
        ));
    };
    let binding = *binding_numbers.get(buffer).ok_or_else(|| {
        AgentError::new(
            ErrorCode::BackendBindingInvalid,
            "kernel binding map omitted a used MemoryIR buffer",
        )
    })?;
    Ok((buffer, binding))
}

fn constant_expression(value: &ConstantValue) -> (BackendExpression, ScalarType) {
    match value {
        ConstantValue::Bool { value } => (
            BackendExpression::Constant {
                ty: ScalarType::Bool,
                value: value.to_string(),
            },
            ScalarType::Bool,
        ),
        ConstantValue::I32 { value } => (
            BackendExpression::Constant {
                ty: ScalarType::I32,
                value: value.to_string(),
            },
            ScalarType::I32,
        ),
        ConstantValue::F32 { bits } => (
            BackendExpression::Constant {
                ty: ScalarType::F32,
                value: bits.clone(),
            },
            ScalarType::F32,
        ),
    }
}

fn capture_expression(
    value: &ImplValueId,
    implementation: &ImplProgram,
    builder: &mut ExpressionBuilder<'_>,
) -> AgentResult<BackendValueId> {
    if let Some((name, _)) = implementation
        .parameters
        .iter()
        .find(|(_, id)| *id == value)
    {
        let parameter_type = scalar_parameter_type(&implementation.values[value].ty)?;
        let scalar = match parameter_type {
            BackendParameterType::F32 => ScalarType::F32,
            BackendParameterType::I32 => ScalarType::I32,
            BackendParameterType::U32 => ScalarType::Index,
        };
        return Ok(builder.push(
            BackendExpression::Parameter {
                name: name.clone(),
                ty: parameter_type,
            },
            scalar,
        ));
    }
    if let Some(constant) = implementation.constants.get(value) {
        let (expression, ty) = constant_expression(constant);
        return Ok(builder.push(expression, ty));
    }
    Err(lowering_error(
        "scalar capture must be an external f32/i32 parameter or exact constant",
    ))
}

fn region_operand(
    operand: &ImplRegionValue,
    arguments: &BTreeMap<String, BackendValueId>,
    locals: &BTreeMap<String, BackendValueId>,
    captures: &BTreeMap<ImplValueId, BackendValueId>,
) -> AgentResult<BackendValueId> {
    match operand {
        ImplRegionValue::Argument(name) => arguments.get(name),
        ImplRegionValue::Local(name) => locals.get(name),
        ImplRegionValue::Capture(value) => captures.get(value),
    }
    .cloned()
    .ok_or_else(|| lowering_error("region operand could not be mapped to BackendIR SSA"))
}

fn lower_region(
    region: &ImplRegion,
    argument_values: Vec<BackendValueId>,
    implementation: &ImplProgram,
    builder: &mut ExpressionBuilder<'_>,
) -> AgentResult<BackendValueId> {
    if argument_values.len() != region.arguments.len() {
        return Err(lowering_error(
            "region argument count changed during lowering",
        ));
    }
    let arguments = region
        .arguments
        .iter()
        .zip(argument_values)
        .map(|(argument, value)| (argument.name.clone(), value))
        .collect::<BTreeMap<_, _>>();
    let mut captures = BTreeMap::new();
    for capture in &region.captures {
        captures.insert(
            capture.clone(),
            capture_expression(capture, implementation, builder)?,
        );
    }
    let mut locals = BTreeMap::new();
    for operation in &region.operations {
        let operands = operation
            .operands
            .iter()
            .map(|operand| region_operand(operand, &arguments, &locals, &captures))
            .collect::<AgentResult<Vec<_>>>()?;
        let ty = match operation.result_type {
            Type::Scalar(ty) => ty,
            Type::Tensor { .. } => {
                return Err(lowering_error(
                    "kernel region operations must remain scalar",
                ));
            }
        };
        let expression = match operation.opcode {
            Opcode::Add => BackendExpression::Add {
                lhs: operands[0].clone(),
                rhs: operands[1].clone(),
            },
            Opcode::Sub => BackendExpression::Sub {
                lhs: operands[0].clone(),
                rhs: operands[1].clone(),
            },
            Opcode::Mul => BackendExpression::Mul {
                lhs: operands[0].clone(),
                rhs: operands[1].clone(),
            },
            Opcode::Div => BackendExpression::Div {
                lhs: operands[0].clone(),
                rhs: operands[1].clone(),
            },
            Opcode::Fma => BackendExpression::Fma {
                a: operands[0].clone(),
                b: operands[1].clone(),
                c: operands[2].clone(),
            },
            Opcode::Compare => BackendExpression::Compare {
                predicate: operation
                    .attributes
                    .get("predicate")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("eq")
                    .to_owned(),
                lhs: operands[0].clone(),
                rhs: operands[1].clone(),
            },
            Opcode::Select => BackendExpression::Select {
                condition: operands[0].clone(),
                on_true: operands[1].clone(),
                on_false: operands[2].clone(),
            },
            Opcode::Cast => BackendExpression::Cast {
                value: operands[0].clone(),
                target: ty,
            },
            Opcode::Constant => {
                return Err(lowering_error(
                    "region constants must be represented as exact captured constants in v1",
                ));
            }
            Opcode::Parameter | Opcode::Map | Opcode::ZipMap | Opcode::Reduce => {
                return Err(lowering_error(format!(
                    "region opcode `{}` is unsupported by WGSL v1",
                    operation.opcode
                )));
            }
        };
        let value = builder.push(expression, ty);
        locals.insert(operation.result.clone(), value);
    }
    region_operand(&region.yield_value, &arguments, &locals, &captures)
}

fn kernel_bindings(
    group: &[ScheduleNodeId],
    schedule: &ScheduleRevision,
    memory: &MemoryProgram,
) -> AgentResult<(Vec<BackendBinding>, BTreeMap<BufferId, u32>)> {
    let mut buffers = BTreeSet::new();
    for node_id in group {
        let node = &schedule.program.nodes[node_id];
        let memory_operation = memory
            .operations
            .get(&node.memory_operation)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::BackendMemoryConflict,
                    "schedule node has no anchored MemoryIR operation",
                )
            })?;
        buffers.extend(
            memory_operation
                .accesses
                .iter()
                .map(|access| access.buffer.clone()),
        );
    }
    let mut bindings = Vec::new();
    let mut numbers = BTreeMap::new();
    for buffer_id in buffers {
        let buffer = &memory.buffers[&buffer_id];
        let number = u32::try_from(bindings.len()).map_err(|_| {
            AgentError::new(
                ErrorCode::BackendBindingInvalid,
                "kernel binding count exceeded u32",
            )
        })?;
        numbers.insert(buffer_id.clone(), number);
        bindings.push(BackendBinding {
            group: 0,
            binding: number,
            buffer: buffer_id,
            external_name: buffer.external_binding.clone(),
            element_type: buffer.element_type,
            shape: buffer.shape.clone(),
            access: buffer_access(buffer.access),
            alignment: buffer.alignment,
            offset_elements: buffer.offset_elements,
        });
    }
    Ok((bindings, numbers))
}

/// Lowers one verified ScheduleIR revision into a separate typed BackendIR graph.
pub fn lower_schedule(
    allocator: &mut BackendAllocator,
    _schedule_plan: &SchedulePlan,
    schedule: &ScheduleRevision,
    memory: &MemoryRevision,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<BackendProgram> {
    validate_subset(schedule, memory, implementation, target)?;
    let mut program = BackendProgram::default();
    for (kernel_index, group) in group_nodes(schedule).into_iter().enumerate() {
        let first_node = &schedule.program.nodes[&group[0]];
        let first_operation = &implementation.operations[&first_node.impl_operation];
        let (_, shape) = operation_tensor_type(first_operation)?;
        let extent = extent_from_dimension(&shape.0[0])?;
        let (bindings, binding_numbers) = kernel_bindings(&group, schedule, &memory.program)?;
        let parameter_block = parameter_block(
            implementation,
            &extent,
            u32::try_from(bindings.len()).unwrap_or(u32::MAX),
        )?;
        let serial = group.iter().all(|node_id| {
            schedule.program.nodes[node_id].axes.iter().all(|axis| {
                schedule.program.axes[axis]
                    .binding
                    .as_ref()
                    .is_none_or(|binding| binding.level == BindingLevel::Serial)
            })
        });
        let vector_width = group
            .iter()
            .flat_map(|node_id| schedule.program.nodes[node_id].axes.iter())
            .filter_map(|axis| {
                schedule
                    .program
                    .vectorizations
                    .iter()
                    .find(|vector| vector.axis == *axis)
                    .map(|vector| vector.width)
            })
            .max()
            .unwrap_or(1);
        let unroll_factor = group
            .iter()
            .flat_map(|node_id| schedule.program.nodes[node_id].axes.iter())
            .filter_map(|axis| {
                schedule
                    .program
                    .unrolls
                    .iter()
                    .find(|unroll| unroll.axis == *axis)
                    .map(|unroll| unroll.factor)
            })
            .max()
            .unwrap_or(1);
        let workgroup_size = if serial {
            [1, 1, 1]
        } else {
            schedule
                .program
                .resource_estimate
                .workgroup_dimensions
                .map(|value| u32::try_from(value.max(1)).unwrap_or(u32::MAX))
        };
        let mut builder = ExpressionBuilder {
            allocator,
            values: BTreeMap::new(),
        };
        let index = builder.push(BackendExpression::InvocationIndex, ScalarType::Index);
        let mut loop_body = Vec::new();
        let mut kernel_outputs = Vec::new();
        for node_id in &group {
            let node = &schedule.program.nodes[node_id];
            let operation = &implementation.operations[&node.impl_operation];
            let region = operation
                .region
                .as_ref()
                .ok_or_else(|| lowering_error("verified map/zip_map operation has no region"))?;
            let mut arguments = Vec::new();
            for operand in &operation.operands {
                let (_, binding) = binding_for_value(operand, &memory.program, &binding_numbers)?;
                arguments.push(builder.push(
                    BackendExpression::Load {
                        binding,
                        index: index.clone(),
                        ty: ScalarType::F32,
                    },
                    ScalarType::F32,
                ));
            }
            let result = lower_region(region, arguments, implementation, &mut builder)?;
            let output_value = operation
                .results
                .first()
                .ok_or_else(|| lowering_error("elementwise operation has no result"))?;
            let (buffer, binding) =
                binding_for_value(output_value, &memory.program, &binding_numbers)?;
            loop_body.push(BackendStatement::Store {
                binding,
                index: index.clone(),
                value: result,
            });
            for (name, output) in &implementation.outputs {
                if &output.value == output_value {
                    kernel_outputs.push(BackendOutput {
                        name: name.clone(),
                        binding,
                        buffer: buffer.clone(),
                    });
                }
            }
        }
        let statements = if serial {
            vec![BackendStatement::SerialLoop {
                index: index.clone(),
                extent: extent.clone(),
                body: loop_body,
            }]
        } else {
            let predicate = builder.push(
                BackendExpression::BoundsPredicate {
                    index: index.clone(),
                    extent: extent.clone(),
                },
                ScalarType::Bool,
            );
            vec![BackendStatement::IfBounds {
                predicate,
                body: loop_body,
            }]
        };
        let kernel_id = builder.allocator.kernel();
        let kernel = BackendKernel {
            id: kernel_id.clone(),
            entry_point: format!("agentir_kernel_{kernel_index}"),
            source_schedule_nodes: group,
            bindings,
            parameter_block,
            workgroup_size,
            logical_extent: extent.clone(),
            serial,
            vector_width: u32::try_from(vector_width).unwrap_or(u32::MAX),
            unroll_factor: u32::try_from(unroll_factor).unwrap_or(u32::MAX),
            values: builder.values,
            statements,
            outputs: kernel_outputs.clone(),
            lowering_method: if serial {
                "serial_elementwise_loop_v1".to_owned()
            } else {
                "global_invocation_bounds_checked_v1".to_owned()
            },
        };
        let workgroups_x = match extent {
            BackendExtent::Static { value } => BackendExtent::Static {
                value: if serial {
                    1
                } else {
                    value.saturating_add(u64::from(workgroup_size[0]).saturating_sub(1))
                        / u64::from(workgroup_size[0])
                },
            },
            BackendExtent::Symbol { name } => BackendExtent::Symbol { name },
        };
        let order = u64::try_from(program.dispatches.len()).unwrap_or(u64::MAX);
        program.dispatches.push(BackendDispatch {
            order,
            kernel: kernel_id.clone(),
            workgroups: [
                workgroups_x,
                BackendExtent::Static { value: 1 },
                BackendExtent::Static { value: 1 },
            ],
            workgroup_size,
            bounds_checked: !serial,
        });
        program.outputs.extend(kernel_outputs);
        program.kernel_order.push(kernel_id.clone());
        program.kernels.insert(kernel_id, kernel);
    }
    let guarded = memory
        .program
        .reuse_decisions
        .values()
        .find_map(|decision| match decision {
            ReuseDecision::Guarded {
                guard, fallback, ..
            } => Some((guard, fallback)),
            ReuseDecision::Fresh { .. } | ReuseDecision::InPlace { .. } => None,
        });
    if let Some((guard, fallback)) = guarded {
        let output_buffer = memory
            .program
            .bindings
            .get(&fallback.result)
            .and_then(MemoryBinding::buffer)
            .cloned()
            .unwrap_or_else(|| fallback.fresh_buffer.id.clone());
        let extent = extent_from_dimension(&fallback.fresh_buffer.shape.0[0])?;
        let bytes = match extent {
            BackendExtent::Static { value } => BackendExtent::Static {
                value: value.checked_mul(4).ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::BackendMemoryConflict,
                        "guarded byte extent overflowed u64",
                    )
                })?,
            },
            BackendExtent::Symbol { name } => BackendExtent::Symbol { name },
        };
        let orders: Vec<u64> = program
            .dispatches
            .iter()
            .map(|dispatch| dispatch.order)
            .collect();
        program.guard = Some(BackendGuardBranch {
            predicate: BackendGuardPredicate::NoOverlap {
                first: guard.primary_buffer.clone(),
                second: guard.other_buffer.clone(),
                bytes,
            },
            true_dispatches: orders.clone(),
            false_dispatches: orders,
        });
        if !program
            .outputs
            .iter()
            .any(|output| output.buffer == output_buffer)
        {
            // Output identity remains the logical MemoryIR binding; runtime swaps only storage.
        }
    }
    Ok(program)
}

const fn wgsl_type(ty: ScalarType) -> &'static str {
    match ty {
        ScalarType::Bool => "bool",
        ScalarType::I32 => "i32",
        ScalarType::F32 => "f32",
        ScalarType::Index => "u32",
    }
}

fn f32_literal(bits: &str) -> AgentResult<String> {
    let raw = u32::from_str_radix(bits.trim_start_matches("0x"), 16).map_err(|_| {
        AgentError::new(
            ErrorCode::WgslEmissionFailed,
            "invalid canonical f32 bits in BackendIR",
        )
    })?;
    let value = f32::from_bits(raw);
    if !value.is_finite() {
        return Err(AgentError::new(
            ErrorCode::WgslEmissionFailed,
            "WGSL v1 does not emit non-finite decimal literals",
        ));
    }
    let mut text = format!("{value:?}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text.push('f');
    Ok(text)
}

fn parameter_field(kernel: &BackendKernel, name: &str) -> AgentResult<String> {
    kernel
        .parameter_block
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .map(|index| format!("p{index}"))
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::WgslEmissionFailed,
                format!("parameter `{name}` is missing from the stable ABI"),
            )
        })
}

fn render_extent(kernel: &BackendKernel, extent: &BackendExtent) -> AgentResult<String> {
    match extent {
        BackendExtent::Static { value } => Ok(format!("{value}u")),
        BackendExtent::Symbol { name } => Ok(format!("params.{}", parameter_field(kernel, name)?)),
    }
}

fn render_expression(
    kernel: &BackendKernel,
    value: &BackendValueId,
    serial_index: &str,
) -> AgentResult<String> {
    let value = kernel.values.get(value).ok_or_else(|| {
        AgentError::new(
            ErrorCode::WgslEmissionFailed,
            "BackendIR expression references a missing value",
        )
    })?;
    let render = |value| render_expression(kernel, value, serial_index);
    match &value.expression {
        BackendExpression::Load { binding, index, .. } => {
            let offset = kernel
                .bindings
                .iter()
                .find(|candidate| candidate.binding == *binding)
                .ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::WgslEmissionFailed,
                        "load references a missing storage binding",
                    )
                })?
                .offset_elements;
            let index = render(index)?;
            let index = if offset == 0 {
                index
            } else {
                format!("({index} + {offset}u)")
            };
            Ok(format!("b{binding}.data[{index}]"))
        }
        BackendExpression::Parameter { name, .. } => {
            Ok(format!("params.{}", parameter_field(kernel, name)?))
        }
        BackendExpression::Constant { ty, value } => match ty {
            ScalarType::Bool => Ok(value.clone()),
            ScalarType::I32 => Ok(format!("{value}i")),
            ScalarType::F32 => f32_literal(value),
            ScalarType::Index => Ok(format!("{value}u")),
        },
        BackendExpression::InvocationIndex => Ok(serial_index.to_owned()),
        BackendExpression::Add { lhs, rhs } => Ok(format!("({} + {})", render(lhs)?, render(rhs)?)),
        BackendExpression::Sub { lhs, rhs } => Ok(format!("({} - {})", render(lhs)?, render(rhs)?)),
        BackendExpression::Mul { lhs, rhs } => Ok(format!("({} * {})", render(lhs)?, render(rhs)?)),
        BackendExpression::Div { lhs, rhs } => Ok(format!("({} / {})", render(lhs)?, render(rhs)?)),
        BackendExpression::Fma { a, b, c } => Ok(format!(
            "fma({}, {}, {})",
            render(a)?,
            render(b)?,
            render(c)?
        )),
        BackendExpression::Compare {
            predicate,
            lhs,
            rhs,
        } => {
            let operator = match predicate.as_str() {
                "lt" => "<",
                "le" => "<=",
                "eq" => "==",
                "ne" => "!=",
                "ge" => ">=",
                "gt" => ">",
                _ => {
                    return Err(AgentError::new(
                        ErrorCode::WgslEmissionFailed,
                        "unsupported comparison predicate",
                    ));
                }
            };
            Ok(format!("({} {operator} {})", render(lhs)?, render(rhs)?))
        }
        BackendExpression::Select {
            condition,
            on_true,
            on_false,
        } => Ok(format!(
            "select({}, {}, {})",
            render(on_false)?,
            render(on_true)?,
            render(condition)?
        )),
        BackendExpression::Cast { value, target } => {
            Ok(format!("{}({})", wgsl_type(*target), render(value)?))
        }
        BackendExpression::VectorConstruct { lanes, element } => Ok(format!(
            "vec{}<{}>({})",
            lanes.len(),
            wgsl_type(*element),
            lanes
                .iter()
                .map(&render)
                .collect::<AgentResult<Vec<_>>>()?
                .join(", ")
        )),
        BackendExpression::VectorExtract { vector, lane } => {
            Ok(format!("{}[{lane}u]", render(vector)?))
        }
        BackendExpression::BoundsPredicate { index, extent } => Ok(format!(
            "({} < {})",
            render(index)?,
            render_extent(kernel, extent)?
        )),
    }
}

fn emit_statements(
    source: &mut String,
    kernel: &BackendKernel,
    statements: &[BackendStatement],
    indent: usize,
    index_name: &str,
) -> AgentResult<()> {
    let padding = "    ".repeat(indent);
    for statement in statements {
        match statement {
            BackendStatement::Store {
                binding,
                index,
                value,
            } => {
                let offset = kernel
                    .bindings
                    .iter()
                    .find(|candidate| candidate.binding == *binding)
                    .ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::WgslEmissionFailed,
                            "store references a missing storage binding",
                        )
                    })?
                    .offset_elements;
                let index = render_expression(kernel, index, index_name)?;
                let index = if offset == 0 {
                    index
                } else {
                    format!("({index} + {offset}u)")
                };
                writeln!(
                    source,
                    "{padding}b{binding}.data[{index}] = {};",
                    render_expression(kernel, value, index_name)?
                )
                .expect("writing to String cannot fail");
            }
            BackendStatement::SerialLoop { extent, body, .. } => {
                writeln!(
                    source,
                    "{padding}for (var i: u32 = 0u; i < {}; i = i + 1u) {{",
                    render_extent(kernel, extent)?
                )
                .expect("writing to String cannot fail");
                emit_statements(source, kernel, body, indent + 1, "i")?;
                writeln!(source, "{padding}}}").expect("writing to String cannot fail");
            }
            BackendStatement::IfBounds { predicate, body } => {
                writeln!(
                    source,
                    "{padding}if {} {{",
                    render_expression(kernel, predicate, index_name)?
                )
                .expect("writing to String cannot fail");
                emit_statements(source, kernel, body, indent + 1, index_name)?;
                writeln!(source, "{padding}}}").expect("writing to String cannot fail");
            }
        }
    }
    Ok(())
}

/// Emits exact deterministic WGSL bytes for one BackendIR kernel.
pub fn emit_kernel_wgsl(kernel: &BackendKernel) -> AgentResult<String> {
    let mut source = String::new();
    for binding in &kernel.bindings {
        writeln!(
            source,
            "struct Storage{} {{ data: array<{}>, }};",
            binding.binding,
            wgsl_type(binding.element_type)
        )
        .expect("writing to String cannot fail");
        let access = match binding.access {
            BackendBindingAccess::Read => "read",
            BackendBindingAccess::Write | BackendBindingAccess::ReadWrite => "read_write",
        };
        writeln!(
            source,
            "@group(0) @binding({}) var<storage, {access}> b{}: Storage{};",
            binding.binding, binding.binding, binding.binding
        )
        .expect("writing to String cannot fail");
    }
    if !kernel.parameter_block.entries.is_empty() {
        source.push_str("struct Parameters {\n");
        for (index, entry) in kernel.parameter_block.entries.iter().enumerate() {
            let ty = match entry.ty {
                BackendParameterType::F32 => "f32",
                BackendParameterType::I32 => "i32",
                BackendParameterType::U32 => "u32",
            };
            writeln!(source, "    p{index}: {ty},").expect("writing to String cannot fail");
        }
        let used = kernel.parameter_block.entries.len().saturating_mul(4);
        let padding_words = usize::try_from(kernel.parameter_block.byte_size)
            .unwrap_or(usize::MAX)
            .saturating_sub(used)
            / 4;
        for index in 0..padding_words {
            writeln!(source, "    _pad{index}: u32,").expect("writing to String cannot fail");
        }
        source.push_str("};\n");
        writeln!(
            source,
            "@group(0) @binding({}) var<uniform> params: Parameters;",
            kernel.parameter_block.binding
        )
        .expect("writing to String cannot fail");
    }
    writeln!(
        source,
        "@compute @workgroup_size({}, {}, {})",
        kernel.workgroup_size[0], kernel.workgroup_size[1], kernel.workgroup_size[2]
    )
    .expect("writing to String cannot fail");
    writeln!(
        source,
        "fn {}(@builtin(global_invocation_id) gid: vec3<u32>) {{",
        kernel.entry_point
    )
    .expect("writing to String cannot fail");
    emit_statements(&mut source, kernel, &kernel.statements, 1, "gid.x")?;
    source.push_str("}\n");
    Ok(source)
}

/// Parses and validates emitted WGSL without opening a GPU device.
pub fn offline_validate_modules(
    modules: &[ArtifactModule],
) -> AgentResult<OfflineValidationReport> {
    for module in modules {
        let parsed = naga::front::wgsl::parse_str(&module.wgsl).map_err(|error| {
            AgentError::new(
                ErrorCode::WgslParseFailed,
                format!("WGSL module `{}` failed parsing: {error}", module.name),
            )
        })?;
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&parsed)
            .map_err(|error| {
                AgentError::new(
                    ErrorCode::WgslValidationFailed,
                    format!("WGSL module `{}` failed validation: {error}", module.name),
                )
            })?;
        for entry_point in &module.entry_points {
            if !parsed.entry_points.iter().any(|entry| {
                entry.name == *entry_point && entry.stage == naga::ShaderStage::Compute
            }) {
                return Err(AgentError::new(
                    ErrorCode::WgslValidationFailed,
                    format!("entry point `{entry_point}` is not a compute entry point"),
                ));
            }
        }
    }
    Ok(OfflineValidationReport {
        parsed: true,
        validated: true,
        validator: format!("naga_wgsl_v24_adapter_v{WGSL_VALIDATOR_VERSION}"),
        diagnostics: Vec::new(),
    })
}

/// Emits and offline-validates one reproducible artifact package.
pub fn emit_artifact(
    allocator: &mut BackendAllocator,
    artifact_id: ArtifactId,
    anchor: agentir_core::backend_ir::BackendAnchor,
    backend_hash_value: BackendHash,
    program: &BackendProgram,
) -> AgentResult<ArtifactPackage> {
    let mut modules = Vec::new();
    let mut entries = Vec::new();
    let mut binding_layouts = Vec::new();
    for kernel_id in &program.kernel_order {
        let kernel = &program.kernels[kernel_id];
        let module_id = allocator.module();
        let source = emit_kernel_wgsl(kernel)?;
        modules.push(ArtifactModule {
            id: module_id.clone(),
            name: format!("{}.wgsl", kernel.entry_point),
            wgsl: source,
            entry_points: vec![kernel.entry_point.clone()],
        });
        entries.push(ArtifactEntryPoint {
            module: module_id,
            name: kernel.entry_point.clone(),
            kernel: kernel.id.clone(),
            workgroup_size: kernel.workgroup_size,
        });
        binding_layouts.push(ArtifactBindingLayout {
            kernel: kernel.id.clone(),
            storage_bindings: kernel.bindings.clone(),
            parameter_block: kernel.parameter_block.clone(),
            logical_extent: kernel.logical_extent.clone(),
            outputs: kernel.outputs.clone(),
        });
    }
    let offline_validation = offline_validate_modules(&modules)?;
    let build_hash = compiler_build_hash();
    let manifest = ArtifactManifest {
        format: "agentir.wgsl.package".to_owned(),
        format_version: ARTIFACT_FORMAT_VERSION,
        backend_kind: BackendKind::WebGpuWgslV1,
        anchor,
        backend_hash: backend_hash_value.clone(),
        compiler_build_hash: build_hash.clone(),
        modules: modules.iter().map(|module| module.id.clone()).collect(),
        entry_points: entries,
        binding_layouts,
        dispatches: program.dispatches.clone(),
        guard: program.guard.clone(),
        outputs: program.outputs.clone(),
        proof_manifest: vec![
            "BackendEquivalentToSchedule".to_owned(),
            "ArtifactEquivalentToBackend".to_owned(),
            "offline WGSL validation is well-formedness evidence only".to_owned(),
        ],
    };
    let mut package = ArtifactPackage {
        id: artifact_id,
        manifest,
        modules,
        offline_validation,
        status: ArtifactStatus::Validated,
        artifact_hash: ArtifactHash::new("pending"),
        certificate: ArtifactCertificate {
            relation: "pending".to_owned(),
            backend_hash: backend_hash_value,
            compiler_build_hash: build_hash,
            artifact_hash: ArtifactHash::new("pending"),
            conditions: Vec::new(),
            validator_version: 1,
        },
    };
    package.artifact_hash = artifact_hash(&package)?;
    Ok(package)
}

/// Returns the compiler build hash used by emitted packages.
#[must_use]
pub fn emitted_compiler_build_hash() -> CompilerBuildHash {
    compiler_build_hash()
}
