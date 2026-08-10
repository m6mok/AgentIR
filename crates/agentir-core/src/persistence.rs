//! Versioned workspace snapshots and replay events.

use crate::{
    actions::Transaction,
    backend::{ArtifactStore, BackendStore, MeasurementStore},
    candidate::{
        Candidate, CandidateAllocator, CandidateEvent, CandidateForest, CandidateHash,
        CandidateRevision, CandidateState, CandidateTransaction, DifferentialValidation,
        EquivalenceCertificate, EquivalenceObligation, EquivalenceStatus, EvidenceClass,
        EvidenceKind, EvidenceProvenance, EvidenceRecord, EvidenceResult,
        LEGACY_CANDIDATE_CANONICAL_VERSION, RelationKind, VersionedCandidateEvent,
    },
    cpu::CpuArtifactStore,
    cpu_measurement::CpuMeasurementStore,
    equality::EqualityStore,
    ids::{
        CandidateId, CandidateObligationId, CandidateRevisionId, EvidenceId, IdAllocator,
        RevisionId, TransactionId, WorkspaceId,
    },
    impl_ir::{ImplHash, ImplProgram},
    ir::Program,
    memory::MemoryPlanStore,
    revision::{Revision, StatusSummary},
    schedule::SchedulePlanStore,
    target::TargetManifestStore,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current compiler semantics used for newly accepted events.
pub const CORE_SEMANTICS_VERSION: u32 = 2;

/// Immutable Stage 1.1 compiler semantics used by archive v1/v2 events.
pub const LEGACY_CORE_SEMANTICS_VERSION: u32 = 1;

/// Current schema version for compiler-core workspace snapshots.
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 11;

/// Immutable Stage 8A snapshot schema migrated explicitly to v11.
pub const LEGACY_WORKSPACE_SNAPSHOT_V10_VERSION: u32 = 10;

/// Immutable Stage 5 snapshot schema migrated explicitly to v10.
pub const LEGACY_WORKSPACE_SNAPSHOT_V9_VERSION: u32 = 9;

/// Immutable Stage 4 snapshot schema migrated explicitly to v9.
pub const LEGACY_WORKSPACE_SNAPSHOT_V8_VERSION: u32 = 8;

/// Immutable Stage 3 snapshot schema migrated explicitly to v8.
pub const LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION: u32 = 7;

/// Immutable Stage 2C snapshot schema migrated explicitly to v7.
pub const LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION: u32 = 6;

/// Immutable Stage 2B snapshot schema migrated explicitly to v6.
pub const LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION: u32 = 5;

/// Immutable Stage 2A snapshot schema migrated explicitly to v5.
pub const LEGACY_WORKSPACE_SNAPSHOT_V4_VERSION: u32 = 4;

/// Immutable Stage 1.2 snapshot schema migrated explicitly to v4.
pub const LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION: u32 = 3;

/// Immutable Stage 1.1 snapshot schema migrated explicitly to v3.
pub const LEGACY_WORKSPACE_SNAPSHOT_V2_VERSION: u32 = 2;

/// Immutable legacy schema version accepted only by the explicit v1 migration.
pub const LEGACY_WORKSPACE_SNAPSHOT_VERSION: u32 = 1;

/// One deterministic state-changing event in workspace history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    /// A verified ActionIR transaction created a revision.
    TransactionApplied {
        /// Expected compiler-assigned transaction ID.
        transaction_id: TransactionId,
        /// Expected child revision ID.
        revision: RevisionId,
        /// Expected canonical content hash.
        content_hash: String,
        /// Complete replayable transaction.
        transaction: Transaction,
    },
    /// An unchanged child snapshot was forked from an existing revision.
    RevisionForked {
        /// Existing parent revision.
        base_revision: RevisionId,
        /// Expected child revision ID.
        revision: RevisionId,
        /// Expected canonical content hash.
        content_hash: String,
    },
}

/// One event paired with the compiler semantics that originally accepted it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionedWorkspaceEvent {
    /// Compiler/replay semantics version, independent of archive format version.
    pub semantics_version: u32,
    /// Replayable state-changing payload.
    pub event: WorkspaceEvent,
}

