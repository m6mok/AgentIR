//! JSON request wire types.

use agentir_core::{
    actions::Action,
    candidate::{CandidateAction, ProposedImplFragment, RelationKind},
    continuation::InteractionMode,
    equality::EqualityHash,
    ids::{
        CandidateId, CandidateRevisionId, EqualityNodeId, EqualityRevisionId, EqualitySpaceId,
        HoleId, ImplOperationId, ProposalId, RevisionId, WorkspaceId,
    },
    impl_ir::ImplHash,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Program query representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryView {
    /// Counts, outputs, hash, and status.
    #[default]
    Summary,
    /// Full canonical revision snapshot.
    Canonical,
    /// History-independent semantic form for a complete frozen SpecIR.
    SemanticCanonical,
}

/// One JSONL command accepted by the Stage 1 engine.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
pub enum Request {
    /// Creates an in-memory workspace.
    #[serde(rename = "workspace.open")]
    WorkspaceOpen {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Optional caller-selected workspace ID.
        #[serde(default)]
        workspace: Option<WorkspaceId>,
    },
    /// Atomically writes one workspace to a versioned local archive.
    #[serde(rename = "workspace.save")]
    WorkspaceSave {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Workspace to persist.
        workspace: WorkspaceId,
        /// Destination archive path.
        path: String,
    },
    /// Loads and replays a versioned local workspace archive.
    #[serde(rename = "workspace.load")]
    WorkspaceLoad {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Source archive path.
        path: String,
        /// Explicitly permits replacing an in-memory workspace with the same ID.
        #[serde(default)]
        replace: bool,
    },
    /// Verifies archive checksum, revisions, and deterministic event replay.
    #[serde(rename = "workspace.verify_archive")]
    WorkspaceVerifyArchive {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Archive path to verify without retaining the workspace.
        path: String,
    },
    /// Verifies and migrates one archive into a current v6 destination.
    #[serde(rename = "workspace.migrate_archive")]
    WorkspaceMigrateArchive {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Fully verified source archive path.
        source_path: String,
        /// Atomically written destination archive path.
        destination_path: String,
        /// Explicit permission to replace an existing destination.
        #[serde(default)]
        overwrite: bool,
    },
    /// Applies an ActionIR transaction while constructing SpecIR.
    #[serde(rename = "spec.apply")]
    SpecApply {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Immutable base revision.
        base_revision: RevisionId,
        /// Atomic action sequence.
        actions: Vec<Action>,
        /// Optional client correlation/idempotency ID.
        #[serde(default)]
        client_transaction_id: Option<String>,
        /// Explicit permission to branch from a non-head revision.
        #[serde(default)]
        allow_branch: bool,
    },
    /// Checks SpecIR invariants and completeness.
    #[serde(rename = "spec.check")]
    SpecCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Revision to check; defaults to head.
        #[serde(default)]
        revision: Option<RevisionId>,
    },
    /// Freezes a complete specification in a new revision.
    #[serde(rename = "spec.freeze")]
    SpecFreeze {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Immutable base revision.
        base_revision: RevisionId,
        /// Optional client correlation/idempotency ID.
        #[serde(default)]
        client_transaction_id: Option<String>,
    },
    /// Applies a general ActionIR transaction using the same compiler core.
    #[serde(rename = "transaction.apply")]
    TransactionApply {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Immutable base revision.
        base_revision: RevisionId,
        /// Atomic action sequence.
        actions: Vec<Action>,
        /// Optional client correlation/idempotency ID.
        #[serde(default)]
        client_transaction_id: Option<String>,
        /// Explicit permission to branch from a non-head revision.
        #[serde(default)]
        allow_branch: bool,
    },
    /// Reads a summary or complete canonical snapshot.
    #[serde(rename = "program.query")]
    ProgramQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Revision to query; defaults to head.
        #[serde(default)]
        revision: Option<RevisionId>,
        /// Requested representation.
        #[serde(default)]
        view: QueryView,
    },
    /// Evaluates a frozen complete specification on the CPU oracle.
    #[serde(rename = "program.evaluate")]
    ProgramEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Revision to evaluate; defaults to head.
        #[serde(default)]
        revision: Option<RevisionId>,
        /// Parameter names to JSON scalar/tensor values.
        inputs: BTreeMap<String, Value>,
    },
    /// Creates an explicit child snapshot from any revision.
    #[serde(rename = "revision.fork")]
    RevisionFork {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Existing revision to fork.
        base_revision: RevisionId,
    },
    /// Computes a structural diff between two revisions.
    #[serde(rename = "revision.diff")]
    RevisionDiff {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Source revision.
        from: RevisionId,
        /// Destination revision.
        to: RevisionId,
    },
    /// Generates a continuation frame for an open typed hole.
    #[serde(rename = "continuation.get")]
    ContinuationGet {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Revision establishing the frame context.
        revision: RevisionId,
        /// Focus hole.
        hole: HoleId,
        /// Free, menu, or hybrid interaction policy.
        #[serde(default)]
        mode: InteractionMode,
    },
    /// Creates an identity ImplIR candidate for a complete frozen SpecIR.
    #[serde(rename = "candidate.create")]
    CandidateCreate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Frozen SpecIR revision used as immutable anchor.
        spec_revision: RevisionId,
        /// Exact relation; approximate refinement is rejected in Stage 2B.
        #[serde(default)]
        relation: RelationKind,
    },
    /// Reads one candidate branch or immutable candidate revision.
    #[serde(rename = "candidate.query")]
    CandidateQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Candidate revision; defaults to branch head.
        #[serde(default)]
        candidate_revision: Option<CandidateRevisionId>,
    },
    /// Verifies ImplIR and the exact compositional proof chain.
    #[serde(rename = "candidate.check")]
    CandidateCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Candidate revision; defaults to branch head.
        #[serde(default)]
        candidate_revision: Option<CandidateRevisionId>,
    },
    /// Atomically applies compiler-known exact rewrites.
    #[serde(rename = "candidate.apply")]
    CandidateApply {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit immutable base candidate revision.
        base_candidate_revision: CandidateRevisionId,
        /// Ordered trusted rewrite actions.
        actions: Vec<CandidateAction>,
    },
    /// Forks one candidate revision into a new branch identity.
    #[serde(rename = "candidate.fork")]
    CandidateFork {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Parent candidate.
        candidate: CandidateId,
        /// Parent candidate revision.
        base_candidate_revision: CandidateRevisionId,
    },
    /// Runs fixed-seed bounded differential confidence validation.
    #[serde(rename = "candidate.validate")]
    CandidateValidate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit immutable base candidate revision.
        base_candidate_revision: CandidateRevisionId,
        /// Fixed generator seed.
        #[serde(default)]
        seed: u64,
        /// Bounded case count.
        #[serde(default = "default_validation_cases")]
        cases: u64,
    },
    /// Seals a proved exact candidate.
    #[serde(rename = "candidate.seal")]
    CandidateSeal {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit immutable base candidate revision.
        base_candidate_revision: CandidateRevisionId,
    },
    /// Enumerates a bounded deterministic known-rewrite continuation.
    #[serde(rename = "candidate.continuation")]
    CandidateContinuation {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Candidate revision; defaults to branch head.
        #[serde(default)]
        candidate_revision: Option<CandidateRevisionId>,
    },
    /// Accepts one bounded typed replacement proposal.
    #[serde(rename = "candidate.propose")]
    CandidatePropose {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit immutable candidate head.
        base_candidate_revision: CandidateRevisionId,
        /// Single-result top-level target operation.
        target: ImplOperationId,
        /// Ordered typed replacement fragment.
        replacement: ProposedImplFragment,
        /// Required stale-state implementation hash.
        expected_before_impl_hash: ImplHash,
        /// Explicit permission to retain unknown proof debt.
        #[serde(default)]
        allow_speculative: bool,
        /// Untrusted advisory rule label.
        #[serde(default)]
        claimed_rule: Option<String>,
    },
    /// Reads one persistent normalized proposal record.
    #[serde(rename = "candidate.proposal_query")]
    CandidateProposalQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Compiler-assigned proposal ID.
        proposal: ProposalId,
    },
    /// Runs compiler-owned ordered translation validation.
    #[serde(rename = "candidate.translation_check")]
    CandidateTranslationCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Explicit immutable candidate head.
        base_candidate_revision: CandidateRevisionId,
        /// Proposal proof-debt item to validate.
        proposal: ProposalId,
    },
    /// Evaluates candidate-level primary/guard/fallback semantics.
    #[serde(rename = "candidate.evaluate")]
    CandidateEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate branch.
        candidate: CandidateId,
        /// Candidate revision; defaults to branch head.
        #[serde(default)]
        candidate_revision: Option<CandidateRevisionId>,
        /// Parameter names to exact JSON scalar/tensor values.
        inputs: BTreeMap<String, Value>,
    },
    /// Creates a root-only exact equality space from one proved candidate revision.
    #[serde(rename = "equality.create")]
    EqualityCreate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact candidate anchor.
        candidate: CandidateId,
        /// Explicit immutable proved candidate revision.
        candidate_revision: CandidateRevisionId,
    },
    /// Reads one immutable equality revision summary.
    #[serde(rename = "equality.query")]
    EqualityQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
    },
    /// Expands a bounded number of canonical equality work items.
    #[serde(rename = "equality.expand")]
    EqualityExpand {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit current equality base revision.
        base_equality_revision: EqualityRevisionId,
        /// Required exact base-state hash.
        expected_equality_hash: EqualityHash,
        /// Explicit positive caller work-item fuel.
        fuel: u64,
    },
    /// Saturates deterministically to fixpoint or bounded caller fuel.
    #[serde(rename = "equality.saturate")]
    EqualitySaturate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit current equality base revision.
        base_equality_revision: EqualityRevisionId,
        /// Required exact base-state hash.
        expected_equality_hash: EqualityHash,
        /// Explicit positive caller work-item fuel.
        fuel: u64,
    },
    /// Rebuilds the canonical trusted root-to-node explanation.
    #[serde(rename = "equality.explain")]
    EqualityExplain {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Selected equality member.
        node: EqualityNodeId,
    },
    /// Evaluates one equality member as a reference semantic oracle only.
    #[serde(rename = "equality.evaluate")]
    EqualityEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Selected equality member.
        node: EqualityNodeId,
        /// Parameter names to exact JSON scalar/tensor values.
        inputs: BTreeMap<String, Value>,
    },
    /// Materializes one explicitly selected equality member as a candidate fork.
    #[serde(rename = "equality.materialize")]
    EqualityMaterialize {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Required exact equality-state hash.
        expected_equality_hash: EqualityHash,
        /// Explicit selected equality member; no ranking is performed.
        node: EqualityNodeId,
    },
    /// Returns bounded deterministic next equality work without mutation.
    #[serde(rename = "equality.continuation")]
    EqualityContinuation {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact equality space.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
    },
    /// Discharges the next matching candidate debt with a core-built equality path.
    #[serde(rename = "candidate.equality_check")]
    CandidateEqualityCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Candidate containing ordered proof debt.
        candidate: CandidateId,
        /// Explicit immutable candidate base revision.
        base_candidate_revision: CandidateRevisionId,
        /// Proposal attached to the next unresolved obligation.
        proposal: ProposalId,
        /// Exact equality space used as proof source.
        equality_space: EqualitySpaceId,
        /// Explicit immutable equality revision.
        equality_revision: EqualityRevisionId,
        /// Required exact equality-state hash.
        expected_equality_hash: EqualityHash,
        /// Selected member whose hash must match the debt target.
        target_node: EqualityNodeId,
    },
}

