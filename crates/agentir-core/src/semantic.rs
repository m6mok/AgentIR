//! History-independent semantic canonicalization for frozen SpecIR.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{OperationId, ValueId},
    ir::{Opcode, Program, Region, RegionValue, ValueOrigin},
    obligations::ObligationStatus,
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    shapes::ShapeConstraint,
    types::{DimExpr, NumericContract, ScalarType, Shape, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
};

/// Current semantic canonical codec version.
pub const SPEC_CANONICAL_VERSION: u32 = 1;

/// Domain separator used before canonical SpecIR bytes are hashed.
pub const SPEC_HASH_DOMAIN: &[u8] = b"agentir.spec.semantic.v1\0";

const SEMANTIC_CODEC: &str = "agentir.spec.semantic";

/// SHA-256 identity of a frozen specification's semantic canonical form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecHash(String);

impl SpecHash {
    /// Creates a hash from its lowercase hexadecimal representation.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the lowercase hexadecimal digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SpecHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Alpha-normalized static, symbolic, or affine dimension expression.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalDimExpr {
    /// A known extent.
    Static {
        /// Non-negative extent.
        value: u64,
    },
    /// A canonical symbolic dimension such as `d0`.
    Symbol {
        /// Canonical symbol.
        symbol: String,
    },
    /// A compact affine expression over one canonical symbol.
    Affine {
        /// Symbol coefficient.
        coefficient: i64,
        /// Canonical symbol.
        symbol: String,
        /// Constant offset.
        constant: i64,
    },
}

/// Alpha-normalized scalar or tensor type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalType {
    /// Scalar type.
    Scalar {
        /// Scalar element kind.
        scalar: ScalarType,
    },
    /// Dense logical tensor type.
    Tensor {
        /// Tensor element kind.
        element: ScalarType,
        /// Logical shape in dimension order.
        shape: Vec<CanonicalDimExpr>,
    },
}

/// Shape constraint after dimension alpha-normalization.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalConstraint {
    /// Two logical shapes must be equal.
    Equal {
        /// Left shape.
        left: Vec<CanonicalDimExpr>,
        /// Right shape.
        right: Vec<CanonicalDimExpr>,
    },
    /// A canonical symbol is non-negative.
    NonNegative {
        /// Canonical symbol.
        symbol: String,
    },
}

/// Reference to an external parameter or canonical reachable node result.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalValueRef {
    /// External parameter reference.
    Parameter {
        /// Canonical parameter ID such as `p0`.
        parameter: String,
    },
    /// Reachable operation result.
    Node {
        /// Canonical node ID such as `n0`.
        node: String,
        /// Result position for forward-compatible multi-result operations.
        result: usize,
    },
}

/// Reference in an alpha-normalized pure region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalRegionValue {
    /// Ordered block argument.
    Argument {
        /// Canonical argument name such as `%arg0`.
        argument: String,
    },
    /// Earlier region-local SSA result.
    Local {
        /// Canonical local name such as `%local0`.
        local: String,
    },
    /// Canonical reference to an actually used outer value.
    Outer {
        /// Reachable outer value.
        value: CanonicalValueRef,
    },
}

/// One canonical region block argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalRegionArgument {
    /// Canonical argument name.
    pub name: String,
    /// Normalized argument type.
    pub ty: CanonicalType,
}

/// One canonical region-local SSA operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRegionOperation {
    /// Canonical result name.
    pub result: String,
    /// Operation semantics.
    pub opcode: Opcode,
    /// Ordered operands.
    pub operands: Vec<CanonicalRegionValue>,
    /// Stable semantic attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Inferred result type.
    pub result_type: CanonicalType,
}

/// Alpha-normalized pure region attached to a reachable node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRegion {
    /// Ordered canonical block arguments.
    pub arguments: Vec<CanonicalRegionArgument>,
    /// Region operations in semantic execution order.
    pub operations: Vec<CanonicalRegionOperation>,
    /// Canonical yielded value.
    pub yield_value: CanonicalRegionValue,
    /// Verified yielded type.
    pub yield_type: CanonicalType,
}