/// Complete versioned in-memory state required to resume one workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Snapshot schema version.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current head revision.
    pub head: RevisionId,
    /// Immutable revision snapshots.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// Compiler ID counters needed to avoid identity reuse after restore.
    pub allocator: IdAllocator,
    /// Ordered state-changing history used for deterministic replay.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Independent candidate forest, allocator, EvidenceIR, and candidate event log.
    pub candidate_forest: CandidateForest,
    /// Independent exact equality spaces and dependency-ordered Stage 2C event log.
    pub equality_store: EqualityStore,
    /// Independent MemoryIR plans, evidence, allocator, and dependency-ordered event log.
    pub memory_store: MemoryPlanStore,
    /// Immutable compiler-owned target capability contracts and target events.
    pub target_store: TargetManifestStore,
    /// Independent ScheduleIR plans, evidence, allocator, and dependency-ordered events.
    pub schedule_store: SchedulePlanStore,
    /// Independent typed BackendIR plans and backend events.
    pub backend_store: BackendStore,
    /// Deterministic WGSL artifact packages and emission events.
    pub artifact_store: ArtifactStore,
    /// Confidence-only hardware measurement records and events.
    pub measurement_store: MeasurementStore,
    /// Deterministic portable CPU packages and dependency-ordered publication events.
    pub cpu_artifact_store: CpuArtifactStore,
    /// Separate bounded CPU measurement records and dependency-ordered events.
    pub cpu_measurement_store: CpuMeasurementStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 10.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV10 {
    /// Legacy schema discriminator, which must equal ten.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current head revision.
    pub head: RevisionId,
    /// Immutable revision snapshots.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// Compiler ID counters.
    pub allocator: IdAllocator,
    /// Ordered SpecIR history.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2 candidate state.
    pub candidate_forest: CandidateForest,
    /// Stage 2C equality state.
    pub equality_store: EqualityStore,
    /// Stage 3 MemoryIR state.
    pub memory_store: MemoryPlanStore,
    /// Stage 4 target state.
    pub target_store: TargetManifestStore,
    /// Stage 4 schedule state.
    pub schedule_store: SchedulePlanStore,
    /// Stage 5 backend state.
    pub backend_store: BackendStore,
    /// Stage 5 WGSL artifact state.
    pub artifact_store: ArtifactStore,
    /// Stage 5 hardware measurement state.
    pub measurement_store: MeasurementStore,
    /// Stage 8A CPU artifact state.
    pub cpu_artifact_store: CpuArtifactStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 9.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV9 {
    /// Legacy schema discriminator, which must equal nine.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current head revision.
    pub head: RevisionId,
    /// Immutable revision snapshots.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// Compiler ID counters.
    pub allocator: IdAllocator,
    /// Ordered SpecIR history.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2 candidate state.
    pub candidate_forest: CandidateForest,
    /// Stage 2C equality state.
    pub equality_store: EqualityStore,
    /// Stage 3 MemoryIR state.
    pub memory_store: MemoryPlanStore,
    /// Stage 4 target state.
    pub target_store: TargetManifestStore,
    /// Stage 4 ScheduleIR state.
    pub schedule_store: SchedulePlanStore,
    /// Stage 5 BackendIR state.
    pub backend_store: BackendStore,
    /// Stage 5 WGSL artifact state.
    pub artifact_store: ArtifactStore,
    /// Stage 5 hardware observation state.
    pub measurement_store: MeasurementStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 8.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV8 {
    /// Legacy schema discriminator, which must equal eight.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head revision.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// SpecIR compiler allocator.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR event log.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2 CandidateForest.
    pub candidate_forest: CandidateForest,
    /// Stage 2C exact equality store.
    pub equality_store: EqualityStore,
    /// Stage 3 MemoryIR plan store.
    pub memory_store: MemoryPlanStore,
    /// Stage 4 immutable target store.
    pub target_store: TargetManifestStore,
    /// Stage 4 ScheduleIR store.
    pub schedule_store: SchedulePlanStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 7.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV7 {
    /// Legacy schema discriminator, which must equal seven.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head revision.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// SpecIR compiler allocator.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR event log.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2 CandidateForest.
    pub candidate_forest: CandidateForest,
    /// Stage 2C exact equality store.
    pub equality_store: EqualityStore,
    /// Stage 3 MemoryIR plan store.
    pub memory_store: MemoryPlanStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 6.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV6 {
    /// Legacy schema discriminator, which must equal six.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head revision.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// SpecIR compiler allocator.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR event log.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2B CandidateForest.
    pub candidate_forest: CandidateForest,
    /// Stage 2C exact equality store.
    pub equality_store: EqualityStore,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV5 {
    /// Legacy schema discriminator, which must equal five.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head revision.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// SpecIR compiler allocator.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR event log.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Stage 2B CandidateForest, including exact and speculative histories.
    pub candidate_forest: CandidateForest,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 3.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV3 {
    /// Legacy schema discriminator, which must equal three.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head revision.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// Legacy SpecIR compiler allocator.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR event log.
    pub events: Vec<VersionedWorkspaceEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum LegacyCandidateStateV1 {
    Draft,
    WellTyped,
    Equivalent,
    Sealed,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum LegacyEquivalenceStatusV1 {
    Open,
    Proved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum LegacyEvidenceKindV1 {
    IdentityLowering,
    KnownRewriteCertificate,
    CompositionalEquivalence,
    DifferentialTest,
    PropertyTest,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyCandidateAllocatorV1 {
    pub candidate: u64,
    pub revision: u64,
    pub operation: u64,
    pub value: u64,
    pub evidence: u64,
    pub obligation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyEvidenceRecordV1 {
    pub id: EvidenceId,
    pub class: EvidenceClass,
    pub kind: LegacyEvidenceKindV1,
    pub spec_hash: crate::semantic::SpecHash,
    pub candidate: CandidateId,
    pub candidate_revision: CandidateRevisionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_impl_hash: Option<ImplHash>,
    pub output_impl_hash: ImplHash,
    pub method: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, serde_json::Value>,
    pub result: EvidenceResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<serde_json::Value>,
    pub provenance: EvidenceProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyEquivalenceObligationV1 {
    pub id: CandidateObligationId,
    pub relation: RelationKind,
    pub spec_hash: crate::semantic::SpecHash,
    pub candidate: CandidateId,
    pub candidate_revision: CandidateRevisionId,
    pub impl_hash: ImplHash,
    pub status: LegacyEquivalenceStatusV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyCandidateRevisionV1 {
    pub id: CandidateRevisionId,
    pub parents: Vec<CandidateRevisionId>,
    pub impl_program: ImplProgram,
    pub impl_hash: ImplHash,
    pub candidate_hash: CandidateHash,
    pub state: LegacyCandidateStateV1,
    pub equivalence: LegacyEquivalenceObligationV1,
    pub proof_chain: Vec<EquivalenceCertificate>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyCandidateV1 {
    pub id: CandidateId,
    pub spec_revision: RevisionId,
    pub spec_hash: crate::semantic::SpecHash,
    pub root_revision: CandidateRevisionId,
    pub head: CandidateRevisionId,
    pub revisions: BTreeMap<CandidateRevisionId, LegacyCandidateRevisionV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_candidate: Option<CandidateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_revision: Option<CandidateRevisionId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum LegacyCandidateEventV1 {
    Created {
        candidate: CandidateId,
        spec_revision: RevisionId,
        relation: RelationKind,
        candidate_revision: CandidateRevisionId,
        impl_hash: ImplHash,
        candidate_hash: CandidateHash,
    },
    TransactionApplied {
        transaction: CandidateTransaction,
        candidate_revision: CandidateRevisionId,
        impl_hash: ImplHash,
        candidate_hash: CandidateHash,
    },
    Forked {
        parent_candidate: CandidateId,
        parent_revision: CandidateRevisionId,
        candidate: CandidateId,
        candidate_revision: CandidateRevisionId,
        candidate_hash: CandidateHash,
    },
    Validated {
        candidate: CandidateId,
        base_revision: CandidateRevisionId,
        candidate_revision: CandidateRevisionId,
        validation: DifferentialValidation,
        candidate_hash: CandidateHash,
    },
    Sealed {
        candidate: CandidateId,
        base_revision: CandidateRevisionId,
        candidate_revision: CandidateRevisionId,
        candidate_hash: CandidateHash,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyVersionedCandidateEventV1 {
    pub semantics_version: u32,
    pub event: LegacyCandidateEventV1,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct LegacyCandidateForestV1 {
    pub candidates: BTreeMap<CandidateId, LegacyCandidateV1>,
    pub evidence: BTreeMap<EvidenceId, LegacyEvidenceRecordV1>,
    pub allocator: LegacyCandidateAllocatorV1,
    pub events: Vec<LegacyVersionedCandidateEventV1>,
}

/// Immutable workspace snapshot schema embedded in archive format v4.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV4 {
    /// Legacy schema discriminator, which must equal four.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current SpecIR head.
    pub head: RevisionId,
    /// Immutable SpecIR revisions.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// SpecIR allocator state.
    pub allocator: IdAllocator,
    /// Semantics-versioned SpecIR events.
    pub events: Vec<VersionedWorkspaceEvent>,
    /// Exact Stage 2A CandidateForest v1 representation.
    pub candidate_forest: LegacyCandidateForestV1,
}

impl LegacyWorkspaceSnapshotV4 {
    /// Returns the number of legacy candidate events for archive preflight/metadata.
    #[must_use]
    pub fn candidate_event_count(&self) -> usize {
        self.candidate_forest.events.len()
    }

    /// Returns legacy candidate/revision/evidence counts for hard-cap preflight.
    #[must_use]
    pub fn candidate_counts(&self) -> (usize, u64, usize) {
        let revisions =
            self.candidate_forest
                .candidates
                .values()
                .fold(0_u64, |total, candidate| {
                    total.saturating_add(
                        u64::try_from(candidate.revisions.len()).unwrap_or(u64::MAX),
                    )
                });
        (
            self.candidate_forest.candidates.len(),
            revisions,
            self.candidate_forest.evidence.len(),
        )
    }

    /// Returns the encoded legacy EvidenceIR byte count for hard-cap preflight.
    pub fn evidence_encoded_bytes(&self) -> crate::AgentResult<u64> {
        let bytes = serde_json::to_vec(&self.candidate_forest.evidence).map_err(|error| {
            crate::AgentError::new(
                crate::ErrorCode::PersistenceFormat,
                format!("legacy evidence preflight encoding failed: {error}"),
            )
        })?;
        Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
    }
}

/// Immutable compiler-core snapshot schema embedded in archive format version 2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV2 {
    /// Legacy schema discriminator, which must equal two.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Current head revision.
    pub head: RevisionId,
    /// Immutable revision snapshots including semantic cache metadata.
    pub revisions: BTreeMap<RevisionId, Revision>,
    /// Compiler ID counters.
    pub allocator: IdAllocator,
    /// Ordered unversioned Stage 1.1 events.
    pub events: Vec<WorkspaceEvent>,
}

/// Revision representation embedded in immutable archive format version 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyRevisionV1 {
    /// Persistent revision ID.
    pub id: RevisionId,
    /// Immutable parent revision IDs.
    pub parents: Vec<RevisionId>,
    /// History-sensitive content hash retained unchanged during migration.
    pub content_hash: String,
    /// Full Stage 1 graph snapshot.
    pub program: Program,
    /// Transaction that created the revision, absent for roots and forks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_transaction: Option<TransactionId>,
    /// Wall-clock metadata excluded from semantic and content hashes.
    pub created_at_unix_ms: u128,
    /// Cached verifier summary.
    pub status: StatusSummary,
}

/// Immutable compiler-core snapshot schema embedded in archive format version 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyWorkspaceSnapshotV1 {
    /// Legacy schema discriminator, which must equal one.
    pub schema_version: u32,
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Archived head revision.
    pub head: RevisionId,
    /// Legacy immutable revisions.
    pub revisions: BTreeMap<RevisionId, LegacyRevisionV1>,
    /// Compiler allocator state.
    pub allocator: IdAllocator,
    /// Ordered replay events.
    pub events: Vec<WorkspaceEvent>,
}

/// Purely migrates legacy snapshot v1 to immutable snapshot schema v2.
pub fn migrate_snapshot_v1(
    snapshot: LegacyWorkspaceSnapshotV1,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV2> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_VERSION
            ),
        ));
    }
    let revisions = snapshot
        .revisions
        .into_iter()
        .map(|(id, revision)| {
            (
                id,
                Revision {
                    id: revision.id,
                    parents: revision.parents,
                    content_hash: revision.content_hash,
                    spec_hash: None,
                    semantic_canonical_version: None,
                    program: revision.program,
                    applied_transaction: revision.applied_transaction,
                    created_at_unix_ms: revision.created_at_unix_ms,
                    status: revision.status,
                },
            )
        })
        .collect();
    Ok(LegacyWorkspaceSnapshotV2 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V2_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
    })
}

/// Purely migrates snapshot schema v2 to immutable v3 by tagging every legacy event.
pub fn migrate_snapshot_v2(
    snapshot: LegacyWorkspaceSnapshotV2,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV3> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V2_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V2_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV3 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot
            .events
            .into_iter()
            .map(|event| VersionedWorkspaceEvent {
                semantics_version: LEGACY_CORE_SEMANTICS_VERSION,
                event,
            })
            .collect(),
    })
}

/// Purely migrates immutable snapshot schema v3 to v4 with an empty candidate forest.
pub fn migrate_snapshot_v3(
    snapshot: LegacyWorkspaceSnapshotV3,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV4> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV4 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V4_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: LegacyCandidateForestV1::default(),
    })
}

fn migrate_candidate_state(state: LegacyCandidateStateV1) -> CandidateState {
    match state {
        LegacyCandidateStateV1::Draft => CandidateState::Draft,
        LegacyCandidateStateV1::WellTyped => CandidateState::WellTyped,
        LegacyCandidateStateV1::Equivalent => CandidateState::Equivalent,
        LegacyCandidateStateV1::Sealed => CandidateState::Sealed,
        LegacyCandidateStateV1::Rejected => CandidateState::Rejected,
    }
}

fn migrate_equivalence_status(status: LegacyEquivalenceStatusV1) -> EquivalenceStatus {
    match status {
        LegacyEquivalenceStatusV1::Open => EquivalenceStatus::Open,
        LegacyEquivalenceStatusV1::Proved => EquivalenceStatus::Proved,
    }
}

fn migrate_evidence_kind(kind: LegacyEvidenceKindV1) -> EvidenceKind {
    match kind {
        LegacyEvidenceKindV1::IdentityLowering => EvidenceKind::IdentityLowering,
        LegacyEvidenceKindV1::KnownRewriteCertificate => EvidenceKind::KnownRewriteCertificate,
        LegacyEvidenceKindV1::CompositionalEquivalence => EvidenceKind::CompositionalEquivalence,
        LegacyEvidenceKindV1::DifferentialTest => EvidenceKind::DifferentialTest,
        LegacyEvidenceKindV1::PropertyTest => EvidenceKind::PropertyTest,
    }
}

fn migrate_candidate_event(event: LegacyCandidateEventV1) -> CandidateEvent {
    match event {
        LegacyCandidateEventV1::Created {
            candidate,
            spec_revision,
            relation,
            candidate_revision,
            impl_hash,
            candidate_hash,
        } => CandidateEvent::Created {
            candidate,
            spec_revision,
            relation,
            candidate_revision,
            impl_hash,
            candidate_hash,
        },
        LegacyCandidateEventV1::TransactionApplied {
            transaction,
            candidate_revision,
            impl_hash,
            candidate_hash,
        } => CandidateEvent::TransactionApplied {
            transaction,
            candidate_revision,
            impl_hash,
            candidate_hash,
        },
        LegacyCandidateEventV1::Forked {
            parent_candidate,
            parent_revision,
            candidate,
            candidate_revision,
            candidate_hash,
        } => CandidateEvent::Forked {
            parent_candidate,
            parent_revision,
            candidate,
            candidate_revision,
            candidate_hash,
        },
        LegacyCandidateEventV1::Validated {
            candidate,
            base_revision,
            candidate_revision,
            validation,
            candidate_hash,
        } => CandidateEvent::Validated {
            candidate,
            base_revision,
            candidate_revision,
            validation,
            candidate_hash,
        },
        LegacyCandidateEventV1::Sealed {
            candidate,
            base_revision,
            candidate_revision,
            candidate_hash,
        } => CandidateEvent::Sealed {
            candidate,
            base_revision,
            candidate_revision,
            candidate_hash,
        },
    }
}

/// Purely migrates immutable snapshot schema v4 to v5 without recalculating legacy hashes.
pub fn migrate_snapshot_v4(
    snapshot: LegacyWorkspaceSnapshotV4,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV5> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V4_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V4_VERSION
            ),
        ));
    }
    let LegacyCandidateAllocatorV1 {
        candidate,
        revision,
        operation,
        value,
        evidence: evidence_counter,
        obligation,
    } = snapshot.candidate_forest.allocator;
    let candidates = snapshot
        .candidate_forest
        .candidates
        .into_iter()
        .map(|(id, candidate)| {
            let revisions = candidate
                .revisions
                .into_iter()
                .map(|(revision_id, revision)| {
                    let equivalence = EquivalenceObligation {
                        id: revision.equivalence.id,
                        relation: revision.equivalence.relation,
                        spec_hash: revision.equivalence.spec_hash,
                        candidate: revision.equivalence.candidate,
                        candidate_revision: revision.equivalence.candidate_revision,
                        impl_hash: revision.equivalence.impl_hash,
                        status: migrate_equivalence_status(revision.equivalence.status),
                    };
                    (
                        revision_id,
                        CandidateRevision {
                            id: revision.id,
                            parents: revision.parents,
                            impl_program: revision.impl_program,
                            impl_hash: revision.impl_hash,
                            candidate_hash: revision.candidate_hash,
                            candidate_hash_version: LEGACY_CANDIDATE_CANONICAL_VERSION,
                            state: migrate_candidate_state(revision.state),
                            equivalence,
                            proof_chain: revision.proof_chain,
                            evidence: revision.evidence,
                            proof_frontier: None,
                            proof_debt: Vec::new(),
                            translation_results: Vec::new(),
                            guarded_fallback: None,
                            equality_proofs: Vec::new(),
                            equality_materializations: Vec::new(),
                        },
                    )
                })
                .collect();
            (
                id,
                Candidate {
                    id: candidate.id,
                    spec_revision: candidate.spec_revision,
                    spec_hash: candidate.spec_hash,
                    root_revision: candidate.root_revision,
                    head: candidate.head,
                    revisions,
                    parent_candidate: candidate.parent_candidate,
                    forked_from_revision: candidate.forked_from_revision,
                },
            )
        })
        .collect();
    let evidence = snapshot
        .candidate_forest
        .evidence
        .into_iter()
        .map(|(id, record)| {
            (
                id,
                EvidenceRecord {
                    id: record.id,
                    class: record.class,
                    kind: migrate_evidence_kind(record.kind),
                    spec_hash: record.spec_hash,
                    candidate: record.candidate,
                    candidate_revision: record.candidate_revision,
                    input_impl_hash: record.input_impl_hash,
                    output_impl_hash: record.output_impl_hash,
                    method: record.method,
                    parameters: record.parameters,
                    result: record.result,
                    counterexample: record.counterexample,
                    provenance: record.provenance,
                },
            )
        })
        .collect();
    let events = snapshot
        .candidate_forest
        .events
        .into_iter()
        .map(|event| VersionedCandidateEvent {
            semantics_version: event.semantics_version,
            event: migrate_candidate_event(event.event),
        })
        .collect();
    Ok(LegacyWorkspaceSnapshotV5 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: CandidateForest {
            candidates,
            evidence,
            proposals: BTreeMap::new(),
            allocator: CandidateAllocator::from_legacy_counters(
                candidate,
                revision,
                operation,
                value,
                evidence_counter,
                obligation,
            ),
            events,
        },
    })
}

