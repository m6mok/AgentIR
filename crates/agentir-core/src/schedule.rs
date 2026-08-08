//! Persistent immutable ScheduleIR plan revisions and atomic schedule transactions.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        CandidateId, CandidateRevisionId, MemoryPlanId, MemoryRevisionId, RevisionId,
        ScheduleAxisId, ScheduleEvidenceId, ScheduleNodeId, ScheduleObligationId,
        ScheduleOperationId, SchedulePlanId, ScheduleRevisionId, TargetManifestId,
        TargetManifestRevisionId,
    },
    impl_ir::{ImplHash, ImplProgram},
    memory::{MemoryHash, MemoryPlan, MemoryRevision, MemoryStatus},
    schedule_ir::{
        AxisExtent, AxisRole, AxisTransform, BindingLevel, FusionGroup, SCHEDULE_SEMANTICS_VERSION,
        SCHEDULE_VALIDATOR_VERSION, ScheduleBinding, ScheduleIdAllocator, ScheduleProgram,
        ScheduleResourceEstimate, SplitSpec, TailStrategy, TileSpec, UnrollSpec, VectorizationSpec,
        build_serial_schedule, operation_buffers, vector_access_compatible,
        verify_and_refresh_schedule,
    },
    semantic::SpecHash,
    target::{TARGET_VALIDATOR_VERSION, TargetHash, TargetManifest},
    types::NumericContract,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, fmt::Write as _};

/// Exact ScheduleIR canonical codec version.
pub const SCHEDULE_CANONICAL_VERSION: u32 = 1;
/// Schedule event replay semantics version.
pub const SCHEDULE_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for exact ScheduleIR state hashes.
pub const SCHEDULE_HASH_DOMAIN: &[u8] = b"agentir.schedule.exact.v1\0";

/// SHA-256 identity of one exact ScheduleIR revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduleHash(String);

impl ScheduleHash {
    /// Creates a schedule hash from lowercase hexadecimal text.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns lowercase hexadecimal text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScheduleHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Schedule-local monotonic allocator isolated from all legacy allocators.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAllocator {
    plan: u64,
    revision: u64,
    node: u64,
    axis: u64,
    operation: u64,
    obligation: u64,
    evidence: u64,
}

impl ScheduleAllocator {
    fn plan(&mut self) -> SchedulePlanId {
        self.plan = self.plan.saturating_add(1);
        SchedulePlanId::new(format!("sp{}", self.plan))
    }

    fn revision(&mut self) -> ScheduleRevisionId {
        self.revision = self.revision.saturating_add(1);
        ScheduleRevisionId::new(format!("sr{}", self.revision))
    }

    fn obligation(&mut self) -> ScheduleObligationId {
        self.obligation = self.obligation.saturating_add(1);
        ScheduleObligationId::new(format!("so{}", self.obligation))
    }

    fn evidence(&mut self) -> ScheduleEvidenceId {
        self.evidence = self.evidence.saturating_add(1);
        ScheduleEvidenceId::new(format!("sev{}", self.evidence))
    }
}

impl ScheduleIdAllocator for ScheduleAllocator {
    fn schedule_node(&mut self) -> ScheduleNodeId {
        self.node = self.node.saturating_add(1);
        ScheduleNodeId::new(format!("sn{}", self.node))
    }

    fn schedule_operation(&mut self) -> ScheduleOperationId {
        self.operation = self.operation.saturating_add(1);
        ScheduleOperationId::new(format!("sop{}", self.operation))
    }

    fn schedule_axis(&mut self) -> ScheduleAxisId {
        self.axis = self.axis.saturating_add(1);
        ScheduleAxisId::new(format!("sa{}", self.axis))
    }
}

/// Immutable SpecIR/ImplIR/MemoryIR/Target anchor for one schedule plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAnchor {
    /// Frozen SpecIR revision.
    pub spec_revision: RevisionId,
    /// Frozen specification semantic hash.
    pub spec_hash: SpecHash,
    /// Exact candidate identity.
    pub candidate: CandidateId,
    /// Exact candidate revision.
    pub candidate_revision: CandidateRevisionId,
    /// Immutable implementation hash.
    pub impl_hash: ImplHash,
    /// Memory plan identity.
    pub memory_plan: MemoryPlanId,
    /// Immutable memory revision.
    pub memory_revision: MemoryRevisionId,
    /// Exact physical-plan hash.
    pub memory_hash: MemoryHash,
    /// Target manifest identity.
    pub target_manifest: TargetManifestId,
    /// Immutable target revision.
    pub target_revision: TargetManifestRevisionId,
    /// Exact target capability hash.
    pub target_hash: TargetHash,
    /// Immutable numerical contract.
    pub numeric_contract: NumericContract,
    /// Schedule semantics version at creation.
    pub schedule_semantics_version: u32,
    /// Schedule canonical version at creation.
    pub schedule_canonical_version: u32,
    /// Schedule validator version at creation.
    pub schedule_validator_version: u32,
}

/// Lifecycle state of one immutable schedule revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// Editable schedule fork.
    Draft,
    /// Typed domains and axes verified.
    WellTyped,
    /// ScheduleEquivalentToMemory structurally proved.
    Proved,
    /// Target resource capacities verified.
    ResourceValid,
    /// Immutable accepted schedule plan.
    Sealed,
    /// Integrity verification rejected the state.
    Rejected,
}

/// State of one compiler-owned schedule correctness obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleObligationStatus {
    /// Awaiting structural proof.
    Open,
    /// Discharged structurally by compiler core.
    Proved,
    /// Structural verification refuted the schedule.
    Refuted,
}

/// One persistent schedule correctness obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleObligation {
    /// Compiler-assigned identity.
    pub id: ScheduleObligationId,
    /// Stable relation name.
    pub kind: String,
    /// Proof state.
    pub status: ScheduleObligationStatus,
    /// Immutable physical anchor.
    pub memory_hash: MemoryHash,
    /// Stable normalized details.
    pub details: BTreeMap<String, String>,
}

/// Correctness or confidence strength for schedule evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleEvidenceClass {
    /// Compiler-owned structural correctness evidence.
    Correctness,
    /// Simulation/testing evidence that proves nothing.
    Confidence,
}

/// Compiler-owned schedule evidence method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleEvidenceKind {
    /// Conservative canonical serial root.
    SerialSchedule,
    /// Exact split/tile coordinate partition.
    Coverage,
    /// Restricted exact fusion proof.
    FusionLegality,
    /// Target hierarchy binding proof.
    BindingLegality,
    /// Exact vectorization proof.
    VectorizationLegality,
    /// Exact bounded unroll proof.
    UnrollLegality,
    /// Memory ordering, alias and lifetime compatibility proof.
    MemoryOrderCompatibility,
    /// Target-resource capacity proof.
    TargetResource,
    /// Whole-schedule compositional verification.
    CompositionalVerification,
    /// Immutable schedule sealing proof.
    Sealing,
    /// Reference schedule differential execution.
    DifferentialExecution,
}

/// One compiler-owned schedule evidence record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleEvidenceRecord {
    /// Compiler-assigned evidence identity.
    pub id: ScheduleEvidenceId,
    /// Evidence strength.
    pub class: ScheduleEvidenceClass,
    /// Stable method category.
    pub kind: ScheduleEvidenceKind,
    /// Schedule plan covered by this record.
    pub schedule_plan: SchedulePlanId,
    /// Immutable schedule revision.
    pub schedule_revision: ScheduleRevisionId,
    /// Immutable memory anchor.
    pub memory_hash: MemoryHash,
    /// Immutable target anchor.
    pub target_hash: TargetHash,
    /// Prior schedule hash, absent for a root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schedule_hash: Option<ScheduleHash>,
    /// Resulting exact schedule hash.
    pub output_schedule_hash: ScheduleHash,
    /// Stable compiler method.
    pub method: String,
    /// Schedule semantics version.
    pub semantics_version: u32,
    /// Validator version.
    pub validator_version: u32,
}

