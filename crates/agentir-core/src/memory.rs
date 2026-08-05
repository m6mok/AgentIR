//! Persistent immutable MemoryIR plan revisions and atomic memory transactions.

use crate::{
    candidate::{
        CandidateForest, CandidateHash, CandidateState, EquivalenceStatus, ProofDebtStatus,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        AliasDomainId, BufferId, CandidateId, CandidateRevisionId, ImplValueId, MemoryEvidenceId,
        MemoryGuardId, MemoryObligationId, MemoryOperationId, MemoryPlanId, MemoryRevisionId,
        RevisionId,
    },
    impl_ir::{ImplHash, ImplProgram, impl_hash, verify_impl},
    ir::{Opcode, Program},
    memory_ir::{
        AccessMode, AddressSpace, AliasDomain, AliasFact, Lifetime, MEMORY_SEMANTICS_VERSION,
        MEMORY_VALIDATOR_VERSION, MemoryBinding, MemoryBuffer, MemoryLayout, MemoryOperation,
        MemoryProgram, Ownership, ReuseDecision, contiguous_strides, prove_static_reuse,
        reachable_impl_operations, rebuild_analysis, verify_memory_program,
    },
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    semantic::SpecHash,
    types::{ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
};

/// Current exact MemoryIR canonical codec version.
pub const MEMORY_CANONICAL_VERSION: u32 = 1;
/// Current memory event/replay semantics version.
pub const MEMORY_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for exact MemoryIR state hashes.
pub const MEMORY_HASH_DOMAIN: &[u8] = b"agentir.memory.exact.v1\0";

/// SHA-256 identity of one exact MemoryIR plan revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryHash(String);

impl MemoryHash {
    /// Creates a memory hash from its lowercase hexadecimal representation.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the lowercase hexadecimal representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemoryHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Memory-local monotonic allocator isolated from every legacy allocator contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAllocator {
    plan: u64,
    revision: u64,
    buffer: u64,
    operation: u64,
    alias_domain: u64,
    obligation: u64,
    evidence: u64,
    guard: u64,
}

macro_rules! memory_allocator_method {
    ($method:ident, $field:ident, $prefix:literal, $kind:ident) => {
        #[doc = concat!("Allocates the next `", stringify!($kind), "`.")]
        pub fn $method(&mut self) -> $kind {
            self.$field = self.$field.saturating_add(1);
            $kind::new(format!(concat!($prefix, "{}"), self.$field))
        }
    };
}

impl MemoryAllocator {
    memory_allocator_method!(plan, plan, "mp", MemoryPlanId);
    memory_allocator_method!(revision, revision, "mr", MemoryRevisionId);
    memory_allocator_method!(buffer, buffer, "buf", BufferId);
    memory_allocator_method!(operation, operation, "mop", MemoryOperationId);
    memory_allocator_method!(alias_domain, alias_domain, "ad", AliasDomainId);
    memory_allocator_method!(obligation, obligation, "mo", MemoryObligationId);
    memory_allocator_method!(evidence, evidence, "mev", MemoryEvidenceId);
    memory_allocator_method!(guard, guard, "mg", MemoryGuardId);
}

/// Immutable SpecIR/candidate/ImplIR anchor for one memory plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAnchor {
    /// Frozen SpecIR revision.
    pub spec_revision: RevisionId,
    /// Immutable frozen specification hash.
    pub spec_hash: SpecHash,
    /// Fully proved exact candidate branch.
    pub candidate: CandidateId,
    /// Explicit immutable candidate revision.
    pub candidate_revision: CandidateRevisionId,
    /// Exact candidate-state hash at creation.
    pub candidate_hash: CandidateHash,
    /// Immutable reachable implementation semantic hash.
    pub impl_hash: ImplHash,
}

/// Lifecycle state of one immutable memory revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Editable fork before complete re-verification.
    Draft,
    /// Typed storage graph is structurally valid.
    WellTyped,
    /// Compiler-owned structural certificates prove exactness.
    Proved,
    /// Exactness composes a compiler guard with a proved fresh fallback.
    Guarded,
    /// Immutable accepted memory plan.
    Sealed,
    /// Integrity verification rejected the state.
    Rejected,
}

/// State of a compiler-owned memory correctness obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryObligationStatus {
    /// Awaiting a compiler-owned structural proof.
    Open,
    /// Discharged by a compiler-owned structural proof.
    Proved,
    /// Rejected by structural verification.
    Refuted,
}

/// One persistent compiler-owned MemoryIR obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryObligation {
    /// Compiler-assigned identity.
    pub id: MemoryObligationId,
    /// Stable obligation kind.
    pub kind: String,
    /// Current proof state.
    pub status: MemoryObligationStatus,
    /// Immutable implementation anchor.
    pub impl_hash: ImplHash,
    /// Stable normalized detail fields.
    pub details: BTreeMap<String, String>,
}

/// Correctness or confidence strength for memory evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceClass {
    /// Compiler-owned structural correctness evidence.
    Correctness,
    /// Reference testing evidence that never proves exactness.
    Confidence,
}

/// Stable compiler-owned memory evidence method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceKind {
    /// Conservative fresh bufferization certificate.
    FreshBufferization,
    /// Static alias/lifetime proof for in-place reuse.
    InPlaceReuse,
    /// Compiler-owned NoOverlap guard plus exact fresh fallback.
    GuardedReuse,
    /// Recomputed whole-program MemoryEquivalentToImpl proof.
    CompositionalMemoryVerification,
    /// Immutable memory-plan sealing certificate.
    Sealing,
    /// Reference differential/trace confidence evidence.
    DifferentialExecution,
}

/// One compiler-owned memory evidence record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEvidenceRecord {
    /// Compiler-assigned evidence identity.
    pub id: MemoryEvidenceId,
    /// Correctness or confidence class.
    pub class: MemoryEvidenceClass,
    /// Stable method kind.
    pub kind: MemoryEvidenceKind,
    /// Memory plan covered by this record.
    pub memory_plan: MemoryPlanId,
    /// Immutable revision covered by this record.
    pub memory_revision: MemoryRevisionId,
    /// Immutable implementation anchor.
    pub impl_hash: ImplHash,
    /// Prior exact memory hash, absent for root bufferization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_memory_hash: Option<MemoryHash>,
    /// Resulting exact memory hash.
    pub output_memory_hash: MemoryHash,
    /// Stable validator method name.
    pub method: String,
    /// Memory semantics version.
    pub memory_semantics_version: u32,
    /// Validator implementation version.
    pub validator_version: u32,
}

/// Compiler-owned correctness certificate attached to one memory revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCertificate {
    /// Stable certificate method.
    pub method: String,
    /// Immutable implementation hash.
    pub impl_hash: ImplHash,
    /// Prior exact memory hash, absent for root bufferization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_memory_hash: Option<MemoryHash>,
    /// Resulting exact memory hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_memory_hash: Option<MemoryHash>,
    /// Ordered normalized side conditions.
    pub side_conditions: Vec<String>,
    /// Linked correctness evidence.
    pub evidence: MemoryEvidenceId,
    /// Memory semantics version.
    pub memory_semantics_version: u32,
    /// Validator version.
    pub validator_version: u32,
}

/// Sole compiler-owned Stage 3 runtime memory predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryGuardPredicate {
    /// Runtime metadata proves two bounded typed regions do not overlap.
    NoOverlap,
}

