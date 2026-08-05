//! Scalar, tensor, and symbolic shape types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{cmp::Ordering, fmt, str::FromStr};

/// A scalar type supported by the Stage 1 profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    /// Boolean value.
    Bool,
    /// Signed 32-bit integer.
    I32,
    /// IEEE-754 binary32 value.
    F32,
    /// Logical index whose physical width is selected later.
    Index,
}

impl ScalarType {
    /// Whether arithmetic operations accept values of this type.
    #[must_use]
    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::I32 | Self::F32 | Self::Index)
    }
}

impl fmt::Display for ScalarType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::Index => "index",
        })
    }
}

impl FromStr for ScalarType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "bool" => Ok(Self::Bool),
            "i32" => Ok(Self::I32),
            "f32" => Ok(Self::F32),
            "index" => Ok(Self::Index),
            other => Err(format!("unsupported scalar type `{other}`")),
        }
    }
}

/// A single static, symbolic, or affine tensor dimension.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DimExpr {
    /// A known non-negative extent.
    Static(u64),
    /// A named symbolic extent.
    Symbol(String),
    /// The compact affine expression `coefficient * symbol + constant`.
    Affine {
        /// Coefficient of the symbolic dimension.
        coefficient: i64,
        /// Symbol used by this expression.
        symbol: String,
        /// Constant offset.
        constant: i64,
    },
}

impl fmt::Display for DimExpr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(value) => value.fmt(formatter),
            Self::Symbol(value) => formatter.write_str(value),
            Self::Affine {
                coefficient,
                symbol,
                constant,
            } => {
                if *coefficient == 1 {
                    formatter.write_str(symbol)?;
                } else {
                    write!(formatter, "{coefficient}*{symbol}")?;
                }
                match constant.cmp(&0) {
                    Ordering::Greater => write!(formatter, "+{constant}"),
                    Ordering::Less => write!(formatter, "{constant}"),
                    Ordering::Equal => Ok(()),
                }
            }
        }
    }
}

impl FromStr for DimExpr {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let compact: String = value
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let compact = compact.strip_prefix('$').unwrap_or(&compact);
        if compact.is_empty() {
            return Err("dimension expression is empty".to_owned());
        }
        if let Ok(static_value) = compact.parse::<u64>() {
            return Ok(Self::Static(static_value));
        }

        let (coefficient, rest) = if let Some(star) = compact.find('*') {
            let coefficient = compact[..star]
                .parse::<i64>()
                .map_err(|_| format!("invalid affine coefficient in `{compact}`"))?;
            (coefficient, &compact[star + 1..])
        } else {
            (1, compact)
        };
        let symbol_end = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_')
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        let symbol = &rest[..symbol_end];
        if symbol.is_empty()
            || !symbol
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        {
            return Err(format!("invalid symbolic dimension `{compact}`"));
        }
        let constant = if symbol_end == rest.len() {
            0
        } else {
            rest[symbol_end..]
                .parse::<i64>()
                .map_err(|_| format!("invalid affine constant in `{compact}`"))?
        };
        if coefficient == 1 && constant == 0 {
            Ok(Self::Symbol(symbol.to_owned()))
        } else {
            Ok(Self::Affine {
                coefficient,
                symbol: symbol.to_owned(),
                constant,
            })
        }
    }
}

/// Tensor shape in logical dimension order.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Shape(pub Vec<DimExpr>);

impl fmt::Display for Shape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, dimension) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(",")?;
            }
            dimension.fmt(formatter)?;
        }
        formatter.write_str("]")
    }
}

impl FromStr for Shape {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let inner = value
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or_else(|| format!("shape must be bracketed: `{value}`"))?;
        if inner.trim().is_empty() {
            return Ok(Self(Vec::new()));
        }
        inner
            .split(',')
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

impl Serialize for Shape {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A scalar or dense logical tensor type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Type {
    /// Scalar value.
    Scalar(ScalarType),
    /// Tensor with a scalar element type and logical shape.
    Tensor {
        /// Element type.
        element: ScalarType,
        /// Logical tensor shape.
        shape: Shape,
    },
}

impl Type {
    /// Returns the scalar element type for both scalar and tensor values.
    #[must_use]
    pub const fn element_type(&self) -> ScalarType {
        match self {
            Self::Scalar(scalar) => *scalar,
            Self::Tensor { element, .. } => *element,
        }
    }

    /// Returns the tensor shape, if this is a tensor type.
    #[must_use]
    pub const fn shape(&self) -> Option<&Shape> {
        match self {
            Self::Scalar(_) => None,
            Self::Tensor { shape, .. } => Some(shape),
        }
    }

    /// Returns a type with the same shape and a different scalar element type.
    #[must_use]
    pub fn with_element_type(&self, element: ScalarType) -> Self {
        match self {
            Self::Scalar(_) => Self::Scalar(element),
            Self::Tensor { shape, .. } => Self::Tensor {
                element,
                shape: shape.clone(),
            },
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(scalar) => scalar.fmt(formatter),
            Self::Tensor { element, shape } => write!(formatter, "tensor<{element},{shape}>"),
        }
    }
}

impl FromStr for Type {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if let Some(inner) = value
            .strip_prefix("tensor<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let comma = inner
                .find(',')
                .ok_or_else(|| format!("tensor type needs element and shape: `{value}`"))?;
            let element = inner[..comma].parse()?;
            let shape = inner[comma + 1..].parse()?;
            Ok(Self::Tensor { element, shape })
        } else {
            value.parse().map(Self::Scalar)
        }
    }
}

impl Serialize for Type {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Type {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Whether the implementation may contract arithmetic into an FMA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FmaPolicy {
    /// Contraction is forbidden.
    Forbidden,
    /// Contraction is allowed but not required.
    Allowed,
    /// FMA semantics are required.
    Required,
}

/// Determinism requirement for numerical operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    /// Results must be deterministic.
    Required,
    /// Determinism is not part of the contract.
    NotRequired,
}

/// Stage 1 numerical behavior contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumericContract {
    /// FMA contraction policy.
    pub fma: FmaPolicy,
    /// Whether reassociation is permitted.
    pub reassociation: bool,
    /// Result determinism requirement.
    pub determinism: Determinism,
}

impl Default for NumericContract {
    fn default() -> Self {
        Self {
            fma: FmaPolicy::Allowed,
            reassociation: false,
            determinism: Determinism::Required,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DimExpr, Type};

    #[test]
    fn parses_and_prints_tensor_type() {
        let ty: Type = "tensor<f32,[M,2*N+1]>".parse().expect("valid type");
        assert_eq!(ty.to_string(), "tensor<f32,[M,2*N+1]>");
    }

    #[test]
    fn parses_affine_dimension() {
        assert_eq!(
            "N-2".parse::<DimExpr>().expect("valid affine expression"),
            DimExpr::Affine {
                coefficient: 1,
                symbol: "N".to_owned(),
                constant: -2,
            }
        );
    }

    #[test]
    fn canonicalizes_temporary_dimension_syntax() {
        let ty: Type = "tensor<f32,[$N]>".parse().expect("valid type");
        assert_eq!(ty.to_string(), "tensor<f32,[N]>");
    }
}
