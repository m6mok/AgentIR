//! Typed physical buffer regions, deterministic bufferization, and memory verification.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{AliasDomainId, BufferId, ImplOperationId, ImplValueId, MemoryOperationId},
    impl_ir::{ImplProgram, ImplValueOrigin},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    types::{DimExpr, ScalarType, Shape, Type},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Current MemoryIR structural/evaluation semantics version.
pub const MEMORY_SEMANTICS_VERSION: u32 = 1;
/// Current MemoryIR validator implementation version.
pub const MEMORY_VALIDATOR_VERSION: u32 = 1;
/// Current deterministic memory trace codec version.
pub const MEMORY_TRACE_CODEC_VERSION: u32 = 1;

/// Abstract typed address space; no variant is a raw pointer space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressSpace {
    /// Externally visible or ordinary plan-owned storage.
    Global,
    /// Abstract workgroup-local storage without a target capacity claim.
    Shared,
    /// Abstract private storage without a target register/local-memory claim.
    Private,
    /// Immutable constant storage.
    Constant,
}

/// Legal access mode for one typed buffer region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// Reads are permitted and writes are forbidden.
    ReadOnly,
    /// Writes are permitted and reads are forbidden.
    WriteOnly,
    /// Both reads and writes are permitted.
    ReadWrite,
}

/// Ownership of one abstract buffer region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// Storage is borrowed from an external caller.
    ExternalBorrowed,
    /// Storage is allocated and owned by this memory plan.
    PlanOwned,
    /// Immutable storage is owned by the constant pool.
    ConstantOwned,
    /// The region is a typed view into another owned region.
    View,
}

/// Exact symbolic stride expression in logical elements.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryStride {
    /// Fully static logical-element stride.
    Static {
        /// Non-zero stride in logical elements.
        value: u64,
    },
    /// Exact ordered product of logical dimensions.
    Product {
        /// Shape factors whose product is the stride.
        factors: Vec<DimExpr>,
    },
}

/// Ordered exact strides, one per logical tensor dimension.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MemoryStrides {
    /// Outer-to-inner logical-element strides.
    pub entries: Vec<MemoryStride>,
}

/// Small explicit Stage 3 physical layout family.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryLayout {
    /// Dense row-major tensor storage.
    ContiguousRowMajor,
    /// Explicit exact logical-element strides.
    ExplicitStrided {
        /// Exact stride vector.
        strides: MemoryStrides,
    },
}

/// Compiler-derived lifetime in canonical logical operation order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifetime {
    /// Definition or external bind point.
    pub first_point: u64,
    /// Ordered logical operation use points.
    pub uses: Vec<u64>,
    /// Last logical use, including output escape.
    pub last_use: u64,
    /// Whether an external output keeps the region alive.
    pub output_escape: bool,
    /// Whether the caller owns the region lifetime.
    pub external: bool,
    /// Whether release after `last_use` is legal.
    pub deallocation_eligible: bool,
}

/// Proven relation between two abstract regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasRelation {
    /// Both names denote the same complete region.
    MustAlias,
    /// The regions are compiler-proved disjoint.
    NoAlias,
    /// The compiler cannot statically rule out overlap.
    MayAlias,
    /// The regions overlap, but neither denotes the complete other region.
    PartialOverlap,
}

/// Compiler-owned provenance for an alias fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasProvenance {
    /// The typed region model proves the relation.
    ProvedFromType,
    /// Fresh region construction proves the relation.
    ProvedFromRegionConstruction,
    /// Non-overlapping logical lifetimes prove the relation.
    ProvedFromLifetime,
    /// A trusted external binding contract proves the relation.
    ProvedFromExternalContract,
    /// A compiler-owned runtime predicate establishes the relation on one branch.
    RuntimeGuard,
    /// Untrusted metadata retained for audit only; never a proof.
    UnverifiedClaim,
}

/// One deterministic alias fact in canonical buffer-ID order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasFact {
    /// Lexicographically first buffer.
    pub first: BufferId,
    /// Lexicographically second buffer.
    pub second: BufferId,
    /// Relation between the two regions.
    pub relation: AliasRelation,
    /// Compiler-owned proof provenance.
    pub provenance: AliasProvenance,
}

/// One alias-domain record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasDomain {
    /// Compiler-assigned domain identity.
    pub id: AliasDomainId,
    /// Stable construction provenance.
    pub provenance: AliasProvenance,
}