/// Normalized compiler-owned NoOverlap guard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryGuard {
    /// Compiler-assigned guard identity.
    pub id: MemoryGuardId,
    /// Sole supported predicate profile.
    pub predicate: MemoryGuardPredicate,
    /// Region reused on the true branch.
    pub primary_buffer: BufferId,
    /// Other region checked for non-overlap.
    pub other_buffer: BufferId,
    /// Trusted runtime metadata dependencies.
    pub dependencies: Vec<String>,
}

/// Immutable exact false-branch storage fragment for guarded reuse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFallback {
    /// Logical result produced by the fallback.
    pub result: ImplValueId,
    /// Fully typed fresh result region.
    pub fresh_buffer: MemoryBuffer,
    /// Stable strategy; always `lazy_fresh_allocation`.
    pub strategy: String,
}

/// One immutable typed memory-plan revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRevision {
    /// Compiler-assigned revision identity.
    pub id: MemoryRevisionId,
    /// Immutable parent revisions.
    pub parents: Vec<MemoryRevisionId>,
    /// Separate typed physical storage graph.
    pub program: MemoryProgram,
    /// Exact MemoryIR state hash.
    pub memory_hash: MemoryHash,
    /// Immutable implementation semantic anchor.
    pub impl_hash: ImplHash,
    /// Lifecycle state.
    pub status: MemoryStatus,
    /// Compiler-owned correctness obligations.
    pub obligations: Vec<MemoryObligation>,
    /// Compiler-owned structural certificates.
    pub certificates: Vec<MemoryCertificate>,
    /// Ordered evidence references.
    pub evidence: Vec<MemoryEvidenceId>,
}

/// Persistent independent memory-plan revision DAG.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPlan {
    /// Compiler-assigned plan identity.
    pub id: MemoryPlanId,
    /// Immutable SpecIR/candidate/ImplIR anchor.
    pub anchor: MemoryAnchor,
    /// Root memory revision.
    pub root_revision: MemoryRevisionId,
    /// Current memory head.
    pub head: MemoryRevisionId,
    /// Immutable memory revisions.
    pub revisions: BTreeMap<MemoryRevisionId, MemoryRevision>,
    /// Parent plan provenance for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_plan: Option<MemoryPlanId>,
    /// Parent revision provenance for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_revision: Option<MemoryRevisionId>,
}

/// One compiler-verified memory transaction action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryAction {
    /// Restores the compiler-owned conservative fresh result buffer.
    ChooseFreshBuffer {
        /// Tensor result whose fresh baseline is selected.
        result: ImplValueId,
    },
    /// Requests compiler-proved in-place input storage reuse.
    RequestInPlaceReuse {
        /// Tensor input proposed for overwrite.
        input: ImplValueId,
        /// Tensor result that will use the region.
        result: ImplValueId,
    },
    /// Requests the compiler-owned NoOverlap guarded reuse profile.
    RequestGuardedReuse {
        /// Tensor input proposed for overwrite.
        input: ImplValueId,
        /// Tensor result that will use the region.
        result: ImplValueId,
        /// Other typed region used to instantiate NoOverlap.
        guard_against: BufferId,
    },
    /// Selects a fully explicit legal physical layout.
    SetLayout {
        /// Buffer to update.
        buffer: BufferId,
        /// Requested layout; proof payloads are not accepted.
        layout: MemoryLayout,
    },
    /// Selects an abstract typed address space.
    SetAddressSpace {
        /// Buffer to update.
        buffer: BufferId,
        /// Requested abstract address space.
        address_space: AddressSpace,
    },
    /// Raises or changes a buffer alignment requirement.
    SetAlignment {
        /// Buffer to update.
        buffer: BufferId,
        /// Requested power-of-two byte alignment.
        alignment: u64,
    },
}

/// Atomic memory transaction against an explicit immutable base revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTransaction {
    /// Memory plan to edit.
    pub memory_plan: MemoryPlanId,
    /// Explicit current base revision.
    pub base_memory_revision: MemoryRevisionId,
    /// Required exact base memory hash.
    pub expected_memory_hash: MemoryHash,
    /// Required immutable implementation hash.
    pub expected_impl_hash: ImplHash,
    /// Ordered compiler-verified storage actions.
    pub actions: Vec<MemoryAction>,
}

/// Replayable memory-plan mutation event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryEvent {
    /// Conservative exact fresh memory root creation.
    Created {
        /// Exact candidate branch anchor.
        candidate: CandidateId,
        /// Explicit candidate revision anchor.
        candidate_revision: CandidateRevisionId,
        /// Expected memory plan ID.
        memory_plan: MemoryPlanId,
        /// Expected root memory revision ID.
        memory_revision: MemoryRevisionId,
        /// Expected exact memory hash.
        memory_hash: MemoryHash,
    },
    /// Atomic memory transaction publication.
    Applied {
        /// Replayable transaction.
        transaction: MemoryTransaction,
        /// Expected child memory revision ID.
        memory_revision: MemoryRevisionId,
        /// Expected exact memory hash.
        memory_hash: MemoryHash,
    },
    /// Explicit memory-plan fork.
    Forked {
        /// Parent memory plan.
        parent_plan: MemoryPlanId,
        /// Parent immutable revision.
        parent_revision: MemoryRevisionId,
        /// Expected child plan.
        memory_plan: MemoryPlanId,
        /// Expected child root revision.
        memory_revision: MemoryRevisionId,
        /// Expected child exact memory hash.
        memory_hash: MemoryHash,
    },
    /// Immutable memory-plan seal transition.
    Sealed {
        /// Memory plan.
        memory_plan: MemoryPlanId,
        /// Explicit base revision.
        base_revision: MemoryRevisionId,
        /// Required base exact memory hash.
        expected_memory_hash: MemoryHash,
        /// Expected sealed revision.
        memory_revision: MemoryRevisionId,
        /// Expected sealed exact memory hash.
        memory_hash: MemoryHash,
    },
}

/// Memory event paired with explicit cross-store dependency cursors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedMemoryEvent {
    /// Memory event semantics version.
    pub semantics_version: u32,
    /// Candidate events that must exist before this event.
    pub candidate_event_cursor: u64,
    /// Equality events that must exist before this event.
    pub equality_event_cursor: u64,
    /// Replayable memory event.
    pub event: MemoryEvent,
}

/// Read-only memory-plan summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Memory plan.
    pub memory_plan: MemoryPlanId,
    /// Selected immutable memory revision.
    pub memory_revision: MemoryRevisionId,
    /// Exact memory hash.
    pub memory_hash: MemoryHash,
    /// Immutable implementation hash.
    pub impl_hash: ImplHash,
    /// Lifecycle state.
    pub status: MemoryStatus,
    /// Active typed buffer count.
    pub buffer_count: usize,
    /// Explicit typed access count.
    pub access_count: usize,
    /// Fresh result decision count.
    pub fresh_buffer_count: usize,
    /// Proved in-place decision count.
    pub reused_buffer_count: usize,
    /// Guarded decision count.
    pub guarded_buffer_count: usize,
    /// Open memory obligations.
    pub open_obligations: Vec<MemoryObligationId>,
    /// Correctness evidence reference count.
    pub correctness_evidence: usize,
    /// Confidence evidence reference count.
    pub confidence_evidence: usize,
}

/// Full structural memory verification report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCheckReport {
    /// Read-only state summary.
    pub query: MemoryQuery,
    /// Whether the typed physical graph verified.
    pub well_typed: bool,
    /// Whether MemoryEquivalentToImpl is structurally proved.
    pub equivalent_to_impl: bool,
    /// Whether immutable sealing is legal.
    pub sealable: bool,
}

