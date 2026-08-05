//! Deterministic compact facts for Stage 1.2 shape constraints.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ir::Program,
    shapes::ShapeConstraint,
    types::{DimExpr, Shape, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// A normalized proof returned by the compact fact engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintProof {
    /// Deterministic normalized left shape.
    pub normalized_left: String,
    /// Deterministic normalized right shape.
    pub normalized_right: String,
    /// Accepted facts sufficient for this compact derivation.
    pub facts: Vec<ShapeConstraint>,
}

/// Deterministic evidence for a proven conflict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintContradiction {
    /// Constraint whose normalized form contradicts the fact set.
    pub normalized_constraint: ShapeConstraint,
    /// Previously accepted facts participating in deterministic order.
    pub conflicting_facts: Vec<ShapeConstraint>,
    /// Expected normalized fact.
    pub expected: String,
    /// Conflicting normalized fact.
    pub actual: String,
}

/// Result of a compact constraint query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConstraintQueryResult {
    /// Equality follows from accepted compact facts.
    Proved {
        /// Deterministic proof evidence.
        proof: ConstraintProof,
    },
    /// Equality is impossible under accepted compact facts.
    Contradiction {
        /// Deterministic conflict evidence.
        contradiction: ConstraintContradiction,
    },
    /// The supported rules cannot decide equality.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NormalizedDim {
    Static(i128),
    Affine {
        coefficient: i64,
        symbol: String,
        constant: i64,
    },
}

enum RawQuery {
    Proved,
    Contradiction { expected: String, actual: String },
    Unknown,
}

impl std::fmt::Display for NormalizedDim {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(value) => value.fmt(formatter),
            Self::Affine {
                coefficient,
                symbol,
                constant,
            } => write!(formatter, "{coefficient}*{symbol}{constant:+}"),
        }
    }
}

/// Immutable-derived, deterministically updated compact fact model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConstraintFacts {
    parent: BTreeMap<String, String>,
    static_bindings: BTreeMap<String, u64>,
    non_negative: BTreeSet<String>,
    accepted: BTreeSet<ShapeConstraint>,
}

impl ConstraintFacts {
    /// Builds facts from one canonical program without mutating it.
    pub fn from_program(program: &Program) -> AgentResult<Self> {
        let mut facts = Self::default();
        for dimension in program.dimensions.values() {
            facts.declare_symbol(&dimension.name, dimension.non_negative)?;
        }
        for constraint in &program.constraints {
            facts.insert(constraint)?;
        }
        Ok(facts)
    }

