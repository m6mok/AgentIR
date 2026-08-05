//! Transaction results and temporary binding metadata.

use crate::{
    actions::ActionClassification,
    ids::{ObligationId, RevisionId, TransactionId},
    semantic::SpecHash,
    types::Type,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Successful atomic commit result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitResult {
    /// Compiler-assigned transaction ID.
    pub transaction: TransactionId,
    /// Newly created immutable revision.
    pub revision: RevisionId,
    /// Mapping from `$bindings` to persistent IDs.
    pub bindings: BTreeMap<String, String>,
    /// Types inferred for value-producing bindings.
    pub inferred: BTreeMap<String, Type>,
    /// Classification for each action in source order.
    pub classifications: Vec<ActionClassification>,
    /// Obligations introduced by this transaction.
    pub obligations_created: Vec<ObligationId>,
    /// Canonical content hash of the new revision.
    pub content_hash: String,
    /// Semantic hash when this transaction produced a frozen specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<SpecHash>,
    /// Semantic canonical codec version when a semantic hash is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_canonical_version: Option<u32>,
}
