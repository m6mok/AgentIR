//! Type and shape inference for SpecIR operations.

use crate::{
    actions::ActionClassification,
    constraints::{ConstraintFacts, ConstraintQueryResult},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ir::{Opcode, Region},
    shapes::{SolverStatus, same_shape},
    types::{ScalarType, Type},
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Inferred operation result and legality class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inference {
    /// Result type.
    pub ty: Type,
    /// Whether the operation is fully legal or shape-conditional.
    pub classification: ActionClassification,
    /// Structured shape equalities that remain unknown after inference.
    pub shape_relations: Vec<ShapeRelation>,
}

/// One unknown type/shape equality emitted by inference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeRelation {
    /// Left inferred or required type.
    pub left: Type,
    /// Right inferred or required type.
    pub right: Type,
}

fn arity(opcode: Opcode, operands: &[Type], expected: usize) -> AgentResult<()> {
    if operands.len() == expected {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::ArityMismatch,
            format!(
                "{opcode} expects {expected} operands, got {}",
                operands.len()
            ),
        )
        .with_types(expected as u64, operands.len() as u64))
    }
}

fn same_type(left: &Type, right: &Type) -> AgentResult<ActionClassification> {
    match (left, right) {
        (Type::Scalar(left), Type::Scalar(right)) if left == right => {
            Ok(ActionClassification::Legal)
        }
        (
            Type::Tensor {
                element: left_element,
                shape: left_shape,
            },
            Type::Tensor {
                element: right_element,
                shape: right_shape,
            },
        ) if left_element == right_element => match same_shape(left_shape, right_shape) {
            SolverStatus::Proved => Ok(ActionClassification::Legal),
            SolverStatus::Unknown => Ok(ActionClassification::Conditional),
            SolverStatus::Contradiction => Err(AgentError::new(
                ErrorCode::ShapeMismatch,
                "tensor shapes contradict",
            )
            .with_types(left.to_string(), right.to_string())),
        },
        _ => Err(
            AgentError::new(ErrorCode::TypeMismatch, "operand types differ")
                .with_types(left.to_string(), right.to_string()),
        ),
    }
}

fn same_type_with_facts(
    left: &Type,
    right: &Type,
    facts: &ConstraintFacts,
) -> AgentResult<(ActionClassification, Vec<ShapeRelation>)> {
    match (left, right) {
        (Type::Scalar(left), Type::Scalar(right)) if left == right => {
            Ok((ActionClassification::Legal, Vec::new()))
        }
        (
            Type::Tensor {
                element: left_element,
                ..
            },
            Type::Tensor {
                element: right_element,
                ..
            },
        ) if left_element == right_element => match facts.query_types(left, right)? {
            ConstraintQueryResult::Proved { .. } => Ok((ActionClassification::Legal, Vec::new())),
            ConstraintQueryResult::Unknown => Ok((
                ActionClassification::Conditional,
                vec![ShapeRelation {
                    left: left.clone(),
                    right: right.clone(),
                }],
            )),
            ConstraintQueryResult::Contradiction { .. } => Err(AgentError::new(
                ErrorCode::ShapeMismatch,
                "tensor shapes contradict accepted facts",
            )
            .with_types(left.to_string(), right.to_string())),
        },
        _ => Err(
            AgentError::new(ErrorCode::TypeMismatch, "operand types differ")
                .with_types(left.to_string(), right.to_string()),
        ),
    }
}

fn compatible_types(
    left: &Type,
    right: &Type,
    facts: Option<&ConstraintFacts>,
) -> AgentResult<(ActionClassification, Vec<ShapeRelation>)> {
    facts.map_or_else(
        || same_type(left, right).map(|classification| (classification, Vec::new())),
        |facts| same_type_with_facts(left, right, facts),
    )
}

fn merge(left: ActionClassification, right: ActionClassification) -> ActionClassification {
    if matches!(left, ActionClassification::Conditional)
        || matches!(right, ActionClassification::Conditional)
    {
        ActionClassification::Conditional
    } else {
        ActionClassification::Legal
    }
}

fn require_numeric(opcode: Opcode, ty: &Type) -> AgentResult<()> {
    if ty.element_type().is_numeric() {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::TypeMismatch,
            format!("{opcode} requires numeric operands"),
        )
        .with_types("numeric", ty.to_string()))
    }
}

