//! Persistent CandidateForest, trusted exact rewrites, proof chains, and EvidenceIR.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        CandidateId, CandidateObligationId, CandidateRevisionId, EvidenceId, ImplOperationId,
        ImplValueId, ProposalId, RevisionId,
    },
    impl_ir::{
        IMPL_SEMANTICS_VERSION, ImplHash, ImplOperation, ImplProgram, ImplRegionValue,
        ImplSourceLink, ImplValue, ImplValueOrigin, identity_lower, impl_hash,
        infer_proposed_operation, verify_impl,
    },
    ir::{ConstantValue, Opcode, Program},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    semantic::SpecHash,
    types::{ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
};

/// Immutable Stage 2A candidate-event semantics version.
pub const LEGACY_CANDIDATE_SEMANTICS_VERSION: u32 = 1;

/// Candidate event semantics version, independent of core and archive versions.
pub const CANDIDATE_SEMANTICS_VERSION: u32 = 2;

/// Candidate event semantics used when extending equality-linked history.
pub const EQUALITY_CANDIDATE_SEMANTICS_VERSION: u32 = 3;

/// Immutable Stage 2A exact candidate-state canonical codec version.
pub const LEGACY_CANDIDATE_CANONICAL_VERSION: u32 = 1;

/// Current exact candidate-state canonical codec version.
pub const CANDIDATE_CANONICAL_VERSION: u32 = 2;

/// Candidate canonical codec used only by Stage 2C equality-linked revisions.
pub const EQUALITY_CANDIDATE_CANONICAL_VERSION: u32 = 3;

/// Immutable domain separator for candidate hash v1.
pub const LEGACY_CANDIDATE_HASH_DOMAIN: &[u8] = b"agentir.candidate.exact.v1\0";

/// Domain separator for speculative/guarded candidate hash v2.
pub const CANDIDATE_HASH_DOMAIN: &[u8] = b"agentir.candidate.exact.v2\0";

/// Domain separator for equality-linked candidate hash v3.
pub const EQUALITY_CANDIDATE_HASH_DOMAIN: &[u8] = b"agentir.candidate.exact.v3\0";

/// Current proposal canonical codec version.
pub const PROPOSAL_CANONICAL_VERSION: u32 = 1;

/// Domain separator for alpha-normalized proposal hashes.
pub const PROPOSAL_HASH_DOMAIN: &[u8] = b"agentir.proposal.semantic.v1\0";

/// Stable validator identity for Stage 2B translation validation.
pub const TRANSLATION_VALIDATOR_ID: &str = "agentir.translation_validator";

/// Current trusted translation-validator version.
pub const TRANSLATION_VALIDATOR_VERSION: u32 = 1;

/// Stable ID for unreachable implementation pruning.
pub const PRUNE_UNREACHABLE_RULE: &str = "prune_unreachable_impl_nodes";

/// Stable ID for exact elimination of a type-identical cast.
pub const ELIMINATE_NOOP_CAST_RULE: &str = "eliminate_noop_cast";

/// Stable ID for exact folding of defined scalar constant operations.
pub const FOLD_SCALAR_CONSTANTS_RULE: &str = "fold_defined_scalar_constants";

/// Immutable descriptor for one compiler-owned exact rewrite rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownRewriteRule {
    /// Stable rule ID accepted by candidate transactions.
    pub id: &'static str,
    /// Human-readable supported graph pattern.
    pub pattern: &'static str,
    /// Exact side conditions discharged by the production matcher.
    pub side_conditions: &'static [&'static str],
}

/// Complete Stage 2A known-rewrite registry in stable rule-ID order.
pub const KNOWN_REWRITE_RULES: &[KnownRewriteRule] = &[
    KnownRewriteRule {
        id: ELIMINATE_NOOP_CAST_RULE,
        pattern: "cast(source) with fully identical source, result, and target types",
        side_conditions: &["source_type == target_type"],
    },
    KnownRewriteRule {
        id: FOLD_SCALAR_CONSTANTS_RULE,
        pattern: "defined scalar add/sub/mul/div/fma/compare/cast/select",
        side_conditions: &[
            "all operands are exact scalar constants",
            "reference evaluation is defined",
        ],
    },
    KnownRewriteRule {
        id: PRUNE_UNREACHABLE_RULE,
        pattern: "ImplIR operations unreachable from parameters and outputs",
        side_conditions: &["target and removed nodes are output-unreachable"],
    },
];

/// Looks up a compiler-owned exact rewrite descriptor by stable ID.
#[must_use]
pub fn known_rewrite_rule(id: &str) -> Option<&'static KnownRewriteRule> {
    KNOWN_REWRITE_RULES.iter().find(|rule| rule.id == id)
}

/// Stable structural locator for reapplying one production rewrite.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RewriteTargetLocator {
    /// Zero-based position in deterministic top-level operation order.
    pub operation_order_index: u64,
    /// Expected opcode at that position.
    pub opcode: String,
}

/// One exact compiler-owned production match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductionRewriteMatch {
    /// Stable production rule ID.
    pub rule: String,
    /// Current persistent target, retained only for diagnostics.
    pub target: ImplOperationId,
    /// Structural target locator suitable for deterministic replay.
    pub locator: RewriteTargetLocator,
    /// Exact side conditions discharged by the matcher.
    pub side_conditions: Vec<String>,
    /// Stable applicability explanation.
    pub reason_code: String,
}

/// Resolves a structural production target in one exact ImplIR snapshot.
pub fn resolve_rewrite_locator(
    program: &ImplProgram,
    locator: &RewriteTargetLocator,
) -> AgentResult<ImplOperationId> {
    let index = usize::try_from(locator.operation_order_index).map_err(|_| {
        candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "rewrite target locator index exceeds platform size",
        )
    })?;
    let target = program.operation_order.get(index).ok_or_else(|| {
        candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "rewrite target locator is outside operation order",
        )
    })?;
    let operation = program.operations.get(target).ok_or_else(|| {
        candidate_error(
            ErrorCode::ImplVerificationFailed,
            "rewrite target locator resolves to a missing operation",
        )
    })?;
    if operation.opcode.to_string() != locator.opcode {
        return Err(candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "rewrite target locator opcode is stale",
        )
        .with_types(locator.opcode.clone(), operation.opcode.to_string()));
    }
    Ok(target.clone())
}

/// SHA-256 identity of one exact candidate revision and its history.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateHash(String);

impl CandidateHash {
    /// Creates a hash from a lowercase hexadecimal digest.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the lowercase hexadecimal digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CandidateHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// SHA-256 identity of one normalized speculative replacement proposal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProposalHash(String);

impl ProposalHash {
    /// Creates a proposal hash from a lowercase hexadecimal digest.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the lowercase hexadecimal digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProposalHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Monotonic allocator isolated from the legacy SpecIR allocator contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateAllocator {
    candidate: u64,
    revision: u64,
    operation: u64,
    value: u64,
    evidence: u64,
    obligation: u64,
    proposal: u64,
}

macro_rules! candidate_allocator_method {
    ($method:ident, $field:ident, $prefix:literal, $kind:ident) => {
        #[doc = concat!("Allocates the next `", stringify!($kind), "`.")]
        pub fn $method(&mut self) -> $kind {
            self.$field += 1;
            $kind::new(format!(concat!($prefix, "{}"), self.$field))
        }
    };
}

impl CandidateAllocator {
    candidate_allocator_method!(candidate, candidate, "c", CandidateId);
    candidate_allocator_method!(revision, revision, "cr", CandidateRevisionId);
    candidate_allocator_method!(impl_operation, operation, "iop", ImplOperationId);
    candidate_allocator_method!(impl_value, value, "iv", ImplValueId);
    candidate_allocator_method!(evidence, evidence, "ev", EvidenceId);
    candidate_allocator_method!(obligation, obligation, "co", CandidateObligationId);
    candidate_allocator_method!(proposal, proposal, "p", ProposalId);

    pub(crate) fn from_legacy_counters(
        candidate: u64,
        revision: u64,
        operation: u64,
        value: u64,
        evidence: u64,
        obligation: u64,
    ) -> Self {
        Self {
            candidate,
            revision,
            operation,
            value,
            evidence,
            obligation,
            proposal: 0,
        }
    }
}

/// Candidate lifecycle state retained through Stage 2C.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// Editable branch state, including a newly forked sealed candidate.
    Draft,
    /// Separate implementation graph verified successfully.
    WellTyped,
    /// Trusted certificates prove exact equivalence to frozen SpecIR.
    Equivalent,
    /// Well-typed implementation with ordered proof debt after its proved frontier.
    Speculative,
    /// Exact candidate-level semantics use a compiler-owned guard and proved fallback.
    Guarded,
    /// Immutable accepted implementation revision.
    Sealed,
    /// Deterministic validation found a counterexample or integrity failure.
    Rejected,
}

/// Relation requested between a candidate and its immutable specification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Exact semantic equivalence supported through Stage 2C.
    #[default]
    EquivalentToSpec,
    /// Approximate refinement, reserved for a later stage.
    RefinesSpecWithinTolerance,
}

/// Proof state of the exact candidate relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquivalenceStatus {
    /// The trusted compositional certificate chain is incomplete.
    Open,
    /// The trusted compositional certificate chain verifies.
    Proved,
    /// Exactness is established by a trusted guard plus proved lazy fallback.
    Guarded,
    /// A deterministic counterexample disproved the claimed exact relation.
    Refuted,
    /// The trusted validator has no proof path for the current proposal.
    Unsupported,
}

/// Structured exact relation owned by one candidate revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquivalenceObligation {
    /// Compiler-assigned obligation ID.
    pub id: CandidateObligationId,
    /// Only exact equivalence is accepted through Stage 2C.
    pub relation: RelationKind,
    /// Immutable frozen SpecIR semantic anchor.
    pub spec_hash: SpecHash,
    /// Candidate branch identity.
    pub candidate: CandidateId,
    /// Candidate revision whose graph is covered.
    pub candidate_revision: CandidateRevisionId,
    /// Current history-independent implementation hash.
    pub impl_hash: ImplHash,
    /// Proof state derived from trusted certificates.
    pub status: EquivalenceStatus,
}

/// Strength category kept separate from evidence method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    /// Evidence capable of discharging correctness under Stage 2A trust rules.
    Correctness,
    /// Testing evidence that increases confidence but never proves equivalence.
    Confidence,
}

/// Minimal deterministic EvidenceIR method kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// Compiler-owned structural identity lowering certificate.
    IdentityLowering,
    /// Certificate emitted by a trusted known rewrite.
    KnownRewriteCertificate,
    /// Verified composition of identity and rewrite certificates.
    CompositionalEquivalence,
    /// Fixed-seed differential execution.
    DifferentialTest,
    /// Fixed-seed bounded property oracle.
    PropertyTest,
    /// Trusted validation of an unchanged implementation semantic hash.
    CanonicalIdentityValidation,
    /// Trusted recognition of an agent proposal as a production known rewrite.
    RecognizedKnownRewrite,
    /// Trusted certificate for the bounded self-division guarded fallback.
    GuardedRewriteCertificate,
    /// Composition of consecutively discharged speculative obligations.
    CompositionalSpeculativeDischarge,
    /// Fixed-seed differential testing of a speculative candidate.
    SpeculativeDifferentialTest,
    /// Fixed-seed bounded property testing of a speculative candidate.
    SpeculativePropertyTest,
    /// Deterministic search that may publish a first counterexample.
    CounterexampleSearch,
    /// Trusted compiler verification of an equality-space membership path.
    EqualityMembershipProof,
    /// Provenance certificate for explicit equality-node materialization.
    EqualityMaterialization,
}

/// Compiler-owned proposal classification at the Stage 2B trust boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalClassification {
    /// Malformed, ill-typed, boundary-invalid, or over budget; never persisted.
    Illegal,
    /// Exactly recognized by a trusted existing proof path.
    Legal,
    /// Recognized by the one bounded guarded rule.
    Conditional,
    /// Well-typed but not proved by the compiler.
    Unknown,
    /// Well-typed structure for which no validator exists.
    Unsupported,
}

/// One declared fragment boundary input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalInput {
    /// Transaction-local binding beginning with `$`.
    pub bind: String,
    /// Existing target operand exposed at the fragment boundary.
    pub value: ImplValueId,
}

/// One ordered pure operation inside a proposed replacement fragment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalOperation {
    /// Transaction-local result binding beginning with `$`.
    pub bind: String,
    /// Existing ImplIR opcode spelling.
    pub opcode: String,
    /// Ordered boundary or earlier-local references.
    pub operands: Vec<String>,
    /// Stable semantic attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Exact scalar literal for `constant`; absent for every other opcode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constant: Option<ConstantValue>,
    /// Optional existing closed typed region model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::impl_ir::ImplRegion>,
}

/// The single yielded value of a proposed replacement fragment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalResult {
    /// Boundary or local binding yielded by the fragment.
    pub value: String,
}

/// Alpha-normalizable typed replacement fragment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedImplFragment {
    /// Ordered declaration of the target operand boundary.
    pub inputs: Vec<ProposalInput>,
    /// Ordered pure operations.
    pub operations: Vec<ProposalOperation>,
    /// Exactly one yielded replacement value.
    pub result: ProposalResult,
}

/// Agent-proposed replacement of one top-level single-result ImplIR operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeculativeRewriteProposal {
    /// Persistent target operation in the explicit base candidate revision.
    pub target: ImplOperationId,
    /// Proposed pure replacement fragment.
    pub replacement: ProposedImplFragment,
    /// Required stale-state precondition.
    pub expected_before_impl_hash: ImplHash,
    /// Explicit permission to retain unknown/unsupported proof debt.
    #[serde(default)]
    pub allow_speculative: bool,
    /// Untrusted advisory label; never used as a certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_rule: Option<String>,
}

/// State of one ordered speculative proof-debt item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofDebtStatus {
    /// Awaiting a trusted translation check.
    Open,
    /// Discharged by exact compiler-owned validation.
    Proved,
    /// Discharged by the bounded guard and exact fallback contract.
    Guarded,
    /// Disproved by the first deterministic counterexample.
    Refuted,
    /// No trusted validator path exists; not a correctness failure.
    Unsupported,
}

/// Compiler-owned predicate supported by Stage 2B guarded execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuardPredicate {
    /// Tests one scalar i32 value without evaluating the primary implementation.
    I32NonZero {
        /// Primary/fallback boundary value whose runtime input is tested.
        value: ImplValueId,
    },
}

/// Candidate-level exact lazy fallback contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardedFallback {
    /// Normalized compiler-owned guard.
    pub guard: GuardPredicate,
    /// Candidate containing the immutable proved fallback revision.
    pub fallback_candidate: CandidateId,
    /// Fully proved exact fallback revision.
    pub fallback_revision: CandidateRevisionId,
    /// Exact fallback candidate-state hash.
    pub fallback_candidate_hash: CandidateHash,
    /// Only supported failure strategy, always `evaluate_fallback`.
    pub failure_strategy: String,
    /// Correctness evidence for the guarded contract.
    pub evidence: EvidenceId,
}

/// Last consecutive exact/guarded prefix before any remaining debt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofFrontier {
    /// Candidate branch containing the frontier revision.
    pub candidate: CandidateId,
    /// Candidate revision through which the relation is trusted.
    pub candidate_revision: CandidateRevisionId,
    /// Terminal implementation hash at that trusted prefix.
    pub terminal_proved_impl_hash: ImplHash,
}

/// One persistent ordered speculative correctness obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofDebtItem {
    /// Compiler-assigned obligation identity.
    pub id: CandidateObligationId,
    /// Proposal record that created the debt.
    pub proposal: ProposalId,
    /// Proposal semantic identity.
    pub proposal_hash: ProposalHash,
    /// Candidate revision used as the proposal base.
    pub base_candidate_revision: CandidateRevisionId,
    /// Implementation hash before replacement.
    pub before_impl_hash: ImplHash,
    /// Implementation hash immediately after replacement.
    pub after_impl_hash: ImplHash,
    /// Replaced operation.
    pub target: ImplOperationId,
    /// Ordered boundary values.
    pub boundary: Vec<ImplValueId>,
    /// Only exact equivalence is supported.
    pub relation: RelationKind,
    /// Current proof-debt state.
    pub status: ProofDebtStatus,
    /// Stable compiler-owned discharge method IDs.
    pub allowed_discharge_methods: Vec<String>,
    /// Ordered evidence records associated with validation/refutation.
    pub evidence: Vec<EvidenceId>,
    /// First deterministic counterexample, if refuted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_counterexample: Option<JsonValue>,
    /// Candidate event index that accepted the proposal.
    pub origin_candidate_event: u64,
}

/// Persistent normalized proposal provenance, never correctness evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposalRecord {
    /// Compiler-assigned proposal ID.
    pub id: ProposalId,
    /// Domain-separated alpha-normalized proposal hash.
    pub proposal_hash: ProposalHash,
    /// Candidate and base revision named by the proposal.
    pub candidate: CandidateId,
    /// Candidate base revision.
    pub base_candidate_revision: CandidateRevisionId,
    /// Revision created by accepting the proposal.
    pub accepted_candidate_revision: CandidateRevisionId,
    /// Compiler-owned action classification.
    pub classification: ProposalClassification,
    /// Normalized proposal independent of local binding spellings.
    pub proposal: SpeculativeRewriteProposal,
    /// Implementation hash after applying the normalized fragment.
    pub after_impl_hash: ImplHash,
    /// Ordered allocated operation IDs for the accepted fragment.
    pub allocated_operations: Vec<ImplOperationId>,
    /// Ordered allocated value IDs for the accepted fragment.
    pub allocated_values: Vec<ImplValueId>,
    /// The accepted replacement yield value.
    pub yielded_value: ImplValueId,
}