/// One eligible deterministic result/input reuse choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryContinuationChoice {
    /// Tensor result storage decision.
    pub result: ImplValueId,
    /// Candidate input for reuse.
    pub input: ImplValueId,
    /// Whether every static side condition is proved.
    pub statically_applicable: bool,
    /// Stable failed side condition when static proof is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_side_condition: Option<ErrorCode>,
    /// Fresh allocation remains the exact fallback.
    pub fresh_fallback: bool,
    /// Sole supported guard profile.
    pub supported_guard_profile: String,
}

/// Bounded deterministic memory continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryContinuation {
    /// Memory plan.
    pub memory_plan: MemoryPlanId,
    /// Memory revision inspected.
    pub memory_revision: MemoryRevisionId,
    /// Required exact memory hash.
    pub expected_memory_hash: MemoryHash,
    /// Required immutable implementation hash.
    pub expected_impl_hash: ImplHash,
    /// Bounded parameteric reuse choices.
    pub reuse_choices: Vec<MemoryContinuationChoice>,
    /// Legal abstract address-space domain.
    pub address_spaces: Vec<AddressSpace>,
    /// Legal small layout domain.
    pub layouts: Vec<String>,
}

/// Persistent independent memory plans, evidence, allocator, and event history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPlanStore {
    /// Memory plan branches by compiler-assigned ID.
    pub plans: BTreeMap<MemoryPlanId, MemoryPlan>,
    /// Compiler-owned memory evidence records.
    pub evidence: BTreeMap<MemoryEvidenceId, MemoryEvidenceRecord>,
    /// Memory-local allocator state.
    pub allocator: MemoryAllocator,
    /// Ordered replayable memory event log.
    pub events: Vec<VersionedMemoryEvent>,
}

#[derive(Serialize)]
struct CanonicalCertificate<'a> {
    method: &'a str,
    impl_hash: &'a ImplHash,
    before_memory_hash: &'a Option<MemoryHash>,
    side_conditions: &'a [String],
    evidence: &'a MemoryEvidenceId,
    memory_semantics_version: u32,
    validator_version: u32,
}

#[derive(Serialize)]
struct MemoryHashModel<'a> {
    codec: &'static str,
    version: u32,
    memory_semantics_version: u32,
    memory_event_semantics_version: u32,
    validator_version: u32,
    memory_plan: &'a MemoryPlanId,
    anchor: &'a MemoryAnchor,
    parent_plan: &'a Option<MemoryPlanId>,
    forked_from_revision: &'a Option<MemoryRevisionId>,
    memory_revision: &'a MemoryRevisionId,
    parents: &'a [MemoryRevisionId],
    program: &'a MemoryProgram,
    impl_hash: &'a ImplHash,
    status: MemoryStatus,
    obligations: &'a [MemoryObligation],
    certificates: Vec<CanonicalCertificate<'a>>,
    evidence: &'a [MemoryEvidenceId],
}

fn memory_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn digest(bytes: &[u8]) -> MemoryHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    MemoryHash(output)
}