/// One typed abstract buffer region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryBuffer {
    /// Compiler-assigned buffer identity.
    pub id: BufferId,
    /// Logical scalar element type.
    pub element_type: ScalarType,
    /// Immutable logical tensor shape.
    pub shape: Shape,
    /// Explicit physical layout family.
    pub layout: MemoryLayout,
    /// Exact logical-element strides.
    pub strides: MemoryStrides,
    /// Abstract typed address space.
    pub address_space: AddressSpace,
    /// Legal access mode.
    pub access: AccessMode,
    /// Required power-of-two byte alignment.
    pub alignment: u64,
    /// Compiler-owned alias domain.
    pub alias_domain: AliasDomainId,
    /// Compiler-derived logical lifetime.
    pub lifetime: Lifetime,
    /// Region ownership.
    pub ownership: Ownership,
    /// Optional external parameter/output binding name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_binding: Option<String>,
    /// Original ImplIR value materialized by this region.
    pub source_value: ImplValueId,
    /// Logical element offset within the abstract region.
    pub offset_elements: u64,
    /// Deterministic compiler provenance label.
    pub provenance: String,
}

/// Scalar SSA or tensor-buffer binding for one ImplIR value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryBinding {
    /// Scalar remains functional SSA and is not forced into storage.
    Scalar {
        /// Logical ImplIR value.
        value: ImplValueId,
    },
    /// Tensor is materialized in one typed buffer region.
    Buffer {
        /// Logical ImplIR value.
        value: ImplValueId,
        /// Physical abstract region.
        buffer: BufferId,
    },
}

impl MemoryBinding {
    /// Returns the bound logical value.
    #[must_use]
    pub fn value(&self) -> &ImplValueId {
        match self {
            Self::Scalar { value } | Self::Buffer { value, .. } => value,
        }
    }

    /// Returns the bound buffer for tensor storage.
    #[must_use]
    pub fn buffer(&self) -> Option<&BufferId> {
        match self {
            Self::Scalar { .. } => None,
            Self::Buffer { buffer, .. } => Some(buffer),
        }
    }
}

/// Read or write performed by one high-level semantic operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferAccessKind {
    /// Typed indexed read.
    Read,
    /// Typed indexed write.
    Write,
    /// A single operation reads and overwrites the same region.
    ReadWrite,
}

/// Explicit typed access to one abstract buffer region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferAccess {
    /// Accessed region.
    pub buffer: BufferId,
    /// Logical value read or produced.
    pub value: ImplValueId,
    /// Access kind.
    pub kind: BufferAccessKind,
    /// Typed index profile; Stage 3 retains the high-level logical index space.
    pub typed_index: String,
}

/// One high-level semantic operation with explicit storage bindings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryOperation {
    /// Compiler-assigned memory operation identity.
    pub id: MemoryOperationId,
    /// Immutable source ImplIR operation.
    pub impl_operation: ImplOperationId,
    /// Stable semantic opcode spelling.
    pub opcode: String,
    /// Ordered logical operand bindings.
    pub operands: Vec<MemoryBinding>,
    /// Ordered logical result bindings.
    pub results: Vec<MemoryBinding>,
    /// Explicit typed buffer accesses.
    pub accesses: Vec<BufferAccess>,
}

/// Physical storage choice for one tensor result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReuseDecision {
    /// Conservative exact baseline using a fresh plan-owned region.
    Fresh {
        /// Fresh result region.
        buffer: BufferId,
    },
    /// Statically proved overwrite of a dead writable input region.
    InPlace {
        /// Logical input whose storage is reused.
        input: ImplValueId,
        /// Reused physical region.
        buffer: BufferId,
        /// Stable compiler-owned certificate label.
        certificate: String,
    },
    /// Compiler-owned NoOverlap guard selecting reuse or an exact fresh fallback.
    Guarded {
        /// Logical input whose storage is reused on the true branch.
        input: ImplValueId,
        /// Reused primary region.
        buffer: BufferId,
        /// Compiler-owned normalized guard.
        guard: Box<crate::memory::MemoryGuard>,
        /// Immutable fresh false-branch plan fragment.
        fallback: Box<crate::memory::MemoryFallback>,
        /// Stable compiler-owned certificate label.
        certificate: String,
    },
}

