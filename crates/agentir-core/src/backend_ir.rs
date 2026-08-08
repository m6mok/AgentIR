//! Typed backend graph, deterministic artifact package, and measurement records.

use crate::{
    ids::{
        ArtifactId, ArtifactModuleId, BackendKernelId, BackendValueId, BufferId, MemoryPlanId,
        MemoryRevisionId, RevisionId, ScheduleNodeId, SchedulePlanId, ScheduleRevisionId,
        TargetManifestId, TargetManifestRevisionId,
    },
    impl_ir::ImplHash,
    memory::MemoryHash,
    schedule::ScheduleHash,
    semantic::SpecHash,
    target::TargetHash,
    types::{NumericContract, ScalarType, Shape},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// BackendIR structural semantics version.
pub const BACKEND_SEMANTICS_VERSION: u32 = 1;
/// BackendIR structural validator version.
pub const BACKEND_VALIDATOR_VERSION: u32 = 1;
/// BackendIR canonical codec version.
pub const BACKEND_CANONICAL_VERSION: u32 = 1;
/// Artifact package codec version.
pub const ARTIFACT_FORMAT_VERSION: u32 = 1;
/// Artifact structural validator version.
pub const ARTIFACT_VALIDATOR_VERSION: u32 = 1;
/// Deterministic artifact trace codec version.
pub const ARTIFACT_TRACE_CODEC_VERSION: u32 = 1;
/// Hardware measurement record version.
pub const MEASUREMENT_FORMAT_VERSION: u32 = 1;

/// Supported executable backend family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// WebGPU compute pipelines emitted as WGSL v1 source modules.
    WebGpuWgslV1,
}

/// Immutable Stage 1-4 anchor of one backend plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendAnchor {
    /// Frozen SpecIR revision identity.
    pub spec_revision: RevisionId,
    /// Frozen SpecIR semantic identity.
    pub spec_hash: SpecHash,
    /// Reachable ImplIR semantic identity.
    pub impl_hash: ImplHash,
    /// Exact MemoryIR identity.
    pub memory_hash: MemoryHash,
    /// Anchored MemoryIR plan.
    pub memory_plan: MemoryPlanId,
    /// Anchored MemoryIR revision.
    pub memory_revision: MemoryRevisionId,
    /// Immutable target capability identity.
    pub target_hash: TargetHash,
    /// Anchored target manifest.
    pub target_manifest: TargetManifestId,
    /// Anchored target revision.
    pub target_revision: TargetManifestRevisionId,
    /// Exact ScheduleIR identity.
    pub schedule_hash: ScheduleHash,
    /// Anchored ScheduleIR plan.
    pub schedule_plan: SchedulePlanId,
    /// Anchored ScheduleIR revision.
    pub schedule_revision: ScheduleRevisionId,
    /// Numeric semantics preserved by lowering.
    pub numeric_contract: NumericContract,
    /// Backend family selected by the compiler.
    pub backend_kind: BackendKind,
}

/// Exact logical extent retained by BackendIR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendExtent {
    /// Fully static non-negative extent.
    Static {
        /// Exact element count.
        value: u64,
    },
    /// One runtime symbol represented in the uniform parameter block.
    Symbol {
        /// Canonical external symbol name.
        name: String,
    },
}

/// Compiler-owned access mode for one backend binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendBindingAccess {
    /// Read-only storage binding.
    Read,
    /// Write-only logical binding emitted as WGSL read-write storage.
    Write,
    /// Proven in-place read/write storage binding.
    ReadWrite,
}

/// Stable resource binding in one kernel ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendBinding {
    /// Deterministic bind-group index; v1 uses group zero.
    pub group: u32,
    /// Deterministic binding number.
    pub binding: u32,
    /// Anchored MemoryIR buffer.
    pub buffer: BufferId,
    /// Optional external interface name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    /// Exact scalar element type.
    pub element_type: ScalarType,
    /// Immutable logical tensor shape.
    pub shape: Shape,
    /// Compiler-derived access mode.
    pub access: BackendBindingAccess,
    /// Required byte alignment.
    pub alignment: u64,
    /// Logical element offset.
    pub offset_elements: u64,
}