/// Purely migrates immutable snapshot schema v5 to v6 with an empty equality store.
pub fn migrate_snapshot_v5(
    snapshot: LegacyWorkspaceSnapshotV5,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV6> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V5_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV6 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: EqualityStore::default(),
    })
}

/// Purely migrates immutable snapshot schema v6 to v7 with an empty MemoryIR store.
pub fn migrate_snapshot_v6(
    snapshot: LegacyWorkspaceSnapshotV6,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV7> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V6_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV7 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
        memory_store: MemoryPlanStore::default(),
    })
}

/// Purely migrates immutable snapshot schema v7 to v8 with empty target/schedule stores.
pub fn migrate_snapshot_v7(
    snapshot: LegacyWorkspaceSnapshotV7,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV8> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V7_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV8 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V8_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
        memory_store: snapshot.memory_store,
        target_store: TargetManifestStore::default(),
        schedule_store: SchedulePlanStore::default(),
    })
}

/// Purely migrates immutable snapshot schema v8 to v9 with empty Stage 5 stores.
pub fn migrate_snapshot_v8(
    snapshot: LegacyWorkspaceSnapshotV8,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV9> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V8_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V8_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV9 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V9_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
        memory_store: snapshot.memory_store,
        target_store: snapshot.target_store,
        schedule_store: snapshot.schedule_store,
        backend_store: BackendStore::default(),
        artifact_store: ArtifactStore::default(),
        measurement_store: MeasurementStore::default(),
    })
}