/// Trusted result persisted by one translation-validation attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TranslationValidationResult {
    /// Before and after semantic implementation hashes are identical.
    CanonicalIdentity,
    /// A production known rewrite exactly reproduced the proposal result.
    RecognizedKnownRewrite {
        /// Compiler-owned stable rule ID.
        rule: String,
        /// Discharged production side conditions.
        side_conditions: Vec<String>,
    },
    /// The one bounded i32 self-division rule with lazy exact fallback.
    GuardedSelfDivision {
        /// Compiler-owned candidate-level fallback contract.
        guarded_fallback: GuardedFallback,
    },
    /// No trusted proof path recognized the well-typed proposal.
    Unsupported,
    /// A deterministic counterexample refuted the obligation.
    Refuted {
        /// First deterministic normalized counterexample.
        counterexample: JsonValue,
    },
}

/// Persisted translation-validation record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranslationValidationRecord {
    /// Proposal checked by the trusted validator.
    pub proposal: ProposalId,
    /// Proof-debt obligation affected by this result.
    pub obligation: CandidateObligationId,
    /// Candidate revision that records the result.
    pub candidate_revision: CandidateRevisionId,
    /// Stable validator ID.
    pub validator_id: String,
    /// Validator implementation version.
    pub validator_version: u32,
    /// Trusted deterministic result.
    pub result: TranslationValidationResult,
    /// Correctness evidence when the result discharged debt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceId>,
}

/// Protocol-friendly translation result paired with the resulting candidate state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranslationCheckReport {
    /// Persisted trusted validator record.
    pub validation: TranslationValidationRecord,
    /// Fully verified candidate report after the attempt.
    pub candidate: CandidateCheckReport,
    /// Stable non-fatal diagnostic for unsupported validation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<ErrorCode>,
}

/// Deterministic evidence result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceResult {
    /// Method completed and its stated condition held.
    Passed,
    /// Method produced a deterministic failure or counterexample.
    Failed,
}

/// Reproducible compiler-owned evidence provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceProvenance {
    /// Compiler package version.
    pub compiler_version: String,
    /// Candidate semantics that produced the record.
    pub candidate_semantics_version: u32,
    /// ImplIR semantics used by verification/evaluation.
    pub impl_semantics_version: u32,
}

/// Minimal deterministic correctness or confidence evidence record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    /// Compiler-assigned evidence ID.
    pub id: EvidenceId,
    /// Correctness or confidence classification.
    pub class: EvidenceClass,
    /// Reproducible evidence method.
    pub kind: EvidenceKind,
    /// Immutable SpecIR anchor.
    pub spec_hash: SpecHash,
    /// Candidate identity current when evidence was recorded.
    pub candidate: CandidateId,
    /// Candidate revision covered by the record.
    pub candidate_revision: CandidateRevisionId,
    /// Input implementation hash, absent for identity lowering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_impl_hash: Option<ImplHash>,
    /// Output/current implementation hash.
    pub output_impl_hash: ImplHash,
    /// Stable method or rule ID.
    pub method: String,
    /// Canonically ordered parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, JsonValue>,
    /// Deterministic outcome.
    pub result: EvidenceResult,
    /// First deterministic counterexample, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<JsonValue>,
    /// Compiler-owned provenance without wall-clock data.
    pub provenance: EvidenceProvenance,
}

/// Trusted edge in the compositional exact-equivalence proof chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquivalenceCertificate {
    /// Stable compiler-owned rule ID.
    pub rule: String,
    /// Prior implementation hash; identity lowering has no prior ImplIR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_impl_hash: Option<ImplHash>,
    /// Result implementation hash.
    pub after_impl_hash: ImplHash,
    /// Deterministically ordered target implementation nodes.
    pub targets: Vec<ImplOperationId>,
    /// Explicit discharged side conditions.
    pub side_conditions: Vec<String>,
    /// ImplIR verifier semantics version.
    pub impl_semantics_version: u32,
    /// Correctness evidence backing this edge.
    pub evidence: EvidenceId,
}

/// One immutable exact candidate revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRevision {
    /// Compiler-assigned candidate revision ID.
    pub id: CandidateRevisionId,
    /// Immutable parent candidate revisions.
    pub parents: Vec<CandidateRevisionId>,
    /// Full separate ImplIR snapshot.
    pub impl_program: ImplProgram,
    /// Reachable, history-independent implementation hash.
    pub impl_hash: ImplHash,
    /// Exact, history-sensitive candidate revision hash.
    pub candidate_hash: CandidateHash,
    /// Per-revision exact candidate-hash contract version (v1 or v2).
    pub candidate_hash_version: u32,
    /// Candidate lifecycle state at this revision.
    pub state: CandidateState,
    /// Exact equivalence obligation.
    pub equivalence: EquivalenceObligation,
    /// Ordered trusted proof chain.
    pub proof_chain: Vec<EquivalenceCertificate>,
    /// Ordered correctness and confidence evidence references.
    pub evidence: Vec<EvidenceId>,
    /// Last trusted exact/guarded prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_frontier: Option<ProofFrontier>,
    /// Ordered proof debt after the frontier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_debt: Vec<ProofDebtItem>,
    /// Ordered persisted translation results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translation_results: Vec<TranslationValidationRecord>,
    /// Candidate-level guarded execution contract, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guarded_fallback: Option<GuardedFallback>,
    /// Trusted equality-backed proof links covered by candidate hash v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equality_proofs: Vec<crate::equality::EqualityMembershipProof>,
    /// Explicit equality materialization provenance covered by candidate hash v3.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equality_materializations: Vec<crate::equality::EqualityMaterializationRecord>,
}

/// Persistent candidate branch with its own immutable revision DAG.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Persistent branch identity.
    pub id: CandidateId,
    /// Frozen SpecIR revision used at creation.
    pub spec_revision: RevisionId,
    /// Immutable semantic anchor.
    pub spec_hash: SpecHash,
    /// Root candidate revision.
    pub root_revision: CandidateRevisionId,
    /// Current candidate head.
    pub head: CandidateRevisionId,
    /// Immutable candidate revisions.
    pub revisions: BTreeMap<CandidateRevisionId, CandidateRevision>,
    /// Parent branch provenance for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_candidate: Option<CandidateId>,
    /// Parent candidate revision for a fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_revision: Option<CandidateRevisionId>,
}

/// One exact compiler-owned candidate rewrite action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateAction {
    /// Applies one registry rule to one deterministic target.
    ApplyKnownRewrite {
        /// Stable rule ID; unknown IDs are structured rewrite rejections.
        rule: String,
        /// Persistent implementation operation target.
        target: ImplOperationId,
        /// Optional stale-state precondition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_before_impl_hash: Option<ImplHash>,
    },
}

/// Atomic candidate transaction against an explicit candidate revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateTransaction {
    /// Candidate branch to edit.
    pub candidate: CandidateId,
    /// Explicit immutable base candidate revision.
    pub base_revision: CandidateRevisionId,
    /// Ordered compiler-known rewrite actions.
    pub actions: Vec<CandidateAction>,
}

/// Hard applicability classification for a known rewrite match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteApplicability {
    /// All exact side conditions are proved.
    Applicable,
    /// The requested target does not match the rule.
    NotApplicable,
    /// A required condition is outside the exact Stage 2A checker.
    Unknown,
}

/// One compiler-generated deterministic rewrite continuation entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateContinuationEntry {
    /// Stable known rule ID.
    pub rule: String,
    /// Deterministic target operation.
    pub target: ImplOperationId,
    /// Exact side conditions required by the rule.
    pub side_conditions: Vec<String>,
    /// Hard applicability result.
    pub applicability: RewriteApplicability,
    /// Stable explanation code.
    pub reason_code: String,
}

/// Bounded deterministic rewrite continuation for one candidate revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateContinuation {
    /// Candidate branch.
    pub candidate: CandidateId,
    /// Candidate revision inspected.
    pub candidate_revision: CandidateRevisionId,
    /// Required stale-state precondition.
    pub expected_before_impl_hash: ImplHash,
    /// Stable rule/target ordered matches.
    pub matches: Vec<CandidateContinuationEntry>,
    /// Trusted known-rewrite space, identical to `matches` for compatibility.
    pub trusted_known_rewrites: Vec<CandidateContinuationEntry>,
    /// Bounded verifier-gated speculative escape schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speculative_escape: Option<SpeculativeEscapeSchema>,
}

/// One bounded proposal shape without enumerating replacement combinations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeculativeEscapeSchema {
    /// Eligible single-result top-level target.
    pub target: ImplOperationId,
    /// Ordered target operands exposed as boundary inputs.
    pub boundary_inputs: Vec<ImplValueId>,
    /// Exact required fragment yield type.
    pub required_yield_type: Type,
    /// Stable allowed opcode subset.
    pub allowed_opcodes: Vec<String>,
    /// Maximum ordered fragment operation count.
    pub fragment_operation_limit: u64,
    /// Required stale-state implementation hash.
    pub expected_before_impl_hash: ImplHash,
    /// Whether unknown/unsupported proposals require explicit opt-in.
    pub requires_speculative_opt_in: bool,
    /// Stable explanation code.
    pub reason_code: String,
}

/// Deterministic outcome supplied by the reference differential validator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DifferentialValidation {
    /// Fixed generator seed.
    pub seed: u64,
    /// Requested bounded cases.
    pub requested_cases: u64,
    /// Cases actually evaluated before success or first failure.
    pub executed_cases: u64,
    /// Whether no counterexample was found.
    pub passed: bool,
    /// First deterministic counterexample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<JsonValue>,
}

/// Candidate check result used by protocol and sealing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateCheckReport {
    /// Candidate identity.
    pub candidate: CandidateId,
    /// Checked candidate revision.
    pub candidate_revision: CandidateRevisionId,
    /// Current lifecycle state.
    pub state: CandidateState,
    /// Whether separate ImplIR verification succeeded.
    pub well_typed: bool,
    /// Exact relation and status.
    pub equivalence: EquivalenceObligation,
    /// Current history-independent implementation hash.
    pub impl_hash: ImplHash,
    /// Current exact candidate hash.
    pub candidate_hash: CandidateHash,
    /// Open blocking candidate obligations.
    pub open_obligations: Vec<CandidateObligationId>,
    /// Ordered evidence counts by strength class.
    pub correctness_evidence: usize,
    /// Ordered confidence-evidence count.
    pub confidence_evidence: usize,
    /// Whether sealing is currently legal.
    pub sealable: bool,
    /// Last trusted exact/guarded prefix for Stage 2B revisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_frontier: Option<ProofFrontier>,
    /// Ordered persistent proof debt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_debt: Vec<ProofDebtItem>,
}

/// Replayable candidate event payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateEvent {
    /// Identity candidate creation.
    Created {
        /// Expected candidate ID.
        candidate: CandidateId,
        /// Frozen SpecIR revision.
        spec_revision: RevisionId,
        /// Requested exact relation.
        relation: RelationKind,
        /// Expected root candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected implementation hash.
        impl_hash: ImplHash,
        /// Expected exact candidate hash.
        candidate_hash: CandidateHash,
    },
    /// Atomic trusted rewrite transaction.
    TransactionApplied {
        /// Replayable transaction.
        transaction: CandidateTransaction,
        /// Expected child candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected implementation hash.
        impl_hash: ImplHash,
        /// Expected exact candidate hash.
        candidate_hash: CandidateHash,
    },
    /// Candidate branch fork.
    Forked {
        /// Parent candidate.
        parent_candidate: CandidateId,
        /// Parent candidate revision.
        parent_revision: CandidateRevisionId,
        /// Expected child candidate.
        candidate: CandidateId,
        /// Expected child root revision.
        candidate_revision: CandidateRevisionId,
        /// Expected exact candidate hash.
        candidate_hash: CandidateHash,
    },
    /// Deterministic confidence validation evidence.
    Validated {
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit base candidate revision.
        base_revision: CandidateRevisionId,
        /// Expected child candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Deterministic validation result.
        validation: DifferentialValidation,
        /// Expected exact candidate hash.
        candidate_hash: CandidateHash,
    },
    /// Immutable seal transition.
    Sealed {
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit base candidate revision.
        base_revision: CandidateRevisionId,
        /// Expected sealed candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected exact candidate hash.
        candidate_hash: CandidateHash,
    },
    /// Accepted bounded speculative replacement proposal.
    ProposalAccepted {
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit candidate head used as the proposal base.
        base_revision: CandidateRevisionId,
        /// Normalized replayable proposal.
        proposal: SpeculativeRewriteProposal,
        /// Expected compiler-assigned proposal ID.
        proposal_id: ProposalId,
        /// Expected child candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected proposal semantic hash.
        proposal_hash: ProposalHash,
        /// Expected implementation hash after replacement.
        impl_hash: ImplHash,
        /// Expected candidate hash v2.
        candidate_hash: CandidateHash,
    },
    /// Persisted trusted translation-validation result.
    TranslationChecked {
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit candidate head used as validation base.
        base_revision: CandidateRevisionId,
        /// Proposal selected for ordered validation.
        proposal: ProposalId,
        /// Expected child candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected deterministic validator result.
        result: TranslationValidationResult,
        /// Expected candidate hash v2.
        candidate_hash: CandidateHash,
    },
}

/// Candidate event paired with independent candidate semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionedCandidateEvent {
    /// Candidate compiler/replay semantics version.
    pub semantics_version: u32,
    /// Replayable candidate event.
    pub event: CandidateEvent,
}

/// Persistent independent candidate forest and EvidenceIR store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CandidateForest {
    /// Candidate branches by compiler-assigned ID.
    pub candidates: BTreeMap<CandidateId, Candidate>,
    /// Evidence records by compiler-assigned ID.
    pub evidence: BTreeMap<EvidenceId, EvidenceRecord>,
    /// Persistent normalized proposal provenance by compiler-assigned ID.
    pub proposals: BTreeMap<ProposalId, ProposalRecord>,
    /// Candidate/ImplIR/evidence allocator state.
    pub allocator: CandidateAllocator,
    /// Ordered candidate event log.
    pub events: Vec<VersionedCandidateEvent>,
}

#[derive(Serialize)]
struct CandidateHashModelV1<'a> {
    codec: &'static str,
    version: u32,
    candidate: &'a CandidateId,
    spec_revision: &'a RevisionId,
    spec_hash: &'a SpecHash,
    parent_candidate: &'a Option<CandidateId>,
    forked_from_revision: &'a Option<CandidateRevisionId>,
    revision: &'a CandidateRevisionId,
    parents: &'a [CandidateRevisionId],
    impl_program: &'a ImplProgram,
    impl_hash: &'a ImplHash,
    state: CandidateState,
    equivalence: &'a EquivalenceObligation,
    proof_chain: &'a [EquivalenceCertificate],
    evidence: &'a [EvidenceId],
}

#[derive(Serialize)]
struct CandidateHashModelV2<'a> {
    codec: &'static str,
    version: u32,
    candidate: &'a CandidateId,
    spec_revision: &'a RevisionId,
    spec_hash: &'a SpecHash,
    parent_candidate: &'a Option<CandidateId>,
    forked_from_revision: &'a Option<CandidateRevisionId>,
    revision: &'a CandidateRevisionId,
    parents: &'a [CandidateRevisionId],
    impl_program: &'a ImplProgram,
    impl_hash: &'a ImplHash,
    state: CandidateState,
    equivalence: &'a EquivalenceObligation,
    proof_chain: &'a [EquivalenceCertificate],
    evidence: &'a [EvidenceId],
    proposal_records: Vec<&'a ProposalRecord>,
    proof_frontier: &'a Option<ProofFrontier>,
    proof_debt: &'a [ProofDebtItem],
    translation_results: &'a [TranslationValidationRecord],
    guarded_fallback: &'a Option<GuardedFallback>,
}

#[derive(Serialize)]
struct CandidateHashModelV3<'a> {
    codec: &'static str,
    version: u32,
    candidate: &'a CandidateId,
    spec_revision: &'a RevisionId,
    spec_hash: &'a SpecHash,
    parent_candidate: &'a Option<CandidateId>,
    forked_from_revision: &'a Option<CandidateRevisionId>,
    revision: &'a CandidateRevisionId,
    parents: &'a [CandidateRevisionId],
    impl_program: &'a ImplProgram,
    impl_hash: &'a ImplHash,
    state: CandidateState,
    equivalence: &'a EquivalenceObligation,
    proof_chain: &'a [EquivalenceCertificate],
    evidence: &'a [EvidenceId],
    proposal_records: Vec<&'a ProposalRecord>,
    proof_frontier: &'a Option<ProofFrontier>,
    proof_debt: &'a [ProofDebtItem],
    translation_results: &'a [TranslationValidationRecord],
    guarded_fallback: &'a Option<GuardedFallback>,
    equality_proofs: &'a [crate::equality::EqualityMembershipProof],
    equality_materializations: &'a [crate::equality::EqualityMaterializationRecord],
}

fn digest_hex(bytes: &[u8]) -> CandidateHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    CandidateHash(output)
}

