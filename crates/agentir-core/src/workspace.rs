//! Workspace state machine, verification, and atomic revision commits.

use crate::{
    actions::{Action, ActionClassification, RegionSpec, Transaction},
    candidate::{
        CANDIDATE_SEMANTICS_VERSION, Candidate, CandidateCheckReport, CandidateContinuation,
        CandidateEvent, CandidateForest, CandidateRevision, CandidateTransaction,
        DifferentialValidation, LEGACY_CANDIDATE_SEMANTICS_VERSION, ProposalRecord, RelationKind,
        SpeculativeRewriteProposal, TranslationCheckReport,
    },
    canonical::{content_hash, content_hash_with_limit},
    constraints::{ConstraintFacts, ConstraintQueryResult},
    continuation::{ContinuationFrame, InteractionMode, build_frame},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    holes::{ExpectedEffects, Hole, HoleStatus},
    ids::{
        ActionId, CandidateId, CandidateRevisionId, HoleId, IdAllocator, ObligationId, ProposalId,
        RevisionId, ValueId, WorkspaceId,
    },
    ir::{
        BlockArgument, ConstantValue, Dimension, Opcode, Operation, Program, Region,
        RegionOperation, RegionValue, ValueDef, ValueOrigin,
    },
    obligations::{
        ObligationKind, ObligationOrigin, ObligationStatus, ProofObligation,
        ShapeCompatibilityProposition, ShapeObligationContext, ShapeRelationKind,
    },
    persistence::{
        CORE_SEMANTICS_VERSION, LEGACY_CORE_SEMANTICS_VERSION, ReplayReport,
        VersionedWorkspaceEvent, WORKSPACE_SNAPSHOT_VERSION, WorkspaceEvent, WorkspaceSnapshot,
    },
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    revision::{Revision, RevisionDiff, StatusSummary, diff},
    semantic::{
        SPEC_CANONICAL_VERSION, SemanticCanonicalization, SpecHash, canonicalize_spec_with_limit,
    },
    shapes::{SolverStatus, same_shape},
    spec::{
        ShapeRelation, infer_higher, infer_higher_with_facts, infer_primitive,
        infer_primitive_with_facts,
    },
    transaction::CommitResult,
    types::{DimExpr, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

/// Result of checking one revision for completeness and deployability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    /// Checked revision.
    pub revision: RevisionId,
    /// Whether all graph invariants currently hold.
    pub well_typed: bool,
    /// Whether outputs exist and no proof debt remains.
    pub complete: bool,
    /// Whether the specification is frozen and complete.
    pub deployable: bool,
    /// Open holes in deterministic ID order.
    pub open_holes: Vec<HoleId>,
    /// Open obligations in deterministic ID order.
    pub open_obligations: Vec<ObligationId>,
    /// Named outputs and their inferred types.
    pub outputs: BTreeMap<String, Type>,
}

#[derive(Clone, Debug)]
enum Binding {
    Value(ValueId),
    Hole(HoleId),
    Dimension(crate::ids::DimensionId),
}

impl Binding {
    fn persistent_id(&self) -> String {
        match self {
            Self::Value(id) => id.to_string(),
            Self::Hole(id) => id.to_string(),
            Self::Dimension(id) => id.to_string(),
        }
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

fn semantic_metadata(
    program: &Program,
    max_bytes: u64,
) -> AgentResult<(Option<SpecHash>, Option<u32>)> {
    if program.frozen {
        let canonical = canonicalize_spec_with_limit(program, max_bytes)?;
        Ok((Some(canonical.spec_hash), Some(SPEC_CANONICAL_VERSION)))
    } else {
        Ok((None, None))
    }
}

fn ensure_new_binding(bindings: &BTreeMap<String, Binding>, binding: &str) -> AgentResult<()> {
    if !binding.starts_with('$') {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            format!("temporary binding `{binding}` must start with `$`"),
        ));
    }
    if bindings.contains_key(binding) {
        return Err(AgentError::new(
            ErrorCode::DuplicateBinding,
            format!("temporary binding `{binding}` is already defined"),
        ));
    }
    Ok(())
}

fn resolve_value(
    reference: &str,
    program: &Program,
    bindings: &BTreeMap<String, Binding>,
) -> AgentResult<ValueId> {
    if let Some(binding) = bindings.get(reference) {
        return match binding {
            Binding::Value(id) => Ok(id.clone()),
            Binding::Hole(id) => program
                .holes
                .get(id)
                .map(|hole| hole.placeholder.clone())
                .ok_or_else(|| AgentError::new(ErrorCode::UnknownReference, reference)),
            Binding::Dimension(_) => Err(AgentError::new(
                ErrorCode::TypeMismatch,
                format!("dimension `{reference}` is not a value"),
            )),
        };
    }
    let reference = reference.strip_prefix('@').unwrap_or(reference);
    if let Ok(index) = reference.parse::<usize>() {
        if index > 0 {
            if let Some(value) = program.values.keys().nth(index - 1) {
                return Ok(value.clone());
            }
        }
        return Err(AgentError::new(
            ErrorCode::UnknownReference,
            format!("short value index `@{index}` is outside the live-value table"),
        ));
    }
    let persistent = ValueId::new(reference);
    if program.values.contains_key(&persistent) {
        return Ok(persistent);
    }
    program
        .parameters
        .get(reference)
        .or_else(|| program.outputs.get(reference))
        .cloned()
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::UnknownReference,
                format!("unknown value reference `{reference}`"),
            )
        })
}

fn resolve_hole(
    reference: &str,
    program: &Program,
    bindings: &BTreeMap<String, Binding>,
) -> AgentResult<HoleId> {
    if let Some(Binding::Hole(id)) = bindings.get(reference) {
        return Ok(id.clone());
    }
    let reference = reference.strip_prefix('@').unwrap_or(reference);
    let persistent = HoleId::new(reference);
    program
        .holes
        .contains_key(&persistent)
        .then_some(persistent)
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::UnknownReference,
                format!("unknown hole reference `{reference}`"),
            )
        })
}

fn type_symbols(ty: &Type) -> BTreeSet<&str> {
    let mut symbols = BTreeSet::new();
    if let Type::Tensor { shape, .. } = ty {
        for dimension in &shape.0 {
            match dimension {
                DimExpr::Static(_) => {}
                DimExpr::Symbol(symbol) | DimExpr::Affine { symbol, .. } => {
                    symbols.insert(symbol.as_str());
                }
            }
        }
    }
    symbols
}

fn validate_type_symbols(program: &Program, ty: &Type) -> AgentResult<()> {
    let missing: Vec<_> = type_symbols(ty)
        .into_iter()
        .filter(|symbol| !program.dimension_names.contains_key(*symbol))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::UnknownReference,
            format!(
                "type references undefined dimensions: {}",
                missing.join(", ")
            ),
        ))
    }
}

fn value_type(program: &Program, value: &ValueId) -> AgentResult<Type> {
    program
        .values
        .get(value)
        .map(|definition| definition.ty.clone())
        .ok_or_else(|| AgentError::new(ErrorCode::UnknownReference, value.to_string()))
}

fn expected_region_arguments(opcode: Opcode, operands: &[Type]) -> AgentResult<Vec<Type>> {
    match opcode {
        Opcode::Map => {
            let [Type::Tensor { element, .. }] = operands else {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "map expects one tensor operand",
                ));
            };
            Ok(vec![Type::Scalar(*element)])
        }
        Opcode::ZipMap => operands
            .iter()
            .map(|operand| match operand {
                Type::Tensor { element, .. } => Ok(Type::Scalar(*element)),
                Type::Scalar(_) => Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "zip_map expects only tensor operands",
                )),
            })
            .collect(),
        Opcode::Reduce => {
            let Some(Type::Tensor { element, .. }) = operands.first() else {
                return Err(AgentError::new(
                    ErrorCode::TypeMismatch,
                    "reduce expects a tensor operand",
                ));
            };
            Ok(vec![Type::Scalar(*element), Type::Scalar(*element)])
        }
        _ => Err(AgentError::new(
            ErrorCode::InvalidRegion,
            format!("{opcode} does not accept a region"),
        )),
    }
}