/// Purely migrates immutable snapshot schema v9 to v10 with an empty CPU artifact store.
pub fn migrate_snapshot_v9(
    snapshot: LegacyWorkspaceSnapshotV9,
) -> crate::AgentResult<LegacyWorkspaceSnapshotV10> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V9_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V9_VERSION
            ),
        ));
    }
    Ok(LegacyWorkspaceSnapshotV10 {
        schema_version: LEGACY_WORKSPACE_SNAPSHOT_V10_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
        memory_store: snapshot.memory_store,
        target_store: snapshot.target_store,
        schedule_store: snapshot.schedule_store,
        backend_store: snapshot.backend_store,
        artifact_store: snapshot.artifact_store,
        measurement_store: snapshot.measurement_store,
        cpu_artifact_store: CpuArtifactStore::default(),
    })
}

/// Purely migrates immutable snapshot schema v10 to v11 with an empty CPU measurement store.
pub fn migrate_snapshot_v10(
    snapshot: LegacyWorkspaceSnapshotV10,
) -> crate::AgentResult<WorkspaceSnapshot> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V10_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V10_VERSION
            ),
        ));
    }
    Ok(WorkspaceSnapshot {
        schema_version: WORKSPACE_SNAPSHOT_VERSION,
        workspace: snapshot.workspace,
        head: snapshot.head,
        revisions: snapshot.revisions,
        allocator: snapshot.allocator,
        events: snapshot.events,
        candidate_forest: snapshot.candidate_forest,
        equality_store: snapshot.equality_store,
        memory_store: snapshot.memory_store,
        target_store: snapshot.target_store,
        schedule_store: snapshot.schedule_store,
        backend_store: snapshot.backend_store,
        artifact_store: snapshot.artifact_store,
        measurement_store: snapshot.measurement_store,
        cpu_artifact_store: snapshot.cpu_artifact_store,
        cpu_measurement_store: CpuMeasurementStore::default(),
    })
}