const fn default_validation_cases() -> u64 {
    16
}

impl Request {
    /// Returns the correlation ID common to every request.
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::WorkspaceOpen { request_id, .. }
            | Self::WorkspaceSave { request_id, .. }
            | Self::WorkspaceLoad { request_id, .. }
            | Self::WorkspaceVerifyArchive { request_id, .. }
            | Self::WorkspaceMigrateArchive { request_id, .. }
            | Self::SpecApply { request_id, .. }
            | Self::SpecCheck { request_id, .. }
            | Self::SpecFreeze { request_id, .. }
            | Self::TransactionApply { request_id, .. }
            | Self::ProgramQuery { request_id, .. }
            | Self::ProgramEvaluate { request_id, .. }
            | Self::RevisionFork { request_id, .. }
            | Self::RevisionDiff { request_id, .. }
            | Self::ContinuationGet { request_id, .. }
            | Self::CandidateCreate { request_id, .. }
            | Self::CandidateQuery { request_id, .. }
            | Self::CandidateCheck { request_id, .. }
            | Self::CandidateApply { request_id, .. }
            | Self::CandidateFork { request_id, .. }
            | Self::CandidateValidate { request_id, .. }
            | Self::CandidateSeal { request_id, .. }
            | Self::CandidateContinuation { request_id, .. }
            | Self::CandidatePropose { request_id, .. }
            | Self::CandidateProposalQuery { request_id, .. }
            | Self::CandidateTranslationCheck { request_id, .. }
            | Self::CandidateEvaluate { request_id, .. }
            | Self::EqualityCreate { request_id, .. }
            | Self::EqualityQuery { request_id, .. }
            | Self::EqualityExpand { request_id, .. }
            | Self::EqualitySaturate { request_id, .. }
            | Self::EqualityExplain { request_id, .. }
            | Self::EqualityEvaluate { request_id, .. }
            | Self::EqualityMaterialize { request_id, .. }
            | Self::EqualityContinuation { request_id, .. }
            | Self::CandidateEqualityCheck { request_id, .. } => request_id,
        }
    }
}
