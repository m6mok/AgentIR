//! Separate typed implementation graph, verifier, identity lowering, and semantic hash.

use crate::{
    candidate::CandidateAllocator,
    constraints::ConstraintFacts,
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{ActionId, DimensionId, ImplOperationId, ImplValueId, OperationId, ValueId},
    ir::{
        BlockArgument, ConstantValue, Dimension, Opcode, Operation, Program, Region,
        RegionOperation, RegionValue, ValueDef, ValueOrigin,
    },
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    semantic::{SemanticCanonicalProgramV1, canonicalize_spec_with_limit},
    shapes::ShapeConstraint,
    spec::{infer_higher_with_facts, infer_primitive_with_facts},
    types::{NumericContract, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
};

/// Current history-independent ImplIR canonical codec version.
pub const IMPL_CANONICAL_VERSION: u32 = 1;

/// Current exact ImplIR evaluator/verifier semantics version.
pub const IMPL_SEMANTICS_VERSION: u32 = 1;

/// Domain separator for history-independent implementation hashes.
pub const IMPL_HASH_DOMAIN: &[u8] = b"agentir.impl.semantic.v1\0";

const IMPL_CANONICAL_CODEC: &str = "agentir.impl.semantic";

/// SHA-256 identity of an implementation's reachable typed semantics.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImplHash(String);

impl ImplHash {
    /// Creates a hash from a lowercase hexadecimal digest.
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

impl fmt::Display for ImplHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Compiler-owned provenance from ImplIR back to SpecIR or a trusted rewrite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplSourceLink {
    /// Source SpecIR operation, when this node descends from one operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_operation: Option<OperationId>,
    /// Source SpecIR result value, when this node descends from one value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_value: Option<ValueId>,
    /// Trusted rule that most recently rewrote this node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite_rule: Option<String>,
}

/// Source of one ImplIR SSA value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ImplValueOrigin {
    /// Result of an ImplIR operation.
    Operation(ImplOperationId),
}

/// One typed SSA value in the separate implementation graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplValue {
    /// Compiler-assigned persistent implementation value ID.
    pub id: ImplValueId,
    /// Compiler-inferred type.
    pub ty: Type,
    /// Defining implementation operation.
    pub origin: ImplValueOrigin,
    /// Optional external-facing name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Source or rewrite provenance.
    pub source_link: ImplSourceLink,
}

/// Reference used inside a pure ImplIR region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ImplRegionValue {
    /// Ordered block argument.
    Argument(String),
    /// Earlier local SSA result.
    Local(String),
    /// Explicit outer implementation value.
    Capture(ImplValueId),
}

/// One typed block argument in an implementation region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplBlockArgument {
    /// Local argument name.
    pub name: String,
    /// Argument type.
    pub ty: Type,
}

/// One pure local operation in an implementation region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplRegionOperation {
    /// Local SSA result name.
    pub result: String,
    /// Exact opcode.
    pub opcode: Opcode,
    /// Ordered operands.
    pub operands: Vec<ImplRegionValue>,
    /// Stable semantic attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Inferred local result type.
    pub result_type: Type,
}

/// Closed pure region owned by one implementation operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplRegion {
    /// Ordered block arguments.
    pub arguments: Vec<ImplBlockArgument>,
    /// Explicit outer captures.
    pub captures: Vec<ImplValueId>,
    /// Ordered local SSA operations.
    pub operations: Vec<ImplRegionOperation>,
    /// Yielded local/argument/capture value.
    pub yield_value: ImplRegionValue,
    /// Verified yielded type.
    pub yield_type: Type,
}

/// One operation in the separate implementation graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplOperation {
    /// Compiler-assigned persistent operation ID.
    pub id: ImplOperationId,
    /// Exact opcode.
    pub opcode: Opcode,
    /// Ordered operand values.
    pub operands: Vec<ImplValueId>,
    /// Ordered result values.
    pub results: Vec<ImplValueId>,
    /// Stable semantic attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, JsonValue>,
    /// Optional closed pure region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ImplRegion>,
    /// Compiler-inferred result types.
    pub result_types: Vec<Type>,
    /// Source or trusted rewrite provenance.
    pub source_link: ImplSourceLink,
}

