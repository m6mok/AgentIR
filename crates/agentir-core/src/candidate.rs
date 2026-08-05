//! Persistent CandidateForest, trusted exact rewrites, proof chains, and EvidenceIR.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        CandidateId, CandidateObligationId, CandidateRevisionId, EvidenceId, ImplOperationId,
        ImplValueId, RevisionId,
    },
    impl_ir::{
        IMPL_SEMANTICS_VERSION, ImplHash, ImplProgram, ImplRegionValue, ImplSourceLink,
        ImplValueOrigin, identity_lower, impl_hash, verify_impl,
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

/// Candidate event semantics version, independent of core and archive versions.
pub const CANDIDATE_SEMANTICS_VERSION: u32 = 1;

/// Exact candidate-state canonical codec version.
pub const CANDIDATE_CANONICAL_VERSION: u32 = 1;

/// Domain separator for exact, history-sensitive candidate hashes.
pub const CANDIDATE_HASH_DOMAIN: &[u8] = b"agentir.candidate.exact.v1\0";

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

/// Monotonic allocator isolated from the legacy SpecIR allocator contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateAllocator {
    candidate: u64,
    revision: u64,
    operation: u64,
    value: u64,
    evidence: u64,
    obligation: u64,
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
}

/// Stage 2A candidate lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// Editable branch state, including a newly forked sealed candidate.
    Draft,
    /// Separate implementation graph verified successfully.
    WellTyped,
    /// Trusted certificates prove exact equivalence to frozen SpecIR.
    Equivalent,
    /// Immutable accepted implementation revision.
    Sealed,
    /// Deterministic validation found a counterexample or integrity failure.
    Rejected,
}

/// Relation requested between a candidate and its immutable specification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Exact semantic equivalence supported by Stage 2A.
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
}

/// Structured exact relation owned by one candidate revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquivalenceObligation {
    /// Compiler-assigned obligation ID.
    pub id: CandidateObligationId,
    /// Only exact equivalence is accepted in Stage 2A.
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
    /// Candidate lifecycle state at this revision.
    pub state: CandidateState,
    /// Exact equivalence obligation.
    pub equivalence: EquivalenceObligation,
    /// Ordered trusted proof chain.
    pub proof_chain: Vec<EquivalenceCertificate>,
    /// Ordered correctness and confidence evidence references.
    pub evidence: Vec<EvidenceId>,
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
    /// Candidate/ImplIR/evidence allocator state.
    pub allocator: CandidateAllocator,
    /// Ordered candidate event log.
    pub events: Vec<VersionedCandidateEvent>,
}

#[derive(Serialize)]
struct CandidateHashModel<'a> {
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

fn digest_hex(bytes: &[u8]) -> CandidateHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    CandidateHash(output)
}

fn candidate_hash_with_limit(
    candidate: &Candidate,
    revision: &CandidateRevision,
    max_bytes: u64,
) -> AgentResult<CandidateHash> {
    let model = CandidateHashModel {
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
    };
    let bytes = serde_json::to_vec(&model).map_err(|error| {
        AgentError::new(
            ErrorCode::CanonicalizationFailed,
            format!("candidate exact serialization failed: {error}"),
        )
    })?;
    BudgetCheck::ensure(
        ResourceKind::CandidateCanonicalBytes,
        max_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "candidate exact canonicalization",
    )?;
    let mut input = Vec::with_capacity(CANDIDATE_HASH_DOMAIN.len() + bytes.len());
    input.extend_from_slice(CANDIDATE_HASH_DOMAIN);
    input.extend_from_slice(&bytes);
    Ok(digest_hex(&input))
}