/// Serializes one memory revision using the versioned exact MemoryIR canonical model.
///
/// Resource policy only bounds the operation and is deliberately absent from the returned bytes.
pub fn canonical_memory_bytes_with_limit(
    plan: &MemoryPlan,
    revision: &MemoryRevision,
    limits: &ResourceLimits,
) -> AgentResult<Vec<u8>> {
    let model = MemoryHashModel {
        codec: "agentir.memory.exact",
        version: MEMORY_CANONICAL_VERSION,
        memory_semantics_version: MEMORY_SEMANTICS_VERSION,
        memory_event_semantics_version: MEMORY_EVENT_SEMANTICS_VERSION,
        validator_version: MEMORY_VALIDATOR_VERSION,
        memory_plan: &plan.id,
        anchor: &plan.anchor,
        parent_plan: &plan.parent_plan,
        forked_from_revision: &plan.forked_from_revision,
        memory_revision: &revision.id,
        parents: &revision.parents,
        program: &revision.program,
        impl_hash: &revision.impl_hash,
        status: revision.status,
        obligations: &revision.obligations,
        certificates: revision
            .certificates
            .iter()
            .map(|certificate| CanonicalCertificate {
                method: &certificate.method,
                impl_hash: &certificate.impl_hash,
                before_memory_hash: &certificate.before_memory_hash,
                side_conditions: &certificate.side_conditions,
                evidence: &certificate.evidence,
                memory_semantics_version: certificate.memory_semantics_version,
                validator_version: certificate.validator_version,
            })
            .collect(),
        evidence: &revision.evidence,
    };
    let bytes = serde_json::to_vec(&model).map_err(|error| {
        memory_error(
            ErrorCode::CanonicalizationFailed,
            format!("MemoryIR exact-state serialization failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryCanonicalBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "MemoryIR exact-state canonicalization",
    )?;
    Ok(bytes)
}

fn memory_hash_with_limit(
    plan: &MemoryPlan,
    revision: &MemoryRevision,
    limits: &ResourceLimits,
) -> AgentResult<MemoryHash> {
    let bytes = canonical_memory_bytes_with_limit(plan, revision, limits)?;
    let mut input = Vec::with_capacity(MEMORY_HASH_DOMAIN.len().saturating_add(bytes.len()));
    input.extend_from_slice(MEMORY_HASH_DOMAIN);
    input.extend_from_slice(&bytes);
    Ok(digest(&input))
}

fn scalar_alignment(element: ScalarType) -> u64 {
    match element {
        ScalarType::Bool => 1,
        ScalarType::I32 | ScalarType::F32 => 4,
        ScalarType::Index => 8,
    }
}

fn lower_fresh(
    implementation: &ImplProgram,
    allocator: &mut MemoryAllocator,
) -> AgentResult<MemoryProgram> {
    let reached = reachable_impl_operations(implementation)?;
    let mut memory = MemoryProgram::default();
    for operation_id in &implementation.operation_order {
        if !reached.contains(operation_id) {
            continue;
        }
        let operation = &implementation.operations[operation_id];
        for value_id in &operation.results {
            let value = &implementation.values[value_id];
            match &value.ty {
                Type::Scalar(_) => {
                    memory.bindings.insert(
                        value_id.clone(),
                        MemoryBinding::Scalar {
                            value: value_id.clone(),
                        },
                    );
                }
                Type::Tensor { element, shape } => {
                    let buffer_id = allocator.buffer();
                    let alias_domain = allocator.alias_domain();
                    let parameter_name = implementation
                        .parameters
                        .iter()
                        .find_map(|(name, value)| (value == value_id).then_some(name.clone()));
                    let is_parameter = operation.opcode == Opcode::Parameter;
                    let is_constant = operation.opcode == Opcode::Constant;
                    let strides = contiguous_strides(shape)?;
                    let buffer = MemoryBuffer {
                        id: buffer_id.clone(),
                        element_type: *element,
                        shape: shape.clone(),
                        layout: MemoryLayout::ContiguousRowMajor,
                        strides,
                        address_space: if is_constant {
                            AddressSpace::Constant
                        } else {
                            AddressSpace::Global
                        },
                        access: if is_parameter || is_constant {
                            AccessMode::ReadOnly
                        } else {
                            AccessMode::ReadWrite
                        },
                        alignment: scalar_alignment(*element),
                        alias_domain: alias_domain.clone(),
                        lifetime: Lifetime {
                            first_point: 0,
                            uses: Vec::new(),
                            last_use: 0,
                            output_escape: false,
                            external: is_parameter,
                            deallocation_eligible: !is_parameter,
                        },
                        ownership: if is_parameter {
                            Ownership::ExternalBorrowed
                        } else if is_constant {
                            Ownership::ConstantOwned
                        } else {
                            Ownership::PlanOwned
                        },
                        external_binding: parameter_name,
                        source_value: value_id.clone(),
                        offset_elements: 0,
                        provenance: if is_parameter {
                            "external_parameter".to_owned()
                        } else if is_constant {
                            "immutable_constant".to_owned()
                        } else {
                            "fresh_conservative_bufferization".to_owned()
                        },
                    };
                    memory.alias_domains.insert(
                        alias_domain.clone(),
                        AliasDomain {
                            id: alias_domain,
                            provenance: if is_parameter {
                                crate::memory_ir::AliasProvenance::ProvedFromExternalContract
                            } else {
                                crate::memory_ir::AliasProvenance::ProvedFromRegionConstruction
                            },
                        },
                    );
                    memory.buffers.insert(buffer_id.clone(), buffer.clone());
                    memory.bindings.insert(
                        value_id.clone(),
                        MemoryBinding::Buffer {
                            value: value_id.clone(),
                            buffer: buffer_id.clone(),
                        },
                    );
                    if !is_parameter && !is_constant {
                        memory
                            .fresh_baseline_buffers
                            .insert(value_id.clone(), buffer);
                        memory
                            .reuse_decisions
                            .insert(value_id.clone(), ReuseDecision::Fresh { buffer: buffer_id });
                    }
                }
            }
        }
        if !matches!(operation.opcode, Opcode::Parameter | Opcode::Constant) {
            let memory_operation = allocator.operation();
            memory
                .operation_for_impl
                .insert(operation_id.clone(), memory_operation.clone());
            memory.operation_order.push(memory_operation.clone());
            memory.operations.insert(
                memory_operation.clone(),
                MemoryOperation {
                    id: memory_operation,
                    impl_operation: operation_id.clone(),
                    opcode: operation.opcode.to_string(),
                    operands: Vec::new(),
                    results: Vec::new(),
                    accesses: Vec::new(),
                },
            );
        }
    }
    rebuild_analysis(&mut memory, implementation)?;
    Ok(memory)
}

fn plan_revision<'a>(
    store: &'a MemoryPlanStore,
    plan: &MemoryPlanId,
    revision: &MemoryRevisionId,
) -> AgentResult<(&'a MemoryPlan, &'a MemoryRevision)> {
    let plan_data = store.plans.get(plan).ok_or_else(|| {
        memory_error(
            ErrorCode::MemoryPlanNotFound,
            format!("memory plan `{plan}` does not exist"),
        )
    })?;
    let revision_data = plan_data.revisions.get(revision).ok_or_else(|| {
        memory_error(
            ErrorCode::MemoryRevisionNotFound,
            format!("memory revision `{revision}` does not exist"),
        )
    })?;
    Ok((plan_data, revision_data))
}

fn total_revisions(store: &MemoryPlanStore) -> u64 {
    store.plans.values().fold(0_u64, |total, plan| {
        total.saturating_add(u64::try_from(plan.revisions.len()).unwrap_or(u64::MAX))
    })
}

fn ensure_store_budgets(store: &MemoryPlanStore, limits: &ResourceLimits) -> AgentResult<()> {
    for (resource, actual) in [
        (
            ResourceKind::MemoryPlansPerWorkspace,
            u64::try_from(store.plans.len()).unwrap_or(u64::MAX),
        ),
        (
            ResourceKind::MemoryRevisionsPerWorkspace,
            total_revisions(store),
        ),
        (
            ResourceKind::MemoryEvents,
            u64::try_from(store.events.len()).unwrap_or(u64::MAX),
        ),
    ] {
        BudgetCheck::against(limits, resource, actual, "MemoryIR plan store")?;
    }
    let bytes = serde_json::to_vec(store).map_err(|error| {
        memory_error(
            ErrorCode::PersistenceFormat,
            format!("MemoryIR archive preflight encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryArchiveBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "MemoryIR archive state",
    )
}

fn verify_anchor(
    forest: &CandidateForest,
    anchor: &MemoryAnchor,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<ImplProgram> {
    let candidate = forest.candidate(&anchor.candidate)?;
    let revision = forest.revision(&anchor.candidate, &anchor.candidate_revision)?;
    forest.check(
        &anchor.candidate,
        &anchor.candidate_revision,
        source,
        limits,
    )?;
    if candidate.spec_revision != anchor.spec_revision
        || candidate.spec_hash != anchor.spec_hash
        || revision.candidate_hash != anchor.candidate_hash
        || revision.impl_hash != anchor.impl_hash
        || revision.equivalence.status != EquivalenceStatus::Proved
        || revision.guarded_fallback.is_some()
        || matches!(
            revision.state,
            CandidateState::Speculative | CandidateState::Guarded | CandidateState::Rejected
        )
        || revision
            .proof_debt
            .iter()
            .any(|debt| debt.status != ProofDebtStatus::Proved)
    {
        return Err(memory_error(
            ErrorCode::MemoryAnchorUnproved,
            "memory root requires a fully proved unconditional exact candidate revision",
        )
        .with_repair(
            "materialize an equality node into CandidateForest and use its exact revision",
        ));
    }
    verify_impl(&revision.impl_program, source, limits)?;
    if impl_hash(&revision.impl_program)? != anchor.impl_hash {
        return Err(memory_error(
            ErrorCode::MemoryAnchorUnproved,
            "memory anchor implementation hash failed recomputation",
        ));
    }
    Ok(revision.impl_program.clone())
}

fn verify_revision(
    store: &MemoryPlanStore,
    plan: &MemoryPlan,
    revision: &MemoryRevision,
    forest: &CandidateForest,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    let implementation = verify_anchor(forest, &plan.anchor, source, limits)?;
    if revision.impl_hash != plan.anchor.impl_hash {
        return Err(memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "memory revision impl_hash differs from immutable plan anchor",
        ));
    }
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryObligations,
        u64::try_from(revision.obligations.len()).unwrap_or(u64::MAX),
        "MemoryIR revision verification",
    )?;
    verify_memory_program(&revision.program, &implementation, limits)?;
    if revision
        .obligations
        .iter()
        .any(|obligation| obligation.status != MemoryObligationStatus::Proved)
    {
        return Err(memory_error(
            ErrorCode::MemoryEquivalenceUnproved,
            "memory revision retains an unproved correctness obligation",
        ));
    }
    let mut seen_evidence = BTreeSet::new();
    for evidence_id in &revision.evidence {
        if !seen_evidence.insert(evidence_id)
            || store.evidence.get(evidence_id).is_none_or(|evidence| {
                evidence.id != *evidence_id
                    || evidence.memory_plan != plan.id
                    || evidence.memory_revision != revision.id
                    || evidence.impl_hash != revision.impl_hash
                    || evidence.class != MemoryEvidenceClass::Correctness
                    || evidence.memory_semantics_version != MEMORY_SEMANTICS_VERSION
                    || evidence.validator_version != MEMORY_VALIDATOR_VERSION
            })
        {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory correctness evidence is missing or inconsistent",
            ));
        }
    }
    for certificate in &revision.certificates {
        if certificate.impl_hash != revision.impl_hash
            || certificate.memory_semantics_version != MEMORY_SEMANTICS_VERSION
            || certificate.validator_version != MEMORY_VALIDATOR_VERSION
            || !revision.evidence.contains(&certificate.evidence)
        {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory structural certificate is inconsistent",
            ));
        }
    }
    let actual_hash = memory_hash_with_limit(plan, revision, limits)?;
    if actual_hash != revision.memory_hash {
        return Err(memory_error(
            ErrorCode::MemoryHashMismatch,
            "memory exact-state hash failed recomputation",
        )
        .with_types(revision.memory_hash.to_string(), actual_hash.to_string()));
    }
    for evidence_id in &revision.evidence {
        if store.evidence[evidence_id].output_memory_hash != revision.memory_hash {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory evidence output hash differs from revision hash",
            ));
        }
    }
    for certificate in &revision.certificates {
        if certificate.after_memory_hash.as_ref() != Some(&revision.memory_hash) {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory certificate output hash differs from revision hash",
            ));
        }
    }
    Ok(())
}