/// One external parameter in sorted interface order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalParameter {
    /// Canonical parameter ID.
    pub id: String,
    /// External interface name.
    pub name: String,
    /// Parameter type.
    pub ty: CanonicalType,
}

/// One named output in sorted interface order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalOutput {
    /// External interface name.
    pub name: String,
    /// Inferred output type.
    pub ty: CanonicalType,
    /// Canonical expression producing the output.
    pub value: CanonicalValueRef,
}

/// One reachable operation in dependency-first canonical order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalNode {
    /// Canonical node ID.
    pub id: String,
    /// Canonical opcode.
    pub opcode: Opcode,
    /// Ordered dependency references.
    pub operands: Vec<CanonicalValueRef>,
    /// Stable semantic attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Optional canonical pure region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<CanonicalRegion>,
    /// Inferred result types in result order.
    pub result_types: Vec<CanonicalType>,
}

/// Version 1 semantic canonical representation of a complete frozen SpecIR.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticCanonicalProgramV1 {
    /// Stable codec discriminator.
    pub codec: String,
    /// Semantic canonical codec version.
    pub version: u32,
    /// Sorted external parameter interface, including unused parameters.
    pub parameters: Vec<CanonicalParameter>,
    /// Sorted output interface.
    pub outputs: Vec<CanonicalOutput>,
    /// Reachable dependency graph in canonical order.
    pub nodes: Vec<CanonicalNode>,
    /// Relevant normalized shape constraints.
    pub constraints: Vec<CanonicalConstraint>,
    /// Explicit numerical semantics.
    pub numeric_contract: NumericContract,
}

/// Canonical representation, exact codec bytes, and domain-separated hash.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticCanonicalization {
    /// Versioned semantic representation.
    pub canonical: SemanticCanonicalProgramV1,
    /// Deterministic compact JSON codec bytes.
    pub bytes: Vec<u8>,
    /// Domain-separated semantic identity.
    pub spec_hash: SpecHash,
}

struct Canonicalizer<'a> {
    program: &'a Program,
    dimensions: BTreeMap<String, String>,
    referenced_symbols: BTreeSet<String>,
    parameters: BTreeMap<ValueId, String>,
    nodes_by_operation: BTreeMap<OperationId, usize>,
    visiting: BTreeSet<OperationId>,
    nodes: Vec<CanonicalNode>,
}

impl<'a> Canonicalizer<'a> {
    fn new(program: &'a Program) -> Self {
        Self {
            program,
            dimensions: BTreeMap::new(),
            referenced_symbols: BTreeSet::new(),
            parameters: BTreeMap::new(),
            nodes_by_operation: BTreeMap::new(),
            visiting: BTreeSet::new(),
            nodes: Vec::new(),
        }
    }

    fn failure(message: impl Into<String>) -> AgentError {
        AgentError::new(ErrorCode::CanonicalizationFailed, message)
    }

    fn canonical_symbol(&mut self, symbol: &str) -> AgentResult<String> {
        if !self.program.dimension_names.contains_key(symbol) {
            return Err(Self::failure(format!(
                "semantic type or constraint references undeclared dimension `{symbol}`"
            )));
        }
        self.referenced_symbols.insert(symbol.to_owned());
        let next = self.dimensions.len();
        Ok(self
            .dimensions
            .entry(symbol.to_owned())
            .or_insert_with(|| format!("d{next}"))
            .clone())
    }

    fn canonical_dim(&mut self, dimension: &DimExpr) -> AgentResult<CanonicalDimExpr> {
        match dimension {
            DimExpr::Static(value) => Ok(CanonicalDimExpr::Static { value: *value }),
            DimExpr::Symbol(symbol) => Ok(CanonicalDimExpr::Symbol {
                symbol: self.canonical_symbol(symbol)?,
            }),
            DimExpr::Affine {
                coefficient,
                symbol,
                constant,
            } => Ok(CanonicalDimExpr::Affine {
                coefficient: *coefficient,
                symbol: self.canonical_symbol(symbol)?,
                constant: *constant,
            }),
        }
    }