/// Compiler-owned structural schedule certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCertificate {
    /// Stable method.
    pub method: String,
    /// Immutable memory anchor.
    pub memory_hash: MemoryHash,
    /// Immutable target anchor.
    pub target_hash: TargetHash,
    /// Prior schedule hash, absent for a root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_schedule_hash: Option<ScheduleHash>,
    /// Result schedule hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_schedule_hash: Option<ScheduleHash>,
    /// Ordered compiler-owned side conditions.
    pub side_conditions: Vec<String>,
    /// Linked correctness evidence.
    pub evidence: ScheduleEvidenceId,
    /// Schedule semantics version.
    pub semantics_version: u32,
    /// Validator version.
    pub validator_version: u32,
}

/// One immutable ScheduleIR revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRevision {
    /// Compiler-assigned revision identity.
    pub id: ScheduleRevisionId,
    /// Immutable parents.
    pub parents: Vec<ScheduleRevisionId>,
    /// Separate typed schedule graph.
    pub program: ScheduleProgram,
    /// Exact schedule-state hash.
    pub schedule_hash: ScheduleHash,
    /// Immutable physical anchor.
    pub memory_hash: MemoryHash,
    /// Immutable target anchor.
    pub target_hash: TargetHash,
    /// Lifecycle state.
    pub status: ScheduleStatus,
    /// Compiler-owned obligations.
    pub obligations: Vec<ScheduleObligation>,
    /// Compiler-owned structural certificates.
    pub certificates: Vec<ScheduleCertificate>,
    /// Ordered evidence references.
    pub evidence: Vec<ScheduleEvidenceId>,
}

/// Persistent independent schedule-plan revision DAG.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulePlan {
    /// Plan identity.
    pub id: SchedulePlanId,
    /// Immutable four-layer anchor.
    pub anchor: ScheduleAnchor,
    /// Conservative serial root revision.
    pub root_revision: ScheduleRevisionId,
    /// Current schedule head.
    pub head: ScheduleRevisionId,
    /// Immutable revisions.
    pub revisions: BTreeMap<ScheduleRevisionId, ScheduleRevision>,
    /// Parent plan provenance for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_plan: Option<SchedulePlanId>,
    /// Parent revision provenance for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_revision: Option<ScheduleRevisionId>,
}

/// One compiler-verified scheduling action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleAction {
    /// Splits one active logical axis by a positive factor.
    SplitAxis {
        /// Active source axis.
        axis: ScheduleAxisId,
        /// Positive split factor.
        factor: u64,
    },
    /// Tiles corresponding active axes by positive sizes.
    TileAxes {
        /// Active source axes.
        axes: Vec<ScheduleAxisId>,
        /// Positive tile sizes.
        tile_sizes: Vec<u64>,
    },
    /// Fuses one restricted pointwise producer-consumer pair.
    FuseOperations {
        /// Producer node.
        producer: ScheduleNodeId,
        /// Consumer node.
        consumer: ScheduleNodeId,
    },
    /// Binds one active axis to the target hierarchy.
    BindAxis {
        /// Active axis.
        axis: ScheduleAxisId,
        /// Requested hierarchy level.
        level: BindingLevel,
    },
    /// Applies exact vectorization to one active axis.
    VectorizeAxis {
        /// Active axis.
        axis: ScheduleAxisId,
        /// Target-supported vector width.
        width: u64,
    },
    /// Applies bounded exact unrolling to one non-reduction axis.
    UnrollAxis {
        /// Active axis.
        axis: ScheduleAxisId,
        /// Positive bounded factor.
        factor: u64,
    },
    /// Restores serial binding on one axis.
    ChooseSerial {
        /// Active axis.
        axis: ScheduleAxisId,
    },
    /// Explicitly accepts the already compiler-generated remainder representation.
    ChooseCompilerRemainder {
        /// Axis carrying a compiler remainder.
        axis: ScheduleAxisId,
    },
    /// Selects explicit logical launch dimensions.
    SetLaunchShape {
        /// Positive grid dimensions.
        grid: [u64; 3],
        /// Positive local dimensions.
        workgroup: [u64; 3],
    },
}

/// Atomic schedule transaction against an explicit immutable base.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleTransaction {
    /// Schedule plan to edit.
    pub schedule_plan: SchedulePlanId,
    /// Explicit current base revision.
    pub base_schedule_revision: ScheduleRevisionId,
    /// Required exact base schedule hash.
    pub expected_schedule_hash: ScheduleHash,
    /// Required immutable MemoryIR hash.
    pub expected_memory_hash: MemoryHash,
    /// Required immutable target hash.
    pub expected_target_hash: TargetHash,
    /// Ordered compiler-verified actions.
    pub actions: Vec<ScheduleAction>,
}

/// Replayable schedule-plan event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleEvent {
    /// Conservative serial root creation.
    Created {
        /// Memory plan anchor.
        memory_plan: MemoryPlanId,
        /// Memory revision anchor.
        memory_revision: MemoryRevisionId,
        /// Target manifest anchor.
        target_manifest: TargetManifestId,
        /// Target revision anchor.
        target_revision: TargetManifestRevisionId,
        /// Expected plan ID.
        schedule_plan: SchedulePlanId,
        /// Expected root revision.
        schedule_revision: ScheduleRevisionId,
        /// Expected exact schedule hash.
        schedule_hash: ScheduleHash,
    },
    /// Atomic schedule transform publication.
    Applied {
        /// Replayable transaction.
        transaction: ScheduleTransaction,
        /// Expected child revision.
        schedule_revision: ScheduleRevisionId,
        /// Expected exact schedule hash.
        schedule_hash: ScheduleHash,
    },
    /// Explicit schedule-plan fork.
    Forked {
        /// Parent plan.
        parent_plan: SchedulePlanId,
        /// Parent revision.
        parent_revision: ScheduleRevisionId,
        /// Expected child plan.
        schedule_plan: SchedulePlanId,
        /// Expected child root.
        schedule_revision: ScheduleRevisionId,
        /// Expected exact hash.
        schedule_hash: ScheduleHash,
    },
    /// Immutable schedule sealing transition.
    Sealed {
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Explicit base revision.
        base_revision: ScheduleRevisionId,
        /// Required base hash.
        expected_schedule_hash: ScheduleHash,
        /// Expected sealed revision.
        schedule_revision: ScheduleRevisionId,
        /// Expected sealed hash.
        schedule_hash: ScheduleHash,
    },
}

/// Schedule event with explicit cross-store dependency cursors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedScheduleEvent {
    /// Schedule event semantics version.
    pub semantics_version: u32,
    /// Candidate event dependency cursor.
    pub candidate_event_cursor: u64,
    /// Equality event dependency cursor.
    pub equality_event_cursor: u64,
    /// Memory event dependency cursor.
    pub memory_event_cursor: u64,
    /// Target event dependency cursor.
    pub target_event_cursor: u64,
    /// Replayable schedule event.
    pub event: ScheduleEvent,
}

