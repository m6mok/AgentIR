//! Typed holes used for partial programs and synthesis tasks.

use crate::{
    ids::{ActionId, HoleId, ValueId},
    shapes::ShapeConstraint,
    types::Type,
};
use serde::{Deserialize, Serialize};

/// Effect requirement for a hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedEffects {
    /// Stage 1 permits only pure values.
    Pure,
}

/// Lifecycle state of a typed hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoleStatus {
    /// No value has filled the hole.
    Open,
    /// A compatible value has filled the hole.
    Filled,
}

/// Missing graph fragment with statically known requirements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hole {
    /// Persistent hole ID.
    pub id: HoleId,
    /// Placeholder SSA value used by consumers.
    pub placeholder: ValueId,
    /// Required value type.
    pub expected_type: Type,
    /// Required effects; only pure is supported in Stage 1.
    pub expected_effects: ExpectedEffects,
    /// Optional shape constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shape_constraints: Vec<ShapeConstraint>,
    /// Current lifecycle state.
    pub status: HoleStatus,
    /// Action that created the hole.
    pub provenance: ActionId,
    /// Compatible value used to fill this hole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_with: Option<ValueId>,
}
