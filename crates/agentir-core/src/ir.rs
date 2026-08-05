//! Canonical Stage 1 SpecIR graph data model.

use crate::{
    holes::Hole,
    ids::{ActionId, DimensionId, HoleId, OperationId, ValueId},
    obligations::ProofObligation,
    shapes::ShapeConstraint,
    types::{NumericContract, ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::{collections::BTreeMap, fmt, str::FromStr};

/// An operation recognized by the Stage 1 compiler core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opcode {
    /// External immutable input.
    Parameter,
    /// Typed literal.
    Constant,
    /// Numeric addition.
    Add,
    /// Numeric subtraction.
    Sub,
    /// Numeric multiplication.
    Mul,
    /// Numeric division.
    Div,
    /// Fused multiply-add with contract-controlled semantics.
    Fma,
    /// Typed comparison.
    Compare,
    /// Conditional value selection.
    Select,
    /// Explicit scalar or element cast.
    Cast,
    /// Unary elementwise region application.
    Map,
    /// N-ary elementwise region application.
    ZipMap,
    /// Deterministic reduction.
    Reduce,
}

impl fmt::Display for Opcode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        formatter.write_str(value.as_str().ok_or(fmt::Error)?)
    }
}

impl FromStr for Opcode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(JsonValue::String(value.to_owned()))
            .map_err(|_| format!("unknown opcode `{value}`"))
    }
}

/// Exactly encoded scalar constant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstantValue {
    /// Boolean literal.
    Bool {
        /// Literal value.
        value: bool,
    },
    /// Signed 32-bit literal.
    I32 {
        /// Literal value.
        value: i32,
    },
    /// Binary32 literal represented by its IEEE-754 bits.
    F32 {
        /// Lowercase hexadecimal bits, including the `0x` prefix.
        bits: String,
    },
}

impl ConstantValue {
    /// Parses a wire JSON literal according to an explicitly supplied scalar type.
    pub fn from_json(ty: ScalarType, value: &JsonValue) -> Result<Self, String> {
        match ty {
            ScalarType::Bool => value
                .as_bool()
                .map(|value| Self::Bool { value })
                .ok_or_else(|| "expected a JSON boolean".to_owned()),
            ScalarType::I32 | ScalarType::Index => value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .map(|value| Self::I32 { value })
                .ok_or_else(|| "expected a JSON integer in i32 range".to_owned()),
            ScalarType::F32 => {
                if let Some(number) = value.as_f64() {
                    let bits = (number as f32).to_bits();
                    Ok(Self::F32 {
                        bits: format!("0x{bits:08x}"),
                    })
                } else if let Some(bits) = value.as_str() {
                    let parsed = u32::from_str_radix(bits.trim_start_matches("0x"), 16)
                        .map_err(|_| "expected f32 number or 0x-prefixed bits".to_owned())?;
                    Ok(Self::F32 {
                        bits: format!("0x{parsed:08x}"),
                    })
                } else {
                    Err("expected f32 number or 0x-prefixed bits".to_owned())
                }
            }
        }
    }

    /// Returns the literal scalar type.
    #[must_use]
    pub const fn scalar_type(&self) -> ScalarType {
        match self {
            Self::Bool { .. } => ScalarType::Bool,
            Self::I32 { .. } => ScalarType::I32,
            Self::F32 { .. } => ScalarType::F32,
        }
    }

    /// Decodes an f32 literal.
    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        let Self::F32 { bits } = self else {
            return None;
        };
        u32::from_str_radix(bits.trim_start_matches("0x"), 16)
            .ok()
            .map(f32::from_bits)
    }
}

/// A declared symbolic dimension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimension {
    /// Persistent dimension ID.
    pub id: DimensionId,
    /// Unique human-facing symbol.
    pub name: String,
    /// Whether the extent is constrained to be non-negative.
    pub non_negative: bool,
    /// Action that created the dimension.
    pub provenance: ActionId,
}

