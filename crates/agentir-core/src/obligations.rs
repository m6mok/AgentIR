//! Proof obligations emitted by Stage 1 verification.

use crate::{
    ids::{ActionId, HoleId, ObligationId, RevisionId, ValueId},
    ir::Opcode,
    types::Type,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Proof obligation kinds implemented by the Stage 1 prototype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    /// A declared or inferred type is well formed.
    TypeWellFormed,
    /// Symbolic tensor shapes must be compatible.
    ShapeCompatible,
    /// A typed hole must be filled.
    HoleFilled,
    /// A specification must have complete valid outputs.
    SpecComplete,
}

/// Current proof state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationStatus {
    /// Proof work remains.
    Open,
    /// The compiler proved the proposition.
    Proved,
    /// The compiler disproved the proposition.
    Refuted,
    /// The Stage 1 proof engine cannot handle the proposition.
    Unsupported,
}

/// Origin of a proof obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationOrigin {
    /// Revision that was current when the obligation was created, if committed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
    /// Action responsible for the proposition.
    pub action: ActionId,
}

/// Relation represented by a structured shape obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeRelationKind {
    /// Left and right logical shapes must be equal in dimension order.
    EqualShape,
}

/// Graph context that created a structured shape obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeObligationContext {
    /// Compatibility required by one operation.
    Operation {
        /// Operation kind being inferred.
        opcode: Opcode,
        /// Persistent operand values participating in inference.
        operands: Vec<ValueId>,
    },
    /// Compatibility required while filling a typed hole.
    Hole {
        /// Hole receiving a value.
        hole: HoleId,
        /// Candidate value used to fill it.
        value: ValueId,
    },
}

/// Machine-readable proposition used for incremental shape discharge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeCompatibilityProposition {
    /// Supported relation kind.
    pub relation: ShapeRelationKind,
    /// Required left type and shape.
    pub left: Type,
    /// Required right type and shape.
    pub right: Type,
    /// Symbols participating in either side, sorted and deduplicated.
    pub involved_symbols: Vec<String>,
    /// Operation or hole context.
    pub context: ShapeObligationContext,
}

/// Explicit proof debt retained in the program state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProofObligation {
    /// Persistent obligation ID.
    pub id: ObligationId,
    /// Stable obligation kind.
    pub kind: ObligationKind,
    /// Machine-readable proposition.
    pub proposition: Value,
    /// Typed shape relation used by Stage 1.2 discharge. Legacy obligations omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape_compatibility: Option<ShapeCompatibilityProposition>,
    /// Provenance.
    pub origin: ObligationOrigin,
    /// Current proof status.
    pub status: ObligationStatus,
    /// Supported ways to discharge or repair the obligation.
    pub discharge_methods: Vec<String>,
}