/// Scalar entry type accepted by the WGSL uniform ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendParameterType {
    /// IEEE-754 binary32 value.
    F32,
    /// Signed 32-bit integer value.
    I32,
    /// Unsigned 32-bit integer value.
    U32,
}

/// One stable scalar/uniform parameter entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendParameterEntry {
    /// Canonical external name.
    pub name: String,
    /// Exact ABI type.
    pub ty: BackendParameterType,
    /// Byte offset under the deterministic v1 ABI.
    pub offset: u64,
    /// Byte width, including no implicit padding.
    pub size: u64,
}

/// Ordered deterministic uniform parameter block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendParameterBlock {
    /// Uniform binding group.
    pub group: u32,
    /// Uniform binding number, after all storage bindings.
    pub binding: u32,
    /// Ordered entries.
    pub entries: Vec<BackendParameterEntry>,
    /// Total ABI bytes rounded to the v1 uniform alignment.
    pub byte_size: u64,
}

/// Exact primitive expression represented by BackendIR, never WGSL tokens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendExpression {
    /// Load one logical element from a storage binding.
    Load {
        /// Binding number in group zero.
        binding: u32,
        /// Typed index expression.
        index: BackendValueId,
        /// Loaded scalar type.
        ty: ScalarType,
    },
    /// Load a scalar from the uniform parameter block.
    Parameter {
        /// Parameter entry name.
        name: String,
        /// Exact scalar type.
        ty: BackendParameterType,
    },
    /// Exact canonical scalar constant bits/text.
    Constant {
        /// Scalar result type.
        ty: ScalarType,
        /// Canonical value representation.
        value: String,
    },
    /// Global invocation X index.
    InvocationIndex,
    /// Addition with ordered operands.
    Add {
        /// Left operand.
        lhs: BackendValueId,
        /// Right operand.
        rhs: BackendValueId,
    },
    /// Subtraction with ordered operands.
    Sub {
        /// Left operand.
        lhs: BackendValueId,
        /// Right operand.
        rhs: BackendValueId,
    },
    /// Multiplication with ordered operands.
    Mul {
        /// Left operand.
        lhs: BackendValueId,
        /// Right operand.
        rhs: BackendValueId,
    },
    /// Division with ordered operands.
    Div {
        /// Left operand.
        lhs: BackendValueId,
        /// Right operand.
        rhs: BackendValueId,
    },
    /// Explicit fused multiply-add.
    Fma {
        /// Multiplicand.
        a: BackendValueId,
        /// Multiplier.
        b: BackendValueId,
        /// Addend.
        c: BackendValueId,
    },
    /// Typed comparison using one stable predicate spelling.
    Compare {
        /// Predicate such as `lt`, `le`, `eq`, `ne`, `ge`, or `gt`.
        predicate: String,
        /// Left operand.
        lhs: BackendValueId,
        /// Right operand.
        rhs: BackendValueId,
    },
    /// Exact conditional scalar selection.
    Select {
        /// Boolean condition.
        condition: BackendValueId,
        /// Value selected when true.
        on_true: BackendValueId,
        /// Value selected when false.
        on_false: BackendValueId,
    },
    /// Explicit supported scalar cast.
    Cast {
        /// Source value.
        value: BackendValueId,
        /// Destination scalar type.
        target: ScalarType,
    },
    /// Construct a supported width-2 or width-4 vector.
    VectorConstruct {
        /// Ordered scalar lanes.
        lanes: Vec<BackendValueId>,
        /// Element type.
        element: ScalarType,
    },
    /// Extract one statically selected vector lane.
    VectorExtract {
        /// Vector value.
        vector: BackendValueId,
        /// Zero-based lane.
        lane: u32,
    },
    /// Compiler-owned exact bounds predicate.
    BoundsPredicate {
        /// Logical index.
        index: BackendValueId,
        /// Logical extent.
        extent: BackendExtent,
    },
}

/// One compiler-owned typed backend SSA value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendValue {
    /// Compiler-assigned identity.
    pub id: BackendValueId,
    /// Typed expression producing this value.
    pub expression: BackendExpression,
    /// Scalar result type.
    pub ty: ScalarType,
}