fn build_region(
    opcode: Opcode,
    spec: &RegionSpec,
    operands: &[Type],
    program: &Program,
    bindings: &BTreeMap<String, Binding>,
    facts: Option<&ConstraintFacts>,
) -> AgentResult<(Region, ActionClassification, Vec<ShapeRelation>)> {
    let expected_arguments = expected_region_arguments(opcode, operands)?;
    if spec.arguments.len() != expected_arguments.len() {
        return Err(AgentError::new(
            ErrorCode::InvalidRegion,
            format!(
                "{opcode} expects {} region arguments, got {}",
                expected_arguments.len(),
                spec.arguments.len()
            ),
        ));
    }
    let mut argument_types = BTreeMap::new();
    let mut arguments = Vec::new();
    for (argument, expected) in spec.arguments.iter().zip(expected_arguments) {
        if argument.ty != expected {
            return Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!(
                    "region argument `{}` has an incompatible type",
                    argument.name
                ),
            )
            .with_types(expected.to_string(), argument.ty.to_string()));
        }
        if argument_types
            .insert(argument.name.clone(), argument.ty.clone())
            .is_some()
        {
            return Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!("duplicate region argument `{}`", argument.name),
            ));
        }
        arguments.push(BlockArgument {
            name: argument.name.clone(),
            ty: argument.ty.clone(),
        });
    }

    let mut captures_by_reference = BTreeMap::new();
    let mut captures = Vec::new();
    for reference in &spec.captures {
        let value = resolve_value(reference, program, bindings)?;
        if captures_by_reference
            .insert(reference.clone(), value.clone())
            .is_some()
        {
            return Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!("duplicate region capture `{reference}`"),
            ));
        }
        captures.push(value);
    }

    let resolve_region_value =
        |reference: &str, locals: &BTreeMap<String, Type>| -> AgentResult<(RegionValue, Type)> {
            if let Some(ty) = argument_types.get(reference) {
                return Ok((RegionValue::Argument(reference.to_owned()), ty.clone()));
            }
            if let Some(ty) = locals.get(reference) {
                return Ok((RegionValue::Local(reference.to_owned()), ty.clone()));
            }
            if let Some(value) = captures_by_reference.get(reference) {
                return Ok((
                    RegionValue::Capture(value.clone()),
                    value_type(program, value)?,
                ));
            }
            Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!(
                    "region reference `{reference}` is not an argument, local, or explicit capture"
                ),
            ))
        };

    let mut local_types = BTreeMap::new();
    let mut operations = Vec::new();
    let mut classification = ActionClassification::Legal;
    let mut shape_relations = Vec::new();
    for operation in &spec.operations {
        if !operation.bind.starts_with('$') || local_types.contains_key(&operation.bind) {
            return Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!("invalid or duplicate local binding `{}`", operation.bind),
            ));
        }
        let opcode = operation
            .opcode
            .parse::<Opcode>()
            .map_err(|message| AgentError::new(ErrorCode::UnknownOpcode, message))?;
        if matches!(
            opcode,
            Opcode::Parameter | Opcode::Constant | Opcode::Map | Opcode::ZipMap | Opcode::Reduce
        ) {
            return Err(AgentError::new(
                ErrorCode::InvalidRegion,
                format!("nested opcode `{opcode}` is not permitted in Stage 1 regions"),
            ));
        }
        let resolved: Vec<_> = operation
            .operands
            .iter()
            .map(|reference| resolve_region_value(reference, &local_types))
            .collect::<AgentResult<_>>()?;
        let operand_types: Vec<_> = resolved.iter().map(|(_, ty)| ty.clone()).collect();
        let inferred = if let Some(facts) = facts {
            infer_primitive_with_facts(opcode, &operand_types, &operation.attributes, facts)?
        } else {
            infer_primitive(opcode, &operand_types, &operation.attributes)?
        };
        if matches!(inferred.classification, ActionClassification::Conditional) {
            classification = ActionClassification::Conditional;
        }
        let result_type = inferred.ty;
        shape_relations.extend(inferred.shape_relations);
        operations.push(RegionOperation {
            result: operation.bind.clone(),
            opcode,
            operands: resolved.into_iter().map(|(value, _)| value).collect(),
            attributes: operation.attributes.clone(),
            result_type: result_type.clone(),
        });
        local_types.insert(operation.bind.clone(), result_type);
    }
    let (yield_value, yield_type) = resolve_region_value(&spec.yield_value, &local_types)?;
    Ok((
        Region {
            arguments,
            captures,
            operations,
            yield_value,
            yield_type,
        },
        classification,
        shape_relations,
    ))
}

struct ObligationDraft {
    kind: ObligationKind,
    status: ObligationStatus,
    proposition: JsonValue,
    discharge_methods: Vec<String>,
}