/// One named external implementation output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplOutput {
    /// External output name.
    pub name: String,
    /// Implementation value returned for the name.
    pub value: ImplValueId,
    /// Verified output type.
    pub ty: Type,
}

/// Functional, transport-independent implementation graph for Stage 2A.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImplProgram {
    /// Logical dimension names and their non-negative declaration bit.
    pub dimensions: BTreeMap<String, bool>,
    /// Top-level operations by persistent implementation ID.
    pub operations: BTreeMap<ImplOperationId, ImplOperation>,
    /// Explicit dependency-before-use operation order.
    pub operation_order: Vec<ImplOperationId>,
    /// SSA values by persistent implementation ID.
    pub values: BTreeMap<ImplValueId, ImplValue>,
    /// External parameter name-to-value mapping.
    pub parameters: BTreeMap<String, ImplValueId>,
    /// Exact scalar constants by result value.
    pub constants: BTreeMap<ImplValueId, ConstantValue>,
    /// External outputs by name.
    pub outputs: BTreeMap<String, ImplOutput>,
    /// Accepted logical shape constraints copied from frozen SpecIR.
    pub constraints: Vec<ShapeConstraint>,
    /// Numeric semantics restricting all rewrites.
    pub numeric_contract: NumericContract,
}

/// Versioned canonical ImplIR model used to compute `impl_hash`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplCanonicalProgramV1 {
    /// Stable codec discriminator.
    pub codec: String,
    /// Canonical model version.
    pub version: u32,
    /// Alpha-normalized reachable typed graph.
    pub graph: SemanticCanonicalProgramV1,
}

/// Canonical model, deterministic bytes, and history-independent hash.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplCanonicalization {
    /// Structured canonical model.
    pub canonical: ImplCanonicalProgramV1,
    /// Compact deterministic JSON bytes.
    pub bytes: Vec<u8>,
    /// Domain-separated implementation hash.
    pub impl_hash: ImplHash,
}

fn impl_error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::ImplVerificationFailed, message)
}

fn verify_source_link(link: &ImplSourceLink, source: &Program, context: &str) -> AgentResult<()> {
    let source_operation = link
        .spec_operation
        .as_ref()
        .map(|operation| {
            source.operations.get(operation).ok_or_else(|| {
                impl_error(format!(
                    "{context} references missing SpecIR operation `{operation}`"
                ))
            })
        })
        .transpose()?;
    let source_value = link
        .spec_value
        .as_ref()
        .map(|value| {
            source.values.get(value).ok_or_else(|| {
                impl_error(format!(
                    "{context} references missing SpecIR value `{value}`"
                ))
            })
        })
        .transpose()?;
    if let (Some(operation), Some(value)) = (source_operation, source_value) {
        if value.origin != ValueOrigin::Operation(operation.id.clone()) {
            return Err(impl_error(format!(
                "{context} SpecIR operation/value source link is inconsistent"
            )));
        }
    }
    if link.spec_operation.is_none() && link.spec_value.is_none() && link.rewrite_rule.is_none() {
        return Err(impl_error(format!(
            "{context} lacks source or trusted rewrite provenance"
        )));
    }
    Ok(())
}

fn convert_region_value_to_impl(
    value: &RegionValue,
    values: &BTreeMap<ValueId, ImplValueId>,
) -> AgentResult<ImplRegionValue> {
    match value {
        RegionValue::Argument(name) => Ok(ImplRegionValue::Argument(name.clone())),
        RegionValue::Local(name) => Ok(ImplRegionValue::Local(name.clone())),
        RegionValue::Capture(value) => values
            .get(value)
            .cloned()
            .map(ImplRegionValue::Capture)
            .ok_or_else(|| impl_error(format!("identity lowering cannot map capture `{value}`"))),
    }
}

