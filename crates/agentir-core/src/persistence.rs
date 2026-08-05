//! Versioned workspace snapshots and replay events.

use crate::{
    actions::Transaction,
    ids::{IdAllocator, RevisionId, TransactionId, WorkspaceId},
    revision::Revision,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current schema version for compiler-core workspace snapshots.
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 1;

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
    pub events: Vec<WorkspaceEvent>,
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
}