/// Read-only schedule summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleQuery {
    /// Schedule plan.
    pub schedule_plan: SchedulePlanId,
    /// Selected immutable revision.
    pub schedule_revision: ScheduleRevisionId,
    /// Exact schedule hash.
    pub schedule_hash: ScheduleHash,
    /// Immutable memory hash.
    pub memory_hash: MemoryHash,
    /// Immutable implementation hash.
    pub impl_hash: ImplHash,
    /// Immutable target hash.
    pub target_hash: TargetHash,
    /// Lifecycle state.
    pub status: ScheduleStatus,
    /// Scheduled high-level node count.
    pub node_count: usize,
    /// Logical domain count.
    pub domain_count: usize,
    /// Allocated axis count.
    pub axis_count: usize,
    /// Split count.
    pub split_count: usize,
    /// Tile count.
    pub tile_count: usize,
    /// Fusion group count.
    pub fusion_count: usize,
    /// Non-serial binding count.
    pub binding_count: usize,
    /// Vectorization count.
    pub vectorization_count: usize,
    /// Unroll count.
    pub unroll_count: usize,
    /// Compiler remainder count.
    pub remainder_count: usize,
    /// Open schedule obligations.
    pub open_obligations: Vec<ScheduleObligationId>,
    /// Analytical resource estimate.
    pub resource_estimate: ScheduleResourceEstimate,
    /// Correctness evidence count.
    pub correctness_evidence: usize,
    /// Confidence evidence count.
    pub confidence_evidence: usize,
}

/// Full structural schedule verification report.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCheckReport {
    /// Read-only summary.
    pub query: ScheduleQuery,
    /// Typed graph validity.
    pub well_typed: bool,
    /// Structural ScheduleEquivalentToMemory result.
    pub equivalent_to_memory: bool,
    /// Target resource capacity result.
    pub resource_valid: bool,
    /// Whether sealing is legal.
    pub sealable: bool,
}

/// Compiler-owned legality query result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleLegalityQuery {
    /// Stable query kind.
    pub kind: String,
    /// Whether all hard conditions hold.
    pub applicable: bool,
    /// Stable failed condition when not applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_side_condition: Option<ErrorCode>,
    /// Deterministic repair recommendation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
}

/// Bounded parametric schedule continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleContinuation {
    /// Plan inspected.
    pub schedule_plan: SchedulePlanId,
    /// Revision inspected.
    pub schedule_revision: ScheduleRevisionId,
    /// Required exact schedule hash.
    pub expected_schedule_hash: ScheduleHash,
    /// Required immutable memory hash.
    pub expected_memory_hash: MemoryHash,
    /// Required immutable target hash.
    pub expected_target_hash: TargetHash,
    /// Eligible active axes.
    pub eligible_axes: Vec<ScheduleAxisId>,
    /// Bounded positive split/tile factors.
    pub factors: Vec<u64>,
    /// Eligible producer-consumer fusion pairs.
    pub fusion_pairs: Vec<[ScheduleNodeId; 2]>,
    /// Supported hierarchy bindings.
    pub binding_levels: Vec<BindingLevel>,
    /// Supported vector widths.
    pub vector_widths: Vec<u64>,
    /// Bounded unroll factors.
    pub unroll_factors: Vec<u64>,
    /// Conservative serial fallback is always available.
    pub serial_fallback: bool,
    /// Compiler-owned exact remainder is available.
    pub compiler_remainder_available: bool,
    /// Current deterministic resource estimate.
    pub projected_resource_estimate: ScheduleResourceEstimate,
}

/// Persistent independent ScheduleIR plans, evidence, allocator and events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulePlanStore {
    /// Schedule plan branches.
    pub plans: BTreeMap<SchedulePlanId, SchedulePlan>,
    /// Compiler-owned schedule evidence.
    pub evidence: BTreeMap<ScheduleEvidenceId, ScheduleEvidenceRecord>,
    /// Schedule-local allocator.
    pub allocator: ScheduleAllocator,
    /// Ordered replayable schedule events.
    pub events: Vec<VersionedScheduleEvent>,
}

#[derive(Serialize)]
struct CanonicalCertificate<'a> {
    method: &'a str,
    memory_hash: &'a MemoryHash,
    target_hash: &'a TargetHash,
    before_schedule_hash: &'a Option<ScheduleHash>,
    side_conditions: &'a [String],
    evidence: &'a ScheduleEvidenceId,
    semantics_version: u32,
    validator_version: u32,
}

#[derive(Serialize)]
struct ScheduleHashModel<'a> {
    codec: &'static str,
    version: u32,
    semantics_version: u32,
    event_semantics_version: u32,
    validator_version: u32,
    schedule_plan: &'a SchedulePlanId,
    schedule_revision: &'a ScheduleRevisionId,
    parents: &'a [ScheduleRevisionId],
    anchor: &'a ScheduleAnchor,
    parent_plan: &'a Option<SchedulePlanId>,
    forked_from_revision: &'a Option<ScheduleRevisionId>,
    program: &'a ScheduleProgram,
    memory_hash: &'a MemoryHash,
    target_hash: &'a TargetHash,
    status: ScheduleStatus,
    obligations: &'a [ScheduleObligation],
    certificates: Vec<CanonicalCertificate<'a>>,
    evidence: &'a [ScheduleEvidenceId],
}

fn schedule_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn digest(bytes: &[u8]) -> ScheduleHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    ScheduleHash(output)
}

/// Returns deterministic exact ScheduleIR canonical bytes.
pub fn canonical_schedule_bytes(
    plan: &SchedulePlan,
    revision: &ScheduleRevision,
) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(&ScheduleHashModel {
        codec: "agentir.schedule.exact",
        version: SCHEDULE_CANONICAL_VERSION,
        semantics_version: SCHEDULE_SEMANTICS_VERSION,
        event_semantics_version: SCHEDULE_EVENT_SEMANTICS_VERSION,
        validator_version: SCHEDULE_VALIDATOR_VERSION,
        schedule_plan: &plan.id,
        schedule_revision: &revision.id,
        parents: &revision.parents,
        anchor: &plan.anchor,
        parent_plan: &plan.parent_plan,
        forked_from_revision: &plan.forked_from_revision,
        program: &revision.program,
        memory_hash: &revision.memory_hash,
        target_hash: &revision.target_hash,
        status: revision.status,
        obligations: &revision.obligations,
        certificates: revision
            .certificates
            .iter()
            .map(|certificate| CanonicalCertificate {
                method: &certificate.method,
                memory_hash: &certificate.memory_hash,
                target_hash: &certificate.target_hash,
                before_schedule_hash: &certificate.before_schedule_hash,
                side_conditions: &certificate.side_conditions,
                evidence: &certificate.evidence,
                semantics_version: certificate.semantics_version,
                validator_version: certificate.validator_version,
            })
            .collect(),
        evidence: &revision.evidence,
    })
    .map_err(|error| {
        schedule_error(
            ErrorCode::CanonicalizationFailed,
            format!("ScheduleIR exact-state serialization failed: {error}"),
        )
    })
}

fn schedule_hash(plan: &SchedulePlan, revision: &ScheduleRevision) -> AgentResult<ScheduleHash> {
    let bytes = canonical_schedule_bytes(plan, revision)?;
    let mut input = Vec::with_capacity(SCHEDULE_HASH_DOMAIN.len().saturating_add(bytes.len()));
    input.extend_from_slice(SCHEDULE_HASH_DOMAIN);
    input.extend_from_slice(&bytes);
    Ok(digest(&input))
}

fn plan_revision<'a>(
    store: &'a SchedulePlanStore,
    plan: &SchedulePlanId,
    revision: &ScheduleRevisionId,
) -> AgentResult<(&'a SchedulePlan, &'a ScheduleRevision)> {
    let plan_data = store.plans.get(plan).ok_or_else(|| {
        schedule_error(
            ErrorCode::SchedulePlanNotFound,
            format!("schedule plan `{plan}` does not exist"),
        )
    })?;
    let revision_data = plan_data.revisions.get(revision).ok_or_else(|| {
        schedule_error(
            ErrorCode::ScheduleRevisionNotFound,
            format!("schedule revision `{revision}` does not exist"),
        )
    })?;
    Ok((plan_data, revision_data))
}