fn lower_region(
    region: &Region,
    values: &BTreeMap<ValueId, ImplValueId>,
) -> AgentResult<ImplRegion> {
    Ok(ImplRegion {
        arguments: region
            .arguments
            .iter()
            .map(|argument| ImplBlockArgument {
                name: argument.name.clone(),
                ty: argument.ty.clone(),
            })
            .collect(),
        captures: region
            .captures
            .iter()
            .map(|value| {
                values.get(value).cloned().ok_or_else(|| {
                    impl_error(format!("identity lowering cannot map capture `{value}`"))
                })
            })
            .collect::<AgentResult<_>>()?,
        operations: region
            .operations
            .iter()
            .map(|operation| {
                Ok(ImplRegionOperation {
                    result: operation.result.clone(),
                    opcode: operation.opcode,
                    operands: operation
                        .operands
                        .iter()
                        .map(|value| convert_region_value_to_impl(value, values))
                        .collect::<AgentResult<_>>()?,
                    attributes: operation.attributes.clone(),
                    result_type: operation.result_type.clone(),
                })
            })
            .collect::<AgentResult<_>>()?,
        yield_value: convert_region_value_to_impl(&region.yield_value, values)?,
        yield_type: region.yield_type.clone(),
    })
}

fn visit_source_value(
    program: &Program,
    value: &ValueId,
    seen: &mut BTreeSet<OperationId>,
    order: &mut Vec<OperationId>,
) -> AgentResult<()> {
    let value = program.resolve_filled_value(value);
    let definition = program
        .values
        .get(value)
        .ok_or_else(|| impl_error(format!("SpecIR value `{value}` is missing")))?;
    let ValueOrigin::Operation(operation) = &definition.origin else {
        return Err(impl_error("frozen SpecIR contains a hole-backed value"));
    };
    if seen.contains(operation) {
        return Ok(());
    }
    let source = program
        .operations
        .get(operation)
        .ok_or_else(|| impl_error(format!("SpecIR operation `{operation}` is missing")))?;
    for operand in &source.operands {
        visit_source_value(program, operand, seen, order)?;
    }
    if let Some(region) = &source.region {
        for capture in &region.captures {
            visit_source_value(program, capture, seen, order)?;
        }
    }
    seen.insert(operation.clone());
    order.push(operation.clone());
    Ok(())
}