/// Typed kernel statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendStatement {
    /// Store one scalar element through a typed binding.
    Store {
        /// Target binding number.
        binding: u32,
        /// Logical element index.
        index: BackendValueId,
        /// Stored scalar value.
        value: BackendValueId,
    },
    /// Conservative serial loop used by a serial ScheduleIR root.
    SerialLoop {
        /// Compiler-assigned induction variable value.
        index: BackendValueId,
        /// Exact logical extent.
        extent: BackendExtent,
        /// Ordered loop body.
        body: Vec<BackendStatement>,
    },
    /// Execute a statement only when a compiler-owned bounds predicate is true.
    IfBounds {
        /// Boolean compiler predicate.
        predicate: BackendValueId,
        /// Ordered guarded statements.
        body: Vec<BackendStatement>,
    },
}

/// Stable output mapping from a kernel write to an external result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendOutput {
    /// External output name.
    pub name: String,
    /// Storage binding containing the result.
    pub binding: u32,
    /// Anchored MemoryIR output buffer.
    pub buffer: BufferId,
}

/// One typed executable kernel in BackendIR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendKernel {
    /// Compiler-assigned kernel identity.
    pub id: BackendKernelId,
    /// Stable generated entry-point name.
    pub entry_point: String,
    /// Ordered ScheduleIR nodes covered by this kernel.
    pub source_schedule_nodes: Vec<ScheduleNodeId>,
    /// Ordered storage bindings.
    pub bindings: Vec<BackendBinding>,
    /// Stable scalar/uniform ABI.
    pub parameter_block: BackendParameterBlock,
    /// Explicit workgroup dimensions.
    pub workgroup_size: [u32; 3],
    /// Exact one-dimensional logical extent.
    pub logical_extent: BackendExtent,
    /// Whether this kernel uses a conservative serial loop.
    pub serial: bool,
    /// Exact vector width selected by ScheduleIR.
    pub vector_width: u32,
    /// Exact bounded unroll factor selected by ScheduleIR.
    pub unroll_factor: u32,
    /// Deterministic typed SSA expressions.
    pub values: BTreeMap<BackendValueId, BackendValue>,
    /// Ordered typed statements.
    pub statements: Vec<BackendStatement>,
    /// Ordered result mappings.
    pub outputs: Vec<BackendOutput>,
    /// Compiler-owned structural lowering method.
    pub lowering_method: String,
}

/// One ordered compute dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDispatch {
    /// Zero-based dispatch order.
    pub order: u64,
    /// Kernel invoked by this dispatch.
    pub kernel: BackendKernelId,
    /// Logical workgroup counts; symbolic X uses the parameter block.
    pub workgroups: [BackendExtent; 3],
    /// Exact workgroup size copied from the kernel.
    pub workgroup_size: [u32; 3],
    /// Whether compiler-owned bounds/remainder handling is required.
    pub bounds_checked: bool,
}

/// Compiler-owned runtime guard kind supported by BackendIR v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendGuardPredicate {
    /// Exact typed region non-overlap check inherited from MemoryIR.
    NoOverlap {
        /// First anchored region.
        first: BufferId,
        /// Second anchored region.
        second: BufferId,
        /// Checked byte length, static or symbol-derived.
        bytes: BackendExtent,
    },
}

/// Lazy exact true/fallback dispatch selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendGuardBranch {
    /// Compiler-owned predicate.
    pub predicate: BackendGuardPredicate,
    /// Dispatch orders used when the predicate is true.
    pub true_dispatches: Vec<u64>,
    /// Exact lazy fresh-fallback dispatch orders.
    pub false_dispatches: Vec<u64>,
}

/// Separate typed backend graph.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendProgram {
    /// Kernels keyed by compiler identity.
    pub kernels: BTreeMap<BackendKernelId, BackendKernel>,
    /// Deterministic kernel declaration order.
    pub kernel_order: Vec<BackendKernelId>,
    /// Ordered dispatch graph.
    pub dispatches: Vec<BackendDispatch>,
    /// Optional exact lazy guarded path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<BackendGuardBranch>,
    /// Stable external output mapping across the package.
    pub outputs: Vec<BackendOutput>,
}