pub(crate) fn candidate_hash_with_limit(
    forest: &CandidateForest,
    candidate: &Candidate,
    revision: &CandidateRevision,
    max_bytes: u64,
) -> AgentResult<CandidateHash> {
    let (bytes, domain) = match revision.candidate_hash_version {
        LEGACY_CANDIDATE_CANONICAL_VERSION => {
            if revision.proof_frontier.is_some()
                || !revision.proof_debt.is_empty()
                || !revision.translation_results.is_empty()
                || revision.guarded_fallback.is_some()
                || !revision.equality_proofs.is_empty()
                || !revision.equality_materializations.is_empty()
            {
                return Err(candidate_error(
                    ErrorCode::PersistenceIntegrity,
                    "candidate hash v1 revision contains Stage 2B state",
                ));
            }
            let model = CandidateHashModelV1 {
                codec: "agentir.candidate.exact",
                version: LEGACY_CANDIDATE_CANONICAL_VERSION,
                candidate: &candidate.id,
                spec_revision: &candidate.spec_revision,
                spec_hash: &candidate.spec_hash,
                parent_candidate: &candidate.parent_candidate,
                forked_from_revision: &candidate.forked_from_revision,
                revision: &revision.id,
                parents: &revision.parents,
                impl_program: &revision.impl_program,
                impl_hash: &revision.impl_hash,
                state: revision.state,
                equivalence: &revision.equivalence,
                proof_chain: &revision.proof_chain,
                evidence: &revision.evidence,
            };
            (serde_json::to_vec(&model), LEGACY_CANDIDATE_HASH_DOMAIN)
        }
        CANDIDATE_CANONICAL_VERSION => {
            if !revision.equality_proofs.is_empty()
                || !revision.equality_materializations.is_empty()
            {
                return Err(candidate_error(
                    ErrorCode::PersistenceIntegrity,
                    "candidate hash v2 revision contains Stage 2C equality state",
                ));
            }
            let proposal_records = revision
                .proof_debt
                .iter()
                .map(|debt| {
                    forest.proposals.get(&debt.proposal).ok_or_else(|| {
                        candidate_error(
                            ErrorCode::ProposalNotFound,
                            format!(
                                "candidate debt references missing proposal `{}`",
                                debt.proposal
                            ),
                        )
                    })
                })
                .collect::<AgentResult<Vec<_>>>()?;
            let model = CandidateHashModelV2 {
                codec: "agentir.candidate.exact",
                version: CANDIDATE_CANONICAL_VERSION,
                candidate: &candidate.id,
                spec_revision: &candidate.spec_revision,
                spec_hash: &candidate.spec_hash,
                parent_candidate: &candidate.parent_candidate,
                forked_from_revision: &candidate.forked_from_revision,
                revision: &revision.id,
                parents: &revision.parents,
                impl_program: &revision.impl_program,
                impl_hash: &revision.impl_hash,
                state: revision.state,
                equivalence: &revision.equivalence,
                proof_chain: &revision.proof_chain,
                evidence: &revision.evidence,
                proposal_records,
                proof_frontier: &revision.proof_frontier,
                proof_debt: &revision.proof_debt,
                translation_results: &revision.translation_results,
                guarded_fallback: &revision.guarded_fallback,
            };
            (serde_json::to_vec(&model), CANDIDATE_HASH_DOMAIN)
        }
        EQUALITY_CANDIDATE_CANONICAL_VERSION => {
            let proposal_records = revision
                .proof_debt
                .iter()
                .map(|debt| {
                    forest.proposals.get(&debt.proposal).ok_or_else(|| {
                        candidate_error(
                            ErrorCode::ProposalNotFound,
                            format!(
                                "candidate debt references missing proposal `{}`",
                                debt.proposal
                            ),
                        )
                    })
                })
                .collect::<AgentResult<Vec<_>>>()?;
            let model = CandidateHashModelV3 {
                codec: "agentir.candidate.exact",
                version: EQUALITY_CANDIDATE_CANONICAL_VERSION,
                candidate: &candidate.id,
                spec_revision: &candidate.spec_revision,
                spec_hash: &candidate.spec_hash,
                parent_candidate: &candidate.parent_candidate,
                forked_from_revision: &candidate.forked_from_revision,
                revision: &revision.id,
                parents: &revision.parents,
                impl_program: &revision.impl_program,
                impl_hash: &revision.impl_hash,
                state: revision.state,
                equivalence: &revision.equivalence,
                proof_chain: &revision.proof_chain,
                evidence: &revision.evidence,
                proposal_records,
                proof_frontier: &revision.proof_frontier,
                proof_debt: &revision.proof_debt,
                translation_results: &revision.translation_results,
                guarded_fallback: &revision.guarded_fallback,
                equality_proofs: &revision.equality_proofs,
                equality_materializations: &revision.equality_materializations,
            };
            (serde_json::to_vec(&model), EQUALITY_CANDIDATE_HASH_DOMAIN)
        }
        version => {
            return Err(candidate_error(
                ErrorCode::PersistenceFormat,
                format!("unsupported candidate hash version {version}"),
            ));
        }
    };
    let bytes = bytes.map_err(|error| {
        AgentError::new(
            ErrorCode::CanonicalizationFailed,
            format!("candidate exact serialization failed: {error}"),
        )
    })?;
    BudgetCheck::ensure(
        if revision.candidate_hash_version == CANDIDATE_CANONICAL_VERSION {
            ResourceKind::CandidateCanonicalV2Bytes
        } else if revision.candidate_hash_version == EQUALITY_CANDIDATE_CANONICAL_VERSION {
            ResourceKind::CandidateCanonicalV3Bytes
        } else {
            ResourceKind::CandidateCanonicalBytes
        },
        max_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "candidate exact canonicalization",
    )?;
    let mut input = Vec::with_capacity(domain.len() + bytes.len());
    input.extend_from_slice(domain);
    input.extend_from_slice(&bytes);
    Ok(digest_hex(&input))
}

pub(crate) fn candidate_canonical_limit(
    revision: &CandidateRevision,
    limits: &ResourceLimits,
) -> u64 {
    match revision.candidate_hash_version {
        CANDIDATE_CANONICAL_VERSION => limits.candidate_canonical_v2_bytes,
        EQUALITY_CANDIDATE_CANONICAL_VERSION => limits.candidate_canonical_v3_bytes,
        _ => limits.candidate_canonical_bytes,
    }
}

fn provenance() -> EvidenceProvenance {
    EvidenceProvenance {
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        candidate_semantics_version: LEGACY_CANDIDATE_SEMANTICS_VERSION,
        impl_semantics_version: IMPL_SEMANTICS_VERSION,
    }
}

fn current_provenance() -> EvidenceProvenance {
    EvidenceProvenance {
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        candidate_semantics_version: CANDIDATE_SEMANTICS_VERSION,
        impl_semantics_version: IMPL_SEMANTICS_VERSION,
    }
}

fn provenance_for_hash_version(version: u32) -> EvidenceProvenance {
    let candidate_semantics_version = match version {
        LEGACY_CANDIDATE_CANONICAL_VERSION => LEGACY_CANDIDATE_SEMANTICS_VERSION,
        EQUALITY_CANDIDATE_CANONICAL_VERSION => EQUALITY_CANDIDATE_SEMANTICS_VERSION,
        _ => CANDIDATE_SEMANTICS_VERSION,
    };
    EvidenceProvenance {
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        candidate_semantics_version,
        impl_semantics_version: IMPL_SEMANTICS_VERSION,
    }
}

fn semantics_for_hash_version(version: u32) -> u32 {
    match version {
        LEGACY_CANDIDATE_CANONICAL_VERSION => LEGACY_CANDIDATE_SEMANTICS_VERSION,
        EQUALITY_CANDIDATE_CANONICAL_VERSION => EQUALITY_CANDIDATE_SEMANTICS_VERSION,
        _ => CANDIDATE_SEMANTICS_VERSION,
    }
}

fn candidate_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn total_revisions(candidates: &BTreeMap<CandidateId, Candidate>) -> u64 {
    candidates.values().fold(0_u64, |total, candidate| {
        total.saturating_add(u64::try_from(candidate.revisions.len()).unwrap_or(u64::MAX))
    })
}

pub(crate) fn ensure_forest_budgets(
    forest: &CandidateForest,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::ProposalsPerWorkspace,
        u64::try_from(forest.proposals.len()).unwrap_or(u64::MAX),
        "candidate proposal store",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::CandidatesPerWorkspace,
        u64::try_from(forest.candidates.len()).unwrap_or(u64::MAX),
        "candidate forest",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::CandidateBranches,
        u64::try_from(forest.candidates.len()).unwrap_or(u64::MAX),
        "candidate forest",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::CandidateRevisionsPerWorkspace,
        total_revisions(&forest.candidates),
        "candidate forest",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::EvidenceRecords,
        u64::try_from(forest.evidence.len()).unwrap_or(u64::MAX),
        "candidate evidence store",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::CandidateEventsPerArchive,
        u64::try_from(forest.events.len()).unwrap_or(u64::MAX),
        "candidate event log",
    )?;
    let open_equivalence = forest.candidates.values().fold(0_u64, |total, candidate| {
        total.saturating_add(
            u64::try_from(
                candidate
                    .revisions
                    .values()
                    .filter(|revision| revision.equivalence.status == EquivalenceStatus::Open)
                    .count(),
            )
            .unwrap_or(u64::MAX),
        )
    });
    BudgetCheck::against(
        limits,
        ResourceKind::OpenEquivalenceObligations,
        open_equivalence,
        "candidate equivalence obligations",
    )?;
    let mut open_debt = 0_u64;
    let mut guarded_candidates = 0_u64;
    for candidate in forest.candidates.values() {
        let head = candidate.revisions.get(&candidate.head).ok_or_else(|| {
            candidate_error(
                ErrorCode::PersistenceIntegrity,
                "candidate head is missing while checking resource budgets",
            )
        })?;
        let retained_debt = u64::try_from(head.proof_debt.len()).unwrap_or(u64::MAX);
        BudgetCheck::against(
            limits,
            ResourceKind::ProposalsPerCandidate,
            retained_debt,
            "proposals reachable from candidate head",
        )?;
        BudgetCheck::against(
            limits,
            ResourceKind::SpeculativeNodesPerCandidate,
            retained_debt,
            "speculative nodes reachable from candidate head",
        )?;
        let unknown_actions = head
            .proof_debt
            .iter()
            .filter(|debt| {
                forest
                    .proposals
                    .get(&debt.proposal)
                    .is_some_and(|proposal| {
                        matches!(
                            proposal.classification,
                            ProposalClassification::Unknown | ProposalClassification::Unsupported
                        )
                    })
            })
            .count();
        BudgetCheck::against(
            limits,
            ResourceKind::UnknownActionsPerBranch,
            u64::try_from(unknown_actions).unwrap_or(u64::MAX),
            "unknown speculative actions reachable from candidate head",
        )?;
        open_debt = open_debt.saturating_add(
            head.proof_debt
                .iter()
                .filter(|debt| {
                    matches!(
                        debt.status,
                        ProofDebtStatus::Open | ProofDebtStatus::Unsupported
                    )
                })
                .count()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        if head.guarded_fallback.is_some() {
            guarded_candidates = guarded_candidates.saturating_add(1);
        }
    }
    BudgetCheck::against(
        limits,
        ResourceKind::OpenProofDebtObligations,
        open_debt,
        "candidate proof debt",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::GuardedCandidates,
        guarded_candidates,
        "guarded candidate heads",
    )?;
    let v2_events = forest
        .events
        .iter()
        .filter(|event| event.semantics_version == CANDIDATE_SEMANTICS_VERSION)
        .count();
    BudgetCheck::against(
        limits,
        ResourceKind::CandidateSemanticsV2Events,
        u64::try_from(v2_events).unwrap_or(u64::MAX),
        "candidate semantics v2 event log",
    )?;
    let evidence_bytes = serde_json::to_vec(&forest.evidence).map_err(|error| {
        candidate_error(
            ErrorCode::EvidenceInvalid,
            format!("evidence serialization failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::EvidenceBytes,
        u64::try_from(evidence_bytes.len()).unwrap_or(u64::MAX),
        "candidate evidence store",
    )
}

fn candidate_revision<'a>(
    forest: &'a CandidateForest,
    candidate: &CandidateId,
    revision: &CandidateRevisionId,
) -> AgentResult<(&'a Candidate, &'a CandidateRevision)> {
    let candidate_data = forest.candidates.get(candidate).ok_or_else(|| {
        candidate_error(
            ErrorCode::CandidateNotFound,
            format!("candidate `{candidate}` does not exist"),
        )
    })?;
    let revision_data = candidate_data.revisions.get(revision).ok_or_else(|| {
        candidate_error(
            ErrorCode::CandidateRevisionNotFound,
            format!("candidate revision `{revision}` does not exist"),
        )
    })?;
    Ok((candidate_data, revision_data))
}

fn verify_proof_chain(
    forest: &CandidateForest,
    candidate: &Candidate,
    revision: &CandidateRevision,
    source: &Program,
) -> AgentResult<()> {
    if revision.proof_chain.is_empty() {
        return Err(candidate_error(
            ErrorCode::EquivalenceNotProved,
            "candidate equivalence proof chain is empty",
        ));
    }
    let mut identity_allocator = CandidateAllocator::default();
    let identity = identity_lower(source, &mut identity_allocator)?;
    let identity_hash = impl_hash(&identity)?;
    let mut current = None::<ImplHash>;
    for (index, certificate) in revision.proof_chain.iter().enumerate() {
        let evidence = forest.evidence.get(&certificate.evidence).ok_or_else(|| {
            candidate_error(
                ErrorCode::EvidenceInvalid,
                format!(
                    "certificate references missing evidence `{}`",
                    certificate.evidence
                ),
            )
        })?;
        if evidence.class != EvidenceClass::Correctness
            || evidence.result != EvidenceResult::Passed
            || evidence.spec_hash != candidate.spec_hash
            || evidence.output_impl_hash != certificate.after_impl_hash
            || evidence.input_impl_hash != certificate.before_impl_hash
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!(
                    "certificate evidence `{}` is inconsistent",
                    certificate.evidence
                ),
            ));
        }
        if index == 0 {
            if certificate.rule != "identity_lowering"
                || certificate.before_impl_hash.is_some()
                || certificate.after_impl_hash != identity_hash
                || evidence.kind != EvidenceKind::IdentityLowering
            {
                return Err(candidate_error(
                    ErrorCode::EquivalenceNotProved,
                    "proof chain does not begin with the exact identity lowering",
                ));
            }
        } else if certificate.before_impl_hash != current {
            return Err(candidate_error(
                ErrorCode::EquivalenceNotProved,
                "proof chain implementation hashes are discontinuous",
            )
            .with_detail("certificate_index", index as u64));
        }
        if certificate.impl_semantics_version != IMPL_SEMANTICS_VERSION {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "certificate uses an unsupported ImplIR semantics version",
            ));
        }
        if index > 0
            && known_rewrite_rule(&certificate.rule).is_none()
            && certificate.rule != "canonical_identity_validation"
            && certificate.rule != "guarded_i32_self_division"
            && certificate.rule != "equality_membership_v1"
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("certificate uses unknown rule `{}`", certificate.rule),
            ));
        }
        if certificate.rule == "canonical_identity_validation"
            && certificate.before_impl_hash.as_ref() != Some(&certificate.after_impl_hash)
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "canonical identity certificate changed impl_hash",
            ));
        }
        if certificate.rule == "guarded_i32_self_division"
            && evidence.kind != EvidenceKind::GuardedRewriteCertificate
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "guarded proof-chain edge lacks its compiler-owned certificate",
            ));
        }
        current = Some(certificate.after_impl_hash.clone());
    }
    let expected_terminal = revision
        .proof_frontier
        .as_ref()
        .map_or(&revision.impl_hash, |frontier| {
            &frontier.terminal_proved_impl_hash
        });
    if current.as_ref() != Some(expected_terminal)
        || revision.equivalence.impl_hash != revision.impl_hash
        || revision.equivalence.spec_hash != candidate.spec_hash
        || revision.equivalence.candidate != candidate.id
        || revision.equivalence.candidate_revision != revision.id
        || revision.equivalence.relation != RelationKind::EquivalentToSpec
    {
        return Err(candidate_error(
            ErrorCode::EquivalenceNotProved,
            "proof chain does not establish the current exact candidate relation",
        ));
    }
    Ok(())
}