/// Deterministically lowers a complete frozen SpecIR into a separate identity ImplIR.
pub fn identity_lower(
    source: &Program,
    allocator: &mut CandidateAllocator,
) -> AgentResult<ImplProgram> {
    if !source.frozen || source.outputs.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotFrozen,
            "candidate creation requires a complete frozen SpecIR",
        ));
    }
    if source.holes.values().any(|hole| hole.filled_with.is_none())
        || source.obligations.values().any(|obligation| {
            matches!(
                obligation.status,
                crate::obligations::ObligationStatus::Open
            )
        })
    {
        return Err(AgentError::new(
            ErrorCode::SpecNotFrozen,
            "candidate creation requires all SpecIR holes and obligations to be closed",
        ));
    }

    let mut source_order = Vec::new();
    let mut seen = BTreeSet::new();
    for value in source.parameters.values() {
        visit_source_value(source, value, &mut seen, &mut source_order)?;
    }
    for value in source.outputs.values() {
        visit_source_value(source, value, &mut seen, &mut source_order)?;
    }

    let mut result = ImplProgram {
        dimensions: source
            .dimensions
            .values()
            .map(|dimension| (dimension.name.clone(), dimension.non_negative))
            .collect(),
        constraints: source.constraints.clone(),
        numeric_contract: source.numeric_contract.clone(),
        ..ImplProgram::default()
    };
    let mut value_map = BTreeMap::<ValueId, ImplValueId>::new();
    for source_operation_id in source_order {
        let source_operation = source
            .operations
            .get(&source_operation_id)
            .ok_or_else(|| impl_error("source operation disappeared during lowering"))?;
        let operation_id = allocator.impl_operation();
        let mut result_ids = Vec::new();
        for source_value_id in &source_operation.results {
            let source_value = source.values.get(source_value_id).ok_or_else(|| {
                impl_error(format!("source result `{source_value_id}` is missing"))
            })?;
            let value_id = allocator.impl_value();
            value_map.insert(source_value_id.clone(), value_id.clone());
            result.values.insert(
                value_id.clone(),
                ImplValue {
                    id: value_id.clone(),
                    ty: source_value.ty.clone(),
                    origin: ImplValueOrigin::Operation(operation_id.clone()),
                    name: source_value.name.clone(),
                    source_link: ImplSourceLink {
                        spec_operation: Some(source_operation_id.clone()),
                        spec_value: Some(source_value_id.clone()),
                        rewrite_rule: None,
                    },
                },
            );
            if let Some(constant) = source.constants.get(source_value_id) {
                result.constants.insert(value_id.clone(), constant.clone());
            }
            result_ids.push(value_id);
        }
        let operands = source_operation
            .operands
            .iter()
            .map(|value| {
                value_map
                    .get(source.resolve_filled_value(value))
                    .cloned()
                    .ok_or_else(|| {
                        impl_error(format!("identity lowering cannot map operand `{value}`"))
                    })
            })
            .collect::<AgentResult<Vec<_>>>()?;
        let region = source_operation
            .region
            .as_ref()
            .map(|region| lower_region(region, &value_map))
            .transpose()?;
        result.operations.insert(
            operation_id.clone(),
            ImplOperation {
                id: operation_id.clone(),
                opcode: source_operation.opcode,
                operands,
                results: result_ids,
                attributes: source_operation.attributes.clone(),
                region,
                result_types: source_operation.result_types.clone(),
                source_link: ImplSourceLink {
                    spec_operation: Some(source_operation_id),
                    spec_value: source_operation.results.first().cloned(),
                    rewrite_rule: None,
                },
            },
        );
        result.operation_order.push(operation_id);
    }
    result.parameters = source
        .parameters
        .iter()
        .map(|(name, value)| {
            value_map
                .get(source.resolve_filled_value(value))
                .cloned()
                .map(|value| (name.clone(), value))
                .ok_or_else(|| impl_error(format!("identity lowering missed parameter `{name}`")))
        })
        .collect::<AgentResult<_>>()?;
    result.outputs = source
        .outputs
        .iter()
        .map(|(name, value)| {
            let value = value_map
                .get(source.resolve_filled_value(value))
                .cloned()
                .ok_or_else(|| impl_error(format!("identity lowering missed output `{name}`")))?;
            let ty = result
                .values
                .get(&value)
                .expect("newly lowered value exists")
                .ty
                .clone();
            Ok((
                name.clone(),
                ImplOutput {
                    name: name.clone(),
                    value,
                    ty,
                },
            ))
        })
        .collect::<AgentResult<_>>()?;
    Ok(result)
}

fn to_spec_value(value: &ImplValueId) -> ValueId {
    ValueId::new(value.as_str())
}

fn to_spec_operation(operation: &ImplOperationId) -> OperationId {
    OperationId::new(operation.as_str())
}

fn region_value_to_spec(value: &ImplRegionValue) -> RegionValue {
    match value {
        ImplRegionValue::Argument(name) => RegionValue::Argument(name.clone()),
        ImplRegionValue::Local(name) => RegionValue::Local(name.clone()),
        ImplRegionValue::Capture(value) => RegionValue::Capture(to_spec_value(value)),
    }
}

fn region_to_spec(region: &ImplRegion) -> Region {
    Region {
        arguments: region
            .arguments
            .iter()
            .map(|argument| BlockArgument {
                name: argument.name.clone(),
                ty: argument.ty.clone(),
            })
            .collect(),
        captures: region.captures.iter().map(to_spec_value).collect(),
        operations: region
            .operations
            .iter()
            .map(|operation| RegionOperation {
                result: operation.result.clone(),
                opcode: operation.opcode,
                operands: operation
                    .operands
                    .iter()
                    .map(region_value_to_spec)
                    .collect(),
                attributes: operation.attributes.clone(),
                result_type: operation.result_type.clone(),
            })
            .collect(),
        yield_value: region_value_to_spec(&region.yield_value),
        yield_type: region.yield_type.clone(),
    }
}