/// Separate typed MemoryIR program preserving high-level operation identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProgram {
    /// One storage/SSA binding for every reachable ImplIR value.
    pub bindings: BTreeMap<ImplValueId, MemoryBinding>,
    /// Active abstract buffer regions.
    pub buffers: BTreeMap<BufferId, MemoryBuffer>,
    /// Immutable fresh templates retained as exact fallbacks.
    pub fresh_baseline_buffers: BTreeMap<ImplValueId, MemoryBuffer>,
    /// High-level memory operations by ID.
    pub operations: BTreeMap<MemoryOperationId, MemoryOperation>,
    /// Canonical logical operation order.
    pub operation_order: Vec<MemoryOperationId>,
    /// Immutable source-operation to memory-operation mapping.
    pub operation_for_impl: BTreeMap<ImplOperationId, MemoryOperationId>,
    /// Named observable output bindings.
    pub outputs: BTreeMap<String, MemoryBinding>,
    /// Alias domains retained by the plan.
    pub alias_domains: BTreeMap<AliasDomainId, AliasDomain>,
    /// Canonically sorted compiler-owned alias facts.
    pub alias_facts: Vec<AliasFact>,
    /// Explicit result storage decisions.
    pub reuse_decisions: BTreeMap<ImplValueId, ReuseDecision>,
}

fn memory_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn element_bytes(element: ScalarType) -> u64 {
    match element {
        ScalarType::Bool => 1,
        ScalarType::I32 | ScalarType::F32 => 4,
        ScalarType::Index => 8,
    }
}

fn stride_product(factors: &[DimExpr]) -> AgentResult<MemoryStride> {
    if factors.is_empty() {
        return Ok(MemoryStride::Static { value: 1 });
    }
    let mut product = 1_u64;
    for factor in factors {
        let DimExpr::Static(value) = factor else {
            return Ok(MemoryStride::Product {
                factors: factors.to_vec(),
            });
        };
        if *value == 0 {
            return Err(memory_error(
                ErrorCode::InvalidMemoryLayout,
                "zero-sized static tensor allocation is unsupported in MemoryIR v1",
            ));
        }
        product = product.checked_mul(*value).ok_or_else(|| {
            memory_error(
                ErrorCode::InvalidMemoryLayout,
                "row-major stride computation overflowed u64",
            )
        })?;
    }
    Ok(MemoryStride::Static { value: product })
}

/// Computes exact contiguous row-major logical-element strides.
pub fn contiguous_strides(shape: &Shape) -> AgentResult<MemoryStrides> {
    let mut entries = Vec::with_capacity(shape.0.len());
    for index in 0..shape.0.len() {
        entries.push(stride_product(&shape.0[index + 1..])?);
    }
    Ok(MemoryStrides { entries })
}

fn static_allocation_bytes(buffer: &MemoryBuffer) -> AgentResult<Option<u64>> {
    let mut elements = 1_u64;
    for dimension in &buffer.shape.0 {
        let DimExpr::Static(extent) = dimension else {
            return Ok(None);
        };
        if *extent == 0 {
            return Err(memory_error(
                ErrorCode::InvalidMemoryLayout,
                "zero-sized static tensor buffer is unsupported",
            ));
        }
        elements = elements.checked_mul(*extent).ok_or_else(|| {
            memory_error(
                ErrorCode::InvalidMemoryLayout,
                "buffer element count overflowed u64",
            )
        })?;
    }
    elements
        .checked_mul(element_bytes(buffer.element_type))
        .map(Some)
        .ok_or_else(|| {
            memory_error(
                ErrorCode::InvalidMemoryLayout,
                "buffer allocation byte count overflowed u64",
            )
        })
}

pub(crate) fn reachable_impl_operations(
    program: &ImplProgram,
) -> AgentResult<BTreeSet<ImplOperationId>> {
    fn visit(
        program: &ImplProgram,
        value: &ImplValueId,
        visiting: &mut BTreeSet<ImplOperationId>,
        reached: &mut BTreeSet<ImplOperationId>,
    ) -> AgentResult<()> {
        let definition = program.values.get(value).ok_or_else(|| {
            memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                format!("reachable ImplIR value `{value}` is missing"),
            )
        })?;
        let ImplValueOrigin::Operation(operation) = &definition.origin;
        if reached.contains(operation) {
            return Ok(());
        }
        if !visiting.insert(operation.clone()) {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "ImplIR dependency cycle reached during bufferization",
            ));
        }
        let definition = program.operations.get(operation).ok_or_else(|| {
            memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "reachable ImplIR operation is missing",
            )
        })?;
        for operand in &definition.operands {
            visit(program, operand, visiting, reached)?;
        }
        if let Some(region) = &definition.region {
            for capture in &region.captures {
                visit(program, capture, visiting, reached)?;
            }
        }
        visiting.remove(operation);
        reached.insert(operation.clone());
        Ok(())
    }
    let mut reached = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    for value in program
        .parameters
        .values()
        .chain(program.outputs.values().map(|output| &output.value))
    {
        visit(program, value, &mut visiting, &mut reached)?;
    }
    Ok(reached)
}