fn verify_proof_debt(
    forest: &CandidateForest,
    candidate: &Candidate,
    revision: &CandidateRevision,
) -> AgentResult<()> {
    if revision.candidate_hash_version == LEGACY_CANDIDATE_CANONICAL_VERSION {
        return Ok(());
    }
    if revision.candidate_hash_version == EQUALITY_CANDIDATE_CANONICAL_VERSION
        && revision.proof_debt.is_empty()
    {
        if revision.equality_materializations.is_empty() || !revision.equality_proofs.is_empty() {
            return Err(candidate_error(
                ErrorCode::PersistenceIntegrity,
                "debt-free candidate hash v3 revision lacks materialization provenance",
            ));
        }
        for record in &revision.equality_materializations {
            if record.materialized_candidate != candidate.id
                || (!candidate
                    .revisions
                    .contains_key(&record.materialized_revision)
                    && record.materialized_revision != revision.id)
                || !forest.evidence.values().any(|evidence| {
                    evidence.candidate == candidate.id
                        && evidence.candidate_revision == record.materialized_revision
                        && evidence.kind == EvidenceKind::EqualityMaterialization
                })
            {
                return Err(candidate_error(
                    ErrorCode::EvidenceInvalid,
                    "equality materialization provenance is inconsistent",
                ));
            }
        }
        return Ok(());
    }
    let frontier = revision.proof_frontier.as_ref().ok_or_else(|| {
        candidate_error(
            ErrorCode::PersistenceIntegrity,
            "candidate hash v2 revision lacks a proof frontier",
        )
    })?;
    let frontier_exists = if frontier.candidate == candidate.id {
        candidate
            .revisions
            .contains_key(&frontier.candidate_revision)
            || frontier.candidate_revision == revision.id
    } else {
        forest
            .candidates
            .get(&frontier.candidate)
            .is_some_and(|candidate| {
                candidate
                    .revisions
                    .contains_key(&frontier.candidate_revision)
            })
    };
    if !frontier_exists {
        return Err(candidate_error(
            ErrorCode::PersistenceIntegrity,
            "proof frontier references a missing candidate revision",
        ));
    }
    if revision.proof_debt.is_empty() {
        return Err(candidate_error(
            ErrorCode::PersistenceIntegrity,
            "candidate hash v2 revision has no proposal proof-debt history",
        ));
    }
    let mut previous_after = None::<ImplHash>;
    let mut encountered_blocker = false;
    let mut guarded = false;
    for debt in &revision.proof_debt {
        let proposal = forest.proposals.get(&debt.proposal).ok_or_else(|| {
            candidate_error(
                ErrorCode::ProposalNotFound,
                format!("proof debt references missing proposal `{}`", debt.proposal),
            )
        })?;
        if proposal.id != debt.proposal
            || proposal.proposal_hash != debt.proposal_hash
            || proposal.base_candidate_revision != debt.base_candidate_revision
            || proposal.after_impl_hash != debt.after_impl_hash
            || proposal.proposal.target != debt.target
            || proposal
                .proposal
                .replacement
                .inputs
                .iter()
                .map(|input| input.value.clone())
                .collect::<Vec<_>>()
                != debt.boundary
            || debt.relation != RelationKind::EquivalentToSpec
        {
            return Err(candidate_error(
                ErrorCode::PersistenceIntegrity,
                "proposal record and ordered proof debt are inconsistent",
            ));
        }
        if previous_after
            .as_ref()
            .is_some_and(|after| after != &debt.before_impl_hash)
        {
            return Err(candidate_error(
                ErrorCode::PersistenceIntegrity,
                "ordered proof-debt implementation hashes are discontinuous",
            ));
        }
        if encountered_blocker
            && matches!(
                debt.status,
                ProofDebtStatus::Proved | ProofDebtStatus::Guarded
            )
        {
            return Err(candidate_error(
                ErrorCode::PersistenceIntegrity,
                "proof frontier skips an earlier open/unsupported/refuted obligation",
            ));
        }
        match debt.status {
            ProofDebtStatus::Proved => {}
            ProofDebtStatus::Guarded => {
                guarded = true;
                encountered_blocker = true;
            }
            ProofDebtStatus::Open | ProofDebtStatus::Unsupported | ProofDebtStatus::Refuted => {
                encountered_blocker = true;
            }
        }
        previous_after = Some(debt.after_impl_hash.clone());
    }
    if guarded != revision.guarded_fallback.is_some()
        || (guarded
            && !matches!(
                revision.state,
                CandidateState::Guarded | CandidateState::Sealed
            ))
    {
        return Err(candidate_error(
            ErrorCode::FallbackInvalid,
            "guarded proof debt and candidate fallback contract disagree",
        ));
    }
    if let Some(fallback) = &revision.guarded_fallback {
        if (fallback.fallback_candidate == candidate.id
            && fallback.fallback_revision == revision.id)
            || fallback.failure_strategy != "evaluate_fallback"
        {
            return Err(candidate_error(
                ErrorCode::FallbackInvalid,
                "guarded fallback has an invalid candidate anchor or strategy",
            ));
        }
        let fallback_candidate = forest
            .candidates
            .get(&fallback.fallback_candidate)
            .ok_or_else(|| {
                candidate_error(
                    ErrorCode::FallbackInvalid,
                    "guarded fallback candidate is missing",
                )
            })?;
        if fallback_candidate.spec_hash != candidate.spec_hash {
            return Err(candidate_error(
                ErrorCode::FallbackInvalid,
                "guarded fallback candidate has a different spec_hash anchor",
            ));
        }
        let fallback_revision = fallback_candidate
            .revisions
            .get(&fallback.fallback_revision)
            .ok_or_else(|| {
                candidate_error(
                    ErrorCode::FallbackInvalid,
                    "guarded fallback revision is missing",
                )
            })?;
        if fallback_revision.candidate_hash != fallback.fallback_candidate_hash
            || fallback_revision.equivalence.status != EquivalenceStatus::Proved
            || fallback_revision.state == CandidateState::Rejected
            || fallback_revision.guarded_fallback.is_some()
        {
            return Err(candidate_error(
                ErrorCode::FallbackInvalid,
                "guarded fallback is not an immutable fully proved exact revision",
            ));
        }
        let GuardPredicate::I32NonZero { value } = &fallback.guard;
        if revision
            .impl_program
            .values
            .get(value)
            .is_none_or(|definition| definition.ty != Type::Scalar(ScalarType::I32))
        {
            return Err(candidate_error(
                ErrorCode::GuardInvalid,
                "guard predicate does not reference a scalar i32 value",
            ));
        }
        let evidence = forest.evidence.get(&fallback.evidence).ok_or_else(|| {
            candidate_error(
                ErrorCode::EvidenceInvalid,
                "guarded fallback certificate evidence is missing",
            )
        })?;
        if evidence.kind != EvidenceKind::GuardedRewriteCertificate
            || evidence.class != EvidenceClass::Correctness
            || evidence.result != EvidenceResult::Passed
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "guarded fallback evidence is not a trusted passed certificate",
            ));
        }
    }
    for record in &revision.translation_results {
        let validator_valid = (record.validator_id == TRANSLATION_VALIDATOR_ID
            && record.validator_version == TRANSLATION_VALIDATOR_VERSION)
            || (record.validator_id == "agentir.equality_validator"
                && record.validator_version == crate::equality::EQUALITY_VALIDATOR_VERSION
                && revision.candidate_hash_version == EQUALITY_CANDIDATE_CANONICAL_VERSION);
        if !validator_valid
            || !revision
                .proof_debt
                .iter()
                .any(|debt| debt.id == record.obligation && debt.proposal == record.proposal)
            || record
                .evidence
                .as_ref()
                .is_some_and(|evidence| !forest.evidence.contains_key(evidence))
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "translation validation record is inconsistent",
            ));
        }
    }
    if revision.candidate_hash_version == EQUALITY_CANDIDATE_CANONICAL_VERSION {
        if revision.equality_proofs.is_empty() && revision.equality_materializations.is_empty() {
            return Err(candidate_error(
                ErrorCode::PersistenceIntegrity,
                "candidate hash v3 revision lacks equality provenance",
            ));
        }
        for proof in &revision.equality_proofs {
            if !revision.proof_debt.iter().any(|debt| {
                debt.id == proof.obligation
                    && debt.proposal == proof.proposal
                    && debt.status == ProofDebtStatus::Proved
            }) || forest.evidence.get(&proof.evidence).is_none_or(|evidence| {
                evidence.kind != EvidenceKind::EqualityMembershipProof
                    || evidence.class != EvidenceClass::Correctness
                    || evidence.result != EvidenceResult::Passed
            }) {
                return Err(candidate_error(
                    ErrorCode::EvidenceInvalid,
                    "equality membership proof and candidate debt are inconsistent",
                ));
            }
        }
    }
    let all_proved = revision
        .proof_debt
        .iter()
        .all(|debt| debt.status == ProofDebtStatus::Proved);
    let has_refuted = revision
        .proof_debt
        .iter()
        .any(|debt| debt.status == ProofDebtStatus::Refuted);
    if (all_proved && revision.equivalence.status != EquivalenceStatus::Proved)
        || (guarded && revision.equivalence.status != EquivalenceStatus::Guarded)
        || (has_refuted
            && (revision.equivalence.status != EquivalenceStatus::Refuted
                || revision.state != CandidateState::Rejected))
    {
        return Err(candidate_error(
            ErrorCode::PersistenceIntegrity,
            "proof-debt statuses disagree with candidate lifecycle state",
        ));
    }
    Ok(())
}

pub(crate) fn verify_candidate_revision(
    forest: &CandidateForest,
    candidate: &Candidate,
    revision: &CandidateRevision,
    source: &Program,
    expected_spec_hash: &SpecHash,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    if &candidate.spec_hash != expected_spec_hash {
        return Err(candidate_error(
            ErrorCode::SpecHashMismatch,
            "candidate spec_hash anchor differs from frozen SpecIR",
        )
        .with_types(
            expected_spec_hash.to_string(),
            candidate.spec_hash.to_string(),
        ));
    }
    verify_impl(&revision.impl_program, source, limits)?;
    verify_proof_debt(forest, candidate, revision)?;
    let mut unique_evidence = BTreeSet::new();
    for evidence_id in &revision.evidence {
        if !unique_evidence.insert(evidence_id) {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("candidate revision repeats evidence `{evidence_id}`"),
            ));
        }
        let evidence = forest.evidence.get(evidence_id).ok_or_else(|| {
            candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("candidate revision references missing evidence `{evidence_id}`"),
            )
        })?;
        if evidence.id != *evidence_id
            || evidence.spec_hash != candidate.spec_hash
            || !matches!(
                evidence.provenance.candidate_semantics_version,
                LEGACY_CANDIDATE_SEMANTICS_VERSION
                    | CANDIDATE_SEMANTICS_VERSION
                    | EQUALITY_CANDIDATE_SEMANTICS_VERSION
            )
            || evidence.provenance.impl_semantics_version != IMPL_SEMANTICS_VERSION
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("evidence `{evidence_id}` has inconsistent identity or provenance"),
            ));
        }
        let evidence_candidate = forest.candidates.get(&evidence.candidate).ok_or_else(|| {
            candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("evidence `{evidence_id}` references a missing candidate"),
            )
        })?;
        if !evidence_candidate
            .revisions
            .contains_key(&evidence.candidate_revision)
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("evidence `{evidence_id}` references a missing revision"),
            ));
        }
        let class_matches_kind = matches!(
            (evidence.class, evidence.kind),
            (
                EvidenceClass::Correctness,
                EvidenceKind::IdentityLowering
                    | EvidenceKind::KnownRewriteCertificate
                    | EvidenceKind::CompositionalEquivalence
                    | EvidenceKind::CanonicalIdentityValidation
                    | EvidenceKind::RecognizedKnownRewrite
                    | EvidenceKind::GuardedRewriteCertificate
                    | EvidenceKind::CompositionalSpeculativeDischarge
                    | EvidenceKind::EqualityMembershipProof
                    | EvidenceKind::EqualityMaterialization
            ) | (
                EvidenceClass::Confidence,
                EvidenceKind::DifferentialTest
                    | EvidenceKind::PropertyTest
                    | EvidenceKind::SpeculativeDifferentialTest
                    | EvidenceKind::SpeculativePropertyTest
                    | EvidenceKind::CounterexampleSearch
            )
        );
        if !class_matches_kind
            || (evidence.result == EvidenceResult::Passed && evidence.counterexample.is_some())
            || (evidence.result == EvidenceResult::Failed && evidence.counterexample.is_none())
        {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("evidence `{evidence_id}` has inconsistent class or result"),
            ));
        }
    }
    let actual_impl_hash = impl_hash(&revision.impl_program)?;
    if actual_impl_hash != revision.impl_hash {
        return Err(candidate_error(
            ErrorCode::PersistenceIntegrity,
            "candidate ImplIR hash is invalid",
        )
        .with_types(revision.impl_hash.to_string(), actual_impl_hash.to_string()));
    }
    verify_proof_chain(forest, candidate, revision, source)?;
    let actual_candidate_hash = candidate_hash_with_limit(
        forest,
        candidate,
        revision,
        candidate_canonical_limit(revision, limits),
    )?;
    if actual_candidate_hash != revision.candidate_hash {
        return Err(candidate_error(
            ErrorCode::PersistenceIntegrity,
            "candidate exact-state hash is invalid",
        )
        .with_types(
            revision.candidate_hash.to_string(),
            actual_candidate_hash.to_string(),
        ));
    }
    Ok(())
}

fn reachable_operations(program: &ImplProgram) -> AgentResult<BTreeSet<ImplOperationId>> {
    fn visit(
        program: &ImplProgram,
        value: &ImplValueId,
        visiting: &mut BTreeSet<ImplOperationId>,
        reached: &mut BTreeSet<ImplOperationId>,
    ) -> AgentResult<()> {
        let definition = program.values.get(value).ok_or_else(|| {
            candidate_error(
                ErrorCode::ImplVerificationFailed,
                format!("reachable value `{value}` is missing"),
            )
        })?;
        let ImplValueOrigin::Operation(operation_id) = &definition.origin;
        if reached.contains(operation_id) {
            return Ok(());
        }
        if !visiting.insert(operation_id.clone()) {
            return Err(candidate_error(
                ErrorCode::ImplVerificationFailed,
                "ImplIR contains a cycle",
            ));
        }
        let operation = program.operations.get(operation_id).ok_or_else(|| {
            candidate_error(
                ErrorCode::ImplVerificationFailed,
                format!("reachable operation `{operation_id}` is missing"),
            )
        })?;
        for operand in &operation.operands {
            visit(program, operand, visiting, reached)?;
        }
        if let Some(region) = &operation.region {
            for capture in &region.captures {
                visit(program, capture, visiting, reached)?;
            }
        }
        visiting.remove(operation_id);
        reached.insert(operation_id.clone());
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

fn noop_cast_side_conditions(
    program: &ImplProgram,
    target: &ImplOperationId,
) -> AgentResult<Vec<String>> {
    let operation = program.operations.get(target).ok_or_else(|| {
        candidate_error(
            ErrorCode::RewriteNotApplicable,
            format!("rewrite target `{target}` does not exist"),
        )
    })?;
    if operation.opcode != Opcode::Cast || operation.operands.len() != 1 {
        return Err(candidate_error(
            ErrorCode::RewriteNotApplicable,
            "target is not a unary cast",
        ));
    }
    let source = program
        .values
        .get(&operation.operands[0])
        .ok_or_else(|| candidate_error(ErrorCode::ImplVerificationFailed, "cast source missing"))?;
    let result = program
        .values
        .get(&operation.results[0])
        .ok_or_else(|| candidate_error(ErrorCode::ImplVerificationFailed, "cast result missing"))?;
    let target_type = operation
        .attributes
        .get("target_type")
        .and_then(JsonValue::as_str)
        .and_then(|value| value.parse::<Type>().ok())
        .ok_or_else(|| {
            candidate_error(
                ErrorCode::RewriteNotApplicable,
                "cast has no valid target_type",
            )
        })?;
    if source.ty != result.ty || target_type != Type::Scalar(source.ty.element_type()) {
        return Err(candidate_error(
            ErrorCode::RewriteNotApplicable,
            "cast source and target types are not fully identical",
        )
        .with_types(source.ty.to_string(), result.ty.to_string()));
    }
    Ok(vec!["source_type == target_type".to_owned()])
}

fn replace_value(program: &mut ImplProgram, from: &ImplValueId, to: &ImplValueId) {
    for operation in program.operations.values_mut() {
        for operand in &mut operation.operands {
            if operand == from {
                *operand = to.clone();
            }
        }
        if let Some(region) = &mut operation.region {
            for capture in &mut region.captures {
                if capture == from {
                    *capture = to.clone();
                }
            }
            for local in &mut region.operations {
                for operand in &mut local.operands {
                    if operand == &ImplRegionValue::Capture(from.clone()) {
                        *operand = ImplRegionValue::Capture(to.clone());
                    }
                }
            }
            if region.yield_value == ImplRegionValue::Capture(from.clone()) {
                region.yield_value = ImplRegionValue::Capture(to.clone());
            }
        }
    }
    for output in program.outputs.values_mut() {
        if &output.value == from {
            output.value = to.clone();
            if let Some(value) = program.values.get(to) {
                output.ty = value.ty.clone();
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FoldValue {
    Bool(bool),
    I32(i32),
    F32(f32),
}

fn fold_operand(constant: &ConstantValue, ty: &Type) -> AgentResult<FoldValue> {
    match (constant, ty) {
        (ConstantValue::Bool { value }, Type::Scalar(ScalarType::Bool)) => {
            Ok(FoldValue::Bool(*value))
        }
        (ConstantValue::I32 { value }, Type::Scalar(ScalarType::I32 | ScalarType::Index)) => {
            Ok(FoldValue::I32(*value))
        }
        (ConstantValue::F32 { .. }, Type::Scalar(ScalarType::F32)) => {
            let value = constant.as_f32().ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "invalid f32 constant bits",
                )
            })?;
            if value.is_finite() {
                Ok(FoldValue::F32(value))
            } else {
                Err(candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "non-finite f32 payload is not foldable in Stage 2A",
                ))
            }
        }
        _ => Err(candidate_error(
            ErrorCode::RewriteNotApplicable,
            "constant operand type does not match its value",
        )),
    }
}

fn bool_compare(predicate: &str, left: bool, right: bool) -> AgentResult<bool> {
    match predicate {
        "eq" => Ok(left == right),
        "ne" => Ok(left != right),
        _ => Err(candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "bool comparison supports only eq/ne",
        )),
    }
}

fn ordering_compare<T: PartialOrd + PartialEq>(
    predicate: &str,
    left: &T,
    right: &T,
) -> AgentResult<bool> {
    match predicate {
        "eq" => Ok(left == right),
        "ne" => Ok(left != right),
        "lt" => Ok(left < right),
        "le" => Ok(left <= right),
        "gt" => Ok(left > right),
        "ge" => Ok(left >= right),
        _ => Err(candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "unknown comparison predicate",
        )),
    }
}

