//! Immutable revision snapshots and structural diffs.

use crate::{
    ids::{HoleId, OperationId, RevisionId, TransactionId, ValueId},
    ir::Program,
    semantic::SpecHash,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Compact verifier summary stored with a revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSummary {
    /// Number of logical values.
    pub values: usize,
    /// Number of operations.
    pub operations: usize,
    /// Number of open holes.
    pub open_holes: usize,
    /// Number of open proof obligations.
    pub open_obligations: usize,
    /// Whether SpecIR is frozen.
    pub frozen: bool,
}

impl StatusSummary {
    /// Builds a summary from canonical program state.
    #[must_use]
    pub fn from_program(program: &Program) -> Self {
        Self {
            values: program.values.len(),
            operations: program.operations.len(),
            open_holes: program
                .holes
                .values()
                .filter(|hole| matches!(hole.status, crate::holes::HoleStatus::Open))
                .count(),
            open_obligations: program
                .obligations
                .values()
                .filter(|obligation| {
                    matches!(
                        obligation.status,
                        crate::obligations::ObligationStatus::Open
                    )
                })
                .count(),
            frozen: program.frozen,
        }
    }
}

/// Immutable workspace revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    /// Persistent revision ID.
    pub id: RevisionId,
    /// One or more immutable parents; Stage 1 creates at most one.
    pub parents: Vec<RevisionId>,
    /// SHA-256 of deterministic canonical state.
    pub content_hash: String,
    /// History-independent semantic identity for a complete frozen SpecIR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<SpecHash>,
    /// Semantic canonical codec version used for `spec_hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_canonical_version: Option<u32>,
    /// Full immutable Stage 1 graph snapshot.
    pub program: Program,
    /// Transaction that created the snapshot, absent for roots and explicit forks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_transaction: Option<TransactionId>,
    /// Wall-clock metadata excluded from the content hash.
    pub created_at_unix_ms: u128,
    /// Cached compact verifier summary.
    pub status: StatusSummary,
}

/// Added and removed IDs between two directly or indirectly related revisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionDiff {
    /// Source revision.
    pub from: RevisionId,
    /// Destination revision.
    pub to: RevisionId,
    /// Added operations.
    pub operations_added: Vec<OperationId>,
    /// Removed operations.
    pub operations_removed: Vec<OperationId>,
    /// Added values.
    pub values_added: Vec<ValueId>,
    /// Removed values.
    pub values_removed: Vec<ValueId>,
    /// Added holes.
    pub holes_added: Vec<HoleId>,
    /// Removed holes.
    pub holes_removed: Vec<HoleId>,
    /// Outputs whose target differs, including additions and removals.
    pub outputs_changed: BTreeMap<String, (Option<ValueId>, Option<ValueId>)>,
    /// Whether freeze state changed.
    pub frozen_changed: bool,
}

fn added<K: Ord + Clone, V>(left: &BTreeMap<K, V>, right: &BTreeMap<K, V>) -> Vec<K> {
    right
        .keys()
        .filter(|key| !left.contains_key(*key))
        .cloned()
        .collect()
}

/// Computes a deterministic structural diff between two snapshots.
#[must_use]
pub fn diff(from: &Revision, to: &Revision) -> RevisionDiff {
    let mut output_names: Vec<_> = from
        .program
        .outputs
        .keys()
        .chain(to.program.outputs.keys())
        .cloned()
        .collect();
    output_names.sort();
    output_names.dedup();
    let outputs_changed = output_names
        .into_iter()
        .filter_map(|name| {
            let before = from.program.outputs.get(&name).cloned();
            let after = to.program.outputs.get(&name).cloned();
            (before != after).then_some((name, (before, after)))
        })
        .collect();
    RevisionDiff {
        from: from.id.clone(),
        to: to.id.clone(),
        operations_added: added(&from.program.operations, &to.program.operations),
        operations_removed: added(&to.program.operations, &from.program.operations),
        values_added: added(&from.program.values, &to.program.values),
        values_removed: added(&to.program.values, &from.program.values),
        holes_added: added(&from.program.holes, &to.program.holes),
        holes_removed: added(&to.program.holes, &from.program.holes),
        outputs_changed,
        frozen_changed: from.program.frozen != to.program.frozen,
    }
}