fn provenance() -> EvidenceProvenance {
    EvidenceProvenance {
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        candidate_semantics_version: CANDIDATE_SEMANTICS_VERSION,
        impl_semantics_version: IMPL_SEMANTICS_VERSION,
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

fn ensure_forest_budgets(forest: &CandidateForest, limits: &ResourceLimits) -> AgentResult<()> {
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
        if index > 0 && known_rewrite_rule(&certificate.rule).is_none() {
            return Err(candidate_error(
                ErrorCode::EvidenceInvalid,
                format!("certificate uses unknown rule `{}`", certificate.rule),
            ));
        }
        current = Some(certificate.after_impl_hash.clone());
    }
    if current.as_ref() != Some(&revision.impl_hash)
        || revision.equivalence.impl_hash != revision.impl_hash
        || revision.equivalence.spec_hash != candidate.spec_hash
        || revision.equivalence.candidate != candidate.id
        || revision.equivalence.candidate_revision != revision.id
        || revision.equivalence.relation != RelationKind::EquivalentToSpec
        || revision.equivalence.status != EquivalenceStatus::Proved
    {
        return Err(candidate_error(
            ErrorCode::EquivalenceNotProved,
            "proof chain does not establish the current exact candidate relation",
        ));
    }
    Ok(())
}

fn verify_candidate_revision(
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
            || evidence.provenance.candidate_semantics_version != CANDIDATE_SEMANTICS_VERSION
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
            ) | (
                EvidenceClass::Confidence,
                EvidenceKind::DifferentialTest | EvidenceKind::PropertyTest
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
    let actual_candidate_hash =
        candidate_hash_with_limit(candidate, revision, limits.candidate_canonical_bytes)?;
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

fn apply_rewrite(
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

fn push_bounded_match(
    matches: &mut Vec<CandidateContinuationEntry>,
    entry: CandidateContinuationEntry,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::RewriteMatchesPerContinuation,
        u64::try_from(matches.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        "candidate continuation during enumeration",
    )?;
    matches.push(entry);
    Ok(())
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
                "Stage 2A supports only EquivalentToSpec",
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
            state: CandidateState::Equivalent,
            equivalence,
            proof_chain: vec![certificate],
            evidence: vec![evidence_id.clone()],
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
        revision.candidate_hash =
            candidate_hash_with_limit(&candidate, &revision, limits.candidate_canonical_bytes)?;
        let exact_hash = revision.candidate_hash.clone();
        candidate.revisions.insert(revision_id.clone(), revision);
        staged.evidence.insert(evidence_id, evidence);
        staged.candidates.insert(candidate_id.clone(), candidate);
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
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
            let side_conditions = apply_rewrite(&mut next.impl_program, rule, target)?;
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
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(&transaction.candidate)
            .expect("candidate was checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &candidate_snapshot,
            &next,
            limits.candidate_canonical_bytes,
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
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
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
        child_revision.state = CandidateState::Draft;
        child_revision.equivalence = EquivalenceObligation {
            id: obligation_id,
            relation: RelationKind::EquivalentToSpec,
            spec_hash: parent.spec_hash.clone(),
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            impl_hash: child_revision.impl_hash.clone(),
            status: EquivalenceStatus::Proved,
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
        child_revision.candidate_hash =
            candidate_hash_with_limit(&child, &child_revision, limits.candidate_canonical_bytes)?;
        let exact_hash = child_revision.candidate_hash.clone();
        child.revisions.insert(revision_id.clone(), child_revision);
        staged.candidates.insert(candidate_id.clone(), child);
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
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
        }
        next.evidence.push(evidence_id.clone());
        let evidence = EvidenceRecord {
            id: evidence_id.clone(),
            class: EvidenceClass::Confidence,
            kind: EvidenceKind::DifferentialTest,
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
            provenance: provenance(),
        };
        staged.evidence.insert(evidence_id, evidence);
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &candidate_snapshot,
            &next,
            limits.candidate_canonical_bytes,
        )?;
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
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
            || base.equivalence.status != EquivalenceStatus::Proved
        {
            return Err(candidate_error(
                ErrorCode::EquivalenceNotProved,
                "candidate cannot be sealed without proved exact equivalence",
            ));
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
                kind: EvidenceKind::CompositionalEquivalence,
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
                provenance: provenance(),
            },
        );
        next.candidate_hash = CandidateHash::new("pending");
        let candidate_snapshot = staged
            .candidates
            .get(candidate_id)
            .expect("checked")
            .clone();
        next.candidate_hash = candidate_hash_with_limit(
            &candidate_snapshot,
            &next,
            limits.candidate_canonical_bytes,
        )?;
        let exact_hash = next.candidate_hash.clone();
        let candidate_mut = staged.candidates.get_mut(candidate_id).expect("checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged.events.push(VersionedCandidateEvent {
            semantics_version: CANDIDATE_SEMANTICS_VERSION,
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
        let proved = revision.equivalence.status == EquivalenceStatus::Proved;
        Ok(CandidateCheckReport {
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            state: revision.state,
            well_typed: true,
            equivalence: revision.equivalence.clone(),
            impl_hash: revision.impl_hash.clone(),
            candidate_hash: revision.candidate_hash.clone(),
            open_obligations: if proved {
                Vec::new()
            } else {
                vec![revision.equivalence.id.clone()]
            },
            correctness_evidence,
            confidence_evidence,
            sealable: proved
                && !matches!(
                    revision.state,
                    CandidateState::Rejected | CandidateState::Sealed
                ),
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
        let reachable = reachable_operations(&revision.impl_program)?;
        let mut matches = Vec::new();
        for operation in revision.impl_program.operations.keys() {
            if !reachable.contains(operation) {
                push_bounded_match(
                    &mut matches,
                    CandidateContinuationEntry {
                        rule: PRUNE_UNREACHABLE_RULE.to_owned(),
                        target: operation.clone(),
                        side_conditions: vec!["target is output-unreachable".to_owned()],
                        applicability: RewriteApplicability::Applicable,
                        reason_code: "UNREACHABLE_IMPL_NODE".to_owned(),
                    },
                    limits,
                )?;
                break;
            }
        }
        for operation in revision.impl_program.operations.keys() {
            if let Ok(side_conditions) =
                noop_cast_side_conditions(&revision.impl_program, operation)
            {
                push_bounded_match(
                    &mut matches,
                    CandidateContinuationEntry {
                        rule: ELIMINATE_NOOP_CAST_RULE.to_owned(),
                        target: operation.clone(),
                        side_conditions,
                        applicability: RewriteApplicability::Applicable,
                        reason_code: "IDENTICAL_CAST_TYPES".to_owned(),
                    },
                    limits,
                )?;
            }
            if fold_operation(&revision.impl_program, operation).is_ok() {
                push_bounded_match(
                    &mut matches,
                    CandidateContinuationEntry {
                        rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                        target: operation.clone(),
                        side_conditions: vec![
                            "all operands are exact scalar constants".to_owned(),
                            "reference evaluation is defined".to_owned(),
                        ],
                        applicability: RewriteApplicability::Applicable,
                        reason_code: "DEFINED_SCALAR_CONSTANT_FOLD".to_owned(),
                    },
                    limits,
                )?;
            }
        }
        matches
            .sort_by(|left, right| (&left.rule, &left.target).cmp(&(&right.rule, &right.target)));
        Ok(CandidateContinuation {
            candidate: candidate_id.clone(),
            candidate_revision: revision_id.clone(),
            expected_before_impl_hash: revision.impl_hash.clone(),
            matches,
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