fn value_definition_point(program: &ImplProgram, value: &ImplValueId) -> u64 {
    let Some(definition) = program.values.get(value) else {
        return 0;
    };
    let ImplValueOrigin::Operation(operation) = &definition.origin;
    program
        .operation_order
        .iter()
        .position(|candidate| candidate == operation)
        .and_then(|index| u64::try_from(index).ok())
        .unwrap_or(0)
        .saturating_add(1)
}

fn value_uses(program: &ImplProgram, value: &ImplValueId) -> Vec<u64> {
    let mut uses = Vec::new();
    for (index, operation) in program.operation_order.iter().enumerate() {
        if program.operations.get(operation).is_some_and(|operation| {
            operation.operands.contains(value)
                || operation
                    .region
                    .as_ref()
                    .is_some_and(|region| region.captures.contains(value))
        }) {
            uses.push(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1));
        }
    }
    if program
        .outputs
        .values()
        .any(|output| &output.value == value)
    {
        uses.push(
            u64::try_from(program.operation_order.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        );
    }
    uses.sort_unstable();
    uses.dedup();
    uses
}

fn recompute_lifetimes(memory: &mut MemoryProgram, implementation: &ImplProgram) {
    let mut values_by_buffer = BTreeMap::<BufferId, Vec<ImplValueId>>::new();
    for (value, binding) in &memory.bindings {
        if let Some(buffer) = binding.buffer() {
            values_by_buffer
                .entry(buffer.clone())
                .or_default()
                .push(value.clone());
        }
    }
    for (buffer_id, buffer) in &mut memory.buffers {
        let values = values_by_buffer.get(buffer_id).cloned().unwrap_or_default();
        let mut uses = values
            .iter()
            .flat_map(|value| value_uses(implementation, value))
            .collect::<Vec<_>>();
        uses.sort_unstable();
        uses.dedup();
        let first_point = values
            .iter()
            .map(|value| value_definition_point(implementation, value))
            .min()
            .unwrap_or(0);
        let output_escape = values.iter().any(|value| {
            implementation
                .outputs
                .values()
                .any(|output| &output.value == value)
        });
        let external = buffer.ownership == Ownership::ExternalBorrowed;
        buffer.lifetime = Lifetime {
            first_point,
            last_use: uses.last().copied().unwrap_or(first_point),
            uses,
            output_escape,
            external,
            deallocation_eligible: !external && !output_escape,
        };
    }
}

fn recompute_alias_facts(memory: &mut MemoryProgram) {
    let buffers = memory.buffers.keys().cloned().collect::<Vec<_>>();
    let mut facts = Vec::new();
    for (index, first) in buffers.iter().enumerate() {
        for second in &buffers[index..] {
            let left = &memory.buffers[first];
            let right = &memory.buffers[second];
            let (relation, provenance) = if first == second {
                (AliasRelation::MustAlias, AliasProvenance::ProvedFromType)
            } else if left.alias_domain == right.alias_domain {
                (
                    AliasRelation::MayAlias,
                    AliasProvenance::ProvedFromRegionConstruction,
                )
            } else if left.ownership == Ownership::ExternalBorrowed
                && right.ownership == Ownership::ExternalBorrowed
            {
                (
                    AliasRelation::MayAlias,
                    AliasProvenance::ProvedFromExternalContract,
                )
            } else {
                (
                    AliasRelation::NoAlias,
                    AliasProvenance::ProvedFromRegionConstruction,
                )
            };
            facts.push(AliasFact {
                first: first.clone(),
                second: second.clone(),
                relation,
                provenance,
            });
        }
    }
    memory.alias_facts = facts;
}