fn make_query(
    store: &MemoryPlanStore,
    plan: &MemoryPlan,
    revision: &MemoryRevision,
) -> MemoryQuery {
    let (fresh, reused, guarded) = revision.program.reuse_decisions.values().fold(
        (0_usize, 0_usize, 0_usize),
        |(fresh, reused, guarded), decision| match decision {
            ReuseDecision::Fresh { .. } => (fresh + 1, reused, guarded),
            ReuseDecision::InPlace { .. } => (fresh, reused + 1, guarded),
            ReuseDecision::Guarded { .. } => (fresh, reused, guarded + 1),
        },
    );
    let (correctness, confidence) =
        revision
            .evidence
            .iter()
            .fold(
                (0_usize, 0_usize),
                |(correctness, confidence), evidence| match store.evidence.get(evidence) {
                    Some(MemoryEvidenceRecord {
                        class: MemoryEvidenceClass::Correctness,
                        ..
                    }) => (correctness + 1, confidence),
                    Some(MemoryEvidenceRecord {
                        class: MemoryEvidenceClass::Confidence,
                        ..
                    }) => (correctness, confidence + 1),
                    None => (correctness, confidence),
                },
            );
    MemoryQuery {
        memory_plan: plan.id.clone(),
        memory_revision: revision.id.clone(),
        memory_hash: revision.memory_hash.clone(),
        impl_hash: revision.impl_hash.clone(),
        status: revision.status,
        buffer_count: revision.program.buffers.len(),
        access_count: revision
            .program
            .operations
            .values()
            .map(|operation| operation.accesses.len())
            .sum(),
        fresh_buffer_count: fresh,
        reused_buffer_count: reused,
        guarded_buffer_count: guarded,
        open_obligations: revision
            .obligations
            .iter()
            .filter(|obligation| obligation.status == MemoryObligationStatus::Open)
            .map(|obligation| obligation.id.clone())
            .collect(),
        correctness_evidence: correctness,
        confidence_evidence: confidence,
    }
}

#[allow(clippy::too_many_arguments)]
fn attach_correctness(
    store: &mut MemoryPlanStore,
    plan: &mut MemoryPlan,
    revision: &mut MemoryRevision,
    before: Option<MemoryHash>,
    kind: MemoryEvidenceKind,
    method: &str,
    side_conditions: Vec<String>,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::MemoryObligations,
        u64::try_from(revision.obligations.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        "MemoryIR correctness publication",
    )?;
    let evidence_id = store.allocator.evidence();
    let obligation_id = store.allocator.obligation();
    revision.obligations.push(MemoryObligation {
        id: obligation_id,
        kind: "memory_equivalent_to_impl".to_owned(),
        status: MemoryObligationStatus::Proved,
        impl_hash: revision.impl_hash.clone(),
        details: BTreeMap::from([("method".to_owned(), method.to_owned())]),
    });
    revision.evidence.push(evidence_id.clone());
    revision.certificates.push(MemoryCertificate {
        method: method.to_owned(),
        impl_hash: revision.impl_hash.clone(),
        before_memory_hash: before.clone(),
        after_memory_hash: None,
        side_conditions,
        evidence: evidence_id.clone(),
        memory_semantics_version: MEMORY_SEMANTICS_VERSION,
        validator_version: MEMORY_VALIDATOR_VERSION,
    });
    revision.memory_hash = MemoryHash::new("pending");
    revision.memory_hash = memory_hash_with_limit(plan, revision, limits)?;
    let memory_hash = revision.memory_hash.clone();
    revision
        .certificates
        .last_mut()
        .expect("new certificate exists")
        .after_memory_hash = Some(memory_hash.clone());
    store.evidence.insert(
        evidence_id.clone(),
        MemoryEvidenceRecord {
            id: evidence_id,
            class: MemoryEvidenceClass::Correctness,
            kind,
            memory_plan: plan.id.clone(),
            memory_revision: revision.id.clone(),
            impl_hash: revision.impl_hash.clone(),
            input_memory_hash: before,
            output_memory_hash: memory_hash,
            method: method.to_owned(),
            memory_semantics_version: MEMORY_SEMANTICS_VERSION,
            validator_version: MEMORY_VALIDATOR_VERSION,
        },
    );
    Ok(())
}

fn update_buffer_and_baselines(
    program: &mut MemoryProgram,
    buffer: &BufferId,
    update: impl Fn(&mut MemoryBuffer),
) -> AgentResult<()> {
    let active = program.buffers.get_mut(buffer).ok_or_else(|| {
        memory_error(
            ErrorCode::BufferNotFound,
            format!("buffer `{buffer}` does not exist"),
        )
    })?;
    update(active);
    for baseline in program.fresh_baseline_buffers.values_mut() {
        if &baseline.id == buffer {
            update(baseline);
        }
    }
    Ok(())
}

impl MemoryPlanStore {
    /// Returns one persistent memory plan.
    pub fn plan(&self, plan: &MemoryPlanId) -> AgentResult<&MemoryPlan> {
        self.plans.get(plan).ok_or_else(|| {
            memory_error(
                ErrorCode::MemoryPlanNotFound,
                format!("memory plan `{plan}` does not exist"),
            )
        })
    }

