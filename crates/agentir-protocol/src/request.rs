//! JSON request wire types.

use agentir_core::{
    actions::Action,
    backend::{ArtifactHash, BackendHash},
    backend_ir::HardwareBenchmarkConfig,
    candidate::{CandidateAction, ProposedImplFragment, RelationKind},
    continuation::InteractionMode,
    equality::EqualityHash,
    ids::{
        ArtifactId, BackendPlanId, BackendRevisionId, BufferId, CandidateId, CandidateRevisionId,
        EqualityNodeId, EqualityRevisionId, EqualitySpaceId, HoleId, ImplOperationId,
        MeasurementId, MemoryGuardId, MemoryPlanId, MemoryRevisionId, ProposalId, RevisionId,
        ScheduleAxisId, SchedulePlanId, ScheduleRevisionId, TargetManifestId,
        TargetManifestRevisionId, WorkspaceId,
    },
    impl_ir::ImplHash,
    memory::{MemoryAction, MemoryHash},
    schedule::{ScheduleAction, ScheduleHash},
    target::{TargetHash, TargetProfile},
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
    /// Verifies and migrates one archive into a current v7 destination.
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
    /// Creates a conservative exact MemoryIR plan from a proved candidate revision.
    #[serde(rename = "memory.create")]
    MemoryCreate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Exact candidate branch.
        candidate: CandidateId,
        /// Explicit immutable proved candidate revision.
        candidate_revision: CandidateRevisionId,
    },
    /// Reads one immutable MemoryIR revision summary.
    #[serde(rename = "memory.query")]
    MemoryQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
    },
    /// Fully verifies one MemoryIR revision against its immutable ImplIR anchor.
    #[serde(rename = "memory.check")]
    MemoryCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
    },
    /// Applies an atomic compiler-verified MemoryIR transaction.
    #[serde(rename = "memory.apply")]
    MemoryApply {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit current base memory revision.
        base_memory_revision: MemoryRevisionId,
        /// Required exact base memory hash.
        expected_memory_hash: MemoryHash,
        /// Required immutable implementation hash.
        expected_impl_hash: ImplHash,
        /// Ordered compiler-verified storage requests.
        actions: Vec<MemoryAction>,
    },
    /// Forks an immutable MemoryIR revision into an independent plan identity.
    #[serde(rename = "memory.fork")]
    MemoryFork {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Parent memory plan.
        memory_plan: MemoryPlanId,
        /// Parent immutable memory revision.
        memory_revision: MemoryRevisionId,
        /// Required exact parent hash.
        expected_memory_hash: MemoryHash,
    },
    /// Seals one structurally proved exact or guarded MemoryIR plan.
    #[serde(rename = "memory.seal")]
    MemorySeal {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit current base revision.
        memory_revision: MemoryRevisionId,
        /// Required exact base hash.
        expected_memory_hash: MemoryHash,
    },
    /// Evaluates the exact physical MemoryIR plan with a deterministic trace.
    #[serde(rename = "memory.evaluate")]
    MemoryEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
        /// Parameter names to exact JSON scalar/tensor values.
        inputs: BTreeMap<String, Value>,
        /// Optional runtime outcomes for compiler-owned guards.
        #[serde(default)]
        guard_outcomes: BTreeMap<MemoryGuardId, bool>,
    },
    /// Returns one compiler-owned alias relation.
    #[serde(rename = "memory.alias_query")]
    MemoryAliasQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
        /// First typed buffer.
        first: BufferId,
        /// Second typed buffer.
        second: BufferId,
    },
    /// Returns one immutable typed buffer region.
    #[serde(rename = "memory.buffer_query")]
    MemoryBufferQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
        /// Typed buffer to inspect.
        buffer: BufferId,
    },
    /// Returns bounded deterministic legal storage choices without mutation.
    #[serde(rename = "memory.continuation")]
    MemoryContinuation {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Memory plan identity.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
    },
    /// Lists compiler-owned immutable target manifests.
    #[serde(rename = "target.list")]
    TargetList {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
    },
    /// Instantiates one compiler-owned target profile.
    #[serde(rename = "target.create")]
    TargetCreate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Compiler-owned profile selector.
        profile: TargetProfile,
    },
    /// Reads one immutable target manifest.
    #[serde(rename = "target.query")]
    TargetQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Manifest identity.
        target_manifest: TargetManifestId,
        /// Immutable manifest revision.
        target_revision: TargetManifestRevisionId,
    },
    /// Fully verifies one target manifest.
    #[serde(rename = "target.check")]
    TargetCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Manifest identity.
        target_manifest: TargetManifestId,
        /// Immutable manifest revision.
        target_revision: TargetManifestRevisionId,
    },
    /// Creates a conservative serial schedule from MemoryIR and a target manifest.
    #[serde(rename = "schedule.create")]
    ScheduleCreate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Verified memory plan.
        memory_plan: MemoryPlanId,
        /// Explicit immutable memory revision.
        memory_revision: MemoryRevisionId,
        /// Immutable target manifest.
        target_manifest: TargetManifestId,
        /// Explicit immutable target revision.
        target_revision: TargetManifestRevisionId,
    },
    /// Reads one immutable schedule summary.
    #[serde(rename = "schedule.query")]
    ScheduleQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
    },
    /// Fully verifies one schedule revision.
    #[serde(rename = "schedule.check")]
    ScheduleCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
    },
    /// Applies an atomic compiler-verified schedule transaction.
    #[serde(rename = "schedule.apply")]
    ScheduleApply {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Explicit current base schedule revision.
        base_schedule_revision: ScheduleRevisionId,
        /// Required exact base schedule hash.
        expected_schedule_hash: ScheduleHash,
        /// Required immutable memory hash.
        expected_memory_hash: MemoryHash,
        /// Required immutable target hash.
        expected_target_hash: TargetHash,
        /// Ordered compiler-verified actions.
        actions: Vec<ScheduleAction>,
    },
    /// Forks one immutable schedule revision.
    #[serde(rename = "schedule.fork")]
    ScheduleFork {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Parent schedule plan.
        schedule_plan: SchedulePlanId,
        /// Parent immutable revision.
        schedule_revision: ScheduleRevisionId,
        /// Required exact parent hash.
        expected_schedule_hash: ScheduleHash,
    },
    /// Seals one resource-valid exact schedule.
    #[serde(rename = "schedule.seal")]
    ScheduleSeal {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Explicit current base revision.
        schedule_revision: ScheduleRevisionId,
        /// Required exact base hash.
        expected_schedule_hash: ScheduleHash,
    },
    /// Evaluates an exact schedule through the deterministic MemoryIR oracle.
    #[serde(rename = "schedule.evaluate")]
    ScheduleEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
        /// Parameter names to exact JSON values.
        inputs: BTreeMap<String, Value>,
        /// Optional runtime outcomes for compiler-owned MemoryIR guards.
        #[serde(default)]
        guard_outcomes: BTreeMap<MemoryGuardId, bool>,
    },
    /// Returns the deterministic analytical target resource estimate.
    #[serde(rename = "schedule.resource_query")]
    ScheduleResourceQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
    },
    /// Reads one compiler-assigned schedule axis.
    #[serde(rename = "schedule.axis_query")]
    ScheduleAxisQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
        /// Axis to inspect.
        axis: ScheduleAxisId,
    },
    /// Answers whether one schedule action satisfies all hard conditions.
    #[serde(rename = "schedule.legality_query")]
    ScheduleLegalityQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
        /// Proposed action; no proof payload is accepted.
        action: ScheduleAction,
    },
    /// Returns bounded parametric schedule choices without mutation.
    #[serde(rename = "schedule.continuation")]
    ScheduleContinuation {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Schedule plan.
        schedule_plan: SchedulePlanId,
        /// Immutable schedule revision.
        schedule_revision: ScheduleRevisionId,
    },
    /// Lowers one immutable ScheduleIR revision into compiler-owned BackendIR.
    #[serde(rename = "backend.lower")]
    BackendLower {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Source schedule plan.
        schedule_plan: SchedulePlanId,
        /// Exact immutable source revision.
        schedule_revision: ScheduleRevisionId,
        /// Required exact schedule hash.
        expected_schedule_hash: ScheduleHash,
    },
    /// Reads one immutable BackendIR summary.
    #[serde(rename = "backend.query")]
    BackendQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Backend plan.
        backend_plan: BackendPlanId,
        /// Immutable backend revision.
        backend_revision: BackendRevisionId,
    },
    /// Fully verifies one BackendIR revision and proof certificate.
    #[serde(rename = "backend.check")]
    BackendCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Backend plan.
        backend_plan: BackendPlanId,
        /// Immutable backend revision.
        backend_revision: BackendRevisionId,
    },
    /// Returns bounded deterministic backend choices.
    #[serde(rename = "backend.continuation")]
    BackendContinuation {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Backend plan.
        backend_plan: BackendPlanId,
        /// Immutable backend revision.
        backend_revision: BackendRevisionId,
    },
    /// Forks one immutable backend revision into an independent plan.
    #[serde(rename = "backend.fork")]
    BackendFork {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Parent backend plan.
        backend_plan: BackendPlanId,
        /// Parent immutable revision.
        backend_revision: BackendRevisionId,
        /// Required exact parent hash.
        expected_backend_hash: BackendHash,
    },
    /// Seals one proved BackendIR revision as an immutable child.
    #[serde(rename = "backend.seal")]
    BackendSeal {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Backend plan.
        backend_plan: BackendPlanId,
        /// Explicit current base revision.
        backend_revision: BackendRevisionId,
        /// Required exact backend hash.
        expected_backend_hash: BackendHash,
    },
    /// Emits one deterministic offline-validated WGSL package.
    #[serde(rename = "artifact.emit")]
    ArtifactEmit {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Source backend plan.
        backend_plan: BackendPlanId,
        /// Exact source backend revision.
        backend_revision: BackendRevisionId,
        /// Required exact backend hash.
        expected_backend_hash: BackendHash,
    },
    /// Lists retained artifact packages.
    #[serde(rename = "artifact.list")]
    ArtifactList {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
    },
    /// Reads one artifact summary.
    #[serde(rename = "artifact.query")]
    ArtifactQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Artifact identity.
        artifact: ArtifactId,
    },
    /// Verifies an artifact manifest, WGSL bytes, hash, and certificate.
    #[serde(rename = "artifact.check")]
    ArtifactCheck {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Artifact identity.
        artifact: ArtifactId,
        /// Required exact artifact hash.
        expected_artifact_hash: ArtifactHash,
    },
    /// Evaluates the anchored exact schedule and returns an artifact-plan trace.
    #[serde(rename = "artifact.reference_evaluate")]
    ArtifactReferenceEvaluate {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Artifact identity.
        artifact: ArtifactId,
        /// Exact parameter values.
        inputs: BTreeMap<String, Value>,
        /// Optional compiler-owned MemoryIR guard outcomes.
        #[serde(default)]
        guard_outcomes: BTreeMap<MemoryGuardId, bool>,
    },
    /// Executes one verified artifact on a selected compatible WebGPU adapter.
    #[serde(rename = "artifact.execute")]
    ArtifactExecute {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Artifact identity.
        artifact: ArtifactId,
        /// Required exact artifact hash.
        expected_artifact_hash: ArtifactHash,
        /// Zero-based adapter selector from `device.list`.
        adapter: u32,
        /// Runtime scalar/tensor values.
        inputs: BTreeMap<String, Value>,
    },
    /// Lists WebGPU adapters against one immutable target contract.
    #[serde(rename = "device.list")]
    DeviceList {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Target manifest identity.
        target_manifest: TargetManifestId,
        /// Immutable target revision.
        target_revision: TargetManifestRevisionId,
    },
    /// Reads one discovered WebGPU adapter against an immutable target.
    #[serde(rename = "device.query")]
    DeviceQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Target manifest identity.
        target_manifest: TargetManifestId,
        /// Immutable target revision.
        target_revision: TargetManifestRevisionId,
        /// Zero-based adapter selector.
        adapter: u32,
    },
    /// Starts a bounded confidence-only hardware benchmark.
    #[serde(rename = "benchmark.start")]
    BenchmarkStart {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Artifact identity.
        artifact: ArtifactId,
        /// Required exact artifact hash.
        expected_artifact_hash: ArtifactHash,
        /// Zero-based adapter selector.
        adapter: u32,
        /// Bounded benchmark configuration.
        config: HardwareBenchmarkConfig,
        /// Runtime scalar/tensor values.
        inputs: BTreeMap<String, Value>,
    },
    /// Reads one benchmark task state.
    #[serde(rename = "benchmark.status")]
    BenchmarkStatus {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Compiler-owned task handle.
        task: String,
    },
    /// Cancels a benchmark task that has not completed.
    #[serde(rename = "benchmark.cancel")]
    BenchmarkCancel {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Compiler-owned task handle.
        task: String,
    },
    /// Reads one completed immutable measurement record.
    #[serde(rename = "benchmark.query")]
    BenchmarkQuery {
        /// Correlation ID echoed in the response.
        request_id: String,
        /// Target workspace.
        workspace: WorkspaceId,
        /// Compiler-owned measurement identity.
        measurement: MeasurementId,
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
            | Self::CandidateEqualityCheck { request_id, .. }
            | Self::MemoryCreate { request_id, .. }
            | Self::MemoryQuery { request_id, .. }
            | Self::MemoryCheck { request_id, .. }
            | Self::MemoryApply { request_id, .. }
            | Self::MemoryFork { request_id, .. }
            | Self::MemorySeal { request_id, .. }
            | Self::MemoryEvaluate { request_id, .. }
            | Self::MemoryAliasQuery { request_id, .. }
            | Self::MemoryBufferQuery { request_id, .. }
            | Self::MemoryContinuation { request_id, .. }
            | Self::TargetList { request_id, .. }
            | Self::TargetCreate { request_id, .. }
            | Self::TargetQuery { request_id, .. }
            | Self::TargetCheck { request_id, .. }
            | Self::ScheduleCreate { request_id, .. }
            | Self::ScheduleQuery { request_id, .. }
            | Self::ScheduleCheck { request_id, .. }
            | Self::ScheduleApply { request_id, .. }
            | Self::ScheduleFork { request_id, .. }
            | Self::ScheduleSeal { request_id, .. }
            | Self::ScheduleEvaluate { request_id, .. }
            | Self::ScheduleResourceQuery { request_id, .. }
            | Self::ScheduleAxisQuery { request_id, .. }
            | Self::ScheduleLegalityQuery { request_id, .. }
            | Self::ScheduleContinuation { request_id, .. }
            | Self::BackendLower { request_id, .. }
            | Self::BackendQuery { request_id, .. }
            | Self::BackendCheck { request_id, .. }
            | Self::BackendContinuation { request_id, .. }
            | Self::BackendFork { request_id, .. }
            | Self::BackendSeal { request_id, .. }
            | Self::ArtifactEmit { request_id, .. }
            | Self::ArtifactList { request_id, .. }
            | Self::ArtifactQuery { request_id, .. }
            | Self::ArtifactCheck { request_id, .. }
            | Self::ArtifactReferenceEvaluate { request_id, .. }
            | Self::ArtifactExecute { request_id, .. }
            | Self::DeviceList { request_id, .. }
            | Self::DeviceQuery { request_id, .. }
            | Self::BenchmarkStart { request_id, .. }
            | Self::BenchmarkStatus { request_id, .. }
            | Self::BenchmarkCancel { request_id, .. }
            | Self::BenchmarkQuery { request_id, .. } => request_id,
        }
    }
}