/// Lifecycle state of one backend revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendStatus {
    /// Structurally verified and proved equivalent to ScheduleIR.
    Proved,
    /// Immutable backend revision ready for artifact emission.
    Sealed,
    /// Corrupted serialized state was rejected.
    Rejected,
}

/// Compiler-owned BackendEquivalentToSchedule certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCertificate {
    /// Stable proof relation.
    pub relation: String,
    /// Anchored schedule hash.
    pub schedule_hash: ScheduleHash,
    /// Exact output backend hash.
    pub backend_hash: crate::backend::BackendHash,
    /// Ordered covered schedule nodes.
    pub schedule_node_coverage: Vec<ScheduleNodeId>,
    /// Ordered structural conditions checked by the compiler.
    pub conditions: Vec<String>,
    /// Backend semantics version.
    pub semantics_version: u32,
    /// Structural validator version.
    pub validator_version: u32,
}

/// Confidence-only backend evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendEvidence {
    /// Stable evidence method.
    pub method: String,
    /// Whether the evidence is correctness-authoritative; always false here.
    pub correctness_authority: bool,
    /// Deterministic result.
    pub result: String,
}

/// Backend proof obligation retained for structured lifecycle checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendObligation {
    /// Stable obligation kind.
    pub kind: String,
    /// Deterministic status.
    pub status: String,
}

/// Exact module bytes and entry-point ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactModule {
    /// Compiler-assigned module identity.
    pub id: ArtifactModuleId,
    /// Stable module name.
    pub name: String,
    /// Exact UTF-8 WGSL source bytes represented as a string.
    pub wgsl: String,
    /// Entry points defined by this module.
    pub entry_points: Vec<String>,
}

/// Manifest entry point mapped to a BackendIR kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEntryPoint {
    /// WGSL module containing the function.
    pub module: ArtifactModuleId,
    /// Exact function name.
    pub name: String,
    /// Source BackendIR kernel.
    pub kernel: BackendKernelId,
    /// Exact workgroup size.
    pub workgroup_size: [u32; 3],
}

/// Complete deterministic runtime ABI for one emitted kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactBindingLayout {
    /// Source BackendIR kernel.
    pub kernel: BackendKernelId,
    /// Ordered storage bindings in bind group zero.
    pub storage_bindings: Vec<BackendBinding>,
    /// Stable scalar/uniform parameter block.
    pub parameter_block: BackendParameterBlock,
    /// Exact logical extent used to size buffers and dispatches.
    pub logical_extent: BackendExtent,
    /// Ordered external result mappings produced by this kernel.
    pub outputs: Vec<BackendOutput>,
}

/// Reproducible package manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactManifest {
    /// Artifact package format.
    pub format: String,
    /// Package format version.
    pub format_version: u32,
    /// Backend family.
    pub backend_kind: BackendKind,
    /// Immutable Stage 1-4 and backend anchors.
    pub anchor: BackendAnchor,
    /// Exact BackendIR identity.
    pub backend_hash: crate::backend::BackendHash,
    /// Compiler build identity.
    pub compiler_build_hash: crate::backend::CompilerBuildHash,
    /// Ordered module IDs.
    pub modules: Vec<ArtifactModuleId>,
    /// Ordered entry points.
    pub entry_points: Vec<ArtifactEntryPoint>,
    /// Complete runtime binding ABI keyed by entry-point order.
    pub binding_layouts: Vec<ArtifactBindingLayout>,
    /// Ordered dispatch plan copied from verified BackendIR.
    pub dispatches: Vec<BackendDispatch>,
    /// Optional lazy exact guard plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<BackendGuardBranch>,
    /// External output mapping.
    pub outputs: Vec<BackendOutput>,
    /// Stable proof manifest entries.
    pub proof_manifest: Vec<String>,
}

/// Result of offline WGSL parser and validator checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineValidationReport {
    /// Whether all modules parsed.
    pub parsed: bool,
    /// Whether types, bindings, entry points and workgroup declarations validated.
    pub validated: bool,
    /// Stable validator implementation label.
    pub validator: String,
    /// Ordered deterministic diagnostics, empty on success.
    pub diagnostics: Vec<String>,
}