    fn canonical_shape(&mut self, shape: &Shape) -> AgentResult<Vec<CanonicalDimExpr>> {
        shape
            .0
            .iter()
            .map(|dimension| self.canonical_dim(dimension))
            .collect()
    }

    fn canonical_type(&mut self, ty: &Type) -> AgentResult<CanonicalType> {
        match ty {
            Type::Scalar(scalar) => Ok(CanonicalType::Scalar { scalar: *scalar }),
            Type::Tensor { element, shape } => Ok(CanonicalType::Tensor {
                element: *element,
                shape: self.canonical_shape(shape)?,
            }),
        }
    }

    fn stable_attribute(value: &JsonValue) -> AgentResult<JsonValue> {
        match value {
            JsonValue::Array(values) => values
                .iter()
                .map(Self::stable_attribute)
                .collect::<AgentResult<Vec<_>>>()
                .map(JsonValue::Array),
            JsonValue::Object(values) => values
                .iter()
                .map(|(key, value)| Ok((key.clone(), Self::stable_attribute(value)?)))
                .collect::<AgentResult<serde_json::Map<_, _>>>()
                .map(JsonValue::Object),
            JsonValue::String(value) if looks_like_persistent_reference(value) => {
                Err(Self::failure(format!(
                    "semantic attribute contains unresolved persistent reference `{value}`"
                )))
            }
            _ => Ok(value.clone()),
        }
    }

    fn stable_attributes(
        attributes: &BTreeMap<String, JsonValue>,
    ) -> AgentResult<BTreeMap<String, JsonValue>> {
        attributes
            .iter()
            .map(|(key, value)| Ok((key.clone(), Self::stable_attribute(value)?)))
            .collect()
    }

    fn region_value(
        &mut self,
        value: &RegionValue,
        arguments: &BTreeMap<String, String>,
        locals: &BTreeMap<String, String>,
    ) -> AgentResult<CanonicalRegionValue> {
        match value {
            RegionValue::Argument(argument) => arguments
                .get(argument)
                .cloned()
                .map(|argument| CanonicalRegionValue::Argument { argument })
                .ok_or_else(|| Self::failure(format!("unknown region argument `{argument}`"))),
            RegionValue::Local(local) => locals
                .get(local)
                .cloned()
                .map(|local| CanonicalRegionValue::Local { local })
                .ok_or_else(|| Self::failure(format!("unknown region local `{local}`"))),
            RegionValue::Capture(value) => Ok(CanonicalRegionValue::Outer {
                value: self.value_ref(value)?,
            }),
        }
    }

    fn canonical_region(&mut self, region: &Region) -> AgentResult<CanonicalRegion> {
        let mut argument_names = BTreeMap::new();
        let mut arguments = Vec::with_capacity(region.arguments.len());
        for (index, argument) in region.arguments.iter().enumerate() {
            let name = format!("%arg{index}");
            if argument_names
                .insert(argument.name.clone(), name.clone())
                .is_some()
            {
                return Err(Self::failure(format!(
                    "duplicate region argument `{}`",
                    argument.name
                )));
            }
            arguments.push(CanonicalRegionArgument {
                name,
                ty: self.canonical_type(&argument.ty)?,
            });
        }

        let mut local_names = BTreeMap::new();
        let mut operations = Vec::with_capacity(region.operations.len());
        for (index, operation) in region.operations.iter().enumerate() {
            let result = format!("%local{index}");
            let operands = operation
                .operands
                .iter()
                .map(|value| self.region_value(value, &argument_names, &local_names))
                .collect::<AgentResult<_>>()?;
            if local_names
                .insert(operation.result.clone(), result.clone())
                .is_some()
            {
                return Err(Self::failure(format!(
                    "duplicate region local `{}`",
                    operation.result
                )));
            }
            operations.push(CanonicalRegionOperation {
                result,
                opcode: operation.opcode,
                operands,
                attributes: Self::stable_attributes(&operation.attributes)?,
                result_type: self.canonical_type(&operation.result_type)?,
            });
        }
        Ok(CanonicalRegion {
            arguments,
            operations,
            yield_value: self.region_value(&region.yield_value, &argument_names, &local_names)?,
            yield_type: self.canonical_type(&region.yield_type)?,
        })
    }