/// Infers a region-free primitive operation.
fn infer_primitive_impl(
    opcode: Opcode,
    operands: &[Type],
    attributes: &BTreeMap<String, Value>,
    facts: Option<&ConstraintFacts>,
) -> AgentResult<Inference> {
    match opcode {
        Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div => {
            arity(opcode, operands, 2)?;
            require_numeric(opcode, &operands[0])?;
            let (classification, shape_relations) =
                compatible_types(&operands[0], &operands[1], facts)?;
            Ok(Inference {
                ty: operands[0].clone(),
                classification,
                shape_relations,
            })
        }
        Opcode::Fma => {
            arity(opcode, operands, 3)?;
            require_numeric(opcode, &operands[0])?;
            let (left_classification, mut shape_relations) =
                compatible_types(&operands[0], &operands[1], facts)?;
            let (right_classification, right_relations) =
                compatible_types(&operands[0], &operands[2], facts)?;
            shape_relations.extend(right_relations);
            let classification = merge(left_classification, right_classification);
            Ok(Inference {
                ty: operands[0].clone(),
                classification,
                shape_relations,
            })
        }
        Opcode::Compare => {
            arity(opcode, operands, 2)?;
            let (classification, shape_relations) =
                compatible_types(&operands[0], &operands[1], facts)?;
            Ok(Inference {
                ty: operands[0].with_element_type(ScalarType::Bool),
                classification,
                shape_relations,
            })
        }
        Opcode::Select => {
            arity(opcode, operands, 3)?;
            let (branch_classification, mut shape_relations) =
                compatible_types(&operands[1], &operands[2], facts)?;
            let condition = &operands[0];
            let mut condition_classification = ActionClassification::Legal;
            let condition_ok = match (condition, &operands[1]) {
                (Type::Scalar(ScalarType::Bool), Type::Scalar(_) | Type::Tensor { .. }) => true,
                (
                    Type::Tensor {
                        element: ScalarType::Bool,
                        shape: condition_shape,
                    },
                    Type::Tensor { shape, .. },
                ) => {
                    if let Some(facts) = facts {
                        match facts.query_shapes(condition_shape, shape)? {
                            ConstraintQueryResult::Proved { .. } => true,
                            ConstraintQueryResult::Unknown => {
                                condition_classification = ActionClassification::Conditional;
                                shape_relations.push(ShapeRelation {
                                    left: condition.clone(),
                                    right: operands[1].with_element_type(ScalarType::Bool),
                                });
                                true
                            }
                            ConstraintQueryResult::Contradiction { .. } => false,
                        }
                    } else {
                        same_shape(condition_shape, shape) == SolverStatus::Proved
                    }
                }
                _ => false,
            };
            if !condition_ok {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "select condition must be bool or same-shape tensor<bool>",
                ));
            }
            Ok(Inference {
                ty: operands[1].clone(),
                classification: merge(branch_classification, condition_classification),
                shape_relations,
            })
        }
        Opcode::Cast => {
            arity(opcode, operands, 1)?;
            let target = attributes
                .get("target_type")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::TypeMismatch,
                        "cast requires string attribute `target_type`",
                    )
                })?
                .parse::<Type>()
                .map_err(|message| AgentError::new(ErrorCode::TypeMismatch, message))?;
            let target_element = match target {
                Type::Scalar(scalar) => scalar,
                Type::Tensor { .. } => {
                    return Err(AgentError::new(
                        ErrorCode::TypeMismatch,
                        "cast target_type must name a scalar element type",
                    ));
                }
            };
            let source_element = operands[0].element_type();
            let supported = source_element == target_element
                || matches!(
                    (source_element, target_element),
                    (ScalarType::I32 | ScalarType::Index, ScalarType::F32)
                        | (ScalarType::F32, ScalarType::I32 | ScalarType::Index)
                        | (ScalarType::I32, ScalarType::Index)
                        | (ScalarType::Index, ScalarType::I32)
                );
            if !supported {
                return Err(
                    AgentError::new(ErrorCode::TypeMismatch, "unsupported explicit cast")
                        .with_types(target_element.to_string(), source_element.to_string()),
                );
            }
            Ok(Inference {
                ty: operands[0].with_element_type(target_element),
                classification: ActionClassification::Legal,
                shape_relations: Vec::new(),
            })
        }
        Opcode::Parameter | Opcode::Constant => Err(AgentError::new(
            ErrorCode::UnknownOpcode,
            format!("{opcode} must be created by its dedicated action"),
        )),
        Opcode::Map | Opcode::ZipMap | Opcode::Reduce => Err(AgentError::new(
            ErrorCode::InvalidRegion,
            format!("{opcode} requires a verified region"),
        )),
    }
}

/// Infers a region-free primitive operation using legacy Stage 1.1 shape semantics.
pub fn infer_primitive(
    opcode: Opcode,
    operands: &[Type],
    attributes: &BTreeMap<String, Value>,
) -> AgentResult<Inference> {
    infer_primitive_impl(opcode, operands, attributes, None)
}

/// Infers a primitive operation using accepted Stage 1.2 constraint facts.
pub fn infer_primitive_with_facts(
    opcode: Opcode,
    operands: &[Type],
    attributes: &BTreeMap<String, Value>,
    facts: &ConstraintFacts,
) -> AgentResult<Inference> {
    infer_primitive_impl(opcode, operands, attributes, Some(facts))
}