fn rebuild_operations(memory: &mut MemoryProgram, implementation: &ImplProgram) -> AgentResult<()> {
    for operation_id in &memory.operation_order {
        let operation = memory.operations.get_mut(operation_id).ok_or_else(|| {
            memory_error(
                ErrorCode::InvalidMemoryAccess,
                "memory operation order references a missing operation",
            )
        })?;
        let source = implementation
            .operations
            .get(&operation.impl_operation)
            .ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "memory operation references a missing ImplIR operation",
                )
            })?;
        operation.operands = source
            .operands
            .iter()
            .map(|value| memory.bindings.get(value).cloned())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "memory operation operand binding is missing",
                )
            })?;
        operation.results = source
            .results
            .iter()
            .map(|value| memory.bindings.get(value).cloned())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "memory operation result binding is missing",
                )
            })?;
        let result_buffers = operation
            .results
            .iter()
            .filter_map(MemoryBinding::buffer)
            .cloned()
            .collect::<BTreeSet<_>>();
        let operand_buffers = operation
            .operands
            .iter()
            .filter_map(MemoryBinding::buffer)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut accesses = Vec::new();
        for binding in &operation.operands {
            if let Some(buffer) = binding.buffer() {
                accesses.push(BufferAccess {
                    buffer: buffer.clone(),
                    value: binding.value().clone(),
                    kind: if result_buffers.contains(buffer) {
                        BufferAccessKind::ReadWrite
                    } else {
                        BufferAccessKind::Read
                    },
                    typed_index: "logical_index".to_owned(),
                });
            }
        }
        for binding in &operation.results {
            if let Some(buffer) = binding.buffer() {
                if !operand_buffers.contains(buffer) {
                    accesses.push(BufferAccess {
                        buffer: buffer.clone(),
                        value: binding.value().clone(),
                        kind: BufferAccessKind::Write,
                        typed_index: "logical_index".to_owned(),
                    });
                }
            }
        }
        operation.accesses = accesses;
    }
    memory.outputs = implementation
        .outputs
        .iter()
        .map(|(name, output)| {
            memory
                .bindings
                .get(&output.value)
                .cloned()
                .map(|binding| (name.clone(), binding))
                .ok_or_else(|| {
                    memory_error(
                        ErrorCode::MemoryEquivalenceUnproved,
                        "memory output binding is missing",
                    )
                })
        })
        .collect::<AgentResult<_>>()?;
    Ok(())
}

/// Recomputes compiler-owned operation accesses, lifetimes, and alias facts.
pub(crate) fn rebuild_analysis(
    memory: &mut MemoryProgram,
    implementation: &ImplProgram,
) -> AgentResult<()> {
    rebuild_operations(memory, implementation)?;
    recompute_lifetimes(memory, implementation);
    recompute_alias_facts(memory);
    Ok(())
}

/// Returns the compiler-owned alias fact for two buffers.
pub fn alias_relation(
    memory: &MemoryProgram,
    first: &BufferId,
    second: &BufferId,
) -> AgentResult<AliasFact> {
    if !memory.buffers.contains_key(first) || !memory.buffers.contains_key(second) {
        return Err(memory_error(
            ErrorCode::BufferNotFound,
            "alias query references a missing buffer",
        ));
    }
    let (left, right) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    memory
        .alias_facts
        .iter()
        .find(|fact| &fact.first == left && &fact.second == right)
        .cloned()
        .ok_or_else(|| {
            memory_error(
                ErrorCode::AliasProofMissing,
                "compiler-owned alias fact is missing",
            )
        })
}

/// Returns the active typed buffer bound to one tensor value.
pub fn buffer_of<'a>(
    memory: &'a MemoryProgram,
    value: &ImplValueId,
) -> AgentResult<&'a MemoryBuffer> {
    let buffer = memory
        .bindings
        .get(value)
        .and_then(MemoryBinding::buffer)
        .ok_or_else(|| {
            memory_error(
                ErrorCode::BufferNotFound,
                "value has no active tensor buffer",
            )
        })?;
    memory.buffers.get(buffer).ok_or_else(|| {
        memory_error(
            ErrorCode::BufferNotFound,
            "value binding references a missing active buffer",
        )
    })
}

/// Returns the compiler-derived logical lifetime for one active buffer.
pub fn lifetime_of<'a>(memory: &'a MemoryProgram, buffer: &BufferId) -> AgentResult<&'a Lifetime> {
    memory
        .buffers
        .get(buffer)
        .map(|buffer| &buffer.lifetime)
        .ok_or_else(|| memory_error(ErrorCode::BufferNotFound, "lifetime buffer does not exist"))
}

/// Returns the last canonical logical use point of one reachable ImplIR value.
#[must_use]
pub fn last_use(implementation: &ImplProgram, value: &ImplValueId) -> Option<u64> {
    value_uses(implementation, value).last().copied()
}

/// Returns whether compiler-owned alias facts permit the two regions to overlap.
pub fn may_overlap(
    memory: &MemoryProgram,
    first: &BufferId,
    second: &BufferId,
) -> AgentResult<bool> {
    Ok(matches!(
        alias_relation(memory, first, second)?.relation,
        AliasRelation::MustAlias | AliasRelation::MayAlias | AliasRelation::PartialOverlap
    ))
}

/// Reuses the full structural proof path to answer whether one input can store one result.
#[must_use]
pub fn can_reuse(
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    input: &ImplValueId,
    result: &ImplValueId,
) -> bool {
    prove_static_reuse(memory, implementation, input, result).is_ok()
}

/// Returns the required power-of-two byte alignment for one active buffer.
pub fn required_alignment(memory: &MemoryProgram, buffer: &BufferId) -> AgentResult<u64> {
    memory
        .buffers
        .get(buffer)
        .map(|buffer| buffer.alignment)
        .ok_or_else(|| memory_error(ErrorCode::BufferNotFound, "alignment buffer does not exist"))
}