/// Builds a verifier/evaluator adapter without changing ImplIR's separate data model.
#[must_use]
pub fn impl_as_program(program: &ImplProgram) -> Program {
    let mut dimensions = BTreeMap::new();
    let mut dimension_names = BTreeMap::new();
    for (index, (name, non_negative)) in program.dimensions.iter().enumerate() {
        let id = DimensionId::new(format!("id{}", index + 1));
        dimensions.insert(
            id.clone(),
            Dimension {
                id: id.clone(),
                name: name.clone(),
                non_negative: *non_negative,
                provenance: ActionId::new("impl"),
            },
        );
        dimension_names.insert(name.clone(), id);
    }
    Program {
        dimensions,
        dimension_names,
        operations: program
            .operations
            .iter()
            .map(|(id, operation)| {
                (
                    to_spec_operation(id),
                    Operation {
                        id: to_spec_operation(id),
                        opcode: operation.opcode,
                        operands: operation.operands.iter().map(to_spec_value).collect(),
                        results: operation.results.iter().map(to_spec_value).collect(),
                        attributes: operation.attributes.clone(),
                        region: operation.region.as_ref().map(region_to_spec),
                        provenance: ActionId::new("impl"),
                        result_types: operation.result_types.clone(),
                    },
                )
            })
            .collect(),
        operation_order: program
            .operation_order
            .iter()
            .map(to_spec_operation)
            .collect(),
        values: program
            .values
            .iter()
            .map(|(id, value)| {
                let ImplValueOrigin::Operation(operation) = &value.origin;
                (
                    to_spec_value(id),
                    ValueDef {
                        id: to_spec_value(id),
                        ty: value.ty.clone(),
                        origin: ValueOrigin::Operation(to_spec_operation(operation)),
                        name: value.name.clone(),
                    },
                )
            })
            .collect(),
        parameters: program
            .parameters
            .iter()
            .map(|(name, value)| (name.clone(), to_spec_value(value)))
            .collect(),
        constants: program
            .constants
            .iter()
            .map(|(value, constant)| (to_spec_value(value), constant.clone()))
            .collect(),
        outputs: program
            .outputs
            .iter()
            .map(|(name, output)| (name.clone(), to_spec_value(&output.value)))
            .collect(),
        holes: BTreeMap::new(),
        constraints: program.constraints.clone(),
        obligations: BTreeMap::new(),
        numeric_contract: program.numeric_contract.clone(),
        frozen: true,
    }
}

fn impl_region_value_type(
    value: &ImplRegionValue,
    arguments: &BTreeMap<String, Type>,
    locals: &BTreeMap<String, Type>,
    program: &ImplProgram,
) -> AgentResult<Type> {
    match value {
        ImplRegionValue::Argument(name) => arguments
            .get(name)
            .cloned()
            .ok_or_else(|| impl_error(format!("unknown region argument `{name}`"))),
        ImplRegionValue::Local(name) => locals
            .get(name)
            .cloned()
            .ok_or_else(|| impl_error(format!("unknown or forward region local `{name}`"))),
        ImplRegionValue::Capture(value) => program
            .values
            .get(value)
            .map(|value| value.ty.clone())
            .ok_or_else(|| impl_error(format!("unknown region capture `{value}`"))),
    }
}

