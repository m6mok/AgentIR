//! Bounded deterministic proof-carrying exact equality spaces over ImplIR.

use crate::{
    candidate::{
        CandidateAction, CandidateForest, CandidateState, CandidateTransaction,
        EQUALITY_CANDIDATE_CANONICAL_VERSION, EQUALITY_CANDIDATE_SEMANTICS_VERSION,
        EquivalenceCertificate, EquivalenceStatus, EvidenceClass, EvidenceKind, EvidenceProvenance,
        EvidenceRecord, EvidenceResult, ProductionRewriteMatch, ProofDebtStatus, ProofFrontier,
        RewriteTargetLocator, TranslationValidationRecord, TranslationValidationResult,
        apply_known_rewrite, candidate_canonical_limit, candidate_hash_with_limit,
        ensure_forest_budgets, production_rewrite_matches, resolve_rewrite_locator,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        CandidateId, CandidateObligationId, CandidateRevisionId, EqualityEdgeId, EqualityNodeId,
        EqualityRevisionId, EqualitySpaceId, EvidenceId, ProposalId, RevisionId,
    },
    impl_ir::{IMPL_SEMANTICS_VERSION, ImplHash, ImplProgram, impl_hash, verify_impl},
    ir::Program,
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    semantic::SpecHash,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
};

/// Exact equality canonical codec version.
pub const EQUALITY_CANONICAL_VERSION: u32 = 1;
/// Equality event semantics version.
pub const EQUALITY_SEMANTICS_VERSION: u32 = 1;
/// Shared production rewrite registry/validator version.
pub const EQUALITY_VALIDATOR_VERSION: u32 = 1;
/// Canonical proof-path codec version.
pub const EQUALITY_PROOF_PATH_VERSION: u32 = 1;
/// Domain separator for equality exact-state hashes.
pub const EQUALITY_HASH_DOMAIN: &[u8] = b"agentir.equality.exact.v1\0";
/// Domain separator for trusted proof-edge digests.
pub const EQUALITY_EDGE_DIGEST_DOMAIN: &[u8] = b"agentir.equality.proof.edge.v1\0";
/// Domain separator for canonical proof-path digests.
pub const EQUALITY_PATH_DIGEST_DOMAIN: &[u8] = b"agentir.equality.proof.path.v1\0";

/// SHA-256 identity of one canonical equality-space state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EqualityHash(String);

impl EqualityHash {
    /// Creates an equality hash from a lowercase hexadecimal digest.
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

impl fmt::Display for EqualityHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Equality-local monotonic allocator isolated from candidate allocation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityAllocator {
    space: u64,
    revision: u64,
    node: u64,
    edge: u64,
}

impl EqualityAllocator {
    fn space(&mut self) -> EqualitySpaceId {
        self.space = self.space.saturating_add(1);
        EqualitySpaceId::new(format!("eqs{}", self.space))
    }

    fn revision(&mut self) -> EqualityRevisionId {
        self.revision = self.revision.saturating_add(1);
        EqualityRevisionId::new(format!("er{}", self.revision))
    }

    fn node(&mut self) -> EqualityNodeId {
        self.node = self.node.saturating_add(1);
        EqualityNodeId::new(format!("en{}", self.node))
    }

    fn edge(&mut self) -> EqualityEdgeId {
        self.edge = self.edge.saturating_add(1);
        EqualityEdgeId::new(format!("ee{}", self.edge))
    }
}

/// Immutable exact candidate anchor for one equality space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityAnchor {
    /// Frozen SpecIR revision.
    pub spec_revision: RevisionId,
    /// Fully proved exact candidate branch.
    pub candidate: CandidateId,
    /// Explicit immutable candidate revision.
    pub candidate_revision: CandidateRevisionId,
    /// Candidate exact-state hash at creation.
    pub candidate_hash: crate::candidate::CandidateHash,
    /// Root semantic implementation hash.
    pub root_impl_hash: ImplHash,
}

/// One fully verified whole-program equality member.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqualityNode {
    /// Equality-local compiler-assigned node ID.
    pub id: EqualityNodeId,
    /// Fully verified typed implementation snapshot.
    pub impl_program: ImplProgram,
    /// Semantic hash used for hash-consing.
    pub impl_hash: ImplHash,
}

/// Stable proof identity independent of equality-local edge IDs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EqualityProofDescriptor {
    /// Semantic implementation before the rewrite.
    pub before_impl_hash: ImplHash,
    /// Semantic implementation after the rewrite.
    pub after_impl_hash: ImplHash,
    /// Stable compiler-owned production rule ID.
    pub rule: String,
    /// Structural operation locator in the source program.
    pub target: RewriteTargetLocator,
    /// Exact production side conditions.
    pub side_conditions: Vec<String>,
    /// ImplIR semantics contract.
    pub impl_semantics_version: u32,
    /// Equality event semantics contract.
    pub equality_semantics_version: u32,
    /// Production registry/validator contract.
    pub validator_version: u32,
}

/// One compiler-owned trusted equality proof edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityProofEdge {
    /// Equality-local edge ID.
    pub id: EqualityEdgeId,
    /// Source semantic node.
    pub source: EqualityNodeId,
    /// Target semantic node.
    pub target: EqualityNodeId,
    /// Stable proof descriptor.
    pub descriptor: EqualityProofDescriptor,
    /// Domain-separated digest of the descriptor.
    pub proof_digest: String,
}

/// Deterministic pending expansion unit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EqualityWorkItem {
    /// Equality node whose complete production match set remains to be expanded.
    pub node: EqualityNodeId,
}

/// Bounded equality expansion state. No status represents refutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqualityStatus {
    /// Newly created space with pending canonical work.
    Open,
    /// Current registry produced no remaining new work.
    FixedPoint,
    /// Explicit caller fuel ended while deterministic work remains.
    FuelExhausted,
}

/// Trusted equality membership used by candidate hash v3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityMembershipProof {
    /// Equality space proving membership.
    pub equality_space: EqualitySpaceId,
    /// Immutable equality revision used by the check.
    pub equality_revision: EqualityRevisionId,
    /// Exact equality state hash.
    pub equality_hash: EqualityHash,
    /// Root semantic implementation hash.
    pub root_impl_hash: ImplHash,
    /// Selected member semantic implementation hash.
    pub target_impl_hash: ImplHash,
    /// Canonical trusted path digest.
    pub path_digest: String,
    /// Ordered trusted edge IDs retained for audit and replay.
    pub edges: Vec<EqualityEdgeId>,
    /// Candidate obligation discharged by this proof.
    pub obligation: CandidateObligationId,
    /// Proposal attached to the obligation.
    pub proposal: ProposalId,
    /// Correctness EvidenceIR record.
    pub evidence: EvidenceId,
}

/// Provenance for explicit equality-node materialization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityMaterializationRecord {
    /// Equality space containing the selected node.
    pub equality_space: EqualitySpaceId,
    /// Immutable equality revision used for selection.
    pub equality_revision: EqualityRevisionId,
    /// Exact equality state hash.
    pub equality_hash: EqualityHash,
    /// Selected semantic node.
    pub target_node: EqualityNodeId,
    /// Canonical trusted path digest.
    pub path_digest: String,
    /// Anchor candidate revision from which the fork was built.
    pub anchor_candidate: CandidateId,
    /// Anchor candidate revision.
    pub anchor_candidate_revision: CandidateRevisionId,
    /// Newly materialized candidate branch.
    pub materialized_candidate: CandidateId,
    /// Terminal exact materialized revision.
    pub materialized_revision: CandidateRevisionId,
}

/// One immutable equality revision snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqualityRevision {
    /// Equality revision ID.
    pub id: EqualityRevisionId,
    /// Immutable parent equality revisions.
    pub parents: Vec<EqualityRevisionId>,
    /// Hash-consed semantic nodes.
    pub nodes: BTreeMap<EqualityNodeId, EqualityNode>,
    /// Trusted proof edges, including alternative paths.
    pub edges: BTreeMap<EqualityEdgeId, EqualityProofEdge>,
    /// Canonically ordered pending node worklist.
    pub worklist: Vec<EqualityWorkItem>,
    /// Bounded expansion status.
    pub status: EqualityStatus,
    /// Exact state hash independent of revision history and resource policy.
    pub equality_hash: EqualityHash,
    /// Candidate proof-debt discharges linked to this state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub membership_proofs: Vec<EqualityMembershipProof>,
    /// Explicit candidate materializations linked to this state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materializations: Vec<EqualityMaterializationRecord>,
}