/// Lifecycle state of an artifact package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    /// Emitted and offline validated.
    Validated,
    /// Immutable executable package.
    Sealed,
    /// Corrupted or invalid package.
    Rejected,
}

/// Compiler-owned ArtifactEquivalentToBackend certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCertificate {
    /// Stable proof relation.
    pub relation: String,
    /// Exact input backend hash.
    pub backend_hash: crate::backend::BackendHash,
    /// Compiler build identity.
    pub compiler_build_hash: crate::backend::CompilerBuildHash,
    /// Exact output artifact hash.
    pub artifact_hash: crate::backend::ArtifactHash,
    /// Ordered checked conditions.
    pub conditions: Vec<String>,
    /// Artifact validator version.
    pub validator_version: u32,
}

/// Directly executable deterministic WGSL artifact package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPackage {
    /// Compiler-assigned artifact identity.
    pub id: ArtifactId,
    /// Exact reproducibility manifest.
    pub manifest: ArtifactManifest,
    /// Ordered WGSL modules.
    pub modules: Vec<ArtifactModule>,
    /// Offline parser/validator report.
    pub offline_validation: OfflineValidationReport,
    /// Lifecycle state.
    pub status: ArtifactStatus,
    /// Exact package hash.
    pub artifact_hash: crate::backend::ArtifactHash,
    /// Compiler-owned emission certificate.
    pub certificate: ArtifactCertificate,
}

/// One deterministic artifact reference-execution trace event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactTraceEvent {
    /// Monotonic event sequence.
    pub sequence: u64,
    /// Stable event kind.
    pub kind: String,
    /// Deterministic detail.
    pub detail: String,
}

/// Deterministic artifact-plan trace, without interpreting arbitrary WGSL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactTrace {
    /// Trace codec version.
    pub trace_codec_version: u32,
    /// Selected artifact.
    pub artifact: ArtifactId,
    /// Selected guard branch, when guarded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard_branch: Option<bool>,
    /// Ordered trace events.
    pub events: Vec<ArtifactTraceEvent>,
}

/// Stable WebGPU device identity and reported capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    /// Runtime backend/API name.
    pub backend_api: String,
    /// Adapter name reported by the runtime.
    pub adapter_name: String,
    /// Numeric vendor identifier when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u32>,
    /// Numeric device identifier when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<u32>,
    /// Driver/backend information when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_info: Option<String>,
    /// Deterministically ordered reported limits.
    pub limits: BTreeMap<String, u64>,
    /// Runtime version.
    pub runtime_version: String,
    /// Compiler build version.
    pub compiler_version: String,
}

/// Exact bounded benchmark configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareBenchmarkConfig {
    /// Warm-up iteration count.
    pub warmups: u32,
    /// Measured iteration count.
    pub iterations: u32,
    /// Stable input distribution descriptor.
    pub input_distribution: String,
    /// Ordered tensor dimensions.
    pub tensor_dimensions: Vec<u64>,
}

/// Confidence-only completed hardware benchmark record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareMeasurementRecord {
    /// Measurement record format version.
    pub format_version: u32,
    /// Exact artifact measured.
    pub artifact_hash: crate::backend::ArtifactHash,
    /// Immutable target contract.
    pub target_hash: TargetHash,
    /// Compiler build identity.
    pub compiler_build_hash: crate::backend::CompilerBuildHash,
    /// Exact device fingerprint hash.
    pub device_fingerprint_hash: crate::backend::DeviceFingerprintHash,
    /// Device fingerprint provenance.
    pub device: DeviceFingerprint,
    /// Bounded benchmark configuration.
    pub config: HardwareBenchmarkConfig,
    /// Minimum device execution nanoseconds.
    pub min_ns: u64,
    /// Median device execution nanoseconds.
    pub median_ns: u64,
    /// P95 device execution nanoseconds.
    pub p95_ns: u64,
    /// Maximum device execution nanoseconds.
    pub max_ns: u64,
    /// Guard outcome counts keyed by stable branch spelling.
    pub guard_outcomes: BTreeMap<String, u64>,
    /// Offline/device validation status.
    pub validation_status: String,
    /// Runtime version.
    pub runtime_version: String,
    /// Exact record hash.
    pub measurement_hash: crate::backend::MeasurementHash,
}
