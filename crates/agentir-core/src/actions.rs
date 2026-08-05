//! Wire-friendly ActionIR transaction model.

use crate::{ids::WorkspaceId, shapes::ShapeConstraint, types::Type};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A block argument supplied by a higher-order operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionArgumentSpec {
    /// Local argument name.
    pub name: String,
    /// Explicit argument type checked against the enclosing operation.
    #[serde(rename = "type")]
    pub ty: Type,
}

/// One local operation in an inline region specification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionOpSpec {
    /// Transaction-local result binding such as `$product`.
    pub bind: String,
    /// Opcode selected by the client.
    pub opcode: String,
    /// Argument, local, or explicit capture references.
    pub operands: Vec<String>,
    /// Opcode-specific attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

/// Inline pure region accepted by `map`, `zip_map`, and `reduce`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionSpec {
    /// Ordered block arguments.
    pub arguments: Vec<RegionArgumentSpec>,
    /// Explicit outer references visible to the region body.
    #[serde(default)]
    pub captures: Vec<String>,
    /// Region-local SSA operations.
    pub operations: Vec<RegionOpSpec>,
    /// Argument, local result, or capture yielded by the region.
    pub yield_value: String,
}

/// One atomic graph-edit action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Declares a symbolic dimension.
    DefineDimension {
        /// Optional transaction-local binding.
        #[serde(skip_serializing_if = "Option::is_none")]
        bind: Option<String>,
        /// Unique symbolic name.
        name: String,
        /// Human-readable constraints; `N >= 0` is recognized in Stage 1.
        #[serde(default)]
        constraints: Vec<String>,
    },
    /// Creates an immutable input parameter.
    CreateParameter {
        /// Transaction-local result binding.
        bind: String,
        /// Unique parameter name.
        name: String,
        /// Explicit input type.
        #[serde(rename = "type")]
        ty: Type,
    },
    /// Creates a typed scalar constant.
    CreateConstant {
        /// Transaction-local result binding.
        bind: String,
        /// Explicit scalar type.
        #[serde(rename = "type")]
        ty: Type,
        /// JSON literal immediately canonicalized by the core.
        value: Value,
    },
    /// Creates a placeholder value with known requirements.
    CreateHole {
        /// Transaction-local hole binding.
        bind: String,
        /// Required value type.
        expected_type: Type,
        /// Optional retained shape constraints.
        #[serde(default)]
        shape_constraints: Vec<ShapeConstraint>,
    },
    /// Creates one top-level SpecIR operation.
    CreateOp {
        /// Transaction-local result binding.
        bind: String,
        /// Selected opcode.
        opcode: String,
        /// Persistent, short, or transaction-local operands.
        operands: Vec<String>,
        /// Opcode-specific attributes.
        #[serde(default)]
        attributes: BTreeMap<String, Value>,
        /// Inline pure region for a higher-order operation.
        #[serde(skip_serializing_if = "Option::is_none")]
        region: Option<RegionSpec>,
    },
    /// Fills an existing typed hole.
    FillHole {
        /// Hole reference.
        hole: String,
        /// Compatible value reference.
        value: String,
    },
    /// Sets or replaces a named program output.
    SetOutput {
        /// Output name.
        name: String,
        /// Value reference.
        value: String,
    },
    /// Retains a shape constraint.
    AddConstraint {
        /// Constraint to add.
        constraint: ShapeConstraint,
    },
    /// Verifies completeness and permanently freezes SpecIR.
    FreezeSpec,
    /// Creates an unchanged child revision as an ActionIR transaction.
    ForkRevision,
}

/// Atomic ActionIR transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    /// Target workspace.
    pub workspace: WorkspaceId,
    /// Immutable base revision.
    pub base_revision: crate::ids::RevisionId,
    /// Ordered actions applied atomically.
    pub actions: Vec<Action>,
    /// Idempotency/correlation identifier supplied by the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_transaction_id: Option<String>,
    /// Allows an explicit branch from a non-head revision.
    #[serde(default)]
    pub allow_branch: bool,
}

/// Static classification of an accepted or proposed action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClassification {
    /// Fully verified by the compiler core.
    Legal,
    /// Accepted with an explicit proof obligation.
    Conditional,
    /// Reserved for speculative actions the verifier cannot classify.
    Unknown,
    /// Proven invalid and rejected.
    Illegal,
}

#[cfg(test)]
mod tests {
    use super::ActionClassification;

    #[test]
    fn unknown_classification_is_reserved_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&ActionClassification::Unknown).expect("serializes"),
            "\"unknown\""
        );
    }
}
