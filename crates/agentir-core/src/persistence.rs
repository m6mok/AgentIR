//! Versioned workspace snapshots and replay events.

use crate::{
    actions::Transaction,
    candidate::CandidateForest,
    ids::{IdAllocator, RevisionId, TransactionId, WorkspaceId},
    ir::Program,
    revision::{Revision, StatusSummary},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current compiler semantics used for newly accepted events.
pub const CORE_SEMANTICS_VERSION: u32 = 2;

/// Immutable Stage 1.1 compiler semantics used by archive v1/v2 events.
pub const LEGACY_CORE_SEMANTICS_VERSION: u32 = 1;

/// Current schema version for compiler-core workspace snapshots.
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 4;

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
) -> crate::AgentResult<WorkspaceSnapshot> {
    if snapshot.schema_version != LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION {
        return Err(crate::AgentError::new(
            crate::ErrorCode::PersistenceFormat,
            format!(
                "legacy workspace snapshot version {} is unsupported; expected {}",
                snapshot.schema_version, LEGACY_WORKSPACE_SNAPSHOT_V3_VERSION
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
        candidate_forest: CandidateForest::default(),
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
}