/// Persistent exact equality space anchored to one frozen specification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqualitySpace {
    /// Compiler-assigned space ID.
    pub id: EqualitySpaceId,
    /// Immutable exact candidate anchor.
    pub anchor: EqualityAnchor,
    /// Immutable frozen SpecIR semantic hash.
    pub spec_hash: SpecHash,
    /// Root equality node.
    pub root_node: EqualityNodeId,
    /// Current immutable equality revision.
    pub head: EqualityRevisionId,
    /// Immutable equality revision DAG.
    pub revisions: BTreeMap<EqualityRevisionId, EqualityRevision>,
    /// Per-space revision/node/edge allocator.
    pub allocator: EqualityAllocator,
}

/// Summary returned by bounded expansion and saturation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityExpansionResult {
    /// Equality space expanded.
    pub equality_space: EqualitySpaceId,
    /// Newly published immutable revision.
    pub equality_revision: EqualityRevisionId,
    /// Exact resulting state hash.
    pub equality_hash: EqualityHash,
    /// Resulting bounded status.
    pub status: EqualityStatus,
    /// Work items consumed by this request.
    pub work_items_processed: u64,
    /// Newly allocated semantic nodes.
    pub new_nodes: u64,
    /// Matches merged into existing semantic nodes.
    pub merged_nodes: u64,
    /// Newly allocated trusted edges.
    pub new_edges: u64,
    /// Total semantic node count.
    pub node_count: usize,
    /// Total trusted edge count.
    pub edge_count: usize,
    /// Remaining deterministic work items.
    pub remaining_work: usize,
}

/// Deterministic trusted explanation from the equality root to one member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityExplanation {
    /// Root equality node.
    pub root: EqualityNodeId,
    /// Selected target node.
    pub target: EqualityNodeId,
    /// Root semantic implementation hash.
    pub root_impl_hash: ImplHash,
    /// Target semantic implementation hash.
    pub target_impl_hash: ImplHash,
    /// Canonical shortest, lexicographically tied proof edges.
    pub edges: Vec<EqualityProofEdge>,
    /// Domain-separated canonical path digest.
    pub proof_digest: String,
    /// Proof-path codec version.
    pub proof_path_version: u32,
    /// Registry/validator version used by every edge.
    pub validator_version: u32,
}

/// Read-only equality state summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityQuery {
    /// Equality space.
    pub equality_space: EqualitySpaceId,
    /// Selected equality revision.
    pub equality_revision: EqualityRevisionId,
    /// Exact equality hash.
    pub equality_hash: EqualityHash,
    /// Immutable specification anchor.
    pub spec_hash: SpecHash,
    /// Root semantic implementation hash.
    pub root_impl_hash: ImplHash,
    /// Current status.
    pub status: EqualityStatus,
    /// Semantic node count.
    pub node_count: usize,
    /// Trusted edge count.
    pub edge_count: usize,
    /// Pending work item count.
    pub remaining_work: usize,
}

/// One deterministic applicable production match returned without mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityContinuationMatch {
    /// Expandable source node.
    pub source_node: EqualityNodeId,
    /// Production match descriptor.
    pub production: ProductionRewriteMatch,
    /// Required current implementation hash.
    pub expected_before_impl_hash: ImplHash,
}

/// Bounded deterministic equality continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityContinuation {
    /// Equality space.
    pub equality_space: EqualitySpaceId,
    /// Equality revision inspected.
    pub equality_revision: EqualityRevisionId,
    /// Required exact state hash.
    pub expected_equality_hash: EqualityHash,
    /// Pending nodes in canonical worklist order.
    pub expandable_nodes: Vec<EqualityNodeId>,
    /// Applicable compiler-owned matches for the next node only.
    pub matches: Vec<EqualityContinuationMatch>,
    /// Maximum caller fuel accepted by interactive policy.
    pub saturation_fuel_limit: u64,
}

/// Replayable Stage 2C optimization event in exact dependency order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EqualityEvent {
    /// Equality space creation from an exact candidate revision.
    Created {
        /// Candidate anchor.
        candidate: CandidateId,
        /// Explicit candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected new space.
        equality_space: EqualitySpaceId,
        /// Expected root revision.
        equality_revision: EqualityRevisionId,
        /// Expected exact state hash.
        equality_hash: EqualityHash,
    },
    /// Bounded expansion of canonical work items.
    Expanded {
        /// Equality space.
        equality_space: EqualitySpaceId,
        /// Explicit equality base.
        base_revision: EqualityRevisionId,
        /// Expected base hash.
        expected_equality_hash: EqualityHash,
        /// Explicit caller fuel.
        fuel: u64,
        /// Expected new revision.
        equality_revision: EqualityRevisionId,
        /// Expected new hash.
        equality_hash: EqualityHash,
    },
    /// Saturation using explicit bounded caller fuel.
    Saturated {
        /// Equality space.
        equality_space: EqualitySpaceId,
        /// Explicit equality base.
        base_revision: EqualityRevisionId,
        /// Expected base hash.
        expected_equality_hash: EqualityHash,
        /// Explicit caller fuel.
        fuel: u64,
        /// Expected new revision.
        equality_revision: EqualityRevisionId,
        /// Expected new hash.
        equality_hash: EqualityHash,
    },
    /// Equality-backed candidate debt discharge.
    CandidateDischarged {
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit candidate base.
        base_candidate_revision: CandidateRevisionId,
        /// Proposal/obligation selected by reference only.
        proposal: ProposalId,
        /// Equality space.
        equality_space: EqualitySpaceId,
        /// Immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Expected equality hash.
        equality_hash: EqualityHash,
        /// Selected target node.
        target_node: EqualityNodeId,
        /// Expected child candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected candidate hash v3.
        candidate_hash: crate::candidate::CandidateHash,
    },
    /// Explicit selected-node materialization into a new candidate fork.
    Materialized {
        /// Equality space.
        equality_space: EqualitySpaceId,
        /// Immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Expected equality hash.
        equality_hash: EqualityHash,
        /// Explicit selected node.
        target_node: EqualityNodeId,
        /// Expected new candidate.
        candidate: CandidateId,
        /// Expected terminal candidate revision.
        candidate_revision: CandidateRevisionId,
        /// Expected candidate hash v3.
        candidate_hash: crate::candidate::CandidateHash,
    },
}

/// Equality event paired with its independent semantics version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionedEqualityEvent {
    /// Equality event compiler/replay semantics.
    pub semantics_version: u32,
    /// Number of ordinary candidate events that must replay before this event.
    pub candidate_event_cursor: u64,
    /// Replayable Stage 2C operation.
    pub event: EqualityEvent,
}

/// Persistent workspace equality store and ordered Stage 2C dependency stream.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EqualityStore {
    /// Exact equality spaces by compiler-assigned ID.
    pub spaces: BTreeMap<EqualitySpaceId, EqualitySpace>,
    /// Workspace-level equality-space allocator.
    pub allocator: EqualityAllocator,
    /// Ordered interleaved equality/candidate Stage 2C events.
    pub events: Vec<VersionedEqualityEvent>,
}

#[derive(Serialize)]
struct EqualityHashModel<'a> {
    codec: &'static str,
    version: u32,
    equality_semantics_version: u32,
    validator_version: u32,
    equality_space: &'a EqualitySpaceId,
    anchor: &'a EqualityAnchor,
    spec_hash: &'a SpecHash,
    root_node: &'a EqualityNodeId,
    nodes: &'a BTreeMap<EqualityNodeId, EqualityNode>,
    edges: &'a BTreeMap<EqualityEdgeId, EqualityProofEdge>,
    worklist: &'a [EqualityWorkItem],
    status: EqualityStatus,
    membership_proofs: &'a [EqualityMembershipProof],
    materializations: &'a [EqualityMaterializationRecord],
}