/// Source of a logical SSA value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ValueOrigin {
    /// Result of an operation.
    Operation(OperationId),
    /// Placeholder value owned by a typed hole.
    Hole(HoleId),
}

/// One typed SSA value in the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueDef {
    /// Persistent value ID.
    pub id: ValueId,
    /// Compiler-inferred or explicitly required type.
    pub ty: Type,
    /// Operation or hole that defines the value.
    pub origin: ValueOrigin,
    /// Optional stable user-facing name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Reference used by an operation inside a pure region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RegionValue {
    /// Region block argument by name.
    Argument(String),
    /// Result of an earlier operation in this region.
    Local(String),
    /// Explicitly captured outer SSA value.
    Capture(ValueId),
}

/// One typed region block argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockArgument {
    /// Local argument name.
    pub name: String,
    /// Argument type.
    pub ty: Type,
}

/// Pure SSA operation nested in a region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionOperation {
    /// Local result binding.
    pub result: String,
    /// Opcode.
    pub opcode: Opcode,
    /// Region-local operands.
    pub operands: Vec<RegionValue>,
    /// Deterministically ordered attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Compiler-inferred result type.
    pub result_type: Type,
}

/// A closed, pure region used by a higher-order operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Ordered block arguments.
    pub arguments: Vec<BlockArgument>,
    /// Explicit outer values visible to the body.
    pub captures: Vec<ValueId>,
    /// Region-local SSA operations.
    pub operations: Vec<RegionOperation>,
    /// Value yielded by the region.
    pub yield_value: RegionValue,
    /// Verified yielded type.
    pub yield_type: Type,
}

/// One top-level SpecIR operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    /// Persistent operation ID.
    pub id: OperationId,
    /// Canonical opcode.
    pub opcode: Opcode,
    /// Persistent operand value IDs.
    pub operands: Vec<ValueId>,
    /// Result value IDs. Stage 1 creates one, while the model permits more later.
    pub results: Vec<ValueId>,
    /// Deterministically ordered attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Optional pure region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Action that created this operation.
    pub provenance: ActionId,
    /// Inferred result types in result order.
    pub result_types: Vec<Type>,
}

/// Canonical functional program stored by a revision.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Program {
    /// Declared symbolic dimensions by persistent ID.
    pub dimensions: BTreeMap<DimensionId, Dimension>,
    /// Name-to-dimension lookup included in canonical state.
    pub dimension_names: BTreeMap<String, DimensionId>,
    /// Operations by persistent ID.
    pub operations: BTreeMap<OperationId, Operation>,
    /// Stable topological insertion order for operations.
    pub operation_order: Vec<OperationId>,
    /// Values by persistent ID.
    pub values: BTreeMap<ValueId, ValueDef>,
    /// Parameter name-to-value mapping.
    pub parameters: BTreeMap<String, ValueId>,
    /// Constant values indexed by their result IDs.
    pub constants: BTreeMap<ValueId, ConstantValue>,
    /// Named program outputs.
    pub outputs: BTreeMap<String, ValueId>,
    /// Typed holes by persistent ID.
    pub holes: BTreeMap<HoleId, Hole>,
    /// Shape constraints retained for future solvers.
    pub constraints: Vec<ShapeConstraint>,
    /// Proof obligations attached to this revision.
    pub obligations: BTreeMap<crate::ids::ObligationId, ProofObligation>,
    /// Explicit numeric semantics.
    pub numeric_contract: NumericContract,
    /// Whether the specification has been frozen.
    pub frozen: bool,
}

impl Program {
    /// Resolves a placeholder value to its filled value, if necessary.
    #[must_use]
    pub fn resolve_filled_value<'a>(&'a self, value: &'a ValueId) -> &'a ValueId {
        let Some(definition) = self.values.get(value) else {
            return value;
        };
        let ValueOrigin::Hole(hole_id) = &definition.origin else {
            return value;
        };
        self.holes
            .get(hole_id)
            .and_then(|hole| hole.filled_with.as_ref())
            .unwrap_or(value)
    }
}
