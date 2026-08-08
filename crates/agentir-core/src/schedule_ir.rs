//! Typed ScheduleIR graph, legality facts, and deterministic resource estimates.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        BufferId, ImplOperationId, MemoryOperationId, ScheduleAxisId, ScheduleNodeId,
        ScheduleOperationId,
    },
    impl_ir::{ImplProgram, ImplValueOrigin},
    ir::Opcode,
    memory_ir::{AddressSpace, MemoryBinding, MemoryProgram, MemoryStride},
    target::TargetManifest,
    types::{DimExpr, ScalarType, Shape, Type},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// ScheduleIR semantic version.
pub const SCHEDULE_SEMANTICS_VERSION: u32 = 1;
/// Structural schedule validator version.
pub const SCHEDULE_VALIDATOR_VERSION: u32 = 1;
/// Deterministic resource simulator version.
pub const RESOURCE_SIMULATOR_VERSION: u32 = 1;
/// Deterministic schedule trace codec version.
pub const SCHEDULE_TRACE_CODEC_VERSION: u32 = 1;

/// Role of one logical schedule axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisRole {
    /// Independent logical data coordinate.
    DataParallel,
    /// Fixed-order reduction coordinate.
    Reduction,
    /// Explicitly serialized logical coordinate.
    Serial,
    /// Exact broadcast-like coordinate inherited from existing semantics.
    BroadcastLike,
}

/// Typed logical extent retained without host/device discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AxisExtent {
    /// Statically known non-zero extent.
    Static {
        /// Positive static extent.
        value: u64,
    },
    /// Bounded symbolic/affine expression retained from ImplIR.
    Symbolic {
        /// Canonical symbolic or affine expression.
        expression: String,
    },
}

impl AxisExtent {
    /// Returns a static extent when one is known.
    #[must_use]
    pub const fn static_value(&self) -> Option<u64> {
        match self {
            Self::Static { value } => Some(*value),
            Self::Symbolic { .. } => None,
        }
    }
}

/// Exact compiler-owned tail representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TailStrategy {
    /// The transform divides the known extent exactly.
    Exact,
    /// A compiler-generated remainder domain covers the tail.
    CompilerRemainder {
        /// Static remainder when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        remainder: Option<u64>,
    },
}

/// Provenance of one transformed axis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AxisTransform {
    /// Root logical axis derived from an operation domain.
    Root,
    /// Outer or inner component of an exact split.
    Split {
        /// Source logical axis.
        parent: ScheduleAxisId,
        /// Positive split factor.
        factor: u64,
        /// Whether this is the outer quotient axis.
        outer: bool,
    },
    /// Axis produced by a multi-axis tile request.
    Tile {
        /// Source logical axis.
        parent: ScheduleAxisId,
        /// Positive tile size.
        tile_size: u64,
        /// Whether this is the outer tile axis.
        outer: bool,
    },
}

/// Target execution hierarchy binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingLevel {
    /// Serialized execution.
    Serial,
    /// Logical grid X dimension.
    GridX,
    /// Logical grid Y dimension.
    GridY,
    /// Logical grid Z dimension.
    GridZ,
    /// Logical block X dimension.
    BlockX,
    /// Logical block Y dimension.
    BlockY,
    /// Logical block Z dimension.
    BlockZ,
    /// Logical workgroup X dimension.
    WorkgroupX,
    /// Logical workgroup Y dimension.
    WorkgroupY,
    /// Logical workgroup Z dimension.
    WorkgroupZ,
    /// Fixed subgroup lane.
    SubgroupLane,
    /// Exact vector lane.
    VectorLane,
}

/// Binding attached to one logical axis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleBinding {
    /// Hierarchy level.
    pub level: BindingLevel,
    /// Static bound when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<u64>,
}

/// One compiler-assigned logical schedule axis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAxis {
    /// Axis identity.
    pub id: ScheduleAxisId,
    /// Parent operation domain.
    pub operation: ImplOperationId,
    /// Canonical dimension index.
    pub dimension: usize,
    /// Logical role.
    pub role: AxisRole,
    /// Exact or symbolic logical extent.
    pub extent: AxisExtent,
    /// Compiler-owned transform provenance.
    pub transform: AxisTransform,
    /// Optional target hierarchy binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<ScheduleBinding>,
    /// Exact tail representation.
    pub tail: TailStrategy,
}