    fn value_ref(&mut self, value: &ValueId) -> AgentResult<CanonicalValueRef> {
        if let Some(parameter) = self.parameters.get(value) {
            return Ok(CanonicalValueRef::Parameter {
                parameter: parameter.clone(),
            });
        }
        let definition = self.program.values.get(value).ok_or_else(|| {
            Self::failure(format!("semantic graph references missing value `{value}`"))
        })?;
        let operation_id = match &definition.origin {
            ValueOrigin::Hole(hole_id) => {
                let filled = self
                    .program
                    .holes
                    .get(hole_id)
                    .and_then(|hole| hole.filled_with.as_ref())
                    .ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::SpecNotComplete,
                            format!("semantic graph reaches open hole `{hole_id}`"),
                        )
                    })?;
                return self.value_ref(filled);
            }
            ValueOrigin::Operation(operation) => operation,
        };
        let operation = self.program.operations.get(operation_id).ok_or_else(|| {
            Self::failure(format!(
                "value `{value}` references missing operation `{operation_id}`"
            ))
        })?;
        let result = operation
            .results
            .iter()
            .position(|candidate| candidate == value)
            .ok_or_else(|| {
                Self::failure(format!(
                    "value `{value}` is not a result of operation `{operation_id}`"
                ))
            })?;
        if operation.opcode == Opcode::Parameter {
            return Err(Self::failure(format!(
                "reachable parameter value `{value}` is absent from the external interface"
            )));
        }
        if let Some(node) = self.nodes_by_operation.get(operation_id) {
            return Ok(CanonicalValueRef::Node {
                node: format!("n{node}"),
                result,
            });
        }
        if !self.visiting.insert(operation_id.clone()) {
            return Err(Self::failure(format!(
                "semantic graph contains a cycle at operation `{operation_id}`"
            )));
        }

        let operands = operation
            .operands
            .iter()
            .map(|operand| self.value_ref(operand))
            .collect::<AgentResult<_>>()?;
        let region = operation
            .region
            .as_ref()
            .map(|region| self.canonical_region(region))
            .transpose()?;
        let result_types: Vec<_> = operation
            .result_types
            .iter()
            .map(|ty| self.canonical_type(ty))
            .collect::<AgentResult<_>>()?;
        if operation.results.len() != result_types.len() {
            return Err(Self::failure(format!(
                "operation `{operation_id}` has mismatched result and type counts"
            )));
        }
        let node = self.nodes.len();
        self.nodes.push(CanonicalNode {
            id: format!("n{node}"),
            opcode: operation.opcode,
            operands,
            attributes: Self::stable_attributes(&operation.attributes)?,
            region,
            result_types,
        });
        self.nodes_by_operation.insert(operation_id.clone(), node);
        self.visiting.remove(operation_id);
        Ok(CanonicalValueRef::Node {
            node: format!("n{node}"),
            result,
        })
    }

    fn canonical_constraint(
        &mut self,
        constraint: &ShapeConstraint,
    ) -> AgentResult<CanonicalConstraint> {
        match constraint {
            ShapeConstraint::Equal { left, right } => Ok(CanonicalConstraint::Equal {
                left: self.canonical_shape(left)?,
                right: self.canonical_shape(right)?,
            }),
            ShapeConstraint::NonNegative { symbol } => Ok(CanonicalConstraint::NonNegative {
                symbol: self.canonical_symbol(symbol)?,
            }),
        }
    }

    fn relevant_constraints(&mut self) -> AgentResult<Vec<CanonicalConstraint>> {
        let mut included = vec![false; self.program.constraints.len()];
        loop {
            let mut changed = false;
            for (index, constraint) in self.program.constraints.iter().enumerate() {
                if included[index] {
                    continue;
                }
                let symbols = constraint_symbols(constraint);
                if symbols.is_empty()
                    || symbols
                        .iter()
                        .any(|symbol| self.referenced_symbols.contains(*symbol))
                {
                    included[index] = true;
                    self.referenced_symbols
                        .extend(symbols.into_iter().map(str::to_owned));
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut relevant = self
            .program
            .constraints
            .iter()
            .enumerate()
            .filter(|(index, _)| included[*index])
            .map(|(_, constraint)| constraint)
            .collect::<Vec<_>>();
        relevant.sort_by_key(|constraint| constraint_signature(constraint, &self.dimensions));

        let mut constraints = relevant
            .into_iter()
            .map(|constraint| self.canonical_constraint(constraint))
            .collect::<AgentResult<Vec<_>>>()?;
        let mapped_dimensions = self.dimensions.clone();
        for (source, canonical) in mapped_dimensions {
            let dimension_id = self
                .program
                .dimension_names
                .get(&source)
                .ok_or_else(|| Self::failure(format!("undeclared dimension `{source}`")))?;
            let dimension = self.program.dimensions.get(dimension_id).ok_or_else(|| {
                Self::failure(format!("missing dimension declaration `{dimension_id}`"))
            })?;
            if dimension.non_negative {
                constraints.push(CanonicalConstraint::NonNegative { symbol: canonical });
            }
        }
        constraints.sort();
        constraints.dedup();
        Ok(constraints)
    }

    fn build(mut self) -> AgentResult<SemanticCanonicalProgramV1> {
        let mut parameters = Vec::with_capacity(self.program.parameters.len());
        for (index, (name, value)) in self.program.parameters.iter().enumerate() {
            let definition = self.program.values.get(value).ok_or_else(|| {
                Self::failure(format!(
                    "parameter `{name}` references missing value `{value}`"
                ))
            })?;
            let id = format!("p{index}");
            self.parameters.insert(value.clone(), id.clone());
            parameters.push(CanonicalParameter {
                id,
                name: name.clone(),
                ty: self.canonical_type(&definition.ty)?,
            });
        }

        let mut output_types = Vec::with_capacity(self.program.outputs.len());
        for (name, value) in &self.program.outputs {
            let definition = self.program.values.get(value).ok_or_else(|| {
                Self::failure(format!(
                    "output `{name}` references missing value `{value}`"
                ))
            })?;
            output_types.push((
                name.clone(),
                value.clone(),
                self.canonical_type(&definition.ty)?,
            ));
        }
        let outputs = output_types
            .into_iter()
            .map(|(name, value, ty)| {
                Ok(CanonicalOutput {
                    name,
                    ty,
                    value: self.value_ref(&value)?,
                })
            })
            .collect::<AgentResult<_>>()?;
        let constraints = self.relevant_constraints()?;
        Ok(SemanticCanonicalProgramV1 {
            codec: SEMANTIC_CODEC.to_owned(),
            version: SPEC_CANONICAL_VERSION,
            parameters,
            outputs,
            nodes: self.nodes,
            constraints,
            numeric_contract: self.program.numeric_contract.clone(),
        })
    }
}

fn looks_like_persistent_reference(value: &str) -> bool {
    let value = value.strip_prefix('@').unwrap_or(value);
    ["op", "tx", "cf", "v", "h", "o", "r", "a", "d"]
        .into_iter()
        .any(|prefix| {
            value.strip_prefix(prefix).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
            })
        })
}