fn equality_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut input = Vec::with_capacity(domain.len().saturating_add(bytes.len()));
    input.extend_from_slice(domain);
    input.extend_from_slice(bytes);
    digest(&input)
}

fn edge_digest(descriptor: &EqualityProofDescriptor) -> AgentResult<String> {
    let bytes = serde_json::to_vec(descriptor).map_err(|error| {
        equality_error(
            ErrorCode::CanonicalizationFailed,
            format!("equality edge serialization failed: {error}"),
        )
    })?;
    Ok(domain_digest(EQUALITY_EDGE_DIGEST_DOMAIN, &bytes))
}

fn equality_hash_with_limit(
    space: &EqualitySpace,
    revision: &EqualityRevision,
    limits: &ResourceLimits,
) -> AgentResult<EqualityHash> {
    let model = EqualityHashModel {
        codec: "agentir.equality.exact",
        version: EQUALITY_CANONICAL_VERSION,
        equality_semantics_version: EQUALITY_SEMANTICS_VERSION,
        validator_version: EQUALITY_VALIDATOR_VERSION,
        equality_space: &space.id,
        anchor: &space.anchor,
        spec_hash: &space.spec_hash,
        root_node: &space.root_node,
        nodes: &revision.nodes,
        edges: &revision.edges,
        worklist: &revision.worklist,
        status: revision.status,
        membership_proofs: &revision.membership_proofs,
        materializations: &revision.materializations,
    };
    let bytes = serde_json::to_vec(&model).map_err(|error| {
        equality_error(
            ErrorCode::CanonicalizationFailed,
            format!("equality exact-state serialization failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::EqualityCanonicalBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "equality exact-state canonicalization",
    )?;
    Ok(EqualityHash(domain_digest(EQUALITY_HASH_DOMAIN, &bytes)))
}

fn total_equality_revisions(spaces: &BTreeMap<EqualitySpaceId, EqualitySpace>) -> u64 {
    spaces.values().fold(0_u64, |total, space| {
        total.saturating_add(u64::try_from(space.revisions.len()).unwrap_or(u64::MAX))
    })
}

fn ensure_store_budgets(store: &EqualityStore, limits: &ResourceLimits) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::EqualitySpacesPerWorkspace,
        u64::try_from(store.spaces.len()).unwrap_or(u64::MAX),
        "equality space store",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::EqualityRevisionsPerWorkspace,
        total_equality_revisions(&store.spaces),
        "equality revision store",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::EqualityEvents,
        u64::try_from(store.events.len()).unwrap_or(u64::MAX),
        "Stage 2C optimization event stream",
    )?;
    let bytes = serde_json::to_vec(store).map_err(|error| {
        equality_error(
            ErrorCode::PersistenceFormat,
            format!("equality archive preflight encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::EqualityArchiveBytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "equality archive state",
    )
}

fn ensure_revision_budgets(
    revision: &EqualityRevision,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    for (resource, actual) in [
        (
            ResourceKind::EqualityNodesPerSpace,
            u64::try_from(revision.nodes.len()).unwrap_or(u64::MAX),
        ),
        (
            ResourceKind::EqualityEdgesPerSpace,
            u64::try_from(revision.edges.len()).unwrap_or(u64::MAX),
        ),
        (
            ResourceKind::EqualityPendingWorkItems,
            u64::try_from(revision.worklist.len()).unwrap_or(u64::MAX),
        ),
    ] {
        BudgetCheck::against(limits, resource, actual, "equality revision state")?;
    }
    Ok(())
}

impl EqualityStore {
    /// Returns one persistent equality space.
    pub fn space(&self, id: &EqualitySpaceId) -> AgentResult<&EqualitySpace> {
        self.spaces.get(id).ok_or_else(|| {
            equality_error(
                ErrorCode::EqualitySpaceNotFound,
                format!("equality space `{id}` does not exist"),
            )
        })
    }

    /// Returns one immutable equality revision.
    pub fn revision(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
    ) -> AgentResult<&EqualityRevision> {
        self.space(space)?.revisions.get(revision).ok_or_else(|| {
            equality_error(
                ErrorCode::EqualityRevisionNotFound,
                format!("equality revision `{revision}` does not exist"),
            )
        })
    }

    /// Creates a root-only exact equality space from one fully proved candidate revision.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        forest: &CandidateForest,
        candidate_id: &CandidateId,
        candidate_revision: &CandidateRevisionId,
        source: &Program,
        spec_revision: &RevisionId,
        spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<EqualityQuery> {
        BudgetCheck::against(
            limits,
            ResourceKind::EqualitySpacesPerWorkspace,
            u64::try_from(self.spaces.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "equality creation before persistent ID allocation",
        )?;
        let candidate = forest.candidate(candidate_id)?;
        let revision = forest.revision(candidate_id, candidate_revision)?;
        if candidate.spec_hash != *spec_hash || candidate.spec_revision != *spec_revision {
            return Err(equality_error(
                ErrorCode::SpecHashMismatch,
                "equality anchor differs from the frozen SpecIR contract",
            ));
        }
        forest.check(candidate_id, candidate_revision, source, limits)?;
        if revision.guarded_fallback.is_some()
            || revision.equivalence.status == EquivalenceStatus::Guarded
        {
            return Err(equality_error(
                ErrorCode::EqualityGuardedAnchorUnsupported,
                "guarded primary revision cannot become an unconditional equality root",
            ));
        }
        if revision.equivalence.status != EquivalenceStatus::Proved
            || matches!(
                revision.state,
                CandidateState::Speculative | CandidateState::Rejected | CandidateState::Guarded
            )
            || revision
                .proof_debt
                .iter()
                .any(|debt| debt.status != crate::candidate::ProofDebtStatus::Proved)
        {
            return Err(equality_error(
                ErrorCode::EqualityAnchorUnproved,
                "equality root requires a fully proved exact candidate revision",
            ));
        }
        verify_impl(&revision.impl_program, source, limits)?;
        let root_hash = impl_hash(&revision.impl_program)?;
        if root_hash != revision.impl_hash {
            return Err(equality_error(
                ErrorCode::EqualityAnchorUnproved,
                "equality anchor terminal impl_hash is invalid",
            ));
        }

        let mut staged = self.clone();
        let space_id = staged.allocator.space();
        let mut local_allocator = EqualityAllocator::default();
        let equality_revision = local_allocator.revision();
        let root_node = local_allocator.node();
        let node = EqualityNode {
            id: root_node.clone(),
            impl_program: revision.impl_program.clone(),
            impl_hash: root_hash.clone(),
        };
        let mut equality_revision_data = EqualityRevision {
            id: equality_revision.clone(),
            parents: Vec::new(),
            nodes: BTreeMap::from([(root_node.clone(), node)]),
            edges: BTreeMap::new(),
            worklist: vec![EqualityWorkItem {
                node: root_node.clone(),
            }],
            status: EqualityStatus::Open,
            equality_hash: EqualityHash::new("pending"),
            membership_proofs: Vec::new(),
            materializations: Vec::new(),
        };
        let mut space = EqualitySpace {
            id: space_id.clone(),
            anchor: EqualityAnchor {
                spec_revision: spec_revision.clone(),
                candidate: candidate_id.clone(),
                candidate_revision: candidate_revision.clone(),
                candidate_hash: revision.candidate_hash.clone(),
                root_impl_hash: root_hash,
            },
            spec_hash: spec_hash.clone(),
            root_node,
            head: equality_revision.clone(),
            revisions: BTreeMap::new(),
            allocator: local_allocator,
        };
        equality_revision_data.equality_hash =
            equality_hash_with_limit(&space, &equality_revision_data, limits)?;
        let equality_hash = equality_revision_data.equality_hash.clone();
        space
            .revisions
            .insert(equality_revision.clone(), equality_revision_data);
        staged.spaces.insert(space_id.clone(), space);
        staged.events.push(VersionedEqualityEvent {
            semantics_version: EQUALITY_SEMANTICS_VERSION,
            candidate_event_cursor: u64::try_from(forest.events.len()).unwrap_or(u64::MAX),
            event: EqualityEvent::Created {
                candidate: candidate_id.clone(),
                candidate_revision: candidate_revision.clone(),
                equality_space: space_id.clone(),
                equality_revision: equality_revision.clone(),
                equality_hash: equality_hash.clone(),
            },
        });
        ensure_store_budgets(&staged, limits)?;
        *self = staged;
        Ok(EqualityQuery {
            equality_space: space_id,
            equality_revision,
            equality_hash,
            spec_hash: spec_hash.clone(),
            root_impl_hash: revision.impl_hash.clone(),
            status: EqualityStatus::Open,
            node_count: 1,
            edge_count: 0,
            remaining_work: 1,
        })
    }

    /// Reads one equality revision without mutating persistent state.
    pub fn query(
        &self,
        space_id: &EqualitySpaceId,
        revision_id: &EqualityRevisionId,
    ) -> AgentResult<EqualityQuery> {
        let space = self.space(space_id)?;
        let revision = self.revision(space_id, revision_id)?;
        Ok(EqualityQuery {
            equality_space: space_id.clone(),
            equality_revision: revision_id.clone(),
            equality_hash: revision.equality_hash.clone(),
            spec_hash: space.spec_hash.clone(),
            root_impl_hash: space.anchor.root_impl_hash.clone(),
            status: revision.status,
            node_count: revision.nodes.len(),
            edge_count: revision.edges.len(),
            remaining_work: revision.worklist.len(),
        })
    }
}

#[derive(Default)]
struct ExpansionCounts {
    processed: u64,
    new_nodes: u64,
    merged_nodes: u64,
    new_edges: u64,
}

fn expand_revision(
    space: &mut EqualitySpace,
    base_revision: &EqualityRevisionId,
    expected_hash: &EqualityHash,
    fuel: u64,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<(EqualityRevision, ExpansionCounts)> {
    if space.head != *base_revision {
        return Err(equality_error(
            ErrorCode::StaleEqualityBase,
            "equality mutation base is stale",
        )
        .with_detail("current_head", space.head.to_string())
        .with_detail("base_revision", base_revision.to_string()));
    }
    if fuel == 0 {
        return Err(equality_error(
            ErrorCode::InvalidRequest,
            "equality expansion fuel must be positive",
        ));
    }
    BudgetCheck::against(
        limits,
        ResourceKind::EqualityExpansionStepsPerRequest,
        fuel,
        "equality expansion before graph clone",
    )?;
    let base = space.revisions.get(base_revision).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityRevisionNotFound,
            format!("equality revision `{base_revision}` does not exist"),
        )
    })?;
    if &base.equality_hash != expected_hash {
        return Err(equality_error(
            ErrorCode::EqualityHashMismatch,
            "expected equality hash is stale",
        )
        .with_types(expected_hash.to_string(), base.equality_hash.to_string()));
    }
    let revision_id = space.allocator.revision();
    let mut next = base.clone();
    next.id = revision_id;
    next.parents = vec![base_revision.clone()];
    let mut counts = ExpansionCounts::default();
    while counts.processed < fuel {
        let Some(work) = next.worklist.first().cloned() else {
            break;
        };
        next.worklist.remove(0);
        let node = next.nodes.get(&work.node).cloned().ok_or_else(|| {
            equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality worklist references a missing node",
            )
        })?;
        let matches = production_rewrite_matches(&node.impl_program, limits)?;
        for production in matches {
            let mut transformed = node.impl_program.clone();
            let target = resolve_rewrite_locator(&transformed, &production.locator)?;
            if target != production.target {
                return Err(equality_error(
                    ErrorCode::EqualityProofInvalid,
                    "production target and stable locator disagree",
                ));
            }
            let side_conditions = apply_known_rewrite(&mut transformed, &production.rule, &target)?;
            if side_conditions != production.side_conditions {
                return Err(equality_error(
                    ErrorCode::EqualityProofInvalid,
                    "production matcher and transform side conditions disagree",
                ));
            }
            verify_impl(&transformed, source, limits)?;
            let target_hash = impl_hash(&transformed)?;
            if target_hash == node.impl_hash {
                continue;
            }
            let existing_target = next
                .nodes
                .values()
                .find(|candidate| candidate.impl_hash == target_hash)
                .map(|candidate| candidate.id.clone());
            let target_node = if let Some(existing) = existing_target {
                counts.merged_nodes = counts.merged_nodes.saturating_add(1);
                existing
            } else {
                BudgetCheck::against(
                    limits,
                    ResourceKind::EqualityNodesPerSpace,
                    u64::try_from(next.nodes.len())
                        .unwrap_or(u64::MAX)
                        .saturating_add(1),
                    "equality node allocation",
                )?;
                let target_node = space.allocator.node();
                next.nodes.insert(
                    target_node.clone(),
                    EqualityNode {
                        id: target_node.clone(),
                        impl_program: transformed,
                        impl_hash: target_hash.clone(),
                    },
                );
                next.worklist.push(EqualityWorkItem {
                    node: target_node.clone(),
                });
                counts.new_nodes = counts.new_nodes.saturating_add(1);
                target_node
            };
            let descriptor = EqualityProofDescriptor {
                before_impl_hash: node.impl_hash.clone(),
                after_impl_hash: target_hash,
                rule: production.rule,
                target: production.locator,
                side_conditions,
                impl_semantics_version: IMPL_SEMANTICS_VERSION,
                equality_semantics_version: EQUALITY_SEMANTICS_VERSION,
                validator_version: EQUALITY_VALIDATOR_VERSION,
            };
            let proof_digest = edge_digest(&descriptor)?;
            let duplicate = next.edges.values().any(|edge| {
                edge.source == node.id
                    && edge.target == target_node
                    && edge.descriptor == descriptor
                    && edge.proof_digest == proof_digest
            });
            if !duplicate {
                BudgetCheck::against(
                    limits,
                    ResourceKind::EqualityEdgesPerSpace,
                    u64::try_from(next.edges.len())
                        .unwrap_or(u64::MAX)
                        .saturating_add(1),
                    "equality proof-edge allocation",
                )?;
                let edge = space.allocator.edge();
                next.edges.insert(
                    edge.clone(),
                    EqualityProofEdge {
                        id: edge,
                        source: node.id.clone(),
                        target: target_node,
                        descriptor,
                        proof_digest,
                    },
                );
                counts.new_edges = counts.new_edges.saturating_add(1);
            }
        }
        next.worklist.sort();
        next.worklist.dedup();
        counts.processed = counts.processed.saturating_add(1);
    }
    next.status = if next.worklist.is_empty() {
        EqualityStatus::FixedPoint
    } else {
        EqualityStatus::FuelExhausted
    };
    ensure_revision_budgets(&next, limits)?;
    next.equality_hash = equality_hash_with_limit(space, &next, limits)?;
    Ok((next, counts))
}

impl EqualityStore {
    #[allow(clippy::too_many_arguments)]
    fn publish_expansion(
        &mut self,
        space_id: &EqualitySpaceId,
        base_revision: &EqualityRevisionId,
        expected_hash: &EqualityHash,
        fuel: u64,
        source: &Program,
        limits: &ResourceLimits,
        saturate: bool,
        candidate_event_cursor: u64,
    ) -> AgentResult<EqualityExpansionResult> {
        if saturate {
            BudgetCheck::against(
                limits,
                ResourceKind::EqualitySaturationFuel,
                fuel,
                "equality saturation caller fuel",
            )?;
        }
        let mut staged = self.clone();
        let space = staged.spaces.get_mut(space_id).ok_or_else(|| {
            equality_error(
                ErrorCode::EqualitySpaceNotFound,
                format!("equality space `{space_id}` does not exist"),
            )
        })?;
        let (next, counts) =
            expand_revision(space, base_revision, expected_hash, fuel, source, limits)?;
        let revision_id = next.id.clone();
        let equality_hash = next.equality_hash.clone();
        let status = next.status;
        let node_count = next.nodes.len();
        let edge_count = next.edges.len();
        let remaining_work = next.worklist.len();
        space.revisions.insert(revision_id.clone(), next);
        space.head = revision_id.clone();
        staged.events.push(VersionedEqualityEvent {
            semantics_version: EQUALITY_SEMANTICS_VERSION,
            candidate_event_cursor,
            event: if saturate {
                EqualityEvent::Saturated {
                    equality_space: space_id.clone(),
                    base_revision: base_revision.clone(),
                    expected_equality_hash: expected_hash.clone(),
                    fuel,
                    equality_revision: revision_id.clone(),
                    equality_hash: equality_hash.clone(),
                }
            } else {
                EqualityEvent::Expanded {
                    equality_space: space_id.clone(),
                    base_revision: base_revision.clone(),
                    expected_equality_hash: expected_hash.clone(),
                    fuel,
                    equality_revision: revision_id.clone(),
                    equality_hash: equality_hash.clone(),
                }
            },
        });
        ensure_store_budgets(&staged, limits)?;
        *self = staged;
        Ok(EqualityExpansionResult {
            equality_space: space_id.clone(),
            equality_revision: revision_id,
            equality_hash,
            status,
            work_items_processed: counts.processed,
            new_nodes: counts.new_nodes,
            merged_nodes: counts.merged_nodes,
            new_edges: counts.new_edges,
            node_count,
            edge_count,
            remaining_work,
        })
    }

    /// Expands a bounded number of canonical node work items atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn expand(
        &mut self,
        space: &EqualitySpaceId,
        base_revision: &EqualityRevisionId,
        expected_hash: &EqualityHash,
        fuel: u64,
        source: &Program,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
    ) -> AgentResult<EqualityExpansionResult> {
        self.publish_expansion(
            space,
            base_revision,
            expected_hash,
            fuel,
            source,
            limits,
            false,
            candidate_event_cursor,
        )
    }

    /// Saturates deterministically to fixpoint or explicit caller fuel.
    #[allow(clippy::too_many_arguments)]
    pub fn saturate(
        &mut self,
        space: &EqualitySpaceId,
        base_revision: &EqualityRevisionId,
        expected_hash: &EqualityHash,
        fuel: u64,
        source: &Program,
        limits: &ResourceLimits,
        candidate_event_cursor: u64,
    ) -> AgentResult<EqualityExpansionResult> {
        self.publish_expansion(
            space,
            base_revision,
            expected_hash,
            fuel,
            source,
            limits,
            true,
            candidate_event_cursor,
        )
    }

    /// Returns bounded deterministic next work without changing state.
    pub fn continuation(
        &self,
        space_id: &EqualitySpaceId,
        revision_id: &EqualityRevisionId,
        limits: &ResourceLimits,
    ) -> AgentResult<EqualityContinuation> {
        let revision = self.revision(space_id, revision_id)?;
        let expandable_nodes = revision
            .worklist
            .iter()
            .map(|work| work.node.clone())
            .collect::<Vec<_>>();
        let matches = revision
            .worklist
            .first()
            .map(|work| {
                let node = revision.nodes.get(&work.node).ok_or_else(|| {
                    equality_error(
                        ErrorCode::EqualityProofInvalid,
                        "equality worklist references a missing node",
                    )
                })?;
                production_rewrite_matches(&node.impl_program, limits).map(|matches| {
                    matches
                        .into_iter()
                        .map(|production| EqualityContinuationMatch {
                            source_node: node.id.clone(),
                            expected_before_impl_hash: node.impl_hash.clone(),
                            production,
                        })
                        .collect()
                })
            })
            .transpose()?
            .unwrap_or_default();
        Ok(EqualityContinuation {
            equality_space: space_id.clone(),
            equality_revision: revision_id.clone(),
            expected_equality_hash: revision.equality_hash.clone(),
            expandable_nodes,
            matches,
            saturation_fuel_limit: limits.equality_saturation_fuel,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PathKey {
    depth: usize,
    descriptors: Vec<EqualityProofDescriptor>,
    edge_ids: Vec<EqualityEdgeId>,
    node: EqualityNodeId,
}

#[derive(Serialize)]
struct PathDigest<'a> {
    version: u32,
    root_impl_hash: &'a ImplHash,
    target_impl_hash: &'a ImplHash,
    edge_digests: Vec<&'a str>,
}

fn explanation_for(
    space: &EqualitySpace,
    revision: &EqualityRevision,
    target: &EqualityNodeId,
    limits: &ResourceLimits,
) -> AgentResult<EqualityExplanation> {
    let root = revision.nodes.get(&space.root_node).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality root node is missing",
        )
    })?;
    let target_node = revision.nodes.get(target).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityNodeNotFound,
            format!("equality node `{target}` does not exist"),
        )
    })?;
    let mut frontier = BTreeSet::from([PathKey {
        depth: 0,
        descriptors: Vec::new(),
        edge_ids: Vec::new(),
        node: space.root_node.clone(),
    }]);
    let mut best_depth = BTreeMap::<EqualityNodeId, usize>::new();
    let selected = loop {
        let Some(path) = frontier.pop_first() else {
            return Err(equality_error(
                ErrorCode::EqualityPathNotFound,
                "selected equality node is disconnected from the root",
            ));
        };
        let depth = path.edge_ids.len();
        BudgetCheck::against(
            limits,
            ResourceKind::EqualityExplanationDepth,
            u64::try_from(depth).unwrap_or(u64::MAX),
            "canonical equality explanation",
        )?;
        if path.node == *target {
            break path;
        }
        if best_depth
            .get(&path.node)
            .is_some_and(|known| *known < depth)
        {
            continue;
        }
        best_depth.insert(path.node.clone(), depth);
        let mut outgoing = revision
            .edges
            .values()
            .filter(|edge| edge.source == path.node)
            .cloned()
            .collect::<Vec<_>>();
        outgoing.sort_by(|left, right| {
            (&left.descriptor, &left.id).cmp(&(&right.descriptor, &right.id))
        });
        for edge in outgoing {
            if path.edge_ids.contains(&edge.id) {
                continue;
            }
            let mut descriptors = path.descriptors.clone();
            descriptors.push(edge.descriptor.clone());
            let mut edge_ids = path.edge_ids.clone();
            edge_ids.push(edge.id.clone());
            BudgetCheck::against(
                limits,
                ResourceKind::EqualityProofPathEdges,
                u64::try_from(edge_ids.len()).unwrap_or(u64::MAX),
                "canonical equality proof path",
            )?;
            frontier.insert(PathKey {
                depth: edge_ids.len(),
                descriptors,
                edge_ids,
                node: edge.target,
            });
        }
    };
    let edges = selected
        .edge_ids
        .iter()
        .map(|edge| {
            revision.edges.get(edge).cloned().ok_or_else(|| {
                equality_error(
                    ErrorCode::EqualityProofInvalid,
                    "canonical explanation references a missing edge",
                )
            })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    let bytes = serde_json::to_vec(&PathDigest {
        version: EQUALITY_PROOF_PATH_VERSION,
        root_impl_hash: &root.impl_hash,
        target_impl_hash: &target_node.impl_hash,
        edge_digests: edges
            .iter()
            .map(|edge| edge.proof_digest.as_str())
            .collect(),
    })
    .map_err(|error| {
        equality_error(
            ErrorCode::CanonicalizationFailed,
            format!("equality proof path serialization failed: {error}"),
        )
    })?;
    Ok(EqualityExplanation {
        root: root.id.clone(),
        target: target_node.id.clone(),
        root_impl_hash: root.impl_hash.clone(),
        target_impl_hash: target_node.impl_hash.clone(),
        edges,
        proof_digest: domain_digest(EQUALITY_PATH_DIGEST_DOMAIN, &bytes),
        proof_path_version: EQUALITY_PROOF_PATH_VERSION,
        validator_version: EQUALITY_VALIDATOR_VERSION,
    })
}

impl EqualityStore {
    /// Builds and verifies the canonical trusted root-to-node explanation.
    pub fn explain(
        &self,
        space_id: &EqualitySpaceId,
        revision_id: &EqualityRevisionId,
        target: &EqualityNodeId,
        source: &Program,
        limits: &ResourceLimits,
    ) -> AgentResult<EqualityExplanation> {
        let space = self.space(space_id)?;
        let revision = self.revision(space_id, revision_id)?;
        verify_revision(space, revision, source, limits)?;
        explanation_for(space, revision, target, limits)
    }

    /// Returns the selected fully verified equality member program.
    pub fn node_program(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
        node: &EqualityNodeId,
    ) -> AgentResult<&ImplProgram> {
        self.revision(space, revision)?
            .nodes
            .get(node)
            .map(|node| &node.impl_program)
            .ok_or_else(|| {
                equality_error(
                    ErrorCode::EqualityNodeNotFound,
                    format!("equality node `{node}` does not exist"),
                )
            })
    }
}

/// Atomic result of equality-backed proof-debt discharge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityDischargeResult {
    /// New candidate revision containing candidate hash v3 proof linkage.
    pub candidate_revision: CandidateRevisionId,
    /// Exact candidate hash v3.
    pub candidate_hash: crate::candidate::CandidateHash,
    /// Trusted equality membership proof recorded by the core.
    pub membership: EqualityMembershipProof,
}