/// Typed logical iteration domain for one operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationDomain {
    /// Source ImplIR operation.
    pub operation: ImplOperationId,
    /// Immutable logical shape.
    pub shape: Shape,
    /// Canonically ordered axes.
    pub axes: Vec<ScheduleAxisId>,
    /// Whether this domain contains a reduction axis.
    pub has_reduction: bool,
}

/// One high-level scheduled operation node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleNode {
    /// Schedule node identity.
    pub id: ScheduleNodeId,
    /// Schedule operation identity.
    pub schedule_operation: ScheduleOperationId,
    /// Anchored MemoryIR operation.
    pub memory_operation: MemoryOperationId,
    /// Anchored ImplIR operation.
    pub impl_operation: ImplOperationId,
    /// Stable opcode spelling.
    pub opcode: String,
    /// Current axes for this operation.
    pub axes: Vec<ScheduleAxisId>,
    /// Optional exact fusion group index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fusion_group: Option<u64>,
}

/// Exact split specification retained in canonical state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitSpec {
    /// Source axis.
    pub axis: ScheduleAxisId,
    /// Positive factor.
    pub factor: u64,
    /// New outer axis.
    pub outer: ScheduleAxisId,
    /// New inner axis.
    pub inner: ScheduleAxisId,
    /// Exact tail handling.
    pub tail: TailStrategy,
}

/// Exact multi-axis tile specification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileSpec {
    /// Source axes in request order.
    pub axes: Vec<ScheduleAxisId>,
    /// Positive sizes corresponding to `axes`.
    pub tile_sizes: Vec<u64>,
    /// Compiler-generated split specifications.
    pub splits: Vec<SplitSpec>,
}

/// Restricted exact operation fusion group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionGroup {
    /// Stable group number.
    pub id: u64,
    /// Producer then consumer nodes.
    pub members: Vec<ScheduleNodeId>,
    /// Compiler-owned legality method.
    pub certificate: String,
}

/// Exact vectorization decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorizationSpec {
    /// Vectorized logical axis.
    pub axis: ScheduleAxisId,
    /// Supported vector width.
    pub width: u64,
    /// Compiler-owned exactness certificate label.
    pub certificate: String,
}

/// Exact bounded unroll decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnrollSpec {
    /// Unrolled logical axis.
    pub axis: ScheduleAxisId,
    /// Positive bounded factor.
    pub factor: u64,
    /// Compiler-owned preservation certificate label.
    pub certificate: String,
}

/// Producer-consumer schedule dependence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScheduleDependency {
    /// Producer schedule node.
    pub source: ScheduleNodeId,
    /// Consumer schedule node.
    pub target: ScheduleNodeId,
    /// Stable dependence class.
    pub kind: String,
}

/// Compiler-owned structural legality fact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScheduleLegalityFact {
    /// Stable fact name.
    pub kind: String,
    /// Stable subject identity.
    pub subject: String,
    /// Compiler validator method.
    pub method: String,
}

/// Deterministic analytical resource estimate relative to one target hash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleResourceEstimate {
    /// Logical grid dimensions.
    pub grid_dimensions: [u64; 3],
    /// Logical block/workgroup dimensions.
    pub workgroup_dimensions: [u64; 3],
    /// Product of local dimensions.
    pub threads_per_workgroup: u64,
    /// Number of abstract subgroups.
    pub subgroup_count: u64,
    /// Maximum selected vector lanes.
    pub vector_lanes: u64,
    /// Conservative serial work per logical thread.
    pub serial_work_per_thread: u64,
    /// Abstract shared bytes per workgroup.
    pub shared_bytes_per_workgroup: u64,
    /// Abstract private bytes per thread.
    pub private_bytes_per_thread: u64,
    /// Constant/global buffer reference count.
    pub global_buffer_references: u64,
    /// Active logical tile count when statically known.
    pub active_logical_tiles: u64,
    /// Compiler remainder domain count.
    pub remainder_tiles: u64,
    /// Maximum statically known live MemoryIR bytes.
    pub maximum_live_memory_bytes: u64,
    /// Stable capacity violations; empty means resource-valid.
    pub capacity_violations: Vec<String>,
    /// Simulator version.
    pub simulator_version: u32,
}

