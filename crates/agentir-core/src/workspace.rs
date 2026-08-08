//! Workspace state machine, verification, and atomic revision commits.

use crate::{
    actions::{Action, ActionClassification, RegionSpec, Transaction},
    backend::{
        ArtifactCheckReport, ArtifactQuery, ArtifactStore, BackendAllocator, BackendCheckReport,
        BackendEvent, BackendHash, BackendQuery, BackendStore, MeasurementStore,
        canonical_backend_bytes,
    },
    backend_ir::{ArtifactPackage, BackendAnchor, BackendKind, BackendProgram},
    candidate::{
        CANDIDATE_SEMANTICS_VERSION, Candidate, CandidateCheckReport, CandidateContinuation,
        CandidateEvent, CandidateForest, CandidateRevision, CandidateTransaction,
        DifferentialValidation, EQUALITY_CANDIDATE_SEMANTICS_VERSION,
        LEGACY_CANDIDATE_SEMANTICS_VERSION, ProposalRecord, RelationKind,
        SpeculativeRewriteProposal, TranslationCheckReport, VersionedCandidateEvent,
    },
    canonical::{content_hash, content_hash_with_limit},
    constraints::{ConstraintFacts, ConstraintQueryResult},
    continuation::{ContinuationFrame, InteractionMode, build_frame},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    equality::{
        EQUALITY_SEMANTICS_VERSION, EqualityContinuation, EqualityDischargeResult, EqualityEvent,
        EqualityExpansionResult, EqualityExplanation, EqualityHash, EqualityMaterializationResult,
        EqualityQuery, EqualityStore, VersionedEqualityEvent,
    },
    holes::{ExpectedEffects, Hole, HoleStatus},
    ids::{
        ActionId, ArtifactId, BackendPlanId, BackendRevisionId, BufferId, CandidateId,
        CandidateRevisionId, EqualityNodeId, EqualityRevisionId, EqualitySpaceId, HoleId,
        IdAllocator, MemoryPlanId, MemoryRevisionId, ObligationId, ProposalId, RevisionId,
        ScheduleAxisId, SchedulePlanId, ScheduleRevisionId, TargetManifestId,
        TargetManifestRevisionId, ValueId, WorkspaceId,
    },
    ir::{
        BlockArgument, ConstantValue, Dimension, Opcode, Operation, Program, Region,
        RegionOperation, RegionValue, ValueDef, ValueOrigin,
    },
    memory::{
        MEMORY_EVENT_SEMANTICS_VERSION, MemoryCheckReport, MemoryContinuation, MemoryEvent,
        MemoryHash, MemoryPlanStore, MemoryQuery, MemoryTransaction, VersionedMemoryEvent,
    },
    memory_ir::{AliasFact, MemoryBuffer, MemoryProgram},
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
    schedule::{
        SCHEDULE_EVENT_SEMANTICS_VERSION, ScheduleCheckReport, ScheduleContinuation, ScheduleEvent,
        ScheduleHash, ScheduleLegalityQuery, SchedulePlanStore, ScheduleQuery, ScheduleTransaction,
        VersionedScheduleEvent, canonical_schedule_bytes,
    },
    schedule_ir::{ScheduleAxis, ScheduleResourceEstimate},
    semantic::{
        SPEC_CANONICAL_VERSION, SemanticCanonicalization, SpecHash, canonicalize_spec_with_limit,
    },
    shapes::{SolverStatus, same_shape},
    spec::{
        ShapeRelation, infer_higher, infer_higher_with_facts, infer_primitive,
        infer_primitive_with_facts,
    },
    target::{
        TARGET_EVENT_SEMANTICS_VERSION, TargetCheckReport, TargetEvent, TargetManifestStore,
        TargetProfile, TargetQuery, VersionedTargetEvent, canonical_target_bytes,
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
    equality: EqualityStore,
    memory: MemoryPlanStore,
    targets: TargetManifestStore,
    schedules: SchedulePlanStore,
    backends: BackendStore,
    artifacts: ArtifactStore,
    measurements: MeasurementStore,
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
            equality: EqualityStore::default(),
            memory: MemoryPlanStore::default(),
            targets: TargetManifestStore::default(),
            schedules: SchedulePlanStore::default(),
            backends: BackendStore::default(),
            artifacts: ArtifactStore::default(),
            measurements: MeasurementStore::default(),
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

    /// Returns the persistent exact equality-space store.
    #[must_use]
    pub const fn equality_store(&self) -> &EqualityStore {
        &self.equality
    }

    /// Returns the independent persistent MemoryIR plan store.
    #[must_use]
    pub const fn memory_store(&self) -> &MemoryPlanStore {
        &self.memory
    }

    /// Returns the immutable compiler-owned target manifest store.
    #[must_use]
    pub const fn target_store(&self) -> &TargetManifestStore {
        &self.targets
    }

    /// Returns the independent persistent ScheduleIR plan store.
    #[must_use]
    pub const fn schedule_store(&self) -> &SchedulePlanStore {
        &self.schedules
    }

    /// Returns persistent typed BackendIR plans.
    #[must_use]
    pub const fn backend_store(&self) -> &BackendStore {
        &self.backends
    }

    /// Returns deterministic WGSL artifact packages.
    #[must_use]
    pub const fn artifact_store(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// Returns confidence-only hardware measurement records.
    #[must_use]
    pub const fn measurement_store(&self) -> &MeasurementStore {
        &self.measurements
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

    /// Creates an exact equality space from one explicit proved candidate revision.
    pub fn equality_create(
        &mut self,
        candidate: &CandidateId,
        candidate_revision: &CandidateRevisionId,
    ) -> AgentResult<EqualityQuery> {
        let candidate_data = self.candidates.candidate(candidate)?;
        let spec_revision = candidate_data.spec_revision.clone();
        let (source, spec_hash) = self.frozen_candidate_source(&spec_revision)?;
        self.equality.create(
            &self.candidates,
            candidate,
            candidate_revision,
            &source,
            &spec_revision,
            &spec_hash,
            &self.limits,
        )
    }

    /// Reads one immutable equality revision summary.
    pub fn equality_query(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
    ) -> AgentResult<EqualityQuery> {
        self.equality.query(space, revision)
    }

    fn equality_source(&self, space: &EqualitySpaceId) -> AgentResult<(Program, SpecHash)> {
        let spec_revision = self.equality.space(space)?.anchor.spec_revision.clone();
        self.frozen_candidate_source(&spec_revision)
    }

    /// Expands a bounded number of deterministic equality work items.
    pub fn equality_expand(
        &mut self,
        space: &EqualitySpaceId,
        base_revision: &EqualityRevisionId,
        expected_hash: &EqualityHash,
        fuel: u64,
    ) -> AgentResult<EqualityExpansionResult> {
        let (source, _) = self.equality_source(space)?;
        self.equality.expand(
            space,
            base_revision,
            expected_hash,
            fuel,
            &source,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Saturates deterministically to fixpoint or explicit caller fuel.
    pub fn equality_saturate(
        &mut self,
        space: &EqualitySpaceId,
        base_revision: &EqualityRevisionId,
        expected_hash: &EqualityHash,
        fuel: u64,
    ) -> AgentResult<EqualityExpansionResult> {
        let (source, _) = self.equality_source(space)?;
        self.equality.saturate(
            space,
            base_revision,
            expected_hash,
            fuel,
            &source,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Builds the canonical trusted root-to-node equality explanation.
    pub fn equality_explain(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
        node: &EqualityNodeId,
    ) -> AgentResult<EqualityExplanation> {
        let (source, _) = self.equality_source(space)?;
        self.equality
            .explain(space, revision, node, &source, &self.limits)
    }

    /// Returns bounded deterministic next equality work.
    pub fn equality_continuation(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
    ) -> AgentResult<EqualityContinuation> {
        self.equality.continuation(space, revision, &self.limits)
    }

    /// Returns one equality member implementation for reference evaluation.
    pub fn equality_node_program(
        &self,
        space: &EqualitySpaceId,
        revision: &EqualityRevisionId,
        node: &EqualityNodeId,
    ) -> AgentResult<&crate::impl_ir::ImplProgram> {
        self.equality.node_program(space, revision, node)
    }

    /// Discharges ordered candidate proof debt using a trusted equality membership path.
    #[allow(clippy::too_many_arguments)]
    pub fn candidate_equality_check(
        &mut self,
        candidate: &CandidateId,
        base_candidate_revision: &CandidateRevisionId,
        proposal: &ProposalId,
        space: &EqualitySpaceId,
        equality_revision: &EqualityRevisionId,
        expected_equality_hash: &EqualityHash,
        target_node: &EqualityNodeId,
    ) -> AgentResult<EqualityDischargeResult> {
        let (source, spec_hash) = self.candidate_source(candidate)?;
        self.equality.candidate_discharge(
            &mut self.candidates,
            candidate,
            base_candidate_revision,
            proposal,
            space,
            equality_revision,
            expected_equality_hash,
            target_node,
            &source,
            &spec_hash,
            &self.limits,
        )
    }

    /// Materializes one explicitly selected equality node as a new exact candidate fork.
    pub fn equality_materialize(
        &mut self,
        space: &EqualitySpaceId,
        equality_revision: &EqualityRevisionId,
        expected_equality_hash: &EqualityHash,
        target_node: &EqualityNodeId,
    ) -> AgentResult<EqualityMaterializationResult> {
        let (source, spec_hash) = self.equality_source(space)?;
        self.equality.materialize(
            &mut self.candidates,
            space,
            equality_revision,
            expected_equality_hash,
            target_node,
            &source,
            &spec_hash,
            &self.limits,
        )
    }

    fn memory_source(&self, plan: &MemoryPlanId) -> AgentResult<(Program, SpecHash)> {
        let spec_revision = self.memory.plan(plan)?.anchor.spec_revision.clone();
        self.frozen_candidate_source(&spec_revision)
    }

    /// Creates a conservative exact MemoryIR plan for one proved candidate revision.
    pub fn memory_create(
        &mut self,
        candidate: &CandidateId,
        candidate_revision: &CandidateRevisionId,
    ) -> AgentResult<MemoryCheckReport> {
        let candidate_data = self.candidates.candidate(candidate)?;
        let spec_revision = candidate_data.spec_revision.clone();
        let (source, spec_hash) = self.frozen_candidate_source(&spec_revision)?;
        self.memory.create(
            &self.candidates,
            candidate,
            candidate_revision,
            &source,
            &spec_revision,
            &spec_hash,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Reads one immutable MemoryIR revision summary.
    pub fn memory_query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<MemoryQuery> {
        self.memory.query(plan, revision)
    }

    /// Fully verifies one MemoryIR revision against its immutable ImplIR anchor.
    pub fn memory_check(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<MemoryCheckReport> {
        let (source, _) = self.memory_source(plan)?;
        self.memory
            .check(plan, revision, &self.candidates, &source, &self.limits)
    }

    /// Atomically applies compiler-verified physical-storage decisions.
    pub fn memory_apply(
        &mut self,
        transaction: &MemoryTransaction,
    ) -> AgentResult<MemoryCheckReport> {
        let (source, _) = self.memory_source(&transaction.memory_plan)?;
        self.memory.apply(
            transaction,
            &self.candidates,
            &source,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Forks one immutable MemoryIR revision into an independent plan identity.
    pub fn memory_fork(
        &mut self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        expected_hash: &MemoryHash,
    ) -> AgentResult<MemoryCheckReport> {
        let (source, _) = self.memory_source(plan)?;
        self.memory.fork(
            plan,
            revision,
            expected_hash,
            &self.candidates,
            &source,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Seals one structurally proved exact or guarded MemoryIR plan.
    pub fn memory_seal(
        &mut self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        expected_hash: &MemoryHash,
    ) -> AgentResult<MemoryCheckReport> {
        let (source, _) = self.memory_source(plan)?;
        self.memory.seal(
            plan,
            revision,
            expected_hash,
            &self.candidates,
            &source,
            &self.limits,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
        )
    }

    /// Returns one compiler-owned alias fact without mutating MemoryIR.
    pub fn memory_alias_query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        first: &BufferId,
        second: &BufferId,
    ) -> AgentResult<AliasFact> {
        self.memory.alias_query(plan, revision, first, second)
    }

    /// Returns one immutable typed buffer region.
    pub fn memory_buffer_query(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
        buffer: &BufferId,
    ) -> AgentResult<&MemoryBuffer> {
        self.memory.buffer_query(plan, revision, buffer)
    }

    /// Returns bounded deterministic legal storage choices.
    pub fn memory_continuation(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<MemoryContinuation> {
        let anchor = &self.memory.plan(plan)?.anchor;
        let implementation = &self
            .candidates
            .revision(&anchor.candidate, &anchor.candidate_revision)?
            .impl_program;
        self.memory
            .continuation(plan, revision, implementation, &self.limits)
    }

    /// Returns one immutable MemoryIR program for reference evaluation.
    pub fn memory_program(
        &self,
        plan: &MemoryPlanId,
        revision: &MemoryRevisionId,
    ) -> AgentResult<&MemoryProgram> {
        Ok(&self.memory.revision(plan, revision)?.program)
    }

    /// Returns the immutable ImplIR program anchored by a MemoryIR plan.
    pub fn memory_impl_program(
        &self,
        plan: &MemoryPlanId,
    ) -> AgentResult<&crate::impl_ir::ImplProgram> {
        let anchor = &self.memory.plan(plan)?.anchor;
        Ok(&self
            .candidates
            .revision(&anchor.candidate, &anchor.candidate_revision)?
            .impl_program)
    }

    /// Instantiates one immutable compiler-owned target profile.
    pub fn target_create(&mut self, profile: TargetProfile) -> AgentResult<TargetCheckReport> {
        BudgetCheck::against(
            &self.limits,
            ResourceKind::TargetManifestsPerWorkspace,
            u64::try_from(self.targets.manifests.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "target.create",
        )?;
        BudgetCheck::against(
            &self.limits,
            ResourceKind::TargetEvents,
            u64::try_from(self.targets.events.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "target.create",
        )?;
        let mut staged = self.targets.clone();
        let report = staged.create(profile)?;
        self.validate_target_budgets(&staged)?;
        self.targets = staged;
        Ok(report)
    }

    fn validate_target_budgets(&self, store: &TargetManifestStore) -> AgentResult<()> {
        BudgetCheck::against(
            &self.limits,
            ResourceKind::TargetRevisionsPerWorkspace,
            u64::try_from(store.manifests.len()).unwrap_or(u64::MAX),
            "TargetManifestStore",
        )?;
        for revision in store.manifests.values() {
            BudgetCheck::against(
                &self.limits,
                ResourceKind::TargetCapabilities,
                u64::try_from(revision.manifest.capabilities.len()).unwrap_or(u64::MAX),
                "TargetManifest capabilities",
            )?;
            BudgetCheck::against(
                &self.limits,
                ResourceKind::TargetCanonicalBytes,
                u64::try_from(canonical_target_bytes(&revision.manifest)?.len())
                    .unwrap_or(u64::MAX),
                "TargetManifest canonical form",
            )?;
        }
        Ok(())
    }

    fn validate_schedule_budgets(&self, store: &SchedulePlanStore) -> AgentResult<()> {
        let revisions = store.plans.values().fold(0_u64, |sum, plan| {
            sum.saturating_add(u64::try_from(plan.revisions.len()).unwrap_or(u64::MAX))
        });
        BudgetCheck::against(
            &self.limits,
            ResourceKind::SchedulePlansPerWorkspace,
            u64::try_from(store.plans.len()).unwrap_or(u64::MAX),
            "SchedulePlanStore",
        )?;
        BudgetCheck::against(
            &self.limits,
            ResourceKind::ScheduleRevisionsPerWorkspace,
            revisions,
            "SchedulePlanStore",
        )?;
        BudgetCheck::against(
            &self.limits,
            ResourceKind::ScheduleEvents,
            u64::try_from(store.events.len()).unwrap_or(u64::MAX),
            "SchedulePlanStore",
        )?;
        BudgetCheck::against(
            &self.limits,
            ResourceKind::ScheduleEvidenceRecords,
            u64::try_from(store.evidence.len()).unwrap_or(u64::MAX),
            "SchedulePlanStore",
        )?;
        for plan in store.plans.values() {
            for revision in plan.revisions.values() {
                let program = &revision.program;
                let transforms = program.splits.len().saturating_add(program.tiles.len());
                let fusion_members = program
                    .fusion_groups
                    .iter()
                    .map(|group| group.members.len())
                    .max()
                    .unwrap_or(0);
                let remainders = program
                    .axes
                    .values()
                    .filter(|axis| !matches!(axis.tail, crate::schedule_ir::TailStrategy::Exact))
                    .count();
                let bindings = program
                    .axes
                    .values()
                    .filter_map(|axis| axis.binding.as_ref().map(|binding| binding.level))
                    .collect::<BTreeSet<_>>()
                    .len();
                let checks = [
                    (ResourceKind::ScheduleNodesPerRevision, program.nodes.len()),
                    (ResourceKind::ScheduleAxesPerRevision, program.axes.len()),
                    (ResourceKind::ScheduleTransformsPerRevision, transforms),
                    (
                        ResourceKind::ScheduleFusionGroups,
                        program.fusion_groups.len(),
                    ),
                    (ResourceKind::ScheduleFusionMembers, fusion_members),
                    (
                        ResourceKind::ScheduleDependencyEdges,
                        program.dependencies.len(),
                    ),
                    (
                        ResourceKind::ScheduleLegalityFacts,
                        program.legality_facts.len(),
                    ),
                    (ResourceKind::ScheduleRemainderDomains, remainders),
                    (ResourceKind::ScheduleBindingDepth, bindings),
                    (
                        ResourceKind::ScheduleObligations,
                        revision.obligations.len(),
                    ),
                ];
                for (kind, actual) in checks {
                    BudgetCheck::against(
                        &self.limits,
                        kind,
                        u64::try_from(actual).unwrap_or(u64::MAX),
                        "ScheduleIR revision",
                    )?;
                }
                for tile in &program.tiles {
                    BudgetCheck::against(
                        &self.limits,
                        ResourceKind::ScheduleTileRank,
                        u64::try_from(tile.axes.len()).unwrap_or(u64::MAX),
                        "ScheduleIR tile",
                    )?;
                }
                for vector in &program.vectorizations {
                    BudgetCheck::against(
                        &self.limits,
                        ResourceKind::ScheduleVectorWidth,
                        vector.width,
                        "ScheduleIR vectorization",
                    )?;
                }
                for unroll in &program.unrolls {
                    BudgetCheck::against(
                        &self.limits,
                        ResourceKind::ScheduleUnrollFactor,
                        unroll.factor,
                        "ScheduleIR unroll",
                    )?;
                }
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::ScheduleCanonicalBytes,
                    u64::try_from(canonical_schedule_bytes(plan, revision)?.len())
                        .unwrap_or(u64::MAX),
                    "ScheduleIR canonical form",
                )?;
            }
        }
        Ok(())
    }

    /// Lists compiler-owned target manifests in deterministic order.
    #[must_use]
    pub fn target_list(&self) -> Vec<TargetQuery> {
        self.targets.list()
    }

    /// Reads one immutable target manifest summary.
    pub fn target_query(
        &self,
        manifest: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<TargetQuery> {
        self.targets.query(manifest, revision)
    }

    /// Fully verifies one immutable target manifest.
    pub fn target_check(
        &self,
        manifest: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<TargetCheckReport> {
        self.targets.check(manifest, revision)
    }

    fn schedule_inputs(
        &self,
        plan: &SchedulePlanId,
    ) -> AgentResult<(
        crate::memory::MemoryPlan,
        crate::memory::MemoryRevision,
        crate::impl_ir::ImplProgram,
        crate::target::TargetManifest,
    )> {
        let anchor = self.schedules.plan(plan)?.anchor.clone();
        let memory_plan = self.memory.plan(&anchor.memory_plan)?.clone();
        let memory_revision = self
            .memory
            .revision(&anchor.memory_plan, &anchor.memory_revision)?
            .clone();
        let implementation = self
            .candidates
            .revision(&anchor.candidate, &anchor.candidate_revision)?
            .impl_program
            .clone();
        let target = self
            .targets
            .manifest(&anchor.target_manifest, &anchor.target_revision)?
            .clone();
        Ok((memory_plan, memory_revision, implementation, target))
    }

    /// Creates a conservative exact serial ScheduleIR root.
    pub fn schedule_create(
        &mut self,
        memory_plan: &MemoryPlanId,
        memory_revision: &MemoryRevisionId,
        target_manifest: &TargetManifestId,
        target_revision: &TargetManifestRevisionId,
    ) -> AgentResult<ScheduleCheckReport> {
        self.memory_check(memory_plan, memory_revision)?;
        self.target_check(target_manifest, target_revision)?;
        let memory_plan_data = self.memory.plan(memory_plan)?.clone();
        let memory_revision_data = self.memory.revision(memory_plan, memory_revision)?.clone();
        let implementation = self
            .candidates
            .revision(
                &memory_plan_data.anchor.candidate,
                &memory_plan_data.anchor.candidate_revision,
            )?
            .impl_program
            .clone();
        let target = self
            .targets
            .manifest(target_manifest, target_revision)?
            .clone();
        BudgetCheck::against(
            &self.limits,
            ResourceKind::SchedulePlansPerWorkspace,
            u64::try_from(self.schedules.plans.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            "schedule.create",
        )?;
        let mut staged = self.schedules.clone();
        let report = staged.create(
            &memory_plan_data,
            &memory_revision_data,
            &implementation,
            &target,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.memory.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.targets.events.len()).unwrap_or(u64::MAX),
        )?;
        self.validate_schedule_budgets(&staged)?;
        self.schedules = staged;
        Ok(report)
    }

    /// Reads one immutable ScheduleIR revision summary.
    pub fn schedule_query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<ScheduleQuery> {
        self.schedules.query(plan, revision)
    }

    /// Fully verifies one ScheduleIR revision against its immutable anchors.
    pub fn schedule_check(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<ScheduleCheckReport> {
        let (memory_plan, memory_revision, implementation, target) = self.schedule_inputs(plan)?;
        self.schedules.check(
            plan,
            revision,
            &memory_plan,
            &memory_revision,
            &implementation,
            &target,
        )
    }

    /// Applies one atomic compiler-verified ScheduleIR transaction.
    pub fn schedule_apply(
        &mut self,
        transaction: &ScheduleTransaction,
    ) -> AgentResult<ScheduleCheckReport> {
        BudgetCheck::against(
            &self.limits,
            ResourceKind::ActionsPerTransaction,
            u64::try_from(transaction.actions.len()).unwrap_or(u64::MAX),
            "schedule.apply",
        )?;
        let (memory_plan, memory_revision, implementation, target) =
            self.schedule_inputs(&transaction.schedule_plan)?;
        let mut staged = self.schedules.clone();
        let report = staged.apply(
            transaction,
            &memory_plan,
            &memory_revision,
            &implementation,
            &target,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.memory.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.targets.events.len()).unwrap_or(u64::MAX),
        )?;
        self.validate_schedule_budgets(&staged)?;
        self.schedules = staged;
        Ok(report)
    }

    /// Forks one immutable schedule revision into an independent plan.
    pub fn schedule_fork(
        &mut self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        expected_hash: &ScheduleHash,
    ) -> AgentResult<ScheduleCheckReport> {
        let (memory_plan, memory_revision, implementation, target) = self.schedule_inputs(plan)?;
        let mut staged = self.schedules.clone();
        let report = staged.fork(
            plan,
            revision,
            expected_hash,
            &memory_plan,
            &memory_revision,
            &implementation,
            &target,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.memory.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.targets.events.len()).unwrap_or(u64::MAX),
        )?;
        self.validate_schedule_budgets(&staged)?;
        self.schedules = staged;
        Ok(report)
    }

    /// Seals one structurally proved resource-valid schedule.
    pub fn schedule_seal(
        &mut self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        expected_hash: &ScheduleHash,
    ) -> AgentResult<ScheduleCheckReport> {
        let (memory_plan, memory_revision, implementation, target) = self.schedule_inputs(plan)?;
        let mut staged = self.schedules.clone();
        let report = staged.seal(
            plan,
            revision,
            expected_hash,
            &memory_plan,
            &memory_revision,
            &implementation,
            &target,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.memory.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.targets.events.len()).unwrap_or(u64::MAX),
        )?;
        self.validate_schedule_budgets(&staged)?;
        self.schedules = staged;
        Ok(report)
    }

    /// Returns one compiler-owned schedule axis without mutation.
    pub fn schedule_axis_query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        axis: &ScheduleAxisId,
    ) -> AgentResult<&ScheduleAxis> {
        self.schedules
            .revision(plan, revision)?
            .program
            .axes
            .get(axis)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::InvalidScheduleAxis,
                    format!("schedule axis `{axis}` does not exist"),
                )
            })
    }

    /// Returns the deterministic analytical target resource estimate.
    pub fn schedule_resource_query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<&ScheduleResourceEstimate> {
        Ok(&self
            .schedules
            .revision(plan, revision)?
            .program
            .resource_estimate)
    }

    /// Answers whether one schedule action satisfies all hard conditions.
    pub fn schedule_legality_query(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
        action: &crate::schedule::ScheduleAction,
    ) -> AgentResult<ScheduleLegalityQuery> {
        let (_, memory_revision, implementation, target) = self.schedule_inputs(plan)?;
        self.schedules.legality_query(
            plan,
            revision,
            action,
            &memory_revision,
            &implementation,
            &target,
        )
    }

    /// Returns bounded deterministic parametric schedule choices.
    pub fn schedule_continuation(
        &self,
        plan: &SchedulePlanId,
        revision: &ScheduleRevisionId,
    ) -> AgentResult<ScheduleContinuation> {
        let (_, _, _, target) = self.schedule_inputs(plan)?;
        self.schedules.continuation(plan, revision, &target)
    }

    /// Returns the immutable MemoryIR revision anchored by a schedule plan.
    pub fn scheduled_memory_revision(
        &self,
        plan: &SchedulePlanId,
    ) -> AgentResult<&crate::memory::MemoryRevision> {
        let anchor = &self.schedules.plan(plan)?.anchor;
        self.memory
            .revision(&anchor.memory_plan, &anchor.memory_revision)
    }

    /// Returns the immutable ImplIR program anchored by a schedule plan.
    pub fn scheduled_impl_program(
        &self,
        plan: &SchedulePlanId,
    ) -> AgentResult<&crate::impl_ir::ImplProgram> {
        let anchor = &self.schedules.plan(plan)?.anchor;
        Ok(&self
            .candidates
            .revision(&anchor.candidate, &anchor.candidate_revision)?
            .impl_program)
    }

    /// Atomically lowers one explicit immutable schedule through a trusted backend component.
    fn validate_backend_budgets(&self, store: &BackendStore) -> AgentResult<()> {
        let revisions = store.plans.values().fold(0_u64, |sum, plan| {
            sum.saturating_add(as_u64(plan.revisions.len()))
        });
        for (kind, actual) in [
            (
                ResourceKind::BackendPlansPerWorkspace,
                as_u64(store.plans.len()),
            ),
            (ResourceKind::BackendRevisionsPerWorkspace, revisions),
            (ResourceKind::BackendEvents, as_u64(store.events.len())),
        ] {
            BudgetCheck::against(&self.limits, kind, actual, "BackendStore")?;
        }
        for (plan_id, plan) in &store.plans {
            for revision in plan.revisions.values() {
                let values = revision
                    .program
                    .kernels
                    .values()
                    .map(|kernel| kernel.values.len())
                    .sum::<usize>();
                let statements = revision
                    .program
                    .kernels
                    .values()
                    .map(|kernel| kernel.statements.len())
                    .sum::<usize>();
                for (kind, actual) in [
                    (ResourceKind::BackendKernels, revision.program.kernels.len()),
                    (ResourceKind::BackendValues, values),
                    (ResourceKind::BackendStatements, statements),
                    (
                        ResourceKind::BackendDispatches,
                        revision.program.dispatches.len(),
                    ),
                    (
                        ResourceKind::BackendGuardBranches,
                        usize::from(revision.program.guard.is_some()),
                    ),
                    (
                        ResourceKind::BackendProofRecords,
                        revision
                            .evidence
                            .len()
                            .saturating_add(revision.obligations.len()),
                    ),
                ] {
                    BudgetCheck::against(&self.limits, kind, as_u64(actual), "BackendIR")?;
                }
                for kernel in revision.program.kernels.values() {
                    for (kind, actual) in [
                        (
                            ResourceKind::BackendSourceNodesPerKernel,
                            kernel.source_schedule_nodes.len(),
                        ),
                        (
                            ResourceKind::BackendBindingsPerKernel,
                            kernel.bindings.len(),
                        ),
                        (
                            ResourceKind::BackendParameterEntries,
                            kernel.parameter_block.entries.len(),
                        ),
                    ] {
                        BudgetCheck::against(&self.limits, kind, as_u64(actual), "BackendKernel")?;
                    }
                    BudgetCheck::against(
                        &self.limits,
                        ResourceKind::BackendParameterBytes,
                        kernel.parameter_block.byte_size,
                        "BackendKernel parameters",
                    )?;
                }
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::BackendCanonicalBytes,
                    as_u64(canonical_backend_bytes(plan_id, &plan.anchor, revision)?.len()),
                    "BackendIR canonical form",
                )?;
            }
        }
        Ok(())
    }

    fn validate_artifact_budgets(&self, store: &ArtifactStore) -> AgentResult<()> {
        for (kind, actual) in [
            (ResourceKind::ArtifactPackages, store.packages.len()),
            (ResourceKind::ArtifactEvents, store.events.len()),
        ] {
            BudgetCheck::against(&self.limits, kind, as_u64(actual), "ArtifactStore")?;
        }
        for package in store.packages.values() {
            for (kind, actual) in [
                (ResourceKind::ArtifactModules, package.modules.len()),
                (
                    ResourceKind::ArtifactEntryPoints,
                    package.manifest.entry_points.len(),
                ),
            ] {
                BudgetCheck::against(&self.limits, kind, as_u64(actual), "ArtifactPackage")?;
            }
            for module in &package.modules {
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::WgslBytesPerModule,
                    as_u64(module.wgsl.len()),
                    "WGSL module",
                )?;
            }
            BudgetCheck::against(
                &self.limits,
                ResourceKind::ArtifactManifestBytes,
                as_u64(
                    serde_json::to_vec(&package.manifest)
                        .map_err(|error| {
                            AgentError::new(ErrorCode::CanonicalizationFailed, error.to_string())
                        })?
                        .len(),
                ),
                "artifact manifest",
            )?;
            BudgetCheck::against(
                &self.limits,
                ResourceKind::ArtifactTotalBytes,
                as_u64(
                    serde_json::to_vec(package)
                        .map_err(|error| {
                            AgentError::new(ErrorCode::CanonicalizationFailed, error.to_string())
                        })?
                        .len(),
                ),
                "artifact package",
            )?;
        }
        Ok(())
    }

    /// Atomically lowers one explicit immutable schedule through a trusted backend component.
    pub fn backend_lower_with<F>(
        &mut self,
        schedule_plan: &SchedulePlanId,
        schedule_revision: &ScheduleRevisionId,
        expected_schedule_hash: &ScheduleHash,
        lower: F,
    ) -> AgentResult<BackendCheckReport>
    where
        F: FnOnce(
            &mut BackendAllocator,
            &crate::schedule::SchedulePlan,
            &crate::schedule::ScheduleRevision,
            &crate::memory::MemoryRevision,
            &crate::impl_ir::ImplProgram,
            &crate::target::TargetManifest,
        ) -> AgentResult<BackendProgram>,
    {
        self.schedule_check(schedule_plan, schedule_revision)?;
        let schedule_plan_data = self.schedules.plan(schedule_plan)?.clone();
        let schedule_revision_data = self
            .schedules
            .revision(schedule_plan, schedule_revision)?
            .clone();
        if &schedule_revision_data.schedule_hash != expected_schedule_hash {
            return Err(AgentError::new(
                ErrorCode::BackendScheduleMismatch,
                "backend.lower expected schedule_hash differs from the selected revision",
            )
            .with_types(
                expected_schedule_hash.to_string(),
                schedule_revision_data.schedule_hash.to_string(),
            ));
        }
        let (_, memory_revision, implementation, target) = self.schedule_inputs(schedule_plan)?;
        let anchor = BackendAnchor {
            spec_revision: schedule_plan_data.anchor.spec_revision.clone(),
            spec_hash: schedule_plan_data.anchor.spec_hash.clone(),
            impl_hash: schedule_plan_data.anchor.impl_hash.clone(),
            memory_hash: schedule_plan_data.anchor.memory_hash.clone(),
            memory_plan: schedule_plan_data.anchor.memory_plan.clone(),
            memory_revision: schedule_plan_data.anchor.memory_revision.clone(),
            target_hash: schedule_plan_data.anchor.target_hash.clone(),
            target_manifest: schedule_plan_data.anchor.target_manifest.clone(),
            target_revision: schedule_plan_data.anchor.target_revision.clone(),
            schedule_hash: schedule_revision_data.schedule_hash.clone(),
            schedule_plan: schedule_plan.clone(),
            schedule_revision: schedule_revision.clone(),
            numeric_contract: schedule_plan_data.anchor.numeric_contract.clone(),
            backend_kind: BackendKind::WebGpuWgslV1,
        };
        let expected_nodes = schedule_revision_data.program.node_order.clone();
        let mut staged = self.backends.clone();
        let report = staged.lower_with(
            anchor,
            &expected_nodes,
            u64::try_from(self.candidates.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.equality.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.memory.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.targets.events.len()).unwrap_or(u64::MAX),
            u64::try_from(self.schedules.events.len()).unwrap_or(u64::MAX),
            |allocator| {
                lower(
                    allocator,
                    &schedule_plan_data,
                    &schedule_revision_data,
                    &memory_revision,
                    &implementation,
                    &target,
                )
            },
        )?;
        self.validate_backend_budgets(&staged)?;
        self.backends = staged;
        Ok(report)
    }

    /// Reads one immutable BackendIR summary.
    pub fn backend_query(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<BackendQuery> {
        self.backends.query(plan, revision)
    }

    /// Fully verifies one BackendIR revision.
    pub fn backend_check(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<BackendCheckReport> {
        let plan_data = self.backends.plan(plan)?;
        let schedule = self.schedules.revision(
            &plan_data.anchor.schedule_plan,
            &plan_data.anchor.schedule_revision,
        )?;
        if schedule.schedule_hash != plan_data.anchor.schedule_hash
            || self
                .memory
                .revision(
                    &plan_data.anchor.memory_plan,
                    &plan_data.anchor.memory_revision,
                )?
                .memory_hash
                != plan_data.anchor.memory_hash
            || self
                .targets
                .manifest(
                    &plan_data.anchor.target_manifest,
                    &plan_data.anchor.target_revision,
                )?
                .target_hash
                != plan_data.anchor.target_hash
        {
            return Err(AgentError::new(
                ErrorCode::BackendScheduleMismatch,
                "backend immutable anchor chain no longer matches Stage 3-4 state",
            ));
        }
        self.backends.check(plan, revision)
    }

    /// Returns bounded deterministic BackendIR capabilities without mutation.
    pub fn backend_continuation(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<crate::backend::BackendContinuation> {
        self.backend_check(plan, revision)?;
        let anchor = &self.backends.plan(plan)?.anchor;
        Ok(crate::backend::BackendContinuation {
            schedule_hash: anchor.schedule_hash.clone(),
            backend_kind: anchor.backend_kind,
            serial_available: true,
            vector_widths: vec![1, 2, 4],
            unsupported: vec![
                "reduce".to_owned(),
                "non_contiguous_or_non_global_storage".to_owned(),
                "subgroup_or_shared_memory".to_owned(),
            ],
        })
    }

    /// Forks one immutable BackendIR revision into an independent plan.
    pub fn backend_fork(
        &mut self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
        expected_hash: &BackendHash,
    ) -> AgentResult<BackendCheckReport> {
        self.backend_check(plan, revision)?;
        let mut staged = self.backends.clone();
        let report = staged.fork(plan, revision, expected_hash)?;
        self.validate_backend_budgets(&staged)?;
        self.backends = staged;
        Ok(report)
    }

    /// Seals one proved BackendIR revision.
    pub fn backend_seal(
        &mut self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
        expected_hash: &BackendHash,
    ) -> AgentResult<BackendCheckReport> {
        self.backend_check(plan, revision)?;
        let mut staged = self.backends.clone();
        let report = staged.seal(plan, revision, expected_hash)?;
        self.validate_backend_budgets(&staged)?;
        self.backends = staged;
        Ok(report)
    }

    /// Atomically emits one deterministic artifact through a trusted compiler component.
    pub fn artifact_emit_with<F>(
        &mut self,
        backend_plan: &BackendPlanId,
        backend_revision: &BackendRevisionId,
        expected_backend_hash: &BackendHash,
        emit: F,
    ) -> AgentResult<ArtifactCheckReport>
    where
        F: FnOnce(
            &mut BackendAllocator,
            ArtifactId,
            BackendAnchor,
            BackendHash,
            &BackendProgram,
        ) -> AgentResult<ArtifactPackage>,
    {
        self.backend_check(backend_plan, backend_revision)?;
        let backend_plan_data = self.backends.plan(backend_plan)?.clone();
        let backend_revision_data = self
            .backends
            .revision(backend_plan, backend_revision)?
            .clone();
        if &backend_revision_data.backend_hash != expected_backend_hash {
            return Err(AgentError::new(
                ErrorCode::BackendHashMismatch,
                "artifact.emit expected backend_hash differs from the selected revision",
            )
            .with_types(
                expected_backend_hash.to_string(),
                backend_revision_data.backend_hash.to_string(),
            ));
        }
        let mut backends = self.backends.clone();
        let mut artifacts = self.artifacts.clone();
        let report = artifacts.emit_with(
            &mut backends,
            backend_plan,
            backend_revision,
            |allocator, artifact| {
                emit(
                    allocator,
                    artifact,
                    backend_plan_data.anchor,
                    backend_revision_data.backend_hash,
                    &backend_revision_data.program,
                )
            },
        )?;
        self.validate_backend_budgets(&backends)?;
        self.validate_artifact_budgets(&artifacts)?;
        self.backends = backends;
        self.artifacts = artifacts;
        Ok(report)
    }

    /// Lists deterministic artifact summaries.
    #[must_use]
    pub fn artifact_list(&self) -> Vec<ArtifactQuery> {
        self.artifacts.list()
    }

    /// Reads one deterministic artifact summary.
    pub fn artifact_query(&self, artifact: &ArtifactId) -> AgentResult<ArtifactQuery> {
        self.artifacts.query(artifact)
    }

    /// Returns one immutable artifact package for runtime or reference evaluation.
    pub fn artifact_package(&self, artifact: &ArtifactId) -> AgentResult<&ArtifactPackage> {
        self.artifacts.package(artifact)
    }

    /// Fully verifies one artifact package against its source BackendIR revision.
    pub fn artifact_check(&self, artifact: &ArtifactId) -> AgentResult<ArtifactCheckReport> {
        let event = self
            .artifacts
            .events
            .iter()
            .find(|event| event.event.package.id == *artifact)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::ArtifactNotFound,
                    format!("artifact `{artifact}` does not exist"),
                )
            })?;
        let backend = self
            .backends
            .revision(&event.event.backend_plan, &event.event.backend_revision)?;
        self.artifacts.check(artifact, backend)
    }

    /// Returns the ScheduleIR source selected by one backend plan.
    pub fn backend_source_schedule(
        &self,
        backend_plan: &BackendPlanId,
    ) -> AgentResult<(&SchedulePlanId, &ScheduleRevisionId)> {
        let anchor = &self.backends.plan(backend_plan)?.anchor;
        Ok((&anchor.schedule_plan, &anchor.schedule_revision))
    }

    /// Returns the backend plan and revision that emitted one artifact.
    pub fn artifact_source_backend(
        &self,
        artifact: &ArtifactId,
    ) -> AgentResult<(&BackendPlanId, &BackendRevisionId)> {
        let event = self
            .artifacts
            .events
            .iter()
            .find(|event| event.event.package.id == *artifact)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::ArtifactNotFound,
                    format!("artifact `{artifact}` does not exist"),
                )
            })?;
        Ok((&event.event.backend_plan, &event.event.backend_revision))
    }

    /// Returns one immutable compiler-owned target manifest for runtime checks.
    pub fn target_manifest(
        &self,
        manifest: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<&crate::target::TargetManifest> {
        self.targets.manifest(manifest, revision)
    }

    /// Reads one completed confidence-only hardware measurement.
    pub fn measurement_query(
        &self,
        measurement: &crate::ids::MeasurementId,
    ) -> AgentResult<&crate::backend_ir::HardwareMeasurementRecord> {
        self.measurements.records.get(measurement).ok_or_else(|| {
            AgentError::new(
                ErrorCode::BenchmarkTaskNotFound,
                format!("measurement `{measurement}` does not exist"),
            )
        })
    }

    /// Publishes one runtime-created confidence-only measurement atomically.
    pub fn measurement_publish(
        &mut self,
        record: crate::backend_ir::HardwareMeasurementRecord,
    ) -> AgentResult<crate::ids::MeasurementId> {
        BudgetCheck::against(
            &self.limits,
            ResourceKind::BenchmarkRecords,
            as_u64(self.measurements.records.len()).saturating_add(1),
            "measurement publication",
        )?;
        let mut allocator = self.backends.allocator.clone();
        let mut measurements = self.measurements.clone();
        let id = measurements.publish(&mut allocator, &self.artifacts, record)?;
        self.backends.allocator = allocator;
        self.measurements = measurements;
        Ok(id)
    }

    fn replay_target_event(&mut self, versioned: &VersionedTargetEvent) -> AgentResult<()> {
        if versioned.semantics_version != TARGET_EVENT_SEMANTICS_VERSION {
            return Err(AgentError::new(
                ErrorCode::ScheduleEventOrderInvalid,
                "unsupported target event semantics version",
            ));
        }
        let TargetEvent::Created {
            profile,
            target_manifest,
            target_revision,
            target_hash,
        } = &versioned.event;
        let result = self.target_create(*profile)?;
        if result.query.target_manifest != *target_manifest
            || result.query.target_revision != *target_revision
            || result.query.target_hash != *target_hash
            || self.targets.events.last() != Some(versioned)
        {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "target event replay diverged",
            ));
        }
        Ok(())
    }

    fn replay_schedule_event(&mut self, versioned: &VersionedScheduleEvent) -> AgentResult<()> {
        if versioned.semantics_version != SCHEDULE_EVENT_SEMANTICS_VERSION {
            return Err(AgentError::new(
                ErrorCode::ScheduleEventOrderInvalid,
                "unsupported schedule event semantics version",
            ));
        }
        let result = match &versioned.event {
            ScheduleEvent::Created {
                memory_plan,
                memory_revision,
                target_manifest,
                target_revision,
                schedule_plan,
                schedule_revision,
                schedule_hash,
            } => {
                self.memory_check(memory_plan, memory_revision)?;
                self.target_check(target_manifest, target_revision)?;
                let memory_plan_data = self.memory.plan(memory_plan)?.clone();
                let memory_revision_data =
                    self.memory.revision(memory_plan, memory_revision)?.clone();
                let implementation = self
                    .candidates
                    .revision(
                        &memory_plan_data.anchor.candidate,
                        &memory_plan_data.anchor.candidate_revision,
                    )?
                    .impl_program
                    .clone();
                let target = self
                    .targets
                    .manifest(target_manifest, target_revision)?
                    .clone();
                let result = self.schedules.create(
                    &memory_plan_data,
                    &memory_revision_data,
                    &implementation,
                    &target,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                    versioned.memory_event_cursor,
                    versioned.target_event_cursor,
                )?;
                if result.query.schedule_plan != *schedule_plan
                    || result.query.schedule_revision != *schedule_revision
                    || result.query.schedule_hash != *schedule_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "schedule creation replay diverged",
                    ));
                }
                result
            }
            ScheduleEvent::Applied {
                transaction,
                schedule_revision,
                schedule_hash,
            } => {
                let (memory_plan, memory_revision, implementation, target) =
                    self.schedule_inputs(&transaction.schedule_plan)?;
                let result = self.schedules.apply(
                    transaction,
                    &memory_plan,
                    &memory_revision,
                    &implementation,
                    &target,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                    versioned.memory_event_cursor,
                    versioned.target_event_cursor,
                )?;
                if result.query.schedule_revision != *schedule_revision
                    || result.query.schedule_hash != *schedule_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "schedule transaction replay diverged",
                    ));
                }
                result
            }
            ScheduleEvent::Forked {
                parent_plan,
                parent_revision,
                schedule_plan,
                schedule_revision,
                schedule_hash,
            } => {
                let expected = self
                    .schedules
                    .revision(parent_plan, parent_revision)?
                    .schedule_hash
                    .clone();
                let (memory_plan, memory_revision, implementation, target) =
                    self.schedule_inputs(parent_plan)?;
                let result = self.schedules.fork(
                    parent_plan,
                    parent_revision,
                    &expected,
                    &memory_plan,
                    &memory_revision,
                    &implementation,
                    &target,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                    versioned.memory_event_cursor,
                    versioned.target_event_cursor,
                )?;
                if result.query.schedule_plan != *schedule_plan
                    || result.query.schedule_revision != *schedule_revision
                    || result.query.schedule_hash != *schedule_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "schedule fork replay diverged",
                    ));
                }
                result
            }
            ScheduleEvent::Sealed {
                schedule_plan,
                base_revision,
                expected_schedule_hash,
                schedule_revision,
                schedule_hash,
            } => {
                let (memory_plan, memory_revision, implementation, target) =
                    self.schedule_inputs(schedule_plan)?;
                let result = self.schedules.seal(
                    schedule_plan,
                    base_revision,
                    expected_schedule_hash,
                    &memory_plan,
                    &memory_revision,
                    &implementation,
                    &target,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                    versioned.memory_event_cursor,
                    versioned.target_event_cursor,
                )?;
                if result.query.schedule_revision != *schedule_revision
                    || result.query.schedule_hash != *schedule_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "schedule sealing replay diverged",
                    ));
                }
                result
            }
        };
        let _ = result;
        if self.schedules.events.last() != Some(versioned) {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "schedule event replay diverged",
            ));
        }
        Ok(())
    }

    fn replay_memory_event(&mut self, versioned: &VersionedMemoryEvent) -> AgentResult<()> {
        if versioned.semantics_version != MEMORY_EVENT_SEMANTICS_VERSION {
            return Err(AgentError::new(
                ErrorCode::MemoryEventOrderInvalid,
                "unsupported memory event semantics version",
            ));
        }
        match &versioned.event {
            MemoryEvent::Created {
                candidate,
                candidate_revision,
                memory_plan,
                memory_revision,
                memory_hash,
            } => {
                let candidate_data = self.candidates.candidate(candidate)?;
                let spec_revision = candidate_data.spec_revision.clone();
                let (source, spec_hash) = self.frozen_candidate_source(&spec_revision)?;
                let result = self.memory.create(
                    &self.candidates,
                    candidate,
                    candidate_revision,
                    &source,
                    &spec_revision,
                    &spec_hash,
                    &self.limits,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                )?;
                if result.query.memory_plan != *memory_plan
                    || result.query.memory_revision != *memory_revision
                    || result.query.memory_hash != *memory_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "memory creation replay diverged",
                    ));
                }
            }
            MemoryEvent::Applied {
                transaction,
                memory_revision,
                memory_hash,
            } => {
                let (source, _) = self.memory_source(&transaction.memory_plan)?;
                let result = self.memory.apply(
                    transaction,
                    &self.candidates,
                    &source,
                    &self.limits,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                )?;
                if result.query.memory_revision != *memory_revision
                    || result.query.memory_hash != *memory_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "memory transaction replay diverged",
                    ));
                }
            }
            MemoryEvent::Forked {
                parent_plan,
                parent_revision,
                memory_plan,
                memory_revision,
                memory_hash,
            } => {
                let expected = self
                    .memory
                    .revision(parent_plan, parent_revision)?
                    .memory_hash
                    .clone();
                let (source, _) = self.memory_source(parent_plan)?;
                let result = self.memory.fork(
                    parent_plan,
                    parent_revision,
                    &expected,
                    &self.candidates,
                    &source,
                    &self.limits,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                )?;
                if result.query.memory_plan != *memory_plan
                    || result.query.memory_revision != *memory_revision
                    || result.query.memory_hash != *memory_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "memory fork replay diverged",
                    ));
                }
            }
            MemoryEvent::Sealed {
                memory_plan,
                base_revision,
                expected_memory_hash,
                memory_revision,
                memory_hash,
            } => {
                let (source, _) = self.memory_source(memory_plan)?;
                let result = self.memory.seal(
                    memory_plan,
                    base_revision,
                    expected_memory_hash,
                    &self.candidates,
                    &source,
                    &self.limits,
                    versioned.candidate_event_cursor,
                    versioned.equality_event_cursor,
                )?;
                if result.query.memory_revision != *memory_revision
                    || result.query.memory_hash != *memory_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "memory sealing replay diverged",
                    ));
                }
            }
        }
        if self.memory.events.last() != Some(versioned) {
            return Err(
                AgentError::new(ErrorCode::ReplayMismatch, "memory event replay diverged")
                    .with_detail("expected_event", json!(versioned))
                    .with_detail("actual_event", json!(self.memory.events.last())),
            );
        }
        Ok(())
    }

    fn replay_candidate_event(&mut self, versioned: &VersionedCandidateEvent) -> AgentResult<()> {
        if !matches!(
            versioned.semantics_version,
            LEGACY_CANDIDATE_SEMANTICS_VERSION
                | CANDIDATE_SEMANTICS_VERSION
                | EQUALITY_CANDIDATE_SEMANTICS_VERSION
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
        Ok(())
    }

    fn replay_equality_event(&mut self, versioned: &VersionedEqualityEvent) -> AgentResult<()> {
        if versioned.semantics_version != EQUALITY_SEMANTICS_VERSION {
            return Err(AgentError::new(
                ErrorCode::EqualityEventOrderInvalid,
                format!(
                    "unsupported equality semantics version {}",
                    versioned.semantics_version
                ),
            ));
        }
        match &versioned.event {
            EqualityEvent::Created {
                candidate,
                candidate_revision,
                equality_space,
                equality_revision,
                equality_hash,
            } => {
                let result = self.equality_create(candidate, candidate_revision)?;
                if result.equality_space != *equality_space
                    || result.equality_revision != *equality_revision
                    || result.equality_hash != *equality_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "equality creation replay diverged",
                    ));
                }
            }
            EqualityEvent::Expanded {
                equality_space,
                base_revision,
                expected_equality_hash,
                fuel,
                equality_revision,
                equality_hash,
            } => {
                let result = self.equality_expand(
                    equality_space,
                    base_revision,
                    expected_equality_hash,
                    *fuel,
                )?;
                if result.equality_revision != *equality_revision
                    || result.equality_hash != *equality_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "equality expansion replay diverged",
                    ));
                }
            }
            EqualityEvent::Saturated {
                equality_space,
                base_revision,
                expected_equality_hash,
                fuel,
                equality_revision,
                equality_hash,
            } => {
                let result = self.equality_saturate(
                    equality_space,
                    base_revision,
                    expected_equality_hash,
                    *fuel,
                )?;
                if result.equality_revision != *equality_revision
                    || result.equality_hash != *equality_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "equality saturation replay diverged",
                    ));
                }
            }
            EqualityEvent::CandidateDischarged {
                candidate,
                base_candidate_revision,
                proposal,
                equality_space,
                equality_revision,
                equality_hash,
                target_node,
                candidate_revision,
                candidate_hash,
            } => {
                let result = self.candidate_equality_check(
                    candidate,
                    base_candidate_revision,
                    proposal,
                    equality_space,
                    equality_revision,
                    equality_hash,
                    target_node,
                )?;
                if result.candidate_revision != *candidate_revision
                    || result.candidate_hash != *candidate_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "equality-backed candidate replay diverged",
                    ));
                }
            }
            EqualityEvent::Materialized {
                equality_space,
                equality_revision,
                equality_hash,
                target_node,
                candidate,
                candidate_revision,
                candidate_hash,
            } => {
                let result = self.equality_materialize(
                    equality_space,
                    equality_revision,
                    equality_hash,
                    target_node,
                )?;
                if result.candidate != *candidate
                    || result.candidate_revision != *candidate_revision
                    || result.candidate_hash != *candidate_hash
                {
                    return Err(AgentError::new(
                        ErrorCode::ReplayMismatch,
                        "equality materialization replay diverged",
                    ));
                }
            }
        }
        if self.equality.events.last() != Some(versioned) {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "equality event replay diverged",
            )
            .with_detail("expected_event", json!(versioned))
            .with_detail("actual_event", json!(self.equality.events.last())));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_optimization_state(
        &mut self,
        expected_candidates: &CandidateForest,
        expected_equality: &EqualityStore,
        expected_memory: &MemoryPlanStore,
        expected_targets: &TargetManifestStore,
        expected_schedules: &SchedulePlanStore,
        expected_backends: &BackendStore,
        expected_artifacts: &ArtifactStore,
        expected_measurements: &MeasurementStore,
    ) -> AgentResult<()> {
        let mut candidate_cursor = 0_usize;
        for equality_event in &expected_equality.events {
            let required_cursor =
                usize::try_from(equality_event.candidate_event_cursor).map_err(|_| {
                    AgentError::new(
                        ErrorCode::EqualityEventOrderInvalid,
                        "equality candidate-event cursor does not fit this platform",
                    )
                })?;
            if required_cursor < candidate_cursor
                || required_cursor > expected_candidates.events.len()
            {
                return Err(AgentError::new(
                    ErrorCode::EqualityEventOrderInvalid,
                    "equality event candidate dependency cursor is invalid",
                )
                .with_detail("current_cursor", candidate_cursor as u64)
                .with_detail("required_cursor", equality_event.candidate_event_cursor)
                .with_detail(
                    "candidate_event_count",
                    expected_candidates.events.len() as u64,
                ));
            }
            for event in &expected_candidates.events[candidate_cursor..required_cursor] {
                self.replay_candidate_event(event)?;
            }
            candidate_cursor = required_cursor;
            self.replay_equality_event(equality_event)?;
        }
        for event in &expected_candidates.events[candidate_cursor..] {
            self.replay_candidate_event(event)?;
        }
        if &self.candidates != expected_candidates {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed CandidateForest differs from snapshot",
            ));
        }
        if &self.equality != expected_equality {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed EqualityStore differs from snapshot",
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
        )?;
        self.equality.verify_all(
            |revision| {
                let source = revisions.get(revision).ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::RevisionNotFound,
                        format!("equality anchor revision `{revision}` does not exist"),
                    )
                })?;
                let spec_hash = source.spec_hash.clone().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::SpecHashMismatch,
                        format!("equality anchor revision `{revision}` has no spec_hash"),
                    )
                })?;
                Ok((source.program.clone(), spec_hash))
            },
            &self.limits,
        )?;
        let mut candidate_cursor = 0_u64;
        let mut equality_cursor = 0_u64;
        let candidate_count = u64::try_from(expected_candidates.events.len()).unwrap_or(u64::MAX);
        let equality_count = u64::try_from(expected_equality.events.len()).unwrap_or(u64::MAX);
        for event in &expected_memory.events {
            if event.candidate_event_cursor < candidate_cursor
                || event.candidate_event_cursor > candidate_count
                || event.equality_event_cursor < equality_cursor
                || event.equality_event_cursor > equality_count
            {
                return Err(AgentError::new(
                    ErrorCode::MemoryEventOrderInvalid,
                    "memory event dependency cursor is invalid",
                )
                .with_detail("candidate_cursor", event.candidate_event_cursor)
                .with_detail("equality_cursor", event.equality_event_cursor));
            }
            if let MemoryEvent::Created {
                candidate,
                candidate_revision,
                ..
            } = &event.event
            {
                let candidate_prefix =
                    usize::try_from(event.candidate_event_cursor).map_err(|_| {
                        AgentError::new(
                            ErrorCode::MemoryEventOrderInvalid,
                            "memory candidate-event cursor does not fit this platform",
                        )
                    })?;
                let equality_prefix =
                    usize::try_from(event.equality_event_cursor).map_err(|_| {
                        AgentError::new(
                            ErrorCode::MemoryEventOrderInvalid,
                            "memory equality-event cursor does not fit this platform",
                        )
                    })?;
                let available_from_candidates = expected_candidates.events[..candidate_prefix]
                    .iter()
                    .any(|versioned| match &versioned.event {
                        CandidateEvent::Created {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | CandidateEvent::Forked {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | CandidateEvent::Validated {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | CandidateEvent::Sealed {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | CandidateEvent::ProposalAccepted {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | CandidateEvent::TranslationChecked {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        } => created == candidate && revision == candidate_revision,
                        CandidateEvent::TransactionApplied {
                            transaction,
                            candidate_revision: revision,
                            ..
                        } => &transaction.candidate == candidate && revision == candidate_revision,
                    });
                let available_from_equality = expected_equality.events[..equality_prefix]
                    .iter()
                    .any(|versioned| match &versioned.event {
                        EqualityEvent::CandidateDischarged {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        }
                        | EqualityEvent::Materialized {
                            candidate: created,
                            candidate_revision: revision,
                            ..
                        } => created == candidate && revision == candidate_revision,
                        EqualityEvent::Created { .. }
                        | EqualityEvent::Expanded { .. }
                        | EqualityEvent::Saturated { .. } => false,
                    });
                if !available_from_candidates && !available_from_equality {
                    return Err(AgentError::new(
                        ErrorCode::MemoryEventOrderInvalid,
                        "memory creation anchor was not available at its dependency cursors",
                    )
                    .with_detail("candidate", candidate.to_string())
                    .with_detail("candidate_revision", candidate_revision.to_string())
                    .with_detail("candidate_cursor", event.candidate_event_cursor)
                    .with_detail("equality_cursor", event.equality_event_cursor));
                }
            }
            candidate_cursor = event.candidate_event_cursor;
            equality_cursor = event.equality_event_cursor;
            self.replay_memory_event(event)?;
        }
        if &self.memory != expected_memory {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed MemoryPlanStore differs from snapshot",
            ));
        }
        let revisions = &self.revisions;
        self.memory.verify_all(
            &self.candidates,
            |revision| {
                let source = revisions.get(revision).ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::RevisionNotFound,
                        format!("memory anchor revision `{revision}` does not exist"),
                    )
                })?;
                let spec_hash = source.spec_hash.clone().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::SpecHashMismatch,
                        format!("memory anchor revision `{revision}` has no spec_hash"),
                    )
                })?;
                Ok((source.program.clone(), spec_hash))
            },
            &self.limits,
        )?;

        let mut target_cursor = 0_usize;
        for event in &expected_schedules.events {
            let required = usize::try_from(event.target_event_cursor).map_err(|_| {
                AgentError::new(
                    ErrorCode::ScheduleEventOrderInvalid,
                    "schedule target-event cursor does not fit this platform",
                )
            })?;
            if required < target_cursor || required > expected_targets.events.len() {
                return Err(AgentError::new(
                    ErrorCode::ScheduleEventOrderInvalid,
                    "schedule target-event dependency cursor is invalid",
                ));
            }
            for target_event in &expected_targets.events[target_cursor..required] {
                self.replay_target_event(target_event)?;
            }
            target_cursor = required;
            if event.candidate_event_cursor > expected_candidates.events.len() as u64
                || event.equality_event_cursor > expected_equality.events.len() as u64
                || event.memory_event_cursor > expected_memory.events.len() as u64
            {
                return Err(AgentError::new(
                    ErrorCode::ScheduleEventOrderInvalid,
                    "schedule dependency cursor exceeds the available event history",
                ));
            }
            self.replay_schedule_event(event)?;
        }
        for target_event in &expected_targets.events[target_cursor..] {
            self.replay_target_event(target_event)?;
        }
        if &self.targets != expected_targets || &self.schedules != expected_schedules {
            return Err(AgentError::new(
                ErrorCode::ReplayMismatch,
                "replayed target or schedule store differs from snapshot",
            ));
        }
        self.targets.verify_all()?;
        self.schedules.verify_all(|anchor| {
            let memory_plan = self.memory.plan(&anchor.memory_plan)?.clone();
            let memory_revision = self
                .memory
                .revision(&anchor.memory_plan, &anchor.memory_revision)?
                .clone();
            let implementation = self
                .candidates
                .revision(&anchor.candidate, &anchor.candidate_revision)?
                .impl_program
                .clone();
            let target = self
                .targets
                .manifest(&anchor.target_manifest, &anchor.target_revision)?
                .clone();
            Ok((memory_plan, memory_revision, implementation, target))
        })?;
        expected_backends.verify_all()?;
        let candidate_count = u64::try_from(expected_candidates.events.len()).unwrap_or(u64::MAX);
        let equality_count = u64::try_from(expected_equality.events.len()).unwrap_or(u64::MAX);
        let memory_count = u64::try_from(expected_memory.events.len()).unwrap_or(u64::MAX);
        let target_count = u64::try_from(expected_targets.events.len()).unwrap_or(u64::MAX);
        let schedule_count = u64::try_from(expected_schedules.events.len()).unwrap_or(u64::MAX);
        let mut dependency_cursors = [0_u64; 5];
        let mut replayed_backend_plans = BTreeMap::new();
        for event in &expected_backends.events {
            if event.semantics_version != crate::backend::BACKEND_EVENT_SEMANTICS_VERSION {
                return Err(AgentError::new(
                    ErrorCode::BackendEventOrderInvalid,
                    "backend event semantics version is invalid",
                ));
            }
            match &event.event {
                BackendEvent::Lowered {
                    plan,
                    candidate_event_cursor,
                    equality_event_cursor,
                    memory_event_cursor,
                    target_event_cursor,
                    schedule_event_cursor,
                } => {
                    let next = [
                        *candidate_event_cursor,
                        *equality_event_cursor,
                        *memory_event_cursor,
                        *target_event_cursor,
                        *schedule_event_cursor,
                    ];
                    let available = [
                        candidate_count,
                        equality_count,
                        memory_count,
                        target_count,
                        schedule_count,
                    ];
                    if next
                        .iter()
                        .zip(dependency_cursors)
                        .any(|(next, previous)| *next < previous)
                        || next
                            .iter()
                            .zip(available)
                            .any(|(required, count)| *required > count)
                        || replayed_backend_plans
                            .insert(plan.id.clone(), plan.clone())
                            .is_some()
                    {
                        return Err(AgentError::new(
                            ErrorCode::BackendEventOrderInvalid,
                            "backend dependency cursors regress/exceed history or duplicate a plan",
                        ));
                    }
                    dependency_cursors = next;
                }
                BackendEvent::Forked {
                    source_plan,
                    source_revision,
                    expected_backend_hash,
                    plan,
                } => {
                    let source = replayed_backend_plans
                        .get(source_plan)
                        .and_then(|source| source.revisions.get(source_revision));
                    if source.is_none_or(|source| &source.backend_hash != expected_backend_hash)
                        || replayed_backend_plans
                            .insert(plan.id.clone(), plan.clone())
                            .is_some()
                    {
                        return Err(AgentError::new(
                            ErrorCode::BackendEventOrderInvalid,
                            "backend fork does not follow an available exact source revision",
                        ));
                    }
                }
                BackendEvent::Sealed {
                    backend_plan,
                    base_revision,
                    expected_backend_hash,
                    revision,
                } => {
                    let Some(plan) = replayed_backend_plans.get_mut(backend_plan) else {
                        return Err(AgentError::new(
                            ErrorCode::BackendEventOrderInvalid,
                            "backend seal references an unavailable plan",
                        ));
                    };
                    if plan.head != *base_revision
                        || plan
                            .revisions
                            .get(base_revision)
                            .is_none_or(|base| &base.backend_hash != expected_backend_hash)
                        || revision.parents != [base_revision.clone()]
                        || plan
                            .revisions
                            .insert(revision.id.clone(), revision.clone())
                            .is_some()
                    {
                        return Err(AgentError::new(
                            ErrorCode::BackendEventOrderInvalid,
                            "backend seal event does not extend the exact current head",
                        ));
                    }
                    plan.head = revision.id.clone();
                }
            }
        }
        if replayed_backend_plans != expected_backends.plans {
            return Err(AgentError::new(
                ErrorCode::BackendEventOrderInvalid,
                "backend event log does not reproduce BackendStore plans",
            ));
        }
        let mut backend_cursor = 0_u64;
        for event in &expected_artifacts.events {
            if event.semantics_version != crate::backend::ARTIFACT_EVENT_SEMANTICS_VERSION
                || event.event.backend_event_cursor < backend_cursor
                || event.event.backend_event_cursor > expected_backends.events.len() as u64
            {
                return Err(AgentError::new(
                    ErrorCode::ArtifactEventOrderInvalid,
                    "artifact event dependency cursor or semantics version is invalid",
                ));
            }
            let backend = expected_backends
                .revision(&event.event.backend_plan, &event.event.backend_revision)?;
            crate::backend::verify_artifact(&event.event.package, backend)?;
            backend_cursor = event.event.backend_event_cursor;
        }
        if expected_artifacts.events.len() != expected_artifacts.packages.len()
            || expected_artifacts.events.iter().any(|event| {
                expected_artifacts.packages.get(&event.event.package.id)
                    != Some(&event.event.package)
            })
        {
            return Err(AgentError::new(
                ErrorCode::ArtifactEventOrderInvalid,
                "artifact event log does not reproduce ArtifactStore",
            ));
        }
        let mut artifact_cursor = 0_u64;
        for event in &expected_measurements.events {
            if event.semantics_version != crate::backend::MEASUREMENT_EVENT_SEMANTICS_VERSION
                || event.event.artifact_event_cursor < artifact_cursor
                || event.event.artifact_event_cursor > expected_artifacts.events.len() as u64
                || expected_measurements.records.get(&event.event.measurement)
                    != Some(&event.event.record)
                || crate::backend::measurement_hash(&event.event.record)?
                    != event.event.record.measurement_hash
            {
                return Err(AgentError::new(
                    ErrorCode::MeasurementEventOrderInvalid,
                    "measurement event provenance, hash, or ordering is invalid",
                ));
            }
            artifact_cursor = event.event.artifact_event_cursor;
        }
        expected_backends.verify_allocator_state(expected_artifacts, expected_measurements)?;
        self.backends = expected_backends.clone();
        self.artifacts = expected_artifacts.clone();
        self.measurements = expected_measurements.clone();
        Ok(())
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
            equality_store: self.equality.clone(),
            memory_store: self.memory.clone(),
            target_store: self.targets.clone(),
            schedule_store: self.schedules.clone(),
            backend_store: self.backends.clone(),
            artifact_store: self.artifacts.clone(),
            measurement_store: self.measurements.clone(),
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
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::EqualityEvents,
            as_u64(snapshot.equality_store.events.len()),
            "equality snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::MemoryEvents,
            as_u64(snapshot.memory_store.events.len()),
            "memory snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::TargetEvents,
            as_u64(snapshot.target_store.events.len()),
            "target snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::ScheduleEvents,
            as_u64(snapshot.schedule_store.events.len()),
            "schedule snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::TargetManifestsPerWorkspace,
            as_u64(snapshot.target_store.manifests.len()),
            "target snapshot replay preflight",
        )?;
        BudgetCheck::against(
            &ResourceLimits::hard_safety_caps(),
            ResourceKind::SchedulePlansPerWorkspace,
            as_u64(snapshot.schedule_store.plans.len()),
            "schedule snapshot replay preflight",
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

        replayed.replay_optimization_state(
            &snapshot.candidate_forest,
            &snapshot.equality_store,
            &snapshot.memory_store,
            &snapshot.target_store,
            &snapshot.schedule_store,
            &snapshot.backend_store,
            &snapshot.artifact_store,
            &snapshot.measurement_store,
        )?;
        replayed.validate_target_budgets(&snapshot.target_store)?;
        replayed.validate_schedule_budgets(&snapshot.schedule_store)?;

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
            equality_spaces_verified: snapshot.equality_store.spaces.len(),
            equality_events_replayed: snapshot.equality_store.events.len(),
            memory_plans_verified: snapshot.memory_store.plans.len(),
            memory_events_replayed: snapshot.memory_store.events.len(),
            target_manifests_verified: snapshot.target_store.manifests.len(),
            target_events_replayed: snapshot.target_store.events.len(),
            schedule_plans_verified: snapshot.schedule_store.plans.len(),
            schedule_events_replayed: snapshot.schedule_store.events.len(),
            backend_plans_verified: snapshot.backend_store.plans.len(),
            backend_events_replayed: snapshot.backend_store.events.len(),
            artifacts_verified: snapshot.artifact_store.packages.len(),
            artifact_events_replayed: snapshot.artifact_store.events.len(),
            measurements_verified: snapshot.measurement_store.records.len(),
            measurement_events_replayed: snapshot.measurement_store.events.len(),
        };
        replayed.revisions = snapshot.revisions;
        replayed.head = snapshot.head;
        replayed.allocator = snapshot.allocator;
        replayed.events = snapshot.events;
        replayed.candidates = snapshot.candidate_forest;
        replayed.equality = snapshot.equality_store;
        replayed.memory = snapshot.memory_store;
        replayed.targets = snapshot.target_store;
        replayed.schedules = snapshot.schedule_store;
        replayed.backends = snapshot.backend_store;
        replayed.artifacts = snapshot.artifact_store;
        replayed.measurements = snapshot.measurement_store;
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