    /// Returns one immutable memory revision.
    pub fn revision(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<&MemoryRevision> {
        plan_revision(self, plan, revision).map(|(_, revision)| revision)
    }

    /// Creates a conservative exact fresh MemoryIR root.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        forest: &CandidateForest,
        candidate: &CandidateId,
        candidate_revision: &CandidateRevisionId,
        source: &Program,
        spec_revision: &RevisionId,
        spec_hash: &SpecHash,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
    ) -> AgentResult<MemoryCheckReport> {
        BudgetCheck::against(
            limits,
            ResourceKind::MemoryPlansPerWorkspace,
            u64::try_from(self.plans.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "memory.create before persistent ID allocation",
        )?;
        let candidate_data = forest.candidate(candidate)?;
        let revision_data = forest.revision(candidate, candidate_revision)?;
        let anchor = MemoryAnchor {
            spec_revision: spec_revision.clone(),
            spec_hash: spec_hash.clone(),
            candidate: candidate.clone(),
            candidate_revision: candidate_revision.clone(),
            candidate_hash: revision_data.candidate_hash.clone(),
            impl_hash: revision_data.impl_hash.clone(),
        };
        if candidate_data.spec_revision != *spec_revision || candidate_data.spec_hash != *spec_hash
        {
            return Err(memory_error(
                ErrorCode::MemoryAnchorUnproved,
                "candidate and requested frozen SpecIR anchors differ",
            ));
        }
        let implementation = verify_anchor(forest, &anchor, source, limits)?;
        let mut staged = self.clone();
        let plan_id = staged.allocator.plan();
        let revision_id = staged.allocator.revision();
        let program = lower_fresh(&implementation, &mut staged.allocator)?;
        verify_memory_program(&program, &implementation, limits)?;
        let mut revision = MemoryRevision {
            id: revision_id.clone(),
            parents: Vec::new(),
            program,
            memory_hash: MemoryHash::new("pending"),
            impl_hash: anchor.impl_hash.clone(),
            status: MemoryStatus::Proved,
            obligations: Vec::new(),
            certificates: Vec::new(),
            evidence: Vec::new(),
        };
        let mut plan = MemoryPlan {
            id: plan_id.clone(),
            anchor,
            root_revision: revision_id.clone(),
            head: revision_id.clone(),
            revisions: BTreeMap::new(),
            parent_plan: None,
            forked_from_revision: None,
        };
        attach_correctness(
            &mut staged,
            &mut plan,
            &mut revision,
            None,
            MemoryEvidenceKind::FreshBufferization,
            "fresh_bufferization_v1",
            vec![
                "fully proved unconditional candidate anchor".to_owned(),
                "fresh buffer for every reachable tensor result".to_owned(),
                "compiler-owned alias and lifetime analysis".to_owned(),
                "MemoryEquivalentToImpl".to_owned(),
            ],
            limits,
        )?;
        let memory_hash = revision.memory_hash.clone();
        plan.revisions.insert(revision_id.clone(), revision);
        staged.plans.insert(plan_id.clone(), plan);
        staged.events.push(VersionedMemoryEvent {
            semantics_version: MEMORY_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            event: MemoryEvent::Created {
                candidate: candidate.clone(),
                candidate_revision: candidate_revision.clone(),
                memory_plan: plan_id.clone(),
                memory_revision: revision_id.clone(),
                memory_hash,
            },
        });
        ensure_store_budgets(&staged, limits)?;
        let report = staged.check(&plan_id, &revision_id, forest, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Reads one immutable memory revision summary.
    pub fn query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<MemoryQuery> {
        let (plan, revision) = plan_revision(self, plan, revision)?;
        Ok(make_query(self, plan, revision))
    }

    /// Fully verifies one memory revision against its immutable candidate anchor.
    pub fn check(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        forest: &CandidateForest,
        source: &Program,
        limits: &ResourceLimits,
    ) -> AgentResult<MemoryCheckReport> {
        let (plan_data, revision_data) = plan_revision(self, plan, revision)?;
        verify_revision(self, plan_data, revision_data, forest, source, limits)?;
        let query = make_query(self, plan_data, revision_data);
        Ok(MemoryCheckReport {
            sealable: matches!(
                revision_data.status,
                MemoryStatus::Proved | MemoryStatus::Guarded
            ),
            query,
            well_typed: true,
            equivalent_to_impl: true,
        })
    }

    /// Atomically applies compiler-verified storage decisions.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        transaction: &MemoryTransaction,
        forest: &CandidateForest,
        source: &Program,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
    ) -> AgentResult<MemoryCheckReport> {
        if transaction.actions.is_empty() {
            return Err(memory_error(
                ErrorCode::InvalidRequest,
                "memory transaction must contain at least one action",
            ));
        }
        BudgetCheck::against(
            limits,
            ResourceKind::MemoryReuseAttempts,
            u64::try_from(
                transaction
                    .actions
                    .iter()
                    .filter(|action| {
                        matches!(
                            action,
                            MemoryAction::RequestInPlaceReuse { .. }
                                | MemoryAction::RequestGuardedReuse { .. }
                        )
                    })
                    .count(),
            )
            .unwrap_or(u64::MAX),
            "memory transaction before graph clone",
        )?;
        let (plan, base) = plan_revision(
            self,
            &transaction.memory_plan,
            &transaction.base_memory_revision,
        )?;
        if plan.head != transaction.base_memory_revision {
            return Err(memory_error(
                ErrorCode::StaleMemoryBase,
                "memory transaction base is stale",
            )
            .with_detail("current_head", plan.head.to_string()));
        }
        if base.status == MemoryStatus::Sealed {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "sealed memory plan cannot be edited",
            ));
        }
        if base.memory_hash != transaction.expected_memory_hash {
            return Err(memory_error(
                ErrorCode::MemoryHashMismatch,
                "memory transaction expected hash is stale",
            )
            .with_types(
                transaction.expected_memory_hash.to_string(),
                base.memory_hash.to_string(),
            ));
        }
        if base.impl_hash != transaction.expected_impl_hash {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory transaction expected impl_hash differs from immutable anchor",
            ));
        }
        verify_revision(self, plan, base, forest, source, limits)?;
        let implementation = verify_anchor(forest, &plan.anchor, source, limits)?;
        let before = base.memory_hash.clone();
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base.id.clone()];
        next.status = MemoryStatus::Proved;
        next.obligations.clear();
        next.certificates.clear();
        next.evidence.clear();
        let mut method = "memory_transaction_v1";
        let mut evidence_kind = MemoryEvidenceKind::CompositionalMemoryVerification;
        let mut side_conditions = Vec::new();
        for action in &transaction.actions {
            match action {
                MemoryAction::ChooseFreshBuffer { result } => {
                    let fresh = next
                        .program
                        .fresh_baseline_buffers
                        .get(result)
                        .cloned()
                        .ok_or_else(|| {
                            memory_error(
                                ErrorCode::BufferNotFound,
                                "result has no compiler-owned fresh baseline",
                            )
                        })?;
                    next.program.buffers.insert(fresh.id.clone(), fresh.clone());
                    next.program.bindings.insert(
                        result.clone(),
                        MemoryBinding::Buffer {
                            value: result.clone(),
                            buffer: fresh.id.clone(),
                        },
                    );
                    next.program
                        .reuse_decisions
                        .insert(result.clone(), ReuseDecision::Fresh { buffer: fresh.id });
                    side_conditions.push("fresh exact fallback selected".to_owned());
                }
                MemoryAction::RequestInPlaceReuse { input, result } => {
                    let proved = prove_static_reuse(&next.program, &implementation, input, result)?;
                    let buffer = next
                        .program
                        .bindings
                        .get(input)
                        .and_then(MemoryBinding::buffer)
                        .cloned()
                        .ok_or_else(|| {
                            memory_error(ErrorCode::BufferNotFound, "reuse input has no buffer")
                        })?;
                    if let Some(fresh) = next.program.fresh_baseline_buffers.get(result) {
                        next.program.buffers.remove(&fresh.id);
                    }
                    next.program.bindings.insert(
                        result.clone(),
                        MemoryBinding::Buffer {
                            value: result.clone(),
                            buffer: buffer.clone(),
                        },
                    );
                    next.program.reuse_decisions.insert(
                        result.clone(),
                        ReuseDecision::InPlace {
                            input: input.clone(),
                            buffer,
                            certificate: "static_in_place_reuse_v1".to_owned(),
                        },
                    );
                    method = "static_in_place_reuse_v1";
                    evidence_kind = MemoryEvidenceKind::InPlaceReuse;
                    side_conditions.extend(proved);
                }
                MemoryAction::RequestGuardedReuse {
                    input,
                    result,
                    guard_against,
                } => {
                    let proved = prove_static_reuse(&next.program, &implementation, input, result)?;
                    let buffer = next
                        .program
                        .bindings
                        .get(input)
                        .and_then(MemoryBinding::buffer)
                        .cloned()
                        .ok_or_else(|| {
                            memory_error(ErrorCode::BufferNotFound, "reuse input has no buffer")
                        })?;
                    if &buffer == guard_against || !next.program.buffers.contains_key(guard_against)
                    {
                        return Err(memory_error(
                            ErrorCode::MemoryGuardInvalid,
                            "NoOverlap requires two distinct existing typed buffers",
                        ));
                    }
                    BudgetCheck::against(
                        limits,
                        ResourceKind::MemoryGuardDependencies,
                        4,
                        "compiler-owned NoOverlap guard",
                    )?;
                    BudgetCheck::against(
                        limits,
                        ResourceKind::MemoryGuardDepth,
                        1,
                        "compiler-owned NoOverlap guard",
                    )?;
                    BudgetCheck::against(
                        limits,
                        ResourceKind::MemoryFallbackDepth,
                        1,
                        "compiler-owned exact memory fallback",
                    )?;
                    let fresh = next
                        .program
                        .fresh_baseline_buffers
                        .get(result)
                        .cloned()
                        .ok_or_else(|| {
                            memory_error(
                                ErrorCode::MemoryFallbackInvalid,
                                "guarded reuse lacks a fresh baseline fallback",
                            )
                        })?;
                    next.program.buffers.insert(fresh.id.clone(), fresh.clone());
                    next.program.bindings.insert(
                        result.clone(),
                        MemoryBinding::Buffer {
                            value: result.clone(),
                            buffer: buffer.clone(),
                        },
                    );
                    next.program.reuse_decisions.insert(
                        result.clone(),
                        ReuseDecision::Guarded {
                            input: input.clone(),
                            buffer: buffer.clone(),
                            guard: Box::new(MemoryGuard {
                                id: staged.allocator.guard(),
                                predicate: MemoryGuardPredicate::NoOverlap,
                                primary_buffer: buffer,
                                other_buffer: guard_against.clone(),
                                dependencies: vec![
                                    "offset_elements".to_owned(),
                                    "shape".to_owned(),
                                    "strides".to_owned(),
                                    "element_type".to_owned(),
                                ],
                            }),
                            fallback: Box::new(MemoryFallback {
                                result: result.clone(),
                                fresh_buffer: fresh,
                                strategy: "lazy_fresh_allocation".to_owned(),
                            }),
                            certificate: "guarded_no_overlap_reuse_v1".to_owned(),
                        },
                    );
                    next.status = MemoryStatus::Guarded;
                    method = "guarded_no_overlap_reuse_v1";
                    evidence_kind = MemoryEvidenceKind::GuardedReuse;
                    side_conditions.extend(proved);
                    side_conditions
                        .push("false branch uses immutable lazy fresh fallback".to_owned());
                }
                MemoryAction::SetLayout { buffer, layout } => {
                    let requested = layout.clone();
                    update_buffer_and_baselines(&mut next.program, buffer, |buffer| {
                        buffer.layout = requested.clone();
                        if let MemoryLayout::ExplicitStrided { strides } = &requested {
                            buffer.strides = strides.clone();
                        } else if let Ok(strides) = contiguous_strides(&buffer.shape) {
                            buffer.strides = strides;
                        }
                    })?;
                    side_conditions.push("explicit layout verified".to_owned());
                }
                MemoryAction::SetAddressSpace {
                    buffer,
                    address_space,
                } => {
                    let requested = *address_space;
                    update_buffer_and_baselines(&mut next.program, buffer, |buffer| {
                        buffer.address_space = requested;
                    })?;
                    side_conditions.push("abstract address space preserves computation".to_owned());
                }
                MemoryAction::SetAlignment { buffer, alignment } => {
                    let requested = *alignment;
                    update_buffer_and_baselines(&mut next.program, buffer, |buffer| {
                        buffer.alignment = requested;
                    })?;
                    side_conditions.push("alignment verified with checked arithmetic".to_owned());
                }
            }
            rebuild_analysis(&mut next.program, &implementation)?;
            verify_memory_program(&next.program, &implementation, limits)?;
        }
        let mut plan_snapshot = staged.plan(&transaction.memory_plan)?.clone();
        attach_correctness(
            &mut staged,
            &mut plan_snapshot,
            &mut next,
            Some(before),
            evidence_kind,
            method,
            side_conditions,
            limits,
        )?;
        let memory_hash = next.memory_hash.clone();
        let plan_mut = staged
            .plans
            .get_mut(&transaction.memory_plan)
            .expect("memory plan was checked");
        plan_mut.revisions.insert(revision_id.clone(), next);
        plan_mut.head = revision_id.clone();
        staged.events.push(VersionedMemoryEvent {
            semantics_version: MEMORY_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            event: MemoryEvent::Applied {
                transaction: transaction.clone(),
                memory_revision: revision_id.clone(),
                memory_hash,
            },
        });
        ensure_store_budgets(&staged, limits)?;
        let report = staged.check(
            &transaction.memory_plan,
            &revision_id,
            forest,
            source,
            limits,
        )?;
        *self = staged;
        Ok(report)
    }

    /// Forks one immutable memory revision into an independent plan identity.
    #[allow(clippy::too_many_arguments)]
    pub fn fork(
        &mut self,
        parent_plan: &MemoryPlanId,
        parent_revision: &MemoryRevisionId,
        expected_memory_hash: &MemoryHash,
        forest: &CandidateForest,
        source: &Program,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
    ) -> AgentResult<MemoryCheckReport> {
        let (parent, revision) = plan_revision(self, parent_plan, parent_revision)?;
        if &revision.memory_hash != expected_memory_hash {
            return Err(memory_error(
                ErrorCode::MemoryHashMismatch,
                "memory fork expected hash is stale",
            ));
        }
        verify_revision(self, parent, revision, forest, source, limits)?;
        BudgetCheck::against(
            limits,
            ResourceKind::MemoryPlansPerWorkspace,
            u64::try_from(self.plans.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "memory fork before persistent ID allocation",
        )?;
        let mut staged = self.clone();
        let plan_id = staged.allocator.plan();
        let revision_id = staged.allocator.revision();
        let mut child_revision = revision.clone();
        child_revision.id = revision_id.clone();
        child_revision.parents.clear();
        child_revision.status = if revision.status == MemoryStatus::Sealed {
            MemoryStatus::Draft
        } else {
            revision.status
        };
        child_revision.evidence.clear();
        child_revision.certificates.clear();
        child_revision.obligations.clear();
        let mut child = MemoryPlan {
            id: plan_id.clone(),
            anchor: parent.anchor.clone(),
            root_revision: revision_id.clone(),
            head: revision_id.clone(),
            revisions: BTreeMap::new(),
            parent_plan: Some(parent_plan.clone()),
            forked_from_revision: Some(parent_revision.clone()),
        };
        attach_correctness(
            &mut staged,
            &mut child,
            &mut child_revision,
            Some(revision.memory_hash.clone()),
            MemoryEvidenceKind::CompositionalMemoryVerification,
            "forked_memory_plan_v1",
            vec!["immutable verified parent memory revision".to_owned()],
            limits,
        )?;
        let memory_hash = child_revision.memory_hash.clone();
        child.revisions.insert(revision_id.clone(), child_revision);
        staged.plans.insert(plan_id.clone(), child);
        staged.events.push(VersionedMemoryEvent {
            semantics_version: MEMORY_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            event: MemoryEvent::Forked {
                parent_plan: parent_plan.clone(),
                parent_revision: parent_revision.clone(),
                memory_plan: plan_id.clone(),
                memory_revision: revision_id.clone(),
                memory_hash,
            },
        });
        ensure_store_budgets(&staged, limits)?;
        let report = staged.check(&plan_id, &revision_id, forest, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Seals a structurally proved exact or guarded memory plan.
    #[allow(clippy::too_many_arguments)]
    pub fn seal(
        &mut self,
        plan_id: &MemoryPlanId,
        base_revision: &MemoryRevisionId,
        expected_memory_hash: &MemoryHash,
        forest: &CandidateForest,
        source: &Program,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
    ) -> AgentResult<MemoryCheckReport> {
        let (plan, base) = plan_revision(self, plan_id, base_revision)?;
        if plan.head != *base_revision {
            return Err(memory_error(
                ErrorCode::StaleMemoryBase,
                "memory seal base is stale",
            ));
        }
        if &base.memory_hash != expected_memory_hash {
            return Err(memory_error(
                ErrorCode::MemoryHashMismatch,
                "memory seal expected hash is stale",
            ));
        }
        verify_revision(self, plan, base, forest, source, limits)?;
        if base.status == MemoryStatus::Sealed {
            return self.check(plan_id, base_revision, forest, source, limits);
        }
        if !matches!(
            base.status,
            MemoryStatus::Proved | MemoryStatus::Guarded | MemoryStatus::Draft
        ) {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory plan cannot be sealed before structural proof",
            ));
        }
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision.clone()];
        next.status = MemoryStatus::Sealed;
        next.obligations.clear();
        next.certificates.clear();
        next.evidence.clear();
        let mut plan_snapshot = staged.plan(plan_id)?.clone();
        attach_correctness(
            &mut staged,
            &mut plan_snapshot,
            &mut next,
            Some(base.memory_hash.clone()),
            MemoryEvidenceKind::Sealing,
            "seal_memory_plan_v1",
            vec!["MemoryEquivalentToImpl verified compositionally".to_owned()],
            limits,
        )?;
        let memory_hash = next.memory_hash.clone();
        let plan_mut = staged.plans.get_mut(plan_id).expect("plan was checked");
        plan_mut.revisions.insert(revision_id.clone(), next);
        plan_mut.head = revision_id.clone();
        staged.events.push(VersionedMemoryEvent {
            semantics_version: MEMORY_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            event: MemoryEvent::Sealed {
                memory_plan: plan_id.clone(),
                base_revision: base_revision.clone(),
                expected_memory_hash: expected_memory_hash.clone(),
                memory_revision: revision_id.clone(),
                memory_hash,
            },
        });
        ensure_store_budgets(&staged, limits)?;
        let report = staged.check(plan_id, &revision_id, forest, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Returns a compiler-owned alias relation without changing state.
    pub fn alias_query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        first: &BufferId,
        second: &BufferId,
    ) -> AgentResult<AliasFact> {
        let revision = self.revision(plan, revision)?;
        crate::memory_ir::alias_relation(&revision.program, first, second)
    }

    /// Returns one typed buffer without changing state.
    pub fn buffer_query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        buffer: &BufferId,
    ) -> AgentResult<&MemoryBuffer> {
        self.revision(plan, revision)?
            .program
            .buffers
            .get(buffer)
            .ok_or_else(|| {
                memory_error(
                    ErrorCode::BufferNotFound,
                    format!("buffer `{buffer}` does not exist"),
                )
            })
    }

    /// Returns bounded parameteric legal storage choices without mutation.
    pub fn continuation(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        implementation: &ImplProgram,
        limits: &ResourceLimits,
    ) -> AgentResult<MemoryContinuation> {
        let revision_data = self.revision(plan, revision)?;
        let mut choices = Vec::new();
        for (result, fresh) in &revision_data.program.fresh_baseline_buffers {
            let definition = implementation.values.get(result).ok_or_else(|| {
                memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "continuation result is missing from ImplIR",
                )
            })?;
            let crate::impl_ir::ImplValueOrigin::Operation(operation_id) = &definition.origin;
            let operation = &implementation.operations[operation_id];
            for input in &operation.operands {
                if implementation.values[input].ty != definition.ty {
                    continue;
                }
                let proof =
                    prove_static_reuse(&revision_data.program, implementation, input, result);
                choices.push(MemoryContinuationChoice {
                    result: result.clone(),
                    input: input.clone(),
                    statically_applicable: proof.is_ok(),
                    failed_side_condition: proof.err().map(|error| error.code),
                    fresh_fallback: revision_data.program.buffers.contains_key(&fresh.id),
                    supported_guard_profile: "no_overlap".to_owned(),
                });
                BudgetCheck::against(
                    limits,
                    ResourceKind::MemoryReuseAttempts,
                    u64::try_from(choices.len()).unwrap_or(u64::MAX),
                    "memory continuation",
                )?;
            }
        }
        choices
            .sort_by(|left, right| (&left.result, &left.input).cmp(&(&right.result, &right.input)));
        Ok(MemoryContinuation {
            memory_plan: plan.clone(),
            memory_revision: revision.clone(),
            expected_memory_hash: revision_data.memory_hash.clone(),
            expected_impl_hash: revision_data.impl_hash.clone(),
            reuse_choices: choices,
            address_spaces: vec![
                AddressSpace::Global,
                AddressSpace::Shared,
                AddressSpace::Private,
                AddressSpace::Constant,
            ],
            layouts: vec![
                "contiguous_row_major".to_owned(),
                "explicit_strided".to_owned(),
            ],
        })
    }

    /// Verifies every plan, revision, hash, evidence record, and resource bound.
    pub fn verify_all<F>(
        &self,
        forest: &CandidateForest,
        mut source: F,
        limits: &ResourceLimits,
    ) -> AgentResult<()>
    where
        F: FnMut(&RevisionId) -> AgentResult<(Program, SpecHash)>,
    {
        ensure_store_budgets(self, limits)?;
        for event in &self.events {
            if event.semantics_version != MEMORY_EVENT_SEMANTICS_VERSION {
                return Err(memory_error(
                    ErrorCode::MemoryEventOrderInvalid,
                    "memory event uses an unsupported semantics version",
                ));
            }
        }
        let mut referenced_evidence = BTreeSet::new();
        for (id, plan) in &self.plans {
            if plan.id != *id || !plan.revisions.contains_key(&plan.head) {
                return Err(memory_error(
                    ErrorCode::MemoryEquivalenceUnproved,
                    "memory plan identity or head is inconsistent",
                ));
            }
            let (program, spec_hash) = source(&plan.anchor.spec_revision)?;
            if spec_hash != plan.anchor.spec_hash {
                return Err(memory_error(
                    ErrorCode::MemoryAnchorUnproved,
                    "memory plan spec_hash anchor is invalid",
                ));
            }
            for revision in plan.revisions.values() {
                verify_revision(self, plan, revision, forest, &program, limits)?;
                referenced_evidence.extend(revision.evidence.iter().cloned());
            }
        }
        if referenced_evidence != self.evidence.keys().cloned().collect() {
            return Err(memory_error(
                ErrorCode::MemoryEquivalenceUnproved,
                "memory evidence store contains missing or orphaned records",
            ));
        }
        Ok(())
    }
}