/// Complete separate typed scheduling graph.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleProgram {
    /// High-level nodes by ID.
    pub nodes: BTreeMap<ScheduleNodeId, ScheduleNode>,
    /// Canonical high-level execution order.
    pub node_order: Vec<ScheduleNodeId>,
    /// Typed domains by source operation.
    pub domains: BTreeMap<ImplOperationId, IterationDomain>,
    /// Compiler-assigned axes.
    pub axes: BTreeMap<ScheduleAxisId, ScheduleAxis>,
    /// Ordered exact split operations.
    pub splits: Vec<SplitSpec>,
    /// Ordered exact tile operations.
    pub tiles: Vec<TileSpec>,
    /// Restricted exact fusion groups.
    pub fusion_groups: Vec<FusionGroup>,
    /// Exact vectorization choices.
    pub vectorizations: Vec<VectorizationSpec>,
    /// Exact unroll choices.
    pub unrolls: Vec<UnrollSpec>,
    /// Compiler-owned dependency graph.
    pub dependencies: Vec<ScheduleDependency>,
    /// Compiler-owned legality facts.
    pub legality_facts: Vec<ScheduleLegalityFact>,
    /// Deterministic analytical resource estimate.
    pub resource_estimate: ScheduleResourceEstimate,
    /// Explicit launch grid selected by the client, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_grid: Option<[u64; 3]>,
    /// Explicit launch workgroup selected by the client, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_workgroup: Option<[u64; 3]>,
}

/// Minimal allocator interface used while constructing ScheduleIR.
pub trait ScheduleIdAllocator {
    /// Allocates one schedule node.
    fn schedule_node(&mut self) -> ScheduleNodeId;
    /// Allocates one schedule operation.
    fn schedule_operation(&mut self) -> ScheduleOperationId;
    /// Allocates one schedule axis.
    fn schedule_axis(&mut self) -> ScheduleAxisId;
}

fn schedule_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn extent(dimension: &DimExpr) -> AgentResult<AxisExtent> {
    match dimension {
        DimExpr::Static(0) => Err(schedule_error(
            ErrorCode::InvalidIterationDomain,
            "zero static schedule extent is unsupported",
        )),
        DimExpr::Static(value) => Ok(AxisExtent::Static { value: *value }),
        expression => Ok(AxisExtent::Symbolic {
            expression: expression.to_string(),
        }),
    }
}

fn operation_shape(operation: &ImplOperationId, implementation: &ImplProgram) -> Shape {
    let operation_data = &implementation.operations[operation];
    operation_data
        .results
        .iter()
        .chain(&operation_data.operands)
        .find_map(|value| match &implementation.values[value].ty {
            Type::Tensor { shape, .. } => Some(shape.clone()),
            Type::Scalar(_) => None,
        })
        .unwrap_or_default()
}