/// Infers a higher-order operation after the region body has been verified.
fn infer_higher_impl(
    opcode: Opcode,
    operands: &[Type],
    region: &Region,
    facts: Option<&ConstraintFacts>,
) -> AgentResult<Inference> {
    match opcode {
        Opcode::Map => {
            arity(opcode, operands, 1)?;
            let Type::Tensor { shape, .. } = &operands[0] else {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "map input must be a tensor",
                ));
            };
            if region.arguments.len() != 1 {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "map region needs one argument",
                ));
            }
            let Type::Scalar(element) = region.yield_type else {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "map region must yield a scalar",
                ));
            };
            Ok(Inference {
                ty: Type::Tensor {
                    element,
                    shape: shape.clone(),
                },
                classification: ActionClassification::Legal,
                shape_relations: Vec::new(),
            })
        }
        Opcode::ZipMap => {
            if operands.is_empty() {
                return Err(AgentError::new(
                    ErrorCode::ArityMismatch,
                    "zip_map needs at least one tensor",
                ));
            }
            if region.arguments.len() != operands.len() {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "zip_map argument count must match tensor operands",
                ));
            }
            let Type::Tensor { shape, .. } = &operands[0] else {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "zip_map inputs must be tensors",
                ));
            };
            let mut classification = ActionClassification::Legal;
            let mut shape_relations = Vec::new();
            for operand in &operands[1..] {
                let (operand_classification, relations) =
                    compatible_types(&operands[0], operand, facts)?;
                classification = merge(classification, operand_classification);
                shape_relations.extend(relations);
            }
            let Type::Scalar(element) = region.yield_type else {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "zip_map region must yield a scalar",
                ));
            };
            Ok(Inference {
                ty: Type::Tensor {
                    element,
                    shape: shape.clone(),
                },
                classification,
                shape_relations,
            })
        }
        Opcode::Reduce => {
            arity(opcode, operands, 2)?;
            let Type::Tensor { element, .. } = operands[0] else {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "reduce input must be a tensor",
                ));
            };
            if operands[1] != Type::Scalar(element) {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "reduce identity must match tensor element type",
                ));
            }
            if region.arguments.len() != 2 || region.yield_type != Type::Scalar(element) {
                return Err(AgentError::new(
                    ErrorCode::InvalidRegion,
                    "reduce combiner must be (T,T) -> T",
                ));
            }
            Ok(Inference {
                ty: Type::Scalar(element),
                classification: ActionClassification::Legal,
                shape_relations: Vec::new(),
            })
        }
        _ => infer_primitive_impl(opcode, operands, &BTreeMap::new(), facts),
    }
}

/// Infers a higher-order operation with legacy Stage 1.1 shape semantics.
pub fn infer_higher(opcode: Opcode, operands: &[Type], region: &Region) -> AgentResult<Inference> {
    infer_higher_impl(opcode, operands, region, None)
}

/// Infers a higher-order operation using accepted Stage 1.2 constraint facts.
pub fn infer_higher_with_facts(
    opcode: Opcode,
    operands: &[Type],
    region: &Region,
    facts: &ConstraintFacts,
) -> AgentResult<Inference> {
    infer_higher_impl(opcode, operands, region, Some(facts))
}

#[cfg(test)]
mod tests {
    use super::infer_primitive;
    use crate::{actions::ActionClassification, ir::Opcode, types::Type};
    use std::collections::BTreeMap;

    #[test]
    fn infers_scalar_arithmetic() {
        let ty: Type = "f32".parse().expect("valid type");
        let inferred = infer_primitive(Opcode::Add, &[ty.clone(), ty.clone()], &BTreeMap::new())
            .expect("valid add");
        assert_eq!(inferred.ty, ty);
        assert_eq!(inferred.classification, ActionClassification::Legal);
    }

    #[test]
    fn marks_unknown_tensor_shapes_conditional() {
        let left: Type = "tensor<f32,[N]>".parse().expect("valid type");
        let right: Type = "tensor<f32,[M]>".parse().expect("valid type");
        let inferred = infer_primitive(Opcode::Add, &[left, right], &BTreeMap::new())
            .expect("conditionally valid add");
        assert_eq!(inferred.classification, ActionClassification::Conditional);
    }

    #[test]
    fn validates_explicit_casts() {
        let attributes = BTreeMap::from([(
            "target_type".to_owned(),
            serde_json::Value::String("i32".to_owned()),
        )]);
        let inferred = infer_primitive(
            Opcode::Cast,
            &["f32".parse().expect("valid type")],
            &attributes,
        )
        .expect("numeric cast is valid");
        assert_eq!(inferred.ty, "i32".parse::<Type>().expect("valid type"));

        let bool_attributes = BTreeMap::from([(
            "target_type".to_owned(),
            serde_json::Value::String("bool".to_owned()),
        )]);
        assert!(
            infer_primitive(
                Opcode::Cast,
                &["f32".parse().expect("valid type")],
                &bool_attributes,
            )
            .is_err()
        );
    }
}