    /// Declares one symbol before it can appear in a fact.
    pub fn declare_symbol(&mut self, symbol: &str, non_negative: bool) -> AgentResult<()> {
        if symbol.is_empty()
            || !symbol
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
            || !symbol
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(AgentError::new(
                ErrorCode::InvalidConstraint,
                format!("invalid dimension symbol `{symbol}`"),
            ));
        }
        if self.parent.contains_key(symbol) {
            return Err(AgentError::new(
                ErrorCode::InvalidConstraint,
                format!("dimension symbol `{symbol}` is already declared"),
            ));
        }
        self.parent.insert(symbol.to_owned(), symbol.to_owned());
        if non_negative {
            self.non_negative.insert(symbol.to_owned());
        }
        Ok(())
    }

    /// Returns accepted facts in deterministic normalized insertion-independent order.
    #[must_use]
    pub fn accepted_facts(&self) -> Vec<ShapeConstraint> {
        self.accepted.iter().cloned().collect()
    }

    fn root(&self, symbol: &str) -> AgentResult<String> {
        let mut current = self.parent.get(symbol).cloned().ok_or_else(|| {
            AgentError::new(
                ErrorCode::InvalidConstraint,
                format!("constraint references undeclared dimension `{symbol}`"),
            )
        })?;
        loop {
            let next = self.parent.get(&current).cloned().ok_or_else(|| {
                AgentError::new(
                    ErrorCode::InvalidConstraint,
                    "invalid constraint fact parent",
                )
            })?;
            if next == current {
                return Ok(current);
            }
            current = next;
        }
    }

    fn normalize_dim(&self, dimension: &DimExpr) -> AgentResult<NormalizedDim> {
        match dimension {
            DimExpr::Static(value) => Ok(NormalizedDim::Static(i128::from(*value))),
            DimExpr::Symbol(symbol) => {
                let root = self.root(symbol)?;
                if let Some(value) = self.static_bindings.get(&root) {
                    Ok(NormalizedDim::Static(i128::from(*value)))
                } else {
                    Ok(NormalizedDim::Affine {
                        coefficient: 1,
                        symbol: root,
                        constant: 0,
                    })
                }
            }
            DimExpr::Affine {
                coefficient,
                symbol,
                constant,
            } => {
                let root = self.root(symbol)?;
                if let Some(value) = self.static_bindings.get(&root) {
                    Ok(NormalizedDim::Static(
                        i128::from(*coefficient) * i128::from(*value) + i128::from(*constant),
                    ))
                } else {
                    Ok(NormalizedDim::Affine {
                        coefficient: *coefficient,
                        symbol: root,
                        constant: *constant,
                    })
                }
            }
        }
    }

    fn normalize_shape(&self, shape: &Shape) -> AgentResult<String> {
        let dimensions = shape
            .0
            .iter()
            .map(|dimension| self.normalize_dim(dimension).map(|value| value.to_string()))
            .collect::<AgentResult<Vec<_>>>()?;
        Ok(format!("[{}]", dimensions.join(",")))
    }

    fn contradiction(
        &self,
        constraint: ShapeConstraint,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> ConstraintQueryResult {
        ConstraintQueryResult::Contradiction {
            contradiction: ConstraintContradiction {
                normalized_constraint: constraint,
                conflicting_facts: self.accepted_facts(),
                expected: expected.into(),
                actual: actual.into(),
            },
        }
    }

    fn query_shapes_raw(&self, left: &Shape, right: &Shape) -> AgentResult<RawQuery> {
        if left.0.len() != right.0.len() {
            return Ok(RawQuery::Contradiction {
                expected: format!("rank {}", left.0.len()),
                actual: format!("rank {}", right.0.len()),
            });
        }
        let mut unknown = false;
        for (left_dimension, right_dimension) in left.0.iter().zip(&right.0) {
            let left_normalized = self.normalize_dim(left_dimension)?;
            let right_normalized = self.normalize_dim(right_dimension)?;
            if left_normalized == right_normalized {
                continue;
            }
            match (&left_normalized, &right_normalized) {
                (NormalizedDim::Static(left), NormalizedDim::Static(right)) => {
                    return Ok(RawQuery::Contradiction {
                        expected: left.to_string(),
                        actual: right.to_string(),
                    });
                }
                (
                    NormalizedDim::Affine {
                        coefficient: left_coefficient,
                        symbol: left_symbol,
                        constant: left_constant,
                    },
                    NormalizedDim::Affine {
                        coefficient: right_coefficient,
                        symbol: right_symbol,
                        constant: right_constant,
                    },
                ) if left_symbol == right_symbol && left_coefficient == right_coefficient => {
                    debug_assert_ne!(left_constant, right_constant);
                    return Ok(RawQuery::Contradiction {
                        expected: left_normalized.to_string(),
                        actual: right_normalized.to_string(),
                    });
                }
                _ => unknown = true,
            }
        }
        if unknown {
            Ok(RawQuery::Unknown)
        } else {
            Ok(RawQuery::Proved)
        }
    }

    /// Queries equality of complete shapes in dimension order.
    pub fn query_shapes(&self, left: &Shape, right: &Shape) -> AgentResult<ConstraintQueryResult> {
        let constraint = ShapeConstraint::Equal {
            left: left.clone(),
            right: right.clone(),
        };
        let reversed = ShapeConstraint::Equal {
            left: right.clone(),
            right: left.clone(),
        };
        if self.accepted.contains(&constraint) || self.accepted.contains(&reversed) {
            return Ok(ConstraintQueryResult::Proved {
                proof: ConstraintProof {
                    normalized_left: self.normalize_shape(left)?,
                    normalized_right: self.normalize_shape(right)?,
                    facts: self.accepted_facts(),
                },
            });
        }
        match self.query_shapes_raw(left, right)? {
            RawQuery::Proved => Ok(ConstraintQueryResult::Proved {
                proof: ConstraintProof {
                    normalized_left: self.normalize_shape(left)?,
                    normalized_right: self.normalize_shape(right)?,
                    facts: self.accepted_facts(),
                },
            }),
            RawQuery::Unknown => Ok(ConstraintQueryResult::Unknown),
            RawQuery::Contradiction { expected, actual } => {
                Ok(self.contradiction(constraint, expected, actual))
            }
        }
    }

    /// Queries compatibility of scalar or tensor types.
    pub fn query_types(&self, left: &Type, right: &Type) -> AgentResult<ConstraintQueryResult> {
        match (left, right) {
            (Type::Scalar(left), Type::Scalar(right)) if left == right => {
                Ok(ConstraintQueryResult::Proved {
                    proof: ConstraintProof {
                        normalized_left: left.to_string(),
                        normalized_right: right.to_string(),
                        facts: self.accepted_facts(),
                    },
                })
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
            ) if left_element == right_element => self.query_shapes(left_shape, right_shape),
            _ => Ok(self.contradiction(
                ShapeConstraint::Equal {
                    left: left.shape().cloned().unwrap_or_default(),
                    right: right.shape().cloned().unwrap_or_default(),
                },
                left.to_string(),
                right.to_string(),
            )),
        }
    }

    fn union(&mut self, left: &str, right: &str) -> AgentResult<()> {
        let left_root = self.root(left)?;
        let right_root = self.root(right)?;
        if left_root == right_root {
            return Ok(());
        }
        let (representative, replaced) = if left_root < right_root {
            (left_root.clone(), right_root.clone())
        } else {
            (right_root.clone(), left_root.clone())
        };
        for parent in self.parent.values_mut() {
            if *parent == replaced {
                parent.clone_from(&representative);
            }
        }
        self.parent.insert(replaced.clone(), representative.clone());
        if self.non_negative.remove(&replaced) {
            self.non_negative.insert(representative.clone());
        }
        match (
            self.static_bindings.remove(&representative),
            self.static_bindings.remove(&replaced),
        ) {
            (Some(left), Some(right)) if left != right => Err(self.contradiction_error(
                &ShapeConstraint::Equal {
                    left: Shape(vec![DimExpr::Symbol(left_root)]),
                    right: Shape(vec![DimExpr::Symbol(right_root)]),
                },
                &left.to_string(),
                &right.to_string(),
            )),
            (Some(value), _) | (_, Some(value)) => {
                self.static_bindings.insert(representative, value);
                Ok(())
            }
            (None, None) => Ok(()),
        }
    }

    fn bind_static(&mut self, symbol: &str, value: u64) -> AgentResult<()> {
        let root = self.root(symbol)?;
        if let Some(existing) = self.static_bindings.get(&root) {
            if *existing != value {
                return Err(self.contradiction_error(
                    &ShapeConstraint::Equal {
                        left: Shape(vec![DimExpr::Symbol(symbol.to_owned())]),
                        right: Shape(vec![DimExpr::Static(value)]),
                    },
                    &existing.to_string(),
                    &value.to_string(),
                ));
            }
        } else {
            self.static_bindings.insert(root, value);
        }
        Ok(())
    }

    fn contradiction_error(
        &self,
        constraint: &ShapeConstraint,
        expected: &str,
        actual: &str,
    ) -> AgentError {
        AgentError::new(
            ErrorCode::ConstraintContradiction,
            "constraint contradicts accepted shape facts",
        )
        .with_types(expected, actual)
        .with_detail("normalized_constraint", json!(constraint))
        .with_detail("conflicting_facts", json!(self.accepted_facts()))
        .with_repair("remove or replace the conflicting shape equality")
    }

    fn validate_constraint_symbols(&self, constraint: &ShapeConstraint) -> AgentResult<()> {
        let mut symbols = BTreeSet::new();
        match constraint {
            ShapeConstraint::Equal { left, right } => {
                for dimension in left.0.iter().chain(&right.0) {
                    if let DimExpr::Symbol(symbol) | DimExpr::Affine { symbol, .. } = dimension {
                        symbols.insert(symbol);
                    }
                }
            }
            ShapeConstraint::NonNegative { symbol } => {
                symbols.insert(symbol);
            }
        }
        let missing = symbols
            .into_iter()
            .filter(|symbol| !self.parent.contains_key(*symbol))
            .cloned()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(AgentError::new(
                ErrorCode::InvalidConstraint,
                "constraint references undeclared dimensions",
            )
            .with_detail("symbols", json!(missing))
            .with_repair("declare every symbolic dimension before adding the constraint"))
        }
    }

    fn add_equality_fact(&mut self, left: &DimExpr, right: &DimExpr) -> AgentResult<()> {
        match (left, right) {
            (DimExpr::Symbol(left), DimExpr::Symbol(right)) => self.union(left, right),
            (DimExpr::Symbol(symbol), DimExpr::Static(value))
            | (DimExpr::Static(value), DimExpr::Symbol(symbol)) => self.bind_static(symbol, *value),
            _ => Ok(()),
        }
    }

    fn verify_all_equalities(&self) -> AgentResult<()> {
        for fact in &self.accepted {
            if let ShapeConstraint::Equal { left, right } = fact {
                if let RawQuery::Contradiction { expected, actual } =
                    self.query_shapes_raw(left, right)?
                {
                    return Err(self.contradiction_error(fact, &expected, &actual));
                }
            }
        }
        Ok(())
    }

    /// Validates and inserts one fact atomically. Duplicate facts are no-ops.
    pub fn insert(&mut self, constraint: &ShapeConstraint) -> AgentResult<ConstraintQueryResult> {
        self.validate_constraint_symbols(constraint)?;
        if self.accepted.contains(constraint) {
            return match constraint {
                ShapeConstraint::Equal { left, right } => self.query_shapes(left, right),
                ShapeConstraint::NonNegative { .. } => Ok(ConstraintQueryResult::Proved {
                    proof: ConstraintProof {
                        normalized_left: "non_negative".to_owned(),
                        normalized_right: "non_negative".to_owned(),
                        facts: self.accepted_facts(),
                    },
                }),
            };
        }
        let mut staged = self.clone();
        match constraint {
            ShapeConstraint::Equal { left, right } => {
                if left.0.len() != right.0.len() {
                    return Err(staged.contradiction_error(
                        constraint,
                        &format!("rank {}", left.0.len()),
                        &format!("rank {}", right.0.len()),
                    ));
                }
                if let RawQuery::Contradiction { expected, actual } =
                    staged.query_shapes_raw(left, right)?
                {
                    return Err(staged.contradiction_error(constraint, &expected, &actual));
                }
                for (left_dimension, right_dimension) in left.0.iter().zip(&right.0) {
                    staged.add_equality_fact(left_dimension, right_dimension)?;
                }
            }
            ShapeConstraint::NonNegative { symbol } => {
                let root = staged.root(symbol)?;
                staged.non_negative.insert(root);
            }
        }
        staged.accepted.insert(constraint.clone());
        staged.verify_all_equalities()?;
        let result = match constraint {
            ShapeConstraint::Equal { left, right } => staged.query_shapes(left, right)?,
            ShapeConstraint::NonNegative { .. } => ConstraintQueryResult::Proved {
                proof: ConstraintProof {
                    normalized_left: "non_negative".to_owned(),
                    normalized_right: "non_negative".to_owned(),
                    facts: staged.accepted_facts(),
                },
            },
        };
        *self = staged;
        Ok(result)
    }
}