fn operation_point_for_result(
    implementation: &ImplProgram,
    result: &ImplValueId,
) -> AgentResult<(ImplOperationId, u64)> {
    let definition = implementation.values.get(result).ok_or_else(|| {
        memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "reuse result does not exist in ImplIR",
        )
    })?;
    let ImplValueOrigin::Operation(operation) = &definition.origin;
    let point = implementation
        .operation_order
        .iter()
        .position(|candidate| candidate == operation)
        .and_then(|index| u64::try_from(index).ok())
        .ok_or_else(|| {
            memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "reuse result definition is absent from operation order",
            )
        })?
        .saturating_add(1);
    Ok((operation.clone(), point))
}

/// Proves the static side conditions for reusing one input region for one result.
pub fn prove_static_reuse(
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    input: &ImplValueId,
    result: &ImplValueId,
) -> AgentResult<Vec<String>> {
    let input_type = implementation.values.get(input).map(|value| &value.ty);
    let result_type = implementation.values.get(result).map(|value| &value.ty);
    if input_type.is_none()
        || input_type != result_type
        || !matches!(input_type, Some(Type::Tensor { .. }))
    {
        return Err(memory_error(
            ErrorCode::InPlaceReuseUnsafe,
            "in-place reuse requires identical tensor element type and shape",
        )
        .with_repair("choose fresh allocation"));
    }
    let input_buffer_id = memory
        .bindings
        .get(input)
        .and_then(MemoryBinding::buffer)
        .ok_or_else(|| memory_error(ErrorCode::BufferNotFound, "reuse input has no buffer"))?;
    let fresh_result = memory.fresh_baseline_buffers.get(result).ok_or_else(|| {
        memory_error(
            ErrorCode::BufferNotFound,
            "reuse result has no fresh baseline buffer",
        )
    })?;
    let input_buffer = memory.buffers.get(input_buffer_id).ok_or_else(|| {
        memory_error(
            ErrorCode::BufferNotFound,
            "reuse input buffer is not active",
        )
    })?;
    if input_buffer.layout != fresh_result.layout
        || input_buffer.strides != fresh_result.strides
        || input_buffer.alignment < fresh_result.alignment
    {
        return Err(memory_error(
            ErrorCode::InPlaceReuseUnsafe,
            "reuse input layout, strides, or alignment are incompatible",
        )
        .with_repair("choose fresh allocation or restore a compatible layout"));
    }
    if input_buffer.ownership != Ownership::PlanOwned
        || !matches!(
            input_buffer.access,
            AccessMode::WriteOnly | AccessMode::ReadWrite
        )
    {
        return Err(memory_error(
            ErrorCode::InPlaceReuseUnsafe,
            "reuse input is not writable plan-owned storage",
        )
        .with_repair("choose fresh allocation"));
    }
    let (operation_id, point) = operation_point_for_result(implementation, result)?;
    let operation = &implementation.operations[&operation_id];
    if !operation.operands.contains(input) {
        return Err(memory_error(
            ErrorCode::InPlaceReuseUnsafe,
            "reuse input is not an operand of the result-defining operation",
        ));
    }
    let logical_input_uses = value_uses(implementation, input);
    let logical_input_last_use = logical_input_uses.last().copied().unwrap_or(0);
    let logical_input_escapes = implementation
        .outputs
        .values()
        .any(|output| &output.value == input);
    if logical_input_last_use != point || logical_input_escapes {
        return Err(memory_error(
            ErrorCode::LifetimeViolation,
            "reuse input remains live after the overwrite point",
        )
        .with_detail("overwrite_point", point)
        .with_detail("last_use", logical_input_last_use)
        .with_repair("choose fresh allocation"));
    }
    for other in memory.buffers.values() {
        if other.id == input_buffer.id {
            continue;
        }
        let fact = alias_relation(memory, &input_buffer.id, &other.id)?;
        if fact.provenance == AliasProvenance::UnverifiedClaim
            || (matches!(
                fact.relation,
                AliasRelation::MayAlias | AliasRelation::PartialOverlap
            ) && other.lifetime.first_point <= point
                && other.lifetime.last_use >= point)
        {
            return Err(memory_error(
                ErrorCode::AliasProofMissing,
                "a live reader may overlap the requested overwrite region",
            )
            .with_detail("other_buffer", other.id.to_string())
            .with_repair("choose fresh allocation or request compiler-owned guarded reuse"));
        }
    }
    Ok(vec![
        "identical element type and shape".to_owned(),
        "compatible layout, strides, and alignment".to_owned(),
        "writable plan-owned input".to_owned(),
        "input last use equals overwrite point".to_owned(),
        "no live overlapping reader".to_owned(),
    ])
}