/// Evidence that an event log reproduced an archived workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Replayed workspace identity.
    pub workspace: WorkspaceId,
    /// Reproduced head revision.
    pub head: RevisionId,
    /// Number of verified revisions, including `r0`.
    pub revisions_verified: usize,
    /// Number of replayed state-changing events.
    pub events_replayed: usize,
    /// Number of revision content hashes independently recomputed.
    pub content_hashes_verified: usize,
    /// Number of frozen revision semantic hashes independently recomputed.
    pub spec_hashes_verified: usize,
    /// Number of independent candidate branches verified.
    pub candidates_verified: usize,
    /// Number of candidate events replayed with candidate semantics v1.
    pub candidate_events_replayed: usize,
    /// Number of evidence records verified through exact candidate state.
    pub evidence_records_verified: usize,
    /// Number of exact equality spaces independently replayed and verified.
    pub equality_spaces_verified: usize,
    /// Number of dependency-ordered Stage 2C events replayed.
    pub equality_events_replayed: usize,
    /// Number of independent MemoryIR plans verified.
    pub memory_plans_verified: usize,
    /// Number of dependency-ordered MemoryIR events replayed.
    pub memory_events_replayed: usize,
    /// Number of immutable target manifests verified.
    pub target_manifests_verified: usize,
    /// Number of target creation events replayed.
    pub target_events_replayed: usize,
    /// Number of independent ScheduleIR plans verified.
    pub schedule_plans_verified: usize,
    /// Number of dependency-ordered schedule events replayed.
    pub schedule_events_replayed: usize,
    /// Number of independent BackendIR plans verified.
    pub backend_plans_verified: usize,
    /// Number of dependency-ordered backend events verified.
    pub backend_events_replayed: usize,
    /// Number of deterministic artifact packages verified.
    pub artifacts_verified: usize,
    /// Number of artifact emission events verified.
    pub artifact_events_replayed: usize,
    /// Number of confidence-only hardware records structurally verified.
    pub measurements_verified: usize,
    /// Number of measurement events verified without hardware replay.
    pub measurement_events_replayed: usize,
    /// Number of deterministic portable CPU packages verified without execution.
    pub cpu_artifacts_verified: usize,
    /// Number of CPU artifact publication events replayed without execution.
    pub cpu_artifact_events_replayed: usize,
    /// Number of CPU timing observations structurally verified without execution or clock reads.
    pub cpu_measurements_verified: usize,
    /// Number of CPU measurement events replayed without execution or clock reads.
    pub cpu_measurement_events_replayed: usize,
}
