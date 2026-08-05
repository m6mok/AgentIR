//! Compact Stage 1 shape reasoning.

use crate::types::{DimExpr, Shape};
use serde::{Deserialize, Serialize};

/// Result of a shape proof attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolverStatus {
    /// The relation follows from the compact solver rules.
    Proved,
    /// The relation is demonstrably false.
    Contradiction,
    /// The solver cannot decide the relation.
    Unknown,
}

/// A shape constraint retained in the program graph.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeConstraint {
    /// Two shape expressions must be equal.
    Equal {
        /// Left-hand logical shape.
        left: Shape,
        /// Right-hand logical shape.
        right: Shape,
    },
    /// A named dimension must be non-negative.
    NonNegative {
        /// Declared dimension symbol.
        symbol: String,
    },
}

fn normalized(dimension: &DimExpr) -> (i64, Option<&str>, i64) {
    match dimension {
        DimExpr::Static(value) => (0, None, i64::try_from(*value).unwrap_or(i64::MAX)),
        DimExpr::Symbol(symbol) => (1, Some(symbol), 0),
        DimExpr::Affine {
            coefficient,
            symbol,
            constant,
        } => (*coefficient, Some(symbol), *constant),
    }
}

/// Checks whether two logical shapes are equal with the compact affine solver.
#[must_use]
pub fn same_shape(left: &Shape, right: &Shape) -> SolverStatus {
    if left.0.len() != right.0.len() {
        return SolverStatus::Contradiction;
    }
    let mut status = SolverStatus::Proved;
    for (left_dimension, right_dimension) in left.0.iter().zip(&right.0) {
        if left_dimension == right_dimension {
            continue;
        }
        if matches!(
            (left_dimension, right_dimension),
            (DimExpr::Static(_), DimExpr::Static(_))
        ) {
            return SolverStatus::Contradiction;
        }
        let left = normalized(left_dimension);
        let right = normalized(right_dimension);
        if left == right {
            continue;
        }
        match (left.1, right.1) {
            (None, None) => return SolverStatus::Contradiction,
            (Some(left_symbol), Some(right_symbol)) if left_symbol == right_symbol => {
                if left.0 == right.0 {
                    return SolverStatus::Contradiction;
                }
                status = SolverStatus::Unknown;
            }
            _ => status = SolverStatus::Unknown,
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::{SolverStatus, same_shape};
    use crate::types::Shape;

    #[test]
    fn proves_identical_symbolic_shapes() {
        let shape: Shape = "[M,N]".parse().expect("valid shape");
        assert_eq!(same_shape(&shape, &shape), SolverStatus::Proved);
    }

    #[test]
    fn rejects_different_static_shapes() {
        let left: Shape = "[4]".parse().expect("valid shape");
        let right: Shape = "[5]".parse().expect("valid shape");
        assert_eq!(same_shape(&left, &right), SolverStatus::Contradiction);
    }

    #[test]
    fn preserves_uncertainty_for_unrelated_symbols() {
        let left: Shape = "[N]".parse().expect("valid shape");
        let right: Shape = "[M]".parse().expect("valid shape");
        assert_eq!(same_shape(&left, &right), SolverStatus::Unknown);
    }
}