pub(crate) fn verify_region(
    region: &ImplRegion,
    program: &ImplProgram,
    facts: &ConstraintFacts,
) -> AgentResult<()> {
    let mut arguments = BTreeMap::new();
    for argument in &region.arguments {
        if arguments
            .insert(argument.name.clone(), argument.ty.clone())
            .is_some()
        {
            return Err(impl_error(format!(
                "duplicate region argument `{}`",
                argument.name
            )));
        }
    }
    let capture_set = region.captures.iter().cloned().collect::<BTreeSet<_>>();
    if capture_set.len() != region.captures.len() {
        return Err(impl_error("duplicate region capture"));
    }
    for capture in &region.captures {
        if !program.values.contains_key(capture) {
            return Err(impl_error(format!("unknown region capture `{capture}`")));
        }
    }
    let mut locals = BTreeMap::new();
    for operation in &region.operations {
        if locals.contains_key(&operation.result) || arguments.contains_key(&operation.result) {
            return Err(impl_error(format!(
                "duplicate region result `{}`",
                operation.result
            )));
        }
        if matches!(
            operation.opcode,
            Opcode::Parameter | Opcode::Constant | Opcode::Map | Opcode::ZipMap | Opcode::Reduce
        ) {
            return Err(impl_error("nested higher-order or defining region opcode"));
        }
        for operand in &operation.operands {
            if let ImplRegionValue::Capture(value) = operand {
                if !capture_set.contains(value) {
                    return Err(impl_error(format!(
                        "region uses undeclared capture `{value}`"
                    )));
                }
            }
        }
        let operand_types = operation
            .operands
            .iter()
            .map(|value| impl_region_value_type(value, &arguments, &locals, program))
            .collect::<AgentResult<Vec<_>>>()?;
        let inferred = infer_primitive_with_facts(
            operation.opcode,
            &operand_types,
            &operation.attributes,
            facts,
        )?;
        if inferred.ty != operation.result_type {
            return Err(impl_error(format!(
                "region result type mismatch for `{}`",
                operation.result
            ))
            .with_types(inferred.ty.to_string(), operation.result_type.to_string()));
        }
        locals.insert(operation.result.clone(), inferred.ty);
    }
    let yielded = impl_region_value_type(&region.yield_value, &arguments, &locals, program)?;
    if yielded != region.yield_type {
        return Err(
            impl_error("region yield type does not match its verified type")
                .with_types(region.yield_type.to_string(), yielded.to_string()),
        );
    }
    Ok(())
}

/// Infers one proposed top-level ImplIR operation without allocating persistent IDs.
pub(crate) fn infer_proposed_operation(
    program: &ImplProgram,
    opcode: Opcode,
    operands: &[ImplValueId],
    attributes: &BTreeMap<String, JsonValue>,
    constant: Option<&ConstantValue>,
    region: Option<&ImplRegion>,
) -> AgentResult<Type> {
    if matches!(opcode, Opcode::Parameter) {
        return Err(impl_error(
            "proposal fragments cannot create external parameters",
        ));
    }
    let adapter = impl_as_program(program);
    let facts = ConstraintFacts::from_program(&adapter)?;
    let operand_types = operands
        .iter()
        .map(|value| {
            program
                .values
                .get(value)
                .map(|value| value.ty.clone())
                .ok_or_else(|| impl_error(format!("proposal operand `{value}` is absent")))
        })
        .collect::<AgentResult<Vec<_>>>()?;
    match opcode {
        Opcode::Constant => {
            if !operands.is_empty() || region.is_some() {
                return Err(impl_error(
                    "proposal constant cannot have operands or a region",
                ));
            }
            constant
                .map(|value| Type::Scalar(value.scalar_type()))
                .ok_or_else(|| impl_error("proposal constant requires an exact literal"))
        }
        Opcode::Map | Opcode::ZipMap | Opcode::Reduce => {
            if constant.is_some() {
                return Err(impl_error(
                    "higher-order proposal operation cannot carry a scalar literal",
                ));
            }
            let region = region.ok_or_else(|| {
                impl_error("higher-order proposal operation requires a closed typed region")
            })?;
            verify_region(region, program, &facts)?;
            Ok(
                infer_higher_with_facts(opcode, &operand_types, &region_to_spec(region), &facts)?
                    .ty,
            )
        }
        _ => {
            if constant.is_some() || region.is_some() {
                return Err(impl_error(
                    "primitive proposal operation has an invalid literal or region",
                ));
            }
            Ok(infer_primitive_with_facts(opcode, &operand_types, attributes, &facts)?.ty)
        }
    }
}