fn validate_anchor(
    plan: &SchedulePlan,
    memory_plan: &MemoryPlan,
    memory_revision: &MemoryRevision,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<()> {
    if plan.anchor.spec_revision != memory_plan.anchor.spec_revision
        || plan.anchor.spec_hash != memory_plan.anchor.spec_hash
        || plan.anchor.candidate != memory_plan.anchor.candidate
        || plan.anchor.candidate_revision != memory_plan.anchor.candidate_revision
        || plan.anchor.impl_hash != memory_plan.anchor.impl_hash
        || plan.anchor.impl_hash != implementation_hash(implementation)?
        || plan.anchor.memory_plan != memory_plan.id
        || plan.anchor.memory_revision != memory_revision.id
        || plan.anchor.memory_hash != memory_revision.memory_hash
        || plan.anchor.target_manifest != target.id
        || plan.anchor.target_revision != target.revision
        || plan.anchor.target_hash != target.target_hash
        || plan.anchor.numeric_contract != implementation.numeric_contract
        || plan.anchor.schedule_semantics_version != SCHEDULE_SEMANTICS_VERSION
        || plan.anchor.schedule_canonical_version != SCHEDULE_CANONICAL_VERSION
        || plan.anchor.schedule_validator_version != SCHEDULE_VALIDATOR_VERSION
        || !matches!(
            memory_revision.status,
            MemoryStatus::Proved | MemoryStatus::Guarded | MemoryStatus::Sealed
        )
        || target.certificate.validator_version != TARGET_VALIDATOR_VERSION
    {
        return Err(schedule_error(
            ErrorCode::ScheduleEquivalenceUnproved,
            "schedule anchor does not match verified SpecIR/ImplIR/MemoryIR/Target state",
        )
        .with_repair("create a new serial schedule from the selected verified memory revision"));
    }
    Ok(())
}

fn implementation_hash(implementation: &ImplProgram) -> AgentResult<ImplHash> {
    crate::impl_ir::impl_hash(implementation)
}

fn attach_correctness(
    store: &mut SchedulePlanStore,
    plan: &SchedulePlan,
    revision: &mut ScheduleRevision,
    before: Option<ScheduleHash>,
    kind: ScheduleEvidenceKind,
    method: &str,
    side_conditions: Vec<String>,
) -> AgentResult<()> {
    let evidence_id = store.allocator.evidence();
    revision.obligations.push(ScheduleObligation {
        id: store.allocator.obligation(),
        kind: "schedule_equivalent_to_memory".to_owned(),
        status: ScheduleObligationStatus::Proved,
        memory_hash: revision.memory_hash.clone(),
        details: BTreeMap::from([("method".to_owned(), method.to_owned())]),
    });
    revision.evidence.push(evidence_id.clone());
    revision.certificates.push(ScheduleCertificate {
        method: method.to_owned(),
        memory_hash: revision.memory_hash.clone(),
        target_hash: revision.target_hash.clone(),
        before_schedule_hash: before.clone(),
        after_schedule_hash: None,
        side_conditions,
        evidence: evidence_id.clone(),
        semantics_version: SCHEDULE_SEMANTICS_VERSION,
        validator_version: SCHEDULE_VALIDATOR_VERSION,
    });
    revision.schedule_hash = ScheduleHash::new("pending");
    revision.schedule_hash = schedule_hash(plan, revision)?;
    let output = revision.schedule_hash.clone();
    revision
        .certificates
        .last_mut()
        .expect("new schedule certificate exists")
        .after_schedule_hash = Some(output.clone());
    store.evidence.insert(
        evidence_id.clone(),
        ScheduleEvidenceRecord {
            id: evidence_id,
            class: ScheduleEvidenceClass::Correctness,
            kind,
            schedule_plan: plan.id.clone(),
            schedule_revision: revision.id.clone(),
            memory_hash: revision.memory_hash.clone(),
            target_hash: revision.target_hash.clone(),
            input_schedule_hash: before,
            output_schedule_hash: output,
            method: method.to_owned(),
            semantics_version: SCHEDULE_SEMANTICS_VERSION,
            validator_version: SCHEDULE_VALIDATOR_VERSION,
        },
    );
    Ok(())
}

fn active_axis(program: &ScheduleProgram, axis: &ScheduleAxisId) -> AgentResult<()> {
    if program.nodes.values().any(|node| node.axes.contains(axis)) {
        Ok(())
    } else {
        Err(schedule_error(
            ErrorCode::InvalidScheduleAxis,
            format!("axis `{axis}` is not active"),
        ))
    }
}

fn split_axis(
    program: &mut ScheduleProgram,
    allocator: &mut ScheduleAllocator,
    axis_id: &ScheduleAxisId,
    factor: u64,
    tile: bool,
) -> AgentResult<SplitSpec> {
    if factor == 0 {
        return Err(schedule_error(
            if tile {
                ErrorCode::InvalidTile
            } else {
                ErrorCode::InvalidSplit
            },
            "split/tile factor must be positive",
        ));
    }
    active_axis(program, axis_id)?;
    let source = program.axes.get(axis_id).cloned().ok_or_else(|| {
        schedule_error(
            ErrorCode::InvalidScheduleAxis,
            "split source axis is missing",
        )
    })?;
    if source.role == AxisRole::Reduction {
        return Err(schedule_error(
            ErrorCode::ReductionOrderViolation,
            "Stage 4 v1 does not split or tile reduction axes",
        ));
    }
    let (outer_extent, tail) = match source.extent.static_value() {
        Some(extent) => {
            let quotient = extent / factor;
            let remainder = extent % factor;
            let outer = quotient
                .checked_add(u64::from(remainder != 0))
                .ok_or_else(|| {
                    schedule_error(ErrorCode::InvalidSplit, "split quotient overflowed u64")
                })?;
            (
                AxisExtent::Static { value: outer },
                if remainder == 0 {
                    TailStrategy::Exact
                } else {
                    TailStrategy::CompilerRemainder {
                        remainder: Some(remainder),
                    }
                },
            )
        }
        None => (
            source.extent.clone(),
            TailStrategy::CompilerRemainder { remainder: None },
        ),
    };
    let outer = allocator.schedule_axis();
    let inner = allocator.schedule_axis();
    let transform = |outer_part| {
        if tile {
            AxisTransform::Tile {
                parent: axis_id.clone(),
                tile_size: factor,
                outer: outer_part,
            }
        } else {
            AxisTransform::Split {
                parent: axis_id.clone(),
                factor,
                outer: outer_part,
            }
        }
    };
    program.axes.insert(
        outer.clone(),
        crate::schedule_ir::ScheduleAxis {
            id: outer.clone(),
            operation: source.operation.clone(),
            dimension: source.dimension,
            role: source.role,
            extent: outer_extent,
            transform: transform(true),
            binding: source.binding.clone(),
            tail: tail.clone(),
        },
    );
    program.axes.insert(
        inner.clone(),
        crate::schedule_ir::ScheduleAxis {
            id: inner.clone(),
            operation: source.operation,
            dimension: source.dimension,
            role: source.role,
            extent: AxisExtent::Static { value: factor },
            transform: transform(false),
            binding: Some(ScheduleBinding {
                level: BindingLevel::Serial,
                extent: Some(factor),
            }),
            tail: TailStrategy::Exact,
        },
    );
    for node in program.nodes.values_mut() {
        if let Some(position) = node.axes.iter().position(|candidate| candidate == axis_id) {
            node.axes
                .splice(position..=position, [outer.clone(), inner.clone()]);
        }
    }
    if let Some(domain) = program.domains.get_mut(&program.axes[&outer].operation) {
        if let Some(position) = domain
            .axes
            .iter()
            .position(|candidate| candidate == axis_id)
        {
            domain
                .axes
                .splice(position..=position, [outer.clone(), inner.clone()]);
        }
    }
    Ok(SplitSpec {
        axis: axis_id.clone(),
        factor,
        outer,
        inner,
        tail,
    })
}

fn is_pointwise(opcode: &str) -> bool {
    matches!(
        opcode,
        "add" | "sub" | "mul" | "div" | "fma" | "compare" | "select" | "cast" | "map" | "zip_map"
    )
}

fn apply_action(
    revision: &mut ScheduleRevision,
    action: &ScheduleAction,
    allocator: &mut ScheduleAllocator,
    memory: &crate::memory_ir::MemoryProgram,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<ScheduleEvidenceKind> {
    match action {
        ScheduleAction::SplitAxis { axis, factor } => {
            let split = split_axis(&mut revision.program, allocator, axis, *factor, false)?;
            revision.program.splits.push(split);
            Ok(ScheduleEvidenceKind::Coverage)
        }
        ScheduleAction::TileAxes { axes, tile_sizes } => {
            if axes.is_empty() || axes.len() != tile_sizes.len() || axes.len() > 8 {
                return Err(schedule_error(
                    ErrorCode::InvalidTile,
                    "tile axes and sizes must have equal bounded non-zero rank",
                ));
            }
            let mut splits = Vec::new();
            for (axis, size) in axes.iter().zip(tile_sizes) {
                splits.push(split_axis(
                    &mut revision.program,
                    allocator,
                    axis,
                    *size,
                    true,
                )?);
            }
            revision.program.tiles.push(TileSpec {
                axes: axes.clone(),
                tile_sizes: tile_sizes.clone(),
                splits,
            });
            Ok(ScheduleEvidenceKind::Coverage)
        }
        ScheduleAction::FuseOperations { producer, consumer } => {
            let producer_node = revision
                .program
                .nodes
                .get(producer)
                .cloned()
                .ok_or_else(|| {
                    schedule_error(ErrorCode::IllegalFusion, "fusion producer does not exist")
                })?;
            let consumer_node = revision
                .program
                .nodes
                .get(consumer)
                .cloned()
                .ok_or_else(|| {
                    schedule_error(ErrorCode::IllegalFusion, "fusion consumer does not exist")
                })?;
            let dependent =
                revision.program.dependencies.iter().any(|dependency| {
                    dependency.source == *producer && dependency.target == *consumer
                });
            let producer_users = revision
                .program
                .dependencies
                .iter()
                .filter(|dependency| dependency.source == *producer)
                .count();
            let same_domain = producer_node
                .axes
                .iter()
                .map(|axis| &revision.program.axes[axis].extent)
                .eq(consumer_node
                    .axes
                    .iter()
                    .map(|axis| &revision.program.axes[axis].extent));
            if !dependent
                || producer_users != 1
                || !same_domain
                || !is_pointwise(&producer_node.opcode)
                || !is_pointwise(&consumer_node.opcode)
            {
                return Err(schedule_error(
                    ErrorCode::IllegalFusion,
                    "fusion requires one pointwise producer-consumer with an identical domain and no conflicting users",
                )
                .with_repair("keep the canonical serial operation boundary"));
            }
            let id = u64::try_from(revision.program.fusion_groups.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            revision.program.fusion_groups.push(FusionGroup {
                id,
                members: vec![producer.clone(), consumer.clone()],
                certificate: "restricted_pointwise_fusion_v1".to_owned(),
            });
            revision
                .program
                .nodes
                .get_mut(producer)
                .expect("checked")
                .fusion_group = Some(id);
            revision
                .program
                .nodes
                .get_mut(consumer)
                .expect("checked")
                .fusion_group = Some(id);
            Ok(ScheduleEvidenceKind::FusionLegality)
        }
        ScheduleAction::BindAxis { axis, level } => {
            active_axis(&revision.program, axis)?;
            let axis_data = revision.program.axes.get(axis).expect("checked");
            if axis_data.role == AxisRole::Reduction && *level != BindingLevel::Serial {
                return Err(schedule_error(
                    ErrorCode::ReductionOrderViolation,
                    "reduction axes remain serial in ScheduleIR v1",
                ));
            }
            if *level == BindingLevel::SubgroupLane && !target.subgroup.lane_binding {
                return Err(schedule_error(
                    ErrorCode::TargetCapabilityUnsupported,
                    "target lacks subgroup-lane binding",
                ));
            }
            if *level == BindingLevel::VectorLane {
                return Err(schedule_error(
                    ErrorCode::InvalidBinding,
                    "vector_lane is compiler-owned by vectorize_axis",
                ));
            }
            let extent = axis_data.extent.static_value();
            revision
                .program
                .axes
                .get_mut(axis)
                .expect("checked")
                .binding = Some(ScheduleBinding {
                level: *level,
                extent,
            });
            Ok(ScheduleEvidenceKind::BindingLegality)
        }
        ScheduleAction::VectorizeAxis { axis, width } => {
            active_axis(&revision.program, axis)?;
            if *width == 0 || !target.vector.widths.contains(width) {
                return Err(schedule_error(
                    ErrorCode::VectorWidthUnsupported,
                    "target does not support the requested vector width",
                ));
            }
            let axis_data = revision.program.axes.get(axis).expect("checked");
            if axis_data.role == AxisRole::Reduction {
                return Err(schedule_error(
                    ErrorCode::ReductionOrderViolation,
                    "reduction vectorization is not supported in ScheduleIR v1",
                ));
            }
            for buffer in operation_buffers(memory, implementation, &axis_data.operation) {
                vector_access_compatible(memory, &buffer, *width)?;
            }
            revision.program.vectorizations.push(VectorizationSpec {
                axis: axis.clone(),
                width: *width,
                certificate: "exact_vectorization_v1".to_owned(),
            });
            revision
                .program
                .axes
                .get_mut(axis)
                .expect("checked")
                .binding = Some(ScheduleBinding {
                level: BindingLevel::VectorLane,
                extent: Some(*width),
            });
            Ok(ScheduleEvidenceKind::VectorizationLegality)
        }
        ScheduleAction::UnrollAxis { axis, factor } => {
            active_axis(&revision.program, axis)?;
            let axis_data = revision.program.axes.get(axis).expect("checked");
            if *factor == 0 || *factor > 256 || axis_data.role == AxisRole::Reduction {
                return Err(schedule_error(
                    ErrorCode::InvalidUnroll,
                    "unroll factor must be in 1..=256 on a non-reduction axis",
                ));
            }
            revision.program.unrolls.push(UnrollSpec {
                axis: axis.clone(),
                factor: *factor,
                certificate: "bounded_exact_unroll_v1".to_owned(),
            });
            Ok(ScheduleEvidenceKind::UnrollLegality)
        }
        ScheduleAction::ChooseSerial { axis } => {
            active_axis(&revision.program, axis)?;
            let extent = revision.program.axes[axis].extent.static_value();
            revision
                .program
                .axes
                .get_mut(axis)
                .expect("checked")
                .binding = Some(ScheduleBinding {
                level: BindingLevel::Serial,
                extent,
            });
            revision
                .program
                .vectorizations
                .retain(|value| value.axis != *axis);
            Ok(ScheduleEvidenceKind::BindingLegality)
        }
        ScheduleAction::ChooseCompilerRemainder { axis } => {
            let axis = revision.program.axes.get(axis).ok_or_else(|| {
                schedule_error(
                    ErrorCode::InvalidScheduleAxis,
                    "remainder axis does not exist",
                )
            })?;
            if !matches!(axis.tail, TailStrategy::CompilerRemainder { .. }) {
                return Err(schedule_error(
                    ErrorCode::InvalidSplit,
                    "selected axis has no compiler remainder",
                ));
            }
            Ok(ScheduleEvidenceKind::Coverage)
        }
        ScheduleAction::SetLaunchShape { grid, workgroup } => {
            if grid.contains(&0) || workgroup.contains(&0) {
                return Err(schedule_error(
                    ErrorCode::InvalidBinding,
                    "launch dimensions must be positive",
                ));
            }
            revision.program.launch_grid = Some(*grid);
            revision.program.launch_workgroup = Some(*workgroup);
            Ok(ScheduleEvidenceKind::TargetResource)
        }
    }
}

fn make_query(
    store: &SchedulePlanStore,
    plan: &SchedulePlan,
    revision: &ScheduleRevision,
) -> ScheduleQuery {
    let (correctness, confidence) =
        revision
            .evidence
            .iter()
            .fold(
                (0_usize, 0_usize),
                |(correctness, confidence), id| match store
                    .evidence
                    .get(id)
                    .map(|value| value.class)
                {
                    Some(ScheduleEvidenceClass::Correctness) => (correctness + 1, confidence),
                    Some(ScheduleEvidenceClass::Confidence) => (correctness, confidence + 1),
                    None => (correctness, confidence),
                },
            );
    ScheduleQuery {
        schedule_plan: plan.id.clone(),
        schedule_revision: revision.id.clone(),
        schedule_hash: revision.schedule_hash.clone(),
        memory_hash: revision.memory_hash.clone(),
        impl_hash: plan.anchor.impl_hash.clone(),
        target_hash: revision.target_hash.clone(),
        status: revision.status,
        node_count: revision.program.nodes.len(),
        domain_count: revision.program.domains.len(),
        axis_count: revision.program.axes.len(),
        split_count: revision.program.splits.len(),
        tile_count: revision.program.tiles.len(),
        fusion_count: revision.program.fusion_groups.len(),
        binding_count: revision
            .program
            .axes
            .values()
            .filter(|axis| {
                axis.binding
                    .as_ref()
                    .is_some_and(|binding| binding.level != BindingLevel::Serial)
            })
            .count(),
        vectorization_count: revision.program.vectorizations.len(),
        unroll_count: revision.program.unrolls.len(),
        remainder_count: revision
            .program
            .axes
            .values()
            .filter(|axis| matches!(axis.tail, TailStrategy::CompilerRemainder { .. }))
            .count(),
        open_obligations: revision
            .obligations
            .iter()
            .filter(|obligation| obligation.status == ScheduleObligationStatus::Open)
            .map(|obligation| obligation.id.clone())
            .collect(),
        resource_estimate: revision.program.resource_estimate.clone(),
        correctness_evidence: correctness,
        confidence_evidence: confidence,
    }
}

fn verify_revision(
    store: &SchedulePlanStore,
    plan: &SchedulePlan,
    revision: &ScheduleRevision,
    memory_plan: &MemoryPlan,
    memory_revision: &MemoryRevision,
    implementation: &ImplProgram,
    target: &TargetManifest,
) -> AgentResult<()> {
    validate_anchor(plan, memory_plan, memory_revision, implementation, target)?;
    if revision.memory_hash != plan.anchor.memory_hash
        || revision.target_hash != plan.anchor.target_hash
        || revision
            .obligations
            .iter()
            .any(|obligation| obligation.status != ScheduleObligationStatus::Proved)
    {
        return Err(schedule_error(
            ErrorCode::ScheduleEquivalenceUnproved,
            "schedule revision differs from its immutable anchors or retains proof debt",
        ));
    }
    let mut refreshed = revision.program.clone();
    verify_and_refresh_schedule(
        &mut refreshed,
        &memory_revision.program,
        implementation,
        target,
    )?;
    if refreshed != revision.program {
        return Err(schedule_error(
            ErrorCode::ScheduleEquivalenceUnproved,
            "stored compiler-owned schedule analysis differs from recomputation",
        ));
    }
    for certificate in &revision.certificates {
        if certificate.memory_hash != revision.memory_hash
            || certificate.target_hash != revision.target_hash
            || certificate.semantics_version != SCHEDULE_SEMANTICS_VERSION
            || certificate.validator_version != SCHEDULE_VALIDATOR_VERSION
            || !revision.evidence.contains(&certificate.evidence)
        {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule certificate is inconsistent",
            ));
        }
    }
    for evidence in &revision.evidence {
        let evidence = store.evidence.get(evidence).ok_or_else(|| {
            schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule evidence record is missing",
            )
        })?;
        if evidence.class != ScheduleEvidenceClass::Correctness
            || evidence.schedule_plan != plan.id
            || evidence.schedule_revision != revision.id
            || evidence.memory_hash != revision.memory_hash
            || evidence.target_hash != revision.target_hash
        {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule evidence is inconsistent",
            ));
        }
    }
    let actual = schedule_hash(plan, revision)?;
    if actual != revision.schedule_hash {
        return Err(schedule_error(
            ErrorCode::ScheduleHashMismatch,
            "schedule exact-state hash failed recomputation",
        )
        .with_types(revision.schedule_hash.to_string(), actual.to_string()));
    }
    for evidence in &revision.evidence {
        if store.evidence[evidence].output_schedule_hash != revision.schedule_hash {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule evidence output hash differs from revision hash",
            ));
        }
    }
    for certificate in &revision.certificates {
        if certificate.after_schedule_hash.as_ref() != Some(&revision.schedule_hash) {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule certificate output hash differs from revision hash",
            ));
        }
    }
    Ok(())
}