fn shape_symbols(shape: &Shape) -> BTreeSet<&str> {
    shape
        .0
        .iter()
        .filter_map(|dimension| match dimension {
            DimExpr::Static(_) => None,
            DimExpr::Symbol(symbol) | DimExpr::Affine { symbol, .. } => Some(symbol.as_str()),
        })
        .collect()
}

fn constraint_symbols(constraint: &ShapeConstraint) -> BTreeSet<&str> {
    match constraint {
        ShapeConstraint::Equal { left, right } => {
            let mut symbols = shape_symbols(left);
            symbols.extend(shape_symbols(right));
            symbols
        }
        ShapeConstraint::NonNegative { symbol } => BTreeSet::from([symbol.as_str()]),
    }
}

fn constraint_signature(constraint: &ShapeConstraint, known: &BTreeMap<String, String>) -> String {
    fn dimension_signature(
        dimension: &DimExpr,
        known: &BTreeMap<String, String>,
        local: &mut BTreeMap<String, String>,
    ) -> String {
        match dimension {
            DimExpr::Static(value) => format!("s{value}"),
            DimExpr::Symbol(symbol) => {
                let next = local.len();
                let symbol = known.get(symbol).cloned().unwrap_or_else(|| {
                    local
                        .entry(symbol.clone())
                        .or_insert_with(|| format!("u{next}"))
                        .clone()
                });
                format!("y{symbol}")
            }
            DimExpr::Affine {
                coefficient,
                symbol,
                constant,
            } => {
                let next = local.len();
                let symbol = known.get(symbol).cloned().unwrap_or_else(|| {
                    local
                        .entry(symbol.clone())
                        .or_insert_with(|| format!("u{next}"))
                        .clone()
                });
                format!("a{coefficient}:{symbol}:{constant}")
            }
        }
    }

    let mut local = BTreeMap::new();
    match constraint {
        ShapeConstraint::Equal { left, right } => {
            let left = left
                .0
                .iter()
                .map(|dimension| dimension_signature(dimension, known, &mut local))
                .collect::<Vec<_>>()
                .join(",");
            let right = right
                .0
                .iter()
                .map(|dimension| dimension_signature(dimension, known, &mut local))
                .collect::<Vec<_>>()
                .join(",");
            format!("equal:{left}={right}")
        }
        ShapeConstraint::NonNegative { symbol } => {
            let symbol = known.get(symbol).map_or("u0", String::as_str);
            format!("non_negative:{symbol}")
        }
    }
}