fn fold_operation(program: &ImplProgram, target: &ImplOperationId) -> AgentResult<ConstantValue> {
    let operation = program.operations.get(target).ok_or_else(|| {
        candidate_error(
            ErrorCode::RewriteNotApplicable,
            format!("rewrite target `{target}` does not exist"),
        )
    })?;
    if !matches!(
        operation.opcode,
        Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Fma
            | Opcode::Compare
            | Opcode::Cast
            | Opcode::Select
    ) || operation.results.len() != 1
    {
        return Err(candidate_error(
            ErrorCode::RewriteNotApplicable,
            "target opcode is not a foldable scalar operation",
        ));
    }
    if !matches!(operation.result_types.first(), Some(Type::Scalar(_))) {
        return Err(candidate_error(
            ErrorCode::RewriteNotApplicable,
            "constant folding is restricted to scalar results",
        ));
    }
    let operands = operation
        .operands
        .iter()
        .map(|value| {
            let ty = &program
                .values
                .get(value)
                .ok_or_else(|| {
                    candidate_error(ErrorCode::ImplVerificationFailed, "operand missing")
                })?
                .ty;
            let constant = program.constants.get(value).ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewriteNotApplicable,
                    "not every operand is a scalar constant",
                )
            })?;
            fold_operand(constant, ty)
        })
        .collect::<AgentResult<Vec<_>>>()?;
    let result = match (operation.opcode, operands.as_slice()) {
        (Opcode::Add, [FoldValue::I32(left), FoldValue::I32(right)]) => {
            FoldValue::I32(left.checked_add(*right).ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "i32 addition overflow",
                )
            })?)
        }
        (Opcode::Sub, [FoldValue::I32(left), FoldValue::I32(right)]) => {
            FoldValue::I32(left.checked_sub(*right).ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "i32 subtraction overflow",
                )
            })?)
        }
        (Opcode::Mul, [FoldValue::I32(left), FoldValue::I32(right)]) => {
            FoldValue::I32(left.checked_mul(*right).ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "i32 multiplication overflow",
                )
            })?)
        }
        (
            Opcode::Div,
            [FoldValue::I32(_), FoldValue::I32(0)] | [FoldValue::F32(_), FoldValue::F32(0.0)],
        ) => {
            return Err(candidate_error(
                ErrorCode::RewritePreconditionFailed,
                "division by zero is not a defined fold",
            ));
        }
        (Opcode::Div, [FoldValue::I32(left), FoldValue::I32(right)]) => {
            FoldValue::I32(left.checked_div(*right).ok_or_else(|| {
                candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "i32 division overflow",
                )
            })?)
        }
        (Opcode::Add, [FoldValue::F32(left), FoldValue::F32(right)]) => {
            FoldValue::F32(left + right)
        }
        (Opcode::Sub, [FoldValue::F32(left), FoldValue::F32(right)]) => {
            FoldValue::F32(left - right)
        }
        (Opcode::Mul, [FoldValue::F32(left), FoldValue::F32(right)]) => {
            FoldValue::F32(left * right)
        }
        (Opcode::Div, [FoldValue::F32(left), FoldValue::F32(right)]) => {
            FoldValue::F32(left / right)
        }
        (
            Opcode::Fma,
            [
                FoldValue::I32(left),
                FoldValue::I32(right),
                FoldValue::I32(addend),
            ],
        ) => FoldValue::I32(
            left.checked_mul(*right)
                .and_then(|value| value.checked_add(*addend))
                .ok_or_else(|| {
                    candidate_error(ErrorCode::RewritePreconditionFailed, "i32 fma overflow")
                })?,
        ),
        (
            Opcode::Fma,
            [
                FoldValue::F32(left),
                FoldValue::F32(right),
                FoldValue::F32(addend),
            ],
        ) => FoldValue::F32(left.mul_add(*right, *addend)),
        (Opcode::Compare, [FoldValue::Bool(left), FoldValue::Bool(right)]) => {
            FoldValue::Bool(bool_compare(
                operation
                    .attributes
                    .get("predicate")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("eq"),
                *left,
                *right,
            )?)
        }
        (Opcode::Compare, [FoldValue::I32(left), FoldValue::I32(right)]) => {
            FoldValue::Bool(ordering_compare(
                operation
                    .attributes
                    .get("predicate")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("eq"),
                left,
                right,
            )?)
        }
        (Opcode::Compare, [FoldValue::F32(left), FoldValue::F32(right)]) => {
            FoldValue::Bool(ordering_compare(
                operation
                    .attributes
                    .get("predicate")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("eq"),
                left,
                right,
            )?)
        }
        (Opcode::Select, [FoldValue::Bool(condition), yes, no]) => {
            if *condition {
                *yes
            } else {
                *no
            }
        }
        (Opcode::Cast, [FoldValue::I32(value)]) => {
            match operation
                .attributes
                .get("target_type")
                .and_then(JsonValue::as_str)
            {
                Some("i32" | "index") => FoldValue::I32(*value),
                Some("f32") => FoldValue::F32(*value as f32),
                _ => {
                    return Err(candidate_error(
                        ErrorCode::RewritePreconditionFailed,
                        "unsupported constant cast",
                    ));
                }
            }
        }
        (Opcode::Cast, [FoldValue::F32(value)]) => {
            match operation
                .attributes
                .get("target_type")
                .and_then(JsonValue::as_str)
            {
                Some("f32") => FoldValue::F32(*value),
                Some("i32" | "index") if *value >= i32::MIN as f32 && *value <= i32::MAX as f32 => {
                    FoldValue::I32(*value as i32)
                }
                _ => {
                    return Err(candidate_error(
                        ErrorCode::RewritePreconditionFailed,
                        "unsupported or out-of-range constant cast",
                    ));
                }
            }
        }
        (Opcode::Cast, [FoldValue::Bool(value)])
            if operation
                .attributes
                .get("target_type")
                .and_then(JsonValue::as_str)
                == Some("bool") =>
        {
            FoldValue::Bool(*value)
        }
        _ => {
            return Err(candidate_error(
                ErrorCode::RewriteNotApplicable,
                "constant operand kinds do not match the operation",
            ));
        }
    };
    match result {
        FoldValue::Bool(value) => Ok(ConstantValue::Bool { value }),
        FoldValue::I32(value) => Ok(ConstantValue::I32 { value }),
        FoldValue::F32(value) if value.is_finite() => Ok(ConstantValue::F32 {
            bits: format!("0x{:08x}", value.to_bits()),
        }),
        FoldValue::F32(_) => Err(candidate_error(
            ErrorCode::RewritePreconditionFailed,
            "non-finite f32 result is not a defined Stage 2A fold",
        )),
    }
}

pub(crate) fn apply_known_rewrite(
    program: &mut ImplProgram,
    rule: &str,
    target: &ImplOperationId,
) -> AgentResult<Vec<String>> {
    let descriptor = known_rewrite_rule(rule).ok_or_else(|| {
        candidate_error(
            ErrorCode::RewriteNotApplicable,
            format!("unknown known-rewrite rule `{rule}`"),
        )
        .with_detail("rule", rule.to_owned())
    })?;
    match descriptor.id {
        PRUNE_UNREACHABLE_RULE => {
            let reachable = reachable_operations(program)?;
            if reachable.contains(target) || !program.operations.contains_key(target) {
                return Err(candidate_error(
                    ErrorCode::RewriteNotApplicable,
                    "target is not an unreachable implementation operation",
                ));
            }
            let removed = program
                .operations
                .keys()
                .filter(|operation| !reachable.contains(*operation))
                .cloned()
                .collect::<BTreeSet<_>>();
            let removed_values = program
                .values
                .iter()
                .filter_map(|(value, definition)| {
                    let ImplValueOrigin::Operation(operation) = &definition.origin;
                    removed.contains(operation).then_some(value.clone())
                })
                .collect::<BTreeSet<_>>();
            program
                .operations
                .retain(|operation, _| !removed.contains(operation));
            program
                .operation_order
                .retain(|operation| !removed.contains(operation));
            program
                .values
                .retain(|value, _| !removed_values.contains(value));
            program
                .constants
                .retain(|value, _| !removed_values.contains(value));
            Ok(vec![
                "target and removed nodes are output-unreachable".to_owned(),
            ])
        }
        ELIMINATE_NOOP_CAST_RULE => {
            let side_conditions = noop_cast_side_conditions(program, target)?;
            let operation = program
                .operations
                .get(target)
                .expect("matched cast exists")
                .clone();
            let source = operation.operands[0].clone();
            let result = operation.results[0].clone();
            replace_value(program, &result, &source);
            program.operations.remove(target);
            program
                .operation_order
                .retain(|operation| operation != target);
            program.values.remove(&result);
            program.constants.remove(&result);
            Ok(side_conditions)
        }
        FOLD_SCALAR_CONSTANTS_RULE => {
            let constant = fold_operation(program, target)?;
            let operation = program
                .operations
                .get_mut(target)
                .expect("matched fold target exists");
            let result = operation.results[0].clone();
            operation.opcode = Opcode::Constant;
            operation.operands.clear();
            operation.attributes = BTreeMap::from([("value".to_owned(), json!(constant))]);
            operation.region = None;
            operation.source_link = ImplSourceLink {
                spec_operation: operation.source_link.spec_operation.clone(),
                spec_value: operation.source_link.spec_value.clone(),
                rewrite_rule: Some(FOLD_SCALAR_CONSTANTS_RULE.to_owned()),
            };
            program.constants.insert(result, constant);
            Ok(vec![
                "all operands are exact scalar constants".to_owned(),
                "reference evaluation is defined".to_owned(),
            ])
        }
        _ => Err(candidate_error(
            ErrorCode::EvidenceInvalid,
            format!("known-rewrite registry entry `{rule}` has no implementation"),
        )),
    }
}

/// Enumerates every applicable production rewrite in stable rule/target order.
pub(crate) fn production_rewrite_matches(
    program: &ImplProgram,
    limits: &ResourceLimits,
) -> AgentResult<Vec<ProductionRewriteMatch>> {
    let reachable = reachable_operations(program)?;
    let mut matches = Vec::new();
    for rule in KNOWN_REWRITE_RULES {
        for (index, target) in program.operation_order.iter().enumerate() {
            let operation = program.operations.get(target).ok_or_else(|| {
                candidate_error(
                    ErrorCode::ImplVerificationFailed,
                    "operation order references a missing rewrite target",
                )
            })?;
            let (side_conditions, reason_code) = match rule.id {
                PRUNE_UNREACHABLE_RULE if !reachable.contains(target) => (
                    vec!["target and removed nodes are output-unreachable".to_owned()],
                    "UNREACHABLE_IMPL_NODE",
                ),
                ELIMINATE_NOOP_CAST_RULE => match noop_cast_side_conditions(program, target) {
                    Ok(side) => (side, "IDENTICAL_CAST_TYPES"),
                    Err(_) => continue,
                },
                FOLD_SCALAR_CONSTANTS_RULE => {
                    if fold_operation(program, target).is_err() {
                        continue;
                    }
                    (
                        vec![
                            "all operands are exact scalar constants".to_owned(),
                            "reference evaluation is defined".to_owned(),
                        ],
                        "DEFINED_SCALAR_CONSTANT_FOLD",
                    )
                }
                _ => continue,
            };
            BudgetCheck::against(
                limits,
                ResourceKind::EqualityMatchesPerNode,
                u64::try_from(matches.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
                "production rewrite enumeration for one equality node",
            )?;
            matches.push(ProductionRewriteMatch {
                rule: rule.id.to_owned(),
                target: target.clone(),
                locator: RewriteTargetLocator {
                    operation_order_index: u64::try_from(index).unwrap_or(u64::MAX),
                    opcode: operation.opcode.to_string(),
                },
                side_conditions,
                reason_code: reason_code.to_owned(),
            });
        }
    }
    Ok(matches)
}

fn invalid_proposal(message: impl Into<String>) -> AgentError {
    candidate_error(ErrorCode::InvalidProposal, message)
        .with_repair("use the bounded candidate.continuation speculative escape schema")
}

fn normalize_region(
    region: &crate::impl_ir::ImplRegion,
) -> AgentResult<crate::impl_ir::ImplRegion> {
    let mut arguments = BTreeMap::new();
    let normalized_arguments = region
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let name = format!("%arg{index}");
            if arguments
                .insert(argument.name.clone(), name.clone())
                .is_some()
            {
                return Err(invalid_proposal("proposal region repeats a block argument"));
            }
            Ok(crate::impl_ir::ImplBlockArgument {
                name,
                ty: argument.ty.clone(),
            })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    let mut locals = BTreeMap::new();
    let mut operations = Vec::with_capacity(region.operations.len());
    for (index, operation) in region.operations.iter().enumerate() {
        let result = format!("%local{index}");
        if locals
            .insert(operation.result.clone(), result.clone())
            .is_some()
        {
            return Err(invalid_proposal("proposal region repeats a local result"));
        }
        let normalize_value = |value: &ImplRegionValue| -> AgentResult<ImplRegionValue> {
            match value {
                ImplRegionValue::Argument(name) => arguments
                    .get(name)
                    .cloned()
                    .map(ImplRegionValue::Argument)
                    .ok_or_else(|| invalid_proposal("proposal region uses an unknown argument")),
                ImplRegionValue::Local(name) => locals
                    .get(name)
                    .cloned()
                    .map(ImplRegionValue::Local)
                    .ok_or_else(|| {
                        invalid_proposal("proposal region uses an unknown or forward local")
                    }),
                ImplRegionValue::Capture(value) => Ok(ImplRegionValue::Capture(value.clone())),
            }
        };
        operations.push(crate::impl_ir::ImplRegionOperation {
            result,
            opcode: operation.opcode,
            operands: operation
                .operands
                .iter()
                .map(normalize_value)
                .collect::<AgentResult<Vec<_>>>()?,
            attributes: operation.attributes.clone(),
            result_type: operation.result_type.clone(),
        });
    }
    let yield_value = match &region.yield_value {
        ImplRegionValue::Argument(name) => arguments
            .get(name)
            .cloned()
            .map(ImplRegionValue::Argument)
            .ok_or_else(|| invalid_proposal("proposal region yields an unknown argument"))?,
        ImplRegionValue::Local(name) => locals
            .get(name)
            .cloned()
            .map(ImplRegionValue::Local)
            .ok_or_else(|| invalid_proposal("proposal region yields an unknown local"))?,
        ImplRegionValue::Capture(value) => ImplRegionValue::Capture(value.clone()),
    };
    Ok(crate::impl_ir::ImplRegion {
        arguments: normalized_arguments,
        captures: region.captures.clone(),
        operations,
        yield_value,
        yield_type: region.yield_type.clone(),
    })
}

/// Alpha-normalizes transaction-local proposal bindings before ID allocation.
pub fn normalize_speculative_proposal(
    proposal: &SpeculativeRewriteProposal,
) -> AgentResult<SpeculativeRewriteProposal> {
    let mut bindings = BTreeMap::new();
    let mut inputs = Vec::with_capacity(proposal.replacement.inputs.len());
    for (index, input) in proposal.replacement.inputs.iter().enumerate() {
        if !input.bind.starts_with('$') || input.bind.len() < 2 {
            return Err(invalid_proposal(
                "proposal boundary bindings must begin with `$`",
            ));
        }
        let normalized = format!("$b{index}");
        if bindings
            .insert(input.bind.clone(), normalized.clone())
            .is_some()
        {
            return Err(invalid_proposal("proposal repeats a boundary binding"));
        }
        inputs.push(ProposalInput {
            bind: normalized,
            value: input.value.clone(),
        });
    }
    let mut operations = Vec::with_capacity(proposal.replacement.operations.len());
    for (index, operation) in proposal.replacement.operations.iter().enumerate() {
        if !operation.bind.starts_with('$') || operation.bind.len() < 2 {
            return Err(invalid_proposal(
                "proposal operation bindings must begin with `$`",
            ));
        }
        let operands = operation
            .operands
            .iter()
            .map(|operand| {
                bindings.get(operand).cloned().ok_or_else(|| {
                    invalid_proposal(format!(
                        "proposal operand `{operand}` is not a boundary or earlier local binding"
                    ))
                })
            })
            .collect::<AgentResult<Vec<_>>>()?;
        let normalized = format!("$n{index}");
        if bindings
            .insert(operation.bind.clone(), normalized.clone())
            .is_some()
        {
            return Err(invalid_proposal("proposal repeats a local binding"));
        }
        operations.push(ProposalOperation {
            bind: normalized,
            opcode: operation.opcode.clone(),
            operands,
            attributes: operation.attributes.clone(),
            constant: operation.constant.clone(),
            region: operation
                .region
                .as_ref()
                .map(normalize_region)
                .transpose()?,
        });
    }
    let result = bindings
        .get(&proposal.replacement.result.value)
        .cloned()
        .ok_or_else(|| invalid_proposal("proposal yield references an unknown binding"))?;
    Ok(SpeculativeRewriteProposal {
        target: proposal.target.clone(),
        replacement: ProposedImplFragment {
            inputs,
            operations,
            result: ProposalResult { value: result },
        },
        expected_before_impl_hash: proposal.expected_before_impl_hash.clone(),
        allow_speculative: proposal.allow_speculative,
        claimed_rule: proposal.claimed_rule.clone(),
    })
}

#[derive(Serialize)]
struct ProposalHashModel<'a> {
    codec: &'static str,
    version: u32,
    base_impl_hash: &'a ImplHash,
    target: &'a ImplOperationId,
    boundary: Vec<&'a ImplValueId>,
    replacement: &'a ProposedImplFragment,
    expected_output_type: &'a Type,
    numeric_contract: &'a crate::types::NumericContract,
}

/// Domain-separated canonical proposal bytes and their semantic hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalCanonical {
    /// Deterministic proposal-hash model encoding.
    pub bytes: Vec<u8>,
    /// SHA-256 digest under the proposal semantic v1 domain.
    pub proposal_hash: ProposalHash,
}