/// Verifies SSA, types, regions, interfaces, source links, and resource limits.
pub fn verify_impl(
    program: &ImplProgram,
    source: &Program,
    limits: &ResourceLimits,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::ImplOperations,
        u64::try_from(program.operations.len()).unwrap_or(u64::MAX),
        "ImplIR verification",
    )?;
    BudgetCheck::against(
        limits,
        ResourceKind::ImplValues,
        u64::try_from(program.values.len()).unwrap_or(u64::MAX),
        "ImplIR verification",
    )?;
    if program.operation_order.len() != program.operations.len()
        || program
            .operation_order
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != program.operation_order.len()
    {
        return Err(impl_error(
            "operation order must contain every ImplIR operation exactly once",
        ));
    }
    if program.outputs.is_empty() {
        return Err(impl_error("ImplIR needs at least one external output"));
    }
    let expected_dimensions = source
        .dimensions
        .values()
        .map(|dimension| (dimension.name.clone(), dimension.non_negative))
        .collect::<BTreeMap<_, _>>();
    if program.dimensions != expected_dimensions
        || program.constraints != source.constraints
        || program.numeric_contract != source.numeric_contract
    {
        return Err(impl_error(
            "ImplIR logical dimensions, constraints, or NumericContract differ from SpecIR",
        ));
    }
    if program.parameters.keys().collect::<BTreeSet<_>>()
        != source.parameters.keys().collect::<BTreeSet<_>>()
        || program.outputs.keys().collect::<BTreeSet<_>>()
            != source.outputs.keys().collect::<BTreeSet<_>>()
    {
        return Err(impl_error(
            "ImplIR external parameter/output names differ from SpecIR",
        ));
    }
    let adapter = impl_as_program(program);
    let facts = ConstraintFacts::from_program(&adapter)?;
    let mut defined = BTreeSet::new();
    for operation_id in &program.operation_order {
        let operation = program.operations.get(operation_id).ok_or_else(|| {
            impl_error(format!(
                "operation order references missing `{operation_id}`"
            ))
        })?;
        if operation.id != *operation_id {
            return Err(impl_error("operation map key does not match embedded ID"));
        }
        if operation.results.len() != operation.result_types.len() || operation.results.is_empty() {
            return Err(impl_error(format!(
                "operation `{operation_id}` has invalid result arity"
            )));
        }
        for operand in &operation.operands {
            if !defined.contains(operand) {
                return Err(impl_error(format!(
                    "operation `{operation_id}` uses missing or forward value `{operand}`"
                )));
            }
        }
        for result in &operation.results {
            let value = program
                .values
                .get(result)
                .ok_or_else(|| impl_error(format!("result `{result}` has no value definition")))?;
            if value.id != *result
                || value.origin != ImplValueOrigin::Operation(operation_id.clone())
            {
                return Err(impl_error(format!(
                    "result `{result}` has inconsistent SSA origin"
                )));
            }
        }
        verify_source_link(
            &operation.source_link,
            source,
            &format!("ImplIR operation `{operation_id}`"),
        )?;
        let operand_types = operation
            .operands
            .iter()
            .map(|value| {
                program
                    .values
                    .get(value)
                    .map(|value| value.ty.clone())
                    .ok_or_else(|| impl_error(format!("operand `{value}` is absent")))
            })
            .collect::<AgentResult<Vec<_>>>()?;
        let inferred = match operation.opcode {
            Opcode::Parameter => {
                if !operation.operands.is_empty() || operation.region.is_some() {
                    return Err(impl_error("parameter operation has operands or a region"));
                }
                operation.result_types[0].clone()
            }
            Opcode::Constant => {
                let result = &operation.results[0];
                let constant = program.constants.get(result).ok_or_else(|| {
                    impl_error(format!(
                        "constant operation `{operation_id}` lacks a literal"
                    ))
                })?;
                let ty = operation.result_types[0].clone();
                let literal_matches = ty == Type::Scalar(constant.scalar_type())
                    || matches!(
                        (&ty, constant),
                        (
                            Type::Scalar(crate::types::ScalarType::Index),
                            ConstantValue::I32 { .. }
                        )
                    );
                if !literal_matches {
                    return Err(impl_error("constant literal and result type differ"));
                }
                ty
            }
            Opcode::Map | Opcode::ZipMap | Opcode::Reduce => {
                let region = operation.region.as_ref().ok_or_else(|| {
                    impl_error(format!("operation `{operation_id}` requires a region"))
                })?;
                verify_region(region, program, &facts)?;
                infer_higher_with_facts(
                    operation.opcode,
                    &operand_types,
                    &region_to_spec(region),
                    &facts,
                )?
                .ty
            }
            _ => {
                if operation.region.is_some() {
                    return Err(impl_error(format!(
                        "primitive operation `{operation_id}` unexpectedly has a region"
                    )));
                }
                infer_primitive_with_facts(
                    operation.opcode,
                    &operand_types,
                    &operation.attributes,
                    &facts,
                )?
                .ty
            }
        };
        if operation.result_types != vec![inferred.clone()]
            || program
                .values
                .get(&operation.results[0])
                .is_none_or(|value| value.ty != inferred)
        {
            return Err(impl_error(format!(
                "operation `{operation_id}` result type failed inference"
            )));
        }
        defined.extend(operation.results.iter().cloned());
    }
    if defined.len() != program.values.len() {
        return Err(impl_error(
            "ImplIR contains values outside operation results",
        ));
    }
    for (value_id, value) in &program.values {
        verify_source_link(
            &value.source_link,
            source,
            &format!("ImplIR value `{value_id}`"),
        )?;
    }
    for (name, value) in &program.parameters {
        let definition = program
            .values
            .get(value)
            .ok_or_else(|| impl_error(format!("parameter `{name}` references missing value")))?;
        let ImplValueOrigin::Operation(operation) = &definition.origin;
        if program
            .operations
            .get(operation)
            .is_none_or(|operation| operation.opcode != Opcode::Parameter)
        {
            return Err(impl_error(format!(
                "parameter `{name}` is not defined by a parameter operation"
            )));
        }
        let source_type = source
            .parameters
            .get(name)
            .and_then(|value| source.values.get(value))
            .map(|value| &value.ty)
            .ok_or_else(|| impl_error(format!("SpecIR parameter `{name}` is missing")))?;
        if &definition.ty != source_type {
            return Err(impl_error(format!(
                "ImplIR parameter `{name}` type differs from SpecIR"
            )));
        }
    }
    for (name, output) in &program.outputs {
        if output.name != *name {
            return Err(impl_error("output map key and embedded name differ"));
        }
        let ty = program
            .values
            .get(&output.value)
            .map(|value| &value.ty)
            .ok_or_else(|| impl_error(format!("output `{name}` references missing value")))?;
        if ty != &output.ty {
            return Err(impl_error(format!("output `{name}` type is stale")));
        }
        let source_type = source
            .outputs
            .get(name)
            .and_then(|value| source.values.get(value))
            .map(|value| &value.ty)
            .ok_or_else(|| impl_error(format!("SpecIR output `{name}` is missing")))?;
        if ty != source_type {
            return Err(impl_error(format!(
                "ImplIR output `{name}` type differs from SpecIR"
            )));
        }
    }
    Ok(())
}