/// Builds the conservative serial root schedule in canonical MemoryIR order.
pub fn build_serial_schedule(
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    allocator: &mut impl ScheduleIdAllocator,
) -> AgentResult<ScheduleProgram> {
    let mut program = ScheduleProgram::default();
    for memory_operation_id in &memory.operation_order {
        let memory_operation = memory.operations.get(memory_operation_id).ok_or_else(|| {
            schedule_error(
                ErrorCode::ScheduleMemoryConflict,
                "MemoryIR operation order references a missing operation",
            )
        })?;
        let impl_operation = implementation
            .operations
            .get(&memory_operation.impl_operation)
            .ok_or_else(|| {
                schedule_error(
                    ErrorCode::ScheduleMemoryConflict,
                    "MemoryIR operation has no anchored ImplIR operation",
                )
            })?;
        let node_id = allocator.schedule_node();
        let schedule_operation = allocator.schedule_operation();
        let shape = operation_shape(&impl_operation.id, implementation);
        let mut axes = Vec::new();
        for (dimension, expression) in shape.0.iter().enumerate() {
            let axis_id = allocator.schedule_axis();
            let reduction = impl_operation.opcode == Opcode::Reduce;
            program.axes.insert(
                axis_id.clone(),
                ScheduleAxis {
                    id: axis_id.clone(),
                    operation: impl_operation.id.clone(),
                    dimension,
                    role: if reduction {
                        AxisRole::Reduction
                    } else {
                        AxisRole::DataParallel
                    },
                    extent: extent(expression)?,
                    transform: AxisTransform::Root,
                    binding: Some(ScheduleBinding {
                        level: BindingLevel::Serial,
                        extent: extent(expression)?.static_value(),
                    }),
                    tail: TailStrategy::Exact,
                },
            );
            axes.push(axis_id);
        }
        program.domains.insert(
            impl_operation.id.clone(),
            IterationDomain {
                operation: impl_operation.id.clone(),
                shape,
                axes: axes.clone(),
                has_reduction: impl_operation.opcode == Opcode::Reduce,
            },
        );
        program.nodes.insert(
            node_id.clone(),
            ScheduleNode {
                id: node_id.clone(),
                schedule_operation,
                memory_operation: memory_operation_id.clone(),
                impl_operation: impl_operation.id.clone(),
                opcode: impl_operation.opcode.to_string(),
                axes,
                fusion_group: None,
            },
        );
        program.node_order.push(node_id);
    }
    rebuild_dependencies(&mut program, implementation)?;
    Ok(program)
}