fn proposal_canonical_with_target(
    program: &ImplProgram,
    target: &ImplOperation,
    proposal: &SpeculativeRewriteProposal,
    limits: &ResourceLimits,
) -> AgentResult<ProposalCanonical> {
    let output_type = target
        .result_types
        .first()
        .ok_or_else(|| invalid_proposal("proposal target has no result type"))?;
    let model = ProposalHashModel {
        codec: "agentir.proposal.semantic",
        version: PROPOSAL_CANONICAL_VERSION,
        base_impl_hash: &proposal.expected_before_impl_hash,
        target: &proposal.target,
        boundary: proposal
            .replacement
            .inputs
            .iter()
            .map(|input| &input.value)
            .collect(),
        replacement: &proposal.replacement,
        expected_output_type: output_type,
        numeric_contract: &program.numeric_contract,
    };
    let bytes = serde_json::to_vec(&model).map_err(|error| {
        invalid_proposal(format!("proposal canonical serialization failed: {error}"))
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::ProposalEncodedBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "normalized proposal before persistent ID allocation",
    )?;
    let mut input = Vec::with_capacity(PROPOSAL_HASH_DOMAIN.len() + bytes.len());
    input.extend_from_slice(PROPOSAL_HASH_DOMAIN);
    input.extend_from_slice(&bytes);
    let CandidateHash(hash) = digest_hex(&input);
    Ok(ProposalCanonical {
        bytes,
        proposal_hash: ProposalHash(hash),
    })
}

/// Normalizes and canonicalizes a proposal against its explicit base implementation.
///
/// This is the exact production codec used before speculative acceptance. It does not
/// allocate persistent IDs or claim that the replacement is well typed or equivalent.
pub fn canonicalize_proposal_with_limit(
    program: &ImplProgram,
    proposal: &SpeculativeRewriteProposal,
    limits: &ResourceLimits,
) -> AgentResult<ProposalCanonical> {
    let normalized = normalize_speculative_proposal(proposal)?;
    let target = program.operations.get(&normalized.target).ok_or_else(|| {
        invalid_proposal(format!(
            "proposal target `{}` does not exist",
            normalized.target
        ))
    })?;
    proposal_canonical_with_target(program, target, &normalized, limits)
}

struct AppliedProposal {
    program: ImplProgram,
    allocated_operations: Vec<ImplOperationId>,
    allocated_values: Vec<ImplValueId>,
    yielded_value: ImplValueId,
}

fn apply_proposal_fragment(
    program: &ImplProgram,
    target: &ImplOperation,
    proposal: &SpeculativeRewriteProposal,
    allocator: &mut CandidateAllocator,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<AppliedProposal> {
    if target.results.len() != 1 || target.result_types.len() != 1 {
        return Err(invalid_proposal(
            "Stage 2B replaces only single-result top-level operations",
        ));
    }
    let boundary = proposal
        .replacement
        .inputs
        .iter()
        .map(|input| input.value.clone())
        .collect::<Vec<_>>();
    if boundary != target.operands {
        return Err(invalid_proposal(
            "proposal boundary must equal the target operand list in order",
        )
        .with_detail("target", target.id.to_string())
        .with_detail("expected_boundary", json!(target.operands))
        .with_detail("actual_boundary", json!(boundary)));
    }
    BudgetCheck::against(
        limits,
        ResourceKind::ProposalFragmentOperations,
        u64::try_from(proposal.replacement.operations.len()).unwrap_or(u64::MAX),
        "proposal fragment before persistent ID allocation",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ProposalFragmentValues,
        u64::try_from(proposal.replacement.operations.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(boundary.len()).unwrap_or(u64::MAX)),
        "proposal fragment before persistent ID allocation",
    )?;
    let boundary_set = boundary.iter().collect::<BTreeSet<_>>();
    for operation in &proposal.replacement.operations {
        if let Some(region) = &operation.region {
            if region
                .captures
                .iter()
                .any(|capture| !boundary_set.contains(capture))
            {
                return Err(invalid_proposal(
                    "proposal region captures a value outside the declared target boundary",
                ));
            }
        }
    }
    let mut next = program.clone();
    let mut values = proposal
        .replacement
        .inputs
        .iter()
        .map(|input| (input.bind.clone(), input.value.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut allocated_operations = Vec::new();
    let mut allocated_values = Vec::new();
    for proposed in &proposal.replacement.operations {
        let opcode = proposed
            .opcode
            .parse::<Opcode>()
            .map_err(invalid_proposal)?;
        if opcode == Opcode::Parameter {
            return Err(invalid_proposal(
                "proposal fragments cannot create parameters",
            ));
        }
        let operands = proposed
            .operands
            .iter()
            .map(|operand| {
                values.get(operand).cloned().ok_or_else(|| {
                    invalid_proposal(format!("proposal binding `{operand}` is unavailable"))
                })
            })
            .collect::<AgentResult<Vec<_>>>()?;
        let result_type = infer_proposed_operation(
            &next,
            opcode,
            &operands,
            &proposed.attributes,
            proposed.constant.as_ref(),
            proposed.region.as_ref(),
        )
        .map_err(|error| {
            invalid_proposal(format!(
                "proposal operation failed inference: {}",
                error.message
            ))
            .with_detail("opcode", proposed.opcode.clone())
        })?;
        let operation_id = allocator.impl_operation();
        let value_id = allocator.impl_value();
        next.values.insert(
            value_id.clone(),
            ImplValue {
                id: value_id.clone(),
                ty: result_type.clone(),
                origin: ImplValueOrigin::Operation(operation_id.clone()),
                name: None,
                source_link: ImplSourceLink {
                    spec_operation: target.source_link.spec_operation.clone(),
                    spec_value: target.source_link.spec_value.clone(),
                    rewrite_rule: Some("speculative_proposal".to_owned()),
                },
            },
        );
        if let Some(constant) = &proposed.constant {
            next.constants.insert(value_id.clone(), constant.clone());
        }
        let attributes = if let Some(constant) = &proposed.constant {
            if !proposed.attributes.is_empty() {
                return Err(invalid_proposal(
                    "proposal constant uses its exact literal instead of arbitrary attributes",
                ));
            }
            BTreeMap::from([("value".to_owned(), json!(constant))])
        } else {
            proposed.attributes.clone()
        };
        next.operations.insert(
            operation_id.clone(),
            ImplOperation {
                id: operation_id.clone(),
                opcode,
                operands,
                results: vec![value_id.clone()],
                attributes,
                region: proposed.region.clone(),
                result_types: vec![result_type],
                source_link: ImplSourceLink {
                    spec_operation: target.source_link.spec_operation.clone(),
                    spec_value: target.source_link.spec_value.clone(),
                    rewrite_rule: Some("speculative_proposal".to_owned()),
                },
            },
        );
        values.insert(proposed.bind.clone(), value_id.clone());
        allocated_operations.push(operation_id);
        allocated_values.push(value_id);
    }
    let yielded_value = values
        .get(&proposal.replacement.result.value)
        .cloned()
        .ok_or_else(|| invalid_proposal("normalized proposal yield is unavailable"))?;
    let yielded_type = next
        .values
        .get(&yielded_value)
        .map(|value| value.ty.clone())
        .ok_or_else(|| invalid_proposal("proposal yield value is absent"))?;
    if yielded_type != target.result_types[0] {
        return Err(
            invalid_proposal("proposal yield type differs from target result type")
                .with_types(target.result_types[0].to_string(), yielded_type.to_string()),
        );
    }
    let target_position = next
        .operation_order
        .iter()
        .position(|operation| operation == &target.id)
        .ok_or_else(|| invalid_proposal("proposal target is absent from operation order"))?;
    let insertion_position = target_position + 1;
    next.operation_order.splice(
        insertion_position..insertion_position,
        allocated_operations.iter().cloned(),
    );
    replace_value(&mut next, &target.results[0], &yielded_value);
    verify_impl(&next, source, limits).map_err(|error| {
        invalid_proposal(format!(
            "proposed ImplIR failed verification: {}",
            error.message
        ))
    })?;
    Ok(AppliedProposal {
        program: next,
        allocated_operations,
        allocated_values,
        yielded_value,
    })
}

fn guarded_self_division_matches(
    before: &ImplProgram,
    target: &ImplOperation,
    applied: &AppliedProposal,
) -> bool {
    if target.opcode != Opcode::Div
        || target.operands.len() != 2
        || target.operands[0] != target.operands[1]
        || target.result_types != vec![Type::Scalar(ScalarType::I32)]
    {
        return false;
    }
    let Some(value) = applied.program.constants.get(&applied.yielded_value) else {
        return false;
    };
    matches!(value, ConstantValue::I32 { value: 1 })
        && before
            .values
            .get(&target.operands[0])
            .is_some_and(|value| value.ty == Type::Scalar(ScalarType::I32))
}

fn classify_proposal(
    before: &ImplProgram,
    target: &ImplOperation,
    applied: &AppliedProposal,
) -> AgentResult<ProposalClassification> {
    let before_hash = impl_hash(before)?;
    let after_hash = impl_hash(&applied.program)?;
    if before_hash == after_hash {
        return Ok(ProposalClassification::Legal);
    }
    for rule in KNOWN_REWRITE_RULES {
        let mut recognized = before.clone();
        if apply_known_rewrite(&mut recognized, rule.id, &target.id).is_ok()
            && impl_hash(&recognized)? == after_hash
        {
            return Ok(ProposalClassification::Legal);
        }
    }
    if guarded_self_division_matches(before, target, applied) {
        return Ok(ProposalClassification::Conditional);
    }
    if applied.program.operations.values().any(|operation| {
        operation.source_link.rewrite_rule.as_deref() == Some("speculative_proposal")
            && operation.region.is_some()
    }) {
        Ok(ProposalClassification::Unsupported)
    } else {
        Ok(ProposalClassification::Unknown)
    }
}

enum TrustedPath {
    Identity,
    Known { rule: String, side: Vec<String> },
    Guarded { guard_value: ImplValueId },
    Unsupported,
}

impl CandidateForest {
    /// Returns a candidate branch by ID.
    pub fn candidate(&self, id: &CandidateId) -> AgentResult<&Candidate> {
        self.candidates.get(id).ok_or_else(|| {
            candidate_error(
                ErrorCode::CandidateNotFound,
                format!("candidate `{id}` does not exist"),
            )
        })
    }

    /// Returns an immutable candidate revision.
    pub fn revision(
        &self,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
    ) -> AgentResult<&CandidateRevision> {
        candidate_revision(self, candidate, revision).map(|(_, revision)| revision)
    }

    /// Returns one persistent normalized speculative proposal record.
    pub fn proposal(&self, proposal: &ProposalId) -> AgentResult<&ProposalRecord> {
        self.proposals.get(proposal).ok_or_else(|| {
            candidate_error(
                ErrorCode::ProposalNotFound,
                format!("proposal `{proposal}` does not exist"),
            )
        })
    }

    /// Creates an identity candidate for one complete frozen SpecIR revision.
    pub fn create(
        &mut self,
        spec_revision: RevisionId,
        spec_hash: SpecHash,
        source: &Program,
        relation: RelationKind,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        if relation != RelationKind::EquivalentToSpec {
            return Err(candidate_error(
                ErrorCode::UnsupportedRefinement,
                "Stage 2B supports only EquivalentToSpec",
            ));
        }
        BudgetCheck::against(
            limits,
            ResourceKind::CandidatesPerWorkspace,
            u64::try_from(self.candidates.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "candidate creation before ID allocation",
        )?;
        let mut staged = self.clone();
        let candidate_id = staged.allocator.candidate();
        let revision_id = staged.allocator.revision();
        let obligation_id = staged.allocator.obligation();
        let evidence_id = staged.allocator.evidence();
        let impl_program = identity_lower(source, &mut staged.allocator)?;
        verify_impl(&impl_program, source, limits)?;
        let implementation_hash = impl_hash(&impl_program)?;
        let evidence = EvidenceRecord {
            id: evidence_id.clone(),
            class: EvidenceClass::Correctness,
            kind: EvidenceKind::IdentityLowering,
            spec_hash: spec_hash.clone(),
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            input_impl_hash: None,
            output_impl_hash: implementation_hash.clone(),
            method: "identity_lowering".to_owned(),
            parameters: BTreeMap::new(),
            result: EvidenceResult::Passed,
            counterexample: None,
            provenance: provenance(),
        };
        let certificate = EquivalenceCertificate {
            rule: "identity_lowering".to_owned(),
            before_impl_hash: None,
            after_impl_hash: implementation_hash.clone(),
            targets: Vec::new(),
            side_conditions: vec![
                "complete frozen SpecIR".to_owned(),
                "structural identity lowering".to_owned(),
            ],
            impl_semantics_version: IMPL_SEMANTICS_VERSION,
            evidence: evidence_id.clone(),
        };
        let equivalence = EquivalenceObligation {
            id: obligation_id,
            relation,
            spec_hash: spec_hash.clone(),
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            impl_hash: implementation_hash.clone(),
            status: EquivalenceStatus::Proved,
        };
        let revision = CandidateRevision {
            id: revision_id.clone(),
            parents: Vec::new(),
            impl_program,
            impl_hash: implementation_hash.clone(),
            candidate_hash: CandidateHash::new("pending"),
            candidate_hash_version: LEGACY_CANDIDATE_CANONICAL_VERSION,
            state: CandidateState::Equivalent,
            equivalence,
            proof_chain: vec![certificate],
            evidence: vec![evidence_id.clone()],
            proof_frontier: None,
            proof_debt: Vec::new(),
            translation_results: Vec::new(),
            guarded_fallback: None,
            equality_proofs: Vec::new(),
            equality_materializations: Vec::new(),
        };
        let mut candidate = Candidate {
            id: candidate_id.clone(),
            spec_revision: spec_revision.clone(),
            spec_hash,
            root_revision: revision_id.clone(),
            head: revision_id.clone(),
            revisions: BTreeMap::new(),
            parent_candidate: None,
            forked_from_revision: None,
        };
        let mut revision = revision;
        revision.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate,
            &revision,
            candidate_canonical_limit(&revision, limits),
        )?;
        let exact_hash = revision.candidate_hash.clone();
        candidate.revisions.insert(revision_id.clone(), revision);
        staged.evidence.insert(evidence_id, evidence);
        staged.candidates.insert(candidate_id.clone(), candidate);
        staged.events.push(VersionedCandidateEvent {
            semantics_version: LEGACY_CANDIDATE_SEMANTICS_VERSION,
            event: CandidateEvent::Created {
                candidate: candidate_id.clone(),
                spec_revision,
                relation,
                candidate_revision: revision_id.clone(),
                impl_hash: implementation_hash,
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(&candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Atomically applies trusted exact rewrite actions.
    pub fn apply(
        &mut self,
        transaction: &CandidateTransaction,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        if transaction.actions.is_empty() {
            return Err(candidate_error(
                ErrorCode::InvalidRequest,
                "candidate transaction must contain at least one action",
            ));
        }
        BudgetCheck::against(
            limits,
            ResourceKind::CandidateActionsPerTransaction,
            u64::try_from(transaction.actions.len()).unwrap_or(u64::MAX),
            "candidate transaction before graph clone",
        )?;
        let (base_candidate, base_revision) =
            candidate_revision(self, &transaction.candidate, &transaction.base_revision)?;
        if base_candidate.head != transaction.base_revision {
            return Err(candidate_error(
                ErrorCode::CandidateRevisionNotFound,
                "candidate transaction base is stale",
            )
            .with_detail("current_head", base_candidate.head.to_string())
            .with_detail("base_revision", transaction.base_revision.to_string()));
        }
        if base_revision.state == CandidateState::Sealed {
            return Err(candidate_error(
                ErrorCode::CandidateSealed,
                "sealed candidate cannot be edited",
            ));
        }
        if base_revision.equivalence.status != EquivalenceStatus::Proved
            || base_revision
                .proof_debt
                .iter()
                .any(|debt| debt.status != ProofDebtStatus::Proved)
        {
            return Err(candidate_error(
                ErrorCode::CandidateHasProofDebt,
                "known rewrites require an exact proved candidate head",
            ));
        }
        verify_candidate_revision(
            self,
            base_candidate,
            base_revision,
            source,
            expected_spec_hash,
            limits,
        )?;
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let mut next = base_revision.clone();
        next.id = revision_id.clone();
        next.parents = vec![transaction.base_revision.clone()];
        next.state = CandidateState::Equivalent;
        next.equivalence.candidate_revision = revision_id.clone();
        for action in &transaction.actions {
            let CandidateAction::ApplyKnownRewrite {
                rule,
                target,
                expected_before_impl_hash,
            } = action;
            if expected_before_impl_hash
                .as_ref()
                .is_some_and(|expected| expected != &next.impl_hash)
            {
                return Err(candidate_error(
                    ErrorCode::RewritePreconditionFailed,
                    "expected_before_impl_hash is stale",
                )
                .with_types(
                    expected_before_impl_hash
                        .as_ref()
                        .expect("checked")
                        .to_string(),
                    next.impl_hash.to_string(),
                )
                .with_detail("rule", rule.clone())
                .with_detail("target", target.to_string()));
            }
            BudgetCheck::against(
                limits,
                ResourceKind::RewriteStepsPerCandidate,
                // The first proof edge is identity lowering, so the current
                // chain length is exactly the projected number of rewrite
                // edges after appending this action.
                u64::try_from(next.proof_chain.len()).unwrap_or(u64::MAX),
                "candidate rewrite proof chain",
            )?;
            let before = next.impl_hash.clone();
            let side_conditions = apply_known_rewrite(&mut next.impl_program, rule, target)?;
            verify_impl(&next.impl_program, source, limits)?;
            let after = impl_hash(&next.impl_program)?;
            let evidence_id = staged.allocator.evidence();
            let evidence = EvidenceRecord {
                id: evidence_id.clone(),
                class: EvidenceClass::Correctness,
                kind: EvidenceKind::KnownRewriteCertificate,
                spec_hash: base_candidate.spec_hash.clone(),
                candidate: base_candidate.id.clone(),
                candidate_revision: revision_id.clone(),
                input_impl_hash: Some(before.clone()),
                output_impl_hash: after.clone(),
                method: rule.clone(),
                parameters: BTreeMap::from([("target".to_owned(), json!(target))]),
                result: EvidenceResult::Passed,
                counterexample: None,
                provenance: provenance(),
            };
            staged.evidence.insert(evidence_id.clone(), evidence);
            next.proof_chain.push(EquivalenceCertificate {
                rule: rule.clone(),
                before_impl_hash: Some(before),
                after_impl_hash: after.clone(),
                targets: vec![target.clone()],
                side_conditions,
                impl_semantics_version: IMPL_SEMANTICS_VERSION,
                evidence: evidence_id.clone(),
            });
            next.evidence.push(evidence_id);
            next.impl_hash = after;
        }
        next.equivalence.impl_hash = next.impl_hash.clone();
        next.equivalence.status = EquivalenceStatus::Proved;
        if next.candidate_hash_version != LEGACY_CANDIDATE_CANONICAL_VERSION
            && next.proof_frontier.is_some()
        {
            next.proof_frontier = Some(ProofFrontier {
                candidate: transaction.candidate.clone(),
                candidate_revision: revision_id.clone(),
                terminal_proved_impl_hash: next.impl_hash.clone(),
            });
        }
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(&transaction.candidate)
            .expect("candidate was checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let implementation_hash = next.impl_hash.clone();
        let exact_hash = next.candidate_hash.clone();
        let candidate = staged
            .candidates
            .get_mut(&transaction.candidate)
            .expect("candidate was checked");
        candidate.revisions.insert(revision_id.clone(), next);
        candidate.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: semantics_for_hash_version(
                candidate_snapshot
                    .revisions
                    .get(&transaction.base_revision)
                    .map_or(LEGACY_CANDIDATE_CANONICAL_VERSION, |revision| {
                        revision.candidate_hash_version
                    }),
            ),
            event: CandidateEvent::TransactionApplied {
                transaction: transaction.clone(),
                candidate_revision: revision_id.clone(),
                impl_hash: implementation_hash,
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(&transaction.candidate, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Atomically accepts one bounded typed replacement and creates ordered proof debt.
    pub fn propose(
        &mut self,
        candidate_id: &CandidateId,
        base_revision_id: &CandidateRevisionId,
        proposal: &SpeculativeRewriteProposal,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        BudgetCheck::against(
            limits,
            ResourceKind::ProposalActionsPerTransaction,
            u64::try_from(proposal.replacement.operations.len()).unwrap_or(u64::MAX),
            "candidate proposal before graph clone",
        )?;
        let (candidate, base) = candidate_revision(self, candidate_id, base_revision_id)?;
        if candidate.head != *base_revision_id {
            return Err(candidate_error(
                ErrorCode::CandidateRevisionNotFound,
                "candidate proposal base is stale",
            )
            .with_detail("current_head", candidate.head.to_string())
            .with_detail("base_revision", base_revision_id.to_string()));
        }
        if matches!(
            base.state,
            CandidateState::Sealed | CandidateState::Rejected
        ) {
            return Err(candidate_error(
                if base.state == CandidateState::Sealed {
                    ErrorCode::CandidateSealed
                } else {
                    ErrorCode::ObligationRefuted
                },
                "sealed or rejected candidate cannot accept a proposal",
            ));
        }
        verify_candidate_revision(self, candidate, base, source, expected_spec_hash, limits)?;
        if proposal.expected_before_impl_hash != base.impl_hash {
            return Err(candidate_error(
                ErrorCode::RewritePreconditionFailed,
                "proposal expected_before_impl_hash is stale",
            )
            .with_types(
                proposal.expected_before_impl_hash.to_string(),
                base.impl_hash.to_string(),
            )
            .with_detail("target", proposal.target.to_string())
            .with_repair("refresh candidate.proposal_query or candidate.continuation"));
        }
        let normalized = normalize_speculative_proposal(proposal)?;
        let target = base
            .impl_program
            .operations
            .get(&normalized.target)
            .ok_or_else(|| {
                invalid_proposal(format!(
                    "proposal target `{}` does not exist",
                    normalized.target
                ))
            })?
            .clone();
        let proposal_hash =
            proposal_canonical_with_target(&base.impl_program, &target, &normalized, limits)?
                .proposal_hash;
        BudgetCheck::against(
            limits,
            ResourceKind::ProposalsPerWorkspace,
            u64::try_from(self.proposals.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "candidate proposal before persistent ID allocation",
        )?;
        let projected_debt = u64::try_from(base.proof_debt.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if projected_debt > limits.open_proof_debt_obligations {
            return Err(candidate_error(
                ErrorCode::ProofDebtLimitExceeded,
                "candidate proposal would exceed the ordered proof-debt limit",
            )
            .with_types(limits.open_proof_debt_obligations, projected_debt)
            .with_detail("candidate", candidate_id.to_string())
            .with_repair("fork the proved proof-frontier ancestor"));
        }
        BudgetCheck::against(
            limits,
            ResourceKind::SpeculativeDepthPerBranch,
            projected_debt,
            "candidate proposal before persistent ID allocation",
        )?;

        let mut staged = self.clone();
        let mut staged_allocator = staged.allocator.clone();
        let applied = apply_proposal_fragment(
            &base.impl_program,
            &target,
            &normalized,
            &mut staged_allocator,
            source,
            limits,
        )?;
        let classification = classify_proposal(&base.impl_program, &target, &applied)?;
        if matches!(
            classification,
            ProposalClassification::Conditional
                | ProposalClassification::Unknown
                | ProposalClassification::Unsupported
        ) && !normalized.allow_speculative
        {
            return Err(candidate_error(
                ErrorCode::SpeculativeOptInRequired,
                "proposal is not yet an exact trusted rewrite",
            )
            .with_detail("classification", json!(classification))
            .with_detail("target", normalized.target.to_string())
            .with_repair("set allow_speculative to true to create bounded proof debt"));
        }
        if classification == ProposalClassification::Conditional
            && (!base.proof_debt.is_empty() || base.equivalence.status != EquivalenceStatus::Proved)
        {
            return Err(candidate_error(
                ErrorCode::FallbackInvalid,
                "guarded self-division requires the exact parent revision as fallback",
            )
            .with_repair("fork or propose from the proved proof-frontier revision"));
        }
        staged.allocator = staged_allocator;
        let proposal_id = staged.allocator.proposal();
        let revision_id = staged.allocator.revision();
        let obligation_id = staged.allocator.obligation();
        let after_impl_hash = impl_hash(&applied.program)?;
        let record = ProposalRecord {
            id: proposal_id.clone(),
            proposal_hash: proposal_hash.clone(),
            candidate: candidate_id.clone(),
            base_candidate_revision: base_revision_id.clone(),
            accepted_candidate_revision: revision_id.clone(),
            classification,
            proposal: normalized.clone(),
            after_impl_hash: after_impl_hash.clone(),
            allocated_operations: applied.allocated_operations,
            allocated_values: applied.allocated_values,
            yielded_value: applied.yielded_value,
        };
        staged.proposals.insert(proposal_id.clone(), record);
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision_id.clone()];
        next.impl_program = applied.program;
        next.impl_hash = after_impl_hash.clone();
        next.candidate_hash_version = CANDIDATE_CANONICAL_VERSION;
        next.state = CandidateState::Speculative;
        next.equivalence.candidate_revision = revision_id.clone();
        next.equivalence.impl_hash = after_impl_hash.clone();
        next.equivalence.status = EquivalenceStatus::Open;
        if next.proof_frontier.is_none() {
            next.proof_frontier = Some(ProofFrontier {
                candidate: candidate_id.clone(),
                candidate_revision: base_revision_id.clone(),
                terminal_proved_impl_hash: base.impl_hash.clone(),
            });
        }
        next.proof_debt.push(ProofDebtItem {
            id: obligation_id,
            proposal: proposal_id.clone(),
            proposal_hash: proposal_hash.clone(),
            base_candidate_revision: base_revision_id.clone(),
            before_impl_hash: base.impl_hash.clone(),
            after_impl_hash: after_impl_hash.clone(),
            target: target.id,
            boundary: target.operands,
            relation: RelationKind::EquivalentToSpec,
            status: ProofDebtStatus::Open,
            allowed_discharge_methods: vec![
                "canonical_identity_validation".to_owned(),
                "recognized_known_rewrite".to_owned(),
                "guarded_self_division".to_owned(),
            ],
            evidence: Vec::new(),
            first_counterexample: None,
            origin_candidate_event: u64::try_from(staged.events.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        });
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
            event: CandidateEvent::ProposalAccepted {
                candidate: candidate_id.clone(),
                base_revision: base_revision_id.clone(),
                proposal: normalized,
                proposal_id,
                candidate_revision: revision_id.clone(),
                proposal_hash,
                impl_hash: after_impl_hash,
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Runs the compiler-owned validator on the next ordered proof-debt item.
    pub fn translation_check(
        &mut self,
        candidate_id: &CandidateId,
        base_revision_id: &CandidateRevisionId,
        proposal_id: &ProposalId,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<TranslationCheckReport> {
        BudgetCheck::against(
            limits,
            ResourceKind::TranslationValidationWorkUnits,
            u64::try_from(KNOWN_REWRITE_RULES.len())
                .unwrap_or(u64::MAX)
                .saturating_add(2),
            "trusted translation validator",
        )?;
        let (candidate, base) = candidate_revision(self, candidate_id, base_revision_id)?;
        if candidate.head != *base_revision_id {
            return Err(candidate_error(
                ErrorCode::CandidateRevisionNotFound,
                "translation check base is stale",
            ));
        }
        if base.state == CandidateState::Sealed {
            return Err(candidate_error(
                ErrorCode::CandidateSealed,
                "sealed candidate cannot record translation validation",
            ));
        }
        verify_candidate_revision(self, candidate, base, source, expected_spec_hash, limits)?;
        let debt_index = base
            .proof_debt
            .iter()
            .position(|debt| &debt.proposal == proposal_id)
            .ok_or_else(|| {
                candidate_error(
                    ErrorCode::ProposalNotFound,
                    format!("proposal `{proposal_id}` is not in candidate proof debt"),
                )
            })?;
        if base.proof_debt[debt_index].status != ProofDebtStatus::Open {
            let existing = base
                .translation_results
                .iter()
                .rev()
                .find(|record| &record.proposal == proposal_id)
                .cloned()
                .ok_or_else(|| {
                    candidate_error(
                        ErrorCode::EvidenceInvalid,
                        "terminal proof debt lacks its translation result",
                    )
                })?;
            let report = self.check(candidate_id, base_revision_id, source, limits)?;
            return Ok(TranslationCheckReport {
                diagnostic: matches!(existing.result, TranslationValidationResult::Unsupported)
                    .then_some(ErrorCode::TranslationUnsupported),
                validation: existing,
                candidate: report,
            });
        }
        if base
            .proof_debt
            .iter()
            .take(debt_index)
            .any(|debt| !matches!(debt.status, ProofDebtStatus::Proved))
        {
            return Err(candidate_error(
                ErrorCode::CandidateHasProofDebt,
                "translation validation cannot skip earlier ordered proof debt",
            )
            .with_detail("proposal", proposal_id.to_string())
            .with_repair("validate the first open obligation in order"));
        }
        let proposal = self.proposal(proposal_id)?.clone();
        let proposal_candidate = self.candidates.get(&proposal.candidate).ok_or_else(|| {
            candidate_error(
                ErrorCode::CandidateNotFound,
                "proposal origin candidate is missing",
            )
        })?;
        let before_revision = proposal_candidate
            .revisions
            .get(&proposal.base_candidate_revision)
            .ok_or_else(|| {
                candidate_error(
                    ErrorCode::CandidateRevisionNotFound,
                    "proposal base revision is missing",
                )
            })?;
        let accepted_revision = proposal_candidate
            .revisions
            .get(&proposal.accepted_candidate_revision)
            .ok_or_else(|| {
                candidate_error(
                    ErrorCode::CandidateRevisionNotFound,
                    "proposal acceptance revision is missing",
                )
            })?;
        let target = before_revision
            .impl_program
            .operations
            .get(&proposal.proposal.target)
            .ok_or_else(|| invalid_proposal("proposal target disappeared from exact base"))?;

        let path = if proposal.proposal.expected_before_impl_hash == proposal.after_impl_hash {
            TrustedPath::Identity
        } else {
            let mut recognized = None;
            for rule in KNOWN_REWRITE_RULES {
                let mut transformed = before_revision.impl_program.clone();
                if let Ok(side) = apply_known_rewrite(&mut transformed, rule.id, &target.id) {
                    if impl_hash(&transformed)? == proposal.after_impl_hash {
                        recognized = Some(TrustedPath::Known {
                            rule: rule.id.to_owned(),
                            side,
                        });
                        break;
                    }
                }
            }
            if let Some(path) = recognized {
                path
            } else {
                let applied = AppliedProposal {
                    program: accepted_revision.impl_program.clone(),
                    allocated_operations: proposal.allocated_operations.clone(),
                    allocated_values: proposal.allocated_values.clone(),
                    yielded_value: proposal.yielded_value.clone(),
                };
                if guarded_self_division_matches(&before_revision.impl_program, target, &applied) {
                    TrustedPath::Guarded {
                        guard_value: target.operands[0].clone(),
                    }
                } else {
                    TrustedPath::Unsupported
                }
            }
        };

        let mut staged = self.clone();
        BudgetCheck::against(
            limits,
            ResourceKind::TranslationValidationAttempts,
            staged
                .events
                .iter()
                .filter(|event| matches!(&event.event, CandidateEvent::TranslationChecked { .. }))
                .count()
                .try_into()
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "translation validation before persistent ID allocation",
        )?;
        let revision_id = staged.allocator.revision();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision_id.clone()];
        next.equivalence.candidate_revision = revision_id.clone();
        next.candidate_hash_version = CANDIDATE_CANONICAL_VERSION;
        let debt = next
            .proof_debt
            .get_mut(debt_index)
            .expect("debt index was checked");
        let (result, evidence_id) = match path {
            TrustedPath::Identity => {
                let evidence_id = staged.allocator.evidence();
                debt.status = ProofDebtStatus::Proved;
                debt.evidence.push(evidence_id.clone());
                next.evidence.push(evidence_id.clone());
                next.proof_chain.push(EquivalenceCertificate {
                    rule: "canonical_identity_validation".to_owned(),
                    before_impl_hash: Some(debt.before_impl_hash.clone()),
                    after_impl_hash: debt.after_impl_hash.clone(),
                    targets: vec![debt.target.clone()],
                    side_conditions: vec!["before_impl_hash == after_impl_hash".to_owned()],
                    impl_semantics_version: IMPL_SEMANTICS_VERSION,
                    evidence: evidence_id.clone(),
                });
                staged.evidence.insert(
                    evidence_id.clone(),
                    EvidenceRecord {
                        id: evidence_id.clone(),
                        class: EvidenceClass::Correctness,
                        kind: EvidenceKind::CanonicalIdentityValidation,
                        spec_hash: candidate.spec_hash.clone(),
                        candidate: candidate_id.clone(),
                        candidate_revision: revision_id.clone(),
                        input_impl_hash: Some(debt.before_impl_hash.clone()),
                        output_impl_hash: debt.after_impl_hash.clone(),
                        method: "canonical_identity_validation".to_owned(),
                        parameters: BTreeMap::from([
                            ("proposal_hash".to_owned(), json!(proposal.proposal_hash)),
                            ("target".to_owned(), json!(debt.target)),
                        ]),
                        result: EvidenceResult::Passed,
                        counterexample: None,
                        provenance: current_provenance(),
                    },
                );
                (
                    TranslationValidationResult::CanonicalIdentity,
                    Some(evidence_id),
                )
            }
            TrustedPath::Known { rule, side } => {
                let evidence_id = staged.allocator.evidence();
                debt.status = ProofDebtStatus::Proved;
                debt.evidence.push(evidence_id.clone());
                next.evidence.push(evidence_id.clone());
                next.proof_chain.push(EquivalenceCertificate {
                    rule: rule.clone(),
                    before_impl_hash: Some(debt.before_impl_hash.clone()),
                    after_impl_hash: debt.after_impl_hash.clone(),
                    targets: vec![debt.target.clone()],
                    side_conditions: side.clone(),
                    impl_semantics_version: IMPL_SEMANTICS_VERSION,
                    evidence: evidence_id.clone(),
                });
                staged.evidence.insert(
                    evidence_id.clone(),
                    EvidenceRecord {
                        id: evidence_id.clone(),
                        class: EvidenceClass::Correctness,
                        kind: EvidenceKind::RecognizedKnownRewrite,
                        spec_hash: candidate.spec_hash.clone(),
                        candidate: candidate_id.clone(),
                        candidate_revision: revision_id.clone(),
                        input_impl_hash: Some(debt.before_impl_hash.clone()),
                        output_impl_hash: debt.after_impl_hash.clone(),
                        method: rule.clone(),
                        parameters: BTreeMap::from([
                            ("proposal_hash".to_owned(), json!(proposal.proposal_hash)),
                            ("target".to_owned(), json!(debt.target)),
                        ]),
                        result: EvidenceResult::Passed,
                        counterexample: None,
                        provenance: current_provenance(),
                    },
                );
                (
                    TranslationValidationResult::RecognizedKnownRewrite {
                        rule,
                        side_conditions: side,
                    },
                    Some(evidence_id),
                )
            }
            TrustedPath::Guarded { guard_value } => {
                if before_revision.equivalence.status != EquivalenceStatus::Proved
                    || before_revision.state == CandidateState::Rejected
                    || !before_revision
                        .proof_debt
                        .iter()
                        .all(|debt| debt.status == ProofDebtStatus::Proved)
                {
                    return Err(candidate_error(
                        ErrorCode::FallbackInvalid,
                        "guarded rewrite fallback is not fully proved exact",
                    ));
                }
                let evidence_id = staged.allocator.evidence();
                let guarded_fallback = GuardedFallback {
                    guard: GuardPredicate::I32NonZero { value: guard_value },
                    fallback_candidate: proposal.candidate.clone(),
                    fallback_revision: proposal.base_candidate_revision.clone(),
                    fallback_candidate_hash: before_revision.candidate_hash.clone(),
                    failure_strategy: "evaluate_fallback".to_owned(),
                    evidence: evidence_id.clone(),
                };
                debt.status = ProofDebtStatus::Guarded;
                debt.evidence.push(evidence_id.clone());
                next.evidence.push(evidence_id.clone());
                next.guarded_fallback = Some(guarded_fallback.clone());
                next.proof_chain.push(EquivalenceCertificate {
                    rule: "guarded_i32_self_division".to_owned(),
                    before_impl_hash: Some(debt.before_impl_hash.clone()),
                    after_impl_hash: debt.after_impl_hash.clone(),
                    targets: vec![debt.target.clone()],
                    side_conditions: vec![
                        "x != 0 selects exact constant-one primary".to_owned(),
                        "x == 0 selects the proved exact fallback".to_owned(),
                    ],
                    impl_semantics_version: IMPL_SEMANTICS_VERSION,
                    evidence: evidence_id.clone(),
                });
                staged.evidence.insert(
                    evidence_id.clone(),
                    EvidenceRecord {
                        id: evidence_id.clone(),
                        class: EvidenceClass::Correctness,
                        kind: EvidenceKind::GuardedRewriteCertificate,
                        spec_hash: candidate.spec_hash.clone(),
                        candidate: candidate_id.clone(),
                        candidate_revision: revision_id.clone(),
                        input_impl_hash: Some(debt.before_impl_hash.clone()),
                        output_impl_hash: debt.after_impl_hash.clone(),
                        method: "guarded_i32_self_division".to_owned(),
                        parameters: BTreeMap::from([
                            ("proposal_hash".to_owned(), json!(proposal.proposal_hash)),
                            ("target".to_owned(), json!(debt.target)),
                            (
                                "fallback_revision".to_owned(),
                                json!(proposal.base_candidate_revision),
                            ),
                        ]),
                        result: EvidenceResult::Passed,
                        counterexample: None,
                        provenance: current_provenance(),
                    },
                );
                (
                    TranslationValidationResult::GuardedSelfDivision { guarded_fallback },
                    Some(evidence_id),
                )
            }
            TrustedPath::Unsupported => {
                debt.status = ProofDebtStatus::Unsupported;
                (TranslationValidationResult::Unsupported, None)
            }
        };
        match &result {
            TranslationValidationResult::CanonicalIdentity
            | TranslationValidationResult::RecognizedKnownRewrite { .. } => {
                let all_proved = next
                    .proof_debt
                    .iter()
                    .all(|debt| debt.status == ProofDebtStatus::Proved);
                let terminal = next.proof_debt[debt_index].after_impl_hash.clone();
                next.proof_frontier = Some(ProofFrontier {
                    candidate: if all_proved {
                        candidate_id.clone()
                    } else {
                        proposal.candidate.clone()
                    },
                    candidate_revision: if all_proved {
                        revision_id.clone()
                    } else {
                        proposal.accepted_candidate_revision.clone()
                    },
                    terminal_proved_impl_hash: terminal,
                });
                next.equivalence.status = if all_proved {
                    EquivalenceStatus::Proved
                } else {
                    EquivalenceStatus::Open
                };
                next.state = if all_proved {
                    CandidateState::Equivalent
                } else {
                    CandidateState::Speculative
                };
            }
            TranslationValidationResult::GuardedSelfDivision { .. } => {
                next.proof_frontier = Some(ProofFrontier {
                    candidate: candidate_id.clone(),
                    candidate_revision: revision_id.clone(),
                    terminal_proved_impl_hash: next.proof_debt[debt_index].after_impl_hash.clone(),
                });
                next.state = CandidateState::Guarded;
                next.equivalence.status = EquivalenceStatus::Guarded;
            }
            TranslationValidationResult::Unsupported => {
                next.state = CandidateState::Speculative;
                next.equivalence.status = EquivalenceStatus::Unsupported;
            }
            TranslationValidationResult::Refuted { .. } => unreachable!("not produced here"),
        }
        let validation = TranslationValidationRecord {
            proposal: proposal_id.clone(),
            obligation: next.proof_debt[debt_index].id.clone(),
            candidate_revision: revision_id.clone(),
            validator_id: TRANSLATION_VALIDATOR_ID.to_owned(),
            validator_version: TRANSLATION_VALIDATOR_VERSION,
            result: result.clone(),
            evidence: evidence_id,
        };
        next.translation_results.push(validation.clone());
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
            event: CandidateEvent::TranslationChecked {
                candidate: candidate_id.clone(),
                base_revision: base_revision_id.clone(),
                proposal: proposal_id.clone(),
                candidate_revision: revision_id.clone(),
                result: result.clone(),
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(TranslationCheckReport {
            diagnostic: matches!(result, TranslationValidationResult::Unsupported)
                .then_some(ErrorCode::TranslationUnsupported),
            validation,
            candidate: report,
        })
    }

    /// Forks one candidate revision into a new editable branch identity.
    pub fn fork(
        &mut self,
        parent_candidate: &CandidateId,
        parent_revision: &CandidateRevisionId,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        let (parent, revision) = candidate_revision(self, parent_candidate, parent_revision)?;
        verify_candidate_revision(self, parent, revision, source, expected_spec_hash, limits)?;
        BudgetCheck::against(
            limits,
            ResourceKind::CandidateBranches,
            u64::try_from(self.candidates.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "candidate fork before ID allocation",
        )?;
        let mut staged = self.clone();
        let candidate_id = staged.allocator.candidate();
        let revision_id = staged.allocator.revision();
        let obligation_id = staged.allocator.obligation();
        let mut child_revision = revision.clone();
        child_revision.id = revision_id.clone();
        child_revision.parents.clear();
        child_revision.state = if revision.state == CandidateState::Sealed {
            CandidateState::Draft
        } else {
            revision.state
        };
        child_revision.equivalence = EquivalenceObligation {
            id: obligation_id,
            relation: RelationKind::EquivalentToSpec,
            spec_hash: parent.spec_hash.clone(),
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            impl_hash: child_revision.impl_hash.clone(),
            status: revision.equivalence.status,
        };
        let mut child = Candidate {
            id: candidate_id.clone(),
            spec_revision: parent.spec_revision.clone(),
            spec_hash: parent.spec_hash.clone(),
            root_revision: revision_id.clone(),
            head: revision_id.clone(),
            revisions: BTreeMap::new(),
            parent_candidate: Some(parent_candidate.clone()),
            forked_from_revision: Some(parent_revision.clone()),
        };
        child_revision.candidate_hash = candidate_hash_with_limit(
            &staged,
            &child,
            &child_revision,
            candidate_canonical_limit(&child_revision, limits),
        )?;
        let event_semantics = semantics_for_hash_version(child_revision.candidate_hash_version);
        let exact_hash = child_revision.candidate_hash.clone();
        child.revisions.insert(revision_id.clone(), child_revision);
        staged.candidates.insert(candidate_id.clone(), child);
        staged.events.push(VersionedCandidateEvent {
            semantics_version: event_semantics,
            event: CandidateEvent::Forked {
                parent_candidate: parent_candidate.clone(),
                parent_revision: parent_revision.clone(),
                candidate: candidate_id.clone(),
                candidate_revision: revision_id.clone(),
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(&candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Records deterministic differential confidence evidence in a new revision.
    pub fn record_validation(
        &mut self,
        candidate_id: &CandidateId,
        base_revision: &CandidateRevisionId,
        validation: DifferentialValidation,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        let valid_result_shape = validation.requested_cases > 0
            && validation.executed_cases > 0
            && validation.executed_cases <= validation.requested_cases
            && if validation.passed {
                validation.executed_cases == validation.requested_cases
                    && validation.counterexample.is_none()
            } else {
                validation.counterexample.is_some()
            };
        if !valid_result_shape {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "differential validation result is internally inconsistent",
            ));
        }
        BudgetCheck::against(
            limits,
            ResourceKind::DifferentialCases,
            validation.requested_cases,
            "candidate differential validation",
        )?;
        if let Some(counterexample) = &validation.counterexample {
            let bytes = serde_json::to_vec(counterexample).map_err(|error| {
                candidate_error(
                    ErrorCode::EvidenceInvalid,
                    format!("counterexample serialization failed: {error}"),
                )
            })?;
            BudgetCheck::against(
                limits,
                ResourceKind::CounterexampleBytes,
                u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                "counterexample before EvidenceIR publication",
            )?;
        }
        let (candidate, base) = candidate_revision(self, candidate_id, base_revision)?;
        if candidate.head != *base_revision {
            return Err(candidate_error(
                ErrorCode::CandidateRevisionNotFound,
                "candidate validation base is stale",
            ));
        }
        if base.state == CandidateState::Sealed {
            return Err(candidate_error(
                ErrorCode::CandidateSealed,
                "sealed candidate cannot record new validation evidence",
            ));
        }
        verify_candidate_revision(self, candidate, base, source, expected_spec_hash, limits)?;
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let evidence_id = staged.allocator.evidence();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision.clone()];
        next.equivalence.candidate_revision = revision_id.clone();
        if !validation.passed {
            next.state = CandidateState::Rejected;
            if next.candidate_hash_version != LEGACY_CANDIDATE_CANONICAL_VERSION {
                let debt = next
                    .proof_debt
                    .iter_mut()
                    .find(|debt| {
                        matches!(
                            debt.status,
                            ProofDebtStatus::Open
                                | ProofDebtStatus::Unsupported
                                | ProofDebtStatus::Guarded
                        )
                    })
                    .ok_or_else(|| {
                        candidate_error(
                            ErrorCode::EvidenceInvalid,
                            "speculative counterexample has no affected proof-debt item",
                        )
                    })?;
                debt.status = ProofDebtStatus::Refuted;
                debt.first_counterexample
                    .clone_from(&validation.counterexample);
                debt.evidence.push(evidence_id.clone());
                next.equivalence.status = EquivalenceStatus::Refuted;
                next.guarded_fallback = None;
            }
        }
        next.evidence.push(evidence_id.clone());
        let evidence = EvidenceRecord {
            id: evidence_id.clone(),
            class: EvidenceClass::Confidence,
            kind: if base.candidate_hash_version == LEGACY_CANDIDATE_CANONICAL_VERSION {
                EvidenceKind::DifferentialTest
            } else {
                EvidenceKind::SpeculativeDifferentialTest
            },
            spec_hash: candidate.spec_hash.clone(),
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            input_impl_hash: Some(next.impl_hash.clone()),
            output_impl_hash: next.impl_hash.clone(),
            method: "fixed_seed_differential_validation".to_owned(),
            parameters: BTreeMap::from([
                ("seed".to_owned(), json!(validation.seed)),
                ("cases".to_owned(), json!(validation.requested_cases)),
            ]),
            result: if validation.passed {
                EvidenceResult::Passed
            } else {
                EvidenceResult::Failed
            },
            counterexample: validation.counterexample.clone(),
            provenance: provenance_for_hash_version(base.candidate_hash_version),
        };
        staged.evidence.insert(evidence_id, evidence);
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let event_semantics = semantics_for_hash_version(next.candidate_hash_version);
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: event_semantics,
            event: CandidateEvent::Validated {
                candidate: candidate_id.clone(),
                base_revision: base_revision.clone(),
                candidate_revision: revision_id.clone(),
                validation,
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Seals a proved exact candidate; repeated seal is idempotent.
    pub fn seal(
        &mut self,
        candidate_id: &CandidateId,
        base_revision: &CandidateRevisionId,
        source: &Program,
        expected_spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        let (candidate, base) = candidate_revision(self, candidate_id, base_revision)?;
        if candidate.head != *base_revision {
            return Err(candidate_error(
                ErrorCode::CandidateRevisionNotFound,
                "candidate seal base is stale",
            ));
        }
        verify_candidate_revision(self, candidate, base, source, expected_spec_hash, limits)?;
        if base.state == CandidateState::Sealed {
            return self.check(candidate_id, base_revision, source, limits);
        }
        if base.state == CandidateState::Rejected
            || base.equivalence.status == EquivalenceStatus::Refuted
        {
            return Err(candidate_error(
                ErrorCode::ObligationRefuted,
                "refuted candidate cannot be sealed",
            ));
        }
        if !matches!(
            base.equivalence.status,
            EquivalenceStatus::Proved | EquivalenceStatus::Guarded
        ) || base.proof_debt.iter().any(|debt| {
            !matches!(
                debt.status,
                ProofDebtStatus::Proved | ProofDebtStatus::Guarded
            )
        }) {
            return Err(candidate_error(
                ErrorCode::CandidateHasProofDebt,
                "candidate cannot be sealed with open or unsupported proof debt",
            )
            .with_detail("candidate", candidate_id.to_string())
            .with_repair("run candidate.translation_check or recover from the proof frontier"));
        }
        let mut staged = self.clone();
        let revision_id = staged.allocator.revision();
        let evidence_id = staged.allocator.evidence();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision.clone()];
        next.state = CandidateState::Sealed;
        next.equivalence.candidate_revision = revision_id.clone();
        next.evidence.push(evidence_id.clone());
        staged.evidence.insert(
            evidence_id.clone(),
            EvidenceRecord {
                id: evidence_id,
                class: EvidenceClass::Correctness,
                kind: if base.candidate_hash_version == LEGACY_CANDIDATE_CANONICAL_VERSION {
                    EvidenceKind::CompositionalEquivalence
                } else {
                    EvidenceKind::CompositionalSpeculativeDischarge
                },
                spec_hash: candidate.spec_hash.clone(),
                candidate: candidate_id.clone(),
                candidate_revision: revision_id.clone(),
                input_impl_hash: Some(next.impl_hash.clone()),
                output_impl_hash: next.impl_hash.clone(),
                method: "verify_compositional_equivalence".to_owned(),
                parameters: BTreeMap::from([(
                    "proof_edges".to_owned(),
                    json!(next.proof_chain.len()),
                )]),
                result: EvidenceResult::Passed,
                counterexample: None,
                provenance: provenance_for_hash_version(base.candidate_hash_version),
            },
        );
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let event_semantics = semantics_for_hash_version(next.candidate_hash_version);
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: event_semantics,
            event: CandidateEvent::Sealed {
                candidate: candidate_id.clone(),
                base_revision: base_revision.clone(),
                candidate_revision: revision_id.clone(),
                candidate_hash: exact_hash,
            },
        });
        ensure_forest_budgets(&staged, limits)?;
        let report = staged.check(candidate_id, &revision_id, source, limits)?;
        *self = staged;
        Ok(report)
    }

    /// Fully verifies one candidate revision and returns its sealability report.
    pub fn check(
        &self,
        candidate_id: &CandidateId,
        revision_id: &CandidateRevisionId,
        source: &Program,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateCheckReport> {
        let (candidate, revision) = candidate_revision(self, candidate_id, revision_id)?;
        verify_candidate_revision(
            self,
            candidate,
            revision,
            source,
            &candidate.spec_hash,
            limits,
        )?;
        let (correctness_evidence, confidence_evidence) =
            revision
                .evidence
                .iter()
                .fold(
                    (0_usize, 0_usize),
                    |(correctness, confidence), id| match self
                        .evidence
                        .get(id)
                        .map(|record| record.class)
                    {
                        Some(EvidenceClass::Correctness) => (correctness + 1, confidence),
                        Some(EvidenceClass::Confidence) => (correctness, confidence + 1),
                        None => (correctness, confidence),
                    },
                );
        let proved = matches!(
            revision.equivalence.status,
            EquivalenceStatus::Proved | EquivalenceStatus::Guarded
        );
        let open_obligations =
            if revision.candidate_hash_version == LEGACY_CANDIDATE_CANONICAL_VERSION {
                if proved {
                    Vec::new()
                } else {
                    vec![revision.equivalence.id.clone()]
                }
            } else {
                revision
                    .proof_debt
                    .iter()
                    .filter(|debt| {
                        !matches!(
                            debt.status,
                            ProofDebtStatus::Proved | ProofDebtStatus::Guarded
                        )
                    })
                    .map(|debt| debt.id.clone())
                    .collect()
            };
        Ok(CandidateCheckReport {
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            state: revision.state,
            well_typed: true,
            equivalence: revision.equivalence.clone(),
            impl_hash: revision.impl_hash.clone(),
            candidate_hash: revision.candidate_hash.clone(),
            open_obligations,
            correctness_evidence,
            confidence_evidence,
            sealable: proved
                && !matches!(
                    revision.state,
                    CandidateState::Rejected | CandidateState::Sealed
                )
                && revision.proof_debt.iter().all(|debt| {
                    matches!(
                        debt.status,
                        ProofDebtStatus::Proved | ProofDebtStatus::Guarded
                    )
                }),
            proof_frontier: revision.proof_frontier.clone(),
            proof_debt: revision.proof_debt.clone(),
        })
    }

    /// Enumerates bounded, deterministic applicable known rewrites.
    pub fn continuation(
        &self,
        candidate_id: &CandidateId,
        revision_id: &CandidateRevisionId,
        limits: &ResourceLimits,
    ) -> AgentResult<CandidateContinuation> {
        let (_, revision) = candidate_revision(self, candidate_id, revision_id)?;
        if revision.state == CandidateState::Sealed {
            return Err(candidate_error(
                ErrorCode::CandidateSealed,
                "sealed candidate has no rewrite continuation",
            ));
        }
        let matches = production_rewrite_matches(&revision.impl_program, limits)?
            .into_iter()
            .map(|production| CandidateContinuationEntry {
                rule: production.rule,
                target: production.target,
                side_conditions: production.side_conditions,
                applicability: RewriteApplicability::Applicable,
                reason_code: production.reason_code,
            })
            .collect::<Vec<_>>();
        BudgetCheck::against(
            limits,
            ResourceKind::RewriteMatchesPerContinuation,
            u64::try_from(matches.len()).unwrap_or(u64::MAX),
            "candidate continuation during shared production enumeration",
        )?;
        let speculative_escape = revision
            .impl_program
            .operation_order
            .iter()
            .filter_map(|operation| revision.impl_program.operations.get(operation))
            .find(|operation| operation.results.len() == 1 && operation.opcode != Opcode::Parameter)
            .map(|operation| SpeculativeEscapeSchema {
                target: operation.id.clone(),
                boundary_inputs: operation.operands.clone(),
                required_yield_type: operation.result_types[0].clone(),
                allowed_opcodes: [
                    Opcode::Constant,
                    Opcode::Add,
                    Opcode::Sub,
                    Opcode::Mul,
                    Opcode::Div,
                    Opcode::Fma,
                    Opcode::Compare,
                    Opcode::Select,
                    Opcode::Cast,
                    Opcode::Map,
                    Opcode::ZipMap,
                    Opcode::Reduce,
                ]
                .into_iter()
                .map(|opcode| opcode.to_string())
                .collect(),
                fragment_operation_limit: limits.proposal_fragment_operations,
                expected_before_impl_hash: revision.impl_hash.clone(),
                requires_speculative_opt_in: true,
                reason_code: "BOUNDED_TYPED_REPLACEMENT".to_owned(),
            });
        Ok(CandidateContinuation {
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            expected_before_impl_hash: revision.impl_hash.clone(),
            trusted_known_rewrites: matches.clone(),
            matches,
            speculative_escape,
        })
    }

    /// Verifies every candidate/evidence/hash contract against frozen SpecIR anchors.
    pub fn verify_all<F>(&self, mut source: F, limits: &ResourceLimits) -> AgentResult<()>
    where
        F: FnMut(&RevisionId) -> AgentResult<(Program, SpecHash)>,
    {
        ensure_forest_budgets(self, limits)?;
        let mut referenced_evidence = BTreeSet::new();
        for candidate in self.candidates.values() {
            let (program, spec_hash) = source(&candidate.spec_revision)?;
            for revision in candidate.revisions.values() {
                verify_candidate_revision(self, candidate, revision, &program, &spec_hash, limits)?;
                referenced_evidence.extend(revision.evidence.iter().cloned());
            }
        }
        if referenced_evidence != self.evidence.keys().cloned().collect() {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                "candidate forest contains missing or orphaned EvidenceIR records",
            ));
        }
        Ok(())
    }
}
