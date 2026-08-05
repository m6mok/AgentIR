//! JSON request wire types.

use agentir_core::{
    actions::Action,
    continuation::InteractionMode,
    ids::{HoleId, RevisionId, WorkspaceId},
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
}

/// One JSONL command accepted by the Stage 1 engine.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "command")]
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
            | Self::SpecApply { request_id, .. }
            | Self::SpecCheck { request_id, .. }
            | Self::SpecFreeze { request_id, .. }
            | Self::TransactionApply { request_id, .. }
            | Self::ProgramQuery { request_id, .. }
            | Self::ProgramEvaluate { request_id, .. }
            | Self::RevisionFork { request_id, .. }
            | Self::RevisionDiff { request_id, .. }
            | Self::ContinuationGet { request_id, .. } => request_id,
        }
    }
}