fn validate_complete(program: &Program) -> AgentResult<()> {
    if !program.frozen {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "semantic canonicalization requires a frozen specification",
        ));
    }
    if program.outputs.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "semantic canonicalization requires at least one output",
        ));
    }
    let open_holes = program
        .holes
        .iter()
        .filter(|(_, hole)| hole.filled_with.is_none())
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>();
    if !open_holes.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "semantic canonicalization requires every hole to be filled",
        )
        .with_detail("holes", serde_json::json!(open_holes)));
    }
    let open_obligations = program
        .obligations
        .iter()
        .filter(|(_, obligation)| matches!(obligation.status, ObligationStatus::Open))
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>();
    if !open_obligations.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "semantic canonicalization requires discharged obligations",
        )
        .with_detail("obligations", serde_json::json!(open_obligations)));
    }
    Ok(())
}

fn digest_hex(bytes: &[u8]) -> SpecHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    SpecHash(output)
}

/// Builds semantic canonical form, deterministic bytes, and a domain-separated hash.
pub fn canonicalize_spec(program: &Program) -> AgentResult<SemanticCanonicalization> {
    canonicalize_spec_with_limit(
        program,
        ResourceLimits::hard_safety_caps().canonical_output_bytes,
    )
}

/// Builds semantic canonical form while enforcing an encoded byte limit.
pub fn canonicalize_spec_with_limit(
    program: &Program,
    max_bytes: u64,
) -> AgentResult<SemanticCanonicalization> {
    validate_complete(program)?;
    let canonical = Canonicalizer::new(program).build()?;
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        AgentError::new(
            ErrorCode::CanonicalizationFailed,
            format!("semantic canonical serialization failed: {error}"),
        )
    })?;
    BudgetCheck::ensure(
        ResourceKind::CanonicalOutputBytes,
        max_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "semantic canonical serialization",
    )?;
    let mut hash_input = Vec::with_capacity(SPEC_HASH_DOMAIN.len() + bytes.len());
    hash_input.extend_from_slice(SPEC_HASH_DOMAIN);
    hash_input.extend_from_slice(&bytes);
    let spec_hash = digest_hex(&hash_input);
    Ok(SemanticCanonicalization {
        canonical,
        bytes,
        spec_hash,
    })
}

/// Computes only the history-independent semantic hash of a frozen SpecIR.
pub fn spec_hash(program: &Program) -> AgentResult<SpecHash> {
    Ok(canonicalize_spec(program)?.spec_hash)
}