fn add_obligation(
    program: &mut Program,
    allocator: &mut IdAllocator,
    action: &ActionId,
    draft: ObligationDraft,
    max_obligations: u64,
) -> AgentResult<ObligationId> {
    BudgetCheck::ensure(
        ResourceKind::ObligationsPerProgram,
        max_obligations,
        u64::try_from(program.obligations.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        "create proof obligation",
    )?;
    let id = allocator.obligation();
    program.obligations.insert(
        id.clone(),
        ProofObligation {
            id: id.clone(),
            kind: draft.kind,
            proposition: draft.proposition,
            shape_compatibility: None,
            origin: ObligationOrigin {
                revision: None,
                action: action.clone(),
            },
            status: draft.status,
            discharge_methods: draft.discharge_methods,
        },
    );
    Ok(id)
}

fn involved_symbols(left: &Type, right: &Type) -> Vec<String> {
    type_symbols(left)
        .into_iter()
        .chain(type_symbols(right))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn add_shape_obligation(
    program: &mut Program,
    allocator: &mut IdAllocator,
    action: &ActionId,
    relation: ShapeRelation,
    context: ShapeObligationContext,
    max_obligations: u64,
) -> AgentResult<ObligationId> {
    BudgetCheck::ensure(
        ResourceKind::ObligationsPerProgram,
        max_obligations,
        u64::try_from(program.obligations.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        "create shape compatibility obligation",
    )?;
    let proposition = ShapeCompatibilityProposition {
        relation: ShapeRelationKind::EqualShape,
        involved_symbols: involved_symbols(&relation.left, &relation.right),
        left: relation.left,
        right: relation.right,
        context,
    };
    let id = allocator.obligation();
    program.obligations.insert(
        id.clone(),
        ProofObligation {
            id: id.clone(),
            kind: ObligationKind::ShapeCompatible,
            proposition: json!({"shape_compatibility": proposition}),
            shape_compatibility: Some(proposition),
            origin: ObligationOrigin {
                revision: None,
                action: action.clone(),
            },
            status: ObligationStatus::Open,
            discharge_methods: vec!["add_constraint".to_owned(), "specialize_shape".to_owned()],
        },
    );
    Ok(id)
}

fn discharge_shape_obligations(
    program: &mut Program,
    facts: &ConstraintFacts,
    reject_contradiction: bool,
) -> AgentResult<()> {
    for obligation in program.obligations.values_mut() {
        if obligation.kind != ObligationKind::ShapeCompatible
            || obligation.status != ObligationStatus::Open
        {
            continue;
        }
        let Some(proposition) = &obligation.shape_compatibility else {
            continue;
        };
        match facts.query_types(&proposition.left, &proposition.right)? {
            ConstraintQueryResult::Proved { .. } => {
                obligation.status = ObligationStatus::Proved;
            }
            ConstraintQueryResult::Unknown => {}
            ConstraintQueryResult::Contradiction { contradiction } if reject_contradiction => {
                return Err(AgentError::new(
                    ErrorCode::ConstraintContradiction,
                    "constraint contradicts an open shape compatibility obligation",
                )
                .with_types(contradiction.expected, contradiction.actual)
                .with_detail("obligation", obligation.id.to_string())
                .with_detail(
                    "normalized_constraint",
                    json!(contradiction.normalized_constraint),
                )
                .with_detail("conflicting_facts", json!(contradiction.conflicting_facts))
                .with_repair("remove the conflicting constraint or rebuild the operation"));
            }
            ConstraintQueryResult::Contradiction { .. } => {
                obligation.status = ObligationStatus::Refuted;
            }
        }
    }
    Ok(())
}

fn check_program(revision: RevisionId, program: &Program) -> CheckReport {
    let open_holes = program
        .holes
        .iter()
        .filter(|(_, hole)| matches!(hole.status, HoleStatus::Open))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let open_obligations = program
        .obligations
        .iter()
        .filter(|(_, obligation)| matches!(obligation.status, ObligationStatus::Open))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let outputs = program
        .outputs
        .iter()
        .filter_map(|(name, value)| {
            program
                .values
                .get(value)
                .map(|definition| (name.clone(), definition.ty.clone()))
        })
        .collect();
    let complete =
        !program.outputs.is_empty() && open_holes.is_empty() && open_obligations.is_empty();
    CheckReport {
        revision,
        well_typed: true,
        complete,
        deployable: complete && program.frozen,
        open_holes,
        open_obligations,
        outputs,
    }
}

fn require_complete(program: &Program) -> AgentResult<()> {
    let open_holes: Vec<_> = program
        .holes
        .iter()
        .filter(|(_, hole)| matches!(hole.status, HoleStatus::Open))
        .map(|(id, _)| id.to_string())
        .collect();
    if !open_holes.is_empty() {
        return Err(AgentError::new(
            ErrorCode::OpenHole,
            "specification contains open typed holes",
        )
        .with_detail("holes", json!(open_holes)));
    }
    if program.outputs.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "specification must define at least one output",
        ));
    }
    let open_obligations: Vec<_> = program
        .obligations
        .iter()
        .filter(|(_, obligation)| matches!(obligation.status, ObligationStatus::Open))
        .map(|(id, _)| id.to_string())
        .collect();
    if !open_obligations.is_empty() {
        return Err(AgentError::new(
            ErrorCode::SpecNotComplete,
            "specification contains open proof obligations",
        )
        .with_detail("obligations", json!(open_obligations)));
    }
    Ok(())
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn check_attributes(
    attributes: &BTreeMap<String, JsonValue>,
    limits: &ResourceLimits,
    context: &str,
) -> AgentResult<()> {
    let bytes = serde_json::to_vec(attributes).map_err(|error| {
        AgentError::new(
            ErrorCode::InvalidRequest,
            format!("attribute encoding failed: {error}"),
        )
    })?;
    BudgetCheck::against(
        limits,
        ResourceKind::AttributeJsonBytes,
        as_u64(bytes.len()),
        context,
    )
}

fn preflight_transaction(
    program: &Program,
    transaction: &Transaction,
    limits: &ResourceLimits,
    semantics_version: u32,
) -> AgentResult<()> {
    BudgetCheck::against(
        limits,
        ResourceKind::ActionsPerTransaction,
        as_u64(transaction.actions.len()),
        "transaction preflight",
    )?;
    let mut dimensions = as_u64(program.dimensions.len());
    let mut operations = as_u64(program.operations.len());
    let mut values = as_u64(program.values.len());
    let mut holes = as_u64(program.holes.len());
    let mut constraints = as_u64(program.constraints.len()).saturating_add(
        program
            .holes
            .values()
            .map(|hole| as_u64(hole.shape_constraints.len()))
            .fold(0, u64::saturating_add),
    );
    let mut projected_constraints = program.constraints.iter().cloned().collect::<BTreeSet<_>>();
    let mut outputs = as_u64(program.outputs.len());
    let mut projected_output_names = program.outputs.keys().cloned().collect::<BTreeSet<_>>();
    for action in &transaction.actions {
        match action {
            Action::DefineDimension { .. } => dimensions = dimensions.saturating_add(1),
            Action::CreateParameter { .. } | Action::CreateConstant { .. } => {
                operations = operations.saturating_add(1);
                values = values.saturating_add(1);
            }
            Action::CreateHole {
                shape_constraints, ..
            } => {
                holes = holes.saturating_add(1);
                values = values.saturating_add(1);
                constraints = constraints.saturating_add(as_u64(shape_constraints.len()));
            }
            Action::CreateOp {
                operands,
                attributes,
                region,
                ..
            } => {
                operations = operations.saturating_add(1);
                values = values.saturating_add(1);
                BudgetCheck::against(
                    limits,
                    ResourceKind::OperandsPerOperation,
                    as_u64(operands.len()),
                    "top-level operation operands",
                )?;
                check_attributes(attributes, limits, "top-level operation attributes")?;
                if let Some(region) = region {
                    BudgetCheck::against(
                        limits,
                        ResourceKind::RegionArguments,
                        as_u64(region.arguments.len()),
                        "inline region arguments",
                    )?;
                    BudgetCheck::against(
                        limits,
                        ResourceKind::RegionOperations,
                        as_u64(region.operations.len()),
                        "inline region operations",
                    )?;
                    for operation in &region.operations {
                        BudgetCheck::against(
                            limits,
                            ResourceKind::OperandsPerOperation,
                            as_u64(operation.operands.len()),
                            "region operation operands",
                        )?;
                        check_attributes(
                            &operation.attributes,
                            limits,
                            "region operation attributes",
                        )?;
                    }
                }
            }
            Action::AddConstraint { constraint } => {
                if semantics_version == LEGACY_CORE_SEMANTICS_VERSION
                    || projected_constraints.insert(constraint.clone())
                {
                    constraints = constraints.saturating_add(1);
                }
            }
            Action::SetOutput { name, .. } => {
                if projected_output_names.insert(name.clone()) {
                    outputs = outputs.saturating_add(1);
                }
            }
            Action::FillHole { .. } | Action::FreezeSpec | Action::ForkRevision => {}
        }
    }
    for (resource, actual) in [
        (ResourceKind::DimensionsPerProgram, dimensions),
        (ResourceKind::OperationsPerProgram, operations),
        (ResourceKind::ValuesPerProgram, values),
        (ResourceKind::HolesPerProgram, holes),
        (ResourceKind::ConstraintsPerProgram, constraints),
        (ResourceKind::OutputCount, outputs),
    ] {
        BudgetCheck::against(limits, resource, actual, "projected program size")?;
    }
    Ok(())
}

/// In-memory Stage 1 workspace with immutable revision snapshots.
#[derive(Clone, Debug)]
pub struct Workspace {
    id: WorkspaceId,
    revisions: BTreeMap<RevisionId, Revision>,
    head: RevisionId,
    allocator: IdAllocator,
    events: Vec<VersionedWorkspaceEvent>,
    candidates: CandidateForest,
    limits: ResourceLimits,
}

impl Workspace {
    /// Creates a workspace with the empty root revision `r0`.
    pub fn new(id: WorkspaceId) -> AgentResult<Self> {
        Self::with_limits(id, ResourceLimits::default())
    }

    /// Creates a workspace with explicit interactive limits.
    pub fn with_limits(id: WorkspaceId, limits: ResourceLimits) -> AgentResult<Self> {
        let program = Program::default();
        let root = RevisionId::new("r0");
        let hash = content_hash_with_limit(&program, limits.canonical_output_bytes)?;
        let revision = Revision {
            id: root.clone(),
            parents: Vec::new(),
            content_hash: hash,
            spec_hash: None,
            semantic_canonical_version: None,
            status: StatusSummary::from_program(&program),
            program,
            applied_transaction: None,
            created_at_unix_ms: now_unix_ms(),
        };
        Ok(Self {
            id,
            revisions: BTreeMap::from([(root.clone(), revision)]),
            head: root,
            allocator: IdAllocator::default(),
            events: Vec::new(),
            candidates: CandidateForest::default(),
            limits,
        })
    }

    /// Replaces interactive limits without changing canonical workspace state.
    pub fn set_resource_limits(&mut self, limits: ResourceLimits) {
        self.limits = limits;
    }

    /// Returns current interactive limits.
    #[must_use]
    pub const fn resource_limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Returns the workspace ID.
    #[must_use]
    pub const fn id(&self) -> &WorkspaceId {
        &self.id
    }

    /// Returns the independent persistent candidate forest.
    #[must_use]
    pub const fn candidate_forest(&self) -> &CandidateForest {
        &self.candidates
    }

    /// Returns the current head revision ID.
    #[must_use]
    pub const fn head(&self) -> &RevisionId {
        &self.head
    }

    /// Returns an immutable revision snapshot.
    pub fn revision(&self, id: &RevisionId) -> AgentResult<&Revision> {
        self.revisions.get(id).ok_or_else(|| {
            AgentError::new(
                ErrorCode::RevisionNotFound,
                format!("revision `{id}` does not exist"),
            )
        })
    }

    /// Checks one revision without changing workspace state.
    pub fn check(&self, revision: &RevisionId) -> AgentResult<CheckReport> {
        let revision_data = self.revision(revision)?;
        Ok(check_program(revision.clone(), &revision_data.program))
    }

    /// Atomically applies a transaction and creates exactly one child revision.
    pub fn apply(&mut self, transaction: &Transaction) -> AgentResult<CommitResult> {
        self.apply_with_semantics(transaction, CORE_SEMANTICS_VERSION)
    }

    fn apply_with_semantics(
        &mut self,
        transaction: &Transaction,
        semantics_version: u32,
    ) -> AgentResult<CommitResult> {
        if !matches!(
            semantics_version,
            LEGACY_CORE_SEMANTICS_VERSION | CORE_SEMANTICS_VERSION
        ) {
            return Err(AgentError::new(
                ErrorCode::PersistenceFormat,
                format!("unsupported compiler semantics version {semantics_version}"),
            )
            .with_detail("semantics_version", semantics_version));
        }
        if transaction.workspace != self.id {
            return Err(AgentError::new(
                ErrorCode::WorkspaceNotFound,
                format!("transaction targets `{}`", transaction.workspace),
            ));
        }
        if transaction.actions.is_empty() {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                "transaction must contain at least one action",
            ));
        }
        if transaction
            .actions
            .iter()
            .any(|action| matches!(action, Action::ForkRevision))
            && transaction.actions.len() != 1
        {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                "fork_revision must be the only action in its transaction",
            ));
        }
        if transaction.base_revision != self.head && !transaction.allow_branch {
            return Err(AgentError::new(
                ErrorCode::BaseRevisionConflict,
                format!(
                    "base `{}` is not current head `{}`",
                    transaction.base_revision, self.head
                ),
            )
            .with_detail("current_head", self.head.to_string()));
        }
        let base = self.revision(&transaction.base_revision)?;
        preflight_transaction(&base.program, transaction, &self.limits, semantics_version)?;
        let base = base.clone();
        let mut program = base.program;
        let mut allocator = self.allocator.clone();
        let mut bindings = BTreeMap::<String, Binding>::new();
        let mut inferred = BTreeMap::new();
        let mut classifications = Vec::new();
        let mut obligations_created = Vec::new();
        let mut facts = if semantics_version == CORE_SEMANTICS_VERSION {
            Some(ConstraintFacts::from_program(&program)?)
        } else {
            None
        };

        for action in &transaction.actions {
            if program.frozen && !matches!(action, Action::ForkRevision) {
                return Err(AgentError::new(
                    ErrorCode::SpecFrozen,
                    "frozen SpecIR cannot be changed",
                ));
            }
            let action_id = allocator.action();
            let mut classification = ActionClassification::Legal;
            match action {
                Action::DefineDimension {
                    bind,
                    name,
                    constraints,
                } => {
                    if program.dimension_names.contains_key(name) {
                        return Err(AgentError::new(
                            ErrorCode::DuplicateBinding,
                            format!("dimension `{name}` already exists"),
                        ));
                    }
                    if let Some(bind) = bind {
                        ensure_new_binding(&bindings, bind)?;
                    }
                    let normalized_constraints = constraints
                        .iter()
                        .map(|constraint| constraint.replace(' ', ""))
                        .collect::<Vec<_>>();
                    if semantics_version == CORE_SEMANTICS_VERSION
                        && normalized_constraints
                            .iter()
                            .any(|constraint| constraint != &format!("{name}>=0"))
                    {
                        return Err(AgentError::new(
                            ErrorCode::InvalidConstraint,
                            "define_dimension supports only `<symbol> >= 0` in Stage 1.2",
                        )
                        .with_detail("constraints", json!(constraints))
                        .with_repair("use a structured add_constraint equality for shape facts"));
                    }
                    let id = allocator.dimension();
                    let non_negative = normalized_constraints
                        .iter()
                        .any(|constraint| constraint == &format!("{name}>=0"));
                    program.dimensions.insert(
                        id.clone(),
                        Dimension {
                            id: id.clone(),
                            name: name.clone(),
                            non_negative,
                            provenance: action_id.clone(),
                        },
                    );
                    program.dimension_names.insert(name.clone(), id.clone());
                    if let Some(facts) = facts.as_mut() {
                        facts.declare_symbol(name, non_negative)?;
                    }
                    if let Some(bind) = bind {
                        bindings.insert(bind.clone(), Binding::Dimension(id));
                    }
                }
                Action::CreateParameter { bind, name, ty } => {
                    ensure_new_binding(&bindings, bind)?;
                    validate_type_symbols(&program, ty)?;
                    if program.parameters.contains_key(name) {
                        return Err(AgentError::new(
                            ErrorCode::DuplicateBinding,
                            format!("parameter `{name}` already exists"),
                        ));
                    }
                    let operation_id = allocator.operation();
                    let value_id = allocator.value();
                    let operation = Operation {
                        id: operation_id.clone(),
                        opcode: Opcode::Parameter,
                        operands: Vec::new(),
                        results: vec![value_id.clone()],
                        attributes: BTreeMap::from([
                            ("name".to_owned(), json!(name)),
                            ("type".to_owned(), json!(ty)),
                        ]),
                        region: None,
                        provenance: action_id.clone(),
                        result_types: vec![ty.clone()],
                    };
                    program.values.insert(
                        value_id.clone(),
                        ValueDef {
                            id: value_id.clone(),
                            ty: ty.clone(),
                            origin: ValueOrigin::Operation(operation_id.clone()),
                            name: Some(name.clone()),
                        },
                    );
                    program.operations.insert(operation_id.clone(), operation);
                    program.operation_order.push(operation_id);
                    program.parameters.insert(name.clone(), value_id.clone());
                    bindings.insert(bind.clone(), Binding::Value(value_id));
                    inferred.insert(bind.clone(), ty.clone());
                    obligations_created.push(add_obligation(
                        &mut program,
                        &mut allocator,
                        &action_id,
                        ObligationDraft {
                            kind: ObligationKind::TypeWellFormed,
                            status: ObligationStatus::Proved,
                            proposition: json!({"type": ty}),
                            discharge_methods: Vec::new(),
                        },
                        self.limits.obligations_per_program,
                    )?);
                }
                Action::CreateConstant { bind, ty, value } => {
                    ensure_new_binding(&bindings, bind)?;
                    let Type::Scalar(scalar) = ty else {
                        return Err(AgentError::new(
                            ErrorCode::TypeMismatch,
                            "Stage 1 constants must be scalar",
                        ));
                    };
                    let constant = ConstantValue::from_json(*scalar, value)
                        .map_err(|message| AgentError::new(ErrorCode::TypeMismatch, message))?;
                    let operation_id = allocator.operation();
                    let value_id = allocator.value();
                    let operation = Operation {
                        id: operation_id.clone(),
                        opcode: Opcode::Constant,
                        operands: Vec::new(),
                        results: vec![value_id.clone()],
                        attributes: BTreeMap::from([("value".to_owned(), json!(constant))]),
                        region: None,
                        provenance: action_id.clone(),
                        result_types: vec![ty.clone()],
                    };
                    program.values.insert(
                        value_id.clone(),
                        ValueDef {
                            id: value_id.clone(),
                            ty: ty.clone(),
                            origin: ValueOrigin::Operation(operation_id.clone()),
                            name: None,
                        },
                    );
                    program.operations.insert(operation_id.clone(), operation);
                    program.operation_order.push(operation_id);
                    program.constants.insert(value_id.clone(), constant);
                    bindings.insert(bind.clone(), Binding::Value(value_id));
                    inferred.insert(bind.clone(), ty.clone());
                }
                Action::CreateHole {
                    bind,
                    expected_type,
                    shape_constraints,
                } => {
                    ensure_new_binding(&bindings, bind)?;
                    validate_type_symbols(&program, expected_type)?;
                    if let Some(facts) = facts.as_ref() {
                        let mut local_facts = facts.clone();
                        for constraint in shape_constraints {
                            local_facts.insert(constraint)?;
                        }
                    }
                    let hole_id = allocator.hole();
                    let value_id = allocator.value();
                    program.values.insert(
                        value_id.clone(),
                        ValueDef {
                            id: value_id.clone(),
                            ty: expected_type.clone(),
                            origin: ValueOrigin::Hole(hole_id.clone()),
                            name: None,
                        },
                    );
                    program.holes.insert(
                        hole_id.clone(),
                        Hole {
                            id: hole_id.clone(),
                            placeholder: value_id,
                            expected_type: expected_type.clone(),
                            expected_effects: ExpectedEffects::Pure,
                            shape_constraints: shape_constraints.clone(),
                            status: HoleStatus::Open,
                            provenance: action_id.clone(),
                            filled_with: None,
                        },
                    );
                    bindings.insert(bind.clone(), Binding::Hole(hole_id.clone()));
                    inferred.insert(bind.clone(), expected_type.clone());
                    obligations_created.push(add_obligation(
                        &mut program,
                        &mut allocator,
                        &action_id,
                        ObligationDraft {
                            kind: ObligationKind::HoleFilled,
                            status: ObligationStatus::Open,
                            proposition: json!({"hole": hole_id}),
                            discharge_methods: vec!["fill_hole".to_owned()],
                        },
                        self.limits.obligations_per_program,
                    )?);
                    classification = ActionClassification::Conditional;
                }
                Action::CreateOp {
                    bind,
                    opcode,
                    operands,
                    attributes,
                    region,
                } => {
                    ensure_new_binding(&bindings, bind)?;
                    let opcode = opcode
                        .parse::<Opcode>()
                        .map_err(|message| AgentError::new(ErrorCode::UnknownOpcode, message))?;
                    if matches!(opcode, Opcode::Parameter | Opcode::Constant) {
                        return Err(AgentError::new(
                            ErrorCode::UnknownOpcode,
                            format!("use the dedicated action for `{opcode}`"),
                        ));
                    }
                    let operand_ids: Vec<_> = operands
                        .iter()
                        .map(|reference| resolve_value(reference, &program, &bindings))
                        .collect::<AgentResult<_>>()?;
                    let operand_types: Vec<_> = operand_ids
                        .iter()
                        .map(|value| value_type(&program, value))
                        .collect::<AgentResult<_>>()?;
                    let (verified_region, region_classification, mut shape_relations) = match region
                    {
                        Some(region) => {
                            let (region, classification, relations) = build_region(
                                opcode,
                                region,
                                &operand_types,
                                &program,
                                &bindings,
                                facts.as_ref(),
                            )?;
                            (Some(region), classification, relations)
                        }
                        None => (None, ActionClassification::Legal, Vec::new()),
                    };
                    let operation_inference =
                        if let (Some(region), Some(facts)) = (&verified_region, facts.as_ref()) {
                            infer_higher_with_facts(opcode, &operand_types, region, facts)?
                        } else if let Some(region) = &verified_region {
                            infer_higher(opcode, &operand_types, region)?
                        } else if let Some(facts) = facts.as_ref() {
                            infer_primitive_with_facts(opcode, &operand_types, attributes, facts)?
                        } else {
                            infer_primitive(opcode, &operand_types, attributes)?
                        };
                    shape_relations.extend(operation_inference.shape_relations.clone());
                    if matches!(
                        operation_inference.classification,
                        ActionClassification::Conditional
                    ) || matches!(region_classification, ActionClassification::Conditional)
                    {
                        classification = ActionClassification::Conditional;
                        if semantics_version == LEGACY_CORE_SEMANTICS_VERSION {
                            obligations_created.push(add_obligation(
                                &mut program,
                                &mut allocator,
                                &action_id,
                                ObligationDraft {
                                    kind: ObligationKind::ShapeCompatible,
                                    status: ObligationStatus::Open,
                                    proposition: json!({"opcode": opcode, "operands": operand_ids}),
                                    discharge_methods: vec![
                                        "add_constraint".to_owned(),
                                        "specialize_shape".to_owned(),
                                    ],
                                },
                                self.limits.obligations_per_program,
                            )?);
                        } else {
                            shape_relations.sort_by(|left, right| {
                                (&left.left, &left.right).cmp(&(&right.left, &right.right))
                            });
                            shape_relations.dedup();
                            for relation in shape_relations {
                                obligations_created.push(add_shape_obligation(
                                    &mut program,
                                    &mut allocator,
                                    &action_id,
                                    relation,
                                    ShapeObligationContext::Operation {
                                        opcode,
                                        operands: operand_ids.clone(),
                                    },
                                    self.limits.obligations_per_program,
                                )?);
                            }
                        }
                    }
                    let result_type = operation_inference.ty;
                    let operation_id = allocator.operation();
                    let value_id = allocator.value();
                    program.values.insert(
                        value_id.clone(),
                        ValueDef {
                            id: value_id.clone(),
                            ty: result_type.clone(),
                            origin: ValueOrigin::Operation(operation_id.clone()),
                            name: None,
                        },
                    );
                    program.operations.insert(
                        operation_id.clone(),
                        Operation {
                            id: operation_id.clone(),
                            opcode,
                            operands: operand_ids,
                            results: vec![value_id.clone()],
                            attributes: attributes.clone(),
                            region: verified_region,
                            provenance: action_id,
                            result_types: vec![result_type.clone()],
                        },
                    );
                    program.operation_order.push(operation_id);
                    bindings.insert(bind.clone(), Binding::Value(value_id));
                    inferred.insert(bind.clone(), result_type);
                }
                Action::FillHole { hole, value } => {
                    let hole_id = resolve_hole(hole, &program, &bindings)?;
                    let value_id = resolve_value(value, &program, &bindings)?;
                    let value_ty = value_type(&program, &value_id)?;
                    let expected_ty = program
                        .holes
                        .get(&hole_id)
                        .ok_or_else(|| AgentError::new(ErrorCode::UnknownReference, hole))?
                        .expected_type
                        .clone();
                    let mut fill_relation = None;
                    let fill_classification = match (&expected_ty, &value_ty) {
                        (Type::Scalar(left), Type::Scalar(right)) if left == right => {
                            ActionClassification::Legal
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
                        ) if left_element == right_element => {
                            let status = if let Some(facts) = facts.as_ref() {
                                let mut local_facts = facts.clone();
                                for constraint in &program
                                    .holes
                                    .get(&hole_id)
                                    .expect("resolved hole exists")
                                    .shape_constraints
                                {
                                    local_facts.insert(constraint)?;
                                }
                                local_facts.query_types(&expected_ty, &value_ty)?
                            } else {
                                match same_shape(left_shape, right_shape) {
                                    SolverStatus::Proved => ConstraintQueryResult::Proved {
                                        proof: crate::constraints::ConstraintProof {
                                            normalized_left: expected_ty.to_string(),
                                            normalized_right: value_ty.to_string(),
                                            facts: Vec::new(),
                                        },
                                    },
                                    SolverStatus::Unknown => ConstraintQueryResult::Unknown,
                                    SolverStatus::Contradiction => {
                                        ConstraintQueryResult::Contradiction {
                                            contradiction:
                                                crate::constraints::ConstraintContradiction {
                                                    normalized_constraint:
                                                        crate::shapes::ShapeConstraint::Equal {
                                                            left: left_shape.clone(),
                                                            right: right_shape.clone(),
                                                        },
                                                    conflicting_facts: Vec::new(),
                                                    expected: expected_ty.to_string(),
                                                    actual: value_ty.to_string(),
                                                },
                                        }
                                    }
                                }
                            };
                            match status {
                                ConstraintQueryResult::Proved { .. } => ActionClassification::Legal,
                                ConstraintQueryResult::Unknown => {
                                    fill_relation = Some(ShapeRelation {
                                        left: expected_ty.clone(),
                                        right: value_ty.clone(),
                                    });
                                    ActionClassification::Conditional
                                }
                                ConstraintQueryResult::Contradiction { .. } => {
                                    return Err(AgentError::new(
                                        ErrorCode::HoleTypeMismatch,
                                        "hole and value shapes contradict accepted facts",
                                    )
                                    .with_types(expected_ty.to_string(), value_ty.to_string()));
                                }
                            }
                        }
                        _ => {
                            return Err(AgentError::new(
                                ErrorCode::HoleTypeMismatch,
                                "value type does not satisfy hole",
                            )
                            .with_types(expected_ty.to_string(), value_ty.to_string()));
                        }
                    };
                    let hole = program
                        .holes
                        .get_mut(&hole_id)
                        .expect("resolved hole exists");
                    if hole.filled_with.is_some() {
                        return Err(AgentError::new(
                            ErrorCode::TransactionRejected,
                            format!("hole `{hole_id}` is already filled"),
                        ));
                    }
                    hole.filled_with = Some(value_id.clone());
                    hole.status = HoleStatus::Filled;
                    for obligation in program.obligations.values_mut() {
                        if obligation.kind == ObligationKind::HoleFilled
                            && obligation.proposition.get("hole") == Some(&json!(hole_id))
                        {
                            obligation.status = ObligationStatus::Proved;
                        }
                    }
                    classification = fill_classification;
                    if matches!(classification, ActionClassification::Conditional) {
                        if semantics_version == LEGACY_CORE_SEMANTICS_VERSION {
                            obligations_created.push(add_obligation(
                                &mut program,
                                &mut allocator,
                                &action_id,
                                ObligationDraft {
                                    kind: ObligationKind::ShapeCompatible,
                                    status: ObligationStatus::Open,
                                    proposition: json!({"hole": hole_id}),
                                    discharge_methods: vec!["add_constraint".to_owned()],
                                },
                                self.limits.obligations_per_program,
                            )?);
                        } else {
                            obligations_created.push(add_shape_obligation(
                                &mut program,
                                &mut allocator,
                                &action_id,
                                fill_relation.expect("conditional fact query has a relation"),
                                ShapeObligationContext::Hole {
                                    hole: hole_id,
                                    value: value_id,
                                },
                                self.limits.obligations_per_program,
                            )?);
                        }
                    }
                }
                Action::SetOutput { name, value } => {
                    let value = resolve_value(value, &program, &bindings)?;
                    program.outputs.insert(name.clone(), value);
                }
                Action::AddConstraint { constraint } => {
                    if let Some(current_facts) = facts.as_mut() {
                        let mut staged_facts = current_facts.clone();
                        staged_facts.insert(constraint)?;
                        if !program.constraints.contains(constraint) {
                            program.constraints.push(constraint.clone());
                        }
                        discharge_shape_obligations(&mut program, &staged_facts, true)?;
                        *current_facts = staged_facts;
                    } else {
                        program.constraints.push(constraint.clone());
                    }
                }
                Action::FreezeSpec => {
                    if let Some(facts) = facts.as_ref() {
                        discharge_shape_obligations(&mut program, facts, true)?;
                    }
                    require_complete(&program)?;
                    program.frozen = true;
                    let outputs = program.outputs.keys().cloned().collect::<Vec<_>>();
                    obligations_created.push(add_obligation(
                        &mut program,
                        &mut allocator,
                        &action_id,
                        ObligationDraft {
                            kind: ObligationKind::SpecComplete,
                            status: ObligationStatus::Proved,
                            proposition: json!({"outputs": outputs}),
                            discharge_methods: Vec::new(),
                        },
                        self.limits.obligations_per_program,
                    )?);
                }
                Action::ForkRevision => {}
            }
            classifications.push(classification);
        }

        let transaction_id = allocator.transaction();
        let revision_id = allocator.revision();
        for obligation_id in &obligations_created {
            if let Some(obligation) = program.obligations.get_mut(obligation_id) {
                obligation.origin.revision = Some(revision_id.clone());
            }
        }
        let hash = content_hash_with_limit(&program, self.limits.canonical_output_bytes)?;
        let (spec_hash, semantic_canonical_version) =
            semantic_metadata(&program, self.limits.canonical_output_bytes)?;
        let revision = Revision {
            id: revision_id.clone(),
            parents: vec![transaction.base_revision.clone()],
            content_hash: hash.clone(),
            spec_hash: spec_hash.clone(),
            semantic_canonical_version,
            status: StatusSummary::from_program(&program),
            program,
            applied_transaction: Some(transaction_id.clone()),
            created_at_unix_ms: now_unix_ms(),
        };
        self.allocator = allocator;
        self.revisions.insert(revision_id.clone(), revision);
        self.head = revision_id.clone();
        self.events.push(VersionedWorkspaceEvent {
            semantics_version,
            event: WorkspaceEvent::TransactionApplied {
                transaction_id: transaction_id.clone(),
                revision: revision_id.clone(),
                content_hash: hash.clone(),
                transaction: transaction.clone(),
            },
        });
        let bindings = bindings
            .into_iter()
            .map(|(binding, value)| (binding, value.persistent_id()))
            .collect();
        Ok(CommitResult {
            transaction: transaction_id,
            revision: revision_id,
            bindings,
            inferred,
            classifications,
            obligations_created,
            content_hash: hash,
            spec_hash,
            semantic_canonical_version,
        })
    }

    /// Creates an explicit child snapshot of any existing revision.
    pub fn fork(&mut self, base_revision: &RevisionId) -> AgentResult<RevisionId> {
        self.fork_with_semantics(base_revision, CORE_SEMANTICS_VERSION)
    }

    fn fork_with_semantics(
        &mut self,
        base_revision: &RevisionId,
        semantics_version: u32,
    ) -> AgentResult<RevisionId> {
        if !matches!(
            semantics_version,
            LEGACY_CORE_SEMANTICS_VERSION | CORE_SEMANTICS_VERSION
        ) {
            return Err(AgentError::new(
                ErrorCode::PersistenceFormat,
                format!("unsupported compiler semantics version {semantics_version}"),
            ));
        }
        let base = self.revision(base_revision)?.clone();
        let revision_id = self.allocator.revision();
        let hash = base.content_hash;
        let revision = Revision {
            id: revision_id.clone(),
            parents: vec![base_revision.clone()],
            content_hash: hash.clone(),
            spec_hash: base.spec_hash,
            semantic_canonical_version: base.semantic_canonical_version,
            status: base.status,
            program: base.program,
            applied_transaction: None,
            created_at_unix_ms: now_unix_ms(),
        };
        self.revisions.insert(revision_id.clone(), revision);
        self.head = revision_id.clone();
        self.events.push(VersionedWorkspaceEvent {
            semantics_version,
            event: WorkspaceEvent::RevisionForked {
                base_revision: base_revision.clone(),
                revision: revision_id.clone(),
                content_hash: hash,
            },
        });
        Ok(revision_id)
    }

    fn frozen_candidate_source(&self, revision: &RevisionId) -> AgentResult<(Program, SpecHash)> {
        let source = self.revision(revision)?;
        if !source.program.frozen {
            return Err(AgentError::new(
                ErrorCode::SpecNotFrozen,
                format!("revision `{revision}` is not a frozen SpecIR"),
            ));
        }
        let spec_hash = source.spec_hash.clone().ok_or_else(|| {
            AgentError::new(
                ErrorCode::SpecHashMismatch,
                format!("frozen revision `{revision}` has no valid spec_hash"),
            )
        })?;
        Ok((source.program.clone(), spec_hash))
    }

    fn candidate_source(&self, candidate: &CandidateId) -> AgentResult<(Program, SpecHash)> {
        let spec_revision = self.candidates.candidate(candidate)?.spec_revision.clone();
        self.frozen_candidate_source(&spec_revision)
    }

    /// Creates a separate identity ImplIR candidate for a frozen SpecIR revision.
    pub fn candidate_create(
        &mut self,
        spec_revision: &RevisionId,
        relation: RelationKind,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.frozen_candidate_source(spec_revision)?;
        self.candidates.create(
            spec_revision.clone(),
            spec_hash,
            &source,
            relation,
            &self.limits,
        )
    }

    /// Returns one persistent candidate branch.
    pub fn candidate_query(&self, candidate: &CandidateId) -> AgentResult<&Candidate> {
        self.candidates.candidate(candidate)
    }

    /// Returns one immutable candidate revision.
    pub fn candidate_revision(
        &self,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
    ) -> AgentResult<&CandidateRevision> {
        self.candidates.revision(candidate, revision)
    }

    /// Verifies one candidate revision against its frozen SpecIR anchor.
    pub fn candidate_check(
        &self,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, _) = self.candidate_source(candidate)?;
        self.candidates
            .check(candidate, revision, &source, &self.limits)
    }

    /// Applies an atomic trusted rewrite transaction to one candidate branch.
    pub fn candidate_apply(
        &mut self,
        transaction: &CandidateTransaction,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.candidate_source(&transaction.candidate)?;
        self.candidates
            .apply(transaction, &source, &spec_hash, &self.limits)
    }

    /// Accepts one bounded typed speculative replacement against an explicit candidate head.
    pub fn candidate_propose(
        &mut self,
        candidate: &CandidateId,
        base_revision: &CandidateRevisionId,
        proposal: &SpeculativeRewriteProposal,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.candidates.propose(
            candidate,
            base_revision,
            proposal,
            &source,
            &spec_hash,
            &self.limits,
        )
    }

    /// Returns one persistent normalized proposal provenance record.
    pub fn candidate_proposal_query(&self, proposal: &ProposalId) -> AgentResult<&ProposalRecord> {
        self.candidates.proposal(proposal)
    }

    /// Runs trusted ordered translation validation for one proposal obligation.
    pub fn candidate_translation_check(
        &mut self,
        candidate: &CandidateId,
        base_revision: &CandidateRevisionId,
        proposal: &ProposalId,
    ) -> AgentResult<TranslationCheckReport> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.candidates.translation_check(
            candidate,
            base_revision,
            proposal,
            &source,
            &spec_hash,
            &self.limits,
        )
    }

    /// Forks one candidate revision into a new editable branch identity.
    pub fn candidate_fork(
        &mut self,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.candidates
            .fork(candidate, revision, &source, &spec_hash, &self.limits)
    }

    /// Records deterministic differential confidence evidence.
    pub fn candidate_record_validation(
        &mut self,
        candidate: &CandidateId,
        base_revision: &CandidateRevisionId,
        validation: DifferentialValidation,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.candidates.record_validation(
            candidate,
            base_revision,
            validation,
            &source,
            &spec_hash,
            &self.limits,
        )
    }

    /// Seals a fully verified exact candidate; repeated seal is idempotent.
    pub fn candidate_seal(
        &mut self,
        candidate: &CandidateId,
        base_revision: &CandidateRevisionId,
    ) -> AgentResult<CandidateCheckReport> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.candidates
            .seal(candidate, base_revision, &source, &spec_hash, &self.limits)
    }

    /// Generates a bounded deterministic known-rewrite continuation.
    pub fn candidate_continuation(
        &self,
        candidate: &CandidateId,
        revision: &CandidateRevisionId,
    ) -> AgentResult<CandidateContinuation> {
        self.candidates
            .continuation(candidate, revision, &self.limits)
    }

    fn replay_candidate_forest(&mut self, expected: &CandidateForest) -> AgentResult<()> {
        for versioned in &expected.events {
            if !matches!(
                versioned.semantics_version,
                LEGACY_CANDIDATE_SEMANTICS_VERSION | CANDIDATE_SEMANTICS_VERSION
            ) {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "unsupported candidate semantics version {}",
                        versioned.semantics_version
                    ),
                )
                .with_detail("candidate_semantics_version", versioned.semantics_version));
            }
            match &versioned.event {
                CandidateEvent::Created {
                    spec_revision,
                    relation,
                    ..
                } => {
                    self.candidate_create(spec_revision, *relation)?;
                }
                CandidateEvent::TransactionApplied { transaction, .. } => {
                    self.candidate_apply(transaction)?;
                }
                CandidateEvent::Forked {
                    parent_candidate,
                    parent_revision,
                    ..
                } => {
                    self.candidate_fork(parent_candidate, parent_revision)?;
                }
                CandidateEvent::Validated {
                    candidate,
                    base_revision,
                    validation,
                    ..
                } => {
                    self.candidate_record_validation(candidate, base_revision, validation.clone())?;
                }
                CandidateEvent::Sealed {
                    candidate,
                    base_revision,
                    ..
                } => {
                    self.candidate_seal(candidate, base_revision)?;
                }
                CandidateEvent::ProposalAccepted {
                    candidate,
                    base_revision,
                    proposal,
                    ..
                } => {
                    self.candidate_propose(candidate, base_revision, proposal)?;
                }
                CandidateEvent::TranslationChecked {
                    candidate,
                    base_revision,
                    proposal,
                    ..
                } => {
                    self.candidate_translation_check(candidate, base_revision, proposal)?;
                }
            }
            if self.candidates.events.last() != Some(versioned) {
                return Err(AgentError::new(
                    ErrorCode::ReplayMismatch,
                    "candidate event replay diverged",
                )
                .with_detail("expected_event", json!(versioned))
                .with_detail("actual_event", json!(self.candidates.events.last())));
            }
        }
        if &self.candidates != expected {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed CandidateForest differs from snapshot",
            ));
        }
        let revisions = &self.revisions;
        self.candidates.verify_all(
            |revision| {
                let source = revisions.get(revision).ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::RevisionNotFound,
                        format!("candidate anchor revision `{revision}` does not exist"),
                    )
                })?;
                let spec_hash = source.spec_hash.clone().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::SpecHashMismatch,
                        format!("candidate anchor revision `{revision}` has no spec_hash"),
                    )
                })?;
                Ok((source.program.clone(), spec_hash))
            },
            &self.limits,
        )
    }

    /// Captures all state required to resume and replay this workspace.
    #[must_use]
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            schema_version: WORKSPACE_SNAPSHOT_VERSION,
            workspace: self.id.clone(),
            head: self.head.clone(),
            revisions: self.revisions.clone(),
            allocator: self.allocator.clone(),
            events: self.events.clone(),
            candidate_forest: self.candidates.clone(),
        }
    }

    /// Reconstructs and verifies a workspace from its event-backed snapshot.
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> AgentResult<(Self, ReplayReport)> {
        Self::from_snapshot_with_cache_policy(snapshot, false)
    }

    /// Verifies a structurally migrated v1 snapshot and populates its new semantic cache fields.
    pub fn from_legacy_migrated_snapshot(
        snapshot: WorkspaceSnapshot,
    ) -> AgentResult<(Self, ReplayReport)> {
        Self::from_snapshot_with_cache_policy(snapshot, true)
    }

    fn from_snapshot_with_cache_policy(
        mut snapshot: WorkspaceSnapshot,
        populate_missing_semantic_cache: bool,
    ) -> AgentResult<(Self, ReplayReport)> {
        if snapshot.schema_version != WORKSPACE_SNAPSHOT_VERSION {
            return Err(AgentError::new(
                ErrorCode::PersistenceFormat,
                format!(
                    "workspace snapshot version {} is unsupported; expected {}",
                    snapshot.schema_version, WORKSPACE_SNAPSHOT_VERSION
                ),
            ));
        }
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::RevisionsPerArchive,
            as_u64(snapshot.revisions.len()),
            "snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::EventsPerArchive,
            as_u64(snapshot.events.len()),
            "snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::CandidateEventsPerArchive,
            as_u64(snapshot.candidate_forest.events.len()),
            "candidate snapshot replay preflight",
        )?;
        let replay_actions = snapshot.events.iter().fold(0_u64, |total, versioned| {
            total.saturating_add(match &versioned.event {
                WorkspaceEvent::TransactionApplied { transaction, .. } => {
                    as_u64(transaction.actions.len())
                }
                WorkspaceEvent::RevisionForked { .. } => 0,
            })
        });
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::ActionsReplayedPerArchive,
            replay_actions,
            "snapshot replay preflight",
        )?;
        let mut replayed = Self::with_limits(
            snapshot.workspace.clone(),
            ResourceLimits::hard_safety_caps(),
        )?;
        for versioned in &snapshot.events {
            if !matches!(
                versioned.semantics_version,
                LEGACY_CORE_SEMANTICS_VERSION | CORE_SEMANTICS_VERSION
            ) {
                return Err(AgentError::new(
                    ErrorCode::PersistenceFormat,
                    format!(
                        "unsupported compiler semantics version {}",
                        versioned.semantics_version
                    ),
                )
                .with_detail("semantics_version", versioned.semantics_version));
            }
            match &versioned.event {
                WorkspaceEvent::TransactionApplied {
                    transaction_id,
                    revision,
                    content_hash: expected_hash,
                    transaction,
                } => {
                    let commit =
                        replayed.apply_with_semantics(transaction, versioned.semantics_version)?;
                    if commit.transaction != *transaction_id
                        || commit.revision != *revision
                        || commit.content_hash != *expected_hash
                    {
                        return Err(AgentError::new(
                            ErrorCode::ReplayMismatch,
                            format!("transaction replay diverged at revision `{revision}`"),
                        )
                        .with_detail("expected_revision", revision.to_string())
                        .with_detail("actual_revision", commit.revision.to_string())
                        .with_detail("expected_hash", expected_hash.clone())
                        .with_detail("actual_hash", commit.content_hash));
                    }
                }
                WorkspaceEvent::RevisionForked {
                    base_revision,
                    revision,
                    content_hash: expected_hash,
                } => {
                    let actual_revision =
                        replayed.fork_with_semantics(base_revision, versioned.semantics_version)?;
                    let actual_hash = replayed.revision(&actual_revision)?.content_hash.clone();
                    if actual_revision != *revision || actual_hash != *expected_hash {
                        return Err(AgentError::new(
                            ErrorCode::ReplayMismatch,
                            format!("fork replay diverged at revision `{revision}`"),
                        )
                        .with_detail("expected_revision", revision.to_string())
                        .with_detail("actual_revision", actual_revision.to_string())
                        .with_detail("expected_hash", expected_hash.clone())
                        .with_detail("actual_hash", actual_hash));
                    }
                }
            }
        }
        if replayed.head != snapshot.head {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed head does not match snapshot head",
            )
            .with_detail("expected_head", snapshot.head.to_string())
            .with_detail("actual_head", replayed.head.to_string()));
        }
        if replayed.revisions.len() != snapshot.revisions.len() {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed revision count does not match snapshot",
            )
            .with_detail("expected_revisions", snapshot.revisions.len() as u64)
            .with_detail("actual_revisions", replayed.revisions.len() as u64));
        }
        if replayed.events != snapshot.events {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed event log differs from snapshot",
            ));
        }
        if !replayed
            .allocator
            .same_persistent_state(&snapshot.allocator)
        {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "persistent ID allocator state differs after replay",
            ));
        }

        let mut content_hashes_verified = 0;
        let mut spec_hashes_verified = 0;
        let mut semantic_cache_updates = Vec::new();
        for (id, expected) in &snapshot.revisions {
            if expected.id != *id {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    format!("revision map key `{id}` does not match embedded ID"),
                ));
            }
            let recomputed = content_hash(&expected.program)?;
            if recomputed != expected.content_hash {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    format!("revision `{id}` content hash is invalid"),
                )
                .with_detail("expected_hash", expected.content_hash.clone())
                .with_detail("actual_hash", recomputed));
            }
            content_hashes_verified += 1;
            if expected.status != StatusSummary::from_program(&expected.program) {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    format!("revision `{id}` status summary is invalid"),
                ));
            }
            let actual = replayed.revisions.get(id).ok_or_else(|| {
                AgentError::new(
                    ErrorCode::ReplayMismatch,
                    format!("replay did not create revision `{id}`"),
                )
            })?;
            if actual.parents != expected.parents
                || actual.content_hash != expected.content_hash
                || actual.program != expected.program
                || actual.applied_transaction != expected.applied_transaction
                || actual.status != expected.status
            {
                return Err(AgentError::new(
                    ErrorCode::ReplayMismatch,
                    format!("replayed revision `{id}` differs from snapshot"),
                ));
            }

            let (recomputed_spec_hash, recomputed_version) = semantic_metadata(
                &expected.program,
                ResourceLimits::hard_safety_caps().canonical_output_bytes,
            )?;
            if populate_missing_semantic_cache {
                if expected.spec_hash.is_some() || expected.semantic_canonical_version.is_some() {
                    return Err(AgentError::new(
                        ErrorCode::PersistenceIntegrity,
                        format!(
                            "structurally migrated legacy revision `{id}` unexpectedly contains semantic cache data"
                        ),
                    ));
                }
                semantic_cache_updates.push((
                    id.clone(),
                    recomputed_spec_hash.clone(),
                    recomputed_version,
                ));
            } else if expected.spec_hash != recomputed_spec_hash
                || expected.semantic_canonical_version != recomputed_version
            {
                return Err(AgentError::new(
                    ErrorCode::PersistenceIntegrity,
                    format!("revision `{id}` semantic hash metadata is invalid"),
                )
                .with_detail("expected_spec_hash", json!(expected.spec_hash))
                .with_detail("actual_spec_hash", json!(recomputed_spec_hash))
                .with_detail(
                    "expected_semantic_canonical_version",
                    json!(expected.semantic_canonical_version),
                )
                .with_detail(
                    "actual_semantic_canonical_version",
                    json!(recomputed_version),
                ));
            }
            if actual.spec_hash != recomputed_spec_hash
                || actual.semantic_canonical_version != recomputed_version
            {
                return Err(AgentError::new(
                    ErrorCode::ReplayMismatch,
                    format!("replayed revision `{id}` semantic metadata differs"),
                ));
            }
            if recomputed_spec_hash.is_some() {
                spec_hashes_verified += 1;
            }
        }
        for (id, spec_hash, version) in semantic_cache_updates {
            let revision = snapshot
                .revisions
                .get_mut(&id)
                .expect("verified migrated revision exists");
            revision.spec_hash = spec_hash;
            revision.semantic_canonical_version = version;
        }

        replayed.replay_candidate_forest(&snapshot.candidate_forest)?;

        let report = ReplayReport {
            workspace: snapshot.workspace.clone(),
            head: snapshot.head.clone(),
            revisions_verified: snapshot.revisions.len(),
            events_replayed: snapshot.events.len(),
            content_hashes_verified,
            spec_hashes_verified,
            candidates_verified: snapshot.candidate_forest.candidates.len(),
            candidate_events_replayed: snapshot.candidate_forest.events.len(),
            evidence_records_verified: snapshot.candidate_forest.evidence.len(),
        };
        replayed.revisions = snapshot.revisions;
        replayed.head = snapshot.head;
        replayed.allocator = snapshot.allocator;
        replayed.events = snapshot.events;
        replayed.candidates = snapshot.candidate_forest;
        replayed.limits = ResourceLimits::default();
        Ok((replayed, report))
    }

    /// Recomputes semantic canonical form and verifies the revision's cached metadata.
    pub fn semantic_canonical(
        &self,
        revision: &RevisionId,
    ) -> AgentResult<SemanticCanonicalization> {
        let revision = self.revision(revision)?;
        let canonical =
            canonicalize_spec_with_limit(&revision.program, self.limits.canonical_output_bytes)?;
        if revision.spec_hash.as_ref() != Some(&canonical.spec_hash)
            || revision.semantic_canonical_version != Some(SPEC_CANONICAL_VERSION)
        {
            return Err(AgentError::new(
                ErrorCode::CanonicalizationFailed,
                format!(
                    "revision `{}` cached semantic hash metadata does not match recomputation",
                    revision.id
                ),
            )
            .with_detail("cached_spec_hash", json!(revision.spec_hash))
            .with_detail("actual_spec_hash", canonical.spec_hash.to_string())
            .with_detail(
                "cached_semantic_canonical_version",
                json!(revision.semantic_canonical_version),
            )
            .with_detail("actual_semantic_canonical_version", SPEC_CANONICAL_VERSION));
        }
        Ok(canonical)
    }

    /// Computes a deterministic structural diff between two revisions.
    pub fn diff(&self, from: &RevisionId, to: &RevisionId) -> AgentResult<RevisionDiff> {
        Ok(diff(self.revision(from)?, self.revision(to)?))
    }

    /// Generates a parameteric continuation for one open hole.
    pub fn continuation(
        &mut self,
        revision: &RevisionId,
        hole: &HoleId,
        mode: InteractionMode,
    ) -> AgentResult<ContinuationFrame> {
        let revision_data = self.revision(revision)?;
        let hole_data = revision_data
            .program
            .holes
            .get(hole)
            .cloned()
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::UnknownReference,
                    format!("hole `{hole}` does not exist"),
                )
            })?;
        if matches!(hole_data.status, HoleStatus::Filled) {
            return Err(AgentError::new(
                ErrorCode::TransactionRejected,
                format!("hole `{hole}` is already filled"),
            ));
        }
        let program = revision_data.program.clone();
        let frame_id = self.allocator.frame();
        build_frame(frame_id, revision.clone(), &program, &hole_data, mode)
    }
}

