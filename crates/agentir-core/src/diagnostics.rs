//! Stable machine-oriented diagnostics.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};

/// Stable compiler diagnostic codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Requested workspace does not exist.
    WorkspaceNotFound,
    /// Requested revision does not exist.
    RevisionNotFound,
    /// The transaction was based on a stale or disallowed revision.
    BaseRevisionConflict,
    /// A value, hole, dimension, or local binding could not be resolved.
    UnknownReference,
    /// A transaction-local binding was defined twice.
    DuplicateBinding,
    /// The opcode is not supported by this profile.
    UnknownOpcode,
    /// An operation received the wrong number of operands.
    ArityMismatch,
    /// Types are not compatible.
    TypeMismatch,
    /// Tensor shapes are incompatible.
    ShapeMismatch,
    /// A region is malformed or yields an incompatible type.
    InvalidRegion,
    /// A value cannot fill a hole of the requested type.
    HoleTypeMismatch,
    /// An operation requires every hole to be filled.
    OpenHole,
    /// The specification has no valid, complete outputs.
    SpecNotComplete,
    /// The frozen specification cannot be edited.
    SpecFrozen,
    /// The transaction failed atomically.
    TransactionRejected,
    /// Evaluation inputs do not match parameters.
    EvaluationInputMismatch,
    /// Arithmetic is defined to reject division by zero in Stage 1.
    DivisionByZero,
    /// JSON or request data is malformed.
    InvalidRequest,
    /// Workspace archive I/O failed.
    PersistenceIo,
    /// Workspace archive format or version is unsupported.
    PersistenceFormat,
    /// Workspace archive failed an integrity check.
    PersistenceIntegrity,
    /// Replayed events did not reproduce the archived revision graph.
    ReplayMismatch,
    /// A complete specification could not be converted to semantic canonical form.
    CanonicalizationFailed,
    /// A shape constraint is malformed or outside the supported Stage 1.2 subset.
    InvalidConstraint,
    /// A shape constraint proves a conflict with accepted facts or obligations.
    ConstraintContradiction,
    /// A configured or hard resource budget was exceeded.
    ResourceLimitExceeded,
    /// Requested implementation candidate does not exist.
    CandidateNotFound,
    /// Requested immutable candidate revision does not exist.
    CandidateRevisionNotFound,
    /// Candidate creation requires a complete frozen SpecIR revision.
    SpecNotFrozen,
    /// A candidate anchor does not match its frozen specification.
    SpecHashMismatch,
    /// The separate ImplIR graph failed verification.
    ImplVerificationFailed,
    /// A compiler-owned exact rewrite has no match at the requested target.
    RewriteNotApplicable,
    /// A rewrite hash or side-condition precondition failed.
    RewritePreconditionFailed,
    /// The compositional equivalence chain is absent or invalid.
    EquivalenceNotProved,
    /// Correctness or confidence evidence is malformed or inconsistent.
    EvidenceInvalid,
    /// A sealed candidate cannot be edited.
    CandidateSealed,
    /// Stage 2B accepts exact equivalence only.
    UnsupportedRefinement,
    /// Requested speculative proposal does not exist.
    ProposalNotFound,
    /// A speculative replacement fragment is malformed or ill-typed.
    InvalidProposal,
    /// An unknown or unsupported proposal omitted explicit speculative opt-in.
    SpeculativeOptInRequired,
    /// The bounded ordered proof-debt budget would be exceeded.
    ProofDebtLimitExceeded,
    /// No trusted translation-validation path recognizes the proposal.
    TranslationUnsupported,
    /// A deterministic counterexample refuted a speculative obligation.
    ObligationRefuted,
    /// A compiler-owned guard or its dependency is invalid.
    GuardInvalid,
    /// A guarded candidate fallback is absent or not fully proved.
    FallbackInvalid,
    /// Candidate fallback references form a cycle.
    FallbackCycle,
    /// Open, unsupported, or refuted proof debt blocks sealing.
    CandidateHasProofDebt,
    /// Requested exact equality space does not exist.
    EqualitySpaceNotFound,
    /// Requested immutable equality revision does not exist.
    EqualityRevisionNotFound,
    /// Requested equality semantic node does not exist.
    EqualityNodeNotFound,
    /// Equality creation requires a fully proved exact candidate anchor.
    EqualityAnchorUnproved,
    /// A guarded primary cannot become an unconditional equality root.
    EqualityGuardedAnchorUnsupported,
    /// Equality mutation was based on a stale equality head.
    StaleEqualityBase,
    /// Expected equality exact-state hash differs from current state.
    EqualityHashMismatch,
    /// An equality proof edge names an unknown compiler-owned rule.
    EqualityRuleUntrusted,
    /// A stored equality proof edge or explanation failed verification.
    EqualityProofInvalid,
    /// No trusted root-to-node equality path exists.
    EqualityPathNotFound,
    /// Equality-specific resource policy rejected an atomic operation.
    EqualityResourceLimit,
    /// A selected equality path could not be atomically materialized.
    EqualityMaterializationFailed,
    /// Stage 2C optimization events violate dependency order.
    EqualityEventOrderInvalid,
    /// Requested memory plan does not exist.
    MemoryPlanNotFound,
    /// Requested immutable memory revision does not exist.
    MemoryRevisionNotFound,
    /// Requested abstract buffer does not exist.
    BufferNotFound,
    /// Memory creation requires a fully proved unconditional candidate anchor.
    MemoryAnchorUnproved,
    /// Expected memory exact-state hash differs from the selected revision.
    MemoryHashMismatch,
    /// A memory mutation was based on a stale memory head.
    StaleMemoryBase,
    /// A physical layout or stride description is invalid.
    InvalidMemoryLayout,
    /// A typed buffer access is invalid.
    InvalidMemoryAccess,
    /// A buffer does not satisfy its required alignment.
    AlignmentUnsatisfied,
    /// Compiler-owned alias analysis could not prove a required fact.
    AliasProofMissing,
    /// A logical lifetime or last-use condition was violated.
    LifetimeViolation,
    /// Requested in-place storage reuse is not statically safe.
    InPlaceReuseUnsafe,
    /// A compiler-owned memory guard is malformed or unsupported.
    MemoryGuardInvalid,
    /// A guarded memory fallback is absent or not exact.
    MemoryFallbackInvalid,
    /// Guarded memory fallback references form a cycle.
    MemoryFallbackCycle,
    /// Structural MemoryIR verification did not prove equivalence to ImplIR.
    MemoryEquivalenceUnproved,
    /// Memory events violate explicit candidate/equality dependency order.
    MemoryEventOrderInvalid,
    /// A MemoryIR-specific hard resource boundary rejected the operation.
    MemoryResourceLimit,
    /// Requested immutable target manifest does not exist.
    TargetManifestNotFound,
    /// Requested target-manifest revision does not exist.
    TargetRevisionNotFound,
    /// Target manifest hash differs from the selected immutable revision.
    TargetHashMismatch,
    /// The selected compiler-owned target profile lacks a capability.
    TargetCapabilityUnsupported,
    /// A schedule exceeds a capacity declared by its target manifest.
    TargetResourceExceeded,
    /// Requested schedule plan does not exist.
    SchedulePlanNotFound,
    /// Requested immutable schedule revision does not exist.
    ScheduleRevisionNotFound,
    /// Schedule exact-state hash differs from the selected revision.
    ScheduleHashMismatch,
    /// A schedule mutation was based on a stale schedule head.
    StaleScheduleBase,
    /// A logical iteration domain is malformed or unsupported.
    InvalidIterationDomain,
    /// A schedule axis is missing, malformed, or incompatible.
    InvalidScheduleAxis,
    /// Axis splitting cannot preserve exact coverage.
    InvalidSplit,
    /// Tiling cannot preserve exact coverage.
    InvalidTile,
    /// Structural schedule coverage is incomplete.
    IncompleteCoverage,
    /// A logical coordinate would execute more than once.
    DuplicateExecution,
    /// A producer-consumer dependence would be violated.
    DependenceViolation,
    /// Requested operation fusion is not in the trusted exact profile.
    IllegalFusion,
    /// Requested hierarchy binding is inconsistent or unsupported.
    InvalidBinding,
    /// Requested vector width is not supported by the target.
    VectorWidthUnsupported,
    /// Memory layout or alignment cannot support vectorization.
    VectorAlignmentUnsatisfied,
    /// Requested unroll factor is invalid or exceeds the bounded profile.
    InvalidUnroll,
    /// A transform would change the fixed reduction order.
    ReductionOrderViolation,
    /// Scheduled order conflicts with MemoryIR access, alias, or lifetime facts.
    ScheduleMemoryConflict,
    /// A ScheduleIR-specific hard resource boundary rejected the operation.
    ScheduleResourceLimit,
    /// Structural verification did not prove ScheduleEquivalentToMemory.
    ScheduleEquivalenceUnproved,
    /// Schedule events violate explicit dependency cursor order.
    ScheduleEventOrderInvalid,
}

/// Structured compiler error suitable for agent repair loops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentError {
    /// Stable error code.
    pub code: ErrorCode,
    /// Short machine-oriented explanation.
    pub message: String,
    /// Optional structured origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Value>,
    /// Optional expected property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    /// Optional actual property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    /// Legal or likely repair actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
    /// Additional deterministic fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

impl AgentError {
    /// Creates an error with no optional diagnostic fields.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            origin: None,
            expected: None,
            actual: None,
            repairs: Vec::new(),
            details: BTreeMap::new(),
        }
    }

    /// Adds expected and actual values.
    #[must_use]
    pub fn with_types(mut self, expected: impl Into<Value>, actual: impl Into<Value>) -> Self {
        self.expected = Some(expected.into());
        self.actual = Some(actual.into());
        self
    }

    /// Adds one structured detail.
    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Adds one deterministic repair recommendation.
    #[must_use]
    pub fn with_repair(mut self, repair: impl Into<String>) -> Self {
        self.repairs.push(repair.into());
        self
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl Error for AgentError {}

/// Result returned by compiler-core operations.
pub type AgentResult<T> = Result<T, AgentError>;