fn verify_layout(buffer: &MemoryBuffer, limits: &ResourceLimits) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryLayoutEntries,
        u64::try_from(buffer.strides.entries.len()).unwrap_or(u64::MAX),
        "MemoryIR buffer layout",
    )?;
    if buffer.strides.entries.len() != buffer.shape.0.len() {
        return Err(memory_error(
            ErrorCode::InvalidMemoryLayout,
            "stride count differs from tensor rank",
        ));
    }
    if buffer.alignment == 0 || !buffer.alignment.is_power_of_two() {
        return Err(memory_error(
            ErrorCode::AlignmentUnsatisfied,
            "buffer alignment must be a non-zero power of two",
        ));
    }
    if buffer.alignment < element_bytes(buffer.element_type) {
        return Err(memory_error(
            ErrorCode::AlignmentUnsatisfied,
            "buffer alignment is smaller than its element size",
        ));
    }
    let contiguous = contiguous_strides(&buffer.shape)?;
    match &buffer.layout {
        MemoryLayout::ContiguousRowMajor if buffer.strides != contiguous => Err(memory_error(
            ErrorCode::InvalidMemoryLayout,
            "contiguous row-major buffer has non-canonical strides",
        )),
        MemoryLayout::ExplicitStrided { strides } if strides != &buffer.strides => {
            Err(memory_error(
                ErrorCode::InvalidMemoryLayout,
                "explicit layout and buffer stride fields disagree",
            ))
        }
        _ => Ok(()),
    }
}