/// Atomic result of selected equality-node materialization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualityMaterializationResult {
    /// Newly forked candidate branch.
    pub candidate: CandidateId,
    /// Terminal exact candidate revision.
    pub candidate_revision: CandidateRevisionId,
    /// Exact candidate hash v3.
    pub candidate_hash: crate::candidate::CandidateHash,
    /// Explicit materialization provenance.
    pub materialization: EqualityMaterializationRecord,
}

impl EqualityStore {
    /// Discharges the next matching proof-debt item using a compiler-rebuilt equality path.
    #[allow(clippy::too_many_arguments)]
    pub fn candidate_discharge(
        &mut self,
        forest: &mut CandidateForest,
        candidate_id: &CandidateId,
        base_revision_id: &CandidateRevisionId,
        proposal_id: &ProposalId,
        space_id: &EqualitySpaceId,
        equality_revision_id: &EqualityRevisionId,
        expected_equality_hash: &EqualityHash,
        target_node: &EqualityNodeId,
        source: &Program,
        spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<EqualityDischargeResult> {
        let candidate = forest.candidate(candidate_id)?;
        let base = forest.revision(candidate_id, base_revision_id)?;
        if candidate.head != *base_revision_id {
            return Err(equality_error(
                ErrorCode::CandidateRevisionNotFound,
                "candidate equality-check base is stale",
            ));
        }
        forest.check(candidate_id, base_revision_id, source, limits)?;
        if candidate.spec_hash != *spec_hash {
            return Err(equality_error(
                ErrorCode::SpecHashMismatch,
                "candidate equality-check spec_hash differs from frozen SpecIR",
            ));
        }
        let debt_index = base
            .proof_debt
            .iter()
            .position(|debt| debt.proposal == *proposal_id)
            .ok_or_else(|| {
                equality_error(
                    ErrorCode::ProposalNotFound,
                    format!("proposal `{proposal_id}` is not in candidate proof debt"),
                )
            })?;
        let debt = &base.proof_debt[debt_index];
        if !matches!(
            debt.status,
            ProofDebtStatus::Open | ProofDebtStatus::Unsupported
        ) || base
            .proof_debt
            .iter()
            .take(debt_index)
            .any(|prior| prior.status != ProofDebtStatus::Proved)
        {
            return Err(equality_error(
                ErrorCode::CandidateHasProofDebt,
                "equality validation must discharge the next unresolved obligation in order",
            ));
        }
        let space = self.space(space_id)?;
        let equality_revision = self.revision(space_id, equality_revision_id)?;
        if &equality_revision.equality_hash != expected_equality_hash {
            return Err(equality_error(
                ErrorCode::EqualityHashMismatch,
                "candidate equality-check expected hash is stale",
            ));
        }
        if space.spec_hash != *spec_hash
            || space.anchor.root_impl_hash != debt.before_impl_hash
            || equality_revision
                .nodes
                .get(target_node)
                .is_none_or(|node| node.impl_hash != debt.after_impl_hash)
        {
            return Err(equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality root/target hashes do not match the selected proof obligation",
            ));
        }
        let anchor_revision =
            forest.revision(&space.anchor.candidate, &space.anchor.candidate_revision)?;
        forest.check(
            &space.anchor.candidate,
            &space.anchor.candidate_revision,
            source,
            limits,
        )?;
        if anchor_revision.candidate_hash != space.anchor.candidate_hash
            || anchor_revision.impl_hash != space.anchor.root_impl_hash
            || anchor_revision.equivalence.status != EquivalenceStatus::Proved
            || anchor_revision.guarded_fallback.is_some()
        {
            return Err(equality_error(
                ErrorCode::EqualityAnchorUnproved,
                "equality proof anchor is no longer a fully proved exact revision",
            ));
        }
        verify_revision(space, equality_revision, source, limits)?;
        let explanation = explanation_for(space, equality_revision, target_node, limits)?;

        let mut staged_forest = forest.clone();
        let mut staged_store = self.clone();
        let revision_id = staged_forest.allocator.revision();
        let evidence_id = staged_forest.allocator.evidence();
        let mut next = base.clone();
        next.id = revision_id.clone();
        next.parents = vec![base_revision_id.clone()];
        next.candidate_hash_version = EQUALITY_CANDIDATE_CANONICAL_VERSION;
        next.equivalence.candidate_revision = revision_id.clone();
        let (selected_id, selected_target, selected_before, selected_after) = {
            let selected = next
                .proof_debt
                .get_mut(debt_index)
                .expect("debt index was checked");
            selected.status = ProofDebtStatus::Proved;
            selected.evidence.push(evidence_id.clone());
            (
                selected.id.clone(),
                selected.target.clone(),
                selected.before_impl_hash.clone(),
                selected.after_impl_hash.clone(),
            )
        };
        next.evidence.push(evidence_id.clone());
        next.proof_chain.push(EquivalenceCertificate {
            rule: "equality_membership_v1".to_owned(),
            before_impl_hash: Some(selected_before),
            after_impl_hash: selected_after.clone(),
            targets: vec![selected_target],
            side_conditions: vec![
                "canonical trusted equality path verified".to_owned(),
                format!("path_digest == {}", explanation.proof_digest),
            ],
            impl_semantics_version: IMPL_SEMANTICS_VERSION,
            evidence: evidence_id.clone(),
        });
        let all_proved = next
            .proof_debt
            .iter()
            .all(|debt| debt.status == ProofDebtStatus::Proved);
        let frontier_hash = selected_after;
        next.proof_frontier = Some(ProofFrontier {
            candidate: candidate_id.clone(),
            candidate_revision: if all_proved {
                revision_id.clone()
            } else {
                staged_forest
                    .proposal(proposal_id)?
                    .accepted_candidate_revision
                    .clone()
            },
            terminal_proved_impl_hash: frontier_hash,
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
        let membership = EqualityMembershipProof {
            equality_space: space_id.clone(),
            equality_revision: equality_revision_id.clone(),
            equality_hash: expected_equality_hash.clone(),
            root_impl_hash: explanation.root_impl_hash.clone(),
            target_impl_hash: explanation.target_impl_hash.clone(),
            path_digest: explanation.proof_digest.clone(),
            edges: explanation
                .edges
                .iter()
                .map(|edge| edge.id.clone())
                .collect(),
            obligation: selected_id.clone(),
            proposal: proposal_id.clone(),
            evidence: evidence_id.clone(),
        };
        next.equality_proofs.push(membership.clone());
        let validation = TranslationValidationRecord {
            proposal: proposal_id.clone(),
            obligation: selected_id,
            candidate_revision: revision_id.clone(),
            validator_id: "agentir.equality_validator".to_owned(),
            validator_version: EQUALITY_VALIDATOR_VERSION,
            result: TranslationValidationResult::RecognizedKnownRewrite {
                rule: "equality_membership_v1".to_owned(),
                side_conditions: vec!["trusted root-to-node path".to_owned()],
            },
            evidence: Some(evidence_id.clone()),
        };
        next.translation_results.push(validation);
        staged_forest.evidence.insert(
            evidence_id.clone(),
            EvidenceRecord {
                id: evidence_id,
                class: EvidenceClass::Correctness,
                kind: EvidenceKind::EqualityMembershipProof,
                spec_hash: spec_hash.clone(),
                candidate: candidate_id.clone(),
                candidate_revision: revision_id.clone(),
                input_impl_hash: Some(explanation.root_impl_hash.clone()),
                output_impl_hash: explanation.target_impl_hash.clone(),
                method: "equality_membership_v1".to_owned(),
                parameters: BTreeMap::from([
                    ("equality_space".to_owned(), serde_json::json!(space_id)),
                    (
                        "equality_revision".to_owned(),
                        serde_json::json!(equality_revision_id),
                    ),
                    (
                        "equality_hash".to_owned(),
                        serde_json::json!(expected_equality_hash),
                    ),
                    (
                        "path_digest".to_owned(),
                        serde_json::json!(explanation.proof_digest),
                    ),
                ]),
                result: EvidenceResult::Passed,
                counterexample: None,
                provenance: EvidenceProvenance {
                    compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
                    candidate_semantics_version: EQUALITY_CANDIDATE_SEMANTICS_VERSION,
                    impl_semantics_version: IMPL_SEMANTICS_VERSION,
                },
            },
        );
        next.candidate_hash = crate::candidate::CandidateHash::new("pending");
        let candidate_snapshot = staged_forest.candidate(candidate_id)?.clone();
        next.candidate_hash = candidate_hash_with_limit(
            &staged_forest,
            &candidate_snapshot,
            &next,
            candidate_canonical_limit(&next, limits),
        )?;
        let candidate_hash = next.candidate_hash.clone();
        let candidate_mut = staged_forest
            .candidates
            .get_mut(candidate_id)
            .expect("candidate was checked");
        candidate_mut.revisions.insert(revision_id.clone(), next);
        candidate_mut.head = revision_id.clone();
        staged_store.events.push(VersionedEqualityEvent {
            semantics_version: EQUALITY_SEMANTICS_VERSION,
            candidate_event_cursor: u64::try_from(staged_forest.events.len()).unwrap_or(u64::MAX),
            event: EqualityEvent::CandidateDischarged {
                candidate: candidate_id.clone(),
                base_candidate_revision: base_revision_id.clone(),
                proposal: proposal_id.clone(),
                equality_space: space_id.clone(),
                equality_revision: equality_revision_id.clone(),
                equality_hash: expected_equality_hash.clone(),
                target_node: target_node.clone(),
                candidate_revision: revision_id.clone(),
                candidate_hash: candidate_hash.clone(),
            },
        });
        ensure_forest_budgets(&staged_forest, limits)?;
        ensure_store_budgets(&staged_store, limits)?;
        staged_forest.check(candidate_id, &revision_id, source, limits)?;
        *forest = staged_forest;
        *self = staged_store;
        Ok(EqualityDischargeResult {
            candidate_revision: revision_id,
            candidate_hash,
            membership,
        })
    }

    /// Atomically materializes one explicit equality node as a new exact candidate fork.
    #[allow(clippy::too_many_arguments)]
    pub fn materialize(
        &mut self,
        forest: &mut CandidateForest,
        space_id: &EqualitySpaceId,
        equality_revision_id: &EqualityRevisionId,
        expected_equality_hash: &EqualityHash,
        target_node: &EqualityNodeId,
        source: &Program,
        spec_hash: &SpecHash,
        limits: &ResourceLimits,
    ) -> AgentResult<EqualityMaterializationResult> {
        let space = self.space(space_id)?;
        let revision = self.revision(space_id, equality_revision_id)?;
        if revision.equality_hash != *expected_equality_hash {
            return Err(equality_error(
                ErrorCode::EqualityHashMismatch,
                "materialization expected equality hash is stale",
            ));
        }
        if space.spec_hash != *spec_hash {
            return Err(equality_error(
                ErrorCode::SpecHashMismatch,
                "materialization equality space has a different spec_hash",
            ));
        }
        let anchor_revision =
            forest.revision(&space.anchor.candidate, &space.anchor.candidate_revision)?;
        forest.check(
            &space.anchor.candidate,
            &space.anchor.candidate_revision,
            source,
            limits,
        )?;
        if anchor_revision.candidate_hash != space.anchor.candidate_hash
            || anchor_revision.impl_hash != space.anchor.root_impl_hash
            || anchor_revision.equivalence.status != EquivalenceStatus::Proved
            || anchor_revision.guarded_fallback.is_some()
        {
            return Err(equality_error(
                ErrorCode::EqualityAnchorUnproved,
                "equality materialization anchor is not fully proved exact",
            ));
        }
        verify_revision(space, revision, source, limits)?;
        let explanation = explanation_for(space, revision, target_node, limits)?;
        BudgetCheck::against(
            limits,
            ResourceKind::EqualityMaterializationSteps,
            u64::try_from(explanation.edges.len()).unwrap_or(u64::MAX),
            "equality materialization before candidate allocation",
        )?;
        let anchor = space.anchor.clone();

        let mut staged_forest = forest.clone();
        let mut staged_store = self.clone();
        let candidate_event_cursor = staged_forest.events.len();
        let fork = staged_forest.fork(
            &anchor.candidate,
            &anchor.candidate_revision,
            source,
            spec_hash,
            limits,
        )?;
        let materialized_candidate = fork.candidate.clone();
        let mut current_revision = fork.candidate_revision;
        for edge in &explanation.edges {
            let current = staged_forest.revision(&materialized_candidate, &current_revision)?;
            let target = resolve_rewrite_locator(&current.impl_program, &edge.descriptor.target)
                .map_err(|error| {
                    equality_error(
                        ErrorCode::EqualityMaterializationFailed,
                        format!("materialization target replay failed: {}", error.message),
                    )
                })?;
            let report = staged_forest.apply(
                &CandidateTransaction {
                    candidate: materialized_candidate.clone(),
                    base_revision: current_revision,
                    actions: vec![CandidateAction::ApplyKnownRewrite {
                        rule: edge.descriptor.rule.clone(),
                        target,
                        expected_before_impl_hash: Some(edge.descriptor.before_impl_hash.clone()),
                    }],
                },
                source,
                spec_hash,
                limits,
            )?;
            if report.impl_hash != edge.descriptor.after_impl_hash {
                return Err(equality_error(
                    ErrorCode::EqualityMaterializationFailed,
                    "materialized rewrite produced an unexpected impl_hash",
                ));
            }
            current_revision = report.candidate_revision;
        }
        staged_forest.events.truncate(candidate_event_cursor);
        let evidence_id = staged_forest.allocator.evidence();
        let record = EqualityMaterializationRecord {
            equality_space: space_id.clone(),
            equality_revision: equality_revision_id.clone(),
            equality_hash: expected_equality_hash.clone(),
            target_node: target_node.clone(),
            path_digest: explanation.proof_digest.clone(),
            anchor_candidate: anchor.candidate,
            anchor_candidate_revision: anchor.candidate_revision,
            materialized_candidate: materialized_candidate.clone(),
            materialized_revision: current_revision.clone(),
        };
        staged_forest.evidence.insert(
            evidence_id.clone(),
            EvidenceRecord {
                id: evidence_id.clone(),
                class: EvidenceClass::Correctness,
                kind: EvidenceKind::EqualityMaterialization,
                spec_hash: spec_hash.clone(),
                candidate: materialized_candidate.clone(),
                candidate_revision: current_revision.clone(),
                input_impl_hash: Some(explanation.root_impl_hash.clone()),
                output_impl_hash: explanation.target_impl_hash.clone(),
                method: "equality_materialization_v1".to_owned(),
                parameters: BTreeMap::from([
                    ("equality_space".to_owned(), serde_json::json!(space_id)),
                    (
                        "path_digest".to_owned(),
                        serde_json::json!(explanation.proof_digest),
                    ),
                ]),
                result: EvidenceResult::Passed,
                counterexample: None,
                provenance: EvidenceProvenance {
                    compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
                    candidate_semantics_version: EQUALITY_CANDIDATE_SEMANTICS_VERSION,
                    impl_semantics_version: IMPL_SEMANTICS_VERSION,
                },
            },
        );
        let candidate_snapshot = staged_forest.candidate(&materialized_candidate)?.clone();
        let mut terminal = staged_forest
            .revision(&materialized_candidate, &current_revision)?
            .clone();
        if terminal.impl_hash != explanation.target_impl_hash {
            return Err(equality_error(
                ErrorCode::EqualityMaterializationFailed,
                "materialized terminal impl_hash differs from selected equality node",
            ));
        }
        terminal.state = CandidateState::Equivalent;
        terminal.candidate_hash_version = EQUALITY_CANDIDATE_CANONICAL_VERSION;
        terminal.equality_materializations.push(record.clone());
        terminal.evidence.push(evidence_id);
        terminal.candidate_hash = crate::candidate::CandidateHash::new("pending");
        terminal.candidate_hash = candidate_hash_with_limit(
            &staged_forest,
            &candidate_snapshot,
            &terminal,
            candidate_canonical_limit(&terminal, limits),
        )?;
        let candidate_hash = terminal.candidate_hash.clone();
        staged_forest
            .candidates
            .get_mut(&materialized_candidate)
            .expect("materialized candidate exists")
            .revisions
            .insert(current_revision.clone(), terminal);
        staged_store.events.push(VersionedEqualityEvent {
            semantics_version: EQUALITY_SEMANTICS_VERSION,
            candidate_event_cursor: u64::try_from(candidate_event_cursor).unwrap_or(u64::MAX),
            event: EqualityEvent::Materialized {
                equality_space: space_id.clone(),
                equality_revision: equality_revision_id.clone(),
                equality_hash: expected_equality_hash.clone(),
                target_node: target_node.clone(),
                candidate: materialized_candidate.clone(),
                candidate_revision: current_revision.clone(),
                candidate_hash: candidate_hash.clone(),
            },
        });
        ensure_forest_budgets(&staged_forest, limits)?;
        ensure_store_budgets(&staged_store, limits)?;
        staged_forest.check(&materialized_candidate, &current_revision, source, limits)?;
        *forest = staged_forest;
        *self = staged_store;
        Ok(EqualityMaterializationResult {
            candidate: materialized_candidate,
            candidate_revision: current_revision,
            candidate_hash,
            materialization: record,
        })
    }
}

fn verify_edge(
    revision: &EqualityRevision,
    edge: &EqualityProofEdge,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    if edge.id.as_str().is_empty()
        || edge.descriptor.impl_semantics_version != IMPL_SEMANTICS_VERSION
        || edge.descriptor.equality_semantics_version != EQUALITY_SEMANTICS_VERSION
        || edge.descriptor.validator_version != EQUALITY_VALIDATOR_VERSION
    {
        return Err(equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality proof edge uses an unsupported contract version",
        ));
    }
    if crate::candidate::known_rewrite_rule(&edge.descriptor.rule).is_none() {
        return Err(equality_error(
            ErrorCode::EqualityRuleUntrusted,
            format!(
                "equality proof edge uses unknown rule `{}`",
                edge.descriptor.rule
            ),
        ));
    }
    let source_node = revision.nodes.get(&edge.source).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality proof edge source node is missing",
        )
    })?;
    let target_node = revision.nodes.get(&edge.target).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality proof edge target node is missing",
        )
    })?;
    if source_node.impl_hash != edge.descriptor.before_impl_hash
        || target_node.impl_hash != edge.descriptor.after_impl_hash
        || source_node.impl_hash == target_node.impl_hash
        || edge.proof_digest != edge_digest(&edge.descriptor)?
    {
        return Err(equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality proof edge hashes or digest are inconsistent",
        ));
    }
    let mut transformed = source_node.impl_program.clone();
    let target = resolve_rewrite_locator(&transformed, &edge.descriptor.target)?;
    let side_conditions = apply_known_rewrite(&mut transformed, &edge.descriptor.rule, &target)?;
    verify_impl(&transformed, source, limits)?;
    if side_conditions != edge.descriptor.side_conditions
        || impl_hash(&transformed)? != target_node.impl_hash
    {
        return Err(equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality proof edge cannot be reproduced by the production registry",
        ));
    }
    Ok(())
}