impl SchedulePlanStore {
    /// Returns one persistent schedule plan.
    pub fn plan(&self, plan: &SchedulePlanId) -> AgentResult<&SchedulePlan> {
        self.plans.get(plan).ok_or_else(|| {
            schedule_error(
                ErrorCode::SchedulePlanNotFound,
                format!("schedule plan `{plan}` does not exist"),
            )
        })
    }

    /// Returns one immutable schedule revision.
    pub fn revision(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<&ScheduleRevision> {
        plan_revision(self, plan, revision).map(|(_, revision)| revision)
    }

    /// Creates a conservative exact serial ScheduleIR root.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        memory_plan: &MemoryPlan,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
        memory_event_cursor: u64,
        target_event_cursor: u64,
    ) -> AgentResult<ScheduleCheckReport> {
        if !matches!(
            memory_revision.status,
            MemoryStatus::Proved | MemoryStatus::Guarded | MemoryStatus::Sealed
        ) {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule.create requires a fully proved MemoryIR revision",
            ));
        }
        let mut staged = self.clone();
        let plan_id = staged.allocator.plan();
        let revision_id = staged.allocator.revision();
        let anchor = ScheduleAnchor {
            spec_revision: memory_plan.anchor.spec_revision.clone(),
            spec_hash: memory_plan.anchor.spec_hash.clone(),
            candidate: memory_plan.anchor.candidate.clone(),
            candidate_revision: memory_plan.anchor.candidate_revision.clone(),
            impl_hash: memory_plan.anchor.impl_hash.clone(),
            memory_plan: memory_plan.id.clone(),
            memory_revision: memory_revision.id.clone(),
            memory_hash: memory_revision.memory_hash.clone(),
            target_manifest: target.id.clone(),
            target_revision: target.revision.clone(),
            target_hash: target.target_hash.clone(),
            numeric_contract: implementation.numeric_contract.clone(),
            schedule_semantics_version: SCHEDULE_SEMANTICS_VERSION,
            schedule_canonical_version: SCHEDULE_CANONICAL_VERSION,
            schedule_validator_version: SCHEDULE_VALIDATOR_VERSION,
        };
        let mut program = build_serial_schedule(
            &memory_revision.program,
            implementation,
            &mut staged.allocator,
        )?;
        verify_and_refresh_schedule(
            &mut program,
            &memory_revision.program,
            implementation,
            target,
        )?;
        let mut revision = ScheduleRevision {
            id: revision_id.clone(),
            parents: Vec::new(),
            program,
            schedule_hash: ScheduleHash::new("pending"),
            memory_hash: memory_revision.memory_hash.clone(),
            target_hash: target.target_hash.clone(),
            status: ScheduleStatus::ResourceValid,
            obligations: Vec::new(),
            certificates: Vec::new(),
            evidence: Vec::new(),
        };
        let mut plan = SchedulePlan {
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
            &plan,
            &mut revision,
            None,
            ScheduleEvidenceKind::SerialSchedule,
            "serial_schedule_equivalent_to_memory_v1",
            vec![
                "canonical MemoryIR operation order".to_owned(),
                "serial binding for every logical axis".to_owned(),
                "complete exact coordinate coverage".to_owned(),
                "fixed reduction order".to_owned(),
                "target resource capacities satisfied".to_owned(),
            ],
        )?;
        let hash = revision.schedule_hash.clone();
        plan.revisions.insert(revision_id.clone(), revision);
        staged.plans.insert(plan_id.clone(), plan);
        staged.events.push(VersionedScheduleEvent {
            semantics_version: SCHEDULE_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            memory_event_cursor,
            target_event_cursor,
            event: ScheduleEvent::Created {
                memory_plan: memory_plan.id.clone(),
                memory_revision: memory_revision.id.clone(),
                target_manifest: target.id.clone(),
                target_revision: target.revision.clone(),
                schedule_plan: plan_id.clone(),
                schedule_revision: revision_id.clone(),
                schedule_hash: hash,
            },
        });
        let report = staged.check(
            &plan_id,
            &revision_id,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        *self = staged;
        Ok(report)
    }

    /// Returns one immutable schedule summary.
    pub fn query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<ScheduleQuery> {
        let (plan, revision) = plan_revision(self, plan, revision)?;
        Ok(make_query(self, plan, revision))
    }

    /// Fully verifies one schedule revision against MemoryIR and target anchors.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        memory_plan: &MemoryPlan,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
    ) -> AgentResult<ScheduleCheckReport> {
        let (plan_data, revision_data) = plan_revision(self, plan, revision)?;
        verify_revision(
            self,
            plan_data,
            revision_data,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        Ok(ScheduleCheckReport {
            query: make_query(self, plan_data, revision_data),
            well_typed: true,
            equivalent_to_memory: true,
            resource_valid: revision_data
                .program
                .resource_estimate
                .capacity_violations
                .is_empty(),
            sealable: matches!(
                revision_data.status,
                ScheduleStatus::Proved | ScheduleStatus::ResourceValid
            ),
        })
    }

    /// Applies one atomic compiler-verified schedule transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        transaction: &ScheduleTransaction,
        memory_plan: &MemoryPlan,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
        memory_event_cursor: u64,
        target_event_cursor: u64,
    ) -> AgentResult<ScheduleCheckReport> {
        if transaction.actions.is_empty() {
            return Err(schedule_error(
                ErrorCode::InvalidRequest,
                "schedule transaction must contain at least one action",
            ));
        }
        let (plan, base) = plan_revision(
            self,
            &transaction.schedule_plan,
            &transaction.base_schedule_revision,
        )?;
        if plan.head != transaction.base_schedule_revision {
            return Err(schedule_error(
                ErrorCode::StaleScheduleBase,
                "schedule transaction base is stale",
            )
            .with_detail("current_head", plan.head.to_string()));
        }
        if base.status == ScheduleStatus::Sealed {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "sealed schedule plan cannot be edited",
            ));
        }
        if base.schedule_hash != transaction.expected_schedule_hash {
            return Err(schedule_error(
                ErrorCode::ScheduleHashMismatch,
                "schedule transaction expected hash is stale",
            )
            .with_types(
                transaction.expected_schedule_hash.to_string(),
                base.schedule_hash.to_string(),
            ));
        }
        if base.memory_hash != transaction.expected_memory_hash
            || base.target_hash != transaction.expected_target_hash
        {
            return Err(schedule_error(
                ErrorCode::ScheduleEquivalenceUnproved,
                "schedule transaction immutable input hash differs from its anchor",
            ));
        }
        verify_revision(
            self,
            plan,
            base,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        let before = base.schedule_hash.clone();
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base.id.clone()];
        next.status = ScheduleStatus::ResourceValid;
        next.obligations.clear();
        next.certificates.clear();
        next.evidence.clear();
        let mut evidence_kind = ScheduleEvidenceKind::CompositionalVerification;
        for action in &transaction.actions {
            evidence_kind = apply_action(
                &mut next,
                action,
                &mut staged.allocator,
                &memory_revision.program,
                implementation,
                target,
            )?;
            verify_and_refresh_schedule(
                &mut next.program,
                &memory_revision.program,
                implementation,
                target,
            )?;
        }
        let plan_snapshot = staged.plan(&transaction.schedule_plan)?.clone();
        attach_correctness(
            &mut staged,
            &plan_snapshot,
            &mut next,
            Some(before),
            evidence_kind,
            "compositional_schedule_verification_v1",
            vec![
                "complete exact iteration coverage".to_owned(),
                "dependence and reduction order preserved".to_owned(),
                "MemoryIR alias/lifetime decisions preserved".to_owned(),
                "target capabilities and capacities satisfied".to_owned(),
            ],
        )?;
        let hash = next.schedule_hash.clone();
        let plan_mut = staged
            .plans
            .get_mut(&transaction.schedule_plan)
            .expect("checked schedule plan exists");
        plan_mut.revisions.insert(revision_id.clone(), next);
        plan_mut.head = revision_id.clone();
        staged.events.push(VersionedScheduleEvent {
            semantics_version: SCHEDULE_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            memory_event_cursor,
            target_event_cursor,
            event: ScheduleEvent::Applied {
                transaction: transaction.clone(),
                schedule_revision: revision_id.clone(),
                schedule_hash: hash,
            },
        });
        let report = staged.check(
            &transaction.schedule_plan,
            &revision_id,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        *self = staged;
        Ok(report)
    }

    /// Forks an immutable schedule revision into an independent plan.
    #[allow(clippy::too_many_arguments)]
    pub fn fork(
        &mut self,
        parent_plan: &SchedulePlanId,
        parent_revision: &ScheduleRevisionId,
        expected_schedule_hash: &ScheduleHash,
        memory_plan: &MemoryPlan,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
        memory_event_cursor: u64,
        target_event_cursor: u64,
    ) -> AgentResult<ScheduleCheckReport> {
        let (parent, revision) = plan_revision(self, parent_plan, parent_revision)?;
        if &revision.schedule_hash != expected_schedule_hash {
            return Err(schedule_error(
                ErrorCode::ScheduleHashMismatch,
                "schedule fork expected hash is stale",
            ));
        }
        verify_revision(
            self,
            parent,
            revision,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        let mut staged = self.clone();
        let plan_id = staged.allocator.plan();
        let revision_id = staged.allocator.revision();
        let mut child_revision = revision.clone();
        child_revision.id = revision_id.clone();
        child_revision.parents.clear();
        child_revision.status = ScheduleStatus::Draft;
        child_revision.obligations.clear();
        child_revision.certificates.clear();
        child_revision.evidence.clear();
        let mut child = SchedulePlan {
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
            &child,
            &mut child_revision,
            Some(revision.schedule_hash.clone()),
            ScheduleEvidenceKind::CompositionalVerification,
            "forked_schedule_plan_v1",
            vec!["immutable verified parent schedule revision".to_owned()],
        )?;
        let hash = child_revision.schedule_hash.clone();
        child.revisions.insert(revision_id.clone(), child_revision);
        staged.plans.insert(plan_id.clone(), child);
        staged.events.push(VersionedScheduleEvent {
            semantics_version: SCHEDULE_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            memory_event_cursor,
            target_event_cursor,
            event: ScheduleEvent::Forked {
                parent_plan: parent_plan.clone(),
                parent_revision: parent_revision.clone(),
                schedule_plan: plan_id.clone(),
                schedule_revision: revision_id.clone(),
                schedule_hash: hash,
            },
        });
        let report = staged.check(
            &plan_id,
            &revision_id,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        *self = staged;
        Ok(report)
    }

    /// Seals a structurally proved resource-valid schedule plan.
    #[allow(clippy::too_many_arguments)]
    pub fn seal(
        &mut self,
        plan_id: &SchedulePlanId,
        base_revision: &ScheduleRevisionId,
        expected_schedule_hash: &ScheduleHash,
        memory_plan: &MemoryPlan,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
        memory_event_cursor: u64,
        target_event_cursor: u64,
    ) -> AgentResult<ScheduleCheckReport> {
        let (plan, base) = plan_revision(self, plan_id, base_revision)?;
        if plan.head != *base_revision {
            return Err(schedule_error(
                ErrorCode::StaleScheduleBase,
                "schedule seal base is stale",
            ));
        }
        if &base.schedule_hash != expected_schedule_hash {
            return Err(schedule_error(
                ErrorCode::ScheduleHashMismatch,
                "schedule seal expected hash is stale",
            ));
        }
        verify_revision(
            self,
            plan,
            base,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        if base.status == ScheduleStatus::Sealed {
            return self.check(
                plan_id,
                base_revision,
                memory_plan,
                memory_revision,
                implementation,
                target,
            );
        }
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base.id.clone()];
        next.status = ScheduleStatus::Sealed;
        next.obligations.clear();
        next.certificates.clear();
        next.evidence.clear();
        let plan_snapshot = staged.plan(plan_id)?.clone();
        attach_correctness(
            &mut staged,
            &plan_snapshot,
            &mut next,
            Some(base.schedule_hash.clone()),
            ScheduleEvidenceKind::Sealing,
            "seal_schedule_plan_v1",
            vec!["ScheduleEquivalentToMemory verified compositionally".to_owned()],
        )?;
        let hash = next.schedule_hash.clone();
        let plan_mut = staged.plans.get_mut(plan_id).expect("checked plan exists");
        plan_mut.revisions.insert(revision_id.clone(), next);
        plan_mut.head = revision_id.clone();
        staged.events.push(VersionedScheduleEvent {
            semantics_version: SCHEDULE_EVENT_SEMANTICS_VERSION,
            candidate_event_cursor,
            equality_event_cursor,
            memory_event_cursor,
            target_event_cursor,
            event: ScheduleEvent::Sealed {
                schedule_plan: plan_id.clone(),
                base_revision: base_revision.clone(),
                expected_schedule_hash: expected_schedule_hash.clone(),
                schedule_revision: revision_id.clone(),
                schedule_hash: hash,
            },
        });
        let report = staged.check(
            plan_id,
            &revision_id,
            memory_plan,
            memory_revision,
            implementation,
            target,
        )?;
        *self = staged;
        Ok(report)
    }

    /// Returns bounded deterministic parametric choices without mutation.
    pub fn continuation(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        target: &TargetManifest,
    ) -> AgentResult<ScheduleContinuation> {
        let (plan_data, revision_data) = plan_revision(self, plan, revision)?;
        let eligible_axes = revision_data
            .program
            .node_order
            .iter()
            .flat_map(|node| revision_data.program.nodes[node].axes.iter().cloned())
            .collect::<Vec<_>>();
        let fusion_pairs = revision_data
            .program
            .dependencies
            .iter()
            .map(|dependency| [dependency.source.clone(), dependency.target.clone()])
            .collect();
        Ok(ScheduleContinuation {
            schedule_plan: plan_data.id.clone(),
            schedule_revision: revision_data.id.clone(),
            expected_schedule_hash: revision_data.schedule_hash.clone(),
            expected_memory_hash: revision_data.memory_hash.clone(),
            expected_target_hash: revision_data.target_hash.clone(),
            eligible_axes,
            factors: vec![2, 4, 8, 16, 32],
            fusion_pairs,
            binding_levels: vec![
                BindingLevel::Serial,
                BindingLevel::GridX,
                BindingLevel::GridY,
                BindingLevel::GridZ,
                BindingLevel::BlockX,
                BindingLevel::BlockY,
                BindingLevel::BlockZ,
                BindingLevel::WorkgroupX,
                BindingLevel::WorkgroupY,
                BindingLevel::WorkgroupZ,
                BindingLevel::SubgroupLane,
            ],
            vector_widths: target.vector.widths.clone(),
            unroll_factors: vec![2, 4, 8, 16],
            serial_fallback: true,
            compiler_remainder_available: true,
            projected_resource_estimate: revision_data.program.resource_estimate.clone(),
        })
    }

    /// Answers a deterministic compiler-owned legality query for one action.
    pub fn legality_query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        action: &ScheduleAction,
        memory_revision: &MemoryRevision,
        implementation: &ImplProgram,
        target: &TargetManifest,
    ) -> AgentResult<ScheduleLegalityQuery> {
        let (_, revision) = plan_revision(self, plan, revision)?;
        let mut staged_revision = revision.clone();
        let mut allocator = self.allocator.clone();
        let result = apply_action(
            &mut staged_revision,
            action,
            &mut allocator,
            &memory_revision.program,
            implementation,
            target,
        )
        .and_then(|_| {
            verify_and_refresh_schedule(
                &mut staged_revision.program,
                &memory_revision.program,
                implementation,
                target,
            )
        });
        Ok(match result {
            Ok(()) => ScheduleLegalityQuery {
                kind: action_kind(action).to_owned(),
                applicable: true,
                failed_side_condition: None,
                repair: None,
            },
            Err(error) => ScheduleLegalityQuery {
                kind: action_kind(action).to_owned(),
                applicable: false,
                failed_side_condition: Some(error.code),
                repair: error.repairs.first().cloned(),
            },
        })
    }

    /// Verifies all plans against externally resolved immutable anchors.
    pub fn verify_all<F>(&self, mut resolve: F) -> AgentResult<()>
    where
        F: FnMut(
            &ScheduleAnchor,
        ) -> AgentResult<(MemoryPlan, MemoryRevision, ImplProgram, TargetManifest)>,
    {
        for plan in self.plans.values() {
            if !plan.revisions.contains_key(&plan.head) {
                return Err(schedule_error(
                    ErrorCode::ScheduleEquivalenceUnproved,
                    "schedule plan head is missing",
                ));
            }
            let (memory_plan, memory_revision, implementation, target) = resolve(&plan.anchor)?;
            for revision in plan.revisions.values() {
                verify_revision(
                    self,
                    plan,
                    revision,
                    &memory_plan,
                    &memory_revision,
                    &implementation,
                    &target,
                )?;
            }
        }
        Ok(())
    }
}

fn action_kind(action: &ScheduleAction) -> &'static str {
    match action {
        ScheduleAction::SplitAxis { .. } => "split_axis",
        ScheduleAction::TileAxes { .. } => "tile_axes",
        ScheduleAction::FuseOperations { .. } => "fuse_operations",
        ScheduleAction::BindAxis { .. } => "bind_axis",
        ScheduleAction::VectorizeAxis { .. } => "vectorize_axis",
        ScheduleAction::UnrollAxis { .. } => "unroll_axis",
        ScheduleAction::ChooseSerial { .. } => "choose_serial",
        ScheduleAction::ChooseCompilerRemainder { .. } => "choose_compiler_remainder",
        ScheduleAction::SetLaunchShape { .. } => "set_launch_shape",
    }
}