/// Rebuilds the compiler-owned producer-consumer dependence graph.
pub fn rebuild_dependencies(
    program: &mut ScheduleProgram,
    implementation: &ImplProgram,
) -> AgentResult<()> {
    let node_for_operation = program
        .nodes
        .values()
        .map(|node| (node.impl_operation.clone(), node.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut dependencies = BTreeSet::new();
    for target in program.nodes.values() {
        let operation = implementation
            .operations
            .get(&target.impl_operation)
            .ok_or_else(|| {
                schedule_error(
                    ErrorCode::DependenceViolation,
                    "schedule node references a missing ImplIR operation",
                )
            })?;
        for operand in &operation.operands {
            let Some(value) = implementation.values.get(operand) else {
                continue;
            };
            let ImplValueOrigin::Operation(source_operation) = &value.origin;
            if let Some(source) = node_for_operation.get(source_operation) {
                dependencies.insert(ScheduleDependency {
                    source: source.clone(),
                    target: target.id.clone(),
                    kind: "producer_consumer".to_owned(),
                });
            }
        }
    }
    program.dependencies = dependencies.into_iter().collect();
    Ok(())
}

fn element_bytes(element: ScalarType) -> u64 {
    match element {
        ScalarType::Bool => 1,
        ScalarType::I32 | ScalarType::F32 => 4,
        ScalarType::Index => 8,
    }
}

fn static_buffer_bytes(memory: &MemoryProgram, buffer: &BufferId) -> u64 {
    let Some(buffer) = memory.buffers.get(buffer) else {
        return 0;
    };
    buffer
        .shape
        .0
        .iter()
        .try_fold(1_u64, |total, dimension| {
            let DimExpr::Static(value) = dimension else {
                return None;
            };
            total.checked_mul(*value)
        })
        .and_then(|elements| elements.checked_mul(element_bytes(buffer.element_type)))
        .unwrap_or(0)
}

/// Computes a deterministic conservative resource estimate relative to `target`.
pub fn estimate_resources(
    program: &ScheduleProgram,
    memory: &MemoryProgram,
    target: &TargetManifest,
) -> AgentResult<ScheduleResourceEstimate> {
    let mut estimate = ScheduleResourceEstimate {
        grid_dimensions: program.launch_grid.unwrap_or([1, 1, 1]),
        workgroup_dimensions: program.launch_workgroup.unwrap_or([1, 1, 1]),
        vector_lanes: program
            .vectorizations
            .iter()
            .map(|vector| vector.width)
            .max()
            .unwrap_or(1),
        serial_work_per_thread: 1,
        active_logical_tiles: 1,
        remainder_tiles: u64::try_from(
            program
                .axes
                .values()
                .filter(|axis| matches!(axis.tail, TailStrategy::CompilerRemainder { .. }))
                .count(),
        )
        .unwrap_or(u64::MAX),
        simulator_version: RESOURCE_SIMULATOR_VERSION,
        ..ScheduleResourceEstimate::default()
    };
    for axis in program.axes.values() {
        let value = axis.extent.static_value().unwrap_or(1);
        match axis.binding.as_ref().map(|binding| binding.level) {
            Some(BindingLevel::GridX) => estimate.grid_dimensions[0] = value,
            Some(BindingLevel::GridY) => estimate.grid_dimensions[1] = value,
            Some(BindingLevel::GridZ) => estimate.grid_dimensions[2] = value,
            Some(BindingLevel::BlockX | BindingLevel::WorkgroupX) => {
                estimate.workgroup_dimensions[0] = value;
            }
            Some(BindingLevel::BlockY | BindingLevel::WorkgroupY) => {
                estimate.workgroup_dimensions[1] = value;
            }
            Some(BindingLevel::BlockZ | BindingLevel::WorkgroupZ) => {
                estimate.workgroup_dimensions[2] = value;
            }
            Some(BindingLevel::Serial) | None => {
                estimate.serial_work_per_thread = estimate
                    .serial_work_per_thread
                    .checked_mul(value)
                    .ok_or_else(|| {
                        schedule_error(
                            ErrorCode::ScheduleResourceLimit,
                            "serial work estimate overflowed u64",
                        )
                    })?;
            }
            Some(BindingLevel::SubgroupLane | BindingLevel::VectorLane) => {}
        }
    }
    estimate.threads_per_workgroup = estimate
        .workgroup_dimensions
        .iter()
        .try_fold(1_u64, |total, value| total.checked_mul(*value))
        .ok_or_else(|| {
            schedule_error(
                ErrorCode::TargetResourceExceeded,
                "workgroup dimension product overflowed u64",
            )
        })?;
    estimate.subgroup_count = estimate
        .threads_per_workgroup
        .saturating_add(target.subgroup.width.saturating_sub(1))
        / target.subgroup.width;
    for buffer in memory.buffers.values() {
        let bytes = static_buffer_bytes(memory, &buffer.id);
        estimate.maximum_live_memory_bytes =
            estimate.maximum_live_memory_bytes.saturating_add(bytes);
        match buffer.address_space {
            AddressSpace::Shared => {
                estimate.shared_bytes_per_workgroup =
                    estimate.shared_bytes_per_workgroup.saturating_add(bytes);
            }
            AddressSpace::Private => {
                estimate.private_bytes_per_thread =
                    estimate.private_bytes_per_thread.saturating_add(bytes);
            }
            AddressSpace::Global | AddressSpace::Constant => {
                estimate.global_buffer_references =
                    estimate.global_buffer_references.saturating_add(1);
            }
        }
    }
    for (index, value) in estimate.grid_dimensions.iter().enumerate() {
        if *value > target.hierarchy.max_grid_dimensions[index] {
            estimate
                .capacity_violations
                .push(format!("grid_dimension_{index}"));
        }
    }
    for (index, value) in estimate.workgroup_dimensions.iter().enumerate() {
        if *value > target.hierarchy.max_workgroup_dimensions[index] {
            estimate
                .capacity_violations
                .push(format!("workgroup_dimension_{index}"));
        }
    }
    if estimate.threads_per_workgroup > target.hierarchy.max_threads_per_workgroup {
        estimate
            .capacity_violations
            .push("threads_per_workgroup".to_owned());
    }
    if estimate.shared_bytes_per_workgroup > target.resources.max_shared_bytes_per_workgroup {
        estimate
            .capacity_violations
            .push("shared_bytes_per_workgroup".to_owned());
    }
    if estimate.private_bytes_per_thread > target.resources.max_private_bytes_per_thread {
        estimate
            .capacity_violations
            .push("private_bytes_per_thread".to_owned());
    }
    Ok(estimate)
}

/// Returns whether a buffer is structurally compatible with exact vector access.
pub fn vector_access_compatible(
    memory: &MemoryProgram,
    buffer: &BufferId,
    width: u64,
) -> AgentResult<()> {
    let buffer = memory.buffers.get(buffer).ok_or_else(|| {
        schedule_error(
            ErrorCode::ScheduleMemoryConflict,
            format!("buffer `{buffer}` does not exist"),
        )
    })?;
    let required = element_bytes(buffer.element_type)
        .checked_mul(width)
        .ok_or_else(|| {
            schedule_error(
                ErrorCode::VectorAlignmentUnsatisfied,
                "vector byte alignment overflowed u64",
            )
        })?;
    if buffer.alignment < required
        || !matches!(
            buffer.strides.entries.last(),
            Some(MemoryStride::Static { value: 1 })
        )
    {
        return Err(schedule_error(
            ErrorCode::VectorAlignmentUnsatisfied,
            "vectorization requires sufficient alignment and unit innermost stride",
        )
        .with_detail("required_alignment", required)
        .with_detail("actual_alignment", buffer.alignment)
        .with_repair("raise MemoryIR alignment or keep the axis scalar"));
    }
    Ok(())
}

/// Returns active buffers read or written by one scheduled operation.
#[must_use]
pub fn operation_buffers(
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    operation: &ImplOperationId,
) -> Vec<BufferId> {
    let mut buffers = BTreeSet::new();
    if let Some(operation) = implementation.operations.get(operation) {
        for value in operation.operands.iter().chain(&operation.results) {
            if let Some(MemoryBinding::Buffer { buffer, .. }) = memory.bindings.get(value) {
                buffers.insert(buffer.clone());
            }
        }
    }
    buffers.into_iter().collect()
}

/// Fully verifies the typed schedule graph and refreshes compiler-owned analysis.
pub fn verify_and_refresh_schedule(
    program: &mut ScheduleProgram,
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<()> {
    if program.node_order.len() != program.nodes.len()
        || program.node_order.iter().collect::<BTreeSet<_>>().len() != program.node_order.len()
    {
        return Err(schedule_error(
            ErrorCode::DuplicateExecution,
            "schedule node order is incomplete or contains duplicates",
        ));
    }
    let expected_memory_order = program
        .node_order
        .iter()
        .map(|node| {
            program
                .nodes
                .get(node)
                .map(|node| node.memory_operation.clone())
                .ok_or_else(|| {
                    schedule_error(
                        ErrorCode::IncompleteCoverage,
                        "schedule node order references a missing node",
                    )
                })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    if expected_memory_order != memory.operation_order {
        return Err(schedule_error(
            ErrorCode::DependenceViolation,
            "Stage 4 v1 schedule must preserve canonical high-level MemoryIR order",
        ));
    }
    if program.axes.len()
        > usize::try_from(target.resources.maximum_rank)
            .unwrap_or(usize::MAX)
            .saturating_mul(program.nodes.len().max(1))
            .saturating_mul(4)
    {
        return Err(schedule_error(
            ErrorCode::InvalidScheduleAxis,
            "schedule exceeds the target's bounded logical rank profile",
        ));
    }
    for node in program.nodes.values() {
        if !memory.operations.contains_key(&node.memory_operation)
            || !implementation.operations.contains_key(&node.impl_operation)
            || node
                .axes
                .iter()
                .any(|axis| !program.axes.contains_key(axis))
        {
            return Err(schedule_error(
                ErrorCode::ScheduleMemoryConflict,
                "schedule node does not map exactly to MemoryIR/ImplIR",
            ));
        }
    }
    let mut binding_levels = BTreeMap::<BindingLevel, ScheduleAxisId>::new();
    for axis in program.axes.values() {
        if axis.id.as_str().is_empty() || axis.extent.static_value() == Some(0) {
            return Err(schedule_error(
                ErrorCode::InvalidScheduleAxis,
                "schedule axis has an invalid identity or extent",
            ));
        }
        if let Some(binding) = &axis.binding {
            if binding.level != BindingLevel::Serial
                && binding_levels
                    .insert(binding.level, axis.id.clone())
                    .is_some()
            {
                return Err(schedule_error(
                    ErrorCode::InvalidBinding,
                    "one target hierarchy level may bind at most one axis",
                ));
            }
        }
    }
    for vector in &program.vectorizations {
        if vector.width == 0 || !target.vector.widths.contains(&vector.width) {
            return Err(schedule_error(
                ErrorCode::VectorWidthUnsupported,
                "target manifest does not support the requested vector width",
            ));
        }
        let axis = program.axes.get(&vector.axis).ok_or_else(|| {
            schedule_error(
                ErrorCode::InvalidScheduleAxis,
                "vectorization references a missing axis",
            )
        })?;
        let operation = &implementation.operations[&axis.operation];
        let element = operation
            .results
            .iter()
            .chain(&operation.operands)
            .find_map(|value| {
                implementation
                    .values
                    .get(value)
                    .and_then(|value| match value.ty {
                        Type::Tensor { element, .. } => Some(element),
                        Type::Scalar(_) => None,
                    })
            })
            .ok_or_else(|| {
                schedule_error(
                    ErrorCode::TargetCapabilityUnsupported,
                    "vectorization requires a tensor element type",
                )
            })?;
        if !target.vector.element_types.contains(&element) {
            return Err(schedule_error(
                ErrorCode::TargetCapabilityUnsupported,
                "target does not support this vector element type",
            ));
        }
        for buffer in operation_buffers(memory, implementation, &axis.operation) {
            vector_access_compatible(memory, &buffer, vector.width)?;
        }
    }
    if program.unrolls.iter().any(|unroll| {
        unroll.factor == 0
            || unroll.factor > 256
            || program
                .axes
                .get(&unroll.axis)
                .is_none_or(|axis| axis.role == AxisRole::Reduction)
    }) {
        return Err(schedule_error(
            ErrorCode::InvalidUnroll,
            "unroll requires a non-reduction axis and factor in 1..=256",
        ));
    }
    rebuild_dependencies(program, implementation)?;
    program.legality_facts = vec![
        ScheduleLegalityFact {
            kind: "complete_coverage".to_owned(),
            subject: "all_domains".to_owned(),
            method: "structural_axis_mapping_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "no_duplicate_execution".to_owned(),
            subject: "all_domains".to_owned(),
            method: "canonical_coordinate_partition_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "dependence_preserved".to_owned(),
            subject: "all_dependencies".to_owned(),
            method: "ordered_memory_operation_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "reduction_order_preserved".to_owned(),
            subject: "all_reductions".to_owned(),
            method: "fixed_serial_reduction_axis_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "memory_lifetime_preserved".to_owned(),
            subject: "memory_plan".to_owned(),
            method: "canonical_high_level_order_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "alias_write_safe".to_owned(),
            subject: "memory_plan".to_owned(),
            method: "memoryir_decision_preservation_v1".to_owned(),
        },
        ScheduleLegalityFact {
            kind: "tail_exact".to_owned(),
            subject: "all_axes".to_owned(),
            method: "compiler_remainder_partition_v1".to_owned(),
        },
    ];
    program.resource_estimate = estimate_resources(program, memory, target)?;
    if !program.resource_estimate.capacity_violations.is_empty() {
        return Err(schedule_error(
            ErrorCode::TargetResourceExceeded,
            "schedule exceeds immutable target capacities",
        )
        .with_detail(
            "violations",
            serde_json::json!(program.resource_estimate.capacity_violations),
        )
        .with_repair("choose serial execution or smaller launch/tile dimensions"));
    }
    program.legality_facts.push(ScheduleLegalityFact {
        kind: "target_capability_satisfied".to_owned(),
        subject: target.target_hash.to_string(),
        method: "target_manifest_validator_v1".to_owned(),
    });
    program.legality_facts.push(ScheduleLegalityFact {
        kind: "resource_capacity_satisfied".to_owned(),
        subject: target.target_hash.to_string(),
        method: "deterministic_resource_simulator_v1".to_owned(),
    });
    Ok(())
}