#[cfg(test)]
mod semantics_tests {
    use super::*;

    #[test]
    fn legacy_shape_obligation_semantics_replay_without_structured_discharge() {
        let mut workspace = Workspace::with_limits(
            WorkspaceId::new("legacy-shape"),
            ResourceLimits::hard_safety_caps(),
        )
        .unwrap();
        let transaction = Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::DefineDimension {
                    bind: None,
                    name: "N".to_owned(),
                    constraints: vec!["N >= 0".to_owned()],
                },
                Action::DefineDimension {
                    bind: None,
                    name: "M".to_owned(),
                    constraints: vec!["M >= 0".to_owned()],
                },
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "tensor<f32,[N]>".parse().unwrap(),
                },
                Action::CreateParameter {
                    bind: "$y".to_owned(),
                    name: "y".to_owned(),
                    ty: "tensor<f32,[M]>".parse().unwrap(),
                },
                Action::CreateOp {
                    bind: "$sum".to_owned(),
                    opcode: "add".to_owned(),
                    operands: vec!["$x".to_owned(), "$y".to_owned()],
                    attributes: BTreeMap::new(),
                    region: None,
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        };
        let commit = workspace
            .apply_with_semantics(&transaction, LEGACY_CORE_SEMANTICS_VERSION)
            .unwrap();
        let revision = workspace.revision(&commit.revision).unwrap();
        let shape = revision
            .program
            .obligations
            .values()
            .find(|obligation| obligation.kind == ObligationKind::ShapeCompatible)
            .unwrap();
        assert_eq!(
            shape.proposition,
            json!({"opcode": "add", "operands": ["v1", "v2"]})
        );
        assert!(shape.shape_compatibility.is_none());
        let snapshot = workspace.snapshot();
        let (replayed, report) = Workspace::from_snapshot(snapshot.clone()).unwrap();
        assert_eq!(report.events_replayed, 1);
        assert_eq!(replayed.snapshot(), snapshot);
    }
}