/// Fully verifies MemoryIR types, accesses, lifetimes, aliases, and resource bounds.
pub fn verify_memory_program(
    memory: &MemoryProgram,
    implementation: &ImplProgram,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryBuffersPerPlan,
        u64::try_from(memory.buffers.len()).unwrap_or(u64::MAX),
        "MemoryIR verification",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryOperationsPerPlan,
        u64::try_from(memory.operations.len()).unwrap_or(u64::MAX),
        "MemoryIR verification",
    )?;
    let accesses = memory.operations.values().fold(0_u64, |total, operation| {
        total.saturating_add(u64::try_from(operation.accesses.len()).unwrap_or(u64::MAX))
    });
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryAccessesPerPlan,
        accesses,
        "MemoryIR verification",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryAliasDomains,
        u64::try_from(memory.alias_domains.len()).unwrap_or(u64::MAX),
        "MemoryIR verification",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryAliasFacts,
        u64::try_from(memory.alias_facts.len()).unwrap_or(u64::MAX),
        "MemoryIR verification",
    )?;
    let lifetime_uses = memory.buffers.values().fold(0_u64, |total, buffer| {
        total.saturating_add(u64::try_from(buffer.lifetime.uses.len()).unwrap_or(u64::MAX))
    });
    let lifetime_points = memory.buffers.values().fold(0_u64, |total, buffer| {
        total
            .saturating_add(2)
            .saturating_add(u64::from(buffer.lifetime.output_escape))
    });
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryLifetimePoints,
        lifetime_points,
        "MemoryIR verification",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryLifetimeUses,
        lifetime_uses,
        "MemoryIR verification",
    )?;

    let reached = reachable_impl_operations(implementation)?;
    let expected_values = reached
        .iter()
        .flat_map(|operation| implementation.operations[operation].results.iter().cloned())
        .collect::<BTreeSet<_>>();
    if memory.bindings.keys().cloned().collect::<BTreeSet<_>>() != expected_values {
        return Err(memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "MemoryIR bindings do not cover exactly the reachable ImplIR values",
        ));
    }
    let mut total_bytes = 0_u64;
    for (id, buffer) in &memory.buffers {
        if buffer.id != *id || !memory.alias_domains.contains_key(&buffer.alias_domain) {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "buffer identity or alias domain is inconsistent",
            ));
        }
        let Type::Tensor { element, shape } = &implementation
            .values
            .get(&buffer.source_value)
            .ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "buffer source value is missing",
                )
            })?
            .ty
        else {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "scalar ImplIR value was materialized as a tensor buffer",
            ));
        };
        if *element != buffer.element_type || *shape != buffer.shape {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "buffer element type or logical shape differs from ImplIR",
            ));
        }
        verify_layout(buffer, limits)?;
        if let Some(bytes) = static_allocation_bytes(buffer)? {
            BudgetCheck::against(
                limits,
                ResourceKind::MemoryAllocationBytesPerBuffer,
                bytes,
                format!("MemoryIR buffer `{id}`"),
            )?;
            total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryResourceLimit,
                    "total abstract allocation bytes overflowed u64",
                )
            })?;
        }
        if buffer.ownership == Ownership::ExternalBorrowed
            && (buffer.access != AccessMode::ReadOnly || buffer.external_binding.is_none())
        {
            return Err(memory_error(
                ErrorCode::InvalidMemoryAccess,
                "external borrowed input must be named and read-only",
            ));
        }
        if buffer.ownership == Ownership::ConstantOwned
            && (buffer.access != AccessMode::ReadOnly
                || buffer.address_space != AddressSpace::Constant)
        {
            return Err(memory_error(
                ErrorCode::InvalidMemoryAccess,
                "constant-owned buffer must be read-only constant storage",
            ));
        }
    }
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryTotalAllocationBytes,
        total_bytes,
        "MemoryIR total abstract allocation",
    )?;
    for (value, binding) in &memory.bindings {
        if binding.value() != value {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory binding map key differs from embedded value",
            ));
        }
        match (&implementation.values[value].ty, binding) {
            (Type::Scalar(_), MemoryBinding::Scalar { .. }) => {}
            (Type::Tensor { .. }, MemoryBinding::Buffer { buffer, .. })
                if memory.buffers.contains_key(buffer) => {}
            _ => {
                return Err(memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "MemoryIR scalar/buffer binding kind differs from ImplIR type",
                ));
            }
        }
    }
    if memory.operation_order.len() != memory.operations.len()
        || memory.operation_order.iter().collect::<BTreeSet<_>>().len()
            != memory.operation_order.len()
    {
        return Err(memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "memory operation order is incomplete or duplicated",
        ));
    }
    let mut expected = memory.clone();
    rebuild_analysis(&mut expected, implementation)?;
    if expected.operations != memory.operations
        || expected.outputs != memory.outputs
        || expected.alias_facts != memory.alias_facts
        || expected.buffers != memory.buffers
    {
        return Err(memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "compiler-owned access, lifetime, or alias analysis is stale",
        ));
    }
    for (result, decision) in &memory.reuse_decisions {
        match decision {
            ReuseDecision::Fresh { buffer } => {
                if memory.bindings.get(result).and_then(MemoryBinding::buffer) != Some(buffer)
                    || memory
                        .fresh_baseline_buffers
                        .get(result)
                        .is_none_or(|fresh| &fresh.id != buffer)
                {
                    return Err(memory_error(
                        ErrorCode::MemoryEquivalenceUnproved,
                        "fresh reuse decision disagrees with result binding",
                    ));
                }
            }
            ReuseDecision::InPlace { input, buffer, .. } => {
                if memory.bindings.get(result).and_then(MemoryBinding::buffer) != Some(buffer)
                    || memory.bindings.get(input).and_then(MemoryBinding::buffer) != Some(buffer)
                {
                    return Err(memory_error(
                        ErrorCode::MemoryEquivalenceUnproved,
                        "in-place decision does not bind input and result to one region",
                    ));
                }
                prove_static_reuse(memory, implementation, input, result)?;
            }
            ReuseDecision::Guarded {
                input,
                buffer,
                guard,
                fallback,
                ..
            } => {
                BudgetCheck::against(
                    limits,
                    ResourceKind::MemoryGuardDependencies,
                    u64::try_from(guard.dependencies.len()).unwrap_or(u64::MAX),
                    "MemoryIR guarded reuse verification",
                )?;
                BudgetCheck::against(
                    limits,
                    ResourceKind::MemoryGuardDepth,
                    1,
                    "MemoryIR guarded reuse verification",
                )?;
                BudgetCheck::against(
                    limits,
                    ResourceKind::MemoryFallbackDepth,
                    1,
                    "MemoryIR guarded fallback verification",
                )?;
                if memory.bindings.get(result).and_then(MemoryBinding::buffer) != Some(buffer)
                    || fallback.result != *result
                    || fallback.fresh_buffer.source_value != *result
                    || guard.primary_buffer != *buffer
                    || guard.predicate != crate::memory::MemoryGuardPredicate::NoOverlap
                    || guard.primary_buffer == guard.other_buffer
                    || !memory.buffers.contains_key(&guard.other_buffer)
                    || !guard.dependencies.iter().map(String::as_str).eq([
                        "offset_elements",
                        "shape",
                        "strides",
                        "element_type",
                    ])
                {
                    return Err(memory_error(
                        ErrorCode::MemoryGuardInvalid,
                        "guarded reuse contract is inconsistent",
                    ));
                }
                prove_static_reuse(memory, implementation, input, result)?;
            }
        }
    }
    Ok(())
}
