//! Parameteric continuation menus for typed holes.

use crate::{
    holes::Hole,
    ids::{ContinuationFrameId, RevisionId, ValueId},
    ir::{Opcode, Program, ValueOrigin},
    shapes::{SolverStatus, same_shape},
    types::{ScalarType, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Client interaction policy sharing the same compiler core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    /// Schema-valid actions are submitted without a menu.
    Free,
    /// Only compiler-generated menu choices are accepted by the client.
    Menu,
    /// Hard masking plus rankings and a verified speculative escape hatch.
    #[default]
    Hybrid,
}

/// Focus of a hole-filling continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationFocus {
    /// Persistent hole ID.
    pub hole: crate::ids::HoleId,
    /// Required value type.
    pub expects: Type,
}

/// One dependent decision slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContinuationSlot {
    /// Stable slot name.
    pub name: String,
    /// Domain kind such as `opcode` or `value_ref`.
    pub kind: String,
    /// Earlier slots that constrain this domain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Parameteric domain rather than a Cartesian product.
    pub domain: Value,
}

/// Policy for proposals outside a compiler-generated menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapePolicy {
    /// Whether escape is enabled for this interaction mode.
    pub allowed: bool,
    /// Stable policy name.
    pub mode: String,
    /// Escaped actions always return to the verifier.
    pub verification_required: bool,
}

/// Compiler-generated parameteric space of valid next steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContinuationFrame {
    /// Persistent frame ID.
    pub frame: ContinuationFrameId,
    /// Revision against which references are valid.
    pub revision: RevisionId,
    /// Stable purpose.
    pub purpose: String,
    /// Hole and expected type.
    pub focus: ContinuationFocus,
    /// Dependent decision slots.
    pub slots: Vec<ContinuationSlot>,
    /// Hard semantic constraints.
    pub hard_constraints: Vec<String>,
    /// Optional heuristic hints with no legality effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_ranking: Option<BTreeMap<String, Value>>,
    /// Escape policy.
    pub escape: EscapePolicy,
}

fn compatible(expected: &Type, candidate: &Type) -> bool {
    match (expected, candidate) {
        (Type::Scalar(left), Type::Scalar(right)) => left == right,
        (
            Type::Tensor {
                element: left_element,
                shape: left_shape,
            },
            Type::Tensor {
                element: right_element,
                shape: right_shape,
            },
        ) => {
            left_element == right_element
                && same_shape(left_shape, right_shape) == SolverStatus::Proved
        }
        _ => false,
    }
}

fn allowed_opcodes(expected: &Type) -> Vec<Opcode> {
    let mut opcodes = vec![Opcode::Select, Opcode::Cast];
    if expected.element_type().is_numeric() {
        opcodes.extend([
            Opcode::Add,
            Opcode::Sub,
            Opcode::Mul,
            Opcode::Div,
            Opcode::Fma,
        ]);
    }
    if matches!(expected, Type::Tensor { .. }) {
        opcodes.extend([Opcode::Map, Opcode::ZipMap]);
    }
    if expected.element_type() == ScalarType::Bool {
        opcodes.push(Opcode::Compare);
    }
    opcodes.sort();
    opcodes.dedup();
    opcodes
}

/// Builds a continuation without materializing operand combinations.
#[must_use]
pub fn build_frame(
    frame: ContinuationFrameId,
    revision: RevisionId,
    program: &Program,
    hole: &Hole,
    mode: InteractionMode,
) -> ContinuationFrame {
    let compatible_values: Vec<ValueId> = program
        .values
        .iter()
        .filter(|(id, definition)| {
            **id != hole.placeholder
                && compatible(&hole.expected_type, &definition.ty)
                && !matches!(definition.origin, ValueOrigin::Hole(ref id) if program.holes.get(id).is_some_and(|candidate| candidate.filled_with.is_none()))
        })
        .map(|(id, _)| id.clone())
        .collect();
    let opcodes = allowed_opcodes(&hole.expected_type);
    let slots = vec![
        ContinuationSlot {
            name: "opcode".to_owned(),
            kind: "opcode".to_owned(),
            depends_on: Vec::new(),
            domain: json!({"enum": opcodes}),
        },
        ContinuationSlot {
            name: "operand_0".to_owned(),
            kind: "value_ref".to_owned(),
            depends_on: vec!["opcode".to_owned()],
            domain: json!({
                "query": "compatible_values",
                "position": 0,
                "values": compatible_values,
            }),
        },
    ];
    let soft_ranking = matches!(mode, InteractionMode::Hybrid).then(|| {
        BTreeMap::from([(
            "reason_code".to_owned(),
            Value::String("PREFER_EXISTING_COMPATIBLE_VALUE".to_owned()),
        )])
    });
    ContinuationFrame {
        frame,
        revision,
        purpose: "fill_hole".to_owned(),
        focus: ContinuationFocus {
            hole: hole.id.clone(),
            expects: hole.expected_type.clone(),
        },
        slots,
        hard_constraints: vec![
            format!("result_type == {}", hole.expected_type),
            "effects == pure".to_owned(),
        ],
        soft_ranking,
        escape: EscapePolicy {
            allowed: !matches!(mode, InteractionMode::Menu),
            mode: if matches!(mode, InteractionMode::Menu) {
                "disabled"
            } else {
                "speculative_proposal"
            }
            .to_owned(),
            verification_required: true,
        },
    }
}