fn verify_revision(
    space: &EqualitySpace,
    revision: &EqualityRevision,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    ensure_revision_budgets(revision, limits)?;
    let root = revision.nodes.get(&space.root_node).ok_or_else(|| {
        equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality revision lacks its root node",
        )
    })?;
    if root.impl_hash != space.anchor.root_impl_hash {
        return Err(equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality root hash differs from immutable anchor",
        ));
    }
    let mut hashes = BTreeSet::new();
    for (id, node) in &revision.nodes {
        if node.id != *id || !hashes.insert(node.impl_hash.clone()) {
            return Err(equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality nodes violate ID consistency or impl_hash uniqueness",
            ));
        }
        verify_impl(&node.impl_program, source, limits)?;
        if impl_hash(&node.impl_program)? != node.impl_hash {
            return Err(equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality node impl_hash is invalid",
            ));
        }
    }
    let mut descriptors = BTreeSet::new();
    for (id, edge) in &revision.edges {
        if edge.id != *id
            || !descriptors.insert((
                edge.source.clone(),
                edge.target.clone(),
                edge.descriptor.clone(),
            ))
        {
            return Err(equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality edges contain a duplicate proof descriptor",
            ));
        }
        verify_edge(revision, edge, source, limits)?;
    }
    let mut work_nodes = BTreeSet::new();
    for work in &revision.worklist {
        if !revision.nodes.contains_key(&work.node) || !work_nodes.insert(work.node.clone()) {
            return Err(equality_error(
                ErrorCode::EqualityProofInvalid,
                "equality worklist is duplicated or references a missing node",
            ));
        }
    }
    if revision.worklist.windows(2).any(|pair| pair[0] > pair[1])
        || (revision.status == EqualityStatus::FixedPoint && !revision.worklist.is_empty())
        || (revision.status != EqualityStatus::FixedPoint && revision.worklist.is_empty())
    {
        return Err(equality_error(
            ErrorCode::EqualityProofInvalid,
            "equality worklist and status are inconsistent",
        ));
    }
    let actual_hash = equality_hash_with_limit(space, revision, limits)?;
    if actual_hash != revision.equality_hash {
        return Err(equality_error(
            ErrorCode::EqualityHashMismatch,
            "equality exact-state hash is invalid",
        )
        .with_types(revision.equality_hash.to_string(), actual_hash.to_string()));
    }
    for node in revision.nodes.keys() {
        explanation_for(space, revision, node, limits)?;
    }
    Ok(())
}

impl EqualityStore {
    /// Fully verifies every equality node, edge, hash, explanation and store budget.
    pub fn verify_all<F>(&self, mut source: F, limits: &ResourceLimits) -> AgentResult<()>
    where
        F: FnMut(&RevisionId) -> AgentResult<(Program, SpecHash)>,
    {
        ensure_store_budgets(self, limits)?;
        for event in &self.events {
            if event.semantics_version != EQUALITY_SEMANTICS_VERSION {
                return Err(equality_error(
                    ErrorCode::EqualityEventOrderInvalid,
                    "equality event uses an unsupported semantics version",
                ));
            }
        }
        for (id, space) in &self.spaces {
            if space.id != *id || !space.revisions.contains_key(&space.head) {
                return Err(equality_error(
                    ErrorCode::EqualityProofInvalid,
                    "equality space identity or head is invalid",
                ));
            }
            let (program, spec_hash) = source(&space.anchor.spec_revision)?;
            if spec_hash != space.spec_hash {
                return Err(equality_error(
                    ErrorCode::SpecHashMismatch,
                    "equality space spec_hash anchor is invalid",
                ));
            }
            for revision in space.revisions.values() {
                verify_revision(space, revision, &program, limits)?;
            }
        }
        Ok(())
    }
}