fn digest_hex(bytes: &[u8]) -> ImplHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    ImplHash(output)
}

/// Builds canonical ImplIR bytes and the domain-separated `impl_hash`.
pub fn canonicalize_impl_with_limit(
    program: &ImplProgram,
    max_bytes: u64,
) -> AgentResult<ImplCanonicalization> {
    let adapter = impl_as_program(program);
    let spec_canonical = canonicalize_spec_with_limit(&adapter, max_bytes)?;
    let canonical = ImplCanonicalProgramV1 {
        codec: IMPL_CANONICAL_CODEC.to_owned(),
        version: IMPL_CANONICAL_VERSION,
        graph: spec_canonical.canonical,
    };
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        AgentError::new(
            ErrorCode::CanonicalizationFailed,
            format!("ImplIR canonical serialization failed: {error}"),
        )
    })?;
    BudgetCheck::ensure(
        ResourceKind::CandidateCanonicalBytes,
        max_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "ImplIR semantic canonicalization",
    )?;
    let mut hash_input = Vec::with_capacity(IMPL_HASH_DOMAIN.len() + bytes.len());
    hash_input.extend_from_slice(IMPL_HASH_DOMAIN);
    hash_input.extend_from_slice(&bytes);
    Ok(ImplCanonicalization {
        canonical,
        bytes,
        impl_hash: digest_hex(&hash_input),
    })
}

/// Computes the history-independent semantic implementation hash.
pub fn impl_hash(program: &ImplProgram) -> AgentResult<ImplHash> {
    Ok(canonicalize_impl_with_limit(
        program,
        ResourceLimits::hard_safety_caps().candidate_canonical_bytes,
    )?
    .impl_hash)
}
